//! Position index (SRCH-04), opening tree (OPEN-01) and position search
//! (SRCH-03) for the position on screen.

use chess_core::notation::{san, uci};
use chess_core::openings;
use chess_core::position;
use chess_core::shakmaty::{Chess, Position};
use chess_db::{GameId, TreeStats, find_position, position_tree, scan_position, update_position_index};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{thousands, to_job_error};
use crate::dto::{DbInfoDto, OpeningDto, TreeDto, TreeRowDto};
use crate::error::AppResult;
use crate::state::{ActiveFilter, AppState};

/// Brings the position index of `handle` up to date in a background job.
/// `None` when it already is.
#[tauri::command]
pub async fn update_index(state: State<'_, AppState>, handle: u32) -> AppResult<Option<u64>> {
    let db = state.db(handle)?;
    {
        let db = db.lock().expect("db lock");
        if db.positions().plan(db.count()) == chess_db::IndexPlan::UpToDate {
            return Ok(None);
        }
    }
    Ok(Some(state.jobs.spawn("index", "Indexing positions", move |ctx| {
        let s = update_position_index(&db, ctx).map_err(to_job_error)?;
        if s.superseded {
            // The frontend asks again, which rebuilds for the new game ids.
            return Ok("The database was compacted; indexing again".to_owned());
        }
        Ok(format!("Indexed the positions of {} games", thousands(u64::from(s.games))))
    })))
}

fn position_of(state: &AppState, session: u32) -> AppResult<Chess> {
    state.with_session(session, |s| Ok(s.editor.game().position_at(s.editor.cursor())))
}

/// Plies from the standard start to `pos` (by move number).
fn ply_of(pos: &Chess) -> u32 {
    (pos.fullmoves().get() - 1) * 2 + u32::from(pos.turn().is_black())
}

fn row(stats: &TreeStats, white_to_move: bool, mv: Option<(String, String)>) -> TreeRowDto {
    let (wins, losses) = if white_to_move {
        (stats.white_wins, stats.black_wins)
    } else {
        (stats.black_wins, stats.white_wins)
    };
    let decided = wins + losses + stats.draws;
    let score = if decided == 0 {
        0.5
    } else {
        (f64::from(wins) + 0.5 * f64::from(stats.draws)) / f64::from(decided)
    };
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
    let pos = position_of(&state, session)?;
    let db = state.db(handle)?;
    let tree = tauri::async_runtime::spawn_blocking({
        let pos = pos.clone();
        move || position_tree(&db, &pos)
    })
    .await
    .map_err(|e| crate::error::AppError::new(crate::error::ErrorCode::Internal, e.to_string()))??;
    let indexed = state.db(handle)?.lock().expect("db lock").positions().is_built();
    let white = pos.turn().is_white();
    let moves =
        tree.moves.iter().map(|(m, stats)| row(stats, white, m.map(|m| (uci(m), san(&pos, m))))).collect();
    Ok(TreeDto {
        fen: position::to_fen(&pos),
        opening: openings::at_position(&pos).map(OpeningDto::from),
        total: row(&tree.total, white, None),
        moves,
        indexed,
        partial: tree.partial,
        beyond_index: ply_of(&pos) > u32::from(tree.max_ply),
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

fn install_filter(state: &AppState, handle: u32, ids: Vec<GameId>, description: String) {
    state.filters.lock().expect("filters lock").insert(handle, ActiveFilter::new(ids, description));
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
    install_filter(&state, handle, ids, describe_position(&pos));
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
        install_filter(
            &app.state::<AppState>(),
            handle,
            ids,
            format!("{} (full search)", describe_position(&pos)),
        );
        Ok(match n {
            0 => "No game reaches this position".to_owned(),
            1 => "Found 1 game with this position".to_owned(),
            n => format!("Found {} games with this position", thousands(n)),
        })
    }))
}
