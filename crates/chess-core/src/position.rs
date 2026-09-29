//! Position helpers on top of `shakmaty`: FEN, hashing, draw rules and
//! move lookup for board input.

use shakmaty::fen::Fen;
use shakmaty::zobrist::Zobrist64;
use shakmaty::{CastlingMode, Chess, Color, EnPassantMode, File, Move, Position, Role, Square};

use crate::Error;

/// Parses a FEN into a standard-chess position. Errors say in plain words
/// what is wrong (for the position editor and FEN paste).
pub fn from_fen(fen: &str) -> Result<Chess, Error> {
    use shakmaty::PositionErrorKinds as K;
    use shakmaty::fen::ParseFenError as P;
    let parsed = Fen::from_ascii(fen.trim().as_bytes()).map_err(|e| {
        Error::InvalidFen(
            match e {
                P::InvalidBoard => "the piece placement (the first part) isn't valid",
                P::InvalidTurn => "the side to move must be w or b",
                P::InvalidCastling => "the castling part must be - or some of KQkq",
                P::InvalidEpSquare => "the en passant part must be - or a square such as e3",
                P::InvalidHalfmoveClock | P::InvalidFullmoves => "the move counters must be numbers",
                _ => "it isn't a FEN",
            }
            .to_owned(),
        )
    })?;
    parsed.into_position(CastlingMode::Standard).map_err(|e| {
        let kinds = e.kinds();
        let reasons: Vec<&str> = [
            (K::EMPTY_BOARD, "the board is empty"),
            (K::MISSING_KING, "each side needs a king"),
            (K::TOO_MANY_KINGS, "each side can have only one king"),
            (K::PAWNS_ON_BACKRANK, "pawns can't stand on the first or last rank"),
            (K::INVALID_CASTLING_RIGHTS, "castling needs the king and that rook on their starting squares"),
            (K::INVALID_EP_SQUARE, "no pawn can have just moved two squares for that en passant square"),
            (K::OPPOSITE_CHECK, "the side that just moved is in check"),
            (K::IMPOSSIBLE_CHECK, "no legal move leads to this check"),
            (K::TOO_MUCH_MATERIAL, "a side has more pieces than a game allows"),
        ]
        .into_iter()
        .filter(|(k, _)| kinds.contains(*k))
        .map(|(_, r)| r)
        .collect();
        Error::InvalidFen(if reasons.is_empty() { "illegal position".to_owned() } else { reasons.join("; ") })
    })
}

/// Finds a FEN in pasted text (QOL-10 "go to position"): a bare FEN, one in
/// a PGN `[FEN "…"]` tag or an EPD line, or a link that ends in one with `_`
/// or `%20` for spaces, as Lichess analysis links do. Returns the first
/// piece placement (eight ranks of eight squares) with the fields after it,
/// plainly spaced for [`from_fen`], or `None` when the text has none.
pub fn find_fen(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let is_board = |b: u8| b == b'/' || (b'1'..=b'8').contains(&b) || b"pnbrqkPNBRQK".contains(&b);
    let is_rank = |r: &[u8]| {
        !r.is_empty()
            && r.iter().map(|&b| if b.is_ascii_digit() { u32::from(b - b'0') } else { 1 }).sum::<u32>() == 8
    };
    let mut i = 0;
    while i < bytes.len() {
        if !is_board(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_board(bytes[i]) {
            i += 1;
        }
        // The run's parts between slashes, as (start, end) offsets; a link
        // path can put a word like "n/" before the placement.
        let mut parts = Vec::new();
        let mut from = start;
        for part in text[start..i].split('/') {
            parts.push((from, from + part.len()));
            from += part.len() + 1;
        }
        if let Some(ranks) = parts.windows(8).find(|w| w.iter().all(|&(a, b)| is_rank(&bytes[a..b]))) {
            let (board_start, board_end) = (ranks[0].0, ranks[7].1);
            return Some(with_fields(&text[board_start..board_end], &text[board_end..]));
        }
    }
    None
}

/// `board` followed by the FEN fields at the start of `rest`, which ends at
/// the line's end or the quote, `?` or `;` closing a tag, link or EPD
/// operation.
fn with_fields(board: &str, rest: &str) -> String {
    let rest = rest.replace("%20", " ");
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, ' ' | '\t' | '_' | '-')))
        .unwrap_or(rest.len());
    let mut fields = rest[..end].split([' ', '\t', '_']).filter(|f| !f.is_empty());
    let mut fen = board.to_owned();
    // Side to move, castling and en passant as written (`from_fen` explains
    // a bad one), then the move counters, but only numbers: EPD operations
    // can follow instead.
    let mut push = |field: &str| {
        fen.push(' ');
        fen.push_str(field);
    };
    fields.by_ref().take(3).for_each(&mut push);
    fields.take(2).take_while(|f| f.bytes().all(|b| b.is_ascii_digit())).for_each(push);
    fen
}

pub fn to_fen(pos: &Chess) -> String {
    Fen::from_position(pos, EnPassantMode::Legal).to_string()
}

