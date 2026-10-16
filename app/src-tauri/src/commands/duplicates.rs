//! Duplicate games (DB-08): found in a background job, then marked as
//! deleted (restorable until the database is compacted), restored, or shown
//! in the game list.

use chess_db::{DuplicateOptions, Duplicates, GameId};
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{info, thousands, to_job_error};
use crate::dto::{DbInfoDto, DuplicateGroupDto, DuplicatesDto, GameRowDto};
use crate::error::{AppError, AppResult};
use crate::settings::FilterStep;
use crate::state::{ActiveFilter, AppState};

/// Groups the report previews.
const PREVIEW: usize = 100;

/// The last search for duplicates in one database, kept until the next one
/// (or until the database is closed and another search runs).
pub struct DuplicateScan {
    options: DuplicateOptions,
    found: Duplicates,
    /// `Database::compactions()` when it ran: a compaction renumbers the
    /// games, so the ids are only good while it's the same.
    compactions: u64,
    /// The copies this search marked as deleted, for Restore.
    deleted: Vec<GameId>,
}

fn stale() -> AppError {
    AppError::invalid("The database was compacted after the duplicates were found, so find them again.")
}

fn none_found() -> AppError {
    AppError::invalid("Find the duplicates first.")
}

fn copies(n: u64) -> String {
    if n == 1 { "1 copy".to_owned() } else { format!("{} copies", thousands(n)) }
}

/// Starts looking for duplicate games in a background job; the report is
/// then in `duplicates_report`. `fuzzy` also finds copies that differ in
/// their headers or stop early.
#[tauri::command]
pub async fn find_duplicates(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    fuzzy: bool,
) -> AppResult<u64> {
    let db = state.db(handle)?;
    let options = DuplicateOptions { fuzzy };
    Ok(state.jobs.spawn("duplicates", "Finding duplicate games", move |ctx| {
        let compactions = db.lock().expect("db lock").compactions();
        let found = chess_db::find_duplicates(&db, options, ctx).map_err(to_job_error)?;
        if db.lock().expect("db lock").compactions() != compactions {
            return Err(chess_jobs::JobError::failed(
                "the database was compacted meanwhile; find them again",
            ));
        }
        let message = match found.removable() as u64 {
            0 => "No duplicate games found".to_owned(),
            n => format!("Found {} of {} games", copies(n), thousands(found.groups.len() as u64)),
        };
        let state = app.state::<AppState>();
        // Forget the searches of databases closed since.
        let known: Vec<u32> = state.duplicates.lock().expect("duplicates lock").keys().copied().collect();
        let closed: Vec<u32> = known.into_iter().filter(|&h| state.db(h).is_err()).collect();
        let mut scans = state.duplicates.lock().expect("duplicates lock");
        for h in closed {
            scans.remove(&h);
        }
        scans.insert(handle, DuplicateScan { options, found, compactions, deleted: Vec::new() });
        Ok(message)
    }))
}

/// What the last search for duplicates in `handle` found, if one ran.
#[tauri::command]
pub async fn duplicates_report(state: State<'_, AppState>, handle: u32) -> AppResult<Option<DuplicatesDto>> {
    let db = state.db(handle)?;
    let scans = state.duplicates.lock().expect("duplicates lock");
    let Some(scan) = scans.get(&handle) else { return Ok(None) };
    let db = db.lock().expect("db lock");
    let stale = db.compactions() != scan.compactions;
    let found = &scan.found;
    let preview = if stale {
        Vec::new()
    } else {
        found
            .groups
            .iter()
            .take(PREVIEW)
            .map(|g| {
                Ok(DuplicateGroupDto {
                    keep: GameRowDto::from(db.row(g.keep)?),
                    copies: g.remove.len() as u32,
                    exact: g.exact,
                })
            })
            .collect::<AppResult<_>>()?
    };
    Ok(Some(DuplicatesDto {
        fuzzy: scan.options.fuzzy,
        games: found.games,
        groups: found.groups.len() as u32,
        exact_groups: found.groups.iter().filter(|g| g.exact).count() as u32,
        to_remove: found.removable() as u32,
        deleted: scan.deleted.len() as u32,
        stale,
        preview,
    }))
}

