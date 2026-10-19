//! Duplicate games (DB-08): found in a background job, then marked as
//! deleted (restorable until the database is compacted), restored, or shown
//! in the game list. Deleting and restoring are jobs too, a chunk of games
//! at a time, so the list stays usable.
//!
//! Every search gets a number, which the dialog sends back with each
//! action: a window still showing an older report (another window on the
//! same database searched again) is refused, rather than acting on groups
//! it never showed. A report also only holds while the database is
//! unchanged: a game deleted, edited, added or restored since may have left
//! a copy as the only one, so Delete and Show refuse a report the database
//! changed under. Restore only needs the game numbers, which hold until a
//! compaction.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chess_db::{Database, DuplicateOptions, Duplicates, GameId, Progress};
use chess_jobs::JobError;
use tauri::{AppHandle, Manager, State};

use crate::commands::db::{info, thousands, to_job_error};
use crate::dto::{DbInfoDto, DuplicateGroupDto, DuplicatesDto, GameRowDto};
use crate::error::{AppError, AppResult};
use crate::state::{ActiveFilter, AppState};

/// Groups the report previews.
const PREVIEW: usize = 100;
/// Games marked or restored per lock of the database.
const CHUNK: usize = 16 * 1024;

static NEXT_SCAN: AtomicU64 = AtomicU64::new(1);

/// The searches for duplicates in one open database (by handle in
/// [`AppState`]), dropped when it leaves the game list.
#[derive(Default)]
pub struct DbDuplicates {
    /// The last search, if one ran.
    scan: Option<DuplicateScan>,
    /// The copies marked as deleted from any of its reports, for Restore
    /// (after Find again too): game numbers of `compactions`.
    deleted: Vec<GameId>,
    compactions: u64,
    /// A delete or restore is running.
    busy: bool,
}

/// One search's report.
struct DuplicateScan {
    id: u64,
    options: DuplicateOptions,
    found: Duplicates,
    /// The database's generation the groups hold for: the search's, then
    /// that of each delete or restore of ours.
    generation: u64,
    /// `Database::compactions()` when it ran: a compaction renumbers the
    /// games.
    compactions: u64,
    /// Its copies were all marked as deleted.
    applied: bool,
}

fn none_found() -> AppError {
    AppError::invalid("Find the duplicates first.")
}

fn replaced() -> String {
    "The duplicates were searched for again, maybe in another window, so the report shown is out of date. \
     Look at the new one and try again."
        .to_owned()
}

fn compacted() -> String {
    "The database was compacted after the duplicates were found, so its games were renumbered. Find them again."
        .to_owned()
}

fn changed() -> String {
    "The database changed after the duplicates were found (games were added, edited, deleted or restored), \
     so a copy may now be the only one left. Find them again first."
        .to_owned()
}

fn busy() -> AppError {
    AppError::invalid("Copies are being deleted or restored; wait for that to finish.")
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

fn copies(n: u64) -> String {
    plural(n, "copy", "copies")
}

impl DbDuplicates {
    /// The last search, if it is `scan`.
    fn scan(&mut self, scan: u64) -> Result<&mut DuplicateScan, String> {
        match &mut self.scan {
            Some(s) if s.id == scan => Ok(s),
            Some(_) => Err(replaced()),
            None => Err(none_found().message),
        }
    }

    /// Forgets the deleted copies' numbers after a compaction (they're gone).
    fn renumbered(&mut self, compactions: u64) {
        if self.compactions != compactions {
            self.deleted.clear();
            self.compactions = compactions;
        }
    }
}

impl DuplicateScan {
    /// Whether its groups still hold for `db`.
    fn check(&self, db: &Database) -> Result<(), String> {
        if db.compactions() != self.compactions {
            Err(compacted())
        } else if db.generation() != self.generation {
            Err(changed())
        } else {
            Ok(())
        }
    }
}

/// Keeps a search's report for `handle`, replacing the last one; the
/// copies deleted from earlier reports can still be restored. Nothing is
/// kept if the database left the list meanwhile. Returns the search's
/// number.
fn keep_scan(state: &AppState, handle: u32, options: DuplicateOptions, found: Duplicates) -> Option<u64> {
    let compactions = state.db(handle).ok()?.lock().expect("db lock").compactions();
    if !state.is_listed(handle) {
        return None;
    }
    let id = NEXT_SCAN.fetch_add(1, Ordering::Relaxed);
    let mut all = state.duplicates.lock().expect("duplicates lock");
    let entry = all.entry(handle).or_default();
    entry.renumbered(compactions);
    let generation = found.generation;
    entry.scan = Some(DuplicateScan { id, options, found, generation, compactions, applied: false });
    Some(id)
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
        let found = chess_db::find_duplicates(&db, options, ctx).map_err(to_job_error)?;
        let message = match found.removable() as u64 {
            0 => "No duplicate games found".to_owned(),
            n => format!("Found {} of {}", copies(n), plural(found.groups.len() as u64, "game", "games")),
        };
        keep_scan(&app.state::<AppState>(), handle, options, found);
        Ok(message)
    }))
}

