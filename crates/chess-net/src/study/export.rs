//! Games out to Lichess studies (NET-12): to new studies of 64 chapters at
//! most, or to one of the user's that has room for them.
//!
//! Through `POST /api/study` (a new study; Lichess makes 30 a day per
//! account, 60 for coaches and titled players) and
//! `/api/study/{id}/import-pgn` (each game a chapter, at most 64 in a
//! study). Writes are sent once: Lichess answers its daily limit with the
//! same 429 as too many requests, and asking again can't help. The client
//! is kept to requests one at a time, so NET-13's chapter updates
//! (`…/{chapter}/moves`, `…/tags`, `DELETE …/{chapter}`) fit beside these.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use chess_db::Progress;
use serde::Deserialize;

use super::{StudyRef, fetch};
use crate::http::{Client, Payload, Write, encoded_len, error_message, read_json, status_error};
use crate::keys::is_lichess_id;
use crate::oauth::Token;
use crate::{NetError, Site};

/// The most chapters a study holds.
pub const MAX_CHAPTERS: usize = 64;
/// The most new studies Lichess makes for one account in a day (twice as
/// many for coaches and titled players).
pub const DAILY_STUDIES: usize = 30;
/// The most PGN one request sends, percent-encoded: Lichess takes bodies of
/// up to 512 KiB.
const MAX_BODY: usize = 400 << 10;

/// Who can see a study.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Anyone, and it's listed.
    Public,
    /// Anyone with its link.
    Unlisted,
    /// Its members only.
    Private,
}

impl Visibility {
    fn name(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Unlisted => "unlisted",
            Visibility::Private => "private",
        }
    }
}

/// Where games go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportTarget {
    /// New studies: one, or as many as 64 chapters each take, named
    /// "Name (1/3)", "Name (2/3)"…
    New { name: String, visibility: Visibility },
    /// One of the user's studies, by id: the games must fit in it.
    Existing(String),
}

/// A study games went to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentStudy {
    pub id: String,
    pub name: String,
    pub url: String,
    /// Made for these games.
    pub made: bool,
    /// The games sent to it.
    pub sent: usize,
    /// The chapters Lichess made of them.
    pub chapters: usize,
    /// What Lichess said about games it didn't take.
    pub refused: Option<String>,
}

/// What [`export_to_studies`] did.
#[derive(Debug)]
pub struct ExportSummary {
    /// Where the games went, in their order.
    pub studies: Vec<SentStudy>,
    /// The games asked to be sent.
    pub games: usize,
    /// Games too long for Lichess to take, left out.
    pub too_long: usize,
    /// Why it ended before the last game, when it did: the games before are
    /// on Lichess, the others weren't sent.
    pub stopped: Option<NetError>,
    pub cancelled: bool,
}

impl ExportSummary {
    /// The games sent (each to a study that took it, or said why not).
    pub fn sent(&self) -> usize {
        self.studies.iter().map(|s| s.sent).sum()
    }

    /// The chapters Lichess made.
    pub fn chapters(&self) -> usize {
        self.studies.iter().map(|s| s.chapters).sum()
    }

    /// The games not sent: left when it stopped, or too long.
    pub fn not_sent(&self) -> usize {
        self.games - self.sent()
    }
}

/// How many studies `games` games need.
pub fn studies_needed(games: usize) -> usize {
    games.div_ceil(MAX_CHAPTERS).max(1)
}

/// A study name as Lichess takes it: 2 to 100 characters, trimmed.
pub fn check_study_name(name: &str) -> Result<&str, NetError> {
    let name = name.trim();
    if (2..=100).contains(&name.chars().count()) { Ok(name) } else { Err(NetError::BadStudyName) }
}

/// The names of `n` studies: the name for one, "Name (1/3)", "Name (2/3)"…
/// for several, the name shortened to fit Lichess's 100 characters.
pub fn study_names(name: &str, n: usize) -> Vec<String> {
    let name = name.trim();
    if n <= 1 {
        return vec![name.chars().take(100).collect()];
    }
    (1..=n)
        .map(|i| {
            let suffix = format!(" ({i}/{n})");
            let room = 100 - suffix.chars().count();
            let base: String = name.chars().take(room).collect();
            format!("{}{suffix}", base.trim_end())
        })
        .collect()
}

