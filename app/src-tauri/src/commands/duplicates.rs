//! Duplicate games (DB-08): found in a background job, then marked as
//! deleted (restorable until the database is compacted), restored, or shown
//! in the game list. Deleting and restoring are jobs too, a chunk of games
//! at a time, so the list stays usable. Before its copies are deleted, a
//! kept game takes the fields of their index records it lacks (R4-1,
//! `chess_db::delete_copies`); Restore takes them out again, while the
//! database stays open (the earlier records are kept in memory).
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
use std::sync::{Arc, Mutex, PoisonError};

use chess_db::{Database, DuplicateOptions, Duplicates, Filled, GameId, Progress};
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
    /// What their kept games took from them, in order, for Restore to take
    /// out again.
    filled: Vec<Filled>,
    compactions: u64,
    /// A delete or restore is running.
    busy: bool,
}

/// One search's report.
struct DuplicateScan {
    id: u64,
    options: DuplicateOptions,
    found: Arc<Duplicates>,
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

    /// Forgets the deleted copies' numbers after a compaction (they're
    /// gone), and what their kept games took (the earlier records are gone
    /// too).
    fn renumbered(&mut self, compactions: u64) {
        if self.compactions != compactions {
            self.deleted.clear();
            self.filled.clear();
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
    let found = Arc::new(found);
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
        filled: entry.filled.len() as u32,
        busy: entry.busy,
        stale,
        compacted,
        preview,
    }))
}

/// Marks the copies of report `scan` as deleted (every game of a group but
/// the kept one), in a background job, after filling in what each kept game
/// lacks of them. They can be restored, here or in the list, until the
/// database is compacted. Refused if another search replaced that report or
/// the database changed since.
#[tauri::command]
pub async fn delete_duplicates(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    scan: u64,
) -> AppResult<u64> {
    let (db, found) = start_delete(&state, handle, scan)?;
    Ok(state.jobs.spawn("duplicates-delete", "Deleting duplicate games", move |ctx| {
        let state = app.state::<AppState>();
        Ok(delete_job(&state, handle, scan, &db, &found, ctx)?.message())
    }))
}

/// Checks report `scan` still holds and marks its database busy; the
/// report.
fn start_delete(
    state: &AppState,
    handle: u32,
    scan: u64,
) -> AppResult<(Arc<Mutex<Database>>, Arc<Duplicates>)> {
    let db = state.db(handle)?;
    let mut all = state.duplicates.lock().expect("duplicates lock");
    let entry = all.get_mut(&handle).ok_or_else(none_found)?;
    if entry.busy {
        return Err(busy());
    }
    let s = entry.scan(scan).map_err(AppError::invalid)?;
    s.check(&db.lock().expect("db lock")).map_err(AppError::invalid)?;
    let found = s.found.clone();
    entry.busy = true;
    Ok((db, found))
}

/// Clears the busy mark of a database when its delete or restore ends,
/// however it ends (a panic too), and puts its deleted copies in order.
struct Busy<'a> {
    state: &'a AppState,
    handle: u32,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        // Not `expect`: a second panic while unwinding would abort the app.
        let mut all = self.state.duplicates.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = all.get_mut(&self.handle) {
            entry.busy = false;
            entry.deleted.sort_unstable();
            entry.deleted.dedup();
        }
    }
}

/// What a delete did.
#[derive(Debug, Default, PartialEq, Eq)]
struct Deleted {
    /// Copies marked as deleted.
    marked: u32,
    /// Kept games that took fields from their copies first.
    filled: u32,
    /// Groups left alone (`chess_db::DeletedCopies::left`).
    left: u32,
}

impl Deleted {
    fn message(&self) -> String {
        let mut out = match self.marked {
            0 => "The copies were marked as deleted already".to_owned(),
            n => format!("Marked {} as deleted", copies(n.into())),
        };
        if self.filled > 0 {
            let kept = plural(self.filled.into(), "kept game", "kept games");
            out.push_str(&format!(", after filling in what {kept} lacked from them"));
        }
        if self.left > 0 {
            let groups = plural(self.left.into(), "group", "groups");
            out.push_str(&format!(
                "; {groups} left alone, as the database changed: find the duplicates again"
            ));
        }
        out
    }
}

