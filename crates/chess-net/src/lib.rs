//! Online game sources (NET-01): a player's games on Lichess or Chess.com,
//! downloaded straight into a database. A game the database has already
//! (told by its page, see [`keys`]) is skipped, so downloading a player's
//! games again only adds the new ones. The summary says how far a download
//! got ([`DownloadSummary::covered`]), so the next one can start there.
//!
//! The games are fetched on a thread of their own and handed over one by
//! one as PGN to `chess_db::import_reader`, so they're committed in batches
//! as they come and a Cancel takes effect at once, even while a server
//! keeps us waiting. No UI dependencies: blocking HTTP through `ureq`.

mod chesscom;
mod http;
pub mod keys;
mod lichess;
pub mod time;

use std::cell::Cell;
use std::collections::HashSet;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chess_db::{Database, DbError, ImportSummary, Progress, import_reader, scan_tags};

pub use http::Client;
use keys::{GameKey, key_of_url, lichess_key};

/// How often a waiting download looks whether it was cancelled.
const POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Site {
    Lichess,
    ChessCom,
}

impl Site {
    pub fn name(self) -> &'static str {
        match self {
            Site::Lichess => "Lichess",
            Site::ChessCom => "Chess.com",
        }
    }
}

/// Time controls, as both sites group them. Lichess's UltraBullet counts as
/// bullet and Chess.com's daily games as correspondence; Chess.com has no
/// classical games (its rapid takes them in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Speed {
    Bullet,
    Blitz,
    Rapid,
    Classical,
    Correspondence,
}

/// Which of the player's games to download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    All,
    /// Games from this time on (ms since the epoch): those started then or
    /// later on Lichess, those that ended then or later on Chess.com.
    Since(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub site: Site,
    pub username: String,
    /// Empty: all of them.
    pub speeds: Vec<Speed>,
    pub rated_only: bool,
    pub period: Period,
}

/// A player as the site knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub site: Site,
    /// The name as the site writes it ("DrNykterstein").
    pub username: String,
    /// When the account was made (ms since the epoch).
    pub joined: i64,
    /// Games played, all kinds (Lichess only).
    pub games: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("\"{0}\" isn't a username: they're letters, digits, - and _")]
    BadUsername(String),
    #[error("{} has no player called {name}. Check the spelling", site.name())]
    NoSuchPlayer { site: Site, name: String },
    #[error("{} has had too many requests from this computer for now. Try again in a few minutes", .0.name())]
    RateLimited(Site),
    #[error("couldn't reach {} ({reason}). Check the internet connection and try again", site.name())]
    Unreachable { site: Site, reason: String },
    #[error("{} answered with an error (HTTP {code}). Try again later", site.name())]
    Status { site: Site, code: u16 },
    #[error("{} sent an answer that can't be read ({reason}). Try again later", site.name())]
    BadData { site: Site, reason: String },
    #[error("{} stopped sending games", .0.name())]
    Stalled(Site),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("cancelled")]
    Cancelled,
}

/// What [`download_into`] did.
#[derive(Debug)]
pub struct DownloadSummary {
    pub player: Player,
    /// The games that went in, and those that couldn't be read.
    pub import: ImportSummary,
    /// Games left out as the database had them already.
    pub already: u64,
    /// Where the download started (`Period::Since`).
    pub since: Option<i64>,
    /// How far the download got: when the newest game the server sent
    /// started (Lichess) or ended (Chess.com), in ms since the epoch; `None`
    /// when it sent none. The games come oldest first and every one sent is
    /// in the database (a cancel ends them between two games), so the next
    /// download of the same games can start there. The server's own times
    /// keep a game ending after that newest one for next time even when
    /// Chess.com's archive didn't have it yet; a game still being played
    /// (Lichess sends none) comes only if it started after the newest one
    /// sent (not a correspondence game started before).
    pub covered: Option<i64>,
    /// Why the download ended before the last game, when it did; the games
    /// before are in the database.
    pub stopped: Option<NetError>,
    pub cancelled: bool,
}

/// What one site is asked for.
pub(crate) struct Fetch {
    pub username: String,
    pub speeds: Vec<Speed>,
    pub rated_only: bool,
    pub since: Option<i64>,
    /// When the last bytes came (ms since the epoch): a download is stalled
    /// when nothing comes for a while, not when a game takes long.
    pub alive: Arc<AtomicI64>,
}

