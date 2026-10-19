//! Duplicate games (DB-08, ChessBase's "Double game killer"): copies of the
//! same game, as importing a file twice or merging two sources leaves.
//! Deleting a game that wasn't a copy loses it, so when in doubt two games
//! are not copies.
//!
//! - **Exact copies** have the same moves (the main line, from the same
//!   start) and the same White, Black, their ratings, Event, Site, Date,
//!   Round and Result, and the same game ids (`Link`, `UTCTime`…, see
//!   [`GAME_ID_TAGS`](crate::blob::GAME_ID_TAGS)). (Online rematches can repeat a short game; their
//!   ratings, pages or start times differ.)
//! - **Fuzzy copies** (optional) have the same players, allowing for how
//!   names are written ("Carlsen, Magnus", "Carlsen, M.", "Magnus Carlsen",
//!   "Magnus Carlsen (NOR)"; a name with digits, such as an engine's
//!   "Stockfish 16 64-bit", must be written the same); dates, results,
//!   rounds (with the board or game after the dot: "5.1" and "5.3" are two
//!   games), players' ratings, Site URLs (one per online game) and game ids
//!   that agree where both copies have them; and the same main line, or one
//!   that is the start of the other (a source that stopped early). That one
//!   has at least 10 moves, both dates have the year, and it lacks only the
//!   last move or shares the event, the round or the full date with the
//!   other: a short draw that follows another game's opening is a game of
//!   its own. Copies shorter than 10 moves also need the same full date.
//!   Event and Site names may differ, and one copy may lack the ratings.
//! - **Games of one move or none** (a forfeit, a placeholder for a missing
//!   game, an odds game recorded by its first move) say nothing by their
//!   moves. They are never fuzzy copies, and exact copies only between two
//!   known players (not "NN", "BYE"…) with the round, or a page or id of
//!   their own (an online game): a team's forfeits ("NN – NN 0-1") or a
//!   day's odds games look alike but are different games.
//!
//! Each group keeps one copy: the one with the longest main line; then the
//! most variations, comments, symbols and arrows; then the most complete
//! headers; then the most clock and eval commands; then the first in the
//! database. The others are the ones to delete, and none of them has
//! anything the kept game lacks: a copy with annotations of its own (a
//! different comment, a shorter copy that is annotated) is no copy. Every
//! one of them is a copy of the kept game itself: fuzzy matches don't chain,
//! and a game that could be a copy of two different games (the start of
//! both, or without the headers that tell them apart), or of many games of
//! its players, is left alone: there's no telling which.
//!
//! It scales to millions of games because it never compares all pairs: the
//! index records are scanned first and bucketed by their headers (exact) or
//! players (fuzzy), and only the games that share a bucket have their moves
//! read. Within a bucket, the main lines are sorted, so the games a line is
//! the start of follow it. Games marked as deleted are left out.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

use crate::blob::{BlobDigest, digest};
use crate::database::NameKind;
use crate::positions::for_each_game;
use crate::record::IndexRecord;
use crate::{Database, DbError, GameId, Progress};

/// Main lines shorter than this (10 moves) are weak evidence on their own:
/// a fuzzy match needs the same full date as well.
const SHORT_PLIES: usize = 20;
/// A copy that stops early has at least this many plies (10 moves): a
/// shorter start says little about which game it was.
const MIN_PREFIX_PLIES: usize = 20;
/// Without more in common, a copy that stops early lacks at most this
/// many plies (the last move): a source that left it out.
const NEAR_PLIES: usize = 2;
/// Games shorter than this (one move or none) say nothing by their moves.
const FEW_PLIES: usize = 3;
/// A fuzzy copy is left alone when more games of its players than this
/// start with its moves: there's no telling which one it would be a copy of.
const CROWD: usize = 64;
/// Index records read per lock.
const CHUNK: u32 = 64 * 1024;
/// Fuzzy comparisons between two checks for cancelling (and reports).
const CHECK_EVERY: u64 = 1024;
/// Names that stand for no player (the first word, folded, letters and
/// digits only); "NN", "N.N." and "NN4" are recognised as well.
const PLACEHOLDERS: [&str; 10] =
    ["unknown", "anonymous", "bye", "empty", "forfeit", "vacant", "none", "nobody", "freilos", "spielfrei"];

/// What counts as a copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DuplicateOptions {
    /// Fuzzy copies too, not only exact ones (see the module docs).
    pub fuzzy: bool,
}

/// Copies of one game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// The copy to keep: the longest, then the best annotated (see the
    /// module docs).
    pub keep: GameId,
    /// The other copies, ascending: the ones to delete.
    pub remove: Vec<GameId>,
    /// Every copy is an exact copy of the kept one (else some are fuzzy).
    pub exact: bool,
}

/// What [`find_duplicates`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Duplicates {
    /// In the order of their first game.
    pub groups: Vec<DuplicateGroup>,
    /// Games looked at (those not marked as deleted).
    pub games: u32,
    /// Games whose moves were read: those that shared a bucket.
    pub compared: u32,
    /// [`Database::generation`] when the search began: the groups hold
    /// while it stays the same. A change since (a game added, edited,
    /// deleted or restored) may have made a copy the only one left.
    pub generation: u64,
}

impl Duplicates {
    /// How many games there are to delete.
    pub fn removable(&self) -> usize {
        self.groups.iter().map(|g| g.remove.len()).sum()
    }

    /// The games to delete, ascending.
    pub fn to_remove(&self) -> Vec<GameId> {
        let mut ids: Vec<GameId> = self.groups.iter().flat_map(|g| g.remove.iter().copied()).collect();
        ids.sort_unstable();
        ids
    }

    /// Every game of every group, group by group: the kept copy, then the
    /// others (for a game list that shows the copies together).
    pub fn listed(&self) -> Vec<GameId> {
        let mut ids = Vec::with_capacity(self.groups.len() + self.removable());
        for g in &self.groups {
            ids.push(g.keep);
            ids.extend_from_slice(&g.remove);
        }
        ids
    }
}

fn hash_of(value: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

/// A player as fuzzy matching sees them: the surname (hashed; 0 when the
/// name stands for no one) and the first letter of the given name, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PlayerKey {
    surname: u64,
    initial: Option<char>,
}

/// A name folded (case and accents), without what's in brackets: the
/// federation in "Carlsen, Magnus (NOR)", a rating in "Kasparov, G [2851]".
fn plain_name(name: &str) -> String {
    let mut depth = 0u32;
    let mut out = String::with_capacity(name.len());
    for c in crate::fold(name).chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Whether a [`plain_name`] stands for no one: empty, "?", "NN", "N.N.",
/// "NN4", "NN YEM", "BYE", "Unknown"…
fn is_placeholder(plain: &str) -> bool {
    let words: Vec<String> = plain
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect();
    let nn = |w: &String| w.strip_prefix("nn").is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()));
    words.first().is_none_or(|first| PLACEHOLDERS.contains(&first.as_str())) || words.iter().any(nn)
}

/// "Last, First" is PGN's form; without a comma the last word is taken as
/// the surname ("Magnus Carlsen"). Case, accents, punctuation and brackets
/// don't count. A name with digits isn't a person's (an engine and its
/// version, a username): it must be written the same, so all of it is the
/// key.
fn player_key(name: &str) -> PlayerKey {
    let plain = plain_name(name);
    if is_placeholder(&plain) {
        return PlayerKey::default();
    }
    let letters = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    if plain.chars().any(|c| c.is_ascii_digit()) {
        return PlayerKey { surname: hash_of(("whole", letters(&plain))).max(1), initial: None };
    }
    let (surname, given) = match plain.split_once(',') {
        Some((last, first)) => (last, first),
        None => match plain.trim().rsplit_once(char::is_whitespace) {
            Some((first, last)) => (last, first),
            None => (plain.as_str(), ""),
        },
    };
    let surname = letters(surname);
    if surname.is_empty() {
        return PlayerKey::default();
    }
    PlayerKey { surname: hash_of(&surname).max(1), initial: given.chars().find(|c| c.is_alphanumeric()) }
}

