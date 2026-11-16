//! The self-hosted server (NET-03, `chess-server`) from the desktop app:
//! signing in, the databases one may read there, and a copy of one in a
//! local database, kept up to date.
//!
//! Signing in takes a username and password once, for a browser-style
//! session that makes an API token for this computer and then ends; the
//! app keeps the token (in the OS's credential store) and sends it from
//! then on.
//!
//! A copy (an offline copy of a server database) keeps each game's
//! permanent id there as the local game's origin (`chess_db::GameUid`,
//! `Database::games_by_origin`). Each update first takes the games changed
//! there since the change of the server's journal it's at (edits,
//! deletions, restores: `sync/changes`), then the games added since, by
//! their numbers there (`sync/games?idFrom=&idTo=&idEpoch=`), a part at a
//! time. Each step goes into the database whole before the next is asked
//! for, and the caller notes how far it got after each one, so a Cancel or
//! a failure keeps what came and the next update goes on from there; a
//! part taken again (after a crash) skips the games the copy has. A
//! compaction there numbers the games again: the copy finds where it got
//! to by the permanent ids of its last games (`sync/locate`).
//!
//! Games edited in the copy go back with [`push_edits`], each refused by
//! the server if it changed there since: then the edit is kept as a new
//! game in the copy, and the copy's game takes the server's version.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use chess_core::game::Game;
use chess_core::pgn::{read_pgn_str, write_game};
use chess_db::{Database, GameId, GameUid, ImportSummary, PreparedGame, Progress};
use serde::Deserialize;
use ureq::http::Response;
use ureq::{Body, RequestBuilder, typestate};

use crate::http::io_reason;
use crate::{Client, NetError, Token};

/// Games asked for at once: a part of a copy, held in memory while it goes
/// into the database (a few MB of PGN).
pub const PART_GAMES: u32 = 2000;
/// The largest JSON answer read (a part of a copy is one).
const JSON_LIMIT: u64 = 512 << 20;
/// The longest a request may take (a part of a copy included).
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(30);
const PART_TIMEOUT: Duration = Duration::from_secs(600);

/// The server's address as typed or pasted: `https://` when no scheme is
/// given; `http://` only when typed (a server at home, without HTTPS).
/// Trailing slashes go, so paths can follow.
pub fn server_url(text: &str) -> Result<String, NetError> {
    let text = text.trim();
    let bad = || NetError::NotAServerAddress(text.to_owned());
    let (scheme, rest) = match text.split_once("://") {
        Some((s, rest)) if s.eq_ignore_ascii_case("https") => ("https", rest),
        Some((s, rest)) if s.eq_ignore_ascii_case("http") => ("http", rest),
        Some(_) => return Err(bad()),
        None => ("https", text),
    };
    let rest = rest.trim_end_matches('/');
    let host = rest.split('/').next().unwrap_or("");
    let fine = |c: char| c.is_ascii_alphanumeric() || "-._~:[]%/".contains(c);
    if host.is_empty() || host.starts_with(':') || !rest.chars().all(fine) || rest.contains("//") {
        return Err(bad());
    }
    Ok(format!("{scheme}://{rest}"))
}

/// A sign-in to a server: the API token made for this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSignIn {
    /// The name as the server writes it.
    pub username: String,
    pub token: Token,
    /// The token's id there, to revoke it when signing out.
    pub token_id: i64,
}

