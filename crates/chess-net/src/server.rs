//! The self-hosted server (NET-03, `chess-server`) from the desktop app:
//! signing in, the databases one may read there, and a copy of one in a
//! local database, kept up to date.
//!
//! Signing in takes a username and password once, for a browser-style
//! session that makes an API token for this computer and then ends; the
//! app keeps the token (in the OS's credential store) and sends it from
//! then on.
//!
//! A copy takes the games by their numbers on the server: each update asks
//! for those from where the last one got to (`export.pgn?idFrom=&idTo=`), a
//! part at a time, at the numbering it knows (`idEpoch`). Each part goes
//! into the database whole before the next is asked for, and the caller
//! notes how far it got after each one, so a Cancel or a failure keeps the
//! parts already in and the next update goes on from there. Games changed
//! or deleted on the server since they came over don't follow (yet). A
//! compaction there numbers the games again: the copy can't go on then, and
//! is made again.

use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use chess_db::{Database, ImportSummary, Progress, import_reader};
use serde::Deserialize;
use ureq::http::Response;
use ureq::{Body, RequestBuilder, typestate};

use crate::http::io_reason;
use crate::{Client, NetError, Token};

/// Games asked for at once: a part of a copy, held in memory while it goes
/// into the database (a few MB of PGN).
pub const PART_GAMES: u32 = 2000;
/// The largest part answer read, and the largest JSON answer.
const PART_LIMIT: u64 = 512 << 20;
const JSON_LIMIT: u64 = 16 << 20;
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
    match (code, detail) {
        (401, _) => NetError::ServerSignInRejected,
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
/// at the server's numbering then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyMark {
    pub next: u32,
    pub epoch: u64,
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
    /// The games that went in, and those that couldn't be read.
    pub import: ImportSummary,
    /// Where it got to (the last part taken whole).
    pub reached: CopyMark,
    /// Why it ended before the last game, when it did; the parts before are
    /// in the database.
    pub stopped: Option<NetError>,
    pub cancelled: bool,
}

/// A part of a copy's progress: the games before it, and its own share.
struct PartProgress<'a> {
    inner: &'a dyn Progress,
    before: u64,
    games: u64,
    total: u64,
}

impl Progress for PartProgress<'_> {
    fn report(&self, done: u64, total: u64) {
        let share = (done.min(total) * self.games).checked_div(total).unwrap_or(0);
        self.inner.report(self.before + share, self.total);
    }
    fn is_cancelled(&self) -> bool {
        // A part goes in whole: a Cancel takes effect between parts.
        false
    }
}

fn add(sum: &mut ImportSummary, part: ImportSummary) {
    sum.imported += part.imported;
    sum.skipped += part.skipped;
    sum.warning_count += part.warning_count;
    let room = 200usize.saturating_sub(sum.warnings.len());
    sum.warnings.extend(part.warnings.into_iter().take(room));
}

/// Brings a copy of the server's database `id` up to date in `db`: the
/// games numbered from `from` on (all of them for a new copy: `None`), a
/// part of [`PART_GAMES`] at a time, each one into the database whole.
/// `noted` hears where it got to after each part, to keep it for the next
/// update. Errors before the first part (no connection, the database gone,
/// compacted since `from`) come back as errors; later ones in `stopped`.
pub fn copy_into(
    client: &Client,
    source: CopySource<'_>,
    from: Option<CopyMark>,
    db: &Mutex<Database>,
    progress: &dyn Progress,
    noted: &mut dyn FnMut(CopyMark),
) -> Result<ServerCopy, NetError> {
    let CopySource { url, token, id } = source;
    let info = database(client, url, token, id)?;
    if from.is_some_and(|m| m.epoch != info.id_epoch) {
        return Err(NetError::ServerCompacted);
    }
    let mut mark = from.unwrap_or(CopyMark { next: 0, epoch: info.id_epoch });
    let total = u64::from(info.games.saturating_sub(mark.next)).max(1);
    let (start, mut import, mut stopped) = (mark.next, ImportSummary::default(), None);
    let path = format!("{url}/api/databases/{}/export.pgn", crate::http::encode(id));
    loop {
        if progress.is_cancelled() {
            break;
        }
        let to = mark.next.saturating_add(PART_GAMES);
        let asked = client
            .agent
            .get(&path)
            .header("Accept", "application/x-chess-pgn")
            .header("Authorization", token.bearer())
            .query("idFrom", mark.next.to_string())
            .query("idTo", to.to_string())
            .query("idEpoch", mark.epoch.to_string())
            .config()
            .timeout_global(Some(PART_TIMEOUT))
            .build()
            .call();
        let part = asked.map_err(|e| unreachable(url, &e)).and_then(|mut resp| {
            if !resp.status().is_success() {
                return Err(match resp.status().as_u16() {
                    404 => NetError::NoSuchServerDb,
                    _ => failure(url, resp),
                });
            }
            let header =
                |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
            let (Some(next), Some(epoch)) = (header("x-next-id"), header("x-id-epoch")) else {
                return Err(bad_data(url, "no X-Next-Id: the server is older than the app"));
            };
            let mut pgn = Vec::new();
            let mut reader = resp.body_mut().with_config().limit(PART_LIMIT).reader();
            reader
                .read_to_end(&mut pgn)
                .map_err(|e| NetError::ServerUnreachable { server: url.to_owned(), reason: io_reason(&e) })?;
            let next = u32::try_from(next).map_err(|e| bad_data(url, e))?;
            Ok((CopyMark { next, epoch }, pgn))
        });
        let (reached, pgn) = match part {
            Ok(p) => p,
            Err(e) if mark.next == start => return Err(e),
            Err(e) => {
                stopped = Some(e);
                break;
            }
        };
        if reached.epoch != mark.epoch {
            // Compacted between two parts: what came is in, the rest can't be told apart.
            stopped = Some(NetError::ServerCompacted);
            break;
        }
        let before = u64::from(mark.next - start);
        let games = u64::from(reached.next.saturating_sub(mark.next));
        let part_progress = PartProgress { inner: progress, before, games, total };
        let len = pgn.len() as u64;
        let read = std::cell::Cell::new(0u64);
        let counted = Counting { inner: pgn.as_slice(), read: &read };
        add(&mut import, import_reader(db, counted, len, &|| read.get(), &part_progress)?);
        let done = reached.next <= mark.next || reached.next < to;
        mark = reached;
        noted(mark);
        progress.report(u64::from(mark.next - start).min(total), total);
        if done {
            break;
        }
    }
    let cancelled = progress.is_cancelled() && stopped.is_none();
    Ok(ServerCopy { db: info, import, reached: mark, stopped, cancelled })
}

/// A reader that counts what it hands over, for the progress.
struct Counting<'a, R> {
    inner: R,
    read: &'a std::cell::Cell<u64>,
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
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
