//! Searching several databases at once (SRCH-13): each database's results,
//! already in one sort order (and cached by the caller, as a database's own
//! game list caches them), merged into one list without copying them.
//!
//! A page at any offset is found by selection, not by merging from the
//! start: the cut through every list where the page begins is found by a
//! binary search over the lists at once (each step drops a quarter of the
//! rows left), reading only the index records it compares. So a page deep
//! into millions of merged rows costs a few thousand record reads, and the
//! merged list is never materialized.
//!
//! The merged order is the sort order of each database extended across
//! them: names compare as each database sorts them (case-insensitive, then
//! by the database they're in), numbers by value; ties go by database (the
//! order the sources are given in), then game id. Sorting by id lists the
//! databases one after the other.

use std::cmp::Ordering;
use std::sync::{Arc, Mutex};

use chess_core::shakmaty::Chess;

use crate::database::NameKind;
use crate::sort::sort_value;
use crate::{
    Database, DbError, GameId, GameRow, GameSet, HeaderQuery, Progress, SortKey, SortSpec, ensure_sort,
    find_position, search_headers, sorted_subset,
};

/// One database's results in a sort order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortedIds {
    /// Every game, in id order (or the reverse): the unfiltered list in
    /// natural order, without a list of its ids.
    Range { count: GameId, descending: bool },
    /// Game ids in sort order: a search's results, or a sort permutation
    /// (shared, never copied).
    List(Arc<Vec<GameId>>),
}

impl SortedIds {
    pub fn len(&self) -> usize {
        match self {
            SortedIds::Range { count, .. } => *count as usize,
            SortedIds::List(ids) => ids.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The game at position `i` (`i < len()`).
    pub fn get(&self, i: usize) -> GameId {
        match self {
            SortedIds::Range { count, descending: false } => (i as GameId).min(count.saturating_sub(1)),
            SortedIds::Range { count, descending: true } => count.saturating_sub(1 + i as GameId),
            SortedIds::List(ids) => ids[i],
        }
    }

    /// Positions `from..to` as game ids.
    pub fn slice(&self, from: usize, to: usize) -> Vec<GameId> {
        let to = to.min(self.len());
        (from.min(to)..to).map(|i| self.get(i)).collect()
    }
}

/// A database and its results in the merge's sort order.
#[derive(Clone, Copy)]
pub struct CrossSource<'a> {
    pub db: &'a Mutex<Database>,
    pub ids: &'a SortedIds,
}

/// The rows in all the sources together.
pub fn merged_total(sources: &[CrossSource<'_>]) -> u64 {
    sources.iter().map(|s| s.ids.len() as u64).sum()
}

/// What a row is compared by: see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    unknown: bool,
    value: Value,
    source: usize,
    id: GameId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    /// Sorting by id: databases one after the other.
    Id(usize, GameId),
    Num(u64),
    /// A name as the database sorts it: lowercased, then by database and
    /// name id (each database orders equal lowercased names by id).
    Name(String, usize, u32),
}

fn compare(a: &Key, b: &Key, descending: bool) -> Ordering {
    let value = a.value.cmp(&b.value);
    a.unknown
        .cmp(&b.unknown)
        .then(if descending { value.reverse() } else { value })
        .then(a.source.cmp(&b.source))
        .then(a.id.cmp(&b.id))
}

fn name_kind(key: SortKey) -> Option<NameKind> {
    match key {
        SortKey::White | SortKey::Black => Some(NameKind::Player),
        SortKey::Event => Some(NameKind::Event),
        SortKey::Site => Some(NameKind::Site),
        _ => None,
    }
}

struct Merger<'s, 'a> {
    sources: &'s [CrossSource<'a>],
    spec: SortSpec,
}

impl Merger<'_, '_> {
    fn key(&self, source: usize, i: usize) -> Result<Key, DbError> {
        let src = &self.sources[source];
        let id = src.ids.get(i);
        let key = self.spec.key;
        let db = src.db.lock().expect("db lock");
        let r = db.record(id)?;
        let (unknown, value) = match key {
            SortKey::Id => (false, Value::Id(source, id)),
            _ => match name_kind(key) {
                Some(kind) => {
                    let name = match key {
                        SortKey::White => r.white,
                        SortKey::Black => r.black,
                        SortKey::Event => r.event,
                        _ => r.site,
                    };
                    (name == 0, Value::Name(db.names(kind).get(name).to_lowercase(), source, name))
                }
                None => {
                    let (unknown, v) = sort_value(key, &r, &[]);
                    (unknown, Value::Num(v))
                }
            },
        };
        Ok(Key { unknown, value, source, id })
    }