/// What a site's fetcher hands over.
pub(crate) enum Fetched {
    /// A game: its key, when it started (Lichess) or ended (Chess.com), and
    /// its PGN.
    Game { key: GameKey, at: i64, pgn: String },
    /// Everything before this time (ms) has been sent.
    Reached(i64),
}

/// The username, trimmed, if it could be one on either site.
pub fn check_username(name: &str) -> Result<&str, NetError> {
    let name = name.trim();
    let ok = (1..=50).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok { Ok(name) } else { Err(NetError::BadUsername(name.to_owned())) }
}

/// Looks the player up: whether they exist, and their name as the site
/// writes it.
pub fn player(
    client: &Client,
    site: Site,
    username: &str,
    stop: &dyn Fn() -> bool,
) -> Result<Player, NetError> {
    let name = check_username(username)?;
    match site {
        Site::Lichess => lichess::player(client, name, stop),
        Site::ChessCom => chesscom::player(client, name, stop),
    }
}

/// The import's progress: the job's, never cancelled. On a cancel the feed
/// ends the input between two games instead, so the reader takes in the
/// games it read ahead (up to 16 kB), and every game sent is in the
/// database, as `covered` says.
struct Uncancelled<'a>(Part<'a>);

impl Progress for Uncancelled<'_> {
    fn report(&self, done: u64, total: u64) {
        self.0.report(done, total);
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// A share of a job's progress: `from`‰ to `from + span`‰ of it.
struct Part<'a> {
    inner: &'a dyn Progress,
    from: u64,
    span: u64,
}

impl Progress for Part<'_> {
    fn report(&self, done: u64, total: u64) {
        let share = (done.min(total) * self.span).checked_div(total).unwrap_or(0);
        self.inner.report(self.from + share, 1000);
    }
    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

/// Tags that say which online game a game is.
const TAGS: [&str; 2] = ["Link", "GameId"];

/// The keys of the online games in the database (deleted ones too, until
/// it's compacted).
fn known_games(db: &Mutex<Database>, progress: &dyn Progress) -> Result<HashSet<GameKey>, DbError> {
    let mut keys = HashSet::new();
    scan_tags(db, &TAGS, progress, &mut |g| {
        let [link, game_id] = &g.tags[..] else { return };
        let key = link.as_deref().and_then(key_of_url).or_else(|| key_of_url(&g.row.site)).or_else(|| {
            game_id.as_deref().filter(|_| g.row.site.contains("lichess.org")).and_then(lichess_key)
        });
        keys.extend(key);
    })?;
    Ok(keys)
}

/// The downloaded games as one PGN text, for the importer: games the
/// database has are left out, and it ends early (cleanly, between games)
/// when the job is cancelled, the server fails or stops sending.
struct Feed<'a> {
    rx: Receiver<Result<Fetched, NetError>>,
    site: Site,
    /// The games the database had.
    known: HashSet<GameKey>,
    /// The games this download handed over (a resumed stream sends one
    /// again).
    seen: HashSet<GameKey>,
    progress: &'a dyn Progress,
    /// How far the download got (ms), for the progress.
    reached: &'a Cell<i64>,
    /// When the newest game sent started or ended (ms).
    newest: Option<i64>,
    buf: Vec<u8>,
    pos: usize,
    already: u64,
    stopped: Option<NetError>,
    ended: bool,
    /// When anything last came (ms since the epoch), and how long nothing
    /// may.
    alive: Arc<AtomicI64>,
    stall: Duration,
}

