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
use crate::engines::{self, Analysis, Events, ThreatSearch};
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
    let path = dunce::canonicalize(&path).unwrap_or(path);
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
            threat: a.has_threat(),
        },
        None => AnalysisStatusDto {
            running: false,
            engine_id: None,
            engine_name: None,
            session: None,
            multipv: 1,
            threat: false,
        },
    }
}

/// Drops `value` on a thread of its own: an engine takes a moment to quit,
/// and the caller may hold locks.
fn drop_later<T: Send + 'static>(value: Option<T>) {
    if let Some(value) = value {
        std::thread::spawn(move || drop(value));
    }
}

/// Stops the analysis; the engine quits on a background thread (that can
/// take a moment).
fn stop_analysis_now(state: &AppState) {
    let old = state.analysis.lock().expect("analysis lock").take();
    drop_later(old);
}

/// Starts analysing the position on screen in `session` with `engine` (the
/// default engine if `None`), replacing any running analysis. With
/// `threat`, the opponent's threat is searched too (ENG-07).
#[tauri::command]
pub async fn start_analysis(
    app: AppHandle,
    state: State<'_, AppState>,
    session: u32,
    engine: Option<String>,
    multipv: u32,
    threat: Option<bool>,
) -> AppResult<AnalysisStatusDto> {
    start_analysis_with(&state, &Events::app(app), session, engine, multipv, threat.unwrap_or(false)).await
}

/// [`start_analysis`], sending what the engines find to `events`.
async fn start_analysis_with(
    state: &AppState,
    events: &Events,
    session: u32,
    engine: Option<String>,
    multipv: u32,
    threat: bool,
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
    stop_analysis_now(state);
    let sink = events.clone();
    let analysis =
        tauri::async_runtime::spawn_blocking(move || Analysis::start(&config, session, multipv, sink))
            .await
            .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))??;
    // Another start that finished meanwhile is replaced.
    let old = state.analysis.lock().expect("analysis lock").replace(analysis);
    drop_later(old);
    // Search the current position right away. The threat engine starts
    // after that, so a slow one doesn't hold up the analysis.
    state.with_session(session, |_| Ok(()))?;
    set_threat(state, events, threat).await?;
    Ok(status(state))
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

/// Turns the threat search (ENG-07) on or off for the running analysis. On
/// starts a second engine process (blocking a moment) that follows the
/// analysis, unless it's turned off again meanwhile; one that can't start
/// is reported as a `threat` event. Off lets it quit. Without a running
/// analysis nothing happens: `start_analysis` takes the setting.
#[tauri::command]
pub async fn set_analysis_threat(
    app: AppHandle,
    state: State<'_, AppState>,
    on: bool,
) -> AppResult<AnalysisStatusDto> {
    set_threat(&state, &Events::app(app), on).await?;
    Ok(status(&state))
}

/// A threat engine (ENG-07) to start for the running analysis.
struct ThreatRequest {
    /// The analysis it's for.
    serial: u64,
    /// The request it answers ([`Analysis::ask_threat`]).
    asked: u64,
    session: u32,
    engine_id: String,
}

/// "Show threat" (ENG-07) for the running analysis: records it and, when
/// it's on and no threat engine runs, starts one (off this thread) and
/// attaches it if that's still wanted once it's up.
async fn set_threat(state: &AppState, events: &Events, on: bool) -> AppResult<()> {
    let Some(request) = ask_threat(state, on) else {
        return Ok(());
    };
    let config = state.settings.lock().expect("settings lock").engine(&request.engine_id).cloned();
    // Removing the engine stopped its analysis.
    let Some(config) = config else {
        return Ok(());
    };
    let (session, sink) = (request.session, events.clone());
    let started = tauri::async_runtime::spawn_blocking(move || ThreatSearch::start(&config, session, sink))
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
        .map_err(|e| e.to_string());
    finish_threat(state, &request, started);
    Ok(())
}

/// Records "Show threat" for the running analysis and says what to start:
/// a threat engine when it's on and none runs. Off lets the running one
/// quit.
fn ask_threat(state: &AppState, on: bool) -> Option<ThreatRequest> {
    let (request, old) = {
        let mut analysis = state.analysis.lock().expect("analysis lock");
        let a = analysis.as_mut()?;
        let (asked, old) = a.ask_threat(on);
        let request = a.needs_threat_engine().then(|| ThreatRequest {
            serial: a.serial,
            asked,
            session: a.session,
            engine_id: a.engine_id.clone(),
        });
        (request, old)
    };
    drop_later(old);
    request
}

