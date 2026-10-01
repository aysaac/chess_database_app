//! Position index (SRCH-04), opening tree (OPEN-01) and position search
//! (SRCH-03) for the position on screen.

use std::sync::Arc;

use chess_core::notation::{san, uci};
use chess_core::openings;
use chess_core::position;
use chess_core::shakmaty::{Chess, Position};
use chess_db::{GameId, TreeStats, find_position, position_tree, scan_position, update_position_index};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{thousands, to_job_error};
use crate::dto::{DbInfoDto, OpeningDto, TreeDto, TreeRowDto};
use crate::error::AppResult;
use crate::settings::FilterStep;
use crate::state::{ActiveFilter, AppState};

/// Brings the position index of `handle` up to date in a background job.
/// `None` when it already is. A build already running for the database (it
/// was reopened under a new handle, A5) is returned instead of a second one.
#[tauri::command]
pub async fn update_index(state: State<'_, AppState>, handle: u32) -> AppResult<Option<u64>> {
    let db = state.db(handle)?;
    // Held until the new build is registered, so two calls can't both start one.
    let mut builds = state.index_builds.lock().expect("index builds lock");
    if let Some(job) = builds.running(&db, &state.jobs) {
        return Ok(Some(job));
    }
    {
        let db = db.lock().expect("db lock");
        if db.positions().plan(db.count()) == chess_db::IndexPlan::UpToDate {
            return Ok(None);
        }
    }
    let key = Arc::downgrade(&db);
    let job = state.jobs.spawn("index", "Indexing positions", move |ctx| {
        let s = update_position_index(&db, ctx).map_err(to_job_error)?;
        if s.superseded {
            // The frontend asks again, which rebuilds for the new game ids.
            return Ok("The database was compacted; indexing again".to_owned());
        }
        Ok(format!("Indexed the positions of {} games", thousands(u64::from(s.games))))
    });
    builds.add(key, job);
    Ok(Some(job))
}

pub(crate) fn position_of(state: &AppState, session: u32) -> AppResult<Chess> {
    state.with_session(session, |s| Ok(s.editor.game().position_at(s.editor.cursor())))
}

/// The position on screen in `session`, and whether its game starts from a
/// set-up position.
pub(crate) fn position_and_setup(state: &AppState, session: u32) -> AppResult<(Chess, bool)> {
    state.with_session(session, |s| {
        let game = s.editor.game();
        Ok((game.position_at(s.editor.cursor()), game.setup.is_some()))
    })
}

/// Plies from the standard start to `pos` (by move number).
pub(crate) fn ply_of(pos: &Chess) -> u32 {
    (pos.fullmoves().get() - 1) * 2 + u32::from(pos.turn().is_black())
}

/// Whether games reaching `pos` may do so past the indexed plies, so some
/// can be missing (a full search finds them): its move number is past them,
/// or the game starts from a set-up position, whose move number is whatever
/// its FEN says (often "1" for a pasted one, QOL-10).
pub(crate) fn beyond_index(pos: &Chess, set_up: bool, max_ply: u16) -> bool {
    set_up || ply_of(pos) > u32::from(max_ply)
}

/// Score for the side to move, 0…1 (0.5 when no game was decided).
pub(crate) fn side_score(white_wins: u32, draws: u32, black_wins: u32, white_to_move: bool) -> f64 {
    let (wins, losses) = if white_to_move { (white_wins, black_wins) } else { (black_wins, white_wins) };
    let decided = wins + losses + draws;
    if decided == 0 { 0.5 } else { (f64::from(wins) + 0.5 * f64::from(draws)) / f64::from(decided) }
}

pub(crate) fn tree_row(stats: &TreeStats, white_to_move: bool, mv: Option<(String, String)>) -> TreeRowDto {
    let decided = stats.white_wins + stats.black_wins + stats.draws;
    let score = side_score(stats.white_wins, stats.draws, stats.black_wins, white_to_move);
    let avg = |sum: u64, n: u32| (n > 0).then(|| (sum / u64::from(n)) as u32);
    let opp = avg(stats.opp_elo_sum, stats.opp_elo_count);
    // Performance: average opponent plus the rating difference the score
    // implies (logistic model, capped at ±800).
    let performance = opp.filter(|_| decided > 0).map(|o| {
        let p = score.clamp(0.01, 0.99);
        o as i32 + (400.0 * (p / (1.0 - p)).log10()).round().clamp(-800.0, 800.0) as i32
    });
    TreeRowDto {
        uci: mv.as_ref().map(|(u, _)| u.clone()),
        san: mv.map(|(_, s)| s),
        games: stats.games,
        white_wins: stats.white_wins,
        draws: stats.draws,
        black_wins: stats.black_wins,
        score,
        avg_elo: avg(stats.elo_sum, stats.elo_count),
        performance,
        last_year: (stats.last_year > 0).then_some(stats.last_year),
    }
}

