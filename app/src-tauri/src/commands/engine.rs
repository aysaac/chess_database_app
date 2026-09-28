//! Engines (ENG-01/09), interactive analysis (ENG-02/05) and the blunder
//! check (ANL-01).

use std::path::PathBuf;

use chess_core::game::NodeId;
use chess_core::position::parse_uci;
use chess_engine::review::{ReviewSettings, apply_review, review_game};
use chess_engine::{Engine, EngineError, Limits};
use chess_jobs::JobError;
use tauri::{AppHandle, Manager, State};

use crate::dto::{AnalysisStatusDto, EngineDto, GameViewDto};
use crate::engines::{self, Analysis};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;
use crate::view::game_view;

fn engine_list(state: &AppState) -> Vec<EngineDto> {
    let s = state.settings.lock().expect("settings lock");
    let default = s.preferred_engine().map(|e| e.id.clone());
    s.engines.iter().map(|e| EngineDto::new(e, default.as_deref())).collect()
}

/// Starts `path` to read its name and options (blocking).
fn probe(path: PathBuf) -> Result<(String, Vec<chess_engine::UciOption>), EngineError> {
    let engine = Engine::start(&path)?;
    let id = engine.id().clone();
    engine.quit();
    Ok((id.name, id.options))
}

async fn probe_async(path: PathBuf) -> AppResult<(String, Vec<chess_engine::UciOption>)> {
    tauri::async_runtime::spawn_blocking(move || probe(path))
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
        .map_err(AppError::from)
}

/// Adds engines found automatically that aren't configured yet.
async fn add_detected(state: &AppState) -> AppResult<usize> {
    let known: Vec<PathBuf> =
        state.settings.lock().expect("settings lock").engines.iter().map(|e| e.path.clone()).collect();
    let mut added = 0;
    for path in engines::detect().into_iter().filter(|p| !known.contains(p)) {
        match probe_async(path.clone()).await {
            Ok((name, declared)) => {
                if state.update_settings(|s| s.add_engine(path, name, declared, true)) {
                    added += 1;
                }
            }
            Err(e) => log::warn!("skipping engine {}: {}", path.display(), e.message),
        }
    }
    Ok(added)
}

/// Configured engines. The first call on a fresh install looks for the
/// bundled Stockfish.
#[tauri::command]
pub async fn list_engines(state: State<'_, AppState>) -> AppResult<Vec<EngineDto>> {
    let empty = state.settings.lock().expect("settings lock").engines.is_empty();
    if empty {
        add_detected(&state).await?;
    }
    Ok(engine_list(&state))
}

#[tauri::command]
pub async fn detect_engines(state: State<'_, AppState>) -> AppResult<Vec<EngineDto>> {
    add_detected(&state).await?;
    Ok(engine_list(&state))
}

/// Adds the engine at `path` after checking that it speaks UCI.
#[tauri::command]
pub async fn add_engine(state: State<'_, AppState>, path: String) -> AppResult<Vec<EngineDto>> {
    let path = PathBuf::from(path);
    let path = path.canonicalize().unwrap_or(path);
    if state.settings.lock().expect("settings lock").engines.iter().any(|e| e.path == path) {
        return Err(AppError::new(ErrorCode::AlreadyExists, "this engine is already in the list"));
    }
    let (name, declared) = probe_async(path.clone()).await?;
    if !state.update_settings(|s| s.add_engine(path, name, declared, false)) {
        return Err(AppError::new(ErrorCode::AlreadyExists, "this engine is already in the list"));
    }
    Ok(engine_list(&state))
}

#[tauri::command]
pub async fn remove_engine(state: State<'_, AppState>, id: String) -> AppResult<Vec<EngineDto>> {
    let running = state.analysis.lock().expect("analysis lock").as_ref().is_some_and(|a| a.engine_id == id);
    if running {
        stop_analysis_now(&state);
    }
    state.update_settings(|s| {
        s.engines.retain(|e| e.id != id);
        if s.default_engine.as_deref() == Some(id.as_str()) {
            s.default_engine = None;
        }
    });
    Ok(engine_list(&state))
}

#[tauri::command]
pub async fn set_default_engine(state: State<'_, AppState>, id: String) -> AppResult<Vec<EngineDto>> {
    state.update_settings(|s| {
        if s.engine(&id).is_some() {
            s.default_engine = Some(id);
            Ok(())
        } else {
            Err(AppError::not_found("engine"))
        }
    })?;
    Ok(engine_list(&state))
}