/// The delete job, started by [`start_delete`].
fn delete_job(
    state: &AppState,
    handle: u32,
    scan: u64,
    db: &Mutex<Database>,
    found: &Duplicates,
    progress: &dyn Progress,
) -> Result<Deleted, JobError> {
    let _busy = Busy { state, handle };
    delete_copies(state, handle, scan, db, found, progress)
}

/// Deletes the copies of `found`'s groups a chunk of groups at a time (up
/// to [`CHUNK`] copies, one group at least), checking before each one that
/// the report still holds. Each chunk fills in what its kept games lack of
/// their copies, committed before its copies are marked as deleted
/// (`chess_db::delete_copies`). On failure or Stop, what it did can be
/// restored.
fn delete_copies(
    state: &AppState,
    handle: u32,
    scan: u64,
    db: &Mutex<Database>,
    found: &Duplicates,
    progress: &dyn Progress,
) -> Result<Deleted, JobError> {
    let mut done = Deleted::default();
    let so_far = |done: &Deleted, why: String| {
        JobError::failed(if done.marked == 0 {
            why
        } else {
            format!("{why} ({} marked as deleted)", copies(done.marked.into()))
        })
    };
    let groups = &found.groups;
    let total = found.removable() as u64;
    let (mut at, mut copies_done) = (0, 0u64);
    while at < groups.len() {
        let mut end = at + 1;
        let mut n = groups[at].remove.len();
        while end < groups.len() && n + groups[end].remove.len() <= CHUNK {
            n += groups[end].remove.len();
            end += 1;
        }
        progress.check_cancelled().map_err(to_job_error)?;
        {
            let mut all = state.duplicates.lock().expect("duplicates lock");
            let entry =
                all.get_mut(&handle).ok_or_else(|| so_far(&done, "The database was closed".into()))?;
            let s = entry.scan(scan).map_err(|why| so_far(&done, why))?;
            let mut db = db.lock().expect("db lock");
            s.check(&db).map_err(|why| so_far(&done, why))?;
            let out = chess_db::delete_copies(&mut db, &groups[at..end]).map_err(to_job_error)?;
            s.generation = db.generation();
            if end == groups.len() {
                s.applied = true;
            }
            done.marked += out.marked.len() as u32;
            done.filled += out.filled.len() as u32;
            done.left += out.left;
            entry.deleted.extend(out.marked);
            entry.filled.extend(out.filled);
        }
        at = end;
        copies_done += n as u64;
        // Not under the locks: whatever happens there leaves them good.
        progress.report(copies_done, total);
    }
    if groups.is_empty()
        && let Some(s) =
            state.duplicates.lock().expect("duplicates lock").get_mut(&handle).and_then(|e| e.scan.as_mut())
        && s.id == scan
    {
        s.applied = true;
    }
    Ok(done)
}

/// Restores, in a background job, the copies deleted from this database's
/// reports (also from earlier ones), then takes out what their kept games
/// took from them. `scan` is the report shown, which must still be the last
/// one. Returns the job.
#[tauri::command]
pub async fn restore_duplicates(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: u32,
    scan: u64,
) -> AppResult<u64> {
    let work = start_restore(&state, handle, scan)?;
    Ok(state.jobs.spawn("duplicates-restore", "Restoring duplicate games", move |ctx| {
        let state = app.state::<AppState>();
        Ok(restore_job(&state, handle, &work, ctx)?.message())
    }))
}

/// What a restore works on ([`start_restore`]).
struct ToRestore {
    db: Arc<Mutex<Database>>,
    /// The deleted copies, in order.
    ids: Vec<GameId>,
    /// What their kept games took, in order.
    fills: Vec<Filled>,
    /// They're all the last report's own: it holds afterwards (else it
    /// lacks the games restored).
    own: bool,
}

