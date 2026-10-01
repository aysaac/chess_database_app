use chess_core::game::{
    CommentSlot, Game, GameEditor, GameResult, Headers, Nag, NodeId, PlayResult, Shape, VariationPolicy,
};
use chess_core::notation;
use chess_core::pgn::{PgnReader, PgnWarning, write_game};
use chess_core::position;
use chess_core::shakmaty::{Position, Role, Square};
use tauri::State;

use crate::dto::{
    CommentSlotDto, FenCheckDto, GameViewDto, GotoPositionDto, HeadersDto, ImportWarningDto, LegalMoveDto,
    PastedGameDto, PlayResultDto, SaveModeDto, ShapeDto, StepDto, VariationPolicyDto,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::{AppState, Session};
use crate::view::game_view;

#[tauri::command]
pub async fn open_game(state: State<'_, AppState>, handle: u32, game: u32) -> AppResult<GameViewDto> {
    let db = state.db(handle)?;
    let loaded = db.lock().expect("db lock").read_game(game)?;
    let id = state.add_session(Session::new(GameEditor::new(loaded), Some((handle, game))));
    state.with_session(id, |s| Ok(game_view(id, s, true)))
}

#[tauri::command]
pub async fn new_game(state: State<'_, AppState>, fen: Option<String>) -> AppResult<GameViewDto> {
    let setup = match fen.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => {
            let pos = position::from_fen(f)?;
            (pos != Default::default()).then_some(pos)
        }
        None => None,
    };
    let game = Game::new(Default::default(), setup);
    let id = state.add_session(Session::new(GameEditor::new(game), None));
    state.with_session(id, |s| Ok(game_view(id, s, true)))
}

/// Checks a FEN for the position editor: the normalized FEN, or what is
/// wrong with it.
#[tauri::command]
pub async fn check_fen(fen: String) -> AppResult<FenCheckDto> {
    Ok(match position::from_fen(&fen) {
        Ok(pos) => FenCheckDto { fen: position::to_fen(&pos), error: None },
        Err(e) => FenCheckDto { fen, error: Some(e.to_string()) },
    })
}

/// Goes to the position of a FEN pasted as `text` (QOL-10): a bare FEN, a
/// PGN FEN tag or a Lichess analysis link. When the game in `session`
/// reaches the position (main line first), its cursor moves there;
/// otherwise the frontend starts a new game from it.
#[tauri::command]
pub async fn goto_position(
    state: State<'_, AppState>,
    session: Option<u32>,
    text: String,
) -> AppResult<GotoPositionDto> {
    let Some(fen) = position::find_fen(&text) else { return Ok(GotoPositionDto::NoFen) };
    let pos = match position::from_fen(&fen) {
        Ok(pos) => pos,
        Err(chess_core::Error::InvalidFen(error)) => return Ok(GotoPositionDto::Invalid { fen, error }),
    };
    if let Some(id) = session {
        let view = state.with_session(id, |s| {
            Ok(s.editor.game().find_position(&pos).map(|node| {
                s.editor.goto(node);
                game_view(id, s, false)
            }))
        })?;
        if let Some(view) = view {
            return Ok(GotoPositionDto::InGame { view: Box::new(view) });
        }
    }
    Ok(GotoPositionDto::NewGame { fen: position::to_fen(&pos) })
}

/// What pasted text holds (IO-08 "paste game").
struct Pasted {
    /// The first game, its zero-based index in the text and its warnings.
    first: Option<(usize, Game, Vec<PgnWarning>)>,
    /// Games in the text that can be opened.
    games: u32,
    /// Why the first game that couldn't be read was skipped, if one was.
    skipped: Option<String>,
}

/// A game has a move, or a tag with a known value. A result alone doesn't
/// count: stray text such as a lone `1-0` is no game.
fn is_game(game: &Game) -> bool {
    !game.root().children.is_empty()
        || Headers { result: GameResult::Unknown, ..game.headers.clone() } != Headers::default()
}

