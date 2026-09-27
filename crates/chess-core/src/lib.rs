//! Chess domain model: rules glue, notation, game tree, editor and PGN I/O.
//!
//! This crate has no UI dependencies so it can be tested and reused
//! headless.

pub mod game;
pub mod notation;
pub mod openings;
pub mod pgn;
pub mod position;

pub use shakmaty;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid FEN: {0}")]
    InvalidFen(String),
}
