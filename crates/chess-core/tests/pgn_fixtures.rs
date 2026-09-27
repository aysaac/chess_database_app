//! PGN fixtures from `testdata/pgn/` plus property-based round trips.

use std::fs::File;
use std::path::PathBuf;

use chess_core::game::{Game, GameResult, Nag, NodeId, Shape, ShapeColor};
use chess_core::pgn::{ParsedGame, PgnReader, read_pgn_str, write_game};
use chess_core::shakmaty::{Position, Square};
use proptest::prelude::*;

fn fixture(name: &str) -> Vec<ParsedGame> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn").join(name);
    PgnReader::new(File::open(path).unwrap()).map(|r| r.unwrap()).collect()
}

fn games(parsed: &[ParsedGame]) -> Vec<&Game> {
    parsed.iter().filter_map(|p| p.game.as_ref()).collect()
}

fn assert_round_trip(game: &Game) {
    let written = write_game(game);
    let reread = read_pgn_str(&written);
    assert_eq!(reread.len(), 1, "one game back from:\n{written}");
    assert_eq!(&reread[0], game, "round trip changed game:\n{written}");
    assert_eq!(write_game(&reread[0]), written, "writer is not stable");
}

#[test]
fn variations_fixture() {
    let parsed = fixture("variations.pgn");
    assert!(parsed.iter().all(|p| p.warnings.is_empty()));
    let gs = games(&parsed);
    assert_eq!(gs.len(), 2);
    assert_eq!(gs[0].mainline_len(), 87);
    assert_eq!(gs[0].headers.result, GameResult::WhiteWins);
    assert!(gs[0].has_variations() && gs[0].has_comments());
    let root_alts = &gs[1].root().children;
    assert_eq!(root_alts.len(), 3);
    assert_eq!(gs[1].node(root_alts[2]).comment_before, "Or");
    gs.iter().for_each(|g| assert_round_trip(g));
}

#[test]
fn setup_fixture() {
    let parsed = fixture("setup.pgn");
    let gs = games(&parsed);
    assert_eq!(gs.len(), 3);
    assert!(parsed.iter().all(|p| p.warnings.is_empty()));
    assert_eq!(gs[0].mainline_len(), 5);
    assert_eq!(gs[1].mainline_len(), 4);
    assert_eq!(gs[2].mainline_len(), 10);
    let end = gs[0].line_end(NodeId::ROOT);
    assert!(gs[0].position_at(end).board().queens().any());
    gs.iter().for_each(|g| assert_round_trip(g));
}

#[test]
fn lichess_fixture_keeps_clock_and_eval() {
    let parsed = fixture("lichess.pgn");
    let g = games(&parsed)[0];
    let e4 = g.root().children[0];
    assert_eq!(
        g.node(e4).commands,
        vec![("eval".to_string(), "0.36".to_string()), ("clk".to_string(), "0:03:00".to_string())]
    );
    assert_eq!(g.headers.get_extra("TimeControl"), Some("180+2"));
    assert_eq!(g.headers.round, "-");
    assert_round_trip(g);
}

#[test]
fn malformed_fixture_is_tolerated() {
    let parsed = fixture("malformed.pgn");
    assert_eq!(parsed.len(), 5);
    // Illegal king move truncates the main line after 1. e4 e5.
    assert_eq!(parsed[0].game.as_ref().unwrap().mainline_len(), 2);
    assert_eq!(parsed[0].warnings.len(), 1);
    // Garbage tokens and stray parentheses are ignored.
    let g1 = parsed[1].game.as_ref().unwrap();
    assert_eq!(g1.mainline_len(), 3);
    assert_eq!(g1.headers.result, GameResult::WhiteWins);
    // Missing result token.
    assert_eq!(parsed[2].game.as_ref().unwrap().headers.result, GameResult::Unknown);
    // Bad FEN skips the game.
    assert!(parsed[3].game.is_none());
    assert!(parsed[3].warnings[0].message.contains("FEN"));
    // Game without tags.
    assert_eq!(parsed[4].game.as_ref().unwrap().mainline_len(), 2);
}

