//! Long work (imports, position index builds, counting lines, duplicates,
//! compaction, backups, TWIC updates) as `chess-jobs` jobs, each noted with
//! the database it works on and the user who started it.
//!
//! Jobs are kept in the metadata store as well (NET-03): a row is written
//! before a job starts, its progress every two seconds, and its end with
//! its report, so they survive a restart. A job the server stopped in (it
//! crashed, or it was shut down) is marked interrupted at the next start,
//! except imports, which go on from what they committed (`resume`, in
//! `routes/imports.rs`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chess_db::Duplicates;
use chess_jobs::{JobContext, JobError, JobId, JobManager, JobState, JobStatus};
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::store::{JobMetaRow, JobRow, Store, User, now};
use crate::wire::{DuplicatesReportDto, ImportReportDto, JobDto, JobStateDto};

/// Finished jobs are forgotten (in memory) once there are this many jobs.
const KEEP_JOBS: usize = 1000;
/// How often a running job's progress is written to the store.
const SAVE_EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobMeta {
    pub db: String,
    pub db_name: String,
    pub user: i64,
    pub username: String,
    pub started_at: i64,
}

/// What a search for duplicates (DB-08) found, kept for deleting the copies.
#[derive(Debug, Clone)]
pub struct DuplicatesFound {
    pub db: String,
    pub fuzzy: bool,
    /// The database's id epoch the ids are of.
    pub id_epoch: u64,
    pub found: Arc<Duplicates>,
}

impl DuplicatesFound {
    pub fn report(&self) -> DuplicatesReportDto {
        let f = &self.found;
        DuplicatesReportDto {
            fuzzy: self.fuzzy,
            groups: f.groups.len() as u32,
            copies: f.removable() as u32,
            exact_groups: f.groups.iter().filter(|g| g.exact).count() as u32,
            games: f.games,
            compared: f.compared,
            id_epoch: self.id_epoch,
        }
    }
}

/// A job's report as the store keeps it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import: Option<ImportReportDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duplicates: Option<DuplicatesReportDto>,
}

struct Inner {
    store: Arc<Store>,
    meta: Mutex<HashMap<JobId, JobMeta>>,
    /// Kept apart from `meta`: a job can finish before `spawn` returns.
    reports: Mutex<HashMap<JobId, ImportReportDto>>,
    duplicates: Mutex<HashMap<JobId, DuplicatesFound>>,
    /// When each running job's progress was last written.
    saved: Mutex<HashMap<JobId, Instant>>,
    /// Imports that keep "running" in the store when the server stops, to go
    /// on at the next start (an import that has `resume`).
    resumable: Mutex<std::collections::HashSet<JobId>>,
    /// The server is stopping: jobs cancelled now were interrupted.
    stopping: Arc<AtomicBool>,
}

impl Inner {
    fn persist(&self, s: &JobStatus) {
        let result = if s.state.is_finished() {
            self.saved.lock().expect("jobs lock").remove(&s.id);
            let stopping = self.stopping.load(Ordering::Acquire);
            let resumable = self.resumable.lock().expect("jobs lock").remove(&s.id);
            if stopping && resumable && s.state == JobState::Cancelled {
                // Left running: it goes on at the next start.
                return;
            }
            let (state, error) = match (&s.state, stopping) {
                (JobState::Cancelled, true) => ("interrupted", Some("The server stopped while it ran")),
                (JobState::Done, _) => ("done", None),
                (JobState::Failed(e), _) => ("failed", Some(e.as_str())),
                (JobState::Cancelled, false) => ("cancelled", None),
                (JobState::Running, _) => return,
            };
            let report = StoredReport {
                import: self.reports.lock().expect("jobs lock").get(&s.id).cloned(),
                duplicates: self
                    .duplicates
                    .lock()
                    .expect("jobs lock")
                    .get(&s.id)
                    .map(DuplicatesFound::report),
            };
            let report = (report.import.is_some() || report.duplicates.is_some())
                .then(|| serde_json::to_string(&report).unwrap_or_default());
            self.store.finish_job(s.id, state, error, &s.message, s.done, s.total, report.as_deref())
        } else {
            let mut saved = self.saved.lock().expect("jobs lock");
            let due = saved.get(&s.id).is_none_or(|t| t.elapsed() >= SAVE_EVERY);
            if !due {
                return;
            }
            saved.insert(s.id, Instant::now());
            drop(saved);
            self.store.job_progress(s.id, s.done, s.total, &s.message)
        };
        if let Err(e) = result {
            log::warn!("job {}: couldn't keep its state: {}", s.id, e.message);
        }
    }
}