/// Checks report `scan` is still the last one and marks its database busy;
/// what to restore.
fn start_restore(state: &AppState, handle: u32, scan: u64) -> AppResult<ToRestore> {
    let db = state.db(handle)?;
    let mut all = state.duplicates.lock().expect("duplicates lock");
    let entry = all.get_mut(&handle).ok_or_else(none_found)?;
    if entry.busy {
        return Err(busy());
    }
    let found = entry.scan(scan).map_err(AppError::invalid)?.found.clone();
    entry.renumbered(db.lock().expect("db lock").compactions());
    if entry.deleted.is_empty() && entry.filled.is_empty() {
        return Err(AppError::invalid(
            "There are no deleted copies to restore: the database was compacted since, or they were restored.",
        ));
    }
    entry.busy = true;
    let removed = found.to_remove();
    let mut kept: Vec<GameId> = found.groups.iter().map(|g| g.keep).collect();
    kept.sort_unstable();
    let own = entry.deleted.iter().all(|id| removed.binary_search(id).is_ok())
        && entry.filled.iter().all(|f| kept.binary_search(&f.game).is_ok());
    Ok(ToRestore { db, ids: entry.deleted.clone(), fills: entry.filled.clone(), own })
}

/// What a restore did.
#[derive(Debug, Default, PartialEq, Eq)]
struct Restored {
    /// Copies restored.
    copies: u32,
    /// Kept games that gave back what they took from their copies.
    unfilled: u32,
    /// Kept games changed since (edited, renamed), which keep what they
    /// took.
    changed: u32,
}

impl Restored {
    fn message(&self) -> String {
        let kept = |n: u32| plural(n.into(), "kept game", "kept games");
        let mut out = match (self.copies, self.unfilled) {
            (0, 1) => "Took out what 1 kept game had filled in from its copies".to_owned(),
            (0, n @ 2..) => format!("Took out what {} had filled in from their copies", kept(n)),
            (n, 0) => format!("Restored {}", copies(n.into())),
            (n, m) => format!(
                "Restored {}, and took out what {} had filled in from them",
                copies(n.into()),
                kept(m)
            ),
        };
        if self.changed > 0 {
            let rest = if self.changed == 1 {
                "was changed since, so it keeps what it took from its copies"
            } else {
                "were changed since, so they keep what they took from their copies"
            };
            out.push_str(&format!("; {} {rest}", kept(self.changed)));
        }
        out
    }
}

/// The restore job, started by [`start_restore`].
fn restore_job(
    state: &AppState,
    handle: u32,
    work: &ToRestore,
    progress: &dyn Progress,
) -> Result<Restored, JobError> {
    let _busy = Busy { state, handle };
    restore_copies(state, handle, work, progress)
}

