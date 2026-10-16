//! Threat display (ENG-07): what the opponent would play if the side to
//! move could pass, as ChessBase and Fritz show it.
//!
//! The position after a "null move" (the other side to move, no en passant
//! square, the clocks moved on as for a quiet move) is searched like any
//! other position, and its best line is the threat. The app runs that
//! search on a second, small engine process next to the main analysis
//! (`app/src-tauri/src/engines.rs` says why); this module holds the parts
//! that need no engine: the null move, the search limits and options, and
//! turning the engine's line into moves to show.

use chess_core::notation::san;
use chess_core::position::{parse_uci, to_fen};
use chess_core::shakmaty::{Chess, Color, EnPassantMode, FromSetup, Position};

use crate::engine::{Limits, SearchPosition};
use crate::uci::{Bound, Info, Score, UciOption};

/// Why a position has no threat to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoThreat {
    /// Mate, stalemate or too little material: the game is over, and the
    /// main analysis doesn't search it either.
    GameOver,
    /// The side to move is in check, so it can't pass: the position after
    /// the pass would be illegal (the opponent could take the king).
    InCheck,
    /// After a pass the opponent would have no legal move: nothing to
    /// threaten.
    NoMoves,
}

/// The position after the side to move passes: the turn goes to the other
/// side, the en passant square goes, the halfmove clock counts the pass,
/// and the move number goes up when Black passed, so the threat's moves are
/// numbered as they would be played.
pub fn null_move(pos: &Chess) -> Result<Chess, NoThreat> {
    if pos.is_game_over() {
        return Err(NoThreat::GameOver);
    }
    if pos.is_check() {
        return Err(NoThreat::InCheck);
    }
    let mode = pos.castles().mode();
    let mut setup = pos.to_setup(EnPassantMode::Legal);
    let passed = setup.turn;
    setup.swap_turn();
    setup.halfmoves = setup.halfmoves.saturating_add(1);
    if passed == Color::Black {
        setup.fullmoves = setup.fullmoves.saturating_add(1);
    }
    // Only a check on the side that passed makes this fail, and that was
    // ruled out above; the rest of the position was legal already.
    let after = Chess::from_setup(setup, mode).map_err(|_| NoThreat::InCheck)?;
    if after.legal_moves().is_empty() {
        return Err(NoThreat::NoMoves);
    }
    Ok(after)
}

/// The position after the pass, as the engine is given it.
pub fn search_position(after_pass: &Chess) -> SearchPosition {
    SearchPosition { fen: Some(to_fen(after_pass)), moves: Vec::new() }
}

/// How long one threat search runs. The threat is a hint next to the main
/// lines, so it stops at a useful depth or after a few seconds, whichever
/// comes first, and the engine then idles until the board moves.
pub const THREAT_LIMITS: Limits =
    Limits { depth: Some(20), nodes: None, movetime_ms: Some(4000), infinite: false };

/// Options the threat engine sets itself, whatever the user chose for the
/// main analysis.
const OWN_OPTIONS: [&str; 3] = ["Threads", "Hash", "MultiPV"];

/// Whether a change to the main engine's option `name` also goes to the
/// threat engine (not its threads, hash or number of lines).
pub fn shares_option(name: &str) -> bool {
    !OWN_OPTIONS.iter().any(|o| o.eq_ignore_ascii_case(name))
}

/// Options for the threat engine: the user's options for the engine
/// (`usable`, already checked against what it declares), except that it
/// runs on one thread with a 16 MB hash, so it takes little from the main
/// analysis, and searches one line.
pub fn engine_options(declared: &[UciOption], usable: &[(String, String)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> =
        usable.iter().filter(|(name, _)| shares_option(name)).cloned().collect();
    for (name, value) in [("Threads", "1"), ("Hash", "16")] {
        if let Some(decl) = declared.iter().find(|o| o.name().eq_ignore_ascii_case(name))
            && let Some(value) = decl.validate(value)
        {
            out.push((decl.name().to_owned(), value));
        }
    }
    out
}

/// A move of a line, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMove {
    pub uci: String,
    /// English SAN; the frontend styles the piece letters.
    pub san: String,
    pub move_number: u32,
    pub white: bool,
}

/// The opponent's best line after the pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Threat {
    /// From White's point of view, like the main lines.
    pub score: Score,
    /// From White's point of view: `Lower` is at least this good for White.
    pub bound: Bound,
    pub depth: u32,
    /// The threatening move first. Never empty.
    pub moves: Vec<LineMove>,
}

