//! Settings the frontend reads and writes (INF-02): recent databases
//! (DB-07) and its own preferences.

use std::path::Path;

use chess_db::Database;
use tauri::{AppHandle, Emitter, State};

use crate::dto::{RecentDbDto, UiPrefChangeDto, UiPrefsDto};
use crate::error::AppResult;
use crate::state::AppState;

/// Event carrying a [`UiPrefChangeDto`] whenever a window saves a preference.
pub const UI_PREF_EVENT: &str = "ui-pref";

#[tauri::command]
pub async fn recent_databases(state: State<'_, AppState>) -> AppResult<Vec<RecentDbDto>> {
    let paths = state.settings.lock().expect("settings lock").recent_databases.clone();
    Ok(paths
        .into_iter()
        .map(|p| {
            let exists = p.is_dir();
            let counted = if exists { count(&state, &p) } else { None };
            RecentDbDto {
                path: p.to_string_lossy().into_owned(),
                name: p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                exists,
                games: counted.map(|(games, _)| games),
                size_bytes: counted.map(|(_, size)| size),
            }
        })
        .collect())
}

/// The game count and size of the database at `path`, for the library
/// (DB-07). None if it can't be read (not a database any more): recent
/// databases still lists it, with nothing to show for them.
fn count(state: &AppState, path: &Path) -> Option<(u32, u64)> {
    let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match state.open_db_at(&canonical) {
        // Open here: ask the handle and leave its files alone. A compaction
        // renames its new files over the old ones, which Windows can refuse
        // while `summary` has `games.idx` open. While a job holds the
        // handle (a compaction, an import), no count rather than waiting.
        Some(db) => db.try_lock().ok().map(|db| {
            let info = db.info();
            (info.games, info.size_bytes)
        }),
        // From the index header, not by opening the database: that took about
        // half a second for one of a million games, every time the library
        // listed it. Another process may have it open, and compact it: see
        // docs/windows-testing.md (B12).
        None => Database::summary(&canonical).ok().map(|s| (s.games, s.size_bytes)),
    }
}

#[tauri::command]
pub async fn forget_recent_database(state: State<'_, AppState>, path: String) -> AppResult<()> {
    state.update_settings(|s| s.recent_databases.retain(|p| p.to_string_lossy() != path));
    Ok(())
}

/// The frontend's saved preferences, as it stored them, and their revision.
#[tauri::command]
pub async fn ui_prefs(state: State<'_, AppState>) -> AppResult<UiPrefsDto> {
    let settings = state.settings.lock().expect("settings lock");
    Ok(UiPrefsDto { values: settings.ui.clone(), revision: settings.ui_revision })
}

/// Saves one preference (`null` removes it) and tells every window, the
/// one that saved it included (`UI_PREF_EVENT`). Returns the revision it's
/// saved at.
#[tauri::command]
pub async fn set_ui_pref(
    app: AppHandle,
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> AppResult<u64> {
    let revision = state.update_settings(|s| s.set_ui(key.clone(), value.clone()));
    // Sent after the settings lock is released: windows order changes by
    // revision, not by when they arrive.
    if let Err(e) = app.emit(UI_PREF_EVENT, UiPrefChangeDto { key, value, revision }) {
        log::warn!("failed to emit a preference change: {e}");
    }
    Ok(revision)
}