/// Sends `games` (PGN texts, one game each) to Lichess studies, each game a
/// chapter: to new studies of 64 chapters at most, or to one of the user's
/// studies that has room for them. Stops at the first refusal (Lichess's
/// daily limit on new studies, say), and says which games went where
/// (`studies`) and why it stopped. A cancel takes effect between two
/// requests, so the summary is always what Lichess has. Errors before
/// anything was sent or made (the study is full, the token was revoked)
/// come back as errors.
pub fn export_to_studies(
    client: &Client,
    token: &Token,
    target: &ExportTarget,
    games: &[String],
    progress: &dyn Progress,
) -> Result<ExportSummary, NetError> {
    let mut summary = ExportSummary {
        studies: Vec::new(),
        games: games.len(),
        too_long: 0,
        stopped: None,
        cancelled: false,
    };
    let stop = || progress.is_cancelled();
    let total = games.len() as u64;
    let mut done = 0u64;
    // (the study if it exists, its name, its games), in order.
    let plan: Vec<(Option<String>, String, &[String])> = match target {
        ExportTarget::New { name, .. } => {
            let name = check_study_name(name)?;
            let n = studies_needed(games.len());
            if n > DAILY_STUDIES {
                return Err(NetError::TooManyStudies { games: games.len(), studies: n });
            }
            let names = study_names(name, n);
            names.into_iter().zip(games.chunks(MAX_CHAPTERS)).map(|(name, g)| (None, name, g)).collect()
        }
        ExportTarget::Existing(id) => {
            let link = StudyRef { study: id.clone(), chapter: None };
            let study = fetch(client, &link, Some(token), None, false, &stop)?
                .ok_or_else(|| NetError::BadData { site: Site::Lichess, reason: "a 304 unasked".into() })?;
            let room = MAX_CHAPTERS.saturating_sub(study.chapters.len());
            if games.len() > room {
                return Err(NetError::StudyFull { name: study.name, room });
            }
            vec![(Some(id.clone()), study.name, games)]
        }
    };
    'studies: for (id, name, chunk) in plan {
        if progress.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        let made = id.is_none();
        let id = match id {
            Some(id) => id,
            None => {
                let ExportTarget::New { visibility, .. } = target else { unreachable!("new studies") };
                match create_study(client, token, &name, *visibility) {
                    Ok(id) => id,
                    Err(e) => {
                        summary.stopped = Some(e);
                        break;
                    }
                }
            }
        };
        let url = format!("{}/study/{id}", client.lichess);
        let mut sent = SentStudy { id, name, url, made, sent: 0, chapters: 0, refused: None };
        // In batches Lichess takes in one request; the first batch into a
        // new study takes the place of its empty first chapter.
        let mut first = made;
        for batch in batches(chunk, &mut summary.too_long) {
            if progress.is_cancelled() {
                summary.cancelled = true;
                summary.studies.push(sent);
                break 'studies;
            }
            match import_pgn(client, token, &sent.id, &batch.pgn, first) {
                Ok((chapters, refused)) => {
                    sent.sent += batch.games;
                    sent.chapters += chapters;
                    if let Some(r) = refused {
                        sent.refused = Some(r);
                    }
                    first = false;
                }
                Err(e) => {
                    summary.stopped = Some(e);
                    summary.studies.push(sent);
                    break 'studies;
                }
            }
            done += batch.games as u64;
            progress.report(done, total);
        }
        summary.studies.push(sent);
    }
    // Nothing sent and no study made: it's the export's error.
    if summary.studies.iter().all(|s| s.sent == 0 && !s.made)
        && let Some(e) = summary.stopped.take()
    {
        return Err(e);
    }
    Ok(summary)
}

/// Games sent in one request.
struct Batch {
    pgn: String,
    games: usize,
}

/// `games` in batches of at most [`MAX_BODY`] bytes once encoded; a game
/// longer than that alone is left out (counted in `too_long`).
fn batches(games: &[String], too_long: &mut usize) -> Vec<Batch> {
    const GAP: &str = "\n\n\n";
    let mut out: Vec<Batch> = Vec::new();
    let mut size = 0;
    for g in games {
        let g = g.trim();
        let len = encoded_len(g) + encoded_len(GAP);
        if len > MAX_BODY {
            *too_long += 1;
            continue;
        }
        match out.last_mut() {
            Some(b) if size + len <= MAX_BODY => {
                b.pgn.push_str(GAP);
                b.pgn.push_str(g);
                b.games += 1;
                size += len;
            }
            _ => {
                out.push(Batch { pgn: g.to_owned(), games: 1 });
                size = len;
            }
        }
    }
    out
}

