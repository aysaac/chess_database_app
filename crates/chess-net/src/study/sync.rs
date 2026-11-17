//! NET-13's sync: a database's games kept in step with a Lichess study's
//! chapters, as the Sync button does it. [`plan`] looks at both sides and
//! says what a sync would do (the preview); [`apply`] does it, as one job;
//! [`resolve`] settles a game left for the user. `docs/net-13-sync.md` has
//! the rules.
//!
//! Each linked game has its link (`chess_db::StudyLink`, by permanent id):
//! the chapter, the mode, and the base snapshot, which is Lichess's own
//! export of the chapter at the last sync. A game **changed here** is one
//! whose Lichess projection differs from the base's ([`merge::changed_here`]);
//! one **changed on Lichess** is a chapter that differs from it
//! ([`merge::same_on_lichess`]). Then:
//!
//! - Neither: nothing (the base is brought up to date if Lichess only
//!   rewrote its export).
//! - Lichess only: the chapter comes here, with what only the database holds
//!   put back ([`super::layer::apply_layer`]).
//! - Here only: its projection goes to Lichess (`…/{chapter}/moves`, and
//!   `…/tags` for the tags that changed), two-way only. After every push the
//!   chapter is fetched again as the new base.
//! - Both: the three-way merge ([`merge::merge`]); a clean one that doesn't
//!   take much away goes both ways, anything else waits for the user with a
//!   pending merge on the link ([`Pending`]): never overwritten.
//! - A chapter with no game here: a game here that is that chapter (its
//!   `ChapterURL`, or the same moves: a push whose link a failure cut short)
//!   is linked to it; otherwise it comes here as a new game.
//! - A game here with no chapter (two-way): a new chapter (`import-pgn`, in
//!   batches), and its link.
//! - A chapter removed on Lichess: its game is marked as deleted here (kept
//!   until Compact) unless it changed here, which waits for the user. A game
//!   deleted here keeps its chapter unless the sync is asked to remove it.
//!
//! Safe to run again: a sync cut short leaves every game either as it was or
//! done, and the next one finds the rest (a pushed game whose link wasn't
//! recorded is the same as its chapter, and is linked to it, never sent
//! twice). Games open in a tab aren't changed. Writes wait out a 429 and are
//! sent again (Lichess did nothing with them); a write that fails otherwise
//! ends the sync there, and what was done before stays done.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

use chess_core::game::Game;
use chess_core::notation::san;
use chess_core::pgn::{read_pgn_str, write_game};
use chess_core::shakmaty::Position;
use chess_db::bytes::{ByteReader, put_str, put_varint};
use chess_db::{Database, DbError, GameId, GameUid, PreparedGame, Progress, StudyLink, SyncMode};
use serde::Deserialize;

use super::export::{MAX_BODY, MAX_CHAPTERS, Visibility, check_study_name, create_study};
use super::layer::{Layer, apply_layer, lichess_projection, local_only_layer};
use super::merge::{self, Conflict, changed_here, same_on_lichess, tag_changes};
use super::{StudyPgn, StudyRef, cancellable, fetch_study, parse_study_link, tag_value};
use crate::http::{Client, encoded_len, error_message, read_json, status_error};
use crate::keys::is_lichess_id;
use crate::oauth::{Token, token_scopes};
use crate::time::now_ms;
use crate::{NetError, Site};

/// The tag that marks a copy kept from a conflict ("Keep both"): this
/// database's version of a chapter. Its value is the chapter's link. A sync
/// never sends it to the study; without the tag it's a new game like any.
pub const CONFLICT_COPY_TAG: &str = "LichessSyncCopy";

/// The most games here outside the study a sync looks at: a study holds 64
/// chapters, so a database with many more isn't one kept in step with one.
pub const MOST_UNLINKED: usize = 1000;

/// How many times a chapter that doesn't show what was sent yet (Lichess's
/// cache) is asked for again, [`Client::with_fresh_wait`] apart.
const FRESH_TRIES: usize = 3;

// ---- A merge waiting for the user --------------------------------------------------

/// Why a game waits for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    /// Changed here and on Lichess, and the merge has conflicts or would take
    /// much away.
    Both,
    /// Its chapter was removed on Lichess, and it changed here.
    RemovedThere,
    /// A game here that is the chapter (its `ChapterURL` says so) but
    /// differs from it, and was never synced: no base says which side
    /// changed.
    NoBase,
}

impl PendingKind {
    fn to_u8(self) -> u8 {
        match self {
            PendingKind::Both => 1,
            PendingKind::RemovedThere => 2,
            PendingKind::NoBase => 3,
        }
    }

    fn from_u8(b: u8) -> Option<PendingKind> {
        match b {
            1 => Some(PendingKind::Both),
            2 => Some(PendingKind::RemovedThere),
            3 => Some(PendingKind::NoBase),
            _ => None,
        }
    }
}

/// A merge waiting for the user, kept with the game's link (after its
/// local-only layer, [`Layer::to_bytes_with`]) until it's resolved. The
/// link's base stays the last one both sides agreed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub kind: PendingKind,
    /// The chapter as Lichess exported it when this was found ("" when it
    /// was removed there).
    pub theirs: String,
    /// When (ms since the epoch).
    pub found: i64,
}

const PENDING_VERSION: u8 = 1;

impl Pending {
    /// `version:u8(=1) kind:u8 found:varint theirs:str`.
    fn to_bytes(&self) -> Vec<u8> {
        let mut b = vec![PENDING_VERSION, self.kind.to_u8()];
        put_varint(&mut b, self.found.max(0) as u64);
        put_str(&mut b, &self.theirs);
        b
    }

    fn from_bytes(bytes: &[u8]) -> Result<Pending, NetError> {
        let bad =
            |what: &str| NetError::Db(DbError::Corrupt(format!("a linked game's pending merge: {what}")));
        let mut r = ByteReader::new(bytes);
        let version = r.u8()?;
        if version != PENDING_VERSION {
            return Err(NetError::Db(DbError::UnsupportedVersion(u32::from(version))));
        }
        let kind = PendingKind::from_u8(r.u8()?).ok_or_else(|| bad("its kind"))?;
        let found = i64::try_from(r.varint()?).map_err(|_| bad("its time"))?;
        let theirs = r.str()?;
        if !r.is_empty() {
            return Err(bad("bytes after its end"));
        }
        Ok(Pending { kind, theirs, found })
    }
}

/// A link's local-only layer and pending merge, as kept in its `layer`.
pub fn link_state(link: &StudyLink) -> Result<(Layer, Option<Pending>), NetError> {
    let (layer, pending) = Layer::from_bytes_with(&link.layer)?;
    Ok((layer, pending.map(|p| Pending::from_bytes(&p)).transpose()?))
}

/// The bytes of a link's `layer`: the local-only layer of `game`, and a
/// pending merge if there's one.
fn layer_bytes(game: &Game, pending: Option<&Pending>) -> Vec<u8> {
    let p = pending.map(Pending::to_bytes);
    local_only_layer(game).to_bytes_with(p.as_deref())
}

// ---- What a sync does ---------------------------------------------------------------

/// What a sync does with a game or chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    Unchanged,
    /// Changed on Lichess: the chapter comes here.
    Pull,
    /// Changed here: it goes to Lichess.
    Push,
    /// Changed on both sides and merged: the merge goes here and to Lichess
    /// (only here, following only).
    Merge,
    /// Changed here, following only: it stays here, nothing goes to Lichess.
    KeptHere,
    /// Changed on both sides, and the merge has conflicts or would take much
    /// away; or removed on Lichess and changed here; or a game here that is
    /// the chapter but differs: left for the user.
    Conflict,
    /// A merge already waits for the user.
    Pending,
    /// A chapter new on Lichess: a new game here.
    NewThere,
    /// A game here that is a chapter of the study (its `ChapterURL`, or the
    /// same moves), the same as it: linked.
    Adopt,
    /// A game here the study doesn't have: a new chapter.
    NewHere,
    /// The same, but the study has no room (64 chapters).
    NoRoom,
    /// The same, but not sent: following only, a copy kept from a conflict,
    /// or the database is linked to other studies too.
    NotSent,
    /// Its chapter was removed on Lichess: marked as deleted here.
    RemovedThere,
    /// Marked as deleted here: its chapter stays on Lichess.
    DeletedHere,
    /// Marked as deleted here: its chapter is removed on Lichess too.
    DeleteChapter,
    /// Open in a tab: left until it's closed.
    Open,
}