impl Feed<'_> {
    /// The next game the database doesn't have, into `buf`; false at the end.
    fn next_game(&mut self) -> bool {
        loop {
            if self.ended || self.progress.is_cancelled() {
                return false;
            }
            match self.rx.recv_timeout(POLL) {
                Ok(Ok(Fetched::Game { key, at, pgn })) => {
                    self.alive.store(time::now_ms(), Ordering::Relaxed);
                    self.reached.set(self.reached.get().max(at));
                    self.newest = self.newest.max(Some(at));
                    // A game sent again (a resumed stream) is skipped and
                    // counted once.
                    if !self.seen.insert(key.clone()) {
                        continue;
                    }
                    if self.known.contains(&key) {
                        self.already += 1;
                        continue;
                    }
                    self.buf.clear();
                    self.buf.extend_from_slice(pgn.trim().as_bytes());
                    self.buf.extend_from_slice(b"\n\n");
                    self.pos = 0;
                    return true;
                }
                Ok(Ok(Fetched::Reached(at))) => {
                    self.alive.store(time::now_ms(), Ordering::Relaxed);
                    self.reached.set(self.reached.get().max(at));
                }
                Ok(Err(e)) => {
                    self.stopped = Some(e);
                    self.ended = true;
                }
                Err(RecvTimeoutError::Timeout) => {
                    let quiet = time::now_ms() - self.alive.load(Ordering::Relaxed);
                    if quiet >= self.stall.as_millis() as i64 {
                        self.stopped = Some(NetError::Stalled(self.site));
                        self.ended = true;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => self.ended = true,
            }
        }
    }
}

impl Read for Feed<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.buf.len() && !self.next_game() {
            return Ok(0);
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Downloads the player's games into `db`, oldest first. The games are
/// committed as they come, so on a cancel or a failure partway those
/// before it stay (see `stopped` and `cancelled` in the summary). Errors
/// before the first game (no such player, no connection) come back as
/// errors.
pub fn download_into(
    client: &Client,
    db: &Mutex<Database>,
    request: &Request,
    progress: &dyn Progress,
) -> Result<DownloadSummary, NetError> {
    let stop = || progress.is_cancelled();
    let player = player(client, request.site, &request.username, &stop)?;
    let known = match known_games(db, &Part { inner: progress, from: 0, span: 50 }) {
        Err(DbError::Cancelled) => return Err(NetError::Cancelled),
        other => other?,
    };
    let since = match request.period {
        Period::All => None,
        Period::Since(t) => Some(t),
    };
    let now = time::now_ms();
    let alive = Arc::new(AtomicI64::new(now));
    let fetch = Fetch {
        username: player.username.clone(),
        speeds: request.speeds.clone(),
        rated_only: request.rated_only,
        since,
        alive: alive.clone(),
    };

    // The fetcher's thread is left to finish on its own: it stops at its
    // next game once nobody takes them, or when a 429 wait sees `stopped`.
    // One stuck reading a server that stopped sending stays until the
    // connection drops (ureq has no timeout between two reads), holding a
    // thread and a socket; nothing waits for it.
    let (tx, rx) = sync_channel(64);
    let stopped = Arc::new(AtomicBool::new(false));
    let (site, thread_client, thread_stop) = (request.site, client.clone(), stopped.clone());
    std::thread::Builder::new()
        .name("download".into())
        .spawn(move || {
            let stop = || thread_stop.load(Ordering::Relaxed);
            let mut send = |f: Fetched| tx.send(Ok(f)).is_ok();
            let result = match site {
                Site::Lichess => lichess::fetch(&thread_client, &fetch, &mut send, &stop),
                Site::ChessCom => chesscom::fetch(&thread_client, &fetch, &mut send, &stop),
            };
            if let Err(e) = result {
                let _ = tx.send(Err(e));
            }
        })
        .map_err(DbError::Io)?;

    // Progress: how much of the time from the start to now is covered.
    let from = since.unwrap_or(player.joined);
    let reached = Cell::new(from);
    let total = ((now - from) / 1000).max(1) as u64;
    let position = || (((reached.get() - from) / 1000).max(0) as u64).min(total);
    let mut feed = Feed {
        rx,
        site: request.site,
        known,
        seen: HashSet::new(),
        progress,
        reached: &reached,
        newest: None,
        buf: Vec::new(),
        pos: 0,
        already: 0,
        stopped: None,
        ended: false,
        alive,
        stall: client.stall,
    };
    let import = import_reader(
        db,
        &mut feed,
        total,
        &position,
        &Uncancelled(Part { inner: progress, from: 50, span: 950 }),
    );
    stopped.store(true, Ordering::Relaxed);
    let import = import?;
    if import.imported == 0
        && import.skipped == 0
        && feed.already == 0
        && let Some(e) = feed.stopped.take()
    {
        // Nothing came before the failure: it's the download's.
        return Err(e);
    }
    let cancelled = progress.is_cancelled();
    let covered = feed.newest;
    Ok(DownloadSummary {
        player,
        import,
        already: feed.already,
        since,
        covered,
        stopped: feed.stopped,
        cancelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames() {
        assert_eq!(check_username("  DrNykterstein ").unwrap(), "DrNykterstein");
        assert_eq!(check_username("a-b_c9").unwrap(), "a-b_c9");
        for bad in ["", "   ", "two words", "../etc", "a?b=c", "名前", &"x".repeat(51)] {
            assert!(matches!(check_username(bad), Err(NetError::BadUsername(_))), "{bad}");
        }
    }
}