#[test]
fn latin1_and_crlf_fixture() {
    let parsed = fixture("latin1-crlf.pgn");
    let g = games(&parsed)[0];
    assert_eq!(g.headers.white, "Réti, Richard");
    assert_eq!(g.headers.black, "Bogoljübow, Efim");
    assert_eq!(g.node(g.root().children[0]).comment_after, "Zügzwang");
    assert_eq!(g.mainline_len(), 2);
    assert_round_trip(g);
}

// ---- property tests -------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    /// Play the n-th legal move (mod count) from the cursor.
    Play(u8),
    /// Move the cursor back n plies.
    Back(u8),
    CommentAfter(String),
    CommentBefore(String),
    Nag(u8),
    Shape(u8, u8, bool, u8),
    Command(String, String),
}

fn word() -> impl Strategy<Value = String> {
    "[a-zA-Zäöü]{1,8}"
}

fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(word(), 1..5).prop_map(|w| w.join(" "))
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => any::<u8>().prop_map(Op::Play),
        2 => (1u8..6).prop_map(Op::Back),
        1 => text().prop_map(Op::CommentAfter),
        1 => text().prop_map(Op::CommentBefore),
        1 => (1u8..=255).prop_map(Op::Nag),
        1 => (0u8..64, 0u8..64, any::<bool>(), 0u8..4).prop_map(|(a, b, arrow, c)| Op::Shape(a, b, arrow, c)),
        1 => ("[a-z]{2,5}", "[0-9:.]{1,8}").prop_map(|(n, v)| Op::Command(n, v)),
    ]
}

fn build(ops: &[Op]) -> Game {
    let mut game = Game::default();
    let mut cursor = NodeId::ROOT;
    for op in ops {
        match op {
            Op::Play(n) => {
                let pos = game.position_at(cursor);
                let moves = pos.legal_moves();
                if moves.is_empty() {
                    continue;
                }
                let m = moves[*n as usize % moves.len()];
                cursor = match game.find_child(cursor, m) {
                    Some(c) => c,
                    None => game.add_child(cursor, m),
                };
            }
            Op::Back(n) => {
                for _ in 0..*n {
                    if let Some(p) = game.node(cursor).parent {
                        cursor = p;
                    }
                }
            }
            Op::CommentAfter(t) => game.node_mut(cursor).comment_after = t.clone(),
            // Before-comments only round-trip on variation starts.
            Op::CommentBefore(t) => {
                if game.variation_start(cursor) == Some(cursor) {
                    game.node_mut(cursor).comment_before = t.clone();
                }
            }
            Op::Nag(n) => {
                let nags = &mut game.node_mut(cursor).nags;
                if cursor != NodeId::ROOT && !nags.contains(&Nag(*n)) {
                    nags.push(Nag(*n));
                }
            }
            Op::Shape(a, b, arrow, c) => {
                let color =
                    [ShapeColor::Green, ShapeColor::Red, ShapeColor::Yellow, ShapeColor::Blue][*c as usize];
                let shape = Shape {
                    color,
                    from: Square::new(u32::from(*a)),
                    to: arrow.then(|| Square::new(u32::from(*b))),
                };
                game.node_mut(cursor).shapes.push(shape);
            }
            Op::Command(n, v) => {
                if !matches!(n.as_str(), "csl" | "cal") {
                    game.node_mut(cursor).commands.push((n.clone(), v.clone()));
                }
            }
        }
    }
    // Squares first, then arrows: the order the writer emits them in.
    for id in game.preorder() {
        game.node_mut(id).shapes.sort_by_key(|s| s.to.is_some());
    }
    game
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn random_games_round_trip(ops in prop::collection::vec(op(), 0..120)) {
        let game = build(&ops);
        let written = write_game(&game);
        let reread = read_pgn_str(&written);
        prop_assert_eq!(reread.len(), 1);
        prop_assert_eq!(&reread[0], &game, "\n{}", written);
    }
}