pub struct Jobs {
    pub manager: JobManager,
    inner: Arc<Inner>,
}

fn state_of(row: &JobRow) -> JobStateDto {
    match row.state.as_str() {
        "running" => JobStateDto::Running,
        "done" => JobStateDto::Done,
        "failed" => JobStateDto::Failed,
        "cancelled" => JobStateDto::Cancelled,
        _ => JobStateDto::Interrupted,
    }
}

fn dto_of(row: &JobRow) -> JobDto {
    JobDto {
        id: row.id,
        kind: row.kind.clone(),
        label: row.label.clone(),
        state: state_of(row),
        done: row.done,
        total: row.total,
        message: row.message.clone(),
        error: row.error.clone(),
    }
}

fn meta_of(row: &JobRow) -> JobMeta {
    JobMeta {
        db: row.db.clone().unwrap_or_default(),
        db_name: row.db_name.clone(),
        user: row.user.unwrap_or(0),
        username: row.username.clone(),
        started_at: row.started_at,
    }
}

impl Jobs {
    /// Jobs kept in `store`; new ones are numbered after those it has.
    pub fn new(store: Arc<Store>) -> ApiResult<Jobs> {
        let manager = JobManager::new();
        manager.reserve_until(store.max_job_id()? + 1);
        let inner = Arc::new(Inner {
            store,
            meta: Mutex::default(),
            reports: Mutex::default(),
            duplicates: Mutex::default(),
            saved: Mutex::default(),
            resumable: Mutex::default(),
            stopping: Arc::default(),
        });
        let i = inner.clone();
        manager.set_listener(move |s| i.persist(s));
        Ok(Jobs { manager, inner })
    }

    /// Set when the server stops: an upload kept for an import to go on
    /// with after a restart isn't deleted then.
    pub fn stopping(&self) -> Arc<AtomicBool> {
        self.inner.stopping.clone()
    }

    pub fn spawn<F>(&self, kind: &str, label: String, db: (&str, &str), user: &User, work: F) -> JobId
    where
        F: FnOnce(&JobContext) -> Result<String, JobError> + Send + 'static,
    {
        let meta = JobMeta {
            db: db.0.to_owned(),
            db_name: db.1.to_owned(),
            user: user.id,
            username: user.username.clone(),
            started_at: now(),
        };
        let id = self.manager.next_id();
        if let Err(e) = self.inner.store.insert_job(id, kind, &label, &row_meta(&meta)) {
            log::warn!("job {id}: couldn't keep it: {}", e.message);
        }
        self.run(id, kind, label, meta, work)
    }

    /// Runs a job the store kept (an import going on after a restart) under
    /// its id.
    pub fn spawn_kept<F>(&self, row: &JobRow, work: F) -> JobId
    where
        F: FnOnce(&JobContext) -> Result<String, JobError> + Send + 'static,
    {
        self.run(row.id, &row.kind, row.label.clone(), meta_of(row), work)
    }

    fn run<F>(&self, id: JobId, kind: &str, label: String, meta: JobMeta, work: F) -> JobId
    where
        F: FnOnce(&JobContext) -> Result<String, JobError> + Send + 'static,
    {
        self.prune();
        // Known before it runs (it can end before `spawn_as` returns), and
        // the lock held while it starts, so `stop_all_for` (which lists jobs
        // under this lock) can't miss a job starting at that moment.
        let mut metas = self.inner.meta.lock().expect("jobs lock");
        metas.insert(id, meta);
        self.manager.spawn_as(id, kind, label, work)
    }

