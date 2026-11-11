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

use chess_core::game::Headers;
use chess_core::pgn::PgnReader;

use crate::{Database, DbError, GameId, PreparedGame, Progress};

const MAX_BATCH: usize = 10_000;
const MAX_BATCH_AGE: Duration = Duration::from_millis(300);
/// Warnings kept in the summary (the total is still counted).
const MAX_WARNINGS: usize = 200;

/// A problem found in one game of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWarning {
    /// Zero-based index of the game in the file.
    pub game_index: usize,
    /// The id the game got in the database; `None` if it was skipped.
    pub game: Option<GameId>,
    /// `White – Black`, or the event when both players are unknown; empty
    /// for a skipped game.
    pub label: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: u64,
    pub skipped: u64,
    pub warning_count: u64,
    /// The first warnings (up to 200).
    pub warnings: Vec<ImportWarning>,
    pub cancelled: bool,
}

/// How a game is named in warnings: `White – Black`, or the event when both
/// players are unknown.
pub fn game_label(h: &Headers) -> String {
    if h.white.is_empty() && h.black.is_empty() {
        return h.event.clone();
    }
    let name = |n: &str| if n.is_empty() { "?".to_owned() } else { n.to_owned() };
    format!("{} – {}", name(&h.white), name(&h.black))
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

/// Where an import starts, and what it says before each commit: for an
/// import interrupted (a server stopped mid-way, NET-03) to go on from what
/// it committed, with no game twice and none missing.
#[derive(Default, Clone, Copy)]
pub struct ImportResume<'a> {
    /// Games of the file an earlier run went through (PGN games read, ok
    /// or skipped; ChessBase game numbers): passed over.
    pub skip: u64,
    /// Called just before each batch is committed, with what the database
    /// and the file will be at once it is. The caller notes it, so that
    /// after a crash it can tell from the database's game count whether
    /// that batch made it, and where to go on from.
    pub before_commit: Option<&'a dyn Fn(Checkpoint)>,
}

/// What [`ImportResume::before_commit`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
    /// The database's game count once the batch is committed.
    pub games: GameId,
    /// Games of the file gone through then (as [`ImportResume::skip`] counts them).
    pub read: u64,
}

/// Imports a PGN file. On cancellation the games parsed so far are kept and
/// `cancelled` is set in the summary.
pub fn import_pgn(
    db: &Mutex<Database>,
    path: &Path,
    progress: &dyn Progress,
) -> Result<ImportSummary, DbError> {
    import_pgn_resumable(db, path, ImportResume::default(), progress)
}

/// [`import_pgn`] going on from where an earlier run left off (see
/// [`ImportResume`]); the summary counts this run's games only.
pub fn import_pgn_resumable(
    db: &Mutex<Database>,
    path: &Path,
    resume: ImportResume<'_>,
    progress: &dyn Progress,
) -> Result<ImportSummary, DbError> {
    let file = File::open(path)?;
    let total = file.metadata()?.len();
    let bytes = Rc::new(Cell::new(0));
    import_with(
        db,
        CountingReader { inner: file, count: bytes.clone() },
        total,
        &|| bytes.get(),
        resume,
        progress,
    )
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
    import_with(db, input, total, position, ImportResume::default(), progress)
}

