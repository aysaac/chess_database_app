use chess_core::game::{CommentSlot, Game, GameEditor, Nag, NodeId, PlayResult, Shape, VariationPolicy};
use chess_core::notation;
use chess_core::pgn::write_game;
use chess_core::position;
use chess_core::shakmaty::{Position, Role, Square};
use tauri::State;

use crate::dto::{
    CommentSlotDto, FenCheckDto, GameViewDto, HeadersDto, LegalMoveDto, PlayResultDto, SaveModeDto, ShapeDto,
    StepDto, VariationPolicyDto,
};
use crate::error::{AppError, AppResult};
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