/// What the last search for duplicates in `handle` found, if one ran.
#[tauri::command]
pub async fn duplicates_report(state: State<'_, AppState>, handle: u32) -> AppResult<Option<DuplicatesDto>> {
    report(&state, handle)
}

fn report(state: &AppState, handle: u32) -> AppResult<Option<DuplicatesDto>> {
    let db = state.db(handle)?;
    let mut all = state.duplicates.lock().expect("duplicates lock");
    let Some(entry) = all.get_mut(&handle) else { return Ok(None) };
    let db = db.lock().expect("db lock");
    entry.renumbered(db.compactions());
    let Some(scan) = &entry.scan else { return Ok(None) };
    let compacted = db.compactions() != scan.compactions;
    let stale = scan.check(&db).is_err();
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
        scan: scan.id,
        fuzzy: scan.options.fuzzy,
        games: found.games,
        groups: found.groups.len() as u32,
        exact_groups: found.groups.iter().filter(|g| g.exact).count() as u32,
        to_remove: found.removable() as u32,
        applied: scan.applied,
        deleted: entry.deleted.len() as u32,
        busy: entry.busy,
        stale,
        compacted,
        preview,
    }))
}

/// Marks the copies of report `scan` as deleted (every game of a group but
/// the kept one), in a background job. They can be restored, here or in the
/// list, until the database is compacted. Refused if another search replaced
/// that report or the database changed since.
#[tauri::command]
pub async fn delete_duplicates(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    scan: u64,
) -> AppResult<u64> {
    let (db, ids) = start_delete(&state, handle, scan)?;
    Ok(state.jobs.spawn("duplicates-delete", "Deleting duplicate games", move |ctx| {
        let state = app.state::<AppState>();
        let result = delete_copies(&state, handle, scan, &db, &ids, ctx);
        finish(&state, handle);
        Ok(match result? {
            0 => "The copies were marked as deleted already".to_owned(),
            n => format!("Marked {} as deleted", copies(n.into())),
        })
    }))
}

/// Checks report `scan` still holds and marks its database busy; the
/// copies to delete.
fn start_delete(state: &AppState, handle: u32, scan: u64) -> AppResult<(Arc<Mutex<Database>>, Vec<GameId>)> {
    let db = state.db(handle)?;
    let mut all = state.duplicates.lock().expect("duplicates lock");
    let entry = all.get_mut(&handle).ok_or_else(none_found)?;
    if entry.busy {
        return Err(busy());
    }
    let s = entry.scan(scan).map_err(AppError::invalid)?;
    s.check(&db.lock().expect("db lock")).map_err(AppError::invalid)?;
    let ids = s.found.to_remove();
    entry.busy = true;
    Ok((db, ids))
}

