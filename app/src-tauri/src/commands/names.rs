//! Player, event, site and annotator lists (DB-04): browse a name table,
//! show the games of one name, rename a name in every game.

use chess_db::{NameKind, NoProgress, fold, games_with_name};
use tauri::State;

use crate::commands::db::thousands;
use crate::dto::{DbInfoDto, NameKindDto, NameListDto, NameRowDto};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::settings::FilterStep;
use crate::state::{ActiveFilter, AppState};

const MAX_ROWS: u32 = 2000;

/// Names of one kind with their game counts: those containing every word
/// of `text` (case and accents ignored), most games first or by name.
#[tauri::command]
pub async fn list_names(
    state: State<'_, AppState>,
    handle: u32,
    kind: NameKindDto,
    text: String,
    by_games: bool,
    limit: u32,
) -> AppResult<NameListDto> {
    let db = state.db(handle)?;
    tauri::async_runtime::spawn_blocking(move || {
        let kind: NameKind = kind.into();
        let mut db = db.lock().expect("db lock");
        let counts = db.name_counts(kind)?;
        let words: Vec<String> = fold(&text)
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect();
        let hits = db.names(kind).matches(|n| words.iter().all(|w| n.contains(w.as_str())));
        let mut ids: Vec<u32> = (1..counts.len() as u32)
            .filter(|&id| counts[id as usize] > 0 && hits.get(id as usize).copied().unwrap_or(false))
            .collect();
        if by_games {
            let ranks = db.name_ranks(kind);
            ids.sort_by_key(|&id| (std::cmp::Reverse(counts[id as usize]), ranks[id as usize]));
        } else {
            let ranks = db.name_ranks(kind);
            ids.sort_by_key(|&id| ranks[id as usize]);
        }
        let total = ids.len() as u32;
        let table = db.names(kind);
        let rows = ids
            .iter()
            .take(limit.min(MAX_ROWS) as usize)
            .map(|&id| NameRowDto { id, name: table.get(id).to_owned(), games: counts[id as usize] })
            .collect();
        Ok::<_, AppError>(NameListDto { total, rows })
    })
    .await
    .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?
}

fn label(kind: NameKind) -> &'static str {
    match kind {
        NameKind::Player => "Player",
        NameKind::Event => "Event",
        NameKind::Site => "Site",
        NameKind::Annotator => "Annotator",
    }
}

/// Limits the game list to the games of one name.
#[tauri::command]
pub async fn show_name_games(
    state: State<'_, AppState>,
    handle: u32,
    kind: NameKindDto,
    id: u32,
) -> AppResult<DbInfoDto> {
    let db = state.db(handle)?;
    let table = kind;
    let kind: NameKind = kind.into();
    let (ids, name) = tauri::async_runtime::spawn_blocking(move || {
        let name = db.lock().expect("db lock").names(kind).get(id).to_owned();
        games_with_name(&db, kind, id, &NoProgress).map(|ids| (ids, name))
    })
    .await
    .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))??;
    let description = format!("{}: {name}", label(kind));
    state.set_filter(handle, ActiveFilter::new(ids, description, vec![FilterStep::Name { table, name }]));
    crate::commands::db::db_info(state, handle).await
}

/// Renames a name in every game at once; renaming onto an existing name
/// merges the two. Returns a message for the user.
#[tauri::command]
pub async fn rename_name(
    state: State<'_, AppState>,
    handle: u32,
    kind: NameKindDto,
    id: u32,
    name: String,
) -> AppResult<String> {
    let db = state.db(handle)?;
    let kind: NameKind = kind.into();
    let mut db = db.lock().expect("db lock");
    let old = db.names(kind).get(id).to_owned();
    let merged = db.rename_name(kind, id, &name)?;
    let name = name.trim();
    Ok(if merged == 1 {
        format!("Merged “{old}” into “{name}” in 1 game")
    } else if merged > 0 {
        format!("Merged “{old}” into “{name}” in {} games", thousands(merged))
    } else {
        format!("Renamed “{old}” to “{name}”")
    })
}