    /// Notes how an import goes on if the server stops (JSON, see
    /// `routes/imports.rs`); from then on the import is left running in the
    /// store when the server stops.
    pub fn set_resume(&self, id: JobId, resume: &str) {
        self.inner.resumable.lock().expect("jobs lock").insert(id);
        if let Err(e) = self.inner.store.job_resume(id, resume) {
            log::warn!("job {id}: couldn't note how to resume it: {}", e.message);
        }
    }

    fn prune(&self) {
        if self.manager.list().len() < KEEP_JOBS {
            return;
        }
        self.manager.clear_finished();
        let live: Vec<JobId> = self.manager.list().iter().map(|s| s.id).collect();
        let i = &self.inner;
        i.meta.lock().expect("jobs lock").retain(|id, _| live.contains(id));
        i.reports.lock().expect("jobs lock").retain(|id, _| live.contains(id));
        i.duplicates.lock().expect("jobs lock").retain(|id, _| live.contains(id));
    }

    pub fn set_duplicates(&self, id: JobId, found: DuplicatesFound) {
        self.inner.duplicates.lock().expect("jobs lock").insert(id, found);
    }

    /// What a search for duplicates found, while the server has it in memory.
    pub fn duplicates(&self, id: JobId) -> Option<DuplicatesFound> {
        self.inner.duplicates.lock().expect("jobs lock").get(&id).cloned()
    }

    /// A job: in memory if it ran since the start, else as the store kept it.
    pub fn info(&self, id: JobId) -> Option<(JobDto, JobMeta)> {
        if let Some(status) = self.manager.status(id)
            && let Some(meta) = self.inner.meta.lock().expect("jobs lock").get(&id).cloned()
        {
            return Some((JobDto::from(&status), meta));
        }
        let row = self.inner.store.job(id).ok()??;
        Some((dto_of(&row), meta_of(&row)))
    }

    fn stored_report(&self, id: JobId) -> Option<StoredReport> {
        let row = self.inner.store.job(id).ok()??;
        serde_json::from_str(row.report.as_deref()?).ok()
    }

    pub fn set_report(&self, id: JobId, report: ImportReportDto) {
        self.inner.reports.lock().expect("jobs lock").insert(id, report);
    }

    /// An import's report.
    pub fn report(&self, id: JobId) -> Option<ImportReportDto> {
        if let Some(r) = self.inner.reports.lock().expect("jobs lock").get(&id).cloned() {
            return Some(r);
        }
        self.stored_report(id)?.import
    }

    /// A search for duplicates' report.
    pub fn duplicates_report(&self, id: JobId) -> Option<DuplicatesReportDto> {
        if let Some(d) = self.duplicates(id) {
            return Some(d.report());
        }
        self.stored_report(id)?.duplicates
    }

    /// The jobs since the start and the newest the store kept, newest first.
    pub fn all(&self) -> Vec<(JobDto, JobMeta)> {
        let mut out: Vec<(JobDto, JobMeta)> = {
            let meta = self.inner.meta.lock().expect("jobs lock");
            self.manager
                .list()
                .into_iter()
                .filter_map(|s| meta.get(&s.id).cloned().map(|m| (JobDto::from(&s), m)))
                .collect()
        };
        if let Ok(rows) = self.inner.store.recent_jobs(200) {
            for row in rows {
                if !out.iter().any(|(j, _)| j.id == row.id) {
                    out.push((dto_of(&row), meta_of(&row)));
                }
            }
        }
        out.sort_by_key(|(j, _)| std::cmp::Reverse(j.id));
        out
    }

    /// The jobs running now (in memory).
    fn running(&self, keep: impl Fn(&JobMeta) -> bool) -> Vec<JobId> {
        let meta = self.inner.meta.lock().expect("jobs lock");
        self.manager
            .list()
            .iter()
            .filter(|s| !s.state.is_finished() && meta.get(&s.id).is_some_and(&keep))
            .map(|s| s.id)
            .collect()
    }