/// One game or chapter in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The game here (none for a chapter new on Lichess).
    pub uid: Option<GameUid>,
    /// Its chapter on Lichess, if it has one.
    pub chapter: Option<String>,
    /// "White - Black", the chapter's name, or its event.
    pub name: String,
    pub step: Step,
    /// More, for some steps: what's in conflict, why a game isn't sent.
    pub note: Option<String>,
}

/// What to keep in step with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A study: the one the database is linked to, or one to link it to (one
    /// of the user's, or the one a static copy came from). `mode` is that of
    /// links made now; links kept keep theirs.
    Study { study: String, mode: SyncMode },
    /// A new study for the database's games, made by [`apply`].
    New { name: String, visibility: Visibility, mode: SyncMode },
}

impl Target {
    pub fn mode(&self) -> SyncMode {
        match self {
            Target::Study { mode, .. } | Target::New { mode, .. } => *mode,
        }
    }
}

/// What [`apply`] may do beyond the usual.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Remove on Lichess the chapters of linked games marked as deleted here
    /// (two-way). Off, they stay there.
    pub delete_chapters: bool,
}

/// What a sync would do: the preview.
#[derive(Debug, Clone)]
pub struct Plan {
    pub target: Target,
    /// The study's id (none for one [`apply`] makes).
    pub study: Option<String>,
    /// Its name.
    pub name: String,
    pub items: Vec<Item>,
    /// Chapters the study has room for, after the new ones here.
    pub room: usize,
    /// Games here outside the study that aren't listed (following a study
    /// with a database of more than [`MOST_UNLINKED`] other games).
    pub unlisted: usize,
}

impl Plan {
    pub fn count(&self, step: Step) -> usize {
        self.items.iter().filter(|i| i.step == step).count()
    }

    pub fn mode(&self) -> SyncMode {
        self.target.mode()
    }
}

/// What [`apply`] did.
#[derive(Debug, Default)]
pub struct Outcome {
    pub study: String,
    pub name: String,
    /// The study was made for this sync.
    pub made: bool,
    /// How many games or chapters each step was done for.
    pub done: HashMap<Step, usize>,
    /// Games left as they were, and why ("open in a tab", "changed on
    /// Lichess meanwhile").
    pub skipped: Vec<(String, String)>,
    /// Why the sync ended before the end, when it did: what was done before
    /// stays done.
    pub stopped: Option<NetError>,
    pub cancelled: bool,
}

impl Outcome {
    pub fn count(&self, step: Step) -> usize {
        self.done.get(&step).copied().unwrap_or(0)
    }

    fn did(&mut self, step: Step) {
        *self.done.entry(step).or_default() += 1;
    }

    fn skip(&mut self, name: &str, why: &str) {
        self.skipped.push((name.to_owned(), why.to_owned()));
    }
}

/// What a decision does, when it's applied.
#[derive(Debug, Clone)]
// A few dozen at most, for a study's 64 chapters: their sizes don't matter.
#[allow(clippy::large_enum_variant)]
enum Act {
    None,
    /// The game's link set (adopted, a pending merge, a base brought up to date).
    SetLink(StudyLink),
    /// The game becomes `game` (if it's still `ours`), then its link `link`.
    Pull {
        ours: Game,
        game: Game,
        link: StudyLink,
    },
    /// `send` goes to the chapter (if Lichess still has `theirs`), with the
    /// tag changes; the game becomes `local` first if it's given (a merge).
    Push {
        ours: Game,
        theirs: Game,
        send: Game,
        tags: Vec<(String, String)>,
        local: Option<Game>,
        link: StudyLink,
    },
    /// A new game here, linked.
    NewThere {
        game: Game,
        link: StudyLink,
    },
    /// A new chapter for this game.
    NewHere {
        game: Game,
    },
    /// Marked as deleted (if it's still `ours`) and unlinked.
    RemovedThere {
        ours: Game,
    },
    /// The chapter removed on Lichess, then the link.
    DeleteChapter,
    /// The link removed (the game is deleted here and its chapter there).
    Unlink,
}

#[derive(Debug, Clone)]
struct Decision {
    item: Item,
    act: Act,
}

/// A chapter of the study's export, read.
struct Ch<'a> {
    id: String,
    name: String,
    pgn: &'a str,
    game: Option<Game>,
}

/// The chapter id of a `ChapterURL` (or any link) of `study`.
fn chapter_of(url: &str, study: &str) -> Option<String> {
    let r = parse_study_link(url).ok()?;
    (r.study == study).then_some(r.chapter).flatten()
}

fn chapters<'a>(export: &'a StudyPgn, study: &str) -> Vec<Ch<'a>> {
    export
        .chapters
        .iter()
        .filter_map(|c| {
            let id = tag_value(&c.pgn, "ChapterURL").and_then(|u| chapter_of(&u, study))?;
            Some(Ch { id, name: c.name.clone(), pgn: &c.pgn, game: read_pgn_str(&c.pgn).into_iter().next() })
        })
        .collect()
}

/// A chapter with nothing in it: a new study's first one.
fn is_empty(g: &Game) -> bool {
    g.setup.is_none()
        && g.mainline_len() == 0
        && g.root().comment_after.trim().is_empty()
        && g.root().shapes.is_empty()
}

fn name_of(g: &Game) -> String {
    let h = &g.headers;
    if !h.white.is_empty() || !h.black.is_empty() {
        let or = |s: &str| if s.is_empty() { "?".to_owned() } else { s.to_owned() };
        format!("{} - {}", or(&h.white), or(&h.black))
    } else if let Some(name) = h.get_extra("ChapterName").filter(|n| !n.trim().is_empty()) {
        name.to_owned()
    } else if !h.event.is_empty() {
        h.event.clone()
    } else {
        "A game".to_owned()
    }
}

/// What the sync sends of a game: its projection, without its study tags
/// (they're the study's).
fn send_pgn(g: &Game) -> String {
    pgn_to_send(&lichess_projection(g))
}

/// A projection's PGN as the sync sends it: its movetext on one line.
/// Lichess's `…/moves` keeps the line breaks inside a comment that our
/// writer's wrapping at 80 columns puts there (seen on lichess.org,
/// 2026-11-16: "{ The⏎Italian. }"); a projection's comments have none of
/// their own (every run of spaces and line breaks is one space), so every
/// line break in its movetext is the writer's.
pub fn pgn_to_send(projection: &Game) -> String {
    let pgn = write_game(projection);
    match pgn.split_once("\n\n") {
        Some((tags, moves)) => format!("{tags}\n\n{}\n", moves.trim().replace('\n', " ")),
        None => pgn,
    }
}

fn item(uid: Option<GameUid>, chapter: Option<&str>, name: String, step: Step) -> Item {
    Item { uid, chapter: chapter.map(str::to_owned), name, step, note: None }
}

/// A linked game's link with a new base, its layer that of `game`.
fn relinked(link: &StudyLink, base: &str, game: &Game, last_modified: Option<&str>) -> StudyLink {
    StudyLink {
        base: base.to_owned(),
        layer: layer_bytes(game, None),
        last_modified: last_modified.map(str::to_owned).or_else(|| link.last_modified.clone()),
        ..link.clone()
    }
}

