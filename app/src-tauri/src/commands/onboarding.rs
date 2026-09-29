//! Onboarding (QOL-08): the welcome guide's state, the sample database and
//! where a new database for imported games goes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chess_db::{Database, NoProgress, import_reader};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{info, open_for};
use crate::dto::{DbInfoDto, OnboardingDto};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::{AppState, DbUse};

/// Famous games shipped with the app: public game scores with our own short
/// notes. Built into a database the first time it's opened.
const SAMPLE_PGN: &str = include_str!("../../data/sample-games.pgn");
const SAMPLE_DB: &str = "Sample games.chessdb";
/// The sample is built under this name and renamed when complete, so a
/// crash never leaves a half-filled sample behind.
const SAMPLE_BUILDING: &str = "Sample games (building).chessdb";

fn sample_games() -> u32 {
    SAMPLE_PGN.lines().filter(|l| l.starts_with("[Event ")).count() as u32
}

fn sample_path(state: &AppState) -> AppResult<PathBuf> {
    Ok(state.data_dir()?.join(SAMPLE_DB))
}

#[tauri::command]
pub async fn onboarding_status(state: State<'_, AppState>) -> AppResult<OnboardingDto> {
    let show_welcome = state.settings.lock().expect("settings lock").needs_welcome();
    Ok(OnboardingDto {
        show_welcome,
        sample_path: sample_path(&state)?.to_string_lossy().into_owned(),
        sample_games: sample_games(),
    })
}

/// The welcome guide was finished or closed: it doesn't open by itself again.
#[tauri::command]
pub async fn finish_welcome(state: State<'_, AppState>) -> AppResult<()> {
    state.update_settings(|s| s.welcome_done = true);
    Ok(())
}

/// Creates the sample database at `path` from the shipped games.
fn build_sample(path: &Path) -> AppResult<()> {
    let dir = path.parent().ok_or_else(|| AppError::invalid("the sample database needs a folder"))?;
    std::fs::create_dir_all(dir).map_err(|e| {
        AppError::new(ErrorCode::Io, format!("couldn't create the folder {}: {e}", dir.display()))
    })?;
    let building = dir.join(SAMPLE_BUILDING);
    if building.exists() {
        // Left over from a build that didn't finish; it's ours.
        std::fs::remove_dir_all(&building).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))?;
    }
    {
        let db = Mutex::new(Database::create(&building)?);
        let summary = import_reader(&db, SAMPLE_PGN.as_bytes(), 0, &|| 0, &NoProgress)?;
        if summary.warning_count > 0 {
            log::warn!("the sample games have {} import warnings", summary.warning_count);
        }
        // Closed here: Windows can't rename a folder with open files.
    }
    std::fs::rename(&building, path).map_err(|e| AppError::new(ErrorCode::Io, e.to_string()))?;
    log::info!("created the sample database {}", path.display());
    Ok(())
}

/// Opens the sample database in the list, creating it in the app's data
/// folder the first time.
#[tauri::command]
pub async fn open_sample_db(state: State<'_, AppState>) -> AppResult<DbInfoDto> {
    let path = sample_path(&state)?;
    if !path.exists() {
        let p = path.clone();
        tauri::async_runtime::spawn_blocking(move || build_sample(&p))
            .await
            .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))??;
    }
    let handle = open_for(&state, &path, DbUse::List)?;
    info(&state, handle)
}

/// `name` made safe as a file name on every platform.
fn file_name(name: &str) -> String {
    let clean: String =
        name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let clean = clean.trim().trim_end_matches(['.', ' ']);
    if clean.is_empty() { "My games".to_owned() } else { clean.to_owned() }
}

/// `<dir>/<name>.chessdb`, or `<name> 2.chessdb`… when that's taken.
fn free_db_path(dir: &Path, name: &str) -> PathBuf {
    let name = file_name(name);
    (1..)
        .map(|n| match n {
            1 => dir.join(format!("{name}.{}", chess_db::EXTENSION)),
            n => dir.join(format!("{name} {n}.{}", chess_db::EXTENSION)),
        })
        .find(|p| !p.exists())
        .expect("unbounded")
}

