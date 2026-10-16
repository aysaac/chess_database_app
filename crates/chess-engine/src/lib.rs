//! UCI engine host (ENG-01): runs engines as child processes, parses the
//! protocol, drives interactive analysis (ENG-02) and batch annotation such
//! as the blunder check (ANL-01), and works out threats (ENG-07).
//!
//! No UI dependencies: engines are plain processes, and results are plain
//! data for the caller to present.

mod analysis;
mod engine;
pub mod review;
pub mod threat;
pub mod uci;

use std::path::PathBuf;

pub use analysis::{AnalysisRequest, AnalysisUpdate, Analyzer, AnalyzerEvent};
pub use engine::{Engine, EngineId, Limits, SearchEvent, SearchPosition, SearchResult};
pub use uci::{Bound, Info, Score, UciOption};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("couldn't start the engine {}: {source}", path.display())]
    Spawn { path: PathBuf, source: std::io::Error },
    #[error("the engine exited ({0})")]
    Exited(String),
    #[error("the engine didn't answer {0} in time")]
    Timeout(String),
    #[error("the engine has no option {0:?}")]
    UnknownOption(String),
    #[error("{value:?} isn't a valid value for the engine option {name:?}")]
    BadOptionValue { name: String, value: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
}