/// Decides what to do with every linked game of `study`, every chapter and
/// every game here outside the study. `export` is the study's (none for a
/// study yet to be made); `made`: the study was made for this sync (its empty
/// first chapter makes way for the games). Reads the database only.
#[allow(clippy::too_many_arguments)]
fn decide(
    db: &mut Database,
    study: Option<&str>,
    export: Option<&StudyPgn>,
    target: &Target,
    made: bool,
    options: Options,
    open: &[GameId],
) -> Result<(Vec<Decision>, usize, usize), NetError> {
    let now = now_ms();
    let mut out = Vec::new();
    let chs = match (export, study) {
        (Some(e), Some(s)) => chapters(e, s),
        _ => Vec::new(),
    };
    let last_modified = export.and_then(|e| e.last_modified.as_deref());
    let by_id: HashMap<&str, &Ch> = chs.iter().map(|c| (c.id.as_str(), c)).collect();
    let all_links = db.links()?;
    let mine: Vec<_> = all_links.iter().filter(|l| Some(l.study.as_str()) == study).cloned().collect();
    let elsewhere = all_links.iter().any(|l| Some(l.study.as_str()) != study);
    let mut held: HashSet<String> = HashSet::new();

    for l in &mine {
        held.insert(l.chapter.clone());
        let Some(link) = db.link(l.uid)? else { continue };
        let ours = db.read_game(l.game)?;
        let name = name_of(&ours);
        let deleted = db.record(l.game)?.is_deleted();
        let mk = |step| item(Some(l.uid), Some(&l.chapter), name.clone(), step);
        let (_, pending) = link_state(&link)?;
        if pending.is_some() {
            out.push(Decision { item: mk(Step::Pending), act: Act::None });
            continue;
        }
        let base = read_pgn_str(&link.base).into_iter().next().unwrap_or_default();
        let theirs = by_id.get(l.chapter.as_str());
        let Some(ch) = theirs else {
            // Removed on Lichess.
            let d = if deleted {
                Decision { item: mk(Step::DeletedHere), act: Act::Unlink }
            } else if !changed_here(&ours, &base) {
                if open.contains(&l.game) {
                    Decision { item: mk(Step::Open), act: Act::None }
                } else {
                    Decision { item: mk(Step::RemovedThere), act: Act::RemovedThere { ours } }
                }
            } else {
                let p = Pending { kind: PendingKind::RemovedThere, theirs: String::new(), found: now };
                let mut it = mk(Step::Conflict);
                it.note = Some("its chapter was removed on Lichess, and it changed here".into());
                let link = StudyLink { layer: layer_bytes(&ours, Some(&p)), ..link };
                Decision { item: it, act: Act::SetLink(link) }
            };
            out.push(d);
            continue;
        };
        let Some(their_game) = ch.game.clone() else {
            let mut it = mk(Step::Unchanged);
            it.note = Some("Lichess's chapter can't be read".into());
            out.push(Decision { item: it, act: Act::None });
            continue;
        };
        if deleted {
            let two_way = l.mode == SyncMode::TwoWay;
            let d = if two_way && options.delete_chapters {
                Decision { item: mk(Step::DeleteChapter), act: Act::DeleteChapter }
            } else {
                Decision { item: mk(Step::DeletedHere), act: Act::None }
            };
            out.push(d);
            continue;
        }
        let here = changed_here(&ours, &base);
        let there = !same_on_lichess(&their_game, &base);
        let d = match (here, there) {
            (false, false) => {
                // Lichess may have rewritten its export: it's the base.
                let act = if ch.pgn.trim() != link.base.trim() {
                    Act::SetLink(relinked(&link, ch.pgn, &ours, last_modified))
                } else {
                    Act::None
                };
                Decision { item: mk(Step::Unchanged), act }
            }
            (false, true) => {
                let m = merge::merge(&base, &ours, &their_game);
                let game = apply_layer(&m.game, &local_only_layer(&ours)).game;
                if open.contains(&l.game) {
                    Decision { item: mk(Step::Open), act: Act::None }
                } else {
                    let link = relinked(&link, ch.pgn, &game, last_modified);
                    Decision { item: mk(Step::Pull), act: Act::Pull { ours, game, link } }
                }
            }
            (true, false) if l.mode == SyncMode::Follow => {
                Decision { item: mk(Step::KeptHere), act: Act::None }
            }
            (true, false) => {
                let send = lichess_projection(&ours);
                let tags = tag_changes(&ours, &their_game);
                let act = Act::Push { ours, theirs: their_game, send, tags, local: None, link };
                Decision { item: mk(Step::Push), act }
            }
            (true, true) => {
                let m = merge::merge(&base, &ours, &their_game);
                let questions = m.questions();
                if !questions.is_empty() {
                    let p = Pending { kind: PendingKind::Both, theirs: ch.pgn.to_owned(), found: now };
                    let mut it = mk(Step::Conflict);
                    it.note = Some(questions.iter().map(Conflict::describe).collect::<Vec<_>>().join("; "));
                    let link = StudyLink { layer: layer_bytes(&ours, Some(&p)), ..link };
                    Decision { item: it, act: Act::SetLink(link) }
                } else {
                    let local = apply_layer(&m.game, &local_only_layer(&ours)).game;
                    let changes_here = local != ours;
                    if changes_here && open.contains(&l.game) {
                        Decision { item: mk(Step::Open), act: Act::None }
                    } else if l.mode == SyncMode::Follow || !changed_here(&local, &their_game) {
                        // Nothing for Lichess: the merge comes here only.
                        let link = relinked(&link, ch.pgn, &local, last_modified);
                        let act = Act::Pull { ours, game: local, link };
                        Decision { item: mk(Step::Merge), act }
                    } else {
                        let send = lichess_projection(&local);
                        let tags = tag_changes(&local, &their_game);
                        let local = changes_here.then_some(local);
                        let act = Act::Push { ours, theirs: their_game, send, tags, local, link };
                        Decision { item: mk(Step::Merge), act }
                    }
                }
            }
        };
        out.push(d);
    }

    // Games here outside the study, live ones.
    let linked: HashSet<GameUid> = all_links.iter().map(|l| l.uid).collect();
    let count = db.count();
    let uids = db.uids(0, count)?;
    let mut outside: Vec<(GameId, GameUid)> = Vec::new();
    let mut start = 0;
    while start < count {
        for (i, r) in db.records(start, 1 << 16)?.iter().enumerate() {
            let slot = start + i as GameId;
            let uid = uids[slot as usize];
            if !r.is_deleted() && !linked.contains(&uid) {
                outside.push((slot, uid));
            }
        }
        start = start.saturating_add(1 << 16);
    }
    let two_way = target.mode() == SyncMode::TwoWay;
    let mut unlisted = 0;
    if outside.len() > MOST_UNLINKED {
        if two_way {
            return Err(NetError::TooManyForStudy { games: outside.len() });
        }
        unlisted = outside.len();
        outside.clear();
    }
    let mut games: Vec<(GameId, GameUid, Game)> = Vec::with_capacity(outside.len());
    for (slot, uid) in outside {
        games.push((slot, uid, db.read_game(slot)?));
    }
    let mut used: HashSet<GameUid> = HashSet::new();

    // Chapters with no game here.
    for ch in chs.iter().filter(|c| !held.contains(&c.id)) {
        let Some(theirs) = &ch.game else { continue };
        let study = study.expect("chapters come with a study");
        let new_link = |game: &Game, pending: Option<&Pending>| StudyLink {
            study: study.to_owned(),
            chapter: ch.id.clone(),
            mode: target.mode(),
            last_modified: last_modified.map(str::to_owned),
            base: ch.pgn.to_owned(),
            layer: layer_bytes(game, pending),
        };
        // A game here that is this chapter: by its ChapterURL, or the same.
        let by_url = games.iter().find(|(_, u, g)| {
            !used.contains(u)
                && g.headers.get_extra("ChapterURL").and_then(|url| chapter_of(url, study)).as_deref()
                    == Some(ch.id.as_str())
        });
        let same = || games.iter().find(|(_, u, g)| !used.contains(u) && !changed_here(g, theirs));
        if let Some((_, uid, g)) = by_url.or_else(same) {
            used.insert(*uid);
            let name = name_of(g);
            let d = if !changed_here(g, theirs) {
                let it = item(Some(*uid), Some(&ch.id), name, Step::Adopt);
                Decision { item: it, act: Act::SetLink(new_link(g, None)) }
            } else {
                let p = Pending { kind: PendingKind::NoBase, theirs: ch.pgn.to_owned(), found: now };
                let mut it = item(Some(*uid), Some(&ch.id), name, Step::Conflict);
                it.note =
                    Some("this game is the chapter but differs from it, and they were never synced".into());
                Decision { item: it, act: Act::SetLink(new_link(g, Some(&p))) }
            };
            out.push(d);
            continue;
        }
        if is_empty(theirs) {
            continue;
        }
        let name = if ch.name.trim().is_empty() { name_of(theirs) } else { ch.name.clone() };
        let link = new_link(theirs, None);
        let act = Act::NewThere { game: theirs.clone(), link };
        out.push(Decision { item: item(None, Some(&ch.id), name, Step::NewThere), act });
    }

    // Games here the study doesn't have.
    let mut room = MAX_CHAPTERS.saturating_sub(chs.len());
    if made && chs.len() == 1 && chs[0].game.as_ref().is_some_and(is_empty) {
        room = MAX_CHAPTERS;
    }
    for (_, uid, g) in games.into_iter().filter(|(_, u, _)| !used.contains(u)) {
        let name = name_of(&g);
        let mut it = item(Some(uid), None, name, Step::NotSent);
        let act = if g.headers.get_extra(CONFLICT_COPY_TAG).is_some() {
            it.note = Some("a copy kept from a conflict".into());
            Act::None
        } else if !two_way {
            it.note = Some("following only".into());
            Act::None
        } else if elsewhere {
            it.note = Some("the database is linked to other studies too".into());
            Act::None
        } else if room == 0 {
            it.step = Step::NoRoom;
            Act::None
        } else {
            room -= 1;
            it.step = Step::NewHere;
            Act::NewHere { game: g }
        };
        out.push(Decision { item: it, act });
    }
    Ok((out, room, unlisted))
}

