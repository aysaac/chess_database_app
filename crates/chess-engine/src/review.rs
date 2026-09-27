//! Blunder check (ANL-01): evaluates every main-line position, judges each
//! move by how much it lowered the mover's winning chances, and annotates
//! the game with evaluations (`[%eval]`, GAME-12), `?!`/`?`/`??` and the
//! engine's better line.
//!
//! Thresholds follow Lichess: a drop in winning chances (on a −1…+1 scale)
//! of 0.1 is an inaccuracy, 0.2 a mistake and 0.3 a blunder.

use chess_core::game::{Game, Nag, NodeId};
use chess_core::notation::uci;
use chess_core::position::{self, parse_uci};
use chess_core::shakmaty::{Chess, Color, Move, Position};

use crate::engine::{Engine, Limits, SearchPosition};
use crate::{EngineError, Score};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judgement {
    Inaccuracy,
    Mistake,
    Blunder,
}

impl Judgement {
    pub fn nag(self) -> Nag {
        match self {
            Judgement::Inaccuracy => Nag(6),
            Judgement::Mistake => Nag(2),
            Judgement::Blunder => Nag(4),
        }
    }

    fn from_drop(drop: f64) -> Option<Judgement> {
        if drop >= 0.3 {
            Some(Judgement::Blunder)
        } else if drop >= 0.2 {
            Some(Judgement::Mistake)
        } else if drop >= 0.1 {
            Some(Judgement::Inaccuracy)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveReview {
    pub node: NodeId,
    pub mv: Move,
    /// Evaluation after the move, from White's point of view.
    pub eval: Score,
    pub depth: u32,
    pub judgement: Option<Judgement>,
    /// For judged moves: the engine's line from the position before the
    /// move, and its evaluation (White's point of view).
    pub better: Option<(Vec<Move>, Score)>,
}

#[derive(Debug, Clone)]
pub struct ReviewSettings {
    /// Search limits per position.
    pub limits: Limits,
    /// Longest better line added, in plies.
    pub max_line: usize,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        ReviewSettings { limits: Limits::movetime(500), max_line: 8 }
    }
}

/// Winning chances on a −1…+1 scale.
fn chances(score: Score) -> f64 {
    2.0 * score.win_chance() - 1.0
}

struct Eval {
    /// Side to move's point of view.
    score: Score,
    depth: u32,
    pv: Vec<String>,
}

/// Evaluates the main line of `game`. `progress(done, total)` is called
/// after each position; `cancelled` aborts with [`EngineError::Cancelled`].
pub fn review_game(
    game: &Game,
    engine: &mut Engine,
    settings: &ReviewSettings,
    progress: &mut dyn FnMut(usize, usize),
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<MoveReview>, EngineError> {
    let mut nodes = Vec::new();
    let mut cur = NodeId::ROOT;
    while let Some(&next) = game.node(cur).children.first() {
        nodes.push(next);
        cur = next;
    }
    let start = game.start_position();
    let fen = game.setup.as_ref().map(position::to_fen);
    let mut positions: Vec<Chess> = vec![start.clone()];
    for &n in &nodes {
        let mut p = positions.last().expect("non-empty").clone();
        p.play_unchecked(game.node(n).mv.expect("non-root node has a move"));
        positions.push(p);
    }

    engine.new_game()?;
    let total = positions.len();
    let mut evals = Vec::with_capacity(total);
    for (i, pos) in positions.iter().enumerate() {
        if cancelled() {
            return Err(EngineError::Cancelled);
        }
        let eval = if pos.is_checkmate() {
            Eval { score: Score::Mate(0), depth: 0, pv: Vec::new() }
        } else if pos.is_game_over() {
            Eval { score: Score::Cp(0), depth: 0, pv: Vec::new() }
        } else {
            let search_pos = SearchPosition {
                fen: fen.clone(),
                moves: nodes[..i].iter().map(|&n| uci(game.node(n).mv.expect("has a move"))).collect(),
            };
            let result = engine.search(&search_pos, &settings.limits, &mut |_| {}, cancelled)?;
            if cancelled() {
                return Err(EngineError::Cancelled);
            }
            match result.lines.into_iter().next() {
                Some(line) => Eval {
                    score: line.score.unwrap_or(Score::Cp(0)),
                    depth: line.depth.unwrap_or(0),
                    pv: line.pv,
                },
                None => Eval { score: Score::Cp(0), depth: 0, pv: result.best.into_iter().collect() },
            }
        };
        evals.push(eval);
        progress(i + 1, total);
    }

    let mut out = Vec::with_capacity(nodes.len());
    for (i, &node) in nodes.iter().enumerate() {
        let mv = game.node(node).mv.expect("non-root node has a move");
        let before_pos = &positions[i];
        let white_moved = before_pos.turn() == Color::White;
        let before = &evals[i];
        let after = &evals[i + 1];
        // Both from the mover's point of view.
        let before_score = before.score;
        let after_score = after.score.flip();
        let played_best = before.pv.first().is_some_and(|b| *b == uci(mv));
        let judgement = if played_best {
            None
        } else {
            Judgement::from_drop(chances(before_score) - chances(after_score))
        };
        let better = judgement.and_then(|_| {
            let line = parse_line(before_pos, &before.pv, settings.max_line);
            (!line.is_empty()).then(|| (line, before_score.for_white(white_moved)))
        });
        out.push(MoveReview {
            node,
            mv,
            eval: after_score.for_white(white_moved),
            depth: after.depth,
            judgement,
            better,
        });
    }
    Ok(out)
}

/// UCI moves as legal moves from `pos`, stopping at the first illegal one.
pub fn parse_line(pos: &Chess, uci_moves: &[String], max: usize) -> Vec<Move> {
    let mut pos = pos.clone();
    let mut out = Vec::new();
    for u in uci_moves.iter().take(max) {
        let Some(m) = parse_uci(&pos, u) else { break };
        pos.play_unchecked(m);
        out.push(m);
    }
    out
}

/// Evaluation comment in ChessBase style: `+0.35/22`.
pub fn eval_comment(score: Score, depth: u32) -> String {
    if depth > 0 { format!("{}/{depth}", score.display()) } else { score.display() }
}

/// Position assessment symbol for an evaluation (White's point of view):
/// `=`, `⩲`/`⩱`, `±`/`∓`, `+−`/`−+`.
pub fn assessment_nag(score: Score) -> Nag {
    let cp = match score {
        Score::Mate(n) if n > 0 => 10_000,
        Score::Mate(_) => -10_000,
        Score::Cp(cp) => cp,
    };
    match cp {
        ..=-300 => Nag(19),
        -299..=-100 => Nag(17),
        -99..=-40 => Nag(15),
        -39..=39 => Nag(10),
        40..=99 => Nag(14),
        100..=299 => Nag(16),
        _ => Nag(18),
    }
}

/// Writes the review into `game`: `[%eval]` on every reviewed move, the
/// judgement symbol, and the better line as a variation ending with its
/// evaluation. Moves that changed since the review are skipped. Returns
/// whether anything changed.
pub fn apply_review(game: &mut Game, reviews: &[MoveReview]) -> bool {
    let mut changed = false;
    for r in reviews {
        if !game.is_attached(r.node) || game.node(r.node).mv != Some(r.mv) {
            continue;
        }
        let eval = r.eval.pgn_eval();
        let node = game.node_mut(r.node);
        match node.commands.iter_mut().find(|(name, _)| name == "eval") {
            Some((_, v)) if *v == eval => {}
            Some((_, v)) => {
                *v = eval;
                changed = true;
            }
            None => {
                node.commands.push(("eval".to_owned(), eval));
                changed = true;
            }
        }
        let Some(judgement) = r.judgement else { continue };
        let nag = judgement.nag();
        if !node.nags.contains(&nag) {
            node.nags.retain(|n| !(1..=6).contains(&n.0));
            node.nags.insert(0, nag);
            changed = true;
        }
        if let Some((line, score)) = &r.better {
            let parent = game.node(r.node).parent.expect("a move has a parent");
            if line.first() == Some(&r.mv) || game.find_child(parent, line[0]).is_some() {
                continue;
            }
            let end = game.add_line(parent, line);
            let last = game.node_mut(end);
            last.comment_after = eval_comment(*score, 0);
            last.nags = vec![assessment_nag(*score)];
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judgement_thresholds() {
        assert_eq!(Judgement::from_drop(0.05), None);
        assert_eq!(Judgement::from_drop(0.1), Some(Judgement::Inaccuracy));
        assert_eq!(Judgement::from_drop(0.25), Some(Judgement::Mistake));
        assert_eq!(Judgement::from_drop(0.9), Some(Judgement::Blunder));
        // From equality: a pawn is an inaccuracy, a pawn and a half a mistake,
        // two pawns a blunder.
        let drop = |a: i32, b: i32| chances(Score::Cp(a)) - chances(Score::Cp(b));
        assert_eq!(Judgement::from_drop(drop(0, -100)), Some(Judgement::Inaccuracy));
        assert_eq!(Judgement::from_drop(drop(0, -150)), Some(Judgement::Mistake));
        assert_eq!(Judgement::from_drop(drop(0, -200)), Some(Judgement::Blunder));
        assert_eq!(Judgement::from_drop(drop(0, -500)), Some(Judgement::Blunder));
        assert_eq!(Judgement::from_drop(drop(800, 600)), None, "still winning");
    }

    #[test]
    fn assessment_symbols() {
        assert_eq!(assessment_nag(Score::Cp(0)), Nag(10));
        assert_eq!(assessment_nag(Score::Cp(60)), Nag(14));
        assert_eq!(assessment_nag(Score::Cp(-150)), Nag(17));
        assert_eq!(assessment_nag(Score::Mate(3)), Nag(18));
        assert_eq!(eval_comment(Score::Cp(35), 22), "+0.35/22");
    }
}