    fn cmp(&self, a: &Key, b: &Key) -> Ordering {
        compare(a, b, self.spec.descending)
    }

    /// The first position in `lo..hi` of source `s` whose row sorts after
    /// `pivot` (a row of another source), i.e. how many of its rows there
    /// sort before it.
    fn rank(&self, s: usize, mut lo: usize, mut hi: usize, pivot: &Key) -> Result<usize, DbError> {
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.cmp(&self.key(s, mid)?, pivot) == Ordering::Less {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }

    /// How many rows of each source come before row `n` of the merged list.
    fn cut(&self, n: u64) -> Result<Vec<usize>, DbError> {
        let k = self.sources.len();
        let mut lo = vec![0usize; k];
        let mut hi: Vec<usize> = self.sources.iter().map(|s| s.ids.len()).collect();
        if n == 0 {
            return Ok(lo);
        }
        if n >= hi.iter().map(|&h| h as u64).sum::<u64>() {
            return Ok(hi);
        }
        // Each round takes the weighted median of the medians of what's
        // left of each source, finds where it falls in every source, and
        // keeps the side the cut is on: at least a quarter of the rows left
        // go each round. Each round also shrinks the pivot's source by one
        // at least, so it ends even if a list were out of order.
        loop {
            let mut medians = Vec::with_capacity(k);
            for s in 0..k {
                if lo[s] < hi[s] {
                    let m = lo[s] + (hi[s] - lo[s]) / 2;
                    medians.push((self.key(s, m)?, s, m, (hi[s] - lo[s]) as u64));
                }
            }
            if medians.is_empty() {
                return Ok(lo);
            }
            medians.sort_by(|a, b| self.cmp(&a.0, &b.0));
            let half = medians.iter().map(|m| m.3).sum::<u64>().div_ceil(2);
            let mut acc = 0;
            let (pivot, ps, pm) = medians
                .into_iter()
                .find_map(|(key, s, m, w)| {
                    acc += w;
                    (acc >= half).then_some((key, s, m))
                })
                .expect("the weights add up");
            let mut ranks = vec![0usize; k];
            for s in 0..k {
                ranks[s] = if s == ps { pm } else { self.rank(s, lo[s], hi[s], &pivot)? };
            }
            if ranks.iter().map(|&r| r as u64).sum::<u64>() < n {
                // The pivot is before the cut, and so is all that sorts before it.
                for s in 0..k {
                    lo[s] = if s == ps { pm + 1 } else { ranks[s] };
                }
            } else {
                for s in 0..k {
                    hi[s] = ranks[s];
                }
            }
        }
    }
}

/// Rows `offset..offset + limit` of the sources merged in `spec` order (each
/// source's ids must be in that order), as (source index, game id).
pub fn merge_page(
    sources: &[CrossSource<'_>],
    spec: SortSpec,
    offset: u64,
    limit: usize,
) -> Result<Vec<(usize, GameId)>, DbError> {
    let m = Merger { sources, spec };
    let mut cursor = m.cut(offset)?;
    let mut heads: Vec<Option<Key>> = Vec::with_capacity(sources.len());
    for (s, src) in sources.iter().enumerate() {
        heads.push(if cursor[s] < src.ids.len() { Some(m.key(s, cursor[s])?) } else { None });
    }
    let mut out = Vec::with_capacity(limit.min(1024));
    while out.len() < limit {
        let mut best: Option<usize> = None;
        for (s, head) in heads.iter().enumerate() {
            if let Some(h) = head
                && best.is_none_or(|b| m.cmp(h, heads[b].as_ref().expect("a head")) == Ordering::Less)
            {
                best = Some(s);
            }
        }
        let Some(s) = best else { break };
        let head = heads[s].take().expect("the best has a head");
        out.push((s, head.id));
        cursor[s] += 1;
        if cursor[s] < sources[s].ids.len() {
            heads[s] = Some(m.key(s, cursor[s])?);
        }
    }
    Ok(out)
}

/// One database's results for `query` (within `within`, e.g. the games
/// reaching a position), in `spec` order. Builds the sort order covering
/// every game if needed ([`ensure_sort`]). The unfiltered list comes as the
/// sort permutation itself, or a range in natural order, never a copy.
pub fn sorted_ids(
    db: &Mutex<Database>,
    query: &HeaderQuery,
    within: Option<&GameSet>,
    spec: SortSpec,
    progress: &dyn Progress,
) -> Result<SortedIds, DbError> {
    let everything = within.is_none()
        && (HeaderQuery { skip_deleted: false, ..query.clone() }) == HeaderQuery::default()
        && (!query.skip_deleted || !db.lock().expect("db lock").game_stats()?.iter().any(|g| g.deleted));
    let perm = if spec.is_natural_order() { None } else { Some(ensure_sort(db, spec, progress)?) };
    if everything {
        return Ok(match perm {
            Some(perm) => SortedIds::List(perm),
            None => {
                SortedIds::Range { count: db.lock().expect("db lock").count(), descending: spec.descending }
            }
        });
    }
    let found = search_headers(db, query, within, progress)?;
    let ordered = match perm {
        None if spec.descending => found.into_iter().rev().collect(),
        None => found,
        Some(_) => sorted_subset(&db.lock().expect("db lock"), spec, &found)?,
    };
    Ok(SortedIds::List(Arc::new(ordered)))
}

/// A page of rows from several databases.
#[derive(Debug, Clone)]
pub struct CrossPage {
    /// Matching games in each database, in the order given.
    pub counts: Vec<u64>,
    pub total: u64,
    /// (database index, row).
    pub rows: Vec<(usize, GameRow)>,
}

/// Searches `dbs` for `query`, limited to the games reaching `position`
/// (through each one's position index, SRCH-03) when given, and returns
/// rows `offset..offset + limit` of the results merged in `spec` order.
/// Callers that page through results keep the [`sorted_ids`] of each
/// database and call [`merge_page`]; this does it all once.
pub fn search_many(
    dbs: &[&Mutex<Database>],
    query: &HeaderQuery,
    position: Option<&Chess>,
    spec: SortSpec,
    offset: u64,
    limit: usize,
    progress: &dyn Progress,
) -> Result<CrossPage, DbError> {
    let mut lists = Vec::with_capacity(dbs.len());
    for db in dbs {
        let within = match position {
            Some(pos) => {
                let hits = find_position(db, pos)?;
                Some(GameSet::from_ids(&hits.entries.iter().map(|e| e.game).collect::<Vec<_>>()))
            }
            None => None,
        };
        lists.push(sorted_ids(db, query, within.as_ref(), spec, progress)?);
    }
    let sources: Vec<CrossSource<'_>> =
        dbs.iter().zip(&lists).map(|(db, ids)| CrossSource { db, ids }).collect();
    let page = merge_page(&sources, spec, offset, limit)?;
    let mut rows = Vec::with_capacity(page.len());
    for (s, id) in page {
        rows.push((s, dbs[s].lock().expect("db lock").row(id)?));
    }
    Ok(CrossPage {
        counts: lists.iter().map(|l| l.len() as u64).collect(),
        total: merged_total(&sources),
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoProgress, build_sort, page_ids};
    use chess_core::pgn::read_pgn_str;

    fn db_with(pgn: &str, name: &str, dir: &std::path::Path) -> Mutex<Database> {
        let mut db = Database::create(&dir.join(name)).unwrap();
        for g in read_pgn_str(pgn) {
            db.append(&g).unwrap();
        }
        Mutex::new(db)
    }

    /// Games with every field from `n`, varied (some unknown).
    fn games(n: usize, seed: u64) -> String {
        let names = ["Carlsen, Magnus", "carlsen, magnus", "Anand, V", "Réti, R", "", "Tal, M", "anand, v"];
        let mut out = String::new();
        let mut x = seed;
        let mut next = |m: u64| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (x >> 33) % m
        };
        for _ in 0..n {
            let w = names[next(names.len() as u64) as usize];
            let b = names[next(names.len() as u64) as usize];
            let elo =
                if next(4) == 0 { String::new() } else { format!("[WhiteElo \"{}\"]", 2000 + next(700)) };
            let date =
                if next(5) == 0 { "????.??.??".to_owned() } else { format!("{}.01.01", 1990 + next(30)) };
            let res = ["1-0", "0-1", "1/2-1/2", "*"][next(4) as usize];
            let moves = ["1. e4 e5", "1. d4 d5 2. c4", "1. Nf3", "1. e4 c5 2. Nf3 d6"][next(4) as usize];
            out.push_str(&format!(
                "[Event \"E{}\"][White \"{w}\"][Black \"{b}\"]{elo}[Date \"{date}\"][Result \"{res}\"]\n{moves} {res}\n\n",
                next(3)
            ));
        }
        out
    }

    /// The merged list the slow way: every row of every source, keyed and sorted.
    fn reference(sources: &[CrossSource<'_>], spec: SortSpec) -> Vec<(usize, GameId)> {
        let m = Merger { sources, spec };
        let mut keys = Vec::new();
        for (s, src) in sources.iter().enumerate() {
            for i in 0..src.ids.len() {
                keys.push(m.key(s, i).unwrap());
            }
        }
        keys.sort_by(|a, b| m.cmp(a, b));
        keys.into_iter().map(|k| (k.source, k.id)).collect()
    }

    #[test]
    fn pages_at_any_offset_are_the_merged_list() {
        let dir = tempfile::tempdir().unwrap();
        let dbs = [
            db_with(&games(120, 1), "A", dir.path()),
            db_with(&games(45, 2), "B", dir.path()),
            db_with(&games(0, 3), "Empty", dir.path()),
            db_with(&games(80, 4), "C", dir.path()),
        ];
        let keys = [
            SortKey::Id,
            SortKey::White,
            SortKey::Black,
            SortKey::Event,
            SortKey::WhiteElo,
            SortKey::Date,
            SortKey::Result,
            SortKey::Plies,
        ];
        let query = HeaderQuery { skip_deleted: true, ..HeaderQuery::default() };
        let filtered = HeaderQuery { text: "carlsen".into(), ..HeaderQuery::default() };
        for key in keys {
            for descending in [false, true] {
                let spec = SortSpec::new(key, descending);
                for q in [&query, &filtered] {
                    let lists: Vec<SortedIds> =
                        dbs.iter().map(|db| sorted_ids(db, q, None, spec, &NoProgress).unwrap()).collect();
                    // Each list is that database's own list.
                    for (db, list) in dbs.iter().zip(&lists) {
                        if q == &query {
                            build_sort(db, spec, &NoProgress).unwrap();
                            let own = page_ids(&db.lock().unwrap(), spec, 0, 1000).unwrap();
                            assert_eq!(list.slice(0, list.len()), own, "{spec:?}");
                        }
                    }
                    let sources: Vec<_> =
                        dbs.iter().zip(&lists).map(|(db, ids)| CrossSource { db, ids }).collect();
                    let all = reference(&sources, spec);
                    assert_eq!(all.len() as u64, merged_total(&sources));
                    for offset in [0, 1, 7, 50, 119, 120, 121, 200, 244, 245, 300] {
                        for limit in [1, 13, 500] {
                            let page = merge_page(&sources, spec, offset, limit).unwrap();
                            let want: Vec<_> =
                                all.iter().skip(offset as usize).take(limit).copied().collect();
                            assert_eq!(page, want, "{spec:?} {q:?} at {offset}+{limit}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn deleted_games_and_positions() {
        let dir = tempfile::tempdir().unwrap();
        let a = db_with(&games(30, 5), "A", dir.path());
        let b = db_with(&games(30, 6), "B", dir.path());
        a.lock().unwrap().set_deleted_many(&[0, 1, 2], true).unwrap();
        crate::update_position_index(&a, &NoProgress).unwrap();
        crate::update_position_index(&b, &NoProgress).unwrap();
        let spec = SortSpec::new(SortKey::Date, true);
        let hide = HeaderQuery { skip_deleted: true, ..HeaderQuery::default() };
        let page = search_many(&[&a, &b], &hide, None, spec, 0, 1000, &NoProgress).unwrap();
        assert_eq!(page.counts, [27, 30]);
        assert!(page.rows.iter().all(|(_, r)| !r.deleted));
        let show = HeaderQuery::default();
        assert_eq!(search_many(&[&a, &b], &show, None, spec, 0, 0, &NoProgress).unwrap().total, 60);
        // Games after 1.e4 e5: the same as each database's own position search.
        let mut pos = Chess::default();
        for uci in ["e2e4", "e7e5"] {
            let m = chess_core::position::parse_uci(&pos, uci).unwrap();
            chess_core::shakmaty::Position::play_unchecked(&mut pos, m);
        }
        let page =
            search_many(&[&a, &b], &show, Some(&pos), SortSpec::NATURAL, 0, 1000, &NoProgress).unwrap();
        let expect = |db: &Mutex<Database>| find_position(db, &pos).unwrap().entries.len() as u64;
        assert_eq!(page.counts, [expect(&a), expect(&b)]);
        let ids: Vec<_> = page.rows.iter().map(|(s, r)| (*s, r.id)).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "by id: the first database's games, then the second's");
    }
}
