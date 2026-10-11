//! Native chess database storage: the on-disk format (see
//! `docs/file-format.md`), PGN import/export, copying games between
//! databases and sorted game lists.

pub mod blob;
mod bytes;
pub mod cb;
mod copy;
mod database;
mod export;
mod import;
mod io;
mod names;
pub mod positions;
pub mod record;
pub mod reference;
mod search;
mod sort;

use std::path::PathBuf;

pub use cb::import::import_cb;
pub use copy::{CopySummary, copy_games};
pub use database::{CompactSummary, Database, DbInfo, EXTENSION, GameRow, GameStat, NameKind, PreparedGame};
pub use export::export_pgn;
pub use import::{ImportSummary, ImportWarning, game_label, import_pgn, import_reader};
pub use names::NameTable;
pub use positions::{
    IndexPlan, IndexSummary, PositionHits, PositionIndex, PositionTree, TreeStats, find_position,
    position_tree, scan_position, update_position_index,
};
pub use reference::{
    BestPlayer, Hot, Line, LineStats, ReferenceLines, ReferenceMove, ReferenceOverview, TopGame, Trend,
    game_mainline, reference_lines, reference_overview,
};
pub use search::{EloMode, GameSet, HeaderQuery, ResultFilter, fold, games_with_name, search_headers};
pub use sort::{SortKey, SortSpec, build_sort, page_ids, sorted_subset};

/// Zero-based index of a game in its database.
pub type GameId = u32;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database is open in another process: {}", .0.display())]
    Locked(PathBuf),
    #[error("not a chess database: {}", .0.display())]
    NotADatabase(PathBuf),
    #[error("already exists: {}", .0.display())]
    AlreadyExists(PathBuf),
    #[error("unsupported format version {0}")]
    UnsupportedVersion(u32),
    #[error("database is corrupt: {0}")]
    Corrupt(String),
    #[error("game {0} not found")]
    NotFound(GameId),
    #[error("sort order not built yet")]
    NotSorted,
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Invalid(String),
}

/// Progress sink for long operations; implemented for job contexts.
pub trait Progress {
    fn report(&self, done: u64, total: u64);
    fn is_cancelled(&self) -> bool;

    fn check_cancelled(&self) -> Result<(), DbError> {
        if self.is_cancelled() { Err(DbError::Cancelled) } else { Ok(()) }
    }
}

/// Progress sink that ignores reports and never cancels.
pub struct NoProgress;

impl Progress for NoProgress {
    fn report(&self, _done: u64, _total: u64) {}
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl Progress for chess_jobs::JobContext {
    fn report(&self, done: u64, total: u64) {
        self.progress(done, total);
    }
    fn is_cancelled(&self) -> bool {
        chess_jobs::JobContext::is_cancelled(self)
    }
}