/// The opening tree for the position on screen in `session`, from database
/// `handle`.
#[tauri::command]
pub async fn opening_tree(state: State<'_, AppState>, handle: u32, session: u32) -> AppResult<TreeDto> {
    let (pos, set_up) = position_and_setup(&state, session)?;
    let db = state.db(handle)?;
    let tree = tauri::async_runtime::spawn_blocking({
        let pos = pos.clone();
        move || position_tree(&db, &pos)
    })
    .await
    .map_err(|e| crate::error::AppError::new(crate::error::ErrorCode::Internal, e.to_string()))??;
    let indexed = state.db(handle)?.lock().expect("db lock").positions().is_built();
    let white = pos.turn().is_white();
    let moves = tree
        .moves
        .iter()
        .map(|(m, stats)| tree_row(stats, white, m.map(|m| (uci(m), san(&pos, m)))))
        .collect();
    Ok(TreeDto {
        fen: position::to_fen(&pos),
        opening: openings::at_position(&pos).map(OpeningDto::from),
        total: tree_row(&tree.total, white, None),
        moves,
        indexed,
        partial: tree.partial,
        beyond_index: beyond_index(&pos, set_up, tree.max_ply),
        max_ply: tree.max_ply,
    })
}

fn describe_position(pos: &Chess) -> String {
    let side = if pos.turn().is_white() { "White" } else { "Black" };
    match openings::at_position(pos) {
        Some(o) => format!("Position: {} ({}), {side} to move", o.name, o.eco),
        None if *pos == Chess::default() => "The starting position".to_owned(),
        None => format!(
            "Position after move {}, {side} to move",
            pos.fullmoves().get() - u32::from(pos.turn().is_white())
        ),
    }
}

/// Limits the list to the games reaching `pos` (found with a `full` scan
/// or the index), remembering the position for next time (QOL-07).
fn install_filter(state: &AppState, handle: u32, ids: Vec<GameId>, pos: &Chess, full: bool) {
    let description = describe_position(pos);
    let description = if full { format!("{description} (full search)") } else { description };
    let step = FilterStep::Position { fen: position::to_fen(pos), full };
    state.set_filter(handle, ActiveFilter::new(ids, description, vec![step]));
}

/// Position search (SRCH-03): limits the list of `handle` to the games
/// reaching the position on screen, through the index.
#[tauri::command]
pub async fn search_position(state: State<'_, AppState>, handle: u32, session: u32) -> AppResult<DbInfoDto> {
    let pos = position_of(&state, session)?;
    let db = state.db(handle)?;
    let hits = tauri::async_runtime::spawn_blocking({
        let pos = pos.clone();
        move || find_position(&db, &pos)
    })
    .await
    .map_err(|e| crate::error::AppError::new(crate::error::ErrorCode::Internal, e.to_string()))??;
    let ids = hits.entries.iter().map(|e| e.game).collect();
    install_filter(&state, handle, ids, &pos, false);
    crate::commands::db::db_info(state, handle).await
}

/// Position search through every game at any depth (slow; a job). The
/// results become the list's filter.
#[tauri::command]
pub async fn scan_position_games(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    session: u32,
) -> AppResult<u64> {
    let pos = position_of(&state, session)?;
    let db = state.db(handle)?;
    Ok(state.jobs.spawn("search", "Searching all games for the position", move |ctx| {
        let ids = scan_position(&db, &pos, ctx).map_err(to_job_error)?;
        let n = ids.len() as u64;
        install_filter(&app.state::<AppState>(), handle, ids, &pos, true);
        Ok(match n {
            0 => "No game reaches this position".to_owned(),
            1 => "Found 1 game with this position".to_owned(),
            n => format!("Found {} games with this position", thousands(n)),
        })
    }))
}