/// Replaces the changed option values of an engine. Values equal to the
/// default are dropped; a running analysis with this engine applies them.
#[tauri::command]
pub async fn set_engine_options(
    state: State<'_, AppState>,
    id: String,
    options: Vec<(String, String)>,
) -> AppResult<Vec<EngineDto>> {
    let applied = state.update_settings(|s| {
        let engine = s.engine_mut(&id).ok_or_else(|| AppError::not_found("engine"))?;
        let mut clean = Vec::new();
        for (name, value) in options {
            let decl = engine
                .declared
                .iter()
                .find(|o| o.name().eq_ignore_ascii_case(&name))
                .ok_or_else(|| AppError::invalid(format!("{} has no option {name:?}", engine.name)))?;
            if matches!(decl, chess_engine::UciOption::Button { .. }) {
                continue;
            }
            let value = decl.validate(&value).ok_or_else(|| {
                AppError::invalid(format!("{value:?} isn't a valid value for {}", decl.name()))
            })?;
            if value != decl.default_value() {
                clean.push((decl.name().to_owned(), value));
            }
        }
        // Options reset to their default must be sent too.
        let mut changes: Vec<(String, String)> = clean.clone();
        for (name, _) in &engine.options {
            if !clean.iter().any(|(n, _)| n == name)
                && let Some(decl) = engine.declared.iter().find(|o| o.name() == name)
            {
                changes.push((name.clone(), decl.default_value()));
            }
        }
        engine.options = clean;
        Ok::<_, AppError>(changes)
    })?;
    if let Some(a) = state.analysis.lock().expect("analysis lock").as_mut()
        && a.engine_id == id
    {
        for (name, value) in applied.iter().filter(|(n, _)| !n.eq_ignore_ascii_case("MultiPV")) {
            a.set_option(name, value);
        }
    }
    Ok(engine_list(&state))
}

fn status(state: &AppState) -> AnalysisStatusDto {
    let analysis = state.analysis.lock().expect("analysis lock");
    match analysis.as_ref().filter(|a| a.is_alive()) {
        Some(a) => AnalysisStatusDto {
            running: true,
            engine_id: Some(a.engine_id.clone()),
            engine_name: Some(a.engine_name.clone()),
            session: Some(a.session),
            multipv: a.multipv,
        },
        None => AnalysisStatusDto {
            running: false,
            engine_id: None,
            engine_name: None,
            session: None,
            multipv: 1,
        },
    }
}

/// Stops the analysis; the engine quits on a background thread (that can
/// take a moment).
fn stop_analysis_now(state: &AppState) {
    let old = state.analysis.lock().expect("analysis lock").take();
    if let Some(old) = old {
        std::thread::spawn(move || drop(old));
    }
}

/// Starts analysing the position on screen in `session` with `engine` (the
/// default engine if `None`), replacing any running analysis.
#[tauri::command]
pub async fn start_analysis(
    app: AppHandle,
    state: State<'_, AppState>,
    session: u32,
    engine: Option<String>,
    multipv: u32,
) -> AppResult<AnalysisStatusDto> {
    let config = {
        let s = state.settings.lock().expect("settings lock");
        match engine.as_deref() {
            Some(id) => s.engine(id).cloned(),
            None => s.preferred_engine().cloned(),
        }
    };
    let Some(config) = config else {
        return Err(AppError::new(ErrorCode::Engine, "no engine is set up yet: add one under Engines"));
    };
    stop_analysis_now(&state);
    let analysis =
        tauri::async_runtime::spawn_blocking(move || Analysis::start(&config, session, multipv, app))
            .await
            .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))??;
    *state.analysis.lock().expect("analysis lock") = Some(analysis);
    // Search the current position right away.
    state.with_session(session, |_| Ok(()))?;
    Ok(status(&state))
}

#[tauri::command]
pub async fn stop_analysis(state: State<'_, AppState>) -> AppResult<AnalysisStatusDto> {
    stop_analysis_now(&state);
    Ok(status(&state))
}

#[tauri::command]
pub async fn analysis_status(state: State<'_, AppState>) -> AppResult<AnalysisStatusDto> {
    Ok(status(&state))
}