fn import_with<R: Read>(
    db: &Mutex<Database>,
    input: R,
    total: u64,
    position: &dyn Fn() -> u64,
    resume: ImportResume<'_>,
    progress: &dyn Progress,
) -> Result<ImportSummary, DbError> {
    let mut summary = ImportSummary::default();
    let mut batch: Vec<PreparedGame> = Vec::with_capacity(1024);
    let mut batch_started = Instant::now();
    // Warnings of games in `batch`: (position in the batch, index in `summary.warnings`).
    let mut unplaced: Vec<(usize, usize)> = Vec::new();
    // Games of the file gone through.
    let mut read = 0u64;

    let commit = |batch: &mut Vec<PreparedGame>,
                  unplaced: &mut Vec<(usize, usize)>,
                  summary: &mut ImportSummary,
                  read: u64|
     -> Result<(), DbError> {
        if let Some(note) = resume.before_commit {
            // Nothing else appends meanwhile: an import has the database to itself.
            let games = db.lock().expect("db lock").count() + batch.len() as GameId;
            note(Checkpoint { games, read });
        }
        if !batch.is_empty() {
            let ids = db.lock().expect("db lock").append_prepared(batch)?;
            for (pos, i) in unplaced.drain(..) {
                summary.warnings[i].game = Some(ids.start + pos as GameId);
            }
            summary.imported += batch.len() as u64;
            batch.clear();
        }
        Ok(())
    };

    for parsed in PgnReader::new(input) {
        let parsed = parsed?;
        read += 1;
        if read <= resume.skip {
            if read.is_multiple_of(4096) {
                progress.report(position(), total);
                progress.check_cancelled()?;
            }
            continue;
        }
        summary.warning_count += parsed.warnings.len() as u64;
        let first = summary.warnings.len();
        let who = match (&parsed.game, parsed.warnings.is_empty()) {
            (Some(g), false) => game_label(&g.headers),
            _ => String::new(),
        };
        for w in parsed.warnings {
            if summary.warnings.len() < MAX_WARNINGS {
                summary.warnings.push(ImportWarning {
                    game_index: w.game_index,
                    game: None,
                    label: who.clone(),
                    message: w.message,
                });
            }
        }
        match parsed.game {
            Some(game) => {
                unplaced.extend((first..summary.warnings.len()).map(|i| (batch.len(), i)));
                batch.push(PreparedGame::new(&game));
            }
            None => summary.skipped += 1,
        }

        if batch.len() >= MAX_BATCH || batch_started.elapsed() >= MAX_BATCH_AGE {
            commit(&mut batch, &mut unplaced, &mut summary, read)?;
            batch_started = Instant::now();
            progress.report(position(), total);
            if progress.is_cancelled() {
                summary.cancelled = true;
                return Ok(summary);
            }
        }
    }
    commit(&mut batch, &mut unplaced, &mut summary, read)?;
    progress.report(total, total);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoProgress;

    /// Stops after its first report (a crash right after a commit).
    struct StopAfterFirst(std::cell::Cell<bool>);

    impl Progress for StopAfterFirst {
        fn report(&self, _: u64, _: u64) {
            self.0.set(true);
        }
        fn is_cancelled(&self) -> bool {
            self.0.get()
        }
    }

    #[test]
    fn an_interrupted_import_goes_on_from_its_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let mut pgn = String::new();
        for i in 0..23_456 {
            if i % 997 == 5 {
                // Skipped: the FEN is bad.
                pgn.push_str("[Event \"bad\"]\n[SetUp \"1\"]\n[FEN \"nonsense\"]\n\n1. e4 *\n\n");
            } else {
                pgn.push_str(&format!("[Event \"E{i}\"]\n[White \"W{i}\"]\n\n1. e4 e5 2. Nf3 *\n\n"));
            }
        }
        let path = dir.path().join("many.pgn");
        std::fs::write(&path, &pgn).unwrap();
        let clean = Mutex::new(Database::create(&dir.path().join("Clean")).unwrap());
        let full = import_pgn(&clean, &path, &NoProgress).unwrap();
        assert_eq!(full.skipped, 24);

        let db = Mutex::new(Database::create(&dir.path().join("Resumed")).unwrap());
        let seen = std::cell::RefCell::new(Vec::new());
        let note = |c: Checkpoint| seen.borrow_mut().push(c);
        let resume = ImportResume { skip: 0, before_commit: Some(&note) };
        let first = import_pgn_resumable(&db, &path, resume, &StopAfterFirst(Default::default())).unwrap();
        assert!(first.cancelled);
        let at = *seen.borrow().last().unwrap();
        assert_eq!(at.games, db.lock().unwrap().count(), "the checkpoint is what was committed");
        assert!(at.games < 23_000, "{at:?}");
        let rest = import_pgn_resumable(
            &db,
            &path,
            ImportResume { skip: at.read, before_commit: None },
            &NoProgress,
        )
        .unwrap();
        assert_eq!(first.imported + rest.imported, full.imported);
        let (a, b) = (clean.lock().unwrap(), db.lock().unwrap());
        assert_eq!(a.count(), b.count());
        for id in 0..a.count() {
            assert_eq!(a.read_blob(id).unwrap(), b.read_blob(id).unwrap(), "game {id}");
            assert_eq!(a.row(id).unwrap().event, b.row(id).unwrap().event);
        }
    }

    #[test]
    fn imports_fixture_files() {
        let dir = tempfile::tempdir().unwrap();
        let db = Mutex::new(Database::create(&dir.path().join("Import")).unwrap());
        let fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn");
        let summary = import_pgn(&db, &fixtures.join("malformed.pgn"), &NoProgress).unwrap();
        assert_eq!(summary.imported, 4);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.warning_count, 2);
        // The illegal move is in game 0 (kept as id 0), the bad FEN in game 3 (skipped).
        let w = &summary.warnings;
        assert_eq!(
            (w[0].game_index, w[0].game, w[0].label.as_str()),
            (0, Some(0), "Illegal move in main line")
        );
        assert_eq!((w[1].game_index, w[1].game, w[1].label.as_str()), (3, None, ""));
        assert_eq!(db.lock().unwrap().row(0).unwrap().event, "Illegal move in main line");
        let summary = import_pgn(&db, &fixtures.join("variations.pgn"), &NoProgress).unwrap();
        assert_eq!(summary.imported, 2);
        let db = db.lock().unwrap();
        assert_eq!(db.count(), 6);
        assert_eq!(db.row(4).unwrap().white, "Kasparov, Garry");
    }
}
