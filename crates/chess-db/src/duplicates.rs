//! Duplicate games (DB-08, ChessBase's "Double game killer"): copies of the
//! same game, as importing a file twice or merging two sources leaves.
//!
//! - **Exact copies** have the same moves (the main line, from the same
//!   start) and the same White, Black, their ratings, Event, Site, Date,
//!   Round and Result. Annotations, ECO and the annotator may differ. (Online
//!   rematches can repeat a short game; the ratings change between them.)
//! - **Fuzzy copies** (optional) have the same players, allowing for how
//!   names are written ("Carlsen, Magnus", "Carlsen, M.", "Magnus
//!   Carlsen"), dates and results that agree where both are known, and
//!   moves where one copy's main line is the start of the other's (a source
//!   that stopped early). Copies shorter than 10 moves also need the same
//!   full date. Games whose rounds, players' ratings or Site URLs (one per
//!   online game) differ, where both have them, are never copies. Event and
//!   Site names may differ, and one copy may lack the ratings.
//!
//! Each group keeps one copy: the one with the most variations, comments,
//! symbols and arrows; then the longest main line; then the most complete
//! headers; then the most clock and eval commands; then the first in the
//! database. The others are the ones to delete. Every one of them is a copy
//! of the kept game itself: fuzzy matches don't chain, so a game that
//! matches two different games doesn't join them.
//!
//! It scales to millions of games because it never compares all pairs: the
//! index records are scanned first and bucketed by their headers (exact) or
//! players (fuzzy), and only the games that share a bucket have their moves
//! read. Within a bucket, fuzzy matches are looked for among games that also
//! share their first moves. Games marked as deleted are left out.

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
/// Fuzzy matches are looked for among games that share this many first
/// plies; shorter games are compared with every game of their players.
const HEAD_PLIES: usize = 10;
/// Index records read per lock.
const CHUNK: u32 = 64 * 1024;
/// Names that stand for an unknown player (after folding and dropping
/// everything but letters and digits).
const UNKNOWN_PLAYERS: [&str; 3] = ["nn", "unknown", "anonymous"];

/// What counts as a copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DuplicateOptions {
    /// Fuzzy copies too, not only exact ones (see the module docs).
    pub fuzzy: bool,
}

/// Copies of one game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// The copy to keep: the best annotated (see the module docs).
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
/// name is unknown) and the first letter of the given name, if any.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PlayerKey {
    surname: u64,
    initial: Option<char>,
}

