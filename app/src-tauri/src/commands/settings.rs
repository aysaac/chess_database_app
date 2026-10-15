//! Settings the frontend reads and writes (INF-02): recent databases
//! (DB-07) and its own preferences.

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
            // Read from the index header, not by opening each database: that
            // took about half a second for one of a million games, every time
            // the library listed it. Failing (not a database any more) just
            // means no count or size to show; recent databases still lists it.
            let counted = exists.then(|| Database::summary(&p).ok()).flatten();
            RecentDbDto {
                path: p.to_string_lossy().into_owned(),
                name: p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                exists,
                games: counted.as_ref().map(|i| i.games),
                size_bytes: counted.as_ref().map(|i| i.size_bytes),
            }
        })
        .collect())
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
