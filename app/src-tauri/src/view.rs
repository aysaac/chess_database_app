//! Builds the frontend's view of an editing session.

use chess_core::game::{Game, NodeId};
use chess_core::notation;
use chess_core::position::{self, GameStatus};
use chess_core::shakmaty::{Chess, Color, Position};

use crate::dto::{GameStatusDto, GameViewDto, NotationDto, NotationMoveDto, OpeningDto, ShapeDto, SourceDto};
use crate::state::Session;

fn command<'a>(game: &'a Game, id: NodeId, name: &str) -> Option<&'a str> {
    game.node(id).commands.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

/// The line starting at `first` (a move played from `pos`), with the
/// alternatives to each main-continuation move attached to it.
fn line(game: &Game, first: NodeId, mut pos: Chess) -> Vec<NotationMoveDto> {
    let mut out = Vec::new();
    let mut id = first;
    loop {
        let node = game.node(id);
        let mv = node.mv.expect("non-root node has a move");
        let parent = node.parent.expect("non-root node has a parent");
        let siblings = &game.node(parent).children;
        let variations = if siblings.first() == Some(&id) {
            siblings[1..].iter().map(|&alt| line(game, alt, pos.clone())).collect()
        } else {
            Vec::new()
        };
        out.push(NotationMoveDto {
            id: id.0,
            san: notation::san(&pos, mv),
            move_number: pos.fullmoves().get(),
            white: pos.turn() == Color::White,
            nags: node.nags.iter().map(|n| n.0).collect(),
            comment_before: node.comment_before.clone(),
            comment_after: node.comment_after.clone(),
            clock: command(game, id, "clk").map(str::to_owned),
            eval: command(game, id, "eval").map(str::to_owned),
            variations,
        });
        pos.play_unchecked(mv);
        match node.children.first() {
            Some(&next) => id = next,
            None => return out,
        }
    }
}

pub fn notation(game: &Game) -> NotationDto {
    let root = game.root();
    NotationDto {
        root_comment: root.comment_after.clone(),
        moves: root
            .children
            .first()
            .map(|&first| line(game, first, game.start_position()))
            .unwrap_or_default(),
    }
}

/// View of `session`; the notation is included when it changed since the
/// last view sent (or when `force` is set).
pub fn game_view(id: u32, session: &mut Session, force: bool) -> GameViewDto {
    let editor = &session.editor;
    let game = editor.game();
    let cursor = editor.cursor();
    let (pos, history) = game.position_with_history(cursor);
    let (status, winner) = match position::status(&pos, &history) {
        GameStatus::Ongoing => (GameStatusDto::Ongoing, None),
        GameStatus::Checkmate { winner } => (GameStatusDto::Checkmate, Some(winner)),
        GameStatus::Stalemate => (GameStatusDto::Stalemate, None),
        GameStatus::InsufficientMaterial => (GameStatusDto::InsufficientMaterial, None),
        GameStatus::FiftyMoves => (GameStatusDto::FiftyMoves, None),
        GameStatus::Threefold => (GameStatusDto::Threefold, None),
    };
    let last_move = game.node(cursor).mv.and_then(|m| {
        let from = m.from()?;
        let to = position::castle_king_target(m).unwrap_or(m.to());
        Some(vec![from.to_string(), to.to_string()])
    });
    let dests = position::legal_destinations(&pos)
        .into_iter()
        .map(|(from, tos)| (from.to_string(), tos.into_iter().map(|t| t.to_string()).collect()))
        .collect();

    let revision = editor.revision();
    let include_notation = force || session.sent_revision != Some(revision);
    let view = GameViewDto {
        session: id,
        source: session.source.map(|(db, game)| SourceDto { db, game }),
        fen: position::to_fen(&pos),
        turn: if pos.turn() == Color::White { "white" } else { "black" }.into(),
        cursor: cursor.0,
        cursor_path: game.path(cursor).into_iter().map(|n| n.0).collect(),
        in_variation: game.variation_start(cursor).is_some(),
        last_move,
        check: pos.is_check(),
        dests,
        status,
        winner: winner.map(|c| if c == Color::White { "white" } else { "black" }.to_owned()),
        shapes: game.node(cursor).shapes.iter().map(ShapeDto::from).collect(),
        headers: (&game.headers).into(),
        dirty: editor.is_dirty(),
        can_undo: editor.can_undo(),
        can_redo: editor.can_redo(),
        revision,
        notation: include_notation.then(|| notation(game)),
        opening: chess_core::openings::classify(game).map(OpeningDto::from),
    };
    session.sent_revision = Some(revision);
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::game::GameEditor;
    use chess_core::pgn::read_pgn_str;

    #[test]
    fn notation_attaches_variations_to_main_moves() {
        let game = read_pgn_str("1. e4 e5 (1... c5 2. Nf3 (2. Nc3)) 2. Nf3 {[%clk 0:03:00]} *").remove(0);
        let n = notation(&game);
        assert_eq!(n.moves.len(), 3);
        assert_eq!(n.moves[1].san, "e5");
        assert_eq!(n.moves[1].variations.len(), 1);
        let var = &n.moves[1].variations[0];
        assert_eq!(var[0].san, "c5");
        assert!(!var[0].white);
        assert_eq!(var[1].variations[0][0].san, "Nc3");
        assert_eq!(n.moves[2].clock.as_deref(), Some("0:03:00"));
    }

    #[test]
    fn view_sends_notation_only_when_changed() {
        let game = read_pgn_str("1. e4 e5 *").remove(0);
        let mut s = Session::new(GameEditor::new(game), None);
        assert!(game_view(1, &mut s, false).notation.is_some());
        s.editor.forward();
        let v = game_view(1, &mut s, false);
        assert!(v.notation.is_none());
        assert_eq!(v.last_move, Some(vec!["e2".into(), "e4".into()]));
        assert_eq!(v.turn, "black");
        assert!(v.dests.contains_key("e7"));
        assert!(game_view(1, &mut s, true).notation.is_some());
    }
}
