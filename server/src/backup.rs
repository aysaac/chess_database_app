//! Online backups (NET-03): a snapshot of the metadata store and every
//! database, written to `CHESS_SERVER_BACKUP_DIR` while the server runs, as
//! `chess-server-<date>-<time>/` (`.partial` until it's complete), when the
//! admin asks or on a schedule; the newest `CHESS_SERVER_BACKUP_KEEP` are
//! kept.
//!
//! - The store is copied with SQLite's online backup API.
//! - Each database is copied holding its writer slot, so no import, index
//!   build or compaction runs meanwhile and no batch is committing; its
//!   `games.idx` (with the committed count) is copied first and its
//!   append-only files after it, so the copy never refers to data it
//!   lacks (`docs/file-format.md`, Crash safety).
//!
//! Backups are never downloadable through the API: they hold every user's
//! databases, private ones too, which the admin can't read. Restoring is
//! done on the server's disk (`docs/server.md`, Backups).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chess_jobs::{JobContext, JobError};

use crate::AppState;
use crate::audit::Target;
use crate::error::{ApiError, ApiResult};
use crate::pool::{dir_size, writer_busy};
use crate::store::User;
use crate::wire::BackupDto;

const PREFIX: &str = "chess-server-";
const PARTIAL: &str = ".partial";
/// The files of a database copied before the others, in this order.
const FIRST: [&str; 6] =
    ["db.meta", "games.idx", "players.names", "events.names", "sites.names", "annotators.names"];

/// `chess-server-20261111-153000`, from Unix seconds (UTC).
fn name_at(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{PREFIX}{year:04}{month:02}{day:02}-{:02}{:02}{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// The complete backups in `dir`, oldest first.
fn complete(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && !n.ends_with(PARTIAL))
        })
        .collect();
    out.sort();
    out
}

/// Bytes under `dir`, its folders included.
fn tree_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.filter_map(|e| e.ok())
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => tree_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// The backups there are, newest first (metadata only).
pub fn list(state: &AppState) -> Vec<BackupDto> {
    let Some(dir) = &state.config.backup_dir else { return Vec::new() };
    complete(dir)
        .into_iter()
        .rev()
        .map(|p| {
            let created_at = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            let databases = std::fs::read_dir(p.join("dbs")).map_or(0, |rd| rd.count() as u32);
            BackupDto {
                name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                size_bytes: tree_size(&p),
                databases,
                created_at,
            }
        })
        .collect()
}

/// When the newest backup was made, if there is one.
pub fn last_made(state: &AppState) -> Option<SystemTime> {
    let dir = state.config.backup_dir.as_ref()?;
    complete(dir).last().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok())
}

/// Starts a backup job, unless one runs. `by` is who asked (none: the
/// schedule).
pub fn start(state: &Arc<AppState>, by: Option<&User>, client: Option<&str>) -> ApiResult<u64> {
    let Some(dir) = state.config.backup_dir.clone() else {
        return Err(ApiError::conflict("No backup folder is set: set CHESS_SERVER_BACKUP_DIR"));
    };
    if state.jobs.manager.list().iter().any(|s| s.kind == "backup" && !s.state.is_finished()) {
        return Err(ApiError::conflict("A backup runs already"));
    }
    let system = User {
        id: 0,
        username: "(schedule)".into(),
        role: crate::wire::Role::Admin,
        disabled: false,
        quota_bytes: None,
        created_at: 0,
    };
    let user = by.unwrap_or(&system);
    let st = state.clone();
    let job = state.jobs.spawn("backup", "Backing up the server".into(), ("", ""), user, move |ctx| {
        let name = run(&st, &dir, ctx)?;
        st.audit("backup", None, Target::Backup(&name), Some("made".into()), None);
        Ok(format!("Made backup {name}"))
    });
    state.audit("backup", by, Target::Job(job), Some("started".into()), client);
    Ok(job)
}

/// Waits for the writer slot of `open` to be free and takes it for `job`;
/// gives it back when dropped.
struct Slot<'a> {
    open: &'a crate::pool::OpenDb,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self.open.writer.lock().expect("writer lock") = None;
    }
}

