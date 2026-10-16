//! Tauri shell: a thin command/event layer over the `chess-*` crates.
//! All state and logic live in Rust; the webview renders views and sends
//! user intents.

mod commands;
mod dto;
mod engines;
mod error;
mod settings;
mod state;
mod view;

use tauri::webview::PageLoadEvent;
use tauri::{Emitter, Manager};

use crate::dto::JobDto;
use crate::state::AppState;

/// Event carrying a [`JobDto`] whenever a background job changes.
pub const JOB_EVENT: &str = "job";

/// Lets smoke-test scripts report progress to the terminal.
#[tauri::command]
async fn dev_log(message: String) {
    log::info!("[smoke] {message}");
}

/// Debug builds only: evaluates the script named by `CHESSDB_SMOKE` once the
/// page has loaded, so the UI can be driven end to end without input tools.
fn run_smoke_script<R: tauri::Runtime>(webview: &tauri::Webview<R>) {
    if !cfg!(debug_assertions) {
        return;
    }
    let Some(path) = std::env::var_os("CHESSDB_SMOKE") else { return };
    match std::fs::read_to_string(&path) {
        Ok(script) => {
            if let Err(e) = webview.eval(&script) {
                log::warn!("smoke script failed to start: {e}");
            }
        }
        Err(e) => log::warn!("cannot read smoke script {}: {e}", path.to_string_lossy()),
    }
}

/// Release builds on Windows: turns off WebView2's browser shortcuts, which
/// Linux and macOS webviews don't have: F5/Ctrl+R reload the page (losing
/// what's on screen), Ctrl+P prints it, Ctrl+F in a text field opens a find
/// bar. The page still gets the keys, so the app's own Ctrl+F (flip) works.
/// Debug builds keep them: they include F12 and Ctrl+Shift+I for DevTools.
#[cfg(windows)]
fn disable_browser_shortcuts(window: &tauri::WebviewWindow) {
    if cfg!(debug_assertions) {
        return;
    }
    let result = window.with_webview(|webview| {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
        use windows_core::Interface;
        // SAFETY: COM calls on the webview's own controller, on the thread
        // `with_webview` runs us on.
        let done = unsafe {
            webview.controller().CoreWebView2().and_then(|core| core.Settings()).and_then(|settings| {
                settings.cast::<ICoreWebView2Settings3>()?.SetAreBrowserAcceleratorKeysEnabled(false)
            })
        };
        if let Err(e) = done {
            log::warn!("couldn't turn off the browser shortcuts: {e}");
        }
    });
    if let Err(e) = result {
        log::warn!("couldn't reach the webview: {e}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().level(log::LevelFilter::Info).build())
        .plugin(tauri_plugin_dialog::init())
        // Board images (QOL-06) are put on the clipboard, and the ribbon's
        // paste buttons (IO-08) read it, from Rust only, so the frontend
        // gets no clipboard permissions.
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::default())
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Finished {
                run_smoke_script(webview);
            }
        })
        .setup(|app| {
            // CHESSDB_CONFIG_DIR (test runs) holds the app's data as well,
            // so a run never touches the user's folders.
            let (config_dir, data_dir) = match std::env::var_os("CHESSDB_CONFIG_DIR") {
                Some(dir) => (std::path::PathBuf::from(&dir), std::path::PathBuf::from(dir)),
                None => (app.path().app_config_dir()?, app.path().app_data_dir()?),
            };
            log::info!("settings in {}", config_dir.display());
            app.state::<AppState>().init_dirs(config_dir, data_dir);
            let handle = app.handle().clone();
            app.state::<AppState>().jobs.set_listener(move |status| {
                if let Err(e) = handle.emit(JOB_EVENT, JobDto::from(status)) {
                    log::warn!("failed to emit job event: {e}");
                }
            });
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                disable_browser_shortcuts(&window);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::db::create_db,
            commands::db::open_db,
            commands::db::peek_database,
            commands::db::peek_game_line,
            commands::window::open_editor_window,
            commands::window::take_editor_target,
            commands::window::list_windows,
            commands::db::close_db,
            commands::db::reference_db,
            commands::db::set_reference_db,
            commands::db::db_info,
            commands::db::list_games,
            commands::db::sort_games,
            commands::db::import_pgn,
            commands::db::import_cb,
            commands::db::import_report,
            commands::db::export_pgn,
            commands::db::set_deleted,
            commands::db::set_deleted_rows,
            commands::db::copy_games,
            commands::db::search_games,
            commands::db::clear_filter,
            commands::db::compact_db,
            commands::duplicates::find_duplicates,
            commands::duplicates::duplicates_report,
            commands::duplicates::delete_duplicates,
            commands::duplicates::restore_duplicates,
            commands::duplicates::show_duplicates,
            commands::dbview::db_view,
            commands::dbview::save_db_layout,
            commands::dbview::restore_filter,
            commands::names::list_names,
            commands::names::show_name_games,
            commands::names::rename_name,
            commands::crosstable::cross_table,
            commands::crosstable::show_edition_games,
            commands::tree::update_index,
            commands::tree::opening_tree,
            commands::tree::search_position,
            commands::tree::scan_position_games,
            commands::reference::reference_view,
            commands::reference::reference_lines,
            commands::reference::game_line,
            commands::game::open_game,
            commands::game::new_game,
            commands::game::close_session,
            commands::game::activate_session,
            commands::game::check_fen,
            commands::game::goto_position,
            commands::game::game_view_cmd,
            commands::game::goto_node,
            commands::game::step,
            commands::game::play_move,
            commands::game::legal_moves,
            commands::game::promote_variation,
            commands::game::delete_variation,
            commands::game::delete_remaining,
            commands::game::set_comment,
            commands::game::toggle_nag,
            commands::game::set_shapes,
            commands::game::set_headers,
            commands::game::undo,
            commands::game::redo,
            commands::game::save_game,
            commands::game::game_pgn,
            commands::game::paste_game,
            commands::clipboard::clipboard_text,
            commands::clipboard::copy_text,
            commands::image::copy_board_image,
            commands::image::save_board_image,
            commands::jobs::cancel_job,
            commands::jobs::list_jobs,
            commands::engine::list_engines,
            commands::engine::detect_engines,
            commands::engine::add_engine,
            commands::engine::remove_engine,
            commands::engine::set_default_engine,
            commands::engine::set_engine_options,
            commands::engine::start_analysis,
            commands::engine::stop_analysis,
            commands::engine::analysis_status,
            commands::engine::set_analysis_lines,
            commands::engine::set_analysis_threat,
            commands::engine::add_engine_line,
            commands::engine::blunder_check,
            commands::settings::recent_databases,
            commands::settings::forget_recent_database,
            commands::settings::ui_prefs,
            commands::settings::set_ui_pref,
            commands::onboarding::onboarding_status,
            commands::onboarding::finish_welcome,
            commands::onboarding::open_sample_db,
            commands::onboarding::suggest_db_path,
            dev_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