/// Attaches a threat engine that finished starting for `request` if the
/// analysis it's for still runs and "Show threat" wasn't asked again
/// meanwhile (a double click: on, then off); it quits otherwise. A failed
/// start is reported (a `threat` event) under the same conditions. Returns
/// whether it was attached.
fn finish_threat(state: &AppState, request: &ThreatRequest, started: Result<ThreatSearch, String>) -> bool {
    let (attached, rest) = {
        let mut analysis = state.analysis.lock().expect("analysis lock");
        match (analysis.as_mut().filter(|a| a.serial == request.serial), started) {
            (Some(a), Ok(search)) => {
                let (attached, rest) = a.offer_threat(request.asked, search);
                (attached.then_some(a.session), rest)
            }
            (Some(a), Err(error)) => {
                log::warn!("couldn't start the threat engine: {error}");
                a.threat_start_failed(request.asked, error);
                (None, None)
            }
            // The analysis stopped or was replaced meanwhile.
            (None, started) => (None, started.ok()),
        }
    };
    drop_later(rest);
    if let Some(session) = attached {
        // Search the position on the board right away. The session may
        // have closed meanwhile; the next one attached is searched anyway.
        let _ = state.with_session(session, |_| Ok(()));
    }
    attached.is_some()
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chess_core::position::parse_uci;

    use super::*;
    use crate::engines::testing::*;
    use crate::settings::EngineConfig;

    /// The app with the engine `config` set up and a game at `SCHOLAR`.
    fn app_with(config: EngineConfig) -> (AppState, u32) {
        let state = AppState::default();
        state.settings.lock().unwrap().engines.push(config);
        let session = state.add_session(session_at(SCHOLAR));
        (state, session)
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    /// Plays `uci` in `session`, as a move on the board does.
    fn play(state: &AppState, session: u32, uci: &str) {
        state
            .with_session(session, |s| {
                let pos = s.editor.game().position_at(s.editor.cursor());
                s.editor.play(parse_uci(&pos, uci).expect("a legal move"), None);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn turning_the_threat_off_while_its_engine_starts_leaves_none() {
        let config = config(fake_uci(), &[]);
        let (state, session) = app_with(config.clone());
        let (events, rx) = events();
        block_on(start_analysis_with(&state, &events, session, None, 1, false)).unwrap();
        let start = |request: &ThreatRequest| {
            ThreatSearch::start(&config, request.session, events.clone()).map_err(|e| e.to_string())
        };

        // A double click: on starts the threat engine, off comes before
        // it's attached.
        let on = ask_threat(&state, true).expect("a threat engine to start");
        let started = start(&on);
        assert!(ask_threat(&state, false).is_none(), "off starts nothing");
        assert!(!finish_threat(&state, &on, started), "not attached once turned off");
        assert!(!status(&state).threat);
        // Moving the board searches no threat.
        play(&state, session, "g7g6");
        assert_eq!(threats_within(&rx, Duration::from_millis(600)).len(), 0, "no threat engine searches");

        // On, off, on while starting: only the last engine is attached.
        let first = ask_threat(&state, true).expect("a threat engine to start");
        let first_started = start(&first);
        assert!(ask_threat(&state, false).is_none());
        let second = ask_threat(&state, true).expect("on again starts another");
        let second_started = start(&second);
        assert!(!finish_threat(&state, &first, first_started), "the first one isn't wanted");
        assert!(!status(&state).threat);
        assert!(finish_threat(&state, &second, second_started));
        assert!(status(&state).threat);
        let t = threat_where(&rx, |t| t.line.is_some());
        assert_eq!((t.session, t.line.unwrap().moves[0].san.as_str()), (session, "gxh5"));
        // Turned off: it goes, and nothing is searched any more.
        block_on(set_threat(&state, &events, false)).unwrap();
        assert!(!status(&state).threat && status(&state).running);
        play(&state, session, "f2f3");
        assert_eq!(threats_within(&rx, Duration::from_millis(600)).len(), 0);
    }

    #[test]
    fn a_threat_engine_that_cant_start_is_reported_once_as_a_threat_error() {
        let dir = tempfile::tempdir().unwrap();
        // Starts: the analysis, "Show threat" on (refused), the analysis
        // again, then with "Show threat" on (refused).
        let path = scripted_fake_uci(dir.path(), "ok refuse ok refuse");
        let (state, session) = app_with(config(path, &[]));
        let (events, rx) = events();
        block_on(start_analysis_with(&state, &events, session, None, 1, false)).unwrap();

        // Turned on while the analysis runs: not an error of the command,
        // a threat event that says the threat engine couldn't start.
        block_on(set_threat(&state, &events, true)).expect("reported as an event");
        let failed = threats_within(&rx, Duration::from_millis(300));
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].start_failed && failed[0].session == session);
        assert!(failed[0].error.as_deref().is_some_and(|e| e.contains("exit")), "{:?}", failed[0].error);
        assert!(status(&state).running && !status(&state).threat);
        // A new game: the analysis follows it, and so does the failure.
        let other = state.add_session(session_at(START));
        state.with_session(other, |_| Ok(())).unwrap();
        let told = threats_within(&rx, Duration::from_millis(300));
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].start_failed && told[0].session == other && told[0].error == failed[0].error);

        // Started with "Show threat" on: one attempt, one report.
        let st = block_on(start_analysis_with(&state, &events, session, None, 1, true)).unwrap();
        assert!(st.running && !st.threat);
        let failed = threats_within(&rx, Duration::from_millis(300));
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].start_failed);

        // A failure for a request turned off meanwhile isn't reported.
        let on = ask_threat(&state, true).expect("a threat engine to start");
        ask_threat(&state, false);
        assert!(!finish_threat(&state, &on, Err("refused".into())));
        assert_eq!(threats_within(&rx, Duration::from_millis(300)).len(), 0);
    }

    #[test]
    fn the_analysis_searches_while_a_slow_threat_engine_starts() {
        let dir = tempfile::tempdir().unwrap();
        let path = scripted_fake_uci(dir.path(), "ok slow:2000");
        let (state, session) = app_with(config(path, &[]));
        let (events, rx) = events();
        std::thread::scope(|scope| {
            let start =
                scope.spawn(|| block_on(start_analysis_with(&state, &events, session, None, 1, true)));
            // The main lines come while the threat engine is still starting.
            let lines = analysis_where(&rx, Duration::from_millis(1500), |d| !d.lines.is_empty());
            assert!(lines.is_some(), "no main lines while the threat engine starts");
            assert!(!start.is_finished(), "the threat engine is still starting");
            let st = start.join().unwrap().unwrap();
            assert!(st.running && st.threat);
        });
        assert_eq!(threat_where(&rx, |t| t.line.is_some()).session, session);
    }
}