fn take_slot<'a>(
    state: &AppState,
    open: &'a crate::pool::OpenDb,
    ctx: &JobContext,
) -> Result<Slot<'a>, JobError> {
    loop {
        {
            let mut writer = open.writer.lock().expect("writer lock");
            if !writer_busy(&writer, &state.jobs.manager) && !open.is_compacting() {
                *writer = Some(ctx.id());
                return Ok(Slot { open });
            }
        }
        ctx.check_cancelled()?;
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn copy_db(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    let mut names: Vec<String> = std::fs::read_dir(from)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| n != ".lock" && !n.ends_with(".tmp"))
        .collect();
    // The index (its committed count) and the name tables first: the
    // games' blobs and positions copied after hold all they refer to.
    names.sort_by_key(|n| (FIRST.iter().position(|f| f == n).unwrap_or(FIRST.len()), n.clone()));
    for n in names {
        std::fs::copy(from.join(&n), to.join(&n))?;
    }
    Ok(())
}

fn run(state: &AppState, dir: &Path, ctx: &JobContext) -> Result<String, JobError> {
    let fail = |e: &dyn std::fmt::Display| JobError::failed(format!("backup: {e}"));
    std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    let base = name_at(crate::store::now());
    // Two in one second (a scheduled one and the admin's) get their own names.
    let name = (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .find(|n| !dir.join(n).exists() && !dir.join(format!("{n}{PARTIAL}")).exists())
        .expect("a free name");
    let partial = dir.join(format!("{name}{PARTIAL}"));
    let result = (|| -> Result<(), JobError> {
        std::fs::create_dir_all(partial.join("dbs")).map_err(|e| fail(&e))?;
        let store = partial.join("server.sqlite");
        state.store.backup_to(&store).map_err(|e| fail(&e.message))?;
        let dbs = state.store.all_databases().map_err(|e| fail(&e.message))?;
        let mut gone = Vec::new();
        for (i, meta) in dbs.iter().enumerate() {
            ctx.check_cancelled()?;
            ctx.progress_msg(i as u64, dbs.len() as u64, format!("Database {} of {}", i + 1, dbs.len()));
            let to = partial.join("dbs").join(format!("{}.{}", meta.id, chess_db::EXTENSION));
            // Opened for the copy (it may have closed while idle), its slot held.
            let open = match state.pool.get(&meta.id) {
                Ok(open) => open,
                Err(e) => {
                    // Deleted since the store was copied: left out of it too.
                    log::warn!("backup: database {} left out: {e}", meta.id);
                    gone.push(meta.id.clone());
                    continue;
                }
            };
            let _slot = take_slot(state, &open, ctx)?;
            copy_db(&state.pool.path(&meta.id), &to).map_err(|e| fail(&e))?;
        }
        if !gone.is_empty() {
            let copy = crate::store::Store::open(&store).map_err(|e| fail(&e))?;
            for id in &gone {
                copy.delete_database(id).map_err(|e| fail(&e.message))?;
            }
        }
        // Nothing of this server's sessions goes into a backup.
        let copy = rusqlite::Connection::open(&store).map_err(|e| fail(&e))?;
        copy.execute_batch("DELETE FROM sessions; PRAGMA wal_checkpoint(TRUNCATE);").map_err(|e| fail(&e))?;
        drop(copy);
        for suffix in ["-wal", "-shm"] {
            let mut p = store.clone().into_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    let done = dir.join(&name);
    std::fs::rename(&partial, &done).map_err(|e| fail(&e))?;
    // The oldest go, beyond the number kept.
    let all = complete(dir);
    let keep = state.config.backup_keep.max(1) as usize;
    for old in all.iter().take(all.len().saturating_sub(keep)) {
        log::info!("deleting old backup {}", old.display());
        let _ = std::fs::remove_dir_all(old);
    }
    log::info!("made backup {} ({} MB)", done.display(), dir_size(&done) / 1_000_000);
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_dates() {
        assert_eq!(name_at(0), "chess-server-19700101-000000");
        assert_eq!(name_at(1_794_330_000), "chess-server-20261110-170000");
        assert_eq!(name_at(951_782_400), "chess-server-20000229-000000");
    }
}
