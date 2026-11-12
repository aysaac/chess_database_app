//! Lichess studies: a study's chapters into a database (IO-10), and games
//! out to studies (NET-12, [`export`]).
//!
//! A study comes as one PGN text (`/api/study/{id}.pgn`, or one chapter's),
//! each chapter a game tagged with `ChapterURL`, `StudyName`, `ChapterName`,
//! `Annotator` and `Orientation`. Chapters are told apart by their
//! `ChapterURL` ([`crate::keys::study_chapter_key`]), so importing a study
//! again adds only the chapters the database doesn't have. A study's
//! export says when it last changed (`Last-Modified`), and asking again
//! with `If-Modified-Since` gets a 304 while it hasn't (a chapter's export
//! is always new).

mod export;

pub use export::*;

use std::cell::Cell;
use std::collections::HashSet;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};

use chess_db::{Database, DbError, ImportSummary, Progress, import_reader};
use serde::Deserialize;

use crate::http::{Client, read_bytes, status_error};
use crate::keys::{GameKey, is_lichess_id, key_of_url};
use crate::oauth::Token;
use crate::{NetError, POLL, Part, Site, known_games};

/// The longest study export taken: 64 chapters of a few kB each, or a few
/// hundred kB when annotated at length.
const MAX_STUDY: u64 = 64 << 20;

/// A study, or one chapter of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StudyRef {
    pub study: String,
    pub chapter: Option<String>,
}

impl StudyRef {
    /// Its link on lichess.org, as one pasted in ([`parse_study_link`]
    /// reads it back), whatever server the client talks to.
    pub fn link(&self) -> String {
        match &self.chapter {
            Some(c) => format!("https://lichess.org/study/{}/{c}", self.study),
            None => format!("https://lichess.org/study/{}", self.study),
        }
    }

    /// The study's (or chapter's) page on the server.
    pub fn url(&self, client: &Client) -> String {
        match &self.chapter {
            Some(c) => format!("{}/study/{}/{c}", client.lichess, self.study),
            None => format!("{}/study/{}", client.lichess, self.study),
        }
    }
}

/// A study or chapter link (`https://lichess.org/study/{id}`,
/// `…/study/{id}/{chapter}`, with or without `https://`, with anything after
/// it), or a study's id alone.
pub fn parse_study_link(text: &str) -> Result<StudyRef, NetError> {
    let bad = || NetError::NotAStudyLink(text.trim().to_owned());
    let t = text.trim();
    if is_lichess_id(t) {
        return Ok(StudyRef { study: t.to_owned(), chapter: None });
    }
    let rest = t.split_once("://").map_or(t, |(_, r)| r);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    if host.strip_prefix("www.").unwrap_or(&host) != "lichess.org" {
        return Err(bad());
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let id = |p: &str| {
        let p = p.strip_suffix(".pgn").unwrap_or(p);
        is_lichess_id(p).then(|| p.to_owned())
    };
    let parts = match parts.as_slice() {
        ["api", rest @ ..] => rest,
        other => other,
    };
    match parts {
        ["study", s, rest @ ..] => {
            let study = id(s).ok_or_else(bad)?;
            Ok(StudyRef { study, chapter: rest.first().and_then(|c| id(c)) })
        }
        _ => Err(bad()),
    }
}

/// A study in a user's list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StudyInfo {
    pub id: String,
    pub name: String,
    /// ms since the epoch.
    pub created_at: i64,
    pub updated_at: i64,
}

/// A user's studies, as Lichess lists them (most recently made first):
/// their public ones, or with their own token their unlisted and private
/// ones too.
pub fn studies_of(
    client: &Client,
    username: &str,
    token: Option<&Token>,
    stop: &dyn Fn() -> bool,
) -> Result<Vec<StudyInfo>, NetError> {
    let name = crate::check_username(username)?;
    let url = format!("{}/api/study/by/{name}", client.lichess);
    let auth: Vec<(&str, String)> = token.map(|t| ("Authorization", t.bearer())).into_iter().collect();
    let mut resp = client.get_with(Site::Lichess, &url, &[], "application/x-ndjson", &auth, stop)?;
    match resp.status().as_u16() {
        200 => {}
        401 if token.is_some() => return Err(NetError::TokenRejected),
        404 => return Err(NetError::NoSuchPlayer { site: Site::Lichess, name: name.to_owned() }),
        _ => return Err(status_error(Site::Lichess, &resp)),
    }
    let body = read_bytes(Site::Lichess, &mut resp, 16 << 20)?;
    let bad = |e: serde_json::Error| NetError::BadData { site: Site::Lichess, reason: e.to_string() };
    body.split(|&b| b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .map(|l| serde_json::from_slice(l).map_err(bad))
        .collect()
}

/// A chapter of a study's export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub name: String,
    /// From its `ChapterURL`, if it has one.
    pub key: Option<GameKey>,
    /// Its PGN, tags and all.
    pub pgn: String,
}