/// The study's export, fetched (a Cancel takes effect at once).
fn fetch_export(
    client: &Client,
    study: &str,
    token: Option<&Token>,
    progress: &dyn Progress,
) -> Result<StudyPgn, NetError> {
    let (c, l, t) = (client.clone(), StudyRef { study: study.to_owned(), chapter: None }, token.cloned());
    cancellable(progress, move |stop| fetch_study(&c, &l, t.as_ref(), None, stop))?
        .ok_or_else(|| NetError::BadData { site: Site::Lichess, reason: "a 304 unasked".into() })
}

/// A two-way sync needs a login that can write studies: checked before
/// anything is sent.
fn check_can_write(client: &Client, token: Option<&Token>) -> Result<(), NetError> {
    let token = token.ok_or(NetError::SyncNeedsLogin)?;
    let scopes = token_scopes(&client.for_lookup(), token)?;
    if !scopes.iter().any(|s| s == "study:write") {
        return Err(NetError::MissingScopes("study:write".into()));
    }
    Ok(())
}

/// Looks at both sides and says what a sync with `target` would do; changes
/// nothing. `open`: the games open in a tab. A two-way sync's login is
/// checked first (it must be able to write studies).
pub fn plan(
    client: &Client,
    token: Option<&Token>,
    db: &Mutex<Database>,
    target: &Target,
    open: &dyn Fn() -> Vec<GameId>,
    progress: &dyn Progress,
) -> Result<Plan, NetError> {
    if target.mode() == SyncMode::TwoWay {
        check_can_write(client, token)?;
    }
    let (study, name, export) = match target {
        Target::Study { study, .. } => {
            if !is_lichess_id(study) {
                return Err(NetError::NoSuchStudy);
            }
            let export = fetch_export(client, study, token, progress)?;
            (Some(study.clone()), export.name.clone(), Some(export))
        }
        Target::New { name, .. } => (None, check_study_name(name)?.to_owned(), None),
    };
    let mut db = db.lock().expect("db lock");
    let open = open();
    let (decisions, room, unlisted) =
        decide(&mut db, study.as_deref(), export.as_ref(), target, false, Options::default(), &open)?;
    Ok(Plan {
        target: target.clone(),
        study,
        name,
        items: decisions.into_iter().map(|d| d.item).collect(),
        room,
        unlisted,
    })
}

// ---- Writes -------------------------------------------------------------------------

/// A write's answer as an error, if it's one.
fn write_result(resp: &mut ureq::http::Response<ureq::Body>) -> Result<(), NetError> {
    match resp.status().as_u16() {
        200..=299 => Ok(()),
        429 => Err(NetError::RateLimited(Site::Lichess)),
        401 => Err(NetError::TokenRejected),
        403 => {
            let why = error_message(Site::Lichess, resp).unwrap_or_default();
            if why.to_ascii_lowercase().contains("scope") {
                Err(NetError::MissingScopes("study:write".into()))
            } else {
                Err(NetError::NotContributor)
            }
        }
        404 => Err(NetError::NoSuchStudy),
        400 => {
            let why = error_message(Site::Lichess, resp).unwrap_or_else(|| "it didn't say why".into());
            Err(NetError::Refused(why))
        }
        _ => Err(status_error(Site::Lichess, resp)),
    }
}

fn post(
    client: &Client,
    token: &Token,
    url: &str,
    fields: &[(&str, &str)],
    stop: &dyn Fn() -> bool,
) -> Result<ureq::http::Response<ureq::Body>, NetError> {
    client.write_retrying(Site::Lichess, stop, &|| client.post_form(Site::Lichess, url, Some(token), fields))
}

/// Replaces a chapter's moves (its tags stay), waiting out a 429.
fn push_moves(
    client: &Client,
    token: &Token,
    study: &str,
    chapter: &str,
    pgn: &str,
    stop: &dyn Fn() -> bool,
) -> Result<(), NetError> {
    if !is_lichess_id(study) || !is_lichess_id(chapter) {
        return Err(NetError::NoSuchStudy);
    }
    if encoded_len(pgn) > MAX_BODY {
        return Err(NetError::Refused(format!(
            "the game is over {} kB, more than Lichess takes",
            MAX_BODY >> 10
        )));
    }
    let url = format!("{}/api/study/{study}/{chapter}/moves", client.lichess);
    write_result(&mut post(client, token, &url, &[("pgn", pgn)], stop)?)
}