/// Reads the games in `text` with the PGN reader imports use.
fn read_pasted(text: &str) -> AppResult<Pasted> {
    let mut pasted = Pasted { first: None, games: 0, skipped: None };
    for parsed in PgnReader::new(text.as_bytes()) {
        let parsed =
            parsed.map_err(|e| AppError::invalid(format!("the pasted text can't be read as PGN ({e})")))?;
        match parsed.game {
            Some(game) if is_game(&game) => {
                pasted.games += 1;
                if pasted.first.is_none() {
                    pasted.first = Some((parsed.index, game, parsed.warnings));
                }
            }
            Some(_) => {}
            None => {
                if pasted.skipped.is_none() {
                    pasted.skipped = parsed.warnings.into_iter().next().map(|w| w.message);
                }
            }
        }
    }
    Ok(pasted)
}

/// Shows `game` as a new unsaved game in `session`, unless that has unsaved
/// changes (or is gone): then in a new session. That's the rule opening a
/// game from the list and QOL-10 follow.
fn open_unsaved(state: &AppState, session: Option<u32>, game: Game) -> AppResult<GameViewDto> {
    let mut editor = Some(GameEditor::new(game));
    if let Some(id) = session {
        let replaced = state.with_session(id, |s| {
            if s.editor.is_dirty() {
                return Ok(None);
            }
            *s = Session::new(editor.take().expect("the editor is only taken here"), None);
            Ok(Some(game_view(id, s, true)))
        });
        match replaced {
            Ok(Some(view)) => return Ok(view),
            Ok(None) => {}
            Err(e) if e.code == ErrorCode::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let editor = editor.expect("the session kept its game, so the editor is unused");
    let id = state.add_session(Session::new(editor, None));
    state.with_session(id, |s| Ok(game_view(id, s, true)))
}

fn paste(state: &AppState, session: Option<u32>, text: &str) -> AppResult<PastedGameDto> {
    let pasted = read_pasted(text)?;
    let Some((index, game, warnings)) = pasted.first else {
        return Ok(PastedGameDto::NoGame { reason: pasted.skipped });
    };
    let label = chess_db::game_label(&game.headers);
    let warnings = warnings
        .into_iter()
        .map(|w| ImportWarningDto {
            number: index as u32 + 1,
            game: None,
            label: label.clone(),
            message: w.message,
        })
        .collect();
    let view = open_unsaved(state, session, game)?;
    Ok(PastedGameDto::Game { view: Box::new(view), games: pasted.games, warnings })
}

/// Opens the first PGN game in pasted `text` (IO-08 "paste game") as a new
/// unsaved game: in `session` unless it has unsaved changes, else in a new
/// session. Says how many games the text has and lists the opened game's
/// problems, as an import would.
#[tauri::command]
pub async fn paste_game(
    state: State<'_, AppState>,
    session: Option<u32>,
    text: String,
) -> AppResult<PastedGameDto> {
    paste(&state, session, &text)
}

#[tauri::command]
pub async fn close_session(state: State<'_, AppState>, session: u32) -> AppResult<()> {
    state.remove_session(session);
    Ok(())
}

/// Makes `session` the game on screen (tab switch, UI-05): a running
/// analysis follows it.
#[tauri::command]
pub async fn activate_session(state: State<'_, AppState>, session: u32) -> AppResult<GameViewDto> {
    if let Some(a) = state.analysis.lock().expect("analysis lock").as_mut() {
        a.attach(session);
    }
    state.with_session(session, |s| Ok(game_view(session, s, false)))
}

#[tauri::command]
pub async fn game_view_cmd(state: State<'_, AppState>, session: u32) -> AppResult<GameViewDto> {
    state.with_session(session, |s| Ok(game_view(session, s, true)))
}

#[tauri::command]
pub async fn goto_node(state: State<'_, AppState>, session: u32, node: u32) -> AppResult<GameViewDto> {
    state.with_session(session, |s| {
        if !s.editor.goto(NodeId(node)) {
            return Err(AppError::not_found(format!("move {node}")));
        }
        Ok(game_view(session, s, false))
    })
}

#[tauri::command]
pub async fn step(state: State<'_, AppState>, session: u32, dir: StepDto) -> AppResult<GameViewDto> {
    state.with_session(session, |s| {
        match dir {
            StepDto::Forward => {
                s.editor.forward();
            }
            StepDto::Back => {
                s.editor.back();
            }
            StepDto::Start => s.editor.to_start(),
            StepDto::End => s.editor.to_end(),
        }
        Ok(game_view(session, s, false))
    })
}

fn parse_square(s: &str) -> AppResult<Square> {
    s.parse().map_err(|_| AppError::invalid(format!("bad square {s:?}")))
}

/// Plays `orig`→`dest` at the cursor. `promotion` is a piece letter
/// (`q`, `r`, `b`, `n`); without it a promotion move returns
/// `needsPromotion`. Without `policy`, playing where another continuation
/// exists returns `needsChoice`.
#[tauri::command]
pub async fn play_move(
    state: State<'_, AppState>,
    session: u32,
    orig: String,
    dest: String,
    promotion: Option<String>,
    policy: Option<VariationPolicyDto>,
) -> AppResult<PlayResultDto> {
    let from = parse_square(&orig)?;
    let to = parse_square(&dest)?;
    let promotion = match promotion.as_deref() {
        Some(p) => Some(
            p.chars()
                .next()
                .and_then(Role::from_char)
                .ok_or_else(|| AppError::invalid(format!("bad promotion {p:?}")))?,
        ),
        None => None,
    };
    state.with_session(session, |s| {
        let pos = s.editor.game().position_at(s.editor.cursor());
        if promotion.is_none() && position::is_promotion(&pos, from, to) {
            return Ok(PlayResultDto::NeedsPromotion);
        }
        let Some(mv) = position::find_move(&pos, from, to, promotion) else {
            return Ok(PlayResultDto::Illegal);
        };
        let policy = policy.map(|p| match p {
            VariationPolicyDto::NewVariation => VariationPolicy::NewVariation,
            VariationPolicyDto::NewMainLine => VariationPolicy::NewMainLine,
            VariationPolicyDto::Overwrite => VariationPolicy::Overwrite,
            VariationPolicyDto::Insert => VariationPolicy::Insert,
        });
        Ok(match s.editor.play(mv, policy) {
            PlayResult::NeedsChoice => PlayResultDto::NeedsChoice,
            PlayResult::Moved { dropped, .. } => {
                PlayResultDto::Moved { view: Box::new(game_view(session, s, false)), dropped: dropped as u32 }
            }
        })
    })
}

/// The legal moves at the cursor, for typing a move (UI-02).
#[tauri::command]
pub async fn legal_moves(state: State<'_, AppState>, session: u32) -> AppResult<Vec<LegalMoveDto>> {
    state.with_session(session, |s| {
        let pos = s.editor.game().position_at(s.editor.cursor());
        Ok(pos
            .legal_moves()
            .iter()
            .map(|&m| LegalMoveDto { san: notation::san(&pos, m), uci: notation::uci(m) })
            .collect())
    })
}

fn edit(
    state: &AppState,
    session: u32,
    f: impl FnOnce(&mut GameEditor) -> AppResult<()>,
) -> AppResult<GameViewDto> {
    state.with_session(session, |s| {
        f(&mut s.editor)?;
        Ok(game_view(session, s, false))
    })
}

#[tauri::command]
pub async fn promote_variation(
    state: State<'_, AppState>,
    session: u32,
    node: u32,
) -> AppResult<GameViewDto> {
    edit(&state, session, |e| {
        e.promote_variation(NodeId(node));
        Ok(())
    })
}

#[tauri::command]
pub async fn delete_variation(state: State<'_, AppState>, session: u32, node: u32) -> AppResult<GameViewDto> {
    edit(&state, session, |e| {
        e.delete_variation(NodeId(node));
        Ok(())
    })
}

#[tauri::command]
pub async fn delete_remaining(state: State<'_, AppState>, session: u32, node: u32) -> AppResult<GameViewDto> {
    edit(&state, session, |e| {
        e.delete_remaining(NodeId(node));
        Ok(())
    })
}

fn check_node(e: &GameEditor, node: u32) -> AppResult<NodeId> {
    let id = NodeId(node);
    if e.game().is_attached(id) { Ok(id) } else { Err(AppError::not_found(format!("move {node}"))) }
}

#[tauri::command]
pub async fn set_comment(
    state: State<'_, AppState>,
    session: u32,
    node: u32,
    slot: CommentSlotDto,
    text: String,
) -> AppResult<GameViewDto> {
    edit(&state, session, |e| {
        let id = check_node(e, node)?;
        let slot = match slot {
            CommentSlotDto::Before => CommentSlot::Before,
            CommentSlotDto::After => CommentSlot::After,
        };
        e.set_comment(id, slot, &text);
        Ok(())
    })
}

#[tauri::command]
pub async fn toggle_nag(
    state: State<'_, AppState>,
    session: u32,
    node: u32,
    nag: u8,
) -> AppResult<GameViewDto> {
    edit(&state, session, |e| {
        let id = check_node(e, node)?;
        e.toggle_nag(id, Nag(nag));
        Ok(())
    })
}

/// Replaces the drawings at the cursor position.
#[tauri::command]
pub async fn set_shapes(
    state: State<'_, AppState>,
    session: u32,
    shapes: Vec<ShapeDto>,
) -> AppResult<GameViewDto> {
    let mut parsed = shapes.into_iter().map(Shape::try_from).collect::<AppResult<Vec<_>>>()?;
    parsed.dedup();
    edit(&state, session, |e| {
        let cursor = e.cursor();
        e.set_shapes(cursor, parsed);
        Ok(())
    })
}

#[tauri::command]
pub async fn set_headers(
    state: State<'_, AppState>,
    session: u32,
    headers: HeadersDto,
) -> AppResult<GameViewDto> {
    edit(&state, session, |e| {
        e.set_headers(headers.into());
        Ok(())
    })
}

#[tauri::command]
pub async fn undo(state: State<'_, AppState>, session: u32) -> AppResult<GameViewDto> {
    edit(&state, session, |e| {
        e.undo();
        Ok(())
    })
}

#[tauri::command]
pub async fn redo(state: State<'_, AppState>, session: u32) -> AppResult<GameViewDto> {
    edit(&state, session, |e| {
        e.redo();
        Ok(())
    })
}

/// Saves the session's game: `replace` overwrites the game it was loaded
/// from; `new` appends it to `handle` (default: the source database).
#[tauri::command]
pub async fn save_game(
    state: State<'_, AppState>,
    session: u32,
    mode: SaveModeDto,
    handle: Option<u32>,
) -> AppResult<GameViewDto> {
    let (game, source) = state.with_session(session, |s| Ok((s.editor.game().clone(), s.source)))?;
    let new_source = match (mode, source) {
        (SaveModeDto::Replace, Some((db_handle, id))) => {
            state.db(db_handle)?.lock().expect("db lock").replace(id, &game)?;
            (db_handle, id)
        }
        (SaveModeDto::Replace, None) => {
            return Err(AppError::invalid("this game has not been saved to a database yet"));
        }
        (SaveModeDto::New, _) => {
            let db_handle = handle
                .or(source.map(|(h, _)| h))
                .ok_or_else(|| AppError::invalid("choose a database to save into"))?;
            let id = state.db(db_handle)?.lock().expect("db lock").append(&game)?;
            (db_handle, id)
        }
    };
    state.with_session(session, |s| {
        s.editor.mark_saved();
        s.source = Some(new_source);
        Ok(game_view(session, s, true))
    })
}

#[tauri::command]
pub async fn game_pgn(state: State<'_, AppState>, session: u32) -> AppResult<String> {
    state.with_session(session, |s| Ok(write_game(s.editor.game())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_GAMES: &str = "[Event \"First\"]\n[White \"Anand\"]\n[Black \"Carlsen\"]\n\n1. e4 e5 2. Nf3 *\n\n\
                             [Event \"Second\"]\n\n1. d4 d5 *\n";

    fn pasted(r: PastedGameDto) -> (GameViewDto, u32, Vec<ImportWarningDto>) {
        match r {
            PastedGameDto::Game { view, games, warnings } => (*view, games, warnings),
            PastedGameDto::NoGame { reason } => panic!("expected a game, got none ({reason:?})"),
        }
    }

    /// The SAN of the session's main line.
    fn main_line(state: &AppState, id: u32) -> Vec<String> {
        state
            .with_session(id, |s| {
                let game = s.editor.game();
                let mut sans = Vec::new();
                let mut pos = game.start_position();
                let mut node = game.root();
                while let Some(&child) = node.children.first() {
                    node = game.node(child);
                    let mv = node.mv.expect("non-root node has a move");
                    sans.push(notation::san(&pos, mv));
                    pos.play_unchecked(mv);
                }
                Ok(sans)
            })
            .unwrap()
    }

    #[test]
    fn pastes_the_first_of_several_games_into_a_clean_session() {
        let state = AppState::default();
        let id = state.add_session(Session::new(GameEditor::new(Game::default()), Some((1, 7))));
        let (view, games, warnings) = pasted(paste(&state, Some(id), TWO_GAMES).unwrap());
        assert_eq!((view.session, games), (id, 2), "the clean session shows the first game");
        assert!(warnings.is_empty());
        assert_eq!(view.headers.white, "Anand");
        assert!(view.source.is_none() && !view.dirty, "a new unsaved game");
        assert!(view.notation.is_some(), "the notation is sent");
        assert_eq!(main_line(&state, id), ["e4", "e5", "Nf3"]);
    }

    #[test]
    fn a_session_with_unsaved_changes_keeps_its_game() {
        let state = AppState::default();
        let mut editor = GameEditor::new(Game::default());
        editor.set_headers(Headers { event: "Edited".into(), ..Headers::default() });
        let id = state.add_session(Session::new(editor, None));
        let (view, _, _) = pasted(paste(&state, Some(id), "1. c4 e5 *").unwrap());
        assert_ne!(view.session, id, "a new session");
        assert_eq!(main_line(&state, view.session), ["c4", "e5"]);
        let event = state.with_session(id, |s| Ok(s.editor.game().headers.event.clone())).unwrap();
        assert_eq!(event, "Edited");
        // No session, or one that's gone: a new session too.
        let (other, _, _) = pasted(paste(&state, None, "1. c4 *").unwrap());
        assert!(other.session != id && other.session != view.session);
        let (gone, _, _) = pasted(paste(&state, Some(999), "1. c4 *").unwrap());
        assert_ne!(gone.session, 999);
    }

    #[test]
    fn lists_the_problems_of_the_pasted_game() {
        let state = AppState::default();
        let text = "[White \"Tal\"]\n[Black \"Botvinnik\"]\n\n1. e4 e5 2. Ke3 Nc6 *";
        let (view, games, warnings) = pasted(paste(&state, None, text).unwrap());
        assert_eq!(games, 1);
        assert_eq!(main_line(&state, view.session), ["e4", "e5"], "the line stops at the illegal move");
        assert_eq!(warnings.len(), 1);
        assert_eq!((warnings[0].number, warnings[0].label.as_str()), (1, "Tal – Botvinnik"));
        assert!(warnings[0].message.contains("2.Ke3"), "{}", warnings[0].message);
    }

    #[test]
    fn text_without_a_game_opens_nothing() {
        let state = AppState::default();
        let fen = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";
        let tag = format!("[FEN \"{fen}\"]");
        for text in ["", "   \n", "hello world", fen, "1-0", "[Event \"?\"]\n\n*", &tag] {
            match paste(&state, None, text).unwrap() {
                PastedGameDto::NoGame { reason: None } => {}
                other => panic!("{text:?}: expected no game, got {other:?}"),
            }
        }
        // A game that can't be read says why.
        match paste(&state, None, "[Variant \"Crazyhouse\"]\n\n1. e4 *").unwrap() {
            PastedGameDto::NoGame { reason: Some(r) } => assert!(r.contains("Crazyhouse"), "{r}"),
            other => panic!("expected the reason, got {other:?}"),
        }
    }

    #[test]
    fn tags_alone_or_a_set_up_position_make_a_game() {
        let state = AppState::default();
        let text = "[White \"Polgar\"]\n[FEN \"8/8/4k3/8/8/4K3/8/8 w - - 0 1\"]\n\n*";
        let (view, games, _) = pasted(paste(&state, None, text).unwrap());
        assert_eq!(games, 1);
        assert_eq!(view.headers.white, "Polgar");
        assert_eq!(view.fen, "8/8/4k3/8/8/4K3/8/8 w - - 0 1");
        // A game that can't be read before one that can: that one opens.
        let text = format!("[Variant \"Atomic\"]\n\n1. e4 *\n\n{TWO_GAMES}");
        let (view, games, _) = pasted(paste(&state, None, &text).unwrap());
        assert_eq!((games, view.headers.white.as_str()), (2, "Anand"));
    }
}