/// A study's (or a chapter's) PGN export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudyPgn {
    pub link: StudyRef,
    /// The study's name (`StudyName`), or its id if no chapter says.
    pub name: String,
    pub chapters: Vec<Chapter>,
    /// When the study last changed, as the server wrote it, for
    /// `If-Modified-Since`.
    pub last_modified: Option<String>,
}

/// Downloads a study's chapters, or one chapter: with their comments,
/// variations, clock times and orientation. Public studies need no token;
/// private and unlisted ones need the token of a member. `None` when the
/// study hasn't changed since `since` (a `Last-Modified` from an earlier
/// download).
pub fn fetch_study(
    client: &Client,
    link: &StudyRef,
    token: Option<&Token>,
    since: Option<&str>,
    stop: &dyn Fn() -> bool,
) -> Result<Option<StudyPgn>, NetError> {
    fetch(client, link, token, since, true, stop)
}

/// A study's (or a chapter's) outline, to show what's in it: the light
/// export, without comments, variations or clock times, which is a
/// fraction of the full one for an annotated study. Its chapters' PGN is
/// their main line only, so the import downloads the full export.
pub fn fetch_study_outline(
    client: &Client,
    link: &StudyRef,
    token: Option<&Token>,
    stop: &dyn Fn() -> bool,
) -> Result<StudyPgn, NetError> {
    fetch(client, link, token, None, false, stop)?
        .ok_or_else(|| NetError::BadData { site: Site::Lichess, reason: "a 304 unasked".into() })
}

fn fetch(
    client: &Client,
    link: &StudyRef,
    token: Option<&Token>,
    since: Option<&str>,
    full: bool,
    stop: &dyn Fn() -> bool,
) -> Result<Option<StudyPgn>, NetError> {
    let url = match &link.chapter {
        Some(c) => format!("{}/api/study/{}/{c}.pgn", client.lichess, link.study),
        None => format!("{}/api/study/{}.pgn", client.lichess, link.study),
    };
    let on = full.to_string();
    let query = [
        ("clocks", on.clone()),
        ("comments", on.clone()),
        ("variations", on),
        ("orientation", "true".into()),
    ];
    let mut headers: Vec<(&str, String)> = token.map(|t| ("Authorization", t.bearer())).into_iter().collect();
    if let Some(since) = since {
        headers.push(("If-Modified-Since", since.to_owned()));
    }
    let mut resp = client.get_with(Site::Lichess, &url, &query, "application/x-chess-pgn", &headers, stop)?;
    match resp.status().as_u16() {
        200 => {}
        304 => return Ok(None),
        // Without a token, Lichess says 401 for a private study; with one
        // that isn't a member's, 403. A token it doesn't know is a 401 too.
        401 if token.is_some() => return Err(NetError::TokenRejected),
        401 | 403 if token.is_none() => return Err(NetError::StudyNeedsLogin),
        403 => return Err(NetError::StudyForbidden),
        404 => return Err(NetError::NoSuchStudy),
        _ => return Err(status_error(Site::Lichess, &resp)),
    }
    let last_modified = resp.headers().get("Last-Modified").and_then(|v| v.to_str().ok()).map(str::to_owned);
    let body = read_bytes(Site::Lichess, &mut resp, MAX_STUDY)?;
    Ok(Some(parse_study(link, &String::from_utf8_lossy(&body), last_modified)))
}