    fn stop(&self, ids: Vec<JobId>) {
        for &id in &ids {
            self.manager.cancel(id);
        }
        for id in ids {
            self.manager.wait(id);
        }
    }

    /// Cancels the jobs user `user` started and waits for them to end
    /// (blocking).
    pub fn stop_all_by(&self, user: i64) {
        self.stop(self.running(|m| m.user == user));
    }

    /// Cancels the jobs working on database `db` and waits for them to end
    /// (blocking).
    pub fn stop_all_for(&self, db: &str) {
        self.stop(self.running(|m| m.db == db));
    }

    /// Stops every running job and waits for them, at shutdown: imports
    /// stop between two batches and go on at the next start; the others
    /// are marked interrupted.
    pub fn stop_all(&self) {
        self.inner.stopping.store(true, Ordering::Release);
        self.stop(self.running(|_| true));
    }

    /// Jobs a stop interrupted, as the store kept them.
    pub fn interrupted(&self) -> Vec<JobRow> {
        self.inner.store.running_jobs().unwrap_or_default()
    }

    /// Marks a kept job interrupted (it can't go on).
    pub fn mark_interrupted(&self, row: &JobRow, why: &str) {
        let r = self.inner.store.finish_job(
            row.id,
            "interrupted",
            Some(why),
            &row.message,
            row.done,
            row.total,
            None,
        );
        if let Err(e) = r {
            log::warn!("job {}: {}", row.id, e.message);
        }
    }

    /// Notes the meta of a kept job resumed under a new run (the same id).
    pub fn note_resumable(&self, id: JobId) {
        self.inner.resumable.lock().expect("jobs lock").insert(id);
    }
}

fn row_meta(m: &JobMeta) -> JobMetaRow {
    JobMetaRow {
        db: m.db.clone(),
        db_name: m.db_name.clone(),
        user: m.user,
        username: m.username.clone(),
        started_at: m.started_at,
    }
}

pub fn to_job_error(e: chess_db::DbError) -> JobError {
    match e {
        chess_db::DbError::Cancelled => JobError::Cancelled,
        other => JobError::failed(other),
    }
}

/// `1,234,567`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Role, Visibility};

    #[test]
    fn jobs_are_kept_with_their_database_and_user() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("s.sqlite")).unwrap());
        let user = store.create_user("u", "h", Role::User).unwrap();
        store.create_database("db", "Name", user.id, Visibility::Private).unwrap();
        let jobs = Jobs::new(store.clone()).unwrap();
        let id = jobs.spawn("test", "Testing".into(), ("db", "Name"), &user, |ctx| {
            ctx.progress(1, 2);
            Ok("done".into())
        });
        jobs.manager.wait(id);
        let (status, meta) = jobs.info(id).unwrap();
        assert_eq!(status.message, "done");
        assert_eq!((meta.db.as_str(), meta.user), ("db", user.id));
        let row = store.job(id).unwrap().unwrap();
        assert_eq!((row.state.as_str(), row.message.as_str()), ("done", "done"));

        // Another start: numbered after, the old one read from the store.
        let again = Jobs::new(store.clone()).unwrap();
        let (dto, meta) = again.info(id).unwrap();
        assert_eq!((dto.state, meta.username.as_str()), (JobStateDto::Done, "u"));
        let next = again.spawn("test", "Again".into(), ("db", "Name"), &user, |_| Ok(String::new()));
        assert!(next > id);
        again.manager.wait(next);
        assert_eq!(again.all().len(), 2);
        // A job stopped at shutdown is interrupted.
        let slow = again.spawn("test", "Slow".into(), ("db", "Name"), &user, move |ctx| {
            for _ in 0..500 {
                ctx.check_cancelled()?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(String::new())
        });
        again.stop_all();
        assert_eq!(store.job(slow).unwrap().unwrap().state, "interrupted");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(12), "12");
    }
}