#[tauri::command]
pub async fn set_analysis_lines(state: State<'_, AppState>, multipv: u32) -> AppResult<AnalysisStatusDto> {
    let session = state.analysis.lock().expect("analysis lock").as_mut().map(|a| {
        a.set_multipv(multipv);
        a.session
    });
    if let Some(session) = session {
        state.with_session(session, |_| Ok(()))?;
    }
    Ok(status(&state))
}

/// Adds engine moves (UCI, from the cursor) to the game: clicking a move of
/// a principal variation plays up to it (`goto`), and "add line" pastes the
/// whole line as a variation with its evaluation as a comment (ENG-05).
#[tauri::command]
pub async fn add_engine_line(
    state: State<'_, AppState>,
    session: u32,
    moves: Vec<String>,
    comment: Option<String>,
    goto: bool,
) -> AppResult<GameViewDto> {
    state.with_session(session, |s| {
        let cursor = s.editor.cursor();
        let mut pos = s.editor.game().position_at(cursor);
        let mut line = Vec::with_capacity(moves.len());
        for m in &moves {
            let mv =
                parse_uci(&pos, m).ok_or_else(|| AppError::invalid(format!("illegal engine move {m}")))?;
            chess_core::shakmaty::Position::play_unchecked(&mut pos, mv);
            line.push(mv);
        }
        let comment = comment.as_deref().map(str::trim).filter(|c| !c.is_empty());
        s.editor.add_line(cursor, &line, comment, goto);
        Ok(game_view(session, s, false))
    })
}

fn to_job_error(e: EngineError) -> JobError {
    match e {
        EngineError::Cancelled => JobError::Cancelled,
        other => JobError::failed(other),
    }
}

/// Runs the blunder check on the session's main line as a background job,
/// `movetime_ms` per position. The annotations are applied as one undo step
/// when it finishes.
#[tauri::command]
pub async fn blunder_check(
    app: AppHandle,
    state: State<'_, AppState>,
    session: u32,
    engine: Option<String>,
    movetime_ms: u64,
) -> AppResult<u64> {
    let config = {
        let s = state.settings.lock().expect("settings lock");
        match engine.as_deref() {
            Some(id) => s.engine(id).cloned(),
            None => s.preferred_engine().cloned(),
        }
    }
    .ok_or_else(|| AppError::new(ErrorCode::Engine, "no engine is set up yet: add one under Engines"))?;
    let game = state.with_session(session, |s| Ok(s.editor.game().clone()))?;
    if game.node(NodeId::ROOT).children.is_empty() {
        return Err(AppError::invalid("this game has no moves to check"));
    }
    let label = format!("Blunder check with {}", config.name);
    let movetime_ms = movetime_ms.clamp(50, 60_000);
    Ok(state.jobs.spawn("review", label, move |ctx| {
        let mut engine = Engine::start(&config.path).map_err(to_job_error)?;
        for (name, value) in engines::usable_options(&config) {
            if !name.eq_ignore_ascii_case("MultiPV") {
                engine.set_option(&name, &value).map_err(to_job_error)?;
            }
        }
        let settings = ReviewSettings { limits: Limits::movetime(movetime_ms), max_line: 10 };
        let reviews = review_game(
            &game,
            &mut engine,
            &settings,
            &mut |done, total| ctx.progress(done as u64, total as u64),
            &|| ctx.is_cancelled(),
        )
        .map_err(to_job_error)?;
        engine.quit();

        let count = |j| reviews.iter().filter(|r| r.judgement == Some(j)).count();
        use chess_engine::review::Judgement::{Blunder, Inaccuracy, Mistake};
        let (b, m, i) = (count(Blunder), count(Mistake), count(Inaccuracy));
        let state = app.state::<AppState>();
        state
            .with_session(session, |s| {
                s.editor.edit_with(|g| {
                    let changed = apply_review(g, &reviews);
                    if changed && g.headers.annotator.is_empty() {
                        g.headers.annotator = config.name.clone();
                    }
                    changed
                });
                Ok(())
            })
            .map_err(|e| JobError::Failed(e.message))?;
        let plural = |n: usize, word: &str| if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") };
        Ok(format!(
            "Blunder check done: {}, {}, {}",
            plural(b, "blunder"),
            plural(m, "mistake"),
            plural(i, "inaccuracy").replace("inaccuracys", "inaccuracies")
        ))
    }))
}