/// The delete job: marks `ids` as deleted a chunk at a time, checking
/// before each one that the report still holds. Returns how many games it
/// marked; on failure or Stop, those it marked can be restored.
fn delete_copies(
    state: &AppState,
    handle: u32,
    scan: u64,
    db: &Mutex<Database>,
    ids: &[GameId],
    progress: &dyn Progress,
) -> Result<u32, JobError> {
    let mut marked = 0u32;
    let so_far = |marked: u32, why: String| {
        JobError::failed(if marked == 0 {
            why
        } else {
            format!("{why} ({} marked as deleted)", copies(marked.into()))
        })
    };
    for (n, chunk) in ids.chunks(CHUNK).enumerate() {
        progress.check_cancelled().map_err(to_job_error)?;
        let mut all = state.duplicates.lock().expect("duplicates lock");
        let entry = all.get_mut(&handle).ok_or_else(|| so_far(marked, "The database was closed".into()))?;
        let s = entry.scan(scan).map_err(|why| so_far(marked, why))?;
        let mut db = db.lock().expect("db lock");
        s.check(&db).map_err(|why| so_far(marked, why))?;
        let mut fresh = Vec::with_capacity(chunk.len());
        for &id in chunk {
            if !db.record(id).map_err(to_job_error)?.is_deleted() {
                fresh.push(id);
            }
        }
        marked += db.set_deleted_many(&fresh, true).map_err(to_job_error)?;
        s.generation = db.generation();
        if n + 1 == ids.len().div_ceil(CHUNK) {
            s.applied = true;
        }
        entry.deleted.extend(fresh);
        progress.report(((n + 1) * CHUNK).min(ids.len()) as u64, ids.len() as u64);
    }
    if ids.is_empty()
        && let Some(s) =
            state.duplicates.lock().expect("duplicates lock").get_mut(&handle).and_then(|e| e.scan.as_mut())
        && s.id == scan
    {
        s.applied = true;
    }
    Ok(marked)
}

/// Clears the busy mark of `handle` once a delete or restore ends, however
/// it ends; the deleted copies' numbers are put in order.
fn finish(state: &AppState, handle: u32) {
    if let Some(entry) = state.duplicates.lock().expect("duplicates lock").get_mut(&handle) {
        entry.busy = false;
        entry.deleted.sort_unstable();
        entry.deleted.dedup();
    }
}

/// Restores, in a background job, the copies deleted from this database's
/// reports (also from earlier ones). `scan` is the report shown, which must
/// still be the last one. Returns the job.
#[tauri::command]
pub async fn restore_duplicates(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    scan: u64,
) -> AppResult<u64> {
    let (db, ids, own) = start_restore(&state, handle, scan)?;
    Ok(state.jobs.spawn("duplicates-restore", "Restoring duplicate games", move |ctx| {
        let state = app.state::<AppState>();
        let result = restore_copies(&state, handle, &db, &ids, own, ctx);
        finish(&state, handle);
        Ok(format!("Restored {}", copies(result?.into())))
    }))
}

/// Checks report `scan` is still the last one and marks its database busy;
/// the copies to restore, and whether they're all the report's own (then
/// the report holds afterwards; else it lacks the games restored).
fn start_restore(
    state: &AppState,
    handle: u32,
    scan: u64,
) -> AppResult<(Arc<Mutex<Database>>, Vec<GameId>, bool)> {
    let db = state.db(handle)?;
    let mut all = state.duplicates.lock().expect("duplicates lock");
    let entry = all.get_mut(&handle).ok_or_else(none_found)?;
    if entry.busy {
        return Err(busy());
    }
    let own = entry.scan(scan).map_err(AppError::invalid)?.found.to_remove();
    entry.renumbered(db.lock().expect("db lock").compactions());
    if entry.deleted.is_empty() {
        return Err(AppError::invalid(
            "There are no deleted copies to restore: the database was compacted since, or they were restored.",
        ));
    }
    entry.busy = true;
    let own = entry.deleted.iter().all(|id| own.binary_search(id).is_ok());
    Ok((db, entry.deleted.clone(), own))
}