/// The chapters of a study's PGN export.
pub fn parse_study(link: &StudyRef, pgn: &str, last_modified: Option<String>) -> StudyPgn {
    let chapters: Vec<Chapter> = split_games(pgn)
        .into_iter()
        .map(|g| Chapter {
            name: tag_value(g, "ChapterName").unwrap_or_default(),
            key: tag_value(g, "ChapterURL")
                .and_then(|u| key_of_url(&u))
                .filter(|k| k.starts_with("lichess-study:")),
            pgn: g.trim().to_owned(),
        })
        .collect();
    let name = chapters
        .iter()
        .find_map(|c| tag_value(&c.pgn, "StudyName"))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| link.study.clone());
    StudyPgn { link: link.clone(), name, chapters, last_modified }
}

/// The games of a PGN text, each from its first tag to the next game's: a
/// `[` starting a line (outside a comment) after movetext starts a game.
/// Comments can span lines, blank ones and ones starting with `[`
/// included (Lichess's do).
pub fn split_games(pgn: &str) -> Vec<&str> {
    let b = pgn.as_bytes();
    let mut starts = Vec::new();
    // Movetext since the last game started; before the first, a tag starts one.
    let mut moves = true;
    let (mut i, mut line_start, mut comment) = (0, true, false);
    let to_eol = |i: &mut usize| {
        while *i < b.len() && b[*i] != b'\n' {
            *i += 1;
        }
    };
    while i < b.len() {
        let c = b[i];
        if comment {
            comment = c != b'}';
            i += 1;
            continue;
        }
        match c {
            b'\n' => {
                line_start = true;
                i += 1;
                continue;
            }
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'[' if line_start => {
                if moves {
                    starts.push(i);
                    moves = false;
                }
                // To the tag's end: a `]` outside its value, or the line's.
                let mut quoted = false;
                i += 1;
                while i < b.len() && b[i] != b'\n' {
                    match b[i] {
                        b'\\' if quoted => i += 1,
                        b'"' => quoted = !quoted,
                        b']' if !quoted => break,
                        _ => {}
                    }
                    i += 1;
                }
            }
            // To the line's end, which the next turn reads.
            b'%' if line_start => {
                to_eol(&mut i);
                continue;
            }
            b';' => {
                moves = true;
                to_eol(&mut i);
                continue;
            }
            b'{' => {
                moves = true;
                comment = true;
            }
            _ => moves = true,
        }
        line_start = false;
        i += 1;
    }
    let mut games = Vec::with_capacity(starts.len() + 1);
    // Movetext before the first tag: a game without tags.
    let first = starts.first().copied().unwrap_or(b.len());
    if !pgn[..first].trim().is_empty() {
        games.push(&pgn[..first]);
    }
    for (n, &s) in starts.iter().enumerate() {
        games.push(&pgn[s..starts.get(n + 1).copied().unwrap_or(b.len())]);
    }
    games
}

/// The value of a tag in a game's tag section (`[Name "value"]`).
pub fn tag_value(game: &str, name: &str) -> Option<String> {
    for line in game.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(inner) = line.strip_prefix('[') else { break };
        let end = inner.find(|c: char| c.is_whitespace() || c == '"').unwrap_or(inner.len());
        if &inner[..end] != name {
            continue;
        }
        let Some(value) = inner[end..].trim_start().strip_prefix('"') else { continue };
        let mut out = String::new();
        let mut chars = value.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.extend(chars.next()),
                '"' => return Some(out),
                c => out.push(c),
            }
        }
        return Some(out);
    }
    None
}

/// What importing a study did.
#[derive(Debug)]
pub struct StudyImport {
    pub link: StudyRef,
    pub name: String,
    /// The chapters in the study (or the one asked for).
    pub chapters: u64,
    /// The chapters that went in, and those that couldn't be read.
    pub import: ImportSummary,
    /// Chapters left out as the database had them already.
    pub already: u64,
    /// The study hadn't changed since it was looked up (a 304): the
    /// chapters came from then.
    pub unchanged: bool,
    pub cancelled: bool,
}

/// A reader that counts what it hands over, for the progress.
struct Counted<'a, R> {
    inner: R,
    read: &'a Cell<u64>,
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