/// Makes a study (Lichess adds an empty first chapter), with the settings
/// that keep a private study to its owner.
fn create_study(
    client: &Client,
    token: &Token,
    name: &str,
    visibility: Visibility,
) -> Result<String, NetError> {
    #[derive(Deserialize)]
    struct Made {
        id: String,
    }
    // Who may clone or share (export) it: anyone who can see it, or for a
    // private one only its owner.
    let others = if visibility == Visibility::Private { "owner" } else { "everyone" };
    let fields = vec![
        ("name", name),
        ("visibility", visibility.name()),
        ("computer", "everyone"),
        ("explorer", "everyone"),
        ("cloneable", others),
        ("shareable", others),
        ("chat", "member"),
    ];
    let url = format!("{}/api/study", client.lichess);
    let write = Write { method: "POST", url: &url, token: Some(token), body: Payload::Form(fields) };
    let mut resp = client.send(Site::Lichess, write)?;
    match resp.status().as_u16() {
        200 => {}
        // The daily limit, or too many requests: the same answer.
        429 => return Err(NetError::StudyLimit),
        401 => return Err(NetError::TokenRejected),
        403 => return Err(NetError::MissingScopes("study:write".into())),
        400 => {
            let why = error_message(Site::Lichess, &mut resp).unwrap_or_else(|| "it didn't say why".into());
            return Err(NetError::Refused(why));
        }
        _ => return Err(status_error(Site::Lichess, &resp)),
    }
    let made: Made = read_json(Site::Lichess, &mut resp, 1 << 16, &Arc::new(AtomicI64::new(0)))?;
    if !is_lichess_id(&made.id) {
        return Err(NetError::BadData { site: Site::Lichess, reason: format!("a study id {:?}", made.id) });
    }
    Ok(made.id)
}

/// Sends games to a study, each a chapter: how many chapters Lichess made,
/// and what it said about the games it didn't take. `initial`: the study's
/// empty first chapter goes (a study made for these games).
fn import_pgn(
    client: &Client,
    token: &Token,
    study: &str,
    pgn: &str,
    initial: bool,
) -> Result<(usize, Option<String>), NetError> {
    #[derive(Deserialize)]
    struct Made {
        #[serde(default)]
        chapters: Vec<serde_json::Value>,
        error: Option<String>,
    }
    let url = format!("{}/api/study/{study}/import-pgn", client.lichess);
    let mut fields = vec![("pgn", pgn)];
    if initial {
        fields.push(("initial", "true"));
    }
    let write = Write { method: "POST", url: &url, token: Some(token), body: Payload::Form(fields) };
    let mut resp = client.send(Site::Lichess, write)?;
    match resp.status().as_u16() {
        200 => {}
        429 => return Err(NetError::RateLimited(Site::Lichess)),
        401 => return Err(NetError::TokenRejected),
        403 => return Err(NetError::NotContributor),
        404 => return Err(NetError::NoSuchStudy),
        400 => {
            let why = error_message(Site::Lichess, &mut resp).unwrap_or_else(|| "it didn't say why".into());
            return Err(NetError::Refused(why));
        }
        _ => return Err(status_error(Site::Lichess, &resp)),
    }
    let made: Made = read_json(Site::Lichess, &mut resp, 4 << 20, &Arc::new(AtomicI64::new(0)))?;
    Ok((made.chapters.len(), made.error.filter(|e| !e.trim().is_empty())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_of_split_studies() {
        assert_eq!(study_names(" Openings ", 1), ["Openings"]);
        assert_eq!(study_names("Openings", 3), ["Openings (1/3)", "Openings (2/3)", "Openings (3/3)"]);
        let long = "x".repeat(120);
        let names = study_names(&long, 12);
        assert!(names.iter().all(|n| n.chars().count() == 100), "{names:?}");
        assert!(names[11].ends_with(" (12/12)"));
        assert_eq!(studies_needed(0), 1);
        assert_eq!(studies_needed(64), 1);
        assert_eq!(studies_needed(65), 2);
        assert!(check_study_name(" a ").is_err() && check_study_name(&long).is_err());
        assert_eq!(check_study_name(" ab ").unwrap(), "ab");
    }

    #[test]
    fn batches_keep_under_the_body_limit() {
        let small = "[Event \"x\"]\n\n1. e4 *".to_owned();
        let big = format!("[Event \"y\"]\n\n{{ {} }} 1. e4 *", "a".repeat(250 << 10));
        let huge = format!("[Event \"z\"]\n\n{{ {} }} 1. e4 *", "a".repeat(MAX_BODY));
        let mut too_long = 0;
        let b = batches(&[small.clone(), big.clone(), big.clone(), huge, small.clone()], &mut too_long);
        assert_eq!(too_long, 1);
        assert_eq!(b.iter().map(|b| b.games).collect::<Vec<_>>(), [2, 2]);
        assert!(b.iter().all(|b| encoded_len(&b.pgn) <= MAX_BODY));
        assert_eq!(b[0].pgn, format!("{small}\n\n\n{}", big.trim()));
    }
}