/// Turns the engine's line for the position after the pass into the
/// threat, keeping at most `max_plies` moves and stopping at the first move
/// that isn't legal. `None` while the line has no score or no move yet.
pub fn threat_line(after_pass: &Chess, info: &Info, max_plies: usize) -> Option<Threat> {
    let white = after_pass.turn() == Color::White;
    let score = info.score?.for_white(white);
    let mut pos = after_pass.clone();
    let mut moves = Vec::new();
    for text in info.pv.iter().take(max_plies) {
        let Some(mv) = parse_uci(&pos, text) else { break };
        moves.push(LineMove {
            uci: text.clone(),
            san: san(&pos, mv),
            move_number: pos.fullmoves().get(),
            white: pos.turn() == Color::White,
        });
        pos.play_unchecked(mv);
    }
    if moves.is_empty() {
        return None;
    }
    let bound = match (info.bound, white) {
        (Bound::Exact, _) => Bound::Exact,
        (Bound::Lower, true) | (Bound::Upper, false) => Bound::Lower,
        (Bound::Upper, true) | (Bound::Lower, false) => Bound::Upper,
    };
    Some(Threat { score, bound, depth: info.depth.unwrap_or(0), moves })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::position::from_fen;

    /// 1.e4 e5 2.Bc4 Nc6 3.Qh5, Black to move: White threatens 4.Qxf7#.
    const MATE_THREAT: &str = "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 3 3";
    /// 1.e4 d5 2.e5 f5, White to move: exf6 en passant is legal.
    const EN_PASSANT: &str = "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3";

    fn pv(moves: &[&str]) -> Vec<String> {
        moves.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn a_black_pass_moves_on_a_move() {
        let after = null_move(&from_fen(MATE_THREAT).unwrap()).unwrap();
        assert_eq!(to_fen(&after), "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4");
        assert_eq!(search_position(&after).fen.as_deref(), Some(to_fen(&after).as_str()));
        assert!(search_position(&after).moves.is_empty());
    }

    #[test]
    fn a_white_pass_clears_en_passant_and_keeps_the_move_number() {
        let pos = from_fen(EN_PASSANT).unwrap();
        assert!(to_fen(&pos).contains(" f6 "), "the en passant capture is legal");
        let after = null_move(&pos).unwrap();
        assert_eq!(to_fen(&after), "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR b KQkq - 1 3");
        assert_eq!(after.turn(), Color::Black);
    }

    #[test]
    fn no_threat_in_check_or_once_the_game_is_over() {
        // 1.e4 f5 2.Qh5+: Black is in check.
        let check = from_fen("rnbqkbnr/ppppp1pp/8/5p1Q/4P3/8/PPPP1PPP/RNB1KBNR b KQkq - 1 2").unwrap();
        assert_eq!(null_move(&check), Err(NoThreat::InCheck));
        // Fool's mate, stalemate and bare kings.
        for fen in [
            "rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3",
            "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1",
            "8/8/4k3/8/8/3K4/8/8 w - - 0 1",
        ] {
            assert_eq!(null_move(&from_fen(fen).unwrap()), Err(NoThreat::GameOver), "{fen}");
        }
    }

    #[test]
    fn no_threat_when_the_opponent_could_not_move_after_the_pass() {
        // White to move; had White passed, Black's king would be stalemated.
        let pos = from_fen("7k/8/6Q1/8/8/8/8/K7 w - - 0 1").unwrap();
        assert_eq!(null_move(&pos), Err(NoThreat::NoMoves));
    }

    #[test]
    fn maps_the_threat_and_stops_at_an_illegal_move() {
        let after = null_move(&from_fen(MATE_THREAT).unwrap()).unwrap();
        let info = Info {
            depth: Some(9),
            score: Some(Score::Mate(1)),
            bound: Bound::Lower,
            pv: pv(&["h5f7", "e8f7"]),
            ..Info::default()
        };
        let threat = threat_line(&after, &info, 8).unwrap();
        assert_eq!(
            (threat.score, threat.bound, threat.depth),
            (Score::Mate(1), Bound::Lower, 9),
            "White mates"
        );
        let first = &threat.moves[0];
        assert_eq!(
            (first.uci.as_str(), first.san.as_str(), first.move_number, first.white),
            ("h5f7", "Qxf7#", 4, true)
        );
        // The engine's second move is illegal (the game is over): the line stops.
        assert_eq!(threat.moves.len(), 1);
    }

    #[test]
    fn turns_blacks_line_around_keeps_it_short_and_numbers_it() {
        let after = null_move(&from_fen(EN_PASSANT).unwrap()).unwrap();
        let info = Info {
            score: Some(Score::Cp(-40)),
            bound: Bound::Lower,
            pv: pv(&["g8f6", "d2d4", "e7e6", "c1g5"]),
            ..Info::default()
        };
        let threat = threat_line(&after, &info, 3).unwrap();
        assert_eq!(threat.score, Score::Cp(40), "Black's -0.40 is White's +0.40");
        assert_eq!(threat.bound, Bound::Upper, "a lower bound for Black caps White's score");
        let shown: Vec<_> = threat.moves.iter().map(|m| (m.san.as_str(), m.move_number, m.white)).collect();
        assert_eq!(shown, [("Nf6", 3, false), ("d4", 4, true), ("e6", 4, false)]);
        // No score or no legal first move: nothing to show yet.
        assert_eq!(threat_line(&after, &Info { score: None, ..info.clone() }, 3), None);
        assert_eq!(threat_line(&after, &Info { pv: pv(&["e2e4"]), ..info }, 3), None);
    }

    #[test]
    fn the_threat_engine_runs_small() {
        let declared = vec![
            UciOption::Spin { name: "Threads".into(), default: 1, min: 1, max: 1024 },
            UciOption::Spin { name: "Hash".into(), default: 16, min: 1, max: 33_554_432 },
            UciOption::Spin { name: "MultiPV".into(), default: 1, min: 1, max: 500 },
            UciOption::String { name: "SyzygyPath".into(), default: "<empty>".into() },
        ];
        let usable = vec![
            ("Threads".to_owned(), "8".to_owned()),
            ("hash".to_owned(), "1024".to_owned()),
            ("MultiPV".to_owned(), "3".to_owned()),
            ("SyzygyPath".to_owned(), "/tb".to_owned()),
        ];
        assert_eq!(
            engine_options(&declared, &usable),
            [("SyzygyPath", "/tb"), ("Threads", "1"), ("Hash", "16")]
                .map(|(n, v)| (n.to_owned(), v.to_owned()))
        );
        // An engine without those options gets only the user's others.
        assert!(engine_options(&[], &usable[3..]).len() == 1);
        assert!(shares_option("SyzygyPath") && !shares_option("threads"));
    }
}
