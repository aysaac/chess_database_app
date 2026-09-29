//! Copying games from one database to another (DB-06), as dragging games
//! onto a database does (QOL-04).
//!
//! Games are copied as stored: the blob bytes are reused and only the names
//! are looked up again in the target's name tables, so nothing is decoded
//! and a copy is exactly the original.

use std::sync::Mutex;

use crate::{Database, DbError, GameId, Progress};

/// Games per commit to the target, and per read lock on the source.
const CHUNK: usize = 2_000;

/// What [`copy_games`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopySummary {
    /// Source ids of the games copied, in the order asked for. A move marks
    /// these as deleted.
    pub copied: Vec<GameId>,
    /// Games skipped because they're marked as deleted.
    pub skipped_deleted: u64,
    /// Cancelled part way: the games copied so far stay in the target.
    pub cancelled: bool,
}

/// Appends games `ids` of `source` to `target`, in that order, in crash-safe
/// batches. Games marked as deleted are skipped; the copies are never marked
/// deleted. Each database is locked only while a batch is read from it or
/// written to it, and never both at once.
pub fn copy_games(
    source: &Mutex<Database>,
    ids: &[GameId],
    target: &Mutex<Database>,
    progress: &dyn Progress,
) -> Result<CopySummary, DbError> {
    if std::ptr::eq(source, target) {
        return Err(DbError::Invalid("the games are already in this database".into()));
    }
    let mut summary = CopySummary::default();
    let total = ids.len() as u64;
    let mut done = 0u64;
    for chunk in ids.chunks(CHUNK) {
        if progress.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        let mut batch = Vec::with_capacity(chunk.len());
        let mut batch_ids = Vec::with_capacity(chunk.len());
        {
            let db = source.lock().expect("db lock");
            for &id in chunk {
                let rec = db.record(id)?;
                if rec.is_deleted() {
                    summary.skipped_deleted += 1;
                } else {
                    batch.push(db.prepared_from(&rec)?);
                    batch_ids.push(id);
                }
            }
        }
        target.lock().expect("db lock").append_prepared(&batch)?;
        summary.copied.extend(batch_ids);
        done += chunk.len() as u64;
        progress.report(done, total);
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use chess_core::pgn::read_pgn_str;

    use super::*;
    use crate::NoProgress;

    fn fixture_db(dir: &std::path::Path, name: &str) -> Mutex<Database> {
        let mut db = Database::create(&dir.join(name)).unwrap();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn");
        for file in ["variations.pgn", "setup.pgn", "lichess.pgn"] {
            for g in read_pgn_str(&std::fs::read_to_string(root.join(file)).unwrap()) {
                db.append(&g).unwrap();
            }
        }
        Mutex::new(db)
    }

    #[test]
    fn copies_games_exactly_and_skips_deleted_ones() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture_db(dir.path(), "Source");
        let target = Mutex::new(Database::create(&dir.path().join("Target")).unwrap());
        target.lock().unwrap().append(&read_pgn_str("[White \"Zed\"]\n\n1. e4 *\n")[0]).unwrap();
        let n = source.lock().unwrap().count();
        assert!(n >= 5);
        source.lock().unwrap().set_deleted(1, true).unwrap();

        let ids = [4, 1, 0, 2];
        let summary = copy_games(&source, &ids, &target, &NoProgress).unwrap();
        assert_eq!(summary, CopySummary { copied: vec![4, 0, 2], skipped_deleted: 1, cancelled: false });

        let (src, dst) = (source.lock().unwrap(), target.lock().unwrap());
        assert_eq!(dst.count(), 4, "appended after the target's own game");
        for (copy, &id) in (1..).zip(&summary.copied) {
            assert_eq!(dst.read_game(copy).unwrap(), src.read_game(id).unwrap(), "game {id}");
            assert_eq!(dst.read_blob(copy).unwrap(), src.read_blob(id).unwrap(), "same bytes");
            let (a, b) = (dst.row(copy).unwrap(), src.row(id).unwrap());
            assert_eq!(
                (a.white, a.date, a.round, a.eco, a.plies),
                (b.white, b.date, b.round, b.eco, b.plies)
            );
            assert!(!a.deleted);
        }
        assert_eq!(src.count(), n, "the source is unchanged");
    }

    #[test]
    fn a_database_is_not_copied_into_itself() {
        let dir = tempfile::tempdir().unwrap();
        let db = fixture_db(dir.path(), "Same");
        assert!(matches!(copy_games(&db, &[0], &db, &NoProgress), Err(DbError::Invalid(_))));
        assert!(matches!(
            copy_games(
                &db,
                &[99],
                &Mutex::new(Database::create(&dir.path().join("B")).unwrap()),
                &NoProgress
            ),
            Err(DbError::NotFound(99))
        ));
    }

    /// Cancels after the first report.
    struct CancelAfterFirst(Cell<bool>);

    impl Progress for CancelAfterFirst {
        fn report(&self, _done: u64, _total: u64) {
            self.0.set(true);
        }
        fn is_cancelled(&self) -> bool {
            self.0.get()
        }
    }

    #[test]
    fn cancelling_keeps_the_batches_copied_so_far() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture_db(dir.path(), "Source");
        let target = Mutex::new(Database::create(&dir.path().join("Target")).unwrap());
        let ids: Vec<GameId> = (0..CHUNK as u32 + 5).map(|i| i % 3).collect();
        let summary = copy_games(&source, &ids, &target, &CancelAfterFirst(Cell::new(false))).unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.copied.len(), CHUNK);
        assert_eq!(target.lock().unwrap().count(), CHUNK as u32);
    }
}
