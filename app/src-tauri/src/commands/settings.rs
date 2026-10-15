//! Settings the frontend reads and writes (INF-02): recent databases
//! (DB-07) and its own preferences.

use tauri::State;

use crate::commands::db::peek;
use crate::dto::RecentDbDto;
use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn recent_databases(state: State<'_, AppState>) -> AppResult<Vec<RecentDbDto>> {
    let paths = state.settings.lock().expect("settings lock").recent_databases.clone();
    Ok(paths
        .into_iter()
        .map(|p| {
            let exists = p.is_dir();
            // A peek's own open failing (in use elsewhere, a stale entry) just
            // means no count or size to show; recent databases still lists it.
            let counted = exists.then(|| peek(&state, &p, |db| Ok(db.info())).ok()).flatten();
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

/// The frontend's saved preferences, as it stored them.
#[tauri::command]
pub async fn ui_prefs(state: State<'_, AppState>) -> AppResult<serde_json::Map<String, serde_json::Value>> {
    Ok(state.settings.lock().expect("settings lock").ui.clone())
}

#[tauri::command]
pub async fn set_ui_pref(state: State<'_, AppState>, key: String, value: serde_json::Value) -> AppResult<()> {
    state.update_settings(|s| {
        if value.is_null() {
            s.ui.remove(&key);
        } else {
            s.ui.insert(key, value);
        }
    });
    Ok(())
}