/// Polyglot-compatible Zobrist hash (en passant only counted when legal).
pub fn hash(pos: &Chess) -> u64 {
    pos.zobrist_hash::<Zobrist64>(EnPassantMode::Legal).0
}

/// Game state of a position, including draws that need the move history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    Ongoing,
    Checkmate {
        winner: Color,
    },
    Stalemate,
    InsufficientMaterial,
    /// 50-move rule reached (claimable).
    FiftyMoves,
    /// Position occurred three times (claimable).
    Threefold,
}

/// Status of `pos`. `history` holds the hashes of all earlier positions on
/// the path from the game start (not including `pos` itself).
pub fn status(pos: &Chess, history: &[u64]) -> GameStatus {
    if pos.is_checkmate() {
        return GameStatus::Checkmate { winner: !pos.turn() };
    }
    if pos.is_stalemate() {
        return GameStatus::Stalemate;
    }
    if pos.is_insufficient_material() {
        return GameStatus::InsufficientMaterial;
    }
    if pos.halfmoves() >= 100 {
        return GameStatus::FiftyMoves;
    }
    // Repetitions can only occur since the last irreversible move.
    let window = (pos.halfmoves() as usize).min(history.len());
    let current = hash(pos);
    let seen = history[history.len() - window..].iter().filter(|&&h| h == current).count();
    if seen >= 2 {
        return GameStatus::Threefold;
    }
    GameStatus::Ongoing
}

/// Square the king lands on when castling.
pub fn castle_king_target(m: Move) -> Option<Square> {
    match m {
        Move::Castle { king, rook } => {
            let file = if rook.file() > king.file() { File::G } else { File::C };
            Some(Square::from_coords(file, king.rank()))
        }
        _ => None,
    }
}

/// Legal destinations per origin square, as a board UI expects them.
/// Castling is offered both as king-to-g/c-file and king-onto-rook.
pub fn legal_destinations(pos: &Chess) -> Vec<(Square, Vec<Square>)> {
    let mut out: Vec<(Square, Vec<Square>)> = Vec::new();
    for m in pos.legal_moves() {
        let Some(from) = m.from() else { continue };
        let mut targets = vec![m.to()];
        if let Some(k) = castle_king_target(m) {
            targets.push(k);
        }
        let entry = match out.iter_mut().position(|(sq, _)| *sq == from) {
            Some(i) => &mut out[i].1,
            None => {
                out.push((from, Vec::new()));
                &mut out.last_mut().expect("just pushed").1
            }
        };
        for t in targets {
            if !entry.contains(&t) {
                entry.push(t);
            }
        }
    }
    out
}

/// Finds the legal move matching a board gesture. Castling matches either
/// the king's target square or the rook's square.
pub fn find_move(pos: &Chess, from: Square, to: Square, promotion: Option<Role>) -> Option<Move> {
    pos.legal_moves().into_iter().find(|&m| {
        if m.from() != Some(from) {
            return false;
        }
        match m {
            Move::Castle { .. } => m.to() == to || castle_king_target(m) == Some(to),
            _ => m.to() == to && m.promotion() == promotion,
        }
    })
}

/// True if `pos` has a legal promotion from `from` to `to` (UI needs to ask
/// for the piece).
pub fn is_promotion(pos: &Chess, from: Square, to: Square) -> bool {
    pos.legal_moves().iter().any(|m| m.from() == Some(from) && m.to() == to && m.is_promotion())
}