/// Restores the deleted copies a chunk at a time, each chunk leaving the
/// deleted copies as it's restored; then, once every copy is back (so that
/// no deleted copy has fields its kept game lacks), takes out what the kept
/// games took, newest first, from those unchanged since. The last report
/// still holds afterwards if they were all its own. Returns what it did;
/// what it didn't reach stays restorable.
fn restore_copies(
    state: &AppState,
    handle: u32,
    work: &ToRestore,
    progress: &dyn Progress,
) -> Result<Restored, JobError> {
    let ToRestore { db, ids, fills, own } = work;
    let own = *own;
    let mut restored = Restored::default();
    let total = (ids.len() + fills.len()) as u64;
    let mut done = 0u64;
    let compacted = || JobError::failed("The database was compacted: the deleted copies are gone for good");
    for chunk in ids.chunks(CHUNK) {
        progress.check_cancelled().map_err(to_job_error)?;
        {
            let mut all = state.duplicates.lock().expect("duplicates lock");
            let entry = all.get_mut(&handle).ok_or_else(|| JobError::failed("The database was closed"))?;
            let mut db = db.lock().expect("db lock");
            if db.compactions() != entry.compactions {
                return Err(compacted());
            }
            let fresh = own && entry.scan.as_ref().is_some_and(|s| s.check(&db).is_ok());
            restored.copies += db.set_deleted_many(chunk, false).map_err(to_job_error)?;
            if let Some(s) = entry.scan.as_mut() {
                s.applied = false;
                if fresh {
                    s.generation = db.generation();
                }
            }
            // Nothing else changes the list meanwhile (busy): the chunk is
            // its start.
            if entry.deleted.starts_with(chunk) {
                entry.deleted.drain(..chunk.len());
            } else {
                entry.deleted.retain(|id| chunk.binary_search(id).is_err());
            }
        }
        done += chunk.len() as u64;
        // Not under the locks: whatever happens there leaves them good.
        progress.report(done, total);
    }
    for chunk in fills.rchunks(CHUNK) {
        progress.check_cancelled().map_err(to_job_error)?;
        {
            let mut all = state.duplicates.lock().expect("duplicates lock");
            let entry = all.get_mut(&handle).ok_or_else(|| JobError::failed("The database was closed"))?;
            let mut db = db.lock().expect("db lock");
            if db.compactions() != entry.compactions {
                return Err(compacted());
            }
            let fresh = own && entry.scan.as_ref().is_some_and(|s| s.check(&db).is_ok());
            let undone = chess_db::unfill(&mut db, chunk).map_err(to_job_error)?;
            restored.unfilled += undone;
            restored.changed += chunk.len() as u32 - undone;
            if let Some(s) = entry.scan.as_mut() {
                s.applied = false;
                if fresh {
                    s.generation = db.generation();
                }
            }
            // The chunk is the list's end, as above.
            if entry.filled.ends_with(chunk) {
                entry.filled.truncate(entry.filled.len() - chunk.len());
            } else {
                entry.filled.retain(|f| !chunk.contains(f));
            }
        }
        done += chunk.len() as u64;
        progress.report(done, total);
    }
    Ok(restored)
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
        deleting(state, handle, scan).map(|d| d.marked)
    }

    fn deleting(state: &AppState, handle: u32, scan: u64) -> Result<Deleted, String> {
        let (db, found) = start_delete(state, handle, scan).map_err(|e| e.message)?;
        delete_job(state, handle, scan, &db, &found, &NoProgress).map_err(|e| e.to_string())
    }

    fn restore(state: &AppState, handle: u32, scan: u64) -> Result<u32, String> {
        restoring(state, handle, scan).map(|r| r.copies)
    }

    fn restoring(state: &AppState, handle: u32, scan: u64) -> Result<Restored, String> {
        let work = start_restore(state, handle, scan).map_err(|e| e.message)?;
        restore_job(state, handle, &work, &NoProgress).map_err(|e| e.to_string())
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
        let (db, found) = start_delete(&state, handle, scan).unwrap();
        assert!(start_delete(&state, handle, scan).unwrap_err().message.contains("wait"), "busy");
        let stop = StopAfter(std::cell::Cell::new(0));
        assert!(matches!(delete_job(&state, handle, scan, &db, &found, &stop), Err(JobError::Cancelled)));
        let now = report(&state, handle).unwrap().unwrap();
        assert_eq!((now.deleted, now.applied, now.stale), (CHUNK as u32, false, false));
        assert_eq!(delete(&state, handle, scan), Ok(10), "the rest");
        assert_eq!(restore(&state, handle, scan), Ok(CHUNK as u32 + 10));
    }

    /// R2-5: a delete or restore that panics (here in its progress report)
    /// still clears the busy mark, and leaves the deleted copies as far as
    /// it got.
    #[test]
    fn a_panic_in_a_delete_or_restore_doesnt_leave_the_database_busy() {
        struct Panics;
        impl Progress for Panics {
            fn report(&self, _done: u64, _total: u64) {
                panic!("a panic in the job");
            }
            fn is_cancelled(&self) -> bool {
                false
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let handle = listed(&state, dir.path(), "duplicates.pgn", 1);
        let scan = search(&state, handle, false);
        let (db, found) = start_delete(&state, handle, scan).unwrap();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            delete_job(&state, handle, scan, &db, &found, &Panics)
        }));
        assert!(panicked.is_err());
        let now = report(&state, handle).unwrap().unwrap();
        assert_eq!((now.busy, now.deleted), (false, 2), "not busy; the two copies it marked restorable");

        let work = start_restore(&state, handle, scan).unwrap();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restore_job(&state, handle, &work, &Panics)
        }));
        assert!(panicked.is_err());
        let now = report(&state, handle).unwrap().unwrap();
        assert_eq!((now.busy, now.deleted), (false, 0), "not busy; the two copies it restored are back");
        assert_eq!(delete(&state, handle, scan), Ok(2), "and it can delete again");
    }

    /// R4-1: deleting the fixture's fuzzy copies first gives the kept game
    /// (round 1) the board of its copy (round 1.3); Restore brings the
    /// copies back, then takes the board out, unless the kept game was
    /// changed since (it then keeps it, and says so).
    #[test]
    fn delete_fills_in_the_kept_game_and_restore_takes_it_out() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let handle = listed(&state, dir.path(), "duplicates.pgn", 1);
        let db = state.db(handle).unwrap();
        let round = |id: GameId| db.lock().unwrap().row(id).unwrap().round;
        let blob = db.lock().unwrap().read_blob(0).unwrap();
        let scan = search(&state, handle, true);
        let done = deleting(&state, handle, scan).unwrap();
        assert_eq!(done, Deleted { marked: 4, filled: 1, left: 0 });
        assert_eq!(
            done.message(),
            "Marked 4 copies as deleted, after filling in what 1 kept game lacked from them"
        );
        assert_eq!(round(0), "1.3");
        let now = report(&state, handle).unwrap().unwrap();
        assert_eq!((now.deleted, now.filled, now.applied, now.stale), (4, 1, true, false));

        let back = restoring(&state, handle, scan).unwrap();
        assert_eq!(back, Restored { copies: 4, unfilled: 1, changed: 0 });
        assert_eq!(
            back.message(),
            "Restored 4 copies, and took out what 1 kept game had filled in from them"
        );
        assert_eq!((round(0), db.lock().unwrap().read_blob(0).unwrap()), ("1".to_owned(), blob));
        let now = report(&state, handle).unwrap().unwrap();
        assert_eq!(
            (now.deleted, now.filled, now.applied, now.stale),
            (0, 0, false, false),
            "the report holds"
        );

        // The kept game edited after the delete: it keeps the board.
        assert_eq!(delete(&state, handle, scan), Ok(4));
        let mut game = db.lock().unwrap().read_game(0).unwrap();
        game.headers.annotator = "Me".into();
        db.lock().unwrap().replace(0, &game).unwrap();
        let back = restoring(&state, handle, scan).unwrap();
        assert_eq!(back, Restored { copies: 4, unfilled: 0, changed: 1 });
        assert_eq!(
            back.message(),
            "Restored 4 copies; 1 kept game was changed since, so it keeps what it took from its copies"
        );
        assert_eq!(round(0), "1.3");
        assert_eq!(report(&state, handle).unwrap().unwrap().filled, 0);
        assert!(restore(&state, handle, scan).unwrap_err().contains("no deleted copies"));
    }

    /// A Stop between restoring the copies and taking out the fills leaves
    /// the fills for the next Restore.
    #[test]
    fn a_restore_stopped_after_the_copies_takes_out_the_fills_next_time() {
        struct StopAfterOne(std::cell::Cell<u32>);
        impl Progress for StopAfterOne {
            fn report(&self, _done: u64, _total: u64) {
                self.0.set(self.0.get() + 1);
            }
            fn is_cancelled(&self) -> bool {
                self.0.get() >= 1
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let handle = listed(&state, dir.path(), "duplicates.pgn", 1);
        let scan = search(&state, handle, true);
        assert_eq!(delete(&state, handle, scan), Ok(4));
        let work = start_restore(&state, handle, scan).unwrap();
        let stop = StopAfterOne(std::cell::Cell::new(0));
        let stopped = restore_job(&state, handle, &work, &stop);
        assert!(matches!(stopped, Err(JobError::Cancelled)));
        let now = report(&state, handle).unwrap().unwrap();
        assert_eq!(
            (now.deleted, now.filled, now.busy),
            (0, 1, false),
            "the copies are back, the board isn't out"
        );
        assert_eq!(restoring(&state, handle, scan), Ok(Restored { copies: 0, unfilled: 1, changed: 0 }));
        assert_eq!(work.db.lock().unwrap().row(0).unwrap().round, "1");
        let restored = Restored { copies: 0, unfilled: 1, changed: 0 }.message();
        assert_eq!(restored, "Took out what 1 kept game had filled in from its copies");
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