/// Imports the chapters of `study` the database doesn't have (told by their
/// `ChapterURL`; deleted games count until the database is compacted)
/// through the PGN importer, in their order.
pub fn import_study(
    db: &Mutex<Database>,
    study: &StudyPgn,
    progress: &dyn Progress,
) -> Result<StudyImport, NetError> {
    let known = match known_games(db, &Part { inner: progress, from: 0, span: 200 }) {
        Err(DbError::Cancelled) => return Err(NetError::Cancelled),
        other => other?,
    };
    let (mut text, mut already, mut seen) = (String::new(), 0, HashSet::new());
    for c in &study.chapters {
        if let Some(k) = &c.key
            && (known.contains(k) || !seen.insert(k))
        {
            already += 1;
            continue;
        }
        text.push_str(&c.pgn);
        text.push_str("\n\n");
    }
    let read = Cell::new(0);
    let input = Counted { inner: text.as_bytes(), read: &read };
    let part = Part { inner: progress, from: 200, span: 800 };
    let import = import_reader(db, input, text.len() as u64, &|| read.get(), &part)?;
    Ok(StudyImport {
        link: study.link.clone(),
        name: study.name.clone(),
        chapters: study.chapters.len() as u64,
        cancelled: import.cancelled,
        import,
        already,
        unchanged: false,
    })
}

/// Runs `work` on a thread of its own and waits for it, or for a cancel,
/// which takes effect at once even while a server keeps us waiting (the
/// thread then stops at its next look at `stop`, or when its connection
/// drops, like a download's fetcher).
pub(crate) fn cancellable<T: Send + 'static>(
    progress: &dyn Progress,
    work: impl FnOnce(&dyn Fn() -> bool) -> Result<T, NetError> + Send + 'static,
) -> Result<T, NetError> {
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    let (tx, rx) = sync_channel(1);
    std::thread::Builder::new()
        .name("lichess".into())
        .spawn(move || {
            let stop = || flag.load(Ordering::Relaxed);
            let _ = tx.send(work(&stop));
        })
        .map_err(DbError::Io)?;
    loop {
        if progress.is_cancelled() {
            stopped.store(true, Ordering::Relaxed);
            return Err(NetError::Cancelled);
        }
        match rx.recv_timeout(POLL) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(NetError::Unreachable {
                    site: Site::Lichess,
                    reason: "the download stopped".into(),
                });
            }
        }
    }
}

/// Downloads a study (or a chapter) and imports the chapters the database
/// doesn't have, as a job: a Cancel takes effect at once. With `looked_up`,
/// the same study downloaded a moment ago (to show its name and chapters),
/// Lichess is only asked whether it changed since (a 304 when it hasn't).
pub fn download_study_into(
    client: &Client,
    db: &Mutex<Database>,
    link: &StudyRef,
    token: Option<&Token>,
    looked_up: Option<&StudyPgn>,
    progress: &dyn Progress,
) -> Result<StudyImport, NetError> {
    let earlier = looked_up.filter(|s| &s.link == link);
    // A chapter's export is new every time: no point asking.
    let since = earlier.filter(|_| link.chapter.is_none()).and_then(|s| s.last_modified.clone());
    let (c, l, t) = (client.clone(), link.clone(), token.cloned());
    let fetched = cancellable(progress, move |stop| fetch_study(&c, &l, t.as_ref(), since.as_deref(), stop))?;
    let (study, unchanged) = match (fetched, earlier) {
        (Some(study), _) => (study, false),
        (None, Some(earlier)) => (earlier.clone(), true),
        (None, None) => {
            return Err(NetError::BadData { site: Site::Lichess, reason: "a 304 unasked".into() });
        }
    };
    let mut summary = import_study(db, &study, progress)?;
    summary.unchanged = unchanged;
    Ok(summary)
}

/// What [`refresh_study_into`] did.
#[derive(Debug)]
pub struct StudyRefresh {
    /// `None` when the study hadn't changed since (a 304): nothing came.
    pub import: Option<StudyImport>,
    /// The study's `Last-Modified` to ask with next time: `None` after an
    /// import that didn't take every chapter (cancelled), so the next
    /// refresh downloads it again.
    pub last_modified: Option<String>,
}