/// Sets a chapter's tags (an empty value removes one), waiting out a 429.
fn push_tags(
    client: &Client,
    token: &Token,
    study: &str,
    chapter: &str,
    tags: &[(String, String)],
    stop: &dyn Fn() -> bool,
) -> Result<(), NetError> {
    if tags.is_empty() {
        return Ok(());
    }
    let text: String = tags
        .iter()
        .map(|(k, v)| format!("[{k} \"{}\"]\n", v.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    let url = format!("{}/api/study/{study}/{chapter}/tags", client.lichess);
    write_result(&mut post(client, token, &url, &[("pgn", &text)], stop)?)
}

/// Removes a chapter, waiting out a 429. One already gone is gone.
fn remove_chapter(
    client: &Client,
    token: &Token,
    study: &str,
    chapter: &str,
    stop: &dyn Fn() -> bool,
) -> Result<(), NetError> {
    if !is_lichess_id(study) || !is_lichess_id(chapter) {
        return Err(NetError::NoSuchStudy);
    }
    let url = format!("{}/api/study/{study}/{chapter}", client.lichess);
    let mut resp =
        client.write_retrying(Site::Lichess, stop, &|| client.delete(Site::Lichess, &url, token))?;
    if resp.status().as_u16() == 404 {
        return Ok(());
    }
    write_result(&mut resp)
}

/// Sends games to a study, each a chapter (`initial`: the empty first
/// chapter of a study made for them goes), waiting out a 429. The ids of
/// the chapters made, in order, and what Lichess said of games it didn't
/// take.
fn import_chapters(
    client: &Client,
    token: &Token,
    study: &str,
    pgn: &str,
    initial: bool,
    stop: &dyn Fn() -> bool,
) -> Result<(Vec<String>, Option<String>), NetError> {
    #[derive(Deserialize)]
    struct Made {
        #[serde(default)]
        chapters: Vec<MadeChapter>,
        error: Option<String>,
    }
    #[derive(Deserialize)]
    struct MadeChapter {
        id: String,
    }
    let url = format!("{}/api/study/{study}/import-pgn", client.lichess);
    let mut fields = vec![("pgn", pgn)];
    if initial {
        fields.push(("initial", "true"));
    }
    let mut resp = post(client, token, &url, &fields, stop)?;
    write_result(&mut resp)?;
    let made: Made = read_json(Site::Lichess, &mut resp, 4 << 20)?;
    let ids: Vec<String> = made.chapters.into_iter().map(|c| c.id).filter(|id| is_lichess_id(id)).collect();
    Ok((ids, made.error.filter(|e| !e.trim().is_empty())))
}

// ---- Applying a plan ----------------------------------------------------------------

/// A game sent to a chapter, waiting for its new base.
struct Sent {
    uid: GameUid,
    chapter: String,
    /// The game here as it is now.
    game: Game,
    /// What was sent.
    sent: Game,
    sent_pgn: String,
    /// Its link before, or none for a new chapter.
    link: Option<StudyLink>,
    step: Step,
}

/// Runs `f` on the database, if `slot`'s game is still `ours` and isn't
/// open in a tab; `Ok(false)` if it isn't.
fn if_unchanged(
    db: &Mutex<Database>,
    uid: GameUid,
    ours: &Game,
    open: &dyn Fn() -> Vec<GameId>,
    f: impl FnOnce(&mut Database, GameId) -> Result<(), DbError>,
) -> Result<Result<(), &'static str>, NetError> {
    let mut db = db.lock().expect("db lock");
    let Some(slot) = db.game_by_uid(uid)? else { return Ok(Err("it's gone from the database")) };
    if open().contains(&slot) {
        return Ok(Err("it's open in a tab: close it and sync again"));
    }
    if db.read_game(slot)? != *ours {
        return Ok(Err("it changed here meanwhile: sync again"));
    }
    f(&mut db, slot)?;
    Ok(Ok(()))
}

/// Does what `plan` previewed, as it is now: Lichess is asked again for the
/// study and both sides are looked at again, so what changed since the
/// preview is taken into account (never overwritten). A new study is made
/// first. Games open in a tab aren't changed. A failure partway (the
/// network, Lichess's limits) ends it there; what was done before stays
/// done, and the summary says why it stopped.
pub fn apply(
    client: &Client,
    token: Option<&Token>,
    db: &Mutex<Database>,
    plan: &Plan,
    options: Options,
    open: &dyn Fn() -> Vec<GameId>,
    progress: &dyn Progress,
) -> Result<Outcome, NetError> {
    let target = &plan.target;
    let two_way = target.mode() == SyncMode::TwoWay;
    if two_way {
        check_can_write(client, token)?;
    }
    let mut outcome = Outcome { name: plan.name.clone(), ..Outcome::default() };
    let (study, made) = match target {
        Target::Study { study, .. } => (study.clone(), false),
        Target::New { name, visibility, .. } => {
            let token = token.ok_or(NetError::SyncNeedsLogin)?;
            (create_study(client, token, check_study_name(name)?, *visibility)?, true)
        }
    };
    outcome.study = study.clone();
    outcome.made = made;
    let export = match fetch_export(client, &study, token, progress) {
        Ok(e) => e,
        // The study was made: say so, rather than fail as if nothing happened.
        Err(e) if made => {
            outcome.stopped = Some(e);
            return Ok(outcome);
        }
        Err(e) => return Err(e),
    };
    if !made {
        outcome.name = export.name.clone();
    }
    let decisions = {
        let mut guard = db.lock().expect("db lock");
        let open_now = open();
        decide(&mut guard, Some(&study), Some(&export), target, made, options, &open_now)?.0
    };
    let total = decisions.iter().filter(|d| !matches!(d.act, Act::None)).count() as u64 + 1;
    let mut done = 0u64;
    let mut tick = |progress: &dyn Progress| {
        done += 1;
        progress.report(done, total);
    };

    // Here only, first: links, new games, pulled games, games removed there.
    let mut links: Vec<(GameUid, StudyLink)> = Vec::new();
    let mut new_games: Vec<PreparedGame> = Vec::new();
    for d in &decisions {
        let Some(uid) = d.item.uid else {
            if let Act::NewThere { game, link } = &d.act {
                new_games.push(PreparedGame::new(game).with_link(link.clone()));
            }
            continue;
        };
        match &d.act {
            Act::SetLink(link) => links.push((uid, link.clone())),
            Act::Unlink => {
                db.lock().expect("db lock").remove_link(uid)?;
                outcome.did(d.item.step);
                tick(progress);
            }
            _ => {}
        }
    }
    if !links.is_empty() {
        db.lock().expect("db lock").set_links(&links)?;
        for d in decisions.iter().filter(|d| matches!(d.act, Act::SetLink(_))) {
            if d.item.step != Step::Unchanged {
                outcome.did(d.item.step);
            }
            tick(progress);
        }
    }
    if !new_games.is_empty() {
        db.lock().expect("db lock").append_prepared(&new_games)?;
        for _ in 0..new_games.len() {
            outcome.did(Step::NewThere);
            tick(progress);
        }
    }
    for d in &decisions {
        let uid = match (d.item.uid, &d.act) {
            (Some(uid), Act::Pull { .. } | Act::RemovedThere { .. }) => uid,
            _ => continue,
        };
        if progress.is_cancelled() {
            outcome.cancelled = true;
            return Ok(outcome);
        }
        let result = match &d.act {
            Act::Pull { ours, game, link } => if_unchanged(db, uid, ours, open, |db, slot| {
                db.replace(slot, game)?;
                db.set_link(uid, link)
            })?,
            Act::RemovedThere { ours } => if_unchanged(db, uid, ours, open, |db, slot| {
                db.set_deleted(slot, true)?;
                db.remove_link(uid).map(|_| ())
            })?,
            _ => unreachable!("pulls and removals"),
        };
        match result {
            Ok(()) => outcome.did(d.item.step),
            Err(why) => outcome.skip(&d.item.name, why),
        }
        tick(progress);
    }

    // Then Lichess: removed chapters, pushes, new chapters.
    let Some(token) = token.filter(|_| two_way) else {
        progress.report(total, total);
        return Ok(outcome);
    };
    let stop = || progress.is_cancelled();
    let mut sent: Vec<Sent> = Vec::new();
    let mut new_here: Vec<(GameUid, Game)> = Vec::new();
    'net: for d in &decisions {
        if progress.is_cancelled() {
            outcome.cancelled = true;
            break;
        }
        let Some(uid) = d.item.uid else { continue };
        match &d.act {
            Act::DeleteChapter => {
                let chapter = d.item.chapter.as_deref().unwrap_or_default();
                match remove_chapter(client, token, &study, chapter, &stop) {
                    Ok(()) => {
                        db.lock().expect("db lock").remove_link(uid)?;
                        outcome.did(Step::DeleteChapter);
                    }
                    Err(NetError::Cancelled) => {
                        outcome.cancelled = true;
                        break 'net;
                    }
                    Err(e) => {
                        outcome.stopped = Some(e);
                        break 'net;
                    }
                }
                tick(progress);
            }
            Act::Push { ours, theirs, send, tags, local, link } => {
                let chapter = link.chapter.clone();
                // Lichess must still have what was decided on.
                let now = cancellable(progress, {
                    let (c, t) = (client.clone(), Some(token.clone()));
                    let l = StudyRef { study: study.clone(), chapter: Some(chapter.clone()) };
                    move |stop| fetch_study(&c, &l, t.as_ref(), None, stop)
                });
                let still = match now {
                    Ok(Some(s)) => s
                        .chapters
                        .first()
                        .and_then(|c| read_pgn_str(&c.pgn).into_iter().next())
                        .is_some_and(|g| same_on_lichess(&g, theirs)),
                    Ok(None) => false,
                    Err(NetError::Cancelled) => {
                        outcome.cancelled = true;
                        break 'net;
                    }
                    Err(NetError::NoSuchStudy) => false,
                    Err(e) => {
                        outcome.stopped = Some(e);
                        break 'net;
                    }
                };
                if !still {
                    outcome.skip(&d.item.name, "it changed on Lichess meanwhile: sync again");
                    tick(progress);
                    continue;
                }
                let game = match local {
                    Some(local) => {
                        match if_unchanged(db, uid, ours, open, |db, slot| db.replace(slot, local))? {
                            Ok(()) => local.clone(),
                            Err(why) => {
                                outcome.skip(&d.item.name, why);
                                tick(progress);
                                continue;
                            }
                        }
                    }
                    None => ours.clone(),
                };
                let pgn = pgn_to_send(send);
                let pushed = push_moves(client, token, &study, &chapter, &pgn, &stop)
                    .and_then(|()| push_tags(client, token, &study, &chapter, tags, &stop));
                match pushed {
                    Ok(()) => sent.push(Sent {
                        uid,
                        chapter,
                        game,
                        sent: send.clone(),
                        sent_pgn: pgn,
                        link: Some(link.clone()),
                        step: d.item.step,
                    }),
                    Err(NetError::Cancelled) => {
                        outcome.cancelled = true;
                        break 'net;
                    }
                    Err(e) => {
                        outcome.stopped = Some(e);
                        break 'net;
                    }
                }
                tick(progress);
            }
            Act::NewHere { game } => new_here.push((uid, game.clone())),
            _ => {}
        }
    }

    // New chapters, in batches Lichess takes in one request.
    if outcome.stopped.is_none() && !outcome.cancelled && !new_here.is_empty() {
        let mut first = made;
        let mut i = 0;
        while i < new_here.len() {
            if progress.is_cancelled() {
                outcome.cancelled = true;
                break;
            }
            let (mut pgn, mut n) = (String::new(), 0);
            while i + n < new_here.len() {
                let text = send_pgn(&new_here[i + n].1);
                let text = text.trim();
                if encoded_len(text) + 3 > MAX_BODY {
                    if n == 0 {
                        outcome.skip(&name_of(&new_here[i].1), "it's too long for Lichess (over 400 kB)");
                        i += 1;
                    }
                    break;
                }
                if !pgn.is_empty() && encoded_len(&pgn) + encoded_len(text) + 9 > MAX_BODY {
                    break;
                }
                if !pgn.is_empty() {
                    pgn.push_str("\n\n\n");
                }
                pgn.push_str(text);
                n += 1;
            }
            if n == 0 {
                continue;
            }
            match import_chapters(client, token, &study, &pgn, first, &stop) {
                Ok((ids, refused)) => {
                    first = false;
                    let batch = &new_here[i..i + n];
                    if ids.len() == batch.len() {
                        for ((uid, game), id) in batch.iter().zip(ids) {
                            let projection = lichess_projection(game);
                            sent.push(Sent {
                                uid: *uid,
                                chapter: id,
                                game: game.clone(),
                                sent_pgn: pgn_to_send(&projection),
                                sent: projection,
                                link: None,
                                step: Step::NewHere,
                            });
                        }
                    } else {
                        // Some weren't taken: which, the export says (the
                        // next sync links those that came by their moves).
                        let why = refused.unwrap_or_else(|| "Lichess didn't take it".into());
                        for (_, game) in batch {
                            outcome.skip(&name_of(game), &why);
                        }
                    }
                }
                Err(NetError::Cancelled) => {
                    outcome.cancelled = true;
                    break;
                }
                Err(e) => {
                    outcome.stopped = Some(e);
                    break;
                }
            }
            i += n;
            for _ in 0..n {
                tick(progress);
            }
        }
    }

    // The new bases: Lichess's own export of what was sent.
    if !sent.is_empty() {
        let export =
            if outcome.cancelled { None } else { fresh_export(client, token, &study, &sent, progress) };
        let fresh: HashMap<String, (String, Option<Game>)> = export
            .iter()
            .flat_map(|e| {
                chapters(e, &study)
                    .into_iter()
                    .map(|c| (c.id.clone(), (c.pgn.to_owned(), c.game)))
                    .collect::<Vec<_>>()
            })
            .collect();
        let last_modified = export.as_ref().and_then(|e| e.last_modified.clone());
        let mut links: Vec<(GameUid, StudyLink)> = Vec::new();
        for s in &sent {
            // Lichess's export if it shows what was sent, else what was sent
            // (the next sync takes Lichess's export then).
            let base = match fresh.get(&s.chapter) {
                Some((pgn, Some(g))) if !changed_here(&s.sent, g) => pgn.clone(),
                _ => s.sent_pgn.clone(),
            };
            let link = match &s.link {
                Some(l) => relinked(l, &base, &s.game, last_modified.as_deref()),
                None => StudyLink {
                    study: study.clone(),
                    chapter: s.chapter.clone(),
                    mode: target.mode(),
                    last_modified: last_modified.clone(),
                    base,
                    layer: layer_bytes(&s.game, None),
                },
            };
            links.push((s.uid, link));
        }
        let mut guard = db.lock().expect("db lock");
        // A game deleted meanwhile, or gone: its chapter stays unlinked.
        let mut kept = Vec::with_capacity(links.len());
        for (uid, link) in links {
            if guard.game_by_uid(uid)?.is_some() {
                kept.push((uid, link));
            }
        }
        guard.set_links(&kept)?;
        drop(guard);
        for s in &sent {
            outcome.did(s.step);
        }
    }
    progress.report(total, total);
    Ok(outcome)
}

