//! Settings the frontend reads and writes (INF-02): recent databases
//! (DB-07) and its own preferences.

use tauri::State;

use crate::dto::RecentDbDto;
use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn recent_databases(state: State<'_, AppState>) -> AppResult<Vec<RecentDbDto>> {
    let s = state.settings.lock().expect("settings lock");
    Ok(s.recent_databases
        .iter()
        .map(|p| RecentDbDto {
            path: p.to_string_lossy().into_owned(),
            name: p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            exists: p.is_dir(),
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