/// Parses a UCI-style move string (`e2e4`, `e7e8q`, `e1g1`, `e1h1`).
pub fn parse_uci(pos: &Chess, uci: &str) -> Option<Move> {
    let b = uci.as_bytes();
    if b.len() < 4 {
        return None;
    }
    let from = Square::from_ascii(&b[0..2]).ok()?;
    let to = Square::from_ascii(&b[2..4]).ok()?;
    let promotion = match b.get(4) {
        Some(&c) => Some(Role::from_char(c as char)?),
        None => None,
    };
    find_move(pos, from, to, promotion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shakmaty::san::San;

    fn play(pos: &mut Chess, history: &mut Vec<u64>, sans: &[&str]) {
        for s in sans {
            let m = s.parse::<San>().unwrap().to_move(pos).unwrap();
            history.push(hash(pos));
            pos.play_unchecked(m);
        }
    }

    #[test]
    fn start_hash_matches_polyglot() {
        assert_eq!(hash(&Chess::default()), 0x463b96181691fc9c);
    }

    #[test]
    fn fen_round_trip() {
        let fen = "r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3";
        assert_eq!(to_fen(&from_fen(fen).unwrap()), fen);
        assert!(from_fen("not a fen").is_err());
    }

    #[test]
    fn fen_errors_are_plain() {
        let err = |f: &str| from_fen(f).unwrap_err().to_string();
        assert!(err("8/8/8/8/8/8/8/8 w - - 0 1").contains("the board is empty"));
        assert!(err("8/8/8/8/8/8/8/4K3 w - - 0 1").contains("each side needs a king"));
        assert!(err("4k3/8/8/8/8/8/8/4K2R w KQ - 0 1").contains("castling needs"));
        assert!(err("4k3/4R3/8/8/8/8/8/4K3 w - - 0 1").contains("the side that just moved is in check"));
        assert!(err("4k3/8/8/8/8/8/8/4K3 x - - 0 1").contains("side to move"));
        assert!(err("P3k3/8/8/8/8/8/8/4K3 w - - 0 1").contains("pawns can't"));
    }

    #[test]
    fn finds_a_fen_in_pasted_text() {
        let e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
        let found = |t: &str| find_fen(t);
        assert_eq!(found(e4).as_deref(), Some(e4));
        assert_eq!(found(&format!("  {e4}\r\n")).as_deref(), Some(e4));
        assert_eq!(found(&format!("FEN: {e4}")).as_deref(), Some(e4));
        // Lichess links: underscores or %20 for spaces, a query after them.
        let url = "https://lichess.org/analysis/standard/rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR_b_KQkq_-_0_1?color=white";
        assert_eq!(found(url).as_deref(), Some(e4));
        let url = "https://lichess.org/analysis/fromPosition/rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR%20b%20KQkq%20-%200%201";
        assert_eq!(found(url).as_deref(), Some(e4));
        // A PGN's FEN tag, not the moves after it.
        let pgn = format!("[Event \"Study\"]\n[SetUp \"1\"]\n[FEN \"{e4}\"]\n\n1... e5 2. Nf3 *\n");
        assert_eq!(found(&pgn).as_deref(), Some(e4));
        // EPD: four fields, then operations.
        let epd = "rnbqkb1r/pppp1ppp/5n2/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - bm Nxe5; id \"x\";";
        assert_eq!(
            found(epd).as_deref(),
            Some("rnbqkb1r/pppp1ppp/5n2/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq -")
        );
        // Only the placement, or part of the fields: from_fen fills in the rest.
        assert_eq!(found("8/8/4k3/8/8/4K3/8/8").as_deref(), Some("8/8/4k3/8/8/4K3/8/8"));
        assert_eq!(found("8/8/4k3/8/8/4K3/8/8 b").as_deref(), Some("8/8/4k3/8/8/4K3/8/8 b"));
        assert!(from_fen(&found("8/8/4k3/8/8/4K3/8/8 b").unwrap()).is_ok());
        // A bad field is kept, so from_fen can say what's wrong with it.
        assert!(
            from_fen(&found("4k3/8/8/8/8/8/8/4K3 x - - 0 1").unwrap())
                .unwrap_err()
                .to_string()
                .contains("side to move")
        );
        // No placement of eight full ranks: no FEN.
        assert_eq!(found("1. e4 e5 2. Nf3 Nc6 1/2-1/2"), None);
        assert_eq!(found("8/8/8/8/8/8/4K3 w - - 0 1"), None);
        assert_eq!(found("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBN w"), None);
        assert_eq!(found("hello"), None);
        assert_eq!(found(""), None);
    }

    #[test]
    fn detects_checkmate_and_stalemate() {
        let mut pos = Chess::default();
        let mut h = Vec::new();
        play(&mut pos, &mut h, &["f3", "e5", "g4", "Qh4#"]);
        assert_eq!(status(&pos, &h), GameStatus::Checkmate { winner: Color::Black });

        let stale = from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").unwrap();
        assert_eq!(status(&stale, &[]), GameStatus::Stalemate);
    }

    #[test]
    fn detects_threefold_and_insufficient_material() {
        let mut pos = Chess::default();
        let mut h = Vec::new();
        play(&mut pos, &mut h, &["Nf3", "Nf6", "Ng1", "Ng8", "Nf3", "Nf6", "Ng1"]);
        assert_eq!(status(&pos, &h), GameStatus::Ongoing);
        play(&mut pos, &mut h, &["Ng8"]);
        assert_eq!(status(&pos, &h), GameStatus::Threefold);

        let kk = from_fen("8/8/4k3/8/8/3K4/8/8 w - - 0 1").unwrap();
        assert_eq!(status(&kk, &[]), GameStatus::InsufficientMaterial);
    }

    #[test]
    fn fifty_move_rule() {
        let pos = from_fen("8/8/4k3/8/8/3K4/7R/8 w - - 100 80").unwrap();
        assert_eq!(status(&pos, &[]), GameStatus::FiftyMoves);
    }

    #[test]
    fn castling_input_accepts_both_gestures() {
        let pos = from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
        let a = parse_uci(&pos, "e1g1").unwrap();
        let b = parse_uci(&pos, "e1h1").unwrap();
        assert_eq!(a, b);
        assert!(matches!(a, Move::Castle { .. }));
        let dests = legal_destinations(&pos);
        let king = dests.iter().find(|(sq, _)| *sq == Square::E1).unwrap();
        assert!(king.1.contains(&Square::G1) && king.1.contains(&Square::H1));
    }

    #[test]
    fn promotion_needs_piece() {
        let pos = from_fen("8/4P3/8/8/8/8/k7/4K3 w - - 0 1").unwrap();
        assert!(is_promotion(&pos, Square::E7, Square::E8));
        assert!(parse_uci(&pos, "e7e8").is_none());
        assert!(parse_uci(&pos, "e7e8n").is_some());
    }
}