/// The study's export once it shows what was sent (Lichess caches a public
/// study's for a few seconds): asked again a few times, then taken as it
/// is. `None` if it can't be fetched (the bases are then what was sent).
fn fresh_export(
    client: &Client,
    token: &Token,
    study: &str,
    sent: &[Sent],
    progress: &dyn Progress,
) -> Option<StudyPgn> {
    let mut last = None;
    for attempt in 0..=FRESH_TRIES {
        let export = match fetch_export(client, study, Some(token), progress) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("couldn't fetch {study} after a sync's pushes: {e}");
                return last;
            }
        };
        let chs = chapters(&export, study);
        let shows = sent.iter().all(|s| {
            chs.iter()
                .find(|c| c.id == s.chapter)
                .and_then(|c| c.game.as_ref())
                .is_some_and(|g| !changed_here(&s.sent, g))
        });
        last = Some(export);
        if shows || attempt == FRESH_TRIES {
            break;
        }
        let until = Instant::now() + client.fresh_wait;
        while Instant::now() < until {
            if progress.is_cancelled() {
                return last;
            }
            std::thread::sleep(std::time::Duration::from_millis(50).min(client.fresh_wait));
        }
    }
    last
}

// ---- Linked studies, and games waiting for the user -------------------------------------

/// A study the database is linked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedStudy {
    pub study: String,
    /// Its name, from a base snapshot's `StudyName` (none if no base has one).
    pub name: Option<String>,
    pub mode: SyncMode,
    /// Its linked games.
    pub games: usize,
    /// Those waiting for the user: (game, slot, name, kind).
    pub pending: Vec<(GameUid, GameId, String, PendingKind)>,
}