/// What the comparisons need of the name tables, read once.
struct Names {
    players: Vec<PlayerKey>,
    /// Sites that are a URL (an online game's own page).
    url_sites: Vec<bool>,
}

impl Names {
    fn read(db: &Database) -> Names {
        let players = db.names(NameKind::Player);
        let sites = db.names(NameKind::Site);
        Names {
            players: (0..players.len() as u32).map(|id| player_key(players.get(id))).collect(),
            url_sites: (0..sites.len() as u32).map(|id| sites.get(id).contains("://")).collect(),
        }
    }

    /// Names added since [`read`](Self::read) count as unknown.
    fn player(&self, id: u32) -> PlayerKey {
        self.players.get(id as usize).copied().unwrap_or_default()
    }

    /// A player with a name, not a placeholder.
    fn known(&self, id: u32) -> bool {
        self.player(id).surname != 0
    }

    fn is_url(&self, site: u32) -> bool {
        self.url_sites.get(site as usize).copied().unwrap_or(false)
    }

    /// The bucket of fuzzy matching: both players' surnames.
    fn players_key(&self, white: u32, black: u32) -> Option<u64> {
        let (w, b) = (self.player(white), self.player(black));
        (w.surname != 0 && b.surname != 0).then(|| hash_of((w.surname, b.surname)))
    }
}

/// The headers exact copies share (name ids, packed date and round,
/// ratings, result).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Tags {
    white: u32,
    black: u32,
    event: u32,
    site: u32,
    date: u32,
    round: u16,
    subround: u16,
    white_elo: u16,
    black_elo: u16,
    result: u8,
}

impl Tags {
    fn of(r: &IndexRecord) -> Tags {
        Tags {
            white: r.white,
            black: r.black,
            event: r.event,
            site: r.site,
            date: r.date,
            round: r.round,
            subround: r.subround,
            white_elo: r.white_elo,
            black_elo: r.black_elo,
            result: r.result,
        }
    }
}

/// Which copy to keep: the greatest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Quality {
    /// The main line's length: a copy that stops early is never kept over
    /// the whole game (exact copies have the same).
    plies: u32,
    /// Variations, comments, symbols and arrows.
    notes: u32,
    headers: u32,
    /// Clock, eval and other commands.
    commands: u32,
    first: Reverse<GameId>,
}

/// How many headers a game has filled in.
fn header_score(r: &IndexRecord, extra_tags: u32) -> u32 {
    let known = [
        r.white_elo != 0,
        r.black_elo != 0,
        r.eco != 0,
        r.event != 0,
        r.site != 0,
        r.annotator != 0,
        r.round != 0,
        r.subround != 0,
        r.result != 0,
        r.date >> 9 != 0,
        (r.date >> 5) & 0xf != 0,
        r.date & 0x1f != 0,
    ];
    known.iter().filter(|&&k| k).count() as u32 + extra_tags.min(32)
}

/// The set-up position (hashed; 0 for the standard start) and the main
/// line (hashed with it) of a digested game.
fn line_of(d: &BlobDigest) -> (u64, u64) {
    let start = d.setup.as_deref().map_or(0, |fen| hash_of(fen).max(1));
    (start, hash_of((start, &d.mainline)))
}

fn full_date(d: u32) -> bool {
    d >> 9 != 0 && (d >> 5) & 0xf != 0 && d & 0x1f != 0
}

/// Whether a game of `plies` can have copies at all (see the module docs):
/// with one move or none, only between known players with the round, or a
/// page or id of its own (`ids`: game id tags; not known yet when `None`).
fn moves_ok(plies: usize, r: &IndexRecord, names: &Names, ids: Option<bool>) -> bool {
    plies >= FEW_PLIES
        || (names.known(r.white)
            && names.known(r.black)
            && (r.round != 0 || names.is_url(r.site) || ids.unwrap_or(true)))
}

/// A game whose moves were read. Only hashes of the moves are kept: a
/// million of these take about 80 MB.
struct Candidate {
    id: GameId,
    tags: Tags,
    start: u64,
    line: u64,
    /// Its [`GAME_ID_TAGS`](crate::blob::GAME_ID_TAGS), hashed together
    /// (0 for none).
    game_ids: u64,
    /// Its annotations ([`BlobDigest::notes_hash`]; 0 for none).
    notes_hash: u64,
    quality: Quality,
}

impl Candidate {
    fn new(id: GameId, rec: &IndexRecord, blob: &[u8]) -> Result<Candidate, DbError> {
        let d = digest(blob)?;
        let (start, line) = line_of(&d);
        Ok(Candidate {
            id,
            tags: Tags::of(rec),
            start,
            line,
            game_ids: if d.game_ids.is_empty() { 0 } else { hash_of(&d.game_ids) },
            notes_hash: d.notes_hash,
            quality: Quality {
                plies: d.mainline.len() as u32,
                notes: d.notes(),
                headers: header_score(rec, d.extra_tags),
                commands: d.commands,
                first: Reverse(id),
            },
        })
    }

    /// What exact copies have in common. 64 bits of the line are plenty:
    /// two lines must also have the same headers and length to be confused.
    fn exact_key(&self) -> (Tags, u64, u64, u32, u64) {
        (self.tags, self.start, self.line, self.quality.plies, self.game_ids)
    }
}

/// A class's best copy as fuzzy matching sees it: with its moves and game
/// ids, read again.
#[derive(Clone, Copy)]
struct Line<'a> {
    c: &'a Candidate,
    moves: &'a [u8],
    ids: &'a [(u8, u64)],
}

/// Year, month and day agree where both dates know them.
fn dates_agree(a: u32, b: u32) -> bool {
    let agree = |x: u32, y: u32| x == 0 || y == 0 || x == y;
    agree(a >> 9, b >> 9) && agree((a >> 5) & 0xf, (b >> 5) & 0xf) && agree(a & 0x1f, b & 0x1f)
}

/// Game ids agree where both games have the same tag.
fn ids_agree(a: &[(u8, u64)], b: &[(u8, u64)]) -> bool {
    a.iter().all(|&(tag, value)| b.iter().all(|&(t, v)| t != tag || v == value))
}

/// Whether `a` and `b` are fuzzy copies (see the module docs). Their
/// players' surnames are known to agree (they share a bucket).
fn fuzzy_match(a: &Line, b: &Line, names: &Names) -> bool {
    let (ta, tb) = (&a.c.tags, &b.c.tags);
    let initials = |x: u32, y: u32| match (names.player(x).initial, names.player(y).initial) {
        (Some(p), Some(q)) => p == q,
        _ => true,
    };
    let known_same = |x: u16, y: u16| x == y || x == 0 || y == 0;
    let year = |d: u32| d >> 9 != 0;
    let (short, long) = if a.moves.len() <= b.moves.len() { (a.moves, b.moves) } else { (b.moves, a.moves) };
    // Something more than the players and the year that they share.
    let more = (ta.event != 0 && ta.event == tb.event)
        || (ta.round != 0 && ta.round == tb.round)
        || (full_date(ta.date) && ta.date == tb.date);
    let stops_early = short.len() >= MIN_PREFIX_PLIES
        && year(ta.date)
        && year(tb.date)
        && (long.len() - short.len() <= NEAR_PLIES || more);
    a.c.start == b.c.start
        && !short.is_empty()
        && long.starts_with(short)
        && (short.len() == long.len() || stops_early)
        && initials(ta.white, tb.white)
        && initials(ta.black, tb.black)
        && dates_agree(ta.date, tb.date)
        && (short.len() >= SHORT_PLIES || (full_date(ta.date) && ta.date == tb.date))
        && (ta.result == tb.result || ta.result == 0 || tb.result == 0)
        && known_same(ta.round, tb.round)
        && known_same(ta.subround, tb.subround)
        && known_same(ta.white_elo, tb.white_elo)
        && known_same(ta.black_elo, tb.black_elo)
        && (ta.site == tb.site || !names.is_url(ta.site) || !names.is_url(tb.site))
        && ids_agree(a.ids, b.ids)
}