/// The restore job: restores `ids` (the database's deleted copies, in
/// order) a chunk at a time. The last report still holds afterwards if they
/// were all its `own` copies. Returns how many games it restored; those it
/// didn't reach stay restorable.
fn restore_copies(
    state: &AppState,
    handle: u32,
    db: &Mutex<Database>,
    ids: &[GameId],
    own: bool,
    progress: &dyn Progress,
) -> Result<u32, JobError> {
    let mut restored = 0u32;
    let mut done = 0usize;
    let result = (|| {
        for chunk in ids.chunks(CHUNK) {
            progress.check_cancelled().map_err(to_job_error)?;
            let mut all = state.duplicates.lock().expect("duplicates lock");
            let entry = all.get_mut(&handle).ok_or_else(|| JobError::failed("The database was closed"))?;
            let mut db = db.lock().expect("db lock");
            if db.compactions() != entry.compactions {
                return Err(JobError::failed(
                    "The database was compacted: the deleted copies are gone for good",
                ));
            }
            let fresh = own && entry.scan.as_ref().is_some_and(|s| s.check(&db).is_ok());
            restored += db.set_deleted_many(chunk, false).map_err(to_job_error)?;
            if let Some(s) = entry.scan.as_mut() {
                s.applied = false;
                if fresh {
                    s.generation = db.generation();
                }
            }
            done += chunk.len();
            progress.report(done as u64, ids.len() as u64);
        }
        Ok(())
    })();
    if let Some(entry) = state.duplicates.lock().expect("duplicates lock").get_mut(&handle) {
        // Nothing else changed the list meanwhile (busy): the first `done`
        // are the ones restored.
        entry.deleted.retain(|id| ids[..done].binary_search(id).is_err());
    }
    result.map(|()| restored)
}

/// Limits the game list to the groups of report `scan`, each kept game
/// followed by its copies (in the list's natural order). The filter isn't
/// saved for the next time the database is opened: it would mean searching
/// again, and finding other groups after a change.
#[tauri::command]
pub async fn show_duplicates(state: State<'_, AppState>, handle: u32, scan: u64) -> AppResult<DbInfoDto> {
    show(&state, handle, scan)?;
    info(&state, handle)
}

