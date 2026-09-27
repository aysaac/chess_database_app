//! Streaming PGN import. Parsing and blob encoding happen outside the
//! database lock; batches are committed every few hundred milliseconds so the
//! game list can grow live during a long import.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chess_core::pgn::{PgnReader, PgnWarning};

use crate::{Database, DbError, PreparedGame, Progress};

const MAX_BATCH: usize = 10_000;
const MAX_BATCH_AGE: Duration = Duration::from_millis(300);
/// Warnings kept in the summary (the total is still counted).
const MAX_WARNINGS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: u64,
    pub skipped: u64,
    pub warning_count: u64,
    pub warnings: Vec<PgnWarning>,
    pub cancelled: bool,
}

struct CountingReader<R> {
    inner: R,
    count: Rc<Cell<u64>>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.set(self.count.get() + n as u64);
        Ok(n)
    }
}

/// Imports a PGN file. On cancellation the games parsed so far are kept and
/// `cancelled` is set in the summary.
pub fn import_pgn(
    db: &Mutex<Database>,
    path: &Path,
    progress: &dyn Progress,
) -> Result<ImportSummary, DbError> {
    let file = File::open(path)?;
    let total = file.metadata()?.len();
    let bytes = Rc::new(Cell::new(0));
    import_reader(db, CountingReader { inner: file, count: bytes.clone() }, total, &|| bytes.get(), progress)
}

/// Imports PGN from any reader. `position` reports bytes consumed so far
/// (for progress against `total`).
pub fn import_reader<R: Read>(
    db: &Mutex<Database>,
    input: R,
    total: u64,
    position: &dyn Fn() -> u64,
    progress: &dyn Progress,
) -> Result<ImportSummary, DbError> {
    let mut summary = ImportSummary::default();
    let mut batch: Vec<PreparedGame> = Vec::with_capacity(1024);
    let mut batch_started = Instant::now();

    let commit = |batch: &mut Vec<PreparedGame>, summary: &mut ImportSummary| -> Result<(), DbError> {
        if !batch.is_empty() {
            db.lock().expect("db lock").append_prepared(batch)?;
            summary.imported += batch.len() as u64;
            batch.clear();
        }
        Ok(())
    };

    for parsed in PgnReader::new(input) {
        let parsed = parsed?;
        summary.warning_count += parsed.warnings.len() as u64;
        for w in parsed.warnings {
            if summary.warnings.len() < MAX_WARNINGS {
                summary.warnings.push(w);
            }
        }
        match parsed.game {
            Some(game) => batch.push(PreparedGame::new(&game)),
            None => summary.skipped += 1,
        }

        if batch.len() >= MAX_BATCH || batch_started.elapsed() >= MAX_BATCH_AGE {
            commit(&mut batch, &mut summary)?;
            batch_started = Instant::now();
            progress.report(position(), total);
            if progress.is_cancelled() {
                summary.cancelled = true;
                return Ok(summary);
            }
        }
    }
    commit(&mut batch, &mut summary)?;
    progress.report(total, total);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoProgress;

    #[test]
    fn imports_fixture_files() {
        let dir = tempfile::tempdir().unwrap();
        let db = Mutex::new(Database::create(&dir.path().join("Import")).unwrap());
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn");
        let summary = import_pgn(&db, &fixtures.join("malformed.pgn"), &NoProgress).unwrap();
        assert_eq!(summary.imported, 4);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.warning_count, 2);
        let summary = import_pgn(&db, &fixtures.join("variations.pgn"), &NoProgress).unwrap();
        assert_eq!(summary.imported, 2);
        let db = db.lock().unwrap();
        assert_eq!(db.count(), 6);
        assert_eq!(db.row(4).unwrap().white, "Kasparov, Garry");
    }
}