/// Where a new database called `name` would go: the Documents folder (like
/// the file dialogs), under a name that isn't taken yet.
#[tauri::command]
pub async fn suggest_db_path(app: AppHandle, name: String) -> AppResult<String> {
    let dir = app
        .path()
        .document_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| AppError::new(ErrorCode::NotFound, format!("no Documents folder: {e}")))?;
    Ok(free_db_path(&dir, &name).to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_games_import_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SAMPLE_DB);
        build_sample(&path).unwrap();
        assert!(!dir.path().join(SAMPLE_BUILDING).exists());

        let db = Database::open(&path).unwrap();
        assert_eq!(db.count(), sample_games());
        assert!(sample_games() >= 10);
        let rows = db.rows(&(0..db.count()).collect::<Vec<_>>()).unwrap();
        assert_eq!(rows[0].white, "Anderssen, Adolf");
        assert!(rows.iter().all(|r| r.plies > 0), "every game has moves");

        // Every game reads back without problems.
        let db = Mutex::new(Database::create(&dir.path().join("check")).unwrap());
        let summary = import_reader(&db, SAMPLE_PGN.as_bytes(), 0, &|| 0, &NoProgress).unwrap();
        assert_eq!((summary.warning_count, summary.skipped), (0, 0), "{:?}", summary.warnings);
    }

    #[test]
    fn the_sample_mates_are_mates() {
        use chess_core::game::NodeId;
        use chess_core::pgn::PgnReader;
        use chess_core::shakmaty::Position;
        let (mut mates, mut stalemates, mut main_mates) = (0, 0, Vec::new());
        for (i, parsed) in PgnReader::new(SAMPLE_PGN.as_bytes()).enumerate() {
            let game = parsed.unwrap().game.unwrap();
            for id in game.preorder().into_iter().filter(|&id| game.node(id).children.is_empty()) {
                let pos = game.position_at(id);
                mates += usize::from(pos.is_checkmate());
                stalemates += usize::from(pos.is_stalemate());
            }
            if game.position_at(game.line_end(NodeId::ROOT)).is_checkmate() {
                main_mates.push(i);
            }
        }
        // Immortal, Evergreen, Opera, Reti (and its 10...Ke8 line), Lasker,
        // Byrne–Fischer; the Saavedra position's 6.c8=Q line is stalemate.
        assert_eq!(main_mates, [0, 1, 2, 5, 6, 7]);
        assert_eq!((mates, stalemates), (7, 1));
    }

    #[test]
    fn a_leftover_partial_sample_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        // The data folder doesn't exist yet, apart from a crashed build.
        let path = dir.path().join("data").join(SAMPLE_DB);
        std::fs::create_dir_all(path.parent().unwrap().join(SAMPLE_BUILDING).join("junk")).unwrap();
        build_sample(&path).unwrap();
        assert_eq!(Database::open(&path).unwrap().count(), sample_games());
        assert!(!path.parent().unwrap().join(SAMPLE_BUILDING).exists());
    }

    #[test]
    fn suggests_a_free_database_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_db_path(dir.path(), "lichess_db"), dir.path().join("lichess_db.chessdb"));
        std::fs::create_dir(dir.path().join("lichess_db.chessdb")).unwrap();
        std::fs::create_dir(dir.path().join("lichess_db 2.chessdb")).unwrap();
        assert_eq!(free_db_path(dir.path(), "lichess_db"), dir.path().join("lichess_db 3.chessdb"));
        assert_eq!(file_name("a/b:c?"), "a_b_c_");
        assert_eq!(file_name("  . "), "My games");
        assert_eq!(file_name("Games. "), "Games");
    }
}
