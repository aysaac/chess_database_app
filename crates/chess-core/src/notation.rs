//! Move formatting: SAN, LAN and UCI, with figurine or localized piece
//! letters for display.

use shakmaty::san::SanPlus;
use shakmaty::uci::UciMove;
use shakmaty::{Chess, Move, Role};

/// How piece letters are rendered in SAN/LAN output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PieceStyle {
    #[default]
    English,
    Figurine,
    Localized(Language),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    German,
    Spanish,
    French,
}

impl Language {
    /// Letters for K, Q, R, B, N.
    fn letters(self) -> [char; 5] {
        match self {
            Language::German => ['K', 'D', 'T', 'L', 'S'],
            Language::Spanish => ['R', 'D', 'T', 'A', 'C'],
            Language::French => ['R', 'D', 'T', 'F', 'C'],
        }
    }
}

fn piece_index(c: char) -> Option<usize> {
    match c {
        'K' => Some(0),
        'Q' => Some(1),
        'R' => Some(2),
        'B' => Some(3),
        'N' => Some(4),
        _ => None,
    }
}

/// Rewrites the upper-case piece letters of an English SAN/LAN string.
/// Files are lower-case and castling uses `O`, so no other characters clash.
pub fn stylize(english: &str, style: PieceStyle) -> String {
    const FIGURINES: [char; 5] = ['♔', '♕', '♖', '♗', '♘'];
    match style {
        PieceStyle::English => english.to_owned(),
        PieceStyle::Figurine => english.chars().map(|c| piece_index(c).map_or(c, |i| FIGURINES[i])).collect(),
        PieceStyle::Localized(lang) => {
            let letters = lang.letters();
            english.chars().map(|c| piece_index(c).map_or(c, |i| letters[i])).collect()
        }
    }
}

/// SAN with check/mate suffix, e.g. `Nf3`, `exd8=Q+`, `O-O#`.
pub fn san(pos: &Chess, m: Move) -> String {
    SanPlus::from_move(pos.clone(), m).to_string()
}

pub fn san_styled(pos: &Chess, m: Move, style: PieceStyle) -> String {
    stylize(&san(pos, m), style)
}

/// Long algebraic notation, e.g. `Ng1-f3`, `e4xd5`, `e7-e8=Q`, `O-O`.
pub fn lan(m: Move) -> String {
    if let Some(side) = m.castling_side() {
        return if side.is_king_side() { "O-O".into() } else { "O-O-O".into() };
    }
    let mut s = String::new();
    if m.role() != Role::Pawn {
        s.push(m.role().upper_char());
    }
    if let Some(from) = m.from() {
        s.push_str(&from.to_string());
    }
    s.push(if m.is_capture() { 'x' } else { '-' });
    s.push_str(&m.to().to_string());
    if let Some(p) = m.promotion() {
        s.push('=');
        s.push(p.upper_char());
    }
    s
}

/// UCI move string in standard castling notation (`e1g1`).
pub fn uci(m: Move) -> String {
    UciMove::from_standard(m).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::{from_fen, parse_uci};

    #[test]
    fn formats() {
        let pos = Chess::default();
        let m = parse_uci(&pos, "g1f3").unwrap();
        assert_eq!(san(&pos, m), "Nf3");
        assert_eq!(lan(m), "Ng1-f3");
        assert_eq!(uci(m), "g1f3");
        assert_eq!(san_styled(&pos, m, PieceStyle::Figurine), "♘f3");
        assert_eq!(san_styled(&pos, m, PieceStyle::Localized(Language::German)), "Sf3");
    }

    #[test]
    fn castling_and_promotion() {
        let pos = from_fen("r3k2r/1P6/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
        let castle = parse_uci(&pos, "e1c1").unwrap();
        assert_eq!(san(&pos, castle), "O-O-O");
        assert_eq!(lan(castle), "O-O-O");
        assert_eq!(uci(castle), "e1c1");
        let promo = parse_uci(&pos, "b7a8q").unwrap();
        assert_eq!(san(&pos, promo), "bxa8=Q+");
        assert_eq!(lan(promo), "b7xa8=Q");
    }
}
