//! Background job manager: runs long operations (imports, exports, sorts,
//! later engine analysis) on dedicated threads with progress reporting and
//! cooperative cancellation.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub type JobId = u64;

/// Minimum interval between progress notifications for one job.
const NOTIFY_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

impl JobState {
    pub fn is_finished(&self) -> bool {
        !matches!(self, JobState::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStatus {
    pub id: JobId,
    /// Machine-readable kind, e.g. `"import"`.
    pub kind: String,
    /// Human-readable description.
    pub label: String,
    pub state: JobState,
    pub done: u64,
    /// 0 when unknown.
    pub total: u64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

impl JobError {
    pub fn failed(e: impl std::fmt::Display) -> JobError {
        JobError::Failed(e.to_string())
    }
}

type Listener = Arc<dyn Fn(&JobStatus) + Send + Sync>;

struct Entry {
    status: JobStatus,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

struct Inner {
    next_id: AtomicU64,
    jobs: Mutex<HashMap<JobId, Entry>>,
    listener: RwLock<Option<Listener>>,
}

impl Inner {
    fn update(&self, id: JobId, f: impl FnOnce(&mut JobStatus)) {
        let snapshot = {
            let mut jobs = self.jobs.lock().expect("jobs lock");
            let Some(entry) = jobs.get_mut(&id) else { return };
            f(&mut entry.status);
            entry.status.clone()
        };
        if let Some(listener) = self.listener.read().expect("listener lock").as_ref() {
            listener(&snapshot);
        }
    }
}

/// Handle passed to a running job.
pub struct JobContext {
    id: JobId,
    inner: Arc<Inner>,
    cancel: Arc<AtomicBool>,
    last_notify: Mutex<Instant>,
}

impl JobContext {
    pub fn id(&self) -> JobId {
        self.id
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Returns `Err(Cancelled)` if cancellation was requested.
    pub fn check_cancelled(&self) -> Result<(), JobError> {
        if self.is_cancelled() { Err(JobError::Cancelled) } else { Ok(()) }
    }

    /// Reports progress; notifications are throttled.
    pub fn progress(&self, done: u64, total: u64) {
        self.progress_with(done, total, None);
    }

    pub fn progress_msg(&self, done: u64, total: u64, message: impl Into<String>) {
        self.progress_with(done, total, Some(message.into()));
    }

    fn progress_with(&self, done: u64, total: u64, message: Option<String>) {
        let mut last = self.last_notify.lock().expect("notify lock");
        let now = Instant::now();
        let due = now.duration_since(*last) >= NOTIFY_INTERVAL;
        if due {
            *last = now;
        }
        drop(last);
        if due {
            self.inner.update(self.id, |s| {
                s.done = done;
                s.total = total;
                if let Some(m) = message {
                    s.message = m;
                }
            });
        } else if let Ok(mut jobs) = self.inner.jobs.lock() {
            // Keep the stored status current even when not notifying.
            if let Some(entry) = jobs.get_mut(&self.id) {
                entry.status.done = done;
                entry.status.total = total;
                if let Some(m) = message {
                    entry.status.message = m;
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct JobManager {
    inner: Arc<Inner>,
}

impl Default for JobManager {
    fn default() -> Self {
        JobManager::new()
    }
}

impl JobManager {
    pub fn new() -> JobManager {
        JobManager {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(1),
                jobs: Mutex::new(HashMap::new()),
                listener: RwLock::new(None),
            }),
        }
    }

    /// Called on every status change (throttled for progress updates).
    pub fn set_listener(&self, listener: impl Fn(&JobStatus) + Send + Sync + 'static) {
        *self.inner.listener.write().expect("listener lock") = Some(Arc::new(listener));
    }

    /// Runs `work` on a new thread. It returns a completion message.
    pub fn spawn<F>(&self, kind: &str, label: impl Into<String>, work: F) -> JobId
    where
        F: FnOnce(&JobContext) -> Result<String, JobError> + Send + 'static,
    {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let status = JobStatus {
            id,
            kind: kind.to_owned(),
            label: label.into(),
            state: JobState::Running,
            done: 0,
            total: 0,
            message: String::new(),
        };
        self.inner
            .jobs
            .lock()
            .expect("jobs lock")
            .insert(id, Entry { status, cancel: cancel.clone(), handle: None });
        self.inner.update(id, |_| {});

        let ctx = JobContext {
            id,
            inner: self.inner.clone(),
            cancel,
            last_notify: Mutex::new(Instant::now() - NOTIFY_INTERVAL),
        };
        let inner = self.inner.clone();
        let handle = std::thread::Builder::new()
            .name(format!("job-{id}-{kind}"))
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| work(&ctx)))
                    .unwrap_or_else(|_| Err(JobError::Failed("job panicked".into())));
                inner.update(id, |s| match result {
                    Ok(message) => {
                        s.state = JobState::Done;
                        s.message = message;
                    }
                    Err(JobError::Cancelled) => s.state = JobState::Cancelled,
                    Err(JobError::Failed(e)) => {
                        log::warn!("job {id} ({}) failed: {e}", s.kind);
                        s.state = JobState::Failed(e);
                    }
                });
            })
            .expect("spawn job thread");
        if let Some(entry) = self.inner.jobs.lock().expect("jobs lock").get_mut(&id) {
            entry.handle = Some(handle);
        }
        id
    }

    /// Requests cancellation. Returns false if the job is unknown or done.
    pub fn cancel(&self, id: JobId) -> bool {
        let jobs = self.inner.jobs.lock().expect("jobs lock");
        match jobs.get(&id) {
            Some(e) if !e.status.state.is_finished() => {
                e.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    pub fn status(&self, id: JobId) -> Option<JobStatus> {
        self.inner.jobs.lock().expect("jobs lock").get(&id).map(|e| e.status.clone())
    }

    pub fn list(&self) -> Vec<JobStatus> {
        let mut out: Vec<_> =
            self.inner.jobs.lock().expect("jobs lock").values().map(|e| e.status.clone()).collect();
        out.sort_by_key(|s| s.id);
        out
    }

    /// Forgets finished jobs.
    pub fn clear_finished(&self) {
        self.inner.jobs.lock().expect("jobs lock").retain(|_, e| !e.status.state.is_finished());
    }

    /// Blocks until the job finishes and returns its final status.
    pub fn wait(&self, id: JobId) -> Option<JobStatus> {
        let handle = self.inner.jobs.lock().expect("jobs lock").get_mut(&id)?.handle.take();
        if let Some(h) = handle {
            let _ = h.join();
        }
        self.status(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_job_and_reports_completion() {
        let jobs = JobManager::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        jobs.set_listener(move |st| s.lock().unwrap().push(st.state.clone()));
        let id = jobs.spawn("test", "Counting", |ctx| {
            for i in 0..10 {
                ctx.progress(i, 10);
            }
            Ok("counted".into())
        });
        let status = jobs.wait(id).unwrap();
        assert_eq!(status.state, JobState::Done);
        assert_eq!(status.message, "counted");
        assert_eq!(status.done, 9);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.first(), Some(&JobState::Running));
        assert_eq!(seen.last(), Some(&JobState::Done));
    }

    #[test]
    fn cancellation_is_cooperative() {
        let jobs = JobManager::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let id = jobs.spawn("test", "Spinning", move |ctx| {
            tx.send(()).unwrap();
            loop {
                ctx.check_cancelled()?;
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        rx.recv().unwrap();
        assert!(jobs.cancel(id));
        assert_eq!(jobs.wait(id).unwrap().state, JobState::Cancelled);
        assert!(!jobs.cancel(id), "finished jobs can't be cancelled");
    }

    #[test]
    fn failures_and_panics_are_reported() {
        let jobs = JobManager::new();
        let a = jobs.spawn("test", "fails", |_| Err(JobError::failed("disk full")));
        let b = jobs.spawn("test", "panics", |_| panic!("boom"));
        assert_eq!(jobs.wait(a).unwrap().state, JobState::Failed("disk full".into()));
        assert!(matches!(jobs.wait(b).unwrap().state, JobState::Failed(_)));
        assert_eq!(jobs.list().len(), 2);
        jobs.clear_finished();
        assert!(jobs.list().is_empty());
    }
}