/// Whether deleting `copy` for `kept` loses nothing: it has no annotations,
/// or the same moves annotated the same.
fn removable(copy: &Line, kept: &Line) -> bool {
    copy.c.notes_hash == 0 || (copy.moves == kept.moves && copy.c.notes_hash == kept.c.notes_hash)
}

/// Reports one part of the whole job's progress, in thousandths.
struct Part<'a> {
    inner: &'a dyn Progress,
    from: u64,
    span: u64,
}

impl Progress for Part<'_> {
    fn report(&self, done: u64, total: u64) {
        let part = (self.span * done.min(total)).checked_div(total).unwrap_or(self.span);
        self.inner.report(self.from + part, 1000);
    }

    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

fn compacted() -> DbError {
    DbError::Invalid("the database was compacted while looking for duplicates, so look for them again".into())
}

/// Sorts exact copies (`run`, the same exact key, best first) into classes
/// by their annotations: the best copy's, with the copies without any; then
/// each other annotation, best first. Pushes each class's range of `order`.
fn split_by_notes(run: &mut [usize], at: usize, copies: &[Candidate], classes: &mut Vec<(usize, usize)>) {
    let lead = copies[run[0]].notes_hash;
    let class = |i: usize| {
        let h = copies[i].notes_hash;
        if h == 0 || h == lead { 0 } else { h }
    };
    // Stable: each class keeps its best copy first.
    run.sort_by_key(|&i| class(i));
    let mut from = 0;
    for part in run.chunk_by(|&a, &b| class(a) == class(b)) {
        classes.push((at + from, at + from + part.len()));
        from += part.len();
    }
}

/// Finds the duplicate games of `db` (see the module docs). The database is
/// locked only while index records and games are read, a batch at a time.
/// Fails if the database is compacted meanwhile (game ids change).
pub fn find_duplicates(
    db: &Mutex<Database>,
    options: DuplicateOptions,
    progress: &dyn Progress,
) -> Result<Duplicates, DbError> {
    let (count, compactions, generation, names) = {
        let db = db.lock().expect("db lock");
        (db.count(), db.compactions(), db.generation(), Names::read(&db))
    };
    let still_numbered = || {
        if db.lock().expect("db lock").compactions() == compactions { Ok(()) } else { Err(compacted()) }
    };

    // 1. Buckets from the index records alone.
    let scan = Part { inner: progress, from: 0, span: 100 };
    let mut exact: Vec<(u64, GameId)> = Vec::new();
    // (players, site, game)
    let mut fuzzy: Vec<(u64, u32, GameId)> = Vec::new();
    let mut games = 0u32;
    let mut start = 0;
    while start < count {
        scan.check_cancelled()?;
        let records = {
            let db = db.lock().expect("db lock");
            if db.compactions() != compactions {
                return Err(compacted());
            }
            db.records(start, CHUNK.min(count - start))?
        };
        for (r, id) in records.iter().zip(start..).filter(|(r, _)| !r.is_deleted()) {
            games += 1;
            if !moves_ok(r.plies.into(), r, &names, None) {
                continue;
            }
            exact.push((hash_of(Tags::of(r)), id));
            if options.fuzzy
                && usize::from(r.plies) >= FEW_PLIES
                && let Some(key) = names.players_key(r.white, r.black)
            {
                fuzzy.push((key, r.site, id));
            }
        }
        start = start.saturating_add(CHUNK);
        scan.report(u64::from(start.min(count)), u64::from(count));
    }
    let mut candidates: Vec<GameId> = Vec::new();
    exact.sort_unstable();
    for run in exact.chunk_by(|a, b| a.0 == b.0).filter(|run| run.len() > 1) {
        candidates.extend(run.iter().map(|&(_, id)| id));
    }
    drop(exact);
    fuzzy.sort_unstable();
    for run in fuzzy.chunk_by(|a, b| a.0 == b.0).filter(|run| run.len() > 1) {
        if run.iter().any(|&(_, site, _)| !names.is_url(site)) {
            candidates.extend(run.iter().map(|&(_, _, id)| id));
        } else {
            // Online games, each with its own page, can only be copies of a
            // game with the same page.
            for same in run.chunk_by(|a, b| a.1 == b.1).filter(|same| same.len() > 1) {
                candidates.extend(same.iter().map(|&(_, _, id)| id));
            }
        }
    }
    drop(fuzzy);
    candidates.sort_unstable();
    candidates.dedup();

    // 2. The moves of the games that share a bucket, hashed.
    let read = Part { inner: progress, from: 100, span: if options.fuzzy { 600 } else { 850 } };
    let work = |id: GameId, rec: &IndexRecord, blob: &[u8]| -> Option<Candidate> {
        if rec.is_deleted() {
            return None;
        }
        let c = Candidate::new(id, rec, blob)
            .inspect_err(|e| log::warn!("duplicates: skipping game {id}: {e}"))
            .ok()?;
        // The record's count of plies decided before; the moves decide.
        moves_ok(c.quality.plies as usize, rec, &names, Some(c.game_ids != 0)).then_some(c)
    };
    let mut copies: Vec<Candidate> = Vec::with_capacity(candidates.len());
    let result = for_each_game(db, &candidates, &read, &work, &mut |batch| {
        copies.extend(batch.into_iter().flatten());
        Ok(())
    });
    still_numbered()?;
    result?;
    drop(candidates);
    let compared = copies.len() as u32;

    // 3. Exact copies: the same key, best first, then apart by their
    // annotations.
    let mut order: Vec<usize> = (0..copies.len()).collect();
    order.sort_unstable_by_key(|&i| (copies[i].exact_key(), Reverse(copies[i].quality)));
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < order.len() {
        let key = copies[order[at]].exact_key();
        let len = order[at..].iter().take_while(|&&i| copies[i].exact_key() == key).count();
        split_by_notes(&mut order[at..at + len], at, &copies, &mut bounds);
        at += len;
    }
    let classes: Vec<&[usize]> = bounds.iter().map(|&(from, to)| &order[from..to]).collect();
    progress.check_cancelled()?;

    // 4. Fuzzy copies: between the exact classes (their best copies) of
    // the same players.
    let mut merged: Vec<Vec<usize>> = Vec::new();
    if options.fuzzy {
        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        for (ci, class) in classes.iter().enumerate() {
            let c = &copies[class[0]];
            match names
                .players_key(c.tags.white, c.tags.black)
                .filter(|_| c.quality.plies as usize >= FEW_PLIES)
            {
                Some(key) => buckets.entry(key).or_default().push(ci),
                None => merged.push(vec![ci]),
            }
        }
        // Buckets where two classes could match: one class alone can't, nor
        // can online games with pages of their own.
        let (open, closed): (Vec<Vec<usize>>, Vec<Vec<usize>>) = buckets.into_values().partition(|b| {
            let mut sites: Vec<u32> = b.iter().map(|&ci| copies[classes[ci][0]].tags.site).collect();
            sites.sort_unstable();
            b.len() > 1 && (sites.iter().any(|&s| !names.is_url(s)) || sites.windows(2).any(|w| w[0] == w[1]))
        });
        merged.extend(closed.into_iter().flatten().map(|ci| vec![ci]));

        // Their main lines and game ids, read again (only hashes were kept).
        let mut leaders: Vec<usize> = open.iter().flatten().map(|&ci| classes[ci][0]).collect();
        leaders.sort_unstable_by_key(|&i| copies[i].id);
        let ids: Vec<GameId> = leaders.iter().map(|&i| copies[i].id).collect();
        let mut arena: Vec<u8> = Vec::new();
        let mut spans: HashMap<usize, (usize, usize)> = HashMap::with_capacity(leaders.len());
        let mut game_ids: HashMap<usize, Vec<(u8, u64)>> = HashMap::new();
        let mut next = leaders.iter();
        let reread = Part { inner: progress, from: 700, span: 250 };
        let work = |_id: GameId, rec: &IndexRecord, blob: &[u8]| -> Option<BlobDigest> {
            if rec.is_deleted() { None } else { digest(blob).ok() }
        };
        let result = for_each_game(db, &ids, &reread, &work, &mut |batch| {
            for d in batch {
                let i = *next.next().expect("one result per game");
                // Skip a game edited since it was hashed.
                if let Some(d) = d.filter(|d| line_of(d) == (copies[i].start, copies[i].line)) {
                    spans.insert(i, (arena.len(), d.mainline.len()));
                    arena.extend_from_slice(&d.mainline);
                    if !d.game_ids.is_empty() {
                        game_ids.insert(i, d.game_ids);
                    }
                }
            }
            Ok(())
        });
        still_numbered()?;
        result?;
        let line = |i: usize| {
            spans.get(&i).map(|&(at, len)| Line {
                c: &copies[i],
                moves: &arena[at..at + len],
                ids: game_ids.get(&i).map_or(&[], Vec::as_slice),
            })
        };
        let compare = Part { inner: progress, from: 950, span: 50 };
        let total: u64 = open.iter().map(|b| b.len() as u64).sum();
        let mut done = 0u64;
        let mut tick = || -> Result<(), DbError> {
            if done.is_multiple_of(CHECK_EVERY) {
                compare.check_cancelled()?;
                compare.report(done, total);
            }
            done += 1;
            Ok(())
        };
        for bucket in &open {
            merged.extend(cluster(bucket, &classes, &line, &names, &mut tick)?);
        }
    } else {
        merged.extend((0..classes.len()).map(|ci| vec![ci]));
    }

    let mut groups: Vec<DuplicateGroup> = merged
        .into_iter()
        .filter(|group| group.len() > 1 || classes[group[0]].len() > 1)
        .map(|group| {
            let keep = copies[classes[group[0]][0]].id;
            let mut remove: Vec<GameId> = group
                .iter()
                .flat_map(|&ci| classes[ci].iter())
                .map(|&i| copies[i].id)
                .filter(|&id| id != keep)
                .collect();
            remove.sort_unstable();
            DuplicateGroup { keep, remove, exact: group.len() == 1 }
        })
        .collect();
    groups.sort_unstable_by_key(|g| g.keep.min(g.remove[0]));
    progress.report(1000, 1000);
    Ok(Duplicates { groups, games, compared, generation })
}