/// "Last, First" is PGN's form; without a comma the last word is taken as
/// the surname ("Magnus Carlsen"). Case, accents and punctuation don't count.
fn player_key(name: &str) -> PlayerKey {
    let folded = crate::fold(name);
    let (surname, given) = match folded.split_once(',') {
        Some((last, first)) => (last, first),
        None => match folded.trim().rsplit_once(char::is_whitespace) {
            Some((first, last)) => (last, first),
            None => (folded.as_str(), ""),
        },
    };
    let surname: String = surname.chars().filter(|c| c.is_alphanumeric()).collect();
    if surname.is_empty() || UNKNOWN_PLAYERS.contains(&surname.as_str()) {
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
    /// Variations, comments, symbols and arrows.
    notes: u32,
    plies: u32,
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

/// A game whose moves were read. Only hashes of the moves are kept: a
/// million of these take about 70 MB.
struct Candidate {
    id: GameId,
    tags: Tags,
    start: u64,
    line: u64,
    plies: u32,
    quality: Quality,
}

impl Candidate {
    fn new(id: GameId, rec: &IndexRecord, blob: &[u8]) -> Result<Candidate, DbError> {
        let d = digest(blob)?;
        let (start, line) = line_of(&d);
        let plies = d.mainline.len() as u32;
        Ok(Candidate {
            id,
            tags: Tags::of(rec),
            start,
            line,
            plies,
            quality: Quality {
                notes: d.variations + d.comments + d.nags + d.shapes,
                plies,
                headers: header_score(rec, d.extra_tags),
                commands: d.commands,
                first: Reverse(id),
            },
        })
    }

    /// What exact copies have in common. 64 bits of the line are plenty:
    /// two lines must also have the same headers and length to be confused.
    fn exact_key(&self) -> (Tags, u64, u64, u32) {
        (self.tags, self.start, self.line, self.plies)
    }
}

/// Year, month and day agree where both dates know them.
fn dates_agree(a: u32, b: u32) -> bool {
    let agree = |x: u32, y: u32| x == 0 || y == 0 || x == y;
    agree(a >> 9, b >> 9) && agree((a >> 5) & 0xf, (b >> 5) & 0xf) && agree(a & 0x1f, b & 0x1f)
}

fn full_date(d: u32) -> bool {
    d >> 9 != 0 && (d >> 5) & 0xf != 0 && d & 0x1f != 0
}

/// Whether `a` (with main line `am`) and `b` (`bm`) are fuzzy copies (see
/// the module docs). Their players' surnames are known to agree (they share
/// a bucket).
fn fuzzy_match(a: &Candidate, am: &[u8], b: &Candidate, bm: &[u8], names: &Names) -> bool {
    let (ta, tb) = (&a.tags, &b.tags);
    let initials = |x: u32, y: u32| match (names.player(x).initial, names.player(y).initial) {
        (Some(p), Some(q)) => p == q,
        _ => true,
    };
    let known_same = |x: u16, y: u16| x == y || x == 0 || y == 0;
    let (short, long) = if am.len() <= bm.len() { (am, bm) } else { (bm, am) };
    a.start == b.start
        && initials(ta.white, tb.white)
        && initials(ta.black, tb.black)
        && long.starts_with(short)
        && dates_agree(ta.date, tb.date)
        && (short.len() >= SHORT_PLIES || (full_date(ta.date) && ta.date == tb.date))
        && (ta.result == tb.result || ta.result == 0 || tb.result == 0)
        && known_same(ta.round, tb.round)
        && known_same(ta.white_elo, tb.white_elo)
        && known_same(ta.black_elo, tb.black_elo)
        && (ta.site == tb.site || !names.is_url(ta.site) || !names.is_url(tb.site))
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

/// Finds the duplicate games of `db` (see the module docs). The database is
/// locked only while index records and games are read, a batch at a time.
/// Fails if the database is compacted meanwhile (game ids change).
pub fn find_duplicates(
    db: &Mutex<Database>,
    options: DuplicateOptions,
    progress: &dyn Progress,
) -> Result<Duplicates, DbError> {
    let (count, compactions, names) = {
        let db = db.lock().expect("db lock");
        (db.count(), db.compactions(), Names::read(&db))
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
            exact.push((hash_of(Tags::of(r)), id));
            if options.fuzzy
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
        Candidate::new(id, rec, blob).inspect_err(|e| log::warn!("duplicates: skipping game {id}: {e}")).ok()
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

    // 3. Exact copies: the same key, best first.
    let mut order: Vec<usize> = (0..copies.len()).collect();
    order.sort_unstable_by_key(|&i| (copies[i].exact_key(), Reverse(copies[i].quality)));
    let classes: Vec<&[usize]> =
        order.chunk_by(|&a, &b| copies[a].exact_key() == copies[b].exact_key()).collect();
    progress.check_cancelled()?;

    // 4. Fuzzy copies: between the exact classes (their best copies) of
    // the same players.
    let mut merged: Vec<Vec<usize>> = Vec::new();
    if options.fuzzy {
        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        for (ci, class) in classes.iter().enumerate() {
            let tags = &copies[class[0]].tags;
            match names.players_key(tags.white, tags.black) {
                Some(key) => buckets.entry(key).or_default().push(ci),
                None => merged.push(vec![ci]),
            }
        }
        // Buckets where two classes could match: one class alone can't, nor
        // can online games with pages of their own.
        let (mut open, closed): (Vec<Vec<usize>>, Vec<Vec<usize>>) = buckets.into_values().partition(|b| {
            let mut sites: Vec<u32> = b.iter().map(|&ci| copies[classes[ci][0]].tags.site).collect();
            sites.sort_unstable();
            b.len() > 1 && (sites.iter().any(|&s| !names.is_url(s)) || sites.windows(2).any(|w| w[0] == w[1]))
        });
        merged.extend(closed.into_iter().flatten().map(|ci| vec![ci]));

        // Their main lines, read again (only the hashes were kept).
        let mut leaders: Vec<usize> = open.iter().flatten().map(|&ci| classes[ci][0]).collect();
        leaders.sort_unstable_by_key(|&i| copies[i].id);
        let ids: Vec<GameId> = leaders.iter().map(|&i| copies[i].id).collect();
        let mut arena: Vec<u8> = Vec::new();
        let mut spans: HashMap<usize, (usize, usize)> = HashMap::with_capacity(leaders.len());
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
                }
            }
            Ok(())
        });
        still_numbered()?;
        result?;
        let moves = |i: usize| spans.get(&i).map(|&(at, len)| &arena[at..at + len]);
        for (n, bucket) in open.iter_mut().enumerate() {
            if n.is_multiple_of(1024) {
                progress.check_cancelled()?;
            }
            bucket.sort_unstable_by_key(|&ci| Reverse(copies[classes[ci][0]].quality));
            merged.extend(cluster(bucket, &classes, &copies, &moves, &names));
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
    Ok(Duplicates { groups, games, compared })
}

/// Fuzzy groups among `bucket` (exact classes of the same players, best
/// first): each class joins the first group whose leader it matches, or
/// leads a new one. The leader is then the best of its group, and every
/// member matches it. A class whose moves couldn't be read again stays alone.
fn cluster<'m>(
    bucket: &[usize],
    classes: &[&[usize]],
    copies: &[Candidate],
    moves: &dyn Fn(usize) -> Option<&'m [u8]>,
    names: &Names,
) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    // The leader of each group, with its moves.
    let mut leaders: Vec<(&Candidate, Option<&[u8]>)> = Vec::new();
    // Groups whose leader has at least HEAD_PLIES plies, by those plies;
    // and the others.
    let mut by_head: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    let mut short: Vec<usize> = Vec::new();
    for &ci in bucket {
        let c = &copies[classes[ci][0]];
        let Some(m) = moves(classes[ci][0]) else {
            groups.push(vec![ci]);
            leaders.push((c, None));
            continue;
        };
        let head = (m.len() >= HEAD_PLIES).then(|| (c.start, hash_of(&m[..HEAD_PLIES])));
        let matches = |g: &usize| {
            let (leader, lm) = leaders[*g];
            lm.is_some_and(|lm| fuzzy_match(leader, lm, c, m, names))
        };
        // Group numbers grow with the leaders' order: the smallest match
        // has the best leader.
        let found = match head {
            Some(head) => {
                let same_head = by_head.get(&head).and_then(|gs| gs.iter().copied().find(matches));
                let shorter = short.iter().copied().find(matches);
                same_head.into_iter().chain(shorter).min()
            }
            None => (0..groups.len()).find(matches),
        };
        match found {
            Some(g) => groups[g].push(ci),
            None => {
                match head {
                    Some(head) => by_head.entry(head).or_default().push(groups.len()),
                    None => short.push(groups.len()),
                }
                groups.push(vec![ci]);
                leaders.push((c, Some(m)));
            }
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use chess_core::pgn::read_pgn_str;

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
        for same in ["Carlsen,M.", "CARLSEN, M", "Magnus Carlsen", " carlsen ,  magnus "] {
            let k = key(same);
            assert_eq!((k.surname, k.initial), (carlsen.surname, Some('m')), "{same}");
        }
        assert_eq!(key("Carlsen").surname, carlsen.surname);
        assert_eq!(key("Carlsen").initial, None);
        assert_eq!(key("Réti, R.").surname, key("Reti, Richard").surname);
        assert_eq!(key("Vachier-Lagrave, M").surname, key("Vachier Lagrave, Maxime").surname);
        assert_ne!(key("Carlsen, Magnus").surname, key("Caruana, Fabiano").surname);
        for unknown in ["", "?", "-", "NN", "N.N.", "Unknown", "  "] {
            assert_eq!(key(unknown), PlayerKey::default(), "{unknown:?}");
        }
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

        db.lock().unwrap().set_deleted(1, true).unwrap();
        assert_eq!(find(&db, false).groups, [group(0, &[2, 6], true)]);
        // The copy with a rating and the one with another event are fuzzy
        // copies; without annotations, the most complete headers win (the
        // rating).
        assert_eq!(find(&db, true).groups, [group(3, &[0, 2, 4, 6], false)]);
    }

    #[test]
    fn fuzzy_copies_allow_other_names_and_a_shorter_copy() {
        let full = LONG;
        let cut = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3 O-O 9. h3 Nb8 \
                   10. d4 Nbd7";
        let games = [
            // 0: the full game.
            pgn("Carlsen, Magnus", "Caruana, Fabiano", "2024.03.01", "1/2-1/2", "", full),
            // 1: the same game from another source, stopped early.
            pgn("Carlsen, M.", "Caruana, F.", "2024.??.??", "1/2-1/2", "", cut)
                .replace("[Event \"Open\"]", "[Event \"Prague Masters\"]"),
            // 2: the same game, names First Last, no round or site.
            pgn("Magnus Carlsen", "Fabiano Caruana", "2024.03.01", "*", "", full)
                .replace("[Round \"3\"]", "[Round \"?\"]")
                .replace("[Site \"Prague\"]", "[Site \"?\"]"),
            // 3: another game: they played the same line, then something else.
            pgn(
                "Carlsen, Magnus",
                "Caruana, Fabiano",
                "2024.03.05",
                "1/2-1/2",
                "",
                &full.replace("Re8", "h6"),
            ),
            // 4: another round.
            pgn("Carlsen, Magnus", "Caruana, Fabiano", "2024.03.01", "1/2-1/2", "", full)
                .replace("[Round \"3\"]", "[Round \"4\"]"),
            // 5: another date.
            pgn("Carlsen, Magnus", "Caruana, Fabiano", "2024.03.02", "1/2-1/2", "", full),
            // 6: another result.
            pgn("Carlsen, Magnus", "Caruana, Fabiano", "2024.03.01", "1-0", "", full),
            // 7: another player with the same surname.
            pgn("Carlsen, Henrik", "Caruana, Fabiano", "2024.03.01", "1/2-1/2", "", full),
            // 8: colours the other way round.
            pgn("Caruana, Fabiano", "Carlsen, Magnus", "2024.03.01", "1/2-1/2", "", full),
        ];
        let (_d, db) = db_of(&games);
        assert_eq!(find(&db, false).groups, [], "no exact copies");
        let found = find(&db, true);
        // 0 and 2 have the longest line; 0 has more headers.
        assert_eq!(found.groups, [group(0, &[1, 2], false)]);
        assert_eq!(found.compared, 8, "colours the other way round are another bucket");
    }

    #[test]
    fn ratings_must_agree_where_both_copies_have_them() {
        let elo = |w: &str| if w.is_empty() { String::new() } else { format!("[WhiteElo \"{w}\"]") };
        let game = |w: &str| pgn("Smith, John", "Jones, Bob", "2010.05.05", "1-0", &elo(w), LONG);
        let other = |g: String, event: &str| g.replace("Open", event);
        let (_d, db) = db_of(&[game("2400"), other(game("2380"), "Open A"), other(game(""), "Open B")]);
        // 0 and 1 aren't copies (they could have played twice that day);
        // the copy without a rating could be either: it goes with the first.
        assert_eq!(find(&db, true).groups, [group(0, &[2], false)]);
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
        // The game without its full date stays; the 2-move stub has it.
        assert_eq!(find(&db, true).groups, [group(0, &[1, 3], false)]);
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

    #[test]
    fn fuzzy_matches_dont_chain() {
        // A stops early; B and C are different games that both start with A's moves.
        let a = "1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3 O-O 9. h3 Nb8 \
                 10. d4 Nbd7";
        let b = format!("{a} 11. Nbd2 Bb7");
        let c = format!("{a} 11. c4 c6");
        let games = [
            pgn("Anand, V", "Kramnik, V", "2008.??.??", "1/2-1/2", "", a),
            pgn("Anand, V", "Kramnik, V", "2008.10.14", "1/2-1/2", "", &b),
            pgn("Anand, V", "Kramnik, V", "2008.10.20", "1/2-1/2", "", &c)
                .replace("[Round \"3\"]", "[Round \"?\"]"),
        ];
        let (_d, db) = db_of(&games);
        // A goes with the first of the longest games; C, which doesn't match B, stays.
        assert_eq!(find(&db, true).groups, [group(1, &[0], false)]);
    }

    #[test]
    fn deleted_games_and_unknown_players() {
        let unknown = pgn("?", "?", "????.??.??", "*", "", "1. e4 e5 2. Nf3");
        let named = pgn("Lasker, E", "Tarrasch, S", "1908.??.??", "1-0", "", LONG);
        let (_d, db) = db_of(&[unknown.clone(), unknown.clone(), named.clone(), named.clone(), named]);
        db.lock().unwrap().set_deleted(3, true).unwrap();
        let found = find(&db, true);
        // Unknown players: exact copies only. The deleted game is left out.
        assert_eq!(found.groups, [group(0, &[1], true), group(2, &[4], true)]);
        assert_eq!(found.games, 4);
        let fuzzy_unknown = pgn("NN", "?", "????.??.??", "*", "", "1. e4 e5 2. Nf3 Nc6");
        let (_d, db) = db_of(&[unknown.clone(), fuzzy_unknown.replace("Open", "Other")]);
        assert_eq!(find(&db, true).groups, []);
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
