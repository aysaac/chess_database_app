//! PGN import and export.

pub mod comment;
mod reader;
mod writer;

pub use reader::{ParsedGame, PgnReader, PgnWarning, read_pgn_str};
pub use writer::{write_game, write_movetext};

/// Decodes PGN text bytes: UTF-8, falling back to Windows-1252 (common in
/// older PGN files; Latin-1 is a subset).
pub fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_latin1_fallback() {
        assert_eq!(decode_text("Réti".as_bytes()), "Réti");
        assert_eq!(decode_text(b"R\xe9ti"), "Réti");
    }
}
