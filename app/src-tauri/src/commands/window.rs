//! Editor windows (DB-07): the library is the app's main window; opening a
//! database or starting a new game spawns a separate window for it, so
//! several can be open side by side.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, Webview, WebviewUrl, WebviewWindowBuilder};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The open windows' labels: `"main"` (the library) plus one `"editor-…"`
/// per editor window. Mainly for the smoke harness, which can't otherwise
/// tell a new OS window opened.
#[tauri::command]
pub async fn list_windows(app: AppHandle) -> AppResult<Vec<String>> {
    Ok(app.webview_windows().keys().cloned().collect())
}

/// The number of the page the calling window shows (R6-1). The commands
/// that open a tab (`open_game`, `new_game`, `paste_game`) take it back, so
/// a tab isn't added for a page that's gone by the time they get to it.
#[tauri::command]
pub async fn page_number(webview: Webview, state: tauri::State<'_, AppState>) -> AppResult<u64> {
    Ok(state.page_number(webview.label()))
}

/// Opens a new editor window, for `path` if given or a blank "New game"
/// otherwise. The window reads its target back with `take_editor_target`
/// once it starts (`App.svelte`'s own `onMount`, by its window label).
#[tauri::command]
pub async fn open_editor_window(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    path: Option<String>,
) -> AppResult<()> {
    let label = state.new_editor_label();
    state.editor_targets.lock().expect("editor targets lock").insert(label.clone(), path.map(PathBuf::from));
    // No min_inner_size: a tiling compositor (Hyprland) can shrink the
    // window's actual frame well below any minimum we'd ask for, but WebKitGTK
    // then clamps the *webview's* content area to that minimum regardless,
    // so the page renders as if it had the room and the frame just clips it
    // (no CSS can see or fix that, since the viewport never reports the real
    // size). Leaving it unset keeps the two in sync; the UI's own responsive
    // layout (the ribbon, the dock, this library's grid) carries the rest.
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))
        .title("Chess Database")
        .inner_size(1440.0, 900.0)
        .build()
        .map_err(|e| AppError::new(crate::error::ErrorCode::Internal, e.to_string()))?;
    Ok(())
}

/// The database an editor window should open on start-up, if the library
/// window spawned it for one; removed once read, so a reload doesn't repeat it.
#[tauri::command]
pub async fn take_editor_target(
    state: tauri::State<'_, AppState>,
    label: String,
) -> AppResult<Option<String>> {
    let path = state.editor_targets.lock().expect("editor targets lock").remove(&label).flatten();
    Ok(path.map(|p| p.to_string_lossy().into_owned()))
}