/// The studies the database is linked to, by how many games each has.
pub fn linked_studies(db: &mut Database) -> Result<Vec<LinkedStudy>, NetError> {
    let mut out: Vec<LinkedStudy> = Vec::new();
    for l in db.links()? {
        let at = match out.iter().position(|s| s.study == l.study) {
            Some(i) => i,
            None => {
                out.push(LinkedStudy {
                    study: l.study.clone(),
                    name: None,
                    mode: l.mode,
                    games: 0,
                    pending: Vec::new(),
                });
                out.len() - 1
            }
        };
        let game = db.read_game(l.game)?;
        let s = &mut out[at];
        s.games += 1;
        let Some(link) = db.link(l.uid)? else { continue };
        // The base is Lichess's export: its StudyName is the study's name.
        if s.name.is_none() {
            s.name = tag_value(&link.base, "StudyName").filter(|n| !n.trim().is_empty());
        }
        if let Ok((_, Some(p))) = link_state(&link) {
            s.pending.push((l.uid, l.game, name_of(&game), p.kind));
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.games));
    Ok(out)
}

/// The study a database of a static copy came from (IO-10's import, its
/// games' `ChapterURL`), to link it to: the one most of its first games say.
pub fn copied_from(db: &Database) -> Result<Option<(String, Option<String>)>, NetError> {
    let mut seen: HashMap<String, (usize, Option<String>)> = HashMap::new();
    for slot in 0..db.count().min(200) {
        let g = db.read_game(slot)?;
        if let Some(r) = g.headers.get_extra("ChapterURL").and_then(|u| parse_study_link(u).ok()) {
            let e = seen.entry(r.study).or_insert((0, None));
            e.0 += 1;
            if e.1.is_none() {
                e.1 = g.headers.get_extra("StudyName").map(str::to_owned);
            }
        }
    }
    Ok(seen.into_iter().max_by_key(|(_, (n, _))| *n).map(|(s, (_, name))| (s, name)))
}

/// Sets how the database follows `study`: every link's mode. Returns how
/// many links there were.
pub fn set_mode(db: &mut Database, study: &str, mode: SyncMode) -> Result<usize, NetError> {
    let mut links = Vec::new();
    for l in db.links_of_study(study)? {
        if let Some(link) = db.link(l.uid)? {
            links.push((l.uid, StudyLink { mode, ..link }));
        }
    }
    db.set_links(&links)?;
    Ok(links.len())
}

/// Unlinks the database from `study`: its games stay as they are (a static
/// copy). Returns how many were linked.
pub fn unlink(db: &mut Database, study: &str) -> Result<u32, NetError> {
    let uids: Vec<GameUid> = db.links_of_study(study)?.into_iter().map(|l| l.uid).collect();
    Ok(db.remove_links(&uids)?)
}

/// A game waiting for the user, as the resolve dialog shows it.
#[derive(Debug, Clone)]
pub struct Waiting {
    pub uid: GameUid,
    pub name: String,
    pub study: String,
    pub chapter: String,
    pub mode: SyncMode,
    pub pending: Pending,
    /// What's in conflict (none for a chapter removed on Lichess).
    pub questions: Vec<Conflict>,
    /// Each side's main line in SAN ("1.e4 e5 2.Nf3"), and its moves and
    /// comments.
    pub mine: Side,
    pub theirs: Option<Side>,
}

/// A side of a conflict, in short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub line: String,
    pub moves: usize,
    pub comments: usize,
}

fn side(g: &Game) -> Side {
    let mut pos = g.start_position();
    let mut parts = Vec::new();
    let mut id = chess_core::game::NodeId::ROOT;
    while let Some(&next) = g.node(id).children.first() {
        let mv = g.node(next).mv.expect("non-root node has a move");
        let n = pos.fullmoves().get();
        let text = san(&pos, mv);
        parts.push(match (pos.turn().is_white(), parts.is_empty()) {
            (true, _) => format!("{n}.{text}"),
            (false, true) => format!("{n}...{text}"),
            (false, false) => text,
        });
        pos.play_unchecked(mv);
        id = next;
    }
    let ids = g.preorder();
    Side {
        line: parts.join(" "),
        moves: ids.len() - 1,
        comments: ids.iter().filter(|&&i| !g.node(i).comment_after.trim().is_empty()).count(),
    }
}

/// The game `uid` if it waits for the user.
pub fn waiting(db: &mut Database, uid: GameUid) -> Result<Option<Waiting>, NetError> {
    let Some(slot) = db.game_by_uid(uid)? else { return Ok(None) };
    let Some(link) = db.link(uid)? else { return Ok(None) };
    let (_, Some(pending)) = link_state(&link)? else { return Ok(None) };
    let ours = db.read_game(slot)?;
    let theirs = read_pgn_str(&pending.theirs).into_iter().next();
    let questions = match (&theirs, pending.kind) {
        (Some(t), PendingKind::Both) => {
            let base = read_pgn_str(&link.base).into_iter().next().unwrap_or_default();
            merge::merge(&base, &ours, t).questions()
        }
        _ => Vec::new(),
    };
    Ok(Some(Waiting {
        uid,
        name: name_of(&ours),
        study: link.study.clone(),
        chapter: link.chapter.clone(),
        mode: link.mode,
        mine: side(&ours),
        theirs: theirs.as_ref().map(side),
        questions,
        pending,
    }))
}

/// A game marked as a copy kept from a conflict over `link`'s chapter.
fn marked_copy(game: &Game, link: &StudyLink) -> Game {
    let mut g = game.clone();
    let url = StudyRef { study: link.study.clone(), chapter: Some(link.chapter.clone()) }.link();
    g.headers.extra.retain(|(k, _)| k != CONFLICT_COPY_TAG);
    g.headers.extra.push((CONFLICT_COPY_TAG.to_owned(), url));
    g
}

/// How the user settles a game waiting for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// This database's game: it goes to Lichess (two-way), Lichess's is gone
    /// there. Following only, it stays here as it is.
    Mine,
    /// Lichess's: it comes here (what only the database holds is kept where
    /// its moves still are). A chapter removed on Lichess: the game is
    /// marked as deleted here.
    Theirs,
    /// Lichess's comes here as for Theirs, and this database's is kept as a
    /// new game, marked as a copy ([`CONFLICT_COPY_TAG`]), which no sync
    /// sends. A chapter removed on Lichess: the game is kept here, unlinked
    /// and marked.
    Both,
}

