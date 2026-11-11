//! Long work (imports, position index builds, counting lines) as
//! `chess-jobs` jobs, each noted with the database it works on and the
//! user who started it.

use std::collections::HashMap;
use std::sync::Mutex;

use chess_jobs::{JobContext, JobError, JobId, JobManager, JobStatus};

use crate::store::{User, now};
use crate::wire::ImportReportDto;

/// Finished jobs are forgotten once there are this many jobs.
const KEEP_JOBS: usize = 1000;

#[derive(Debug, Clone)]
pub struct JobMeta {
    pub db: String,
    pub db_name: String,
    pub user: i64,
    pub username: String,
    pub started_at: i64,
}

#[derive(Default)]
pub struct Jobs {
    pub manager: JobManager,
    meta: Mutex<HashMap<JobId, JobMeta>>,
    /// Kept apart from `meta`: a job can finish before `spawn` returns.
    reports: Mutex<HashMap<JobId, ImportReportDto>>,
}

impl Jobs {
    pub fn spawn<F>(&self, kind: &str, label: String, db: (&str, &str), user: &User, work: F) -> JobId
    where
        F: FnOnce(&JobContext) -> Result<String, JobError> + Send + 'static,
    {
        self.prune();
        let meta = JobMeta {
            db: db.0.to_owned(),
            db_name: db.1.to_owned(),
            user: user.id,
            username: user.username.clone(),
            started_at: now(),
        };
        // Held while it starts, so `stop_all_for` (which lists jobs under
        // this lock) can't miss a job starting at that moment.
        let mut metas = self.meta.lock().expect("jobs lock");
        let id = self.manager.spawn(kind, label, work);
        metas.insert(id, meta);
        id
    }

    fn prune(&self) {
        if self.manager.list().len() < KEEP_JOBS {
            return;
        }
        self.manager.clear_finished();
        let live: Vec<JobId> = self.manager.list().iter().map(|s| s.id).collect();
        self.meta.lock().expect("jobs lock").retain(|id, _| live.contains(id));
        self.reports.lock().expect("jobs lock").retain(|id, _| live.contains(id));
    }

    pub fn info(&self, id: JobId) -> Option<(JobStatus, JobMeta)> {
        let status = self.manager.status(id)?;
        let meta = self.meta.lock().expect("jobs lock").get(&id).cloned()?;
        Some((status, meta))
    }

    pub fn set_report(&self, id: JobId, report: ImportReportDto) {
        self.reports.lock().expect("jobs lock").insert(id, report);
    }

    pub fn report(&self, id: JobId) -> Option<ImportReportDto> {
        self.reports.lock().expect("jobs lock").get(&id).cloned()
    }

    /// Every job known, oldest first.
    pub fn all(&self) -> Vec<(JobStatus, JobMeta)> {
        let meta = self.meta.lock().expect("jobs lock");
        self.manager.list().into_iter().filter_map(|s| meta.get(&s.id).cloned().map(|m| (s, m))).collect()
    }

    /// Cancels the jobs working on database `db` and waits for them to end
    /// (blocking).
    pub fn stop_all_for(&self, db: &str) {
        let ids: Vec<JobId> = self
            .all()
            .into_iter()
            .filter(|(s, m)| m.db == db && !s.state.is_finished())
            .map(|(s, _)| s.id)
            .collect();
        for &id in &ids {
            self.manager.cancel(id);
        }
        for id in ids {
            self.manager.wait(id);
        }
    }

    /// Cancels every running job and waits for them (at shutdown).
    pub fn stop_all(&self) {
        let ids: Vec<JobId> =
            self.manager.list().iter().filter(|s| !s.state.is_finished()).map(|s| s.id).collect();
        for &id in &ids {
            self.manager.cancel(id);
        }
        for id in ids {
            self.manager.wait(id);
        }
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
    use crate::wire::Role;

    #[test]
    fn jobs_remember_their_database_and_user() {
        let jobs = Jobs::default();
        let user = User {
            id: 7,
            username: "u".into(),
            role: Role::User,
            disabled: false,
            quota_bytes: None,
            created_at: 0,
        };
        let id = jobs.spawn("test", "Testing".into(), ("db", "Name"), &user, |ctx| {
            ctx.progress(1, 2);
            Ok("done".into())
        });
        jobs.manager.wait(id);
        let (status, meta) = jobs.info(id).unwrap();
        assert_eq!(status.message, "done");
        assert_eq!((meta.db.as_str(), meta.user), ("db", 7));
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(12), "12");
    }
}