/// A database one may read on the server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDb {
    /// Opaque.
    pub id: String,
    pub name: String,
    pub owner: String,
    /// `read`, `write` or `owner`.
    pub access: String,
    pub games: u32,
    /// Bumped by each compaction there, which numbers the games again.
    pub id_epoch: u64,
    /// The latest change in its journal of changed games: only in one
    /// database's info, from servers that keep one.
    #[serde(default)]
    pub latest_change: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Me {
    username: String,
    #[serde(default)]
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct NewToken {
    id: i64,
    token: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

/// The server's own error: its code and message (`{"error": …}`), if the
/// answer has one.
fn error_of(resp: &mut Response<Body>) -> Option<ErrorDetail> {
    let mut bytes = Vec::new();
    resp.body_mut().with_config().limit(64 << 10).reader().read_to_end(&mut bytes).ok()?;
    serde_json::from_slice::<ErrorBody>(&bytes).ok().map(|b| b.error)
}

/// An answer that isn't a success, as an error: a token it no longer takes,
/// a database that's gone, a compaction since, or its own message.
fn failure(url: &str, mut resp: Response<Body>) -> NetError {
    let code = resp.status().as_u16();
    let detail = error_of(&mut resp);
    failure_of(url, code, detail)
}

/// [`failure`] from the answer's status and error, once read.
fn failure_of(url: &str, code: u16, detail: Option<ErrorDetail>) -> NetError {
    match (code, detail) {
        // Only the server's own answer is its turning the token down: a
        // proxy's 401, or one after a redirect that dropped the token, isn't.
        (401, Some(_)) => NetError::ServerSignInRejected,
        (401, None) => NetError::ServerError {
            server: url.to_owned(),
            code,
            message: "it asked for a login, but isn't the chess server's own answer (a proxy, or an address that redirects?)".to_owned(),
        },
        (409, Some(d)) if d.code == "staleIds" => NetError::ServerCompacted,
        (_, Some(d)) => NetError::ServerError { server: url.to_owned(), code, message: d.message },
        (_, None) => NetError::ServerError { server: url.to_owned(), code, message: String::new() },
    }
}

fn unreachable(url: &str, e: &ureq::Error) -> NetError {
    let reason = match e {
        ureq::Error::Io(e) => io_reason(e),
        ureq::Error::Timeout(_) => "no answer in time".to_owned(),
        ureq::Error::HostNotFound => "its address can't be found".to_owned(),
        ureq::Error::ConnectionFailed => "the connection failed".to_owned(),
        e => e.to_string(),
    };
    NetError::ServerUnreachable { server: url.to_owned(), reason }
}

fn bad_data(url: &str, reason: impl ToString) -> NetError {
    NetError::ServerBadData { server: url.to_owned(), reason: reason.to_string() }
}

fn read_json<T: serde::de::DeserializeOwned>(url: &str, resp: &mut Response<Body>) -> Result<T, NetError> {
    let mut bytes = Vec::new();
    let mut reader = resp.body_mut().with_config().limit(JSON_LIMIT).reader();
    reader.read_to_end(&mut bytes).map_err(|e| bad_data(url, io_reason(&e)))?;
    serde_json::from_slice(&bytes).map_err(|e| bad_data(url, e))
}

impl Client {
    fn server_get(&self, url: &str, path: &str, token: &Token) -> RequestBuilder<typestate::WithoutBody> {
        self.agent
            .get(format!("{url}{path}"))
            .header("Accept", "application/json")
            .header("Authorization", token.bearer())
            .config()
            .timeout_global(Some(LOOKUP_TIMEOUT))
            .build()
    }
}

/// Signs in with a username and password: a session that makes an API
/// token named `token_name` for this computer, then ends. A wrong password
/// is [`NetError::ServerLoginRefused`] with the server's words.
pub fn sign_in(
    client: &Client,
    url: &str,
    username: &str,
    password: &str,
    token_name: &str,
) -> Result<ServerSignIn, NetError> {
    let post = |path: &str| {
        client
            .agent
            .post(format!("{url}{path}"))
            .header("Accept", "application/json")
            .config()
            .timeout_global(Some(LOOKUP_TIMEOUT))
            .build()
    };
    let json = |v: serde_json::Value| v.to_string();
    let body = json(serde_json::json!({ "username": username, "password": password }));
    let mut resp =
        post("/api/login").content_type("application/json").send(body).map_err(|e| unreachable(url, &e))?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        return Err(match (code, error_of(&mut resp)) {
            (401 | 403 | 429, Some(d)) => NetError::ServerLoginRefused(d.message),
            (404, _) => NetError::NotAChessServer(url.to_owned()),
            (_, d) => NetError::ServerError {
                server: url.to_owned(),
                code,
                message: d.map(|d| d.message).unwrap_or_default(),
            },
        });
    }
    let cookie = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .find(|v| v.trim_start().starts_with("chessdb_session="))
        .map(|v| v.trim().to_owned())
        .ok_or_else(|| NetError::NotAChessServer(url.to_owned()))?;
    let me: Me = read_json(url, &mut resp).map_err(|_| NetError::NotAChessServer(url.to_owned()))?;
    let csrf = me.csrf_token.unwrap_or_default();
    let session = |req: RequestBuilder<typestate::WithBody>| {
        req.header("Cookie", &cookie).header("X-CSRF-Token", &csrf)
    };
    let made = session(post("/api/tokens"))
        .content_type("application/json")
        .send(json(serde_json::json!({ "name": token_name })));
    // The session ends whatever happened; the token outlives it.
    let _ = session(post("/api/logout")).send_empty();
    let mut resp = made.map_err(|e| unreachable(url, &e))?;
    if !resp.status().is_success() {
        return Err(failure(url, resp));
    }
    let made: NewToken = read_json(url, &mut resp)?;
    Ok(ServerSignIn { username: me.username, token: Token::new(&made.token), token_id: made.id })
}

/// Who `token` signs in as; [`NetError::ServerSignInRejected`] when the
/// server no longer takes it.
pub fn whoami(client: &Client, url: &str, token: &Token) -> Result<String, NetError> {
    let mut resp = client.server_get(url, "/api/me", token).call().map_err(|e| unreachable(url, &e))?;
    if !resp.status().is_success() {
        return Err(failure(url, resp));
    }
    Ok(read_json::<Me>(url, &mut resp)?.username)
}

/// Revokes this computer's token (signing out). A token the server no
/// longer takes is gone already.
pub fn sign_out(client: &Client, url: &str, token: &Token, token_id: i64) -> Result<(), NetError> {
    let resp = client
        .agent
        .delete(format!("{url}/api/tokens/{token_id}"))
        .header("Authorization", token.bearer())
        .config()
        .timeout_global(Some(LOOKUP_TIMEOUT))
        .build()
        .call()
        .map_err(|e| unreachable(url, &e))?;
    match resp.status().as_u16() {
        200..300 | 401 | 404 => Ok(()),
        _ => Err(failure(url, resp)),
    }
}

/// The databases the account may read, as the server lists them.
pub fn databases(client: &Client, url: &str, token: &Token) -> Result<Vec<ServerDb>, NetError> {
    let mut resp =
        client.server_get(url, "/api/databases", token).call().map_err(|e| unreachable(url, &e))?;
    if !resp.status().is_success() {
        return Err(failure(url, resp));
    }
    read_json(url, &mut resp)
}

/// One database's info; [`NetError::NoSuchServerDb`] when it's gone or no
/// longer readable for the account (the server answers 404 to both).
pub fn database(client: &Client, url: &str, token: &Token, id: &str) -> Result<ServerDb, NetError> {
    let path = format!("/api/databases/{}", crate::http::encode(id));
    let mut resp = client.server_get(url, &path, token).call().map_err(|e| unreachable(url, &e))?;
    match resp.status().as_u16() {
        200..300 => read_json(url, &mut resp),
        404 => Err(NetError::NoSuchServerDb),
        _ => Err(failure(url, resp)),
    }
}

/// Where a copy's update got to: the first game number it hasn't taken,
/// at the server's numbering then, and the change of the server's journal
/// of changed games it has (`None` for a copy made before there was one:
/// it follows the changes from its next update on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyMark {
    pub next: u32,
    pub epoch: u64,
    pub seq: Option<u64>,
}