/// What a class became while clustering.
#[derive(Debug, Clone, Copy)]
enum Role {
    Unseen,
    /// Leads this group (kept).
    Leads(usize),
    /// A copy in a group.
    Joined,
    /// Left alone: it can't be told whose copy it is.
    Alone,
}

/// Fuzzy groups among `bucket` (exact classes of the same players). The
/// classes are taken longest first, then best annotated: each joins the
/// group of the best game it's a copy of (and has nothing more than), or
/// leads a new group, or stays alone when it could be a copy of two
/// different games (or of too many). The leader is kept, and every member matches it.
/// A class whose moves couldn't be read again, or that has next to none,
/// stays alone.
fn cluster<'m>(
    bucket: &[usize],
    classes: &[&[usize]],
    line: &dyn Fn(usize) -> Option<Line<'m>>,
    names: &Names,
    tick: &mut dyn FnMut() -> Result<(), DbError>,
) -> Result<Vec<Vec<usize>>, DbError> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    // (class, its best copy's line)
    let mut live: Vec<(usize, Line<'m>)> = Vec::with_capacity(bucket.len());
    for &ci in bucket {
        match line(classes[ci][0]) {
            Some(l) if l.moves.len() >= FEW_PLIES => live.push((ci, l)),
            _ => out.push(vec![ci]),
        }
    }
    let key = |i: usize| (live[i].1.c.start, live[i].1.moves);
    // Sorted by their moves: the lines that start with a line's moves follow it.
    let mut lex: Vec<usize> = (0..live.len()).collect();
    lex.sort_unstable_by(|&a, &b| key(a).cmp(&key(b)));
    let mut todo: Vec<usize> = (0..live.len()).collect();
    todo.sort_unstable_by_key(|&i| Reverse(live[i].1.c.quality));

    let mut role = vec![Role::Unseen; live.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &x in &todo {
        tick()?;
        let xl = &live[x].1;
        // The groups it could be a copy of: their leaders start with its
        // moves (they're as long or longer, so they came first).
        let mut matches: Vec<(usize, usize)> = Vec::new();
        let mut crowded = false;
        let mut seen = 0;
        for &y in &lex[lex.partition_point(|&y| key(y) < key(x))..] {
            let yl = &live[y].1;
            if yl.c.start != xl.c.start || !yl.moves.starts_with(xl.moves) {
                break;
            }
            if y == x {
                continue;
            }
            seen += 1;
            if seen > CROWD {
                crowded = true;
                break;
            }
            if let Role::Leads(g) = role[y]
                && fuzzy_match(xl, yl, names)
            {
                matches.push((y, g));
            }
        }
        // It could be a copy of two games that aren't copies of each other
        // (they go different ways after its moves, or their headers differ
        // where it has none): no telling which.
        let ambiguous = matches.iter().enumerate().any(|(n, &(a, _))| {
            matches[n + 1..].iter().any(|&(b, _)| !fuzzy_match(&live[a].1, &live[b].1, names))
        });
        role[x] = if crowded || ambiguous {
            out.push(vec![live[x].0]);
            Role::Alone
        } else {
            let target = matches
                .iter()
                .filter(|&&(y, _)| removable(xl, &live[y].1))
                .max_by_key(|&&(y, _)| live[y].1.c.quality);
            match target {
                Some(&(_, g)) => {
                    groups[g].push(live[x].0);
                    Role::Joined
                }
                None => {
                    groups.push(vec![live[x].0]);
                    Role::Leads(groups.len() - 1)
                }
            }
        };
    }
    out.extend(groups);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use chess_core::pgn::read_pgn_str;
    use chess_core::shakmaty::san::San;
    use chess_core::shakmaty::{Chess, Position};

    use super::*;
    use crate::NoProgress;

    /// A game in PGN; `tags` are extra header lines.
    fn pgn(white: &str, black: &str, date: &str, result: &str, tags: &str, moves: &str) -> String {
        format!(
            "[Event \"Open\"]\n[Site \"Prague\"]\n[Date \"{date}\"]\n[Round \"3\"]\n[White \"{white}\"]\n\
             [Black \"{black}\"]\n[Result \"{result}\"]\n{tags}\n\n{moves} {result}\n\n"
        )
    }

    const LONG: &str = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3 O-O \
                        9. h3 Nb8 10. d4 Nbd7 11. Nbd2 Bb7 12. Bc2 Re8";
    /// LONG's first 20 plies.
    const CUT: &str = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3 O-O \
                       9. h3 Nb8 10. d4 Nbd7";
    /// LONG, then 20 plies more (44).
    const FULL: &str = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3 O-O \
                        9. h3 Nb8 10. d4 Nbd7 11. Nbd2 Bb7 12. Bc2 Re8 13. Nf1 Bf8 14. Ng3 g6 15. a4 c5 \
                        16. d5 c4 17. Bg5 h6 18. Be3 Nc5 19. Qd2 h5 20. Bg5 Be7 21. Ra3 Nfd7 22. Bh6 Bf6";

    fn db_of(pgns: &[String]) -> (tempfile::TempDir, Mutex<Database>) {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Dups")).unwrap();
        for g in read_pgn_str(&pgns.concat()) {
            db.append(&g).unwrap();
        }
        (dir, Mutex::new(db))
    }

    fn find(db: &Mutex<Database>, fuzzy: bool) -> Duplicates {
        find_duplicates(db, DuplicateOptions { fuzzy }, &NoProgress).unwrap()
    }

    fn group(keep: GameId, remove: &[GameId], exact: bool) -> DuplicateGroup {
        DuplicateGroup { keep, remove: remove.to_vec(), exact }
    }

    #[test]
    fn player_keys_allow_for_how_names_are_written() {
        let key = |name: &str| player_key(name);
        let carlsen = key("Carlsen, Magnus");
        assert_ne!(carlsen.surname, 0);
        assert_eq!(carlsen.initial, Some('m'));
        for same in
            ["Carlsen,M.", "CARLSEN, M", "Magnus Carlsen", " carlsen ,  magnus ", "Magnus Carlsen (NOR)"]
        {
            let k = key(same);
            assert_eq!((k.surname, k.initial), (carlsen.surname, Some('m')), "{same}");
        }
        assert_eq!(key("Carlsen, Magnus [2830]"), carlsen);
        assert_eq!(key("Carlsen").surname, carlsen.surname);
        assert_eq!(key("Carlsen").initial, None);
        assert_eq!(key("Réti, R.").surname, key("Reti, Richard").surname);
        assert_eq!(key("Vachier-Lagrave, M").surname, key("Vachier Lagrave, Maxime").surname);
        assert_ne!(key("Carlsen, Magnus").surname, key("Caruana, Fabiano").surname);
        for unknown in
            ["", "?", "-", "NN", "N.N.", "Unknown", "  ", "NN4", "NN YEM", "BYE", "EMPTY", "(NOR)", "nn, nn"]
        {
            assert_eq!(key(unknown), PlayerKey::default(), "{unknown:?}");
        }
        assert_ne!(key("Nunn, John").surname, 0, "a real name that starts with nn");
    }

    /// R1-11: names with digits (engines, usernames) aren't a person's
    /// "First Last": all of the name counts.
    #[test]
    fn names_with_digits_must_be_written_the_same() {
        let key = |name: &str| player_key(name);
        let stockfish = key("Stockfish 16 64-bit");
        assert_ne!(stockfish.surname, 0);
        assert_eq!(stockfish.initial, None);
        assert_eq!(key("stockfish 16 64bit"), stockfish, "case and punctuation still don't count");
        for other in ["Shredder 13 64-bit", "Stockfish 15 64-bit", "Komodo 14 64-bit", "64-bit"] {
            assert_ne!(key(other).surname, stockfish.surname, "{other}");
        }
        assert_ne!(key("Stockfish 16 64-bit").surname, key("Bit, Stockfish").surname);

        let moves = "1. e4 c5 2. Nf3 d6 3. d4 cxd4 4. Nxd4 Nf6 5. Nc3 a6 6. Be3 e5 7. Nb3 Be6 8. f3 Be7 \
                     9. Qd2 O-O 10. O-O-O Nbd7 11. g4 b5";
        let game = |white: &str, more: &str| {
            pgn(white, "Komodo 14 64-bit", "2024.01.05", "1/2-1/2", "", &format!("{moves}{more}"))
        };
        let (_d, db) = db_of(&[game("Stockfish 16 64-bit", " 12. g5 b4"), game("Shredder 13 64-bit", "")]);
        assert_eq!(find(&db, true).groups, [], "two engines");
        let (_d, db) = db_of(&[game("Stockfish 16 64-bit", " 12. g5 b4"), game("Stockfish 16 64bit", "")]);
        assert_eq!(find(&db, true).groups, [group(0, &[1], false)], "one engine, written two ways");
    }

    #[test]
    fn dates_agree_where_both_are_known() {
        let d = crate::record::pack_date;
        assert!(dates_agree(d("2024.03.01"), d("2024.03.01")));
        assert!(dates_agree(d("2024.03.01"), d("2024.??.??")));
        assert!(dates_agree(d("2024.03.01"), d("????.??.??")));
        assert!(!dates_agree(d("2024.03.01"), d("2024.03.02")));
        assert!(!dates_agree(d("2024.03.01"), d("2023.??.??")));
        assert!(full_date(d("2024.03.01")) && !full_date(d("2024.03.??")));
    }

    #[test]
    fn exact_copies_keep_the_best_annotated() {
        let plain = pgn("Kasparov, Garry", "Topalov, Veselin", "1999.01.20", "1-0", "", LONG);
        let annotated = pgn(
            "Kasparov, Garry",
            "Topalov, Veselin",
            "1999.01.20",
            "1-0",
            "[Annotator \"Me\"]",
            &LONG.replace("Re1", "Re1 $1 {Natural.}"),
        );
        let with_elo =
            pgn("Kasparov, Garry", "Topalov, Veselin", "1999.01.20", "1-0", "[WhiteElo \"2812\"]", LONG);
        let other_event = pgn("Kasparov, Garry", "Topalov, Veselin", "1999.01.20", "1-0", "", LONG)
            .replace("[Event \"Open\"]", "[Event \"Wijk aan Zee\"]");
        let other_game = pgn("Kasparov, Garry", "Topalov, Veselin", "1999.01.20", "1-0", "", "1. d4 d5");
        let (_d, db) =
            db_of(&[plain.clone(), annotated, plain.clone(), with_elo, other_event, other_game, plain]);
        let found = find(&db, false);
        assert_eq!(found.groups, [group(1, &[0, 2, 6], true)]);
        assert_eq!((found.games, found.removable()), (7, 3));
        assert_eq!(found.compared, 5, "the games with the same headers had their moves read");
        assert_eq!(found.to_remove(), [0, 2, 6]);
        assert_eq!(found.listed(), [1, 0, 2, 6]);
        assert_eq!(found.generation, db.lock().unwrap().generation());

        db.lock().unwrap().set_deleted(1, true).unwrap();
        assert_eq!(find(&db, false).groups, [group(0, &[2, 6], true)]);
        // The copy with a rating and the one with another event are fuzzy
        // copies; without annotations, the most complete headers win (the
        // rating).
        assert_eq!(find(&db, true).groups, [group(3, &[0, 2, 4, 6], false)]);
    }

    /// R1-8: a copy annotated differently has something the kept game
    /// lacks: it isn't deleted (nor are copies annotated the same way as
    /// it, which keep their own best).
    #[test]
    fn copies_annotated_differently_are_not_copies() {
        let game = |moves: &str| pgn("Smith, John", "Jones, Bob", "2024.03.01", "1-0", "", moves);
        let a = game(&LONG.replace("1. e4", "1. e4 {Annotator A: the best move.}"));
        let b = game(&LONG.replace("2. Nf3", "2. Nf3 (2. f4 {B's line} exf4) (2. Nc3 {B again})"));
        let plain = game(LONG);
        let (_d, db) = db_of(&[a.clone(), b.clone()]);
        for fuzzy in [false, true] {
            assert_eq!(find(&db, fuzzy).groups, [], "fuzzy: {fuzzy}");
        }
        // 0 and 3 annotated alike, 1 and 4 alike; 2 has nothing of its own.
        let (_d, db) = db_of(&[b.clone(), a.clone(), plain, b, a]);
        for fuzzy in [false, true] {
            assert_eq!(find(&db, fuzzy).groups, [group(0, &[2, 3], true), group(1, &[4], true)], "{fuzzy}");
        }
    }

    #[test]
    fn fuzzy_copies_allow_other_names_and_a_shorter_copy() {
        let games = [
            // 0: the full game.
            pgn("Carlsen, Magnus", "Caruana, Fabiano", "2024.03.01", "1/2-1/2", "", LONG),
            // 1: the same game from another source, stopped early.
            pgn("Carlsen, M.", "Caruana, F.", "2024.??.??", "1/2-1/2", "", CUT)
                .replace("[Event \"Open\"]", "[Event \"Prague Masters\"]"),
            // 2: the same game, names First Last, no round or site.
            pgn("Magnus Carlsen", "Fabiano Caruana", "2024.03.01", "*", "", LONG)
                .replace("[Round \"3\"]", "[Round \"?\"]")
                .replace("[Site \"Prague\"]", "[Site \"?\"]"),
            // 3: another game, in another round: they played the same line,
            // then something else.
            pgn(
                "Carlsen, Magnus",
                "Caruana, Fabiano",
                "2024.03.05",
                "1/2-1/2",
                "",
                &LONG.replace("Re8", "h6"),
            )
            .replace("[Round \"3\"]", "[Round \"7\"]"),
            // 4: another round.
            pgn("Carlsen, Magnus", "Caruana, Fabiano", "2024.03.01", "1/2-1/2", "", LONG)
                .replace("[Round \"3\"]", "[Round \"4\"]"),
            // 5: another date.
            pgn("Carlsen, Magnus", "Caruana, Fabiano", "2024.03.02", "1/2-1/2", "", LONG),
            // 6: another result.
            pgn("Carlsen, Magnus", "Caruana, Fabiano", "2024.03.01", "1-0", "", LONG),
            // 7: another player with the same surname.
            pgn("Carlsen, Henrik", "Caruana, Fabiano", "2024.03.01", "1/2-1/2", "", LONG),
            // 8: colours the other way round.
            pgn("Caruana, Fabiano", "Carlsen, Magnus", "2024.03.01", "1/2-1/2", "", LONG),
        ];
        let (_d, db) = db_of(&games);
        assert_eq!(find(&db, false).groups, [], "no exact copies");
        let some = |ids: &[usize]| ids.iter().map(|&i| games[i].clone()).collect::<Vec<_>>();
        let (_d, db) = db_of(&some(&[0, 1, 2, 3, 7, 8]));
        let found = find(&db, true);
        // 0 and 2 have the longest line; 0 has more headers.
        assert_eq!(found.groups, [group(0, &[1, 2], false)]);
        assert_eq!(found.compared, 5, "colours the other way round are another bucket");
        // Another round, date or result: another game.
        let (_d, db) = db_of(&some(&[0, 4, 5, 6]));
        assert_eq!(find(&db, true).groups, []);
        // Without its round, 2 could be a copy of 0 or of 4, and 1 (no day)
        // of 0 or of 5: no telling, so they stay.
        let (_d, db) = db_of(&games);
        assert_eq!(find(&db, true).groups, []);
    }

    /// R1-1: the kept copy is the longest. A shorter copy with annotations
    /// of its own isn't deleted for it either; one shorter than 10 moves,
    /// or without the year, isn't taken for a copy at all.
    #[test]
    fn a_copy_that_stops_early_is_never_kept_over_the_whole_game() {
        let game = |white: &str, date: &str, moves: &str| {
            pgn(white, "Caruana, Fabiano", date, "1-0", "", moves).replace("Round \"3\"", "Round \"?\"")
        };
        let annotated_cut = LONG.replace("Nb8", "Nb8 {The Breyer.}");
        let (_d, db) = db_of(&[
            game("Carlsen, Magnus", "2024.03.01", FULL),
            game("Carlsen, M.", "2024.03.01", &annotated_cut),
        ]);
        assert_eq!(
            find(&db, true).groups,
            [],
            "the annotated copy that stops early stays, and so does the game"
        );

        let (_d, db) =
            db_of(&[game("Carlsen, M.", "2024.03.01", LONG), game("Carlsen, Magnus", "2024.03.01", FULL)]);
        assert_eq!(find(&db, true).groups, [group(1, &[0], false)], "a plain copy that stops early");

        // 8 moves: too short to tell. Without the year: no telling it's the same game.
        let eight = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3 O-O";
        let (_d, db) = db_of(&[
            game("Carlsen, Magnus", "2024.03.01", FULL),
            game("Carlsen, Magnus", "2024.03.01", eight),
            game("Carlsen, Magnus", "????.??.??", LONG),
        ]);
        assert_eq!(find(&db, true).groups, []);
    }

    /// R1-1: Mega Database's Tomashevsky–Jobava playoff: games 3.1 and 3.2
    /// on one day and 3.3 a placeholder (`{Game missing}`) with no moves.
    /// Each is a game of its own.
    #[test]
    fn a_placeholder_without_moves_and_two_games_of_one_day() {
        let game = |round: &str, moves: &str| {
            pgn("Tomashevsky, Evgeny", "Jobava, Baadur", "2009.03.18", "1-0", "", moves)
                .replace("Round \"3\"", &format!("Round \"{round}\""))
        };
        let (_d, db) = db_of(&[game("3.1", FULL), game("3.2", LONG), game("3.3", "{Game missing}")]);
        for fuzzy in [false, true] {
            assert_eq!(find(&db, fuzzy).groups, [], "fuzzy: {fuzzy}");
        }
        // No round: the game without moves is still no copy of a game with moves.
        let (_d, db) = db_of(&[game("?", FULL), game("?", "{keine Notation vorhanden}")]);
        assert_eq!(find(&db, true).groups, []);
    }

    /// R1-2: the board or game after the dot counts where both have it.
    #[test]
    fn rounds_and_boards_must_agree() {
        let draw = "1. e4 e5 2. Nf3 Nc6 3. Bb5 Nf6 4. O-O Nxe4 5. Re1 Nd6 6. Nxe5 Be7 7. Bf1 Nxe5";
        let game = |round: &str| {
            pgn("Aronian, Levon", "So, Wesley", "2023.08.10", "1/2-1/2", "[WhiteElo \"2727\"]", draw)
                .replace("Round \"3\"", &format!("Round \"{round}\""))
        };
        let (_d, db) = db_of(&[game("5.1"), game("5.3")]);
        for fuzzy in [false, true] {
            assert_eq!(find(&db, fuzzy).groups, [], "fuzzy: {fuzzy}");
        }
        let (_d, db) = db_of(&[game("5"), game("5.3"), game("?")]);
        // "5.3" says more than "5" (and "?"), and agrees with both.
        assert_eq!(find(&db, true).groups, [group(1, &[0, 2], false)], "a round without a board agrees");
    }

    /// R1-3: games without moves (or with one at most).
    #[test]
    fn games_without_moves() {
        // A team's forfeits: alike, but each a game of its own.
        let forfeit = pgn("NN", "NN", "1989.??.??", "0-1", "", "{kampflos}")
            .replace("Round \"3\"", "Round \"?\"")
            .replace("Open", "Oberliga NRW 8889 I");
        let byes = |event: &str, round: &str| {
            pgn("BYE", "BYE", "2004.06.20", "1-0", "", "")
                .replace("Open", event)
                .replace("Round \"3\"", &format!("Round \"{round}\""))
        };
        let (_d, db) = db_of(&[
            forfeit.clone(),
            forfeit.clone(),
            forfeit,
            byes("BCF-chT4 0304", "11.1"),
            byes("BCF-chT3 0304", "11.2"),
            byes("BCF-chT4 0304", "11.1"),
            "{Plan for the Najdorf: play f4 and Qf3.} *\n\n".into(),
            "{Notes on rook endings: cut the king off.} *\n\n".into(),
            "{Notes on rook endings: cut the king off.} *\n\n".into(),
        ]);
        for fuzzy in [false, true] {
            let found = find(&db, fuzzy);
            assert_eq!(found.groups, [], "fuzzy: {fuzzy}");
            assert_eq!((found.games, found.compared), (9, 0), "none of them is read");
        }

        // The same record twice: a copy, between known players in a known round.
        let record = pgn("Zahn, Richard", "Ruprecht, Thomas", "2016.03.22", "1-0", "", "{kampflos}");
        // Without the round, the day isn't enough; an online game's page is.
        let no_round = record.replace("Round \"3\"", "Round \"?\"");
        let online = no_round.replace("Prague", "https://lichess.org/abcdefgh");
        let (_d, db) =
            db_of(&[record.clone(), record.clone(), no_round.clone(), no_round, online.clone(), online]);
        for fuzzy in [false, true] {
            assert_eq!(
                find(&db, fuzzy).groups,
                [group(0, &[1], true), group(4, &[5], true)],
                "fuzzy: {fuzzy}"
            );
        }
        // Never a fuzzy copy, nor one with another comment.
        let other_comment = record.replace("kampflos", "Weiß trat nicht an");
        let other_event = record.replace("Open", "Niederrhein-ch");
        let (_d, db) = db_of(&[record, other_comment, other_event]);
        assert_eq!(find(&db, true).groups, []);
    }

    /// R1-10: a game's own page (`Link` on chess.com), id or start time
    /// tells two games apart where both have them.
    #[test]
    fn games_with_their_own_ids_are_never_copies() {
        let game = |tags: &str| {
            pgn(
                "alice",
                "bob",
                "2024.01.15",
                "0-1",
                &format!("[WhiteElo \"1500\"]\n{tags}"),
                "1. f3 e5 2. g4 Qh4#",
            )
            .replace("[Site \"Prague\"]", "[Site \"Chess.com\"]")
        };
        let link = |n: u32, time: &str| {
            format!("[Link \"https://www.chess.com/game/live/{n}\"]\n[UTCTime \"{time}\"]")
        };
        let (_d, db) = db_of(&[game(&link(1, "18:00:01")), game(&link(2, "18:01:30"))]);
        for fuzzy in [false, true] {
            assert_eq!(find(&db, fuzzy).groups, [], "rematches, fuzzy: {fuzzy}");
        }
        let (_d, db) =
            db_of(&[game(&link(1, "18:00:01")), game(&link(1, "18:00:01")), game("[UTCTime \"18:00:01\"]")]);
        assert_eq!(find(&db, false).groups, [group(0, &[1], true)], "the same page and time");
        assert_eq!(find(&db, true).groups, [group(0, &[1, 2], false)], "without the page, the same time");
        let (_d, db) = db_of(&[game("[Board \"1\"]"), game("[Board \"2\"]")]);
        assert_eq!(find(&db, true).groups, [], "two boards");
    }

    #[test]
    fn ratings_must_agree_where_both_copies_have_them() {
        let elo = |w: &str| if w.is_empty() { String::new() } else { format!("[WhiteElo \"{w}\"]") };
        let game = |w: &str| pgn("Smith, John", "Jones, Bob", "2010.05.05", "1-0", &elo(w), LONG);
        let other = |g: String, event: &str| g.replace("Open", event);
        let (_d, db) = db_of(&[game("2400"), other(game("2380"), "Open A"), other(game(""), "Open B")]);
        // 0 and 1 aren't copies (they could have played twice that day);
        // the copy without a rating could be either: it stays.
        assert_eq!(find(&db, true).groups, []);
        let (_d, db) = db_of(&[game("2400"), other(game(""), "Open B")]);
        assert_eq!(find(&db, true).groups, [group(0, &[1], false)]);
    }

    #[test]
    fn short_games_need_the_same_full_date() {
        let mini = "1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7#";
        let games = [
            pgn("Smith, John", "Jones, Bob", "2010.05.05", "1-0", "", mini),
            pgn("Smith, J.", "Jones, B.", "2010.05.05", "1-0", "", mini),
            pgn("Smith, J.", "Jones, B.", "2010.??.??", "1-0", "", mini),
            pgn("Smith, J.", "Jones, B.", "2010.05.05", "1-0", "", "1. e4 e5"),
        ];
        let (_d, db) = db_of(&games);
        // The game without its full date stays, and so does the 2-move stub:
        // a copy that stops early has 10 moves at least.
        assert_eq!(find(&db, true).groups, [group(0, &[1], false)]);
    }

    #[test]
    fn online_games_with_their_own_pages_are_never_copies() {
        let lichess = |site: &str| {
            pgn("alice", "bob", "2024.03.01", "1-0", "", "1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7#")
                .replace("[Site \"Prague\"]", &format!("[Site \"https://lichess.org/{site}\"]"))
                .replace("[Round \"3\"]", "[Round \"-\"]")
        };
        let (_d, db) = db_of(&[lichess("aaaa"), lichess("bbbb"), lichess("aaaa")]);
        let found = find(&db, true);
        assert_eq!(found.groups, [group(0, &[2], true)]);
        assert_eq!(found.compared, 2, "the game with its own page isn't read");
    }

    /// R1-1: a copy that stops early where two games part could be a copy
    /// of either: it stays. Fuzzy matches don't chain either.
    #[test]
    fn a_copy_that_stops_where_two_games_part_stays() {
        // A stops early; B and C are different games that both start with A's moves.
        let b = format!("{CUT} 11. Nbd2 Bb7");
        let c = format!("{CUT} 11. c4 c6");
        let games = [
            pgn("Anand, V", "Kramnik, V", "2008.??.??", "1/2-1/2", "", CUT),
            pgn("Anand, V", "Kramnik, V", "2008.10.14", "1/2-1/2", "", &b),
            pgn("Anand, V", "Kramnik, V", "2008.10.20", "1/2-1/2", "", &c)
                .replace("[Round \"3\"]", "[Round \"?\"]"),
        ];
        let (_d, db) = db_of(&games);
        assert_eq!(find(&db, true).groups, []);
        // With C on another day than A, A is B's.
        let (_d, db) = db_of(&[
            games[0].replace("2008.??.??", "2008.10.??"),
            games[1].clone(),
            games[2].replace("2008.10.20", "2008.11.20"),
        ]);
        assert_eq!(find(&db, true).groups, [group(1, &[0], false)]);
    }

    #[test]
    fn deleted_games_and_unknown_players() {
        let unknown = pgn("?", "?", "????.??.??", "*", "", LONG);
        let named = pgn("Lasker, E", "Tarrasch, S", "1908.??.??", "1-0", "", LONG);
        let (_d, db) = db_of(&[unknown.clone(), unknown.clone(), named.clone(), named.clone(), named]);
        db.lock().unwrap().set_deleted(3, true).unwrap();
        let found = find(&db, true);
        // Unknown players: exact copies only. The deleted game is left out.
        assert_eq!(found.groups, [group(0, &[1], true), group(2, &[4], true)]);
        assert_eq!(found.games, 4);
        let fuzzy_unknown = pgn("NN", "?", "????.??.??", "*", "", LONG);
        let (_d, db) = db_of(&[unknown.clone(), fuzzy_unknown.replace("Open", "Other")]);
        assert_eq!(find(&db, true).groups, []);
        // One move of unknown players: no telling.
        let short = pgn("?", "?", "????.??.??", "*", "", "1. e4 e5");
        let (_d, db) = db_of(&[short.clone(), short]);
        assert_eq!(find(&db, false).groups, []);
    }

    /// R1-3: Mega Database's odds games of 1869, recorded by their first
    /// move and the result: one move is as good as none.
    #[test]
    fn a_series_of_games_recorded_by_their_first_move() {
        let odds = |date: &str| {
            pgn(
                "Zerega, Augustus",
                "Mackenzie, George Henry",
                date,
                "0-1",
                "",
                "{Odds of f7 and two moves} 1. e4",
            )
            .replace("Round \"3\"", "Round \"?\"")
        };
        let (_d, db) = db_of(&[odds("1869.05.??"), odds("1869.05.??"), odds("1869.05.??")]);
        for fuzzy in [false, true] {
            assert_eq!(find(&db, fuzzy).groups, [], "fuzzy: {fuzzy}");
        }
        // Even on one day they could have played twice; with the round it's
        // one game recorded twice.
        let (_d, db) = db_of(&[odds("1869.05.12"), odds("1869.05.12")]);
        assert_eq!(find(&db, false).groups, []);
        let round = odds("1869.05.12").replace("Round \"?\"", "Round \"4\"");
        let (_d, db) = db_of(&[round.clone(), round]);
        assert_eq!(find(&db, false).groups, [group(0, &[1], true)]);
    }

    /// R1-1: Mega Database's short draws that follow a longer game's
    /// opening in another event of the same year are games of their own; a
    /// copy that lacks only the last move is a copy.
    #[test]
    fn a_short_game_that_follows_another_games_opening() {
        let game = |event: &str, moves: &str| {
            pgn("Dueckstein, Andreas", "Brestian, Egon", "1990.??.??", "1/2-1/2", "", moves)
                .replace("Open", event)
                .replace("Round \"3\"", "Round \"?\"")
        };
        let (_d, db) = db_of(&[game("AUT-chT", FULL), game("AUT-chT playoff", CUT)]);
        assert_eq!(find(&db, true).groups, []);
        // The same event, or the source left out the last move.
        let (_d, db) = db_of(&[game("AUT-chT", FULL), game("AUT-chT", CUT)]);
        assert_eq!(find(&db, true).groups, [group(0, &[1], false)]);
        let last_move_less = FULL.trim_end_matches(" Bf6");
        let (_d, db) = db_of(&[game("AUT-chT", FULL), game("Austria", last_move_less)]);
        assert_eq!(find(&db, true).groups, [group(0, &[1], false)]);
    }

    #[test]
    fn set_up_positions_must_match() {
        let fen = |f: &str| format!("[SetUp \"1\"]\n[FEN \"{f}\"]");
        let a = "8/8/8/4k3/8/8/4P3/4K3 w - - 0 1";
        let b = "8/8/8/3k4/8/8/4P3/4K3 w - - 0 1";
        let moves = "1. Kd2";
        let games = [
            pgn("Study, A", "Study, B", "2000.01.01", "*", &fen(a), moves),
            pgn("Study, A", "Study, B", "2000.01.01", "*", &fen(b), moves),
            pgn("Study, A", "Study, B", "2000.01.01", "*", &fen(a), moves),
        ];
        let (_d, db) = db_of(&games);
        assert_eq!(find(&db, true).groups, [group(0, &[2], true)]);
    }

    /// `n` main lines (movetext) that start with `head` and go on at random
    /// for `more` plies.
    fn random_lines(n: usize, head: &[&str], more: usize, seed: u64) -> Vec<String> {
        let mut seed = seed;
        (0..n)
            .map(|_| {
                let mut pos = Chess::default();
                let mut sans: Vec<String> = Vec::new();
                for san in head {
                    let m = san.parse::<San>().unwrap().to_move(&pos).unwrap();
                    sans.push(san.to_string());
                    pos.play_unchecked(m);
                }
                for _ in 0..more {
                    let legal = pos.legal_moves();
                    if legal.is_empty() {
                        break;
                    }
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let m = legal[((seed >> 33) as usize) % legal.len()];
                    sans.push(San::from_move(&pos, m).to_string());
                    pos.play_unchecked(m);
                }
                let numbered = sans
                    .iter()
                    .enumerate()
                    .map(|(i, san)| if i % 2 == 0 { format!("{}. {san}", i / 2 + 1) } else { san.clone() });
                numbered.collect::<Vec<_>>().join(" ")
            })
            .collect()
    }

    /// R1-9: many games of the same players that start alike: copies are
    /// found among them, Stop works while they're compared, and a copy that
    /// stops early is left alone when too many games start with its moves.
    #[test]
    fn a_crowd_of_games_that_start_alike() {
        let head = ["e4", "c5", "Nf3", "d6", "d4", "cxd4", "Nxd4", "Nf6", "Nc3", "a6"];
        let game = |moves: &str, result: &str| {
            pgn("Engine, A", "Engine, B", "2024.??.??", result, "", moves)
                .replace("Round \"3\"", "Round \"?\"")
        };
        let mut games: Vec<String> =
            random_lines(3000, &head, 30, 7).iter().map(|l| game(l, "1/2-1/2")).collect();
        games.push(games[0].clone());
        let (_d, db) = db_of(&games);
        for fuzzy in [false, true] {
            assert_eq!(find(&db, fuzzy).groups, [group(0, &[3000], true)], "fuzzy: {fuzzy}");
        }

        /// Stops once the fuzzy comparisons have begun.
        struct StopWhenComparing(Cell<bool>);
        impl Progress for StopWhenComparing {
            fn report(&self, done: u64, total: u64) {
                if total == 1000 && done >= 950 {
                    self.0.set(true);
                }
            }
            fn is_cancelled(&self) -> bool {
                self.0.get()
            }
        }
        let stop = StopWhenComparing(Cell::new(false));
        let result = find_duplicates(&db, DuplicateOptions { fuzzy: true }, &stop);
        assert!(matches!(result, Err(DbError::Cancelled)), "{result:?}");

        // A copy that stops early (20 plies, 1-0), and games that start with
        // its moves, of which only the first has its result.
        let english = ["Be3", "e5", "Nb3", "Be6", "f3", "Be7", "Qd2", "O-O", "O-O-O", "Nbd7"];
        let start: Vec<&str> = head.iter().chain(&english).copied().collect();
        let longer = random_lines(CROWD + 6, &start, 10, 3);
        let crowd = |n: usize| {
            let mut games: Vec<String> = longer[..n]
                .iter()
                .enumerate()
                .map(|(i, l)| game(l, if i == 0 { "1-0" } else { "0-1" }))
                .collect();
            games.push(game(&random_lines(1, &start, 0, 0)[0], "1-0"));
            games
        };
        let (_d, db) = db_of(&crowd(10));
        assert_eq!(find(&db, true).groups, [group(0, &[10], false)]);
        let (_d, db) = db_of(&crowd(CROWD + 6));
        assert_eq!(find(&db, true).groups, [], "too many games start that way to tell");
    }

    /// Cancels at the first report.
    struct CancelAtOnce(Cell<bool>);

    impl Progress for CancelAtOnce {
        fn report(&self, _done: u64, _total: u64) {
            self.0.set(true);
        }
        fn is_cancelled(&self) -> bool {
            self.0.get()
        }
    }

    #[test]
    fn cancelling_stops_it() {
        let plain = pgn("A, B", "C, D", "2000.01.01", "1-0", "", LONG);
        let (_d, db) = db_of(&[plain.clone(), plain]);
        let cancel = CancelAtOnce(Cell::new(false));
        assert!(matches!(
            find_duplicates(&db, DuplicateOptions { fuzzy: true }, &cancel),
            Err(DbError::Cancelled)
        ));
    }

    /// Compacts the database at the first report, as the user might while it runs.
    struct CompactMeanwhile<'a>(&'a Mutex<Database>, Cell<bool>);

    impl Progress for CompactMeanwhile<'_> {
        fn report(&self, _done: u64, _total: u64) {
            if !self.1.replace(true) {
                self.0.lock().unwrap().compact(&NoProgress).unwrap();
            }
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_compaction_meanwhile_makes_it_fail() {
        let plain = pgn("A, B", "C, D", "2000.01.01", "1-0", "", LONG);
        let (_d, db) = db_of(&[plain.clone(), plain.clone(), plain]);
        db.lock().unwrap().set_deleted(0, true).unwrap();
        let progress = CompactMeanwhile(&db, Cell::new(false));
        let err = find_duplicates(&db, DuplicateOptions::default(), &progress).unwrap_err();
        assert!(err.to_string().contains("compacted"), "{err}");
        assert_eq!(find(&db, false).groups, [group(0, &[1], true)], "renumbered: fine when run again");
    }
}