/// Marks the copies found as deleted (every game of a group but the kept
/// one). They can be restored, here or in the list, until the database is
/// compacted. Returns how many games were marked.
#[tauri::command]
pub async fn delete_duplicates(state: State<'_, AppState>, handle: u32) -> AppResult<u32> {
    let db = state.db(handle)?;
    let mut scans = state.duplicates.lock().expect("duplicates lock");
    let scan = scans.get_mut(&handle).ok_or_else(none_found)?;
    let mut db = db.lock().expect("db lock");
    if db.compactions() != scan.compactions {
        return Err(stale());
    }
    // Only those not marked already: Restore gives back just these.
    let mut ids = Vec::new();
    for id in scan.found.to_remove() {
        if !db.record(id)?.is_deleted() {
            ids.push(id);
        }
    }
    let n = db.set_deleted_many(&ids, true)?;
    scan.deleted.extend(ids);
    scan.deleted.sort_unstable();
    scan.deleted.dedup();
    Ok(n)
}

/// Restores the copies `delete_duplicates` marked as deleted. Returns how
/// many games were restored.
#[tauri::command]
pub async fn restore_duplicates(state: State<'_, AppState>, handle: u32) -> AppResult<u32> {
    let db = state.db(handle)?;
    let mut scans = state.duplicates.lock().expect("duplicates lock");
    let scan = scans.get_mut(&handle).ok_or_else(none_found)?;
    let mut db = db.lock().expect("db lock");
    if db.compactions() != scan.compactions {
        return Err(stale());
    }
    let n = db.set_deleted_many(&scan.deleted, false)?;
    scan.deleted.clear();
    Ok(n)
}

/// Limits the game list to the groups found, each kept game followed by
/// its copies (in the list's natural order).
#[tauri::command]
pub async fn show_duplicates(state: State<'_, AppState>, handle: u32) -> AppResult<DbInfoDto> {
    let db = state.db(handle)?;
    let (ids, groups, fuzzy) = {
        let scans = state.duplicates.lock().expect("duplicates lock");
        let scan = scans.get(&handle).ok_or_else(none_found)?;
        if db.lock().expect("db lock").compactions() != scan.compactions {
            return Err(stale());
        }
        (scan.found.listed(), scan.found.groups.len() as u64, scan.options.fuzzy)
    };
    let description = format!(
        "Duplicates{}: {} {}, kept game first",
        if fuzzy { " (fuzzy too)" } else { "" },
        thousands(groups),
        if groups == 1 { "group" } else { "groups" }
    );
    state.set_filter(handle, ActiveFilter::new(ids, description, vec![FilterStep::Duplicates { fuzzy }]));
    info(&state, handle)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chess_db::{Database, NoProgress};

    use super::*;
    use crate::commands::dbview::run_steps;

    #[test]
    fn a_saved_duplicates_filter_is_made_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Mutex::new(Database::create(&dir.path().join("Dups")).unwrap());
        let pgn = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/variations.pgn");
        for _ in 0..2 {
            chess_db::import_pgn(&db, &pgn, &NoProgress).unwrap();
        }
        let steps = [FilterStep::Duplicates { fuzzy: false }];
        assert_eq!(
            run_steps(&db, &steps, &NoProgress).unwrap(),
            [0, 2, 1, 3],
            "each kept game, then its copy"
        );
        db.lock().unwrap().set_deleted_many(&[2, 3], true).unwrap();
        assert!(run_steps(&db, &steps, &NoProgress).unwrap().is_empty(), "deleted copies don't count");
        assert_eq!(copies(1), "1 copy");
        assert_eq!(copies(1234), "1,234 copies");
    }
}