/// The database a copy is of: the server, the sign-in, and its id there.
#[derive(Debug, Clone, Copy)]
pub struct CopySource<'a> {
    pub url: &'a str,
    pub token: &'a Token,
    pub id: &'a str,
}

/// What [`copy_into`] did.
#[derive(Debug)]
pub struct ServerCopy {
    /// The database's info on the server.
    pub db: ServerDb,
    /// The new games that went in, and those that couldn't be read.
    pub import: ImportSummary,
    /// Games changed there (edited, deleted, restored, or removed by a
    /// compaction) that changed here.
    pub changed: u64,
    /// Games changed there that were left as they are here: edited here
    /// and not sent yet (`keep_local`).
    pub kept: u64,
    /// Where it got to.
    pub reached: CopyMark,
    /// Why it ended before the last game, when it did; what came before is
    /// in the database.
    pub stopped: Option<NetError>,
    pub cancelled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncGame {
    uid: String,
    deleted: bool,
    pgn: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncPart {
    epoch: u64,
    next: u32,
    seq: u64,
    games: Vec<SyncGame>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncChange {
    uid: String,
    game: Option<SyncGame>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Changes {
    latest: u64,
    more: bool,
    games: Vec<SyncChange>,
}

#[derive(Deserialize)]
struct Located {
    epoch: u64,
    ids: Vec<Option<u32>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OneGame {
    pgn: String,
    row: OneRow,
}

#[derive(Deserialize)]
struct OneRow {
    #[serde(default)]
    deleted: bool,
}

/// Changed games asked for at once.
const CHANGES_AT_ONCE: u32 = 500;
/// The last games of a copy looked for after a compaction there.
const LOCATE_AT_ONCE: usize = 1000;

fn uid_of(url: &str, text: &str) -> Result<GameUid, NetError> {
    text.parse().map_err(|_| bad_data(url, format!("{text:?} isn't a game id")))
}

/// The one game a PGN the server sent holds.
fn game_of(url: &str, pgn: &str) -> Result<Game, NetError> {
    read_pgn_str(pgn).into_iter().next().ok_or_else(|| bad_data(url, "a game that can't be read"))
}

impl Client {
    fn server_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        req: RequestBuilder<typestate::WithoutBody>,
        timeout: Duration,
    ) -> Result<T, NetError> {
        let mut resp = req
            .header("Accept", "application/json")
            .config()
            .timeout_global(Some(timeout))
            .build()
            .call()
            .map_err(|e| unreachable(url, &e))?;
        if !resp.status().is_success() {
            return Err(match (resp.status().as_u16(), error_of(&mut resp)) {
                (404, Some(d)) if d.code == "notFound" => NetError::NoSuchServerDb,
                (code, detail) => failure_of(url, code, detail),
            });
        }
        read_json(url, &mut resp)
    }
}

/// The local games of a copy, by their permanent ids on the server; built
/// once an update needs it.
struct Local<'a> {
    db: &'a Mutex<Database>,
    map: Option<HashMap<GameUid, GameId>>,
}

impl Local<'_> {
    fn map(&mut self) -> Result<&mut HashMap<GameUid, GameId>, NetError> {
        if self.map.is_none() {
            self.map = Some(self.db.lock().expect("db lock").games_by_origin()?);
        }
        Ok(self.map.as_mut().expect("just built"))
    }
}

/// Makes local game `slot` what the server sent; whether it changed.
fn apply(db: &mut Database, slot: GameId, url: &str, game: Option<&SyncGame>) -> Result<bool, NetError> {
    let deleted_here = db.record(slot)?.is_deleted();
    let Some(g) = game else {
        // Removed by a compaction there: it was deleted first.
        if !deleted_here {
            db.set_deleted(slot, true)?;
        }
        return Ok(!deleted_here);
    };
    let mut changed = false;
    if write_game(&db.read_game(slot)?) != g.pgn {
        db.replace(slot, &game_of(url, &g.pgn)?)?;
        changed = true;
    }
    if deleted_here != g.deleted {
        db.set_deleted(slot, g.deleted)?;
        changed = true;
    }
    Ok(changed)
}

/// Brings a copy of the server's database `id` up to date in `db`. First
/// the games changed there since the copy's mark (edited, deleted,
/// restored: each into its local game, found by its permanent id there,
/// unless `keep_local` says it was edited here and not sent yet), then the
/// games added since, numbered from the mark's `next` (all of them for a
/// new copy: `None`), a part of [`PART_GAMES`] at a time, each one into the
/// database whole with its permanent id there. A game the copy has already
/// (a part taken again after a crash) is left out. After a compaction
/// there, which numbers the games again, the copy finds where it got to by
/// the permanent ids of its last games. `noted` hears where it got to after
/// each step, to keep it for the next update. Errors before the first
/// step (no connection, the database gone) come back as errors; later
/// ones in `stopped`.
pub fn copy_into(
    client: &Client,
    source: CopySource<'_>,
    from: Option<CopyMark>,
    db: &Mutex<Database>,
    progress: &dyn Progress,
    keep_local: &dyn Fn(GameUid) -> bool,
    noted: &mut dyn FnMut(CopyMark),
) -> Result<ServerCopy, NetError> {
    let CopySource { url, token, id } = source;
    let info = database(client, url, token, id)?;
    let Some(latest) = info.latest_change else {
        return Err(bad_data(
            url,
            "no journal of changed games: the server is older than the app; update it",
        ));
    };
    let base = format!("{url}/api/databases/{}", crate::http::encode(id));
    let mut local = Local { db, map: None };
    let mut out = ServerCopy {
        db: info.clone(),
        import: ImportSummary::default(),
        changed: 0,
        kept: 0,
        reached: from.unwrap_or(CopyMark { next: 0, epoch: info.id_epoch, seq: None }),
        stopped: None,
        cancelled: false,
    };
    let fresh = from.is_none();
    let first = std::cell::Cell::new(true);
    // An error in the first step is the update's; after it, what came stays.
    let step = |out: &mut ServerCopy, r: Result<(), NetError>| -> Result<bool, NetError> {
        match r {
            Ok(()) => {
                first.set(false);
                Ok(true)
            }
            Err(NetError::Db(e)) => Err(NetError::Db(e)),
            Err(e) if first.get() => Err(e),
            Err(e) => {
                out.stopped = Some(e);
                Ok(false)
            }
        }
    };

    // 1. The games changed there.
    match out.reached.seq {
        Some(_) => {
            let r =
                changes_into(client, url, token, &base, &mut local, &mut out, progress, keep_local, noted);
            if !step(&mut out, r)? {
                return Ok(out);
            }
        }
        // A copy from before the journal: from now on.
        None if !fresh => {
            out.reached.seq = Some(latest);
            noted(out.reached);
        }
        None => {}
    }

    // 2. After a compaction there: where the copy got to, in its numbering.
    if out.reached.epoch != info.id_epoch && !fresh {
        let r = relocate(client, url, token, &base, &mut local, &mut out.reached);
        // Nowhere to go on from: the copy is made again, whatever came before.
        if let Err(NetError::ServerCompacted) = r {
            return Err(NetError::ServerCompacted);
        }
        if !step(&mut out, r)? {
            return Ok(out);
        }
        noted(out.reached);
    }

    // 3. The games added since.
    let total = u64::from(info.games.saturating_sub(out.reached.next)).max(1);
    let start = out.reached.next;
    loop {
        if progress.is_cancelled() {
            out.cancelled = out.stopped.is_none();
            break;
        }
        let mark = out.reached;
        let to = mark.next.saturating_add(PART_GAMES);
        let req = client
            .agent
            .get(format!("{base}/sync/games"))
            .header("Authorization", token.bearer())
            .query("idFrom", mark.next.to_string())
            .query("idTo", to.to_string())
            .query("idEpoch", mark.epoch.to_string());
        let part = client.server_json::<SyncPart>(url, req, PART_TIMEOUT).and_then(|p| {
            // Compacted between two parts: the next update finds its place.
            if p.epoch != mark.epoch { Err(NetError::ServerCompactedMeanwhile) } else { Ok(p) }
        });
        let part = part.map_err(|e| match e {
            NetError::ServerCompacted => NetError::ServerCompactedMeanwhile,
            e => e,
        });
        let part = match part {
            Ok(p) => p,
            Err(e) => {
                step(&mut out, Err(e))?;
                break;
            }
        };
        first.set(false);
        let have = local.map()?;
        let mut batch = Vec::with_capacity(part.games.len());
        let mut deleted = Vec::new();
        for g in &part.games {
            let uid = uid_of(url, &g.uid)?;
            if have.contains_key(&uid) {
                continue;
            }
            match read_pgn_str(&g.pgn).into_iter().next() {
                Some(game) => {
                    deleted.push(g.deleted);
                    batch.push((uid, PreparedGame::new(&game).with_origin(uid)));
                }
                None => out.import.skipped += 1,
            }
        }
        if !batch.is_empty() {
            let prepared: Vec<_> = batch.iter().map(|(_, p)| p.clone()).collect();
            let mut d = db.lock().expect("db lock");
            let slots = d.append_prepared(&prepared)?;
            let marked: Vec<GameId> =
                slots.clone().zip(&deleted).filter(|(_, del)| **del).map(|(s, _)| s).collect();
            d.set_deleted_many(&marked, true)?;
            drop(d);
            for ((uid, _), slot) in batch.iter().zip(slots) {
                have.insert(*uid, slot);
            }
            out.import.imported += batch.len() as u64;
        }
        let done = part.next <= mark.next || part.next < to;
        out.reached.next = part.next;
        // A new copy follows the changes from its first part on: those
        // after are in the journal after it.
        out.reached.seq.get_or_insert(part.seq);
        noted(out.reached);
        progress.report(u64::from(out.reached.next - start).min(total), total);
        if done {
            break;
        }
    }
    Ok(out)
}

/// The games changed there since the mark, into their local games.
#[allow(clippy::too_many_arguments)]
fn changes_into(
    client: &Client,
    url: &str,
    token: &Token,
    base: &str,
    local: &mut Local<'_>,
    out: &mut ServerCopy,
    progress: &dyn Progress,
    keep_local: &dyn Fn(GameUid) -> bool,
    noted: &mut dyn FnMut(CopyMark),
) -> Result<(), NetError> {
    loop {
        let since = out.reached.seq.unwrap_or(0);
        let req = client
            .agent
            .get(format!("{base}/sync/changes"))
            .header("Authorization", token.bearer())
            .query("since", since.to_string())
            .query("limit", CHANGES_AT_ONCE.to_string());
        let changes: Changes = client.server_json(url, req, LOOKUP_TIMEOUT)?;
        if !changes.games.is_empty() {
            let db = local.db;
            let map = local.map()?;
            // Which to apply, asked of the caller before the database is
            // locked (`keep_local` may take locks of its own).
            let mut todo = Vec::with_capacity(changes.games.len());
            for c in &changes.games {
                let uid = uid_of(url, &c.uid)?;
                // Not copied yet (it comes with its part), or from before.
                let Some(&slot) = map.get(&uid) else { continue };
                if keep_local(uid) {
                    out.kept += 1;
                } else {
                    todo.push((slot, c));
                }
            }
            let mut db = db.lock().expect("db lock");
            for (slot, c) in todo {
                if apply(&mut db, slot, url, c.game.as_ref())? {
                    out.changed += 1;
                }
            }
        }
        out.reached.seq = Some(changes.latest.max(since));
        noted(out.reached);
        if !changes.more || progress.is_cancelled() {
            return Ok(());
        }
    }
}

/// After a compaction there: the first number after the last games the
/// copy took, in the new numbering. [`NetError::ServerCompacted`] when
/// none of them is there any more.
fn relocate(
    client: &Client,
    url: &str,
    token: &Token,
    base: &str,
    local: &mut Local<'_>,
    mark: &mut CopyMark,
) -> Result<(), NetError> {
    let mut last: Vec<(GameId, GameUid)> = local.map()?.iter().map(|(u, s)| (*s, *u)).collect();
    last.sort_unstable_by(|a, b| b.cmp(a));
    last.truncate(LOCATE_AT_ONCE);
    if last.is_empty() {
        return if mark.next == 0 { Ok(()) } else { Err(NetError::ServerCompacted) };
    }
    let uids: Vec<String> = last.iter().map(|(_, u)| u.to_string()).collect();
    let body = serde_json::json!({ "uids": uids }).to_string();
    let mut resp = client
        .agent
        .post(format!("{base}/sync/locate"))
        .header("Accept", "application/json")
        .header("Authorization", token.bearer())
        .content_type("application/json")
        .config()
        .timeout_global(Some(LOOKUP_TIMEOUT))
        .build()
        .send(body)
        .map_err(|e| unreachable(url, &e))?;
    if !resp.status().is_success() {
        return Err(match resp.status().as_u16() {
            404 => NetError::NoSuchServerDb,
            _ => failure(url, resp),
        });
    }
    let found: Located = read_json(url, &mut resp)?;
    let Some(max) = found.ids.iter().flatten().max() else { return Err(NetError::ServerCompacted) };
    *mark = CopyMark { next: max + 1, epoch: found.epoch, seq: mark.seq };
    Ok(())
}

/// A game of a copy edited here, to send to the server: its permanent id
/// there, and the journal's change the copy was at when it was edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalEdit {
    pub uid: GameUid,
    pub base_seq: u64,
}

/// What [`push_edits`] did.
#[derive(Debug, Default)]
pub struct Pushed {
    /// The edits dealt with, sent or not: no longer to send.
    pub done: Vec<GameUid>,
    /// Sent to the server.
    pub sent: u64,
    /// Not sent: the game changed on the server too, the account may only
    /// read the database, or the game is gone there. Each edit was kept as
    /// a new game in the copy (not on the server), and the copy's game took
    /// the server's version.
    pub kept_as_new: u64,
    /// Why it stopped before the last edit, when it did.
    pub stopped: Option<NetError>,
}

/// Sends the games of a copy edited here to the server (moves, headers and
/// the deleted flag), each refused there if it changed on the server after
/// the edit's `base_seq`. A refused edit, or one the account can't make,
/// isn't lost: it's added to the copy as a new game, and the copy's game
/// takes the server's version.
pub fn push_edits(
    client: &Client,
    source: CopySource<'_>,
    db: &Mutex<Database>,
    edits: &[LocalEdit],
) -> Result<Pushed, NetError> {
    let CopySource { url, token, id } = source;
    let base = format!("{url}/api/databases/{}", crate::http::encode(id));
    let mut out = Pushed::default();
    if edits.is_empty() {
        return Ok(out);
    }
    let map = db.lock().expect("db lock").games_by_origin()?;
    for edit in edits {
        let Some(&slot) = map.get(&edit.uid) else {
            // Not in the copy any more (compacted away here).
            out.done.push(edit.uid);
            continue;
        };
        let (game, deleted) = {
            let d = db.lock().expect("db lock");
            (d.read_game(slot)?, d.record(slot)?.is_deleted())
        };
        let by_uid = format!("{base}/by-uid/{}", edit.uid);
        let body =
            serde_json::json!({ "pgn": write_game(&game), "deleted": deleted, "baseSeq": edit.base_seq });
        let sent = client
            .agent
            .put(&by_uid)
            .header("Accept", "application/json")
            .header("Authorization", token.bearer())
            .content_type("application/json")
            .config()
            .timeout_global(Some(LOOKUP_TIMEOUT))
            .build()
            .send(body.to_string());
        let mut resp = match sent {
            Ok(r) => r,
            Err(e) => {
                out.stopped = Some(unreachable(url, &e));
                return Ok(out);
            }
        };
        let code = resp.status().as_u16();
        if resp.status().is_success() {
            out.sent += 1;
            out.done.push(edit.uid);
            continue;
        }
        let detail = error_of(&mut resp);
        let gone = match (code, detail.as_ref().map(|d| d.code.as_str())) {
            (409, Some("changedOnServer")) | (403, _) => false,
            (404, Some("noSuchGame")) => true,
            (404, _) => {
                out.stopped = Some(NetError::NoSuchServerDb);
                return Ok(out);
            }
            (401, Some(_)) => {
                out.stopped = Some(NetError::ServerSignInRejected);
                return Ok(out);
            }
            _ => {
                let message = detail.map(|d| d.message).unwrap_or_default();
                out.stopped = Some(NetError::ServerError { server: url.to_owned(), code, message });
                return Ok(out);
            }
        };
        // The edit, kept as a new game here; then the server's version.
        db.lock().expect("db lock").append(&game)?;
        out.kept_as_new += 1;
        if gone {
            db.lock().expect("db lock").set_deleted(slot, true)?;
        } else {
            let req = client.agent.get(&by_uid).header("Authorization", token.bearer());
            match client.server_json::<OneGame>(url, req, LOOKUP_TIMEOUT) {
                Ok(there) => {
                    let g =
                        SyncGame { uid: edit.uid.to_string(), deleted: there.row.deleted, pgn: there.pgn };
                    apply(&mut db.lock().expect("db lock"), slot, url, Some(&g))?;
                }
                Err(e) => {
                    out.done.push(edit.uid);
                    out.stopped = Some(e);
                    return Ok(out);
                }
            }
        }
        out.done.push(edit.uid);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_addresses() {
        let ok = |text: &str, want: &str| assert_eq!(server_url(text).unwrap(), want, "{text}");
        ok("chess.example.com", "https://chess.example.com");
        ok("  https://chess.example.com/  ", "https://chess.example.com");
        ok("http://192.168.1.20:8080", "http://192.168.1.20:8080");
        ok("HTTP://localhost:8080/", "http://localhost:8080");
        ok("https://example.com/chess", "https://example.com/chess");
        ok("http://[::1]:8080", "http://[::1]:8080");
        for bad in
            ["", "   ", "ftp://x.org", "https://", "two words.com", "https://a.com//b", "://x", ":8080"]
        {
            assert!(matches!(server_url(bad), Err(NetError::NotAServerAddress(_))), "{bad:?}");
        }
    }
}
