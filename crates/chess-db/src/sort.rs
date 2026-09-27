//! Sorted views of the game list. A sort builds a permutation of game ids
//! once (in a background job); pages are then served from it. Games added
//! after the sort was built are listed after the sorted ones, in id order,
//! until the next sort.

use std::cmp::Reverse;
use std::sync::{Arc, Mutex};

use crate::database::NameKind;
use crate::record::IndexRecord;
use crate::{Database, DbError, GameId, GameSet, Progress};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortKey {
    Id,
    White,
    Black,
    WhiteElo,
    BlackElo,
    Date,
    Event,
    Site,
    Round,
    Result,
    Eco,
    Plies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SortSpec {
    pub key: SortKey,
    pub descending: bool,
}

impl SortSpec {
    pub const NATURAL: SortSpec = SortSpec { key: SortKey::Id, descending: false };

    pub fn new(key: SortKey, descending: bool) -> SortSpec {
        SortSpec { key, descending }
    }

    /// Sorting by id needs no precomputed permutation.
    pub fn is_natural_order(&self) -> bool {
        self.key == SortKey::Id
    }
}

/// `(unknown, value)`: unknown values sort last in both directions.
fn sort_value(key: SortKey, r: &IndexRecord, ranks: &[u32]) -> (bool, u64) {
    let rank = |id: u32| u64::from(ranks.get(id as usize).copied().unwrap_or(u32::MAX));
    match key {
        SortKey::Id => (false, 0),
        SortKey::White => (r.white == 0, rank(r.white)),
        SortKey::Black => (r.black == 0, rank(r.black)),
        SortKey::Event => (r.event == 0, rank(r.event)),
        SortKey::Site => (r.site == 0, rank(r.site)),
        SortKey::WhiteElo => (r.white_elo == 0, u64::from(r.white_elo)),
        SortKey::BlackElo => (r.black_elo == 0, u64::from(r.black_elo)),
        SortKey::Date => (r.date == 0, u64::from(r.date)),
        SortKey::Round => (r.round == 0, (u64::from(r.round) << 16) | u64::from(r.subround)),
        SortKey::Result => (r.result == 0, u64::from(r.result)),
        SortKey::Eco => (r.eco == 0, u64::from(r.eco)),
        SortKey::Plies => (false, u64::from(r.plies)),
    }
}

fn name_kind(key: SortKey) -> Option<NameKind> {
    match key {
        SortKey::White | SortKey::Black => Some(NameKind::Player),
        SortKey::Event => Some(NameKind::Event),
        SortKey::Site => Some(NameKind::Site),
        _ => None,
    }
}

/// Builds (or returns the cached) permutation for `spec`. The database lock
/// is only held while reading records, not while sorting.
pub fn build_sort(
    db: &Mutex<Database>,
    spec: SortSpec,
    progress: &dyn Progress,
) -> Result<Arc<Vec<GameId>>, DbError> {
    const CHUNK: u32 = 64 * 1024;
    let (count, ranks, epoch) = {
        let db = db.lock().expect("db lock");
        if let Some(perm) = db.cached_sort(spec) {
            return Ok(perm);
        }
        let ranks = name_kind(spec.key).map(|k| db.names(k).ranks()).unwrap_or_default();
        (db.count(), ranks, db.sort_epoch())
    };

    let mut keyed: Vec<((bool, u64), GameId)> = Vec::with_capacity(count as usize);
    let mut start = 0;
    while start < count {
        progress.check_cancelled()?;
        let records = db.lock().expect("db lock").records(start, CHUNK)?;
        for (r, id) in records.iter().zip(start..) {
            keyed.push((sort_value(spec.key, r, &ranks), id));
        }
        start += CHUNK;
        progress.report(u64::from(start.min(count)), u64::from(count) * 2);
    }
    if spec.descending {
        keyed.sort_unstable_by_key(|&((unknown, v), id)| (unknown, Reverse(v), id));
    } else {
        keyed.sort_unstable_by_key(|&((unknown, v), id)| (unknown, v, id));
    }
    progress.report(u64::from(count) * 2, u64::from(count) * 2);
    let perm = Arc::new(keyed.into_iter().map(|(_, id)| id).collect::<Vec<_>>());

    let mut db = db.lock().expect("db lock");
    if db.sort_epoch() == epoch {
        db.store_sort(spec, perm.clone());
    }
    Ok(perm)
}

/// Ids for rows `offset..offset+limit` of the list in `spec` order.
/// Returns [`DbError::NotSorted`] if the permutation hasn't been built.
pub fn page_ids(db: &Database, spec: SortSpec, offset: u32, limit: u32) -> Result<Vec<GameId>, DbError> {
    let count = db.count();
    let end = offset.saturating_add(limit).min(count);
    if offset >= end {
        return Ok(Vec::new());
    }
    if spec.is_natural_order() {
        return Ok(if spec.descending {
            (offset..end).map(|i| count - 1 - i).collect()
        } else {
            (offset..end).collect()
        });
    }
    let perm = db.cached_sort(spec).ok_or(DbError::NotSorted)?;
    let sorted = perm.len() as u32;
    Ok((offset..end).map(|i| if i < sorted { perm[i as usize] } else { i }).collect())
}

/// `ids` (ascending, e.g. a search result) in `spec` order. Needs the
/// permutation for `spec` (else [`DbError::NotSorted`]); ids added after it
/// was built follow in id order.
pub fn sorted_subset(db: &Database, spec: SortSpec, ids: &[GameId]) -> Result<Vec<GameId>, DbError> {
    if spec.is_natural_order() {
        return Ok(if spec.descending { ids.iter().rev().copied().collect() } else { ids.to_vec() });
    }
    let perm = db.cached_sort(spec).ok_or(DbError::NotSorted)?;
    let set = GameSet::from_ids(ids);
    let sorted = perm.len() as GameId;
    let mut out: Vec<GameId> = perm.iter().copied().filter(|&id| set.contains(id)).collect();
    out.extend(ids.iter().copied().filter(|&id| id >= sorted));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoProgress;
    use chess_core::pgn::read_pgn_str;

    fn db_with(pgn: &str) -> (tempfile::TempDir, Mutex<Database>) {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Sort")).unwrap();
        for g in read_pgn_str(pgn) {
            db.append(&g).unwrap();
        }
        (dir, Mutex::new(db))
    }

    const PGN: &str = r#"[White "carol"][WhiteElo "2100"][Date "2020.01.01"]
1. e4 *

[White "Alice"][Date "2021.??.??"]
1. d4 d5 *

[White "bob"][WhiteElo "1900"]
1. c4 *

[White "?"][WhiteElo "2500"][Date "2019.06.01"]
1. Nf3 *
"#;

    fn sorted(db: &Mutex<Database>, key: SortKey, desc: bool) -> Vec<GameId> {
        let spec = SortSpec::new(key, desc);
        build_sort(db, spec, &NoProgress).unwrap();
        page_ids(&db.lock().unwrap(), spec, 0, 100).unwrap()
    }

    #[test]
    fn sorts_with_unknown_last() {
        let (_dir, db) = db_with(PGN);
        assert_eq!(sorted(&db, SortKey::White, false), [1, 2, 0, 3]);
        assert_eq!(sorted(&db, SortKey::White, true), [0, 2, 1, 3]);
        assert_eq!(sorted(&db, SortKey::WhiteElo, true), [3, 0, 2, 1]);
        assert_eq!(sorted(&db, SortKey::WhiteElo, false), [2, 0, 3, 1]);
        assert_eq!(sorted(&db, SortKey::Date, false), [3, 0, 1, 2]);
        assert_eq!(sorted(&db, SortKey::Plies, true), [1, 0, 2, 3]);
    }

    #[test]
    fn natural_order_and_paging() {
        let (_dir, db) = db_with(PGN);
        let db = db.lock().unwrap();
        assert_eq!(page_ids(&db, SortSpec::NATURAL, 1, 2).unwrap(), [1, 2]);
        assert_eq!(page_ids(&db, SortSpec::new(SortKey::Id, true), 0, 10).unwrap(), [3, 2, 1, 0]);
        assert_eq!(page_ids(&db, SortSpec::NATURAL, 10, 5).unwrap(), Vec::<GameId>::new());
        assert!(matches!(page_ids(&db, SortSpec::new(SortKey::Eco, false), 0, 1), Err(DbError::NotSorted)));
    }

    #[test]
    fn subsets_follow_the_sort() {
        let (_dir, db) = db_with(PGN);
        let spec = SortSpec::new(SortKey::White, false);
        assert!(matches!(sorted_subset(&db.lock().unwrap(), spec, &[0, 1]), Err(DbError::NotSorted)));
        build_sort(&db, spec, &NoProgress).unwrap();
        let db = db.lock().unwrap();
        assert_eq!(sorted_subset(&db, spec, &[0, 1, 3]).unwrap(), [1, 0, 3]);
        assert_eq!(sorted_subset(&db, SortSpec::new(SortKey::Id, true), &[0, 1, 3]).unwrap(), [3, 1, 0]);
    }

    #[test]
    fn appended_games_follow_sorted_ones_until_resort() {
        let (_dir, db) = db_with(PGN);
        let spec = SortSpec::new(SortKey::White, false);
        build_sort(&db, spec, &NoProgress).unwrap();
        let extra = read_pgn_str("[White \"Aaron\"]\n1. e4 *").remove(0);
        db.lock().unwrap().append(&extra).unwrap();
        assert_eq!(page_ids(&db.lock().unwrap(), spec, 0, 10).unwrap(), [1, 2, 0, 3, 4]);

        // Replacing a game invalidates cached sorts.
        let mut g = db.lock().unwrap().read_game(0).unwrap();
        g.headers.white = "Zed".into();
        db.lock().unwrap().replace(0, &g).unwrap();
        assert!(matches!(page_ids(&db.lock().unwrap(), spec, 0, 1), Err(DbError::NotSorted)));
        assert_eq!(sorted(&db, SortKey::White, false), [4, 1, 2, 0, 3]);
    }
}