/// Settles a game waiting for the user (see [`Choice`]). Mine, two-way,
/// goes to Lichess only if the chapter is still what was found: if it
/// changed again, the game keeps waiting with Lichess's new version, and
/// the answer says so. Refused for a game open in a tab. Returns what was
/// done, in a sentence.
pub fn resolve(
    client: &Client,
    token: Option<&Token>,
    db: &Mutex<Database>,
    uid: GameUid,
    choice: Choice,
    open: &dyn Fn() -> Vec<GameId>,
    progress: &dyn Progress,
) -> Result<String, NetError> {
    let (link, pending, ours, name) = {
        let mut guard = db.lock().expect("db lock");
        let slot = guard.game_by_uid(uid)?.ok_or_else(|| NetError::Refused("the game is gone".into()))?;
        if open().contains(&slot) {
            return Err(NetError::Refused("the game is open in a tab: close it first".into()));
        }
        let link =
            guard.link(uid)?.ok_or_else(|| NetError::Refused("the game isn't linked any more".into()))?;
        let (_, pending) = link_state(&link)?;
        let pending =
            pending.ok_or_else(|| NetError::Refused("the game doesn't wait for you any more".into()))?;
        let ours = guard.read_game(slot)?;
        let name = name_of(&ours);
        (link, pending, ours, name)
    };
    let theirs = read_pgn_str(&pending.theirs).into_iter().next();
    let unchanged =
        |f: &mut dyn FnMut(&mut Database, GameId) -> Result<(), DbError>| -> Result<(), NetError> {
            match if_unchanged(db, uid, &ours, open, |db, slot| f(db, slot))? {
                Ok(()) => Ok(()),
                Err(why) => Err(NetError::Refused(why.into())),
            }
        };
    let copy = |db: &mut Database| db.append(&marked_copy(&ours, &link)).map(|_| ());
    match (pending.kind, choice) {
        (PendingKind::RemovedThere, Choice::Theirs) => {
            unchanged(&mut |db, slot| {
                db.set_deleted(slot, true)?;
                db.remove_link(uid).map(|_| ())
            })?;
            Ok(format!("{name} is marked as deleted here, as its chapter is on Lichess"))
        }
        (PendingKind::RemovedThere, Choice::Both) => {
            unchanged(&mut |db, slot| {
                db.remove_link(uid)?;
                db.replace(slot, &marked_copy(&ours, &link))
            })?;
            Ok(format!("{name} stays here, unlinked, marked as a copy kept from a conflict"))
        }
        (PendingKind::RemovedThere, Choice::Mine) if link.mode == SyncMode::TwoWay => {
            let token = token.ok_or(NetError::SyncNeedsLogin)?;
            let stop = || progress.is_cancelled();
            let (ids, refused) =
                import_chapters(client, token, &link.study, send_pgn(&ours).trim(), false, &stop)?;
            let Some(id) = ids.into_iter().next() else {
                return Err(NetError::Refused(
                    refused.unwrap_or_else(|| "Lichess didn't take the game".into()),
                ));
            };
            let projection = lichess_projection(&ours);
            let s = Sent {
                uid,
                chapter: id.clone(),
                game: ours.clone(),
                sent_pgn: pgn_to_send(&projection),
                sent: projection,
                link: None,
                step: Step::NewHere,
            };
            let export = fresh_export(client, token, &link.study, std::slice::from_ref(&s), progress);
            let base = export
                .as_ref()
                .and_then(|e| {
                    chapters(e, &link.study).into_iter().find(|c| c.id == id).map(|c| c.pgn.to_owned())
                })
                .unwrap_or(s.sent_pgn);
            let new = StudyLink { chapter: id, base, layer: layer_bytes(&ours, None), ..link.clone() };
            db.lock().expect("db lock").set_link(uid, &new)?;
            Ok(format!("{name} went to the study again as a new chapter"))
        }
        (PendingKind::RemovedThere, Choice::Mine) => {
            db.lock().expect("db lock").remove_link(uid)?;
            Ok(format!("{name} stays here, unlinked"))
        }
        (_, Choice::Mine) if link.mode == SyncMode::TwoWay => {
            let token = token.ok_or(NetError::SyncNeedsLogin)?;
            check_can_write(client, Some(token))?;
            let stop = || progress.is_cancelled();
            let chapter = StudyRef { study: link.study.clone(), chapter: Some(link.chapter.clone()) };
            let (c, t, l) = (client.clone(), token.clone(), chapter.clone());
            let now = cancellable(progress, move |stop| fetch_study(&c, &l, Some(&t), None, stop))?;
            let now = now.and_then(|s| s.chapters.into_iter().next());
            let Some(now) = now else {
                return Err(NetError::Refused("its chapter is gone from the study: sync again".into()));
            };
            let now_game = read_pgn_str(&now.pgn).into_iter().next().unwrap_or_default();
            if theirs.as_ref().is_none_or(|t| !same_on_lichess(&now_game, t)) {
                let p = Pending { theirs: now.pgn.clone(), found: now_ms(), ..pending };
                let l = StudyLink { layer: layer_bytes(&ours, Some(&p)), ..link };
                db.lock().expect("db lock").set_link(uid, &l)?;
                return Err(NetError::Refused(
                    "the chapter changed on Lichess again since: look at it again before choosing".into(),
                ));
            }
            let send = lichess_projection(&ours);
            let pgn = pgn_to_send(&send);
            push_moves(client, token, &link.study, &link.chapter, &pgn, &stop)?;
            push_tags(client, token, &link.study, &link.chapter, &tag_changes(&ours, &now_game), &stop)?;
            let s = Sent {
                uid,
                chapter: link.chapter.clone(),
                game: ours.clone(),
                sent: send,
                sent_pgn: pgn,
                link: None,
                step: Step::Push,
            };
            let export = fresh_export(client, token, &link.study, std::slice::from_ref(&s), progress);
            let base = export
                .as_ref()
                .and_then(|e| {
                    chapters(e, &link.study)
                        .into_iter()
                        .find(|c| c.id == link.chapter)
                        .map(|c| c.pgn.to_owned())
                })
                .filter(|pgn| read_pgn_str(pgn).first().is_some_and(|g| !changed_here(&s.sent, g)))
                .unwrap_or(s.sent_pgn);
            db.lock().expect("db lock").set_link(uid, &relinked(&link, &base, &ours, None))?;
            Ok(format!("{name} went to Lichess: the chapter is this database's version now"))
        }
        (_, Choice::Mine) => {
            // Following only: kept here; Lichess's is the base from now on.
            let base = if pending.theirs.is_empty() { link.base.clone() } else { pending.theirs.clone() };
            db.lock().expect("db lock").set_link(uid, &relinked(&link, &base, &ours, None))?;
            Ok(format!("{name} stays as it is here"))
        }
        (_, choice @ (Choice::Theirs | Choice::Both)) => {
            let theirs = theirs.ok_or_else(|| NetError::Refused("Lichess's version can't be read".into()))?;
            let game = apply_layer(&theirs, &local_only_layer(&ours)).game;
            let new = relinked(&link, &pending.theirs, &game, None);
            unchanged(&mut |db, slot| {
                if choice == Choice::Both {
                    copy(db)?;
                }
                db.replace(slot, &game)?;
                db.set_link(uid, &new)
            })?;
            Ok(match choice {
                Choice::Both => format!(
                    "{name} is Lichess's version now; this database's is kept as a new game, marked as a copy"
                ),
                _ => format!("{name} is Lichess's version now"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_merge_reads_back() {
        let g = read_pgn_str("1. e4 { [%eval 0.3] } e5 *").remove(0);
        let p = Pending {
            kind: PendingKind::NoBase,
            theirs: "[Event \"x\"]\n\n1. e4 *".into(),
            found: 1_793_000_000_000,
        };
        let link = StudyLink {
            study: "abcdEFGH".into(),
            chapter: "ijklMNOP".into(),
            mode: SyncMode::TwoWay,
            last_modified: None,
            base: String::new(),
            layer: layer_bytes(&g, Some(&p)),
        };
        let (layer, back) = link_state(&link).unwrap();
        assert_eq!(layer, local_only_layer(&g));
        assert_eq!(back, Some(p));
        let link = StudyLink { layer: layer_bytes(&g, None), ..link };
        assert_eq!(link_state(&link).unwrap().1, None);
        // A layer without one, as the layer alone writes it.
        assert_eq!(Layer::from_bytes(&link.layer).unwrap(), local_only_layer(&g));
        let mut bad = Pending { kind: PendingKind::Both, theirs: String::new(), found: 0 }.to_bytes();
        bad[1] = 9;
        assert!(Pending::from_bytes(&bad).is_err());
    }

    #[test]
    fn what_is_sent_has_its_movetext_on_one_line() {
        let long =
            "A comment long enough for our writer to wrap it at eighty columns, which Lichess would keep.";
        let g =
            read_pgn_str(&format!("[White \"A\"]\n\n1. e4 {{ {long} }} e5 2. Nf3 {{ {long} }} *")).remove(0);
        let p = lichess_projection(&g);
        assert!(write_game(&p).trim().lines().count() > 4, "wrapped by the writer");
        let sent = pgn_to_send(&p);
        let (tags, moves) = sent.split_once("\n\n").unwrap();
        assert!(tags.contains("[White \"A\"]"));
        assert_eq!(moves.trim().lines().count(), 1, "{sent}");
        assert_eq!(read_pgn_str(&sent).remove(0), p, "the same game");
    }

    #[test]
    fn chapter_links() {
        assert_eq!(
            chapter_of("https://lichess.org/study/abcdEFGH/ijklMNOP", "abcdEFGH").as_deref(),
            Some("ijklMNOP")
        );
        assert_eq!(chapter_of("https://lichess.org/study/abcdEFGH/ijklMNOP", "zzzzZZZZ"), None);
        assert_eq!(chapter_of("https://lichess.org/study/abcdEFGH", "abcdEFGH"), None);
    }
}