/// Brings a study's new chapters into a database kept up to date with it
/// (its sources: NET-01, IO-10): Lichess is asked whether the study changed
/// since `since`, the `Last-Modified` of the last refresh, and only then is
/// it downloaded and the chapters the database lacks imported. A Cancel
/// takes effect at once.
pub fn refresh_study_into(
    client: &Client,
    db: &Mutex<Database>,
    link: &StudyRef,
    token: Option<&Token>,
    since: Option<&str>,
    progress: &dyn Progress,
) -> Result<StudyRefresh, NetError> {
    // A chapter's export is new every time: no point asking.
    let since = since.filter(|_| link.chapter.is_none()).map(str::to_owned);
    let (c, l, t, asked) = (client.clone(), link.clone(), token.cloned(), since.clone());
    let fetched = cancellable(progress, move |stop| fetch_study(&c, &l, t.as_ref(), asked.as_deref(), stop))?;
    let Some(study) = fetched else {
        return Ok(StudyRefresh { import: None, last_modified: since });
    };
    let import = import_study(db, &study, progress)?;
    let last_modified = if import.cancelled { None } else { study.last_modified.clone() };
    Ok(StudyRefresh { import: Some(import), last_modified })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn study_links() {
        let study = |s: &str, c: Option<&str>| StudyRef { study: s.into(), chapter: c.map(Into::into) };
        for (text, want) in [
            ("https://lichess.org/study/keq5vaIF", study("keq5vaIF", None)),
            ("  lichess.org/study/keq5vaIF/  ", study("keq5vaIF", None)),
            ("https://lichess.org/study/keq5vaIF/dLpkgwla", study("keq5vaIF", Some("dLpkgwla"))),
            ("https://lichess.org/study/keq5vaIF/dLpkgwla#last", study("keq5vaIF", Some("dLpkgwla"))),
            ("http://www.lichess.org/study/keq5vaIF?x=1", study("keq5vaIF", None)),
            ("https://lichess.org/api/study/keq5vaIF.pgn", study("keq5vaIF", None)),
            ("https://lichess.org/api/study/keq5vaIF/dLpkgwla.pgn", study("keq5vaIF", Some("dLpkgwla"))),
            ("keq5vaIF", study("keq5vaIF", None)),
        ] {
            assert_eq!(parse_study_link(text).unwrap(), want, "{text}");
        }
        for text in [
            "",
            "https://lichess.org/keq5vaIF",
            "https://lichess.org/study",
            "https://lichess.org/study/by/thibault",
            "https://example.org/study/keq5vaIF",
            "https://lichess.org/study/keq5va",
            "keq5va-F",
        ] {
            assert!(matches!(parse_study_link(text), Err(NetError::NotAStudyLink(_))), "{text}");
        }
        let client = Client::with_servers("http://127.0.0.1:9", "https://api.chess.com");
        let chapter = study("keq5vaIF", Some("dLpkgwla"));
        assert_eq!(chapter.url(&client), "http://127.0.0.1:9/study/keq5vaIF/dLpkgwla");
        assert_eq!(chapter.link(), "https://lichess.org/study/keq5vaIF/dLpkgwla");
        assert_eq!(parse_study_link(&chapter.link()).unwrap(), chapter, "a link reads back");
    }

    #[test]
    fn splits_games_with_comments_over_lines() {
        let pgn = "[Event \"A\"]\n[Site \"x\"]\n\n{ Intro\n\n[not a tag] still the comment }\n1. e4 e5 *\n\n\n\
                   [Event \"B \\\"quoted\\\" ]\"]\n\n1. d4 ; a [comment\n[Event \"x\"] is still B's? no: a new game\n";
        let games = split_games(pgn);
        assert_eq!(games.len(), 3, "{games:#?}");
        assert!(games[0].contains("[not a tag]") && games[0].contains("1. e4"));
        assert_eq!(tag_value(games[1], "Event").as_deref(), Some("B \"quoted\" ]"));
        assert_eq!(tag_value(games[2], "Event").as_deref(), Some("x"));
        // A chapter with no moves.
        assert_eq!(split_games("[Event \"A\"]\n\n *\n\n\n[Event \"B\"]\n\n1. e4 *\n").len(), 2);
        assert_eq!(split_games("1. e4 *\n\n[Event \"B\"]\n\n1. d4 *").len(), 2, "a game without tags first");
        assert!(split_games("  \n").is_empty());
        assert_eq!(tag_value("[White \"Kasparov, G\"]\n[Black \"\"]\n\n1. e4", "Black").as_deref(), Some(""));
        assert_eq!(tag_value("[White \"Kasparov\"]\n\n1. e4 [Black \"x\"]", "Black"), None);
    }
}