fn show(state: &AppState, handle: u32, scan: u64) -> AppResult<()> {
    let db = state.db(handle)?;
    let (ids, groups, fuzzy) = {
        let mut all = state.duplicates.lock().expect("duplicates lock");
        let s = all.get_mut(&handle).ok_or_else(none_found)?.scan(scan).map_err(AppError::invalid)?;
        s.check(&db.lock().expect("db lock")).map_err(AppError::invalid)?;
        (s.found.listed(), s.found.groups.len() as u64, s.options.fuzzy)
    };
    let kind = if fuzzy { "exact and fuzzy" } else { "exact" };
    let description = format!("Duplicates ({kind}): {}", plural(groups, "group", "groups"));
    state.set_filter(handle, ActiveFilter::new(ids, description, Vec::new()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chess_db::NoProgress;

    use super::*;
    use crate::state::DbUse;

    /// A database in the list with `pgn` imported `times` times.
    fn listed(state: &AppState, dir: &Path, pgn: &str, times: usize) -> u32 {
        let handle = state.add_db(Database::create(&dir.join("Dups")).unwrap(), DbUse::List);
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn").join(pgn);
        let db = state.db(handle).unwrap();
        for _ in 0..times {
            chess_db::import_pgn(&db, &file, &NoProgress).unwrap();
        }
        handle
    }

    /// Runs a search as the job does; its number.
    fn search(state: &AppState, handle: u32, fuzzy: bool) -> u64 {
        let options = DuplicateOptions { fuzzy };
        let found = chess_db::find_duplicates(&state.db(handle).unwrap(), options, &NoProgress).unwrap();
        keep_scan(state, handle, options, found).unwrap()
    }

    fn delete(state: &AppState, handle: u32, scan: u64) -> Result<u32, String> {
        let (db, ids) = start_delete(state, handle, scan).map_err(|e| e.message)?;
        let result = delete_copies(state, handle, scan, &db, &ids, &NoProgress);
        finish(state, handle);
        result.map_err(|e| e.to_string())
    }

    fn restore(state: &AppState, handle: u32, scan: u64) -> Result<u32, String> {
        let (db, ids, own) = start_restore(state, handle, scan).map_err(|e| e.message)?;
        let result = restore_copies(state, handle, &db, &ids, own, &NoProgress);
        finish(state, handle);
        result.map_err(|e| e.to_string())
    }

    fn deleted(state: &AppState, handle: u32) -> Vec<GameId> {
        let db = state.db(handle).unwrap();
        let db = db.lock().unwrap();
        (0..db.count()).filter(|&id| db.record(id).unwrap().is_deleted()).collect()
    }

    /// R1-4: a game deleted (or edited, or restored) in the list after the
    /// search makes the report stale: Delete refuses it instead of deleting
    /// every copy of a game whose kept copy is gone.
    #[test]
    fn a_report_the_database_changed_under_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        // 0–8: the fixture; copies of 4 in 5 and 6.
        let handle = listed(&state, dir.path(), "duplicates.pgn", 1);
        let scan = search(&state, handle, false);
        state.db(handle).unwrap().lock().unwrap().set_deleted(4, true).unwrap();
        let now = report(&state, handle).unwrap().unwrap();
        assert!(now.stale && !now.compacted && now.preview.is_empty());
        let err = delete(&state, handle, scan).unwrap_err();
        assert!(err.contains("changed after the duplicates were found"), "{err}");
        assert!(show(&state, handle, scan).unwrap_err().message.contains("changed"));
        assert_eq!(deleted(&state, handle), [4], "5 and 6 are still there");

        // Found again, 5 is kept now.
        let scan = search(&state, handle, false);
        assert_eq!(delete(&state, handle, scan), Ok(1));
        assert_eq!(deleted(&state, handle), [4, 6]);
        // Its own delete leaves the report good, for Show.
        assert!(!report(&state, handle).unwrap().unwrap().stale);
        show(&state, handle, scan).unwrap();
    }

    /// R1-5: an action names the report it was shown; another search (in
    /// another window on the same database) replaced it: refused.
    #[test]
    fn an_action_on_a_replaced_report_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let handle = listed(&state, dir.path(), "duplicates.pgn", 1);
        let exact = search(&state, handle, false);
        let fuzzy = search(&state, handle, true);
        assert_ne!(exact, fuzzy);
        for err in [
            delete(&state, handle, exact).unwrap_err(),
            restore(&state, handle, exact).unwrap_err(),
            show(&state, handle, exact).unwrap_err().message,
        ] {
            assert!(err.contains("searched for again"), "{err}");
        }
        assert!(deleted(&state, handle).is_empty());
        assert_eq!(delete(&state, handle, fuzzy), Ok(4));
        assert_eq!(deleted(&state, handle), [1, 2, 5, 6]);
    }

    /// R1-7: Find again after deleting keeps what Restore brings back;
    /// R1-12: both run a chunk at a time (here 2.5 chunks).
    #[test]
    fn restore_after_finding_again() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let handle = listed(&state, dir.path(), "duplicates.pgn", 1);
        let scan = search(&state, handle, false);
        assert_eq!(delete(&state, handle, scan), Ok(2));
        let report1 = report(&state, handle).unwrap().unwrap();
        assert!(report1.applied && report1.deleted == 2);
        assert_eq!(delete(&state, handle, scan), Ok(0), "marked already");

        let again = search(&state, handle, true);
        let report2 = report(&state, handle).unwrap().unwrap();
        assert_eq!(
            (report2.to_remove, report2.applied, report2.deleted),
            (2, false, 2),
            "the fuzzy group's copies"
        );
        assert_eq!(delete(&state, handle, again), Ok(2));
        assert_eq!(report(&state, handle).unwrap().unwrap().deleted, 4);
        assert_eq!(restore(&state, handle, again), Ok(4));
        assert!(deleted(&state, handle).is_empty());
        // Copies of the first report came back, which the second lacks.
        let report3 = report(&state, handle).unwrap().unwrap();
        assert_eq!((report3.applied, report3.deleted, report3.stale), (false, 0, true));
        assert!(restore(&state, handle, again).unwrap_err().contains("no deleted copies"));

        // Many copies: chunk by chunk.
        let dir = tempfile::tempdir().unwrap();
        let big = state.add_db(Database::create(&dir.path().join("Big")).unwrap(), DbUse::List);
        let db = state.db(big).unwrap();
        let game =
            chess_core::pgn::read_pgn_str("[White \"A, B\"]\n[Black \"C, D\"]\n\n1. e4 e5 *").remove(0);
        let mut prepared = Vec::new();
        for n in 0..(5 * CHUNK / 2) {
            let mut g = game.clone();
            g.headers.round = format!("{}", n % 60_000 + 1);
            g.headers.event = format!("Event {}", n / 60_000);
            prepared.push(chess_db::PreparedGame::new(&g));
        }
        db.lock().unwrap().append_prepared(&prepared).unwrap();
        db.lock().unwrap().append_prepared(&prepared).unwrap();
        let scan = search(&state, big, false);
        let n = 5 * CHUNK as u32 / 2;
        assert_eq!(delete(&state, big, scan), Ok(n));
        assert_eq!(report(&state, big).unwrap().unwrap().deleted, n);
        assert_eq!(restore(&state, big, scan), Ok(n));
        // Its own copies back: the report holds, to delete them again.
        let now = report(&state, big).unwrap().unwrap();
        assert_eq!((now.deleted, now.applied, now.stale), (0, false, false));
    }

    /// Stop while deleting: what was marked can be restored, the rest
    /// deleted later.
    #[test]
    fn stopping_a_delete_leaves_what_it_did_restorable() {
        struct StopAfter(std::cell::Cell<u32>);
        impl Progress for StopAfter {
            fn report(&self, _done: u64, _total: u64) {
                self.0.set(self.0.get() + 1);
            }
            fn is_cancelled(&self) -> bool {
                self.0.get() >= 1
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let handle = state.add_db(Database::create(&dir.path().join("Stop")).unwrap(), DbUse::List);
        let db = state.db(handle).unwrap();
        let game =
            chess_core::pgn::read_pgn_str("[White \"A, B\"]\n[Black \"C, D\"]\n\n1. e4 e5 *").remove(0);
        let prepared: Vec<_> = (0..CHUNK + 10)
            .map(|n| {
                let mut g = game.clone();
                g.headers.round = format!("{}", n + 1);
                chess_db::PreparedGame::new(&g)
            })
            .collect();
        db.lock().unwrap().append_prepared(&prepared).unwrap();
        db.lock().unwrap().append_prepared(&prepared).unwrap();
        let scan = search(&state, handle, false);
        let (db, ids) = start_delete(&state, handle, scan).unwrap();
        assert!(start_delete(&state, handle, scan).unwrap_err().message.contains("wait"), "busy");
        let stop = StopAfter(std::cell::Cell::new(0));
        assert!(matches!(delete_copies(&state, handle, scan, &db, &ids, &stop), Err(JobError::Cancelled)));
        finish(&state, handle);
        let now = report(&state, handle).unwrap().unwrap();
        assert_eq!((now.deleted, now.applied, now.stale), (CHUNK as u32, false, false));
        assert_eq!(delete(&state, handle, scan), Ok(10), "the rest");
        assert_eq!(restore(&state, handle, scan), Ok(CHUNK as u32 + 10));
    }

    /// R1-6: the list filter isn't saved (an older version can't read it,
    /// and reopening would search again); R1-13: it doesn't claim an order.
    #[test]
    fn show_in_game_list_isnt_saved_for_next_time() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        state.init_dirs(dir.path().join("config"), dir.path().join("data"));
        let handle = listed(&state, dir.path(), "variations.pgn", 2);
        let scan = search(&state, handle, false);
        show(&state, handle, scan).unwrap();
        let filter = state.filters.lock().unwrap();
        let filter = &filter[&handle];
        assert_eq!(filter.ids.as_slice(), [0, 2, 1, 3], "each kept game, then its copy");
        assert_eq!(filter.description, "Duplicates (exact): 2 groups");
        assert!(filter.steps.is_empty());
        assert_eq!(state.db_view(handle).unwrap().filter, None, "nothing saved");
        let saved = std::fs::read_to_string(dir.path().join("config/settings.json")).unwrap_or_default();
        assert!(!saved.contains("duplicates"), "{saved}");
    }

    /// R1-13: closing the database drops its searches.
    #[test]
    fn closing_the_database_drops_its_searches() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let handle = listed(&state, dir.path(), "duplicates.pgn", 1);
        search(&state, handle, false);
        assert!(report(&state, handle).unwrap().is_some());
        state.release_db(handle, DbUse::List);
        assert!(state.duplicates.lock().unwrap().is_empty());
        assert_eq!(copies(1), "1 copy");
        assert_eq!(copies(1234), "1,234 copies");
        assert_eq!(plural(1, "game", "games"), "1 game");
    }
}
