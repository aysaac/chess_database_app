use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use chess_core::game::GameEditor;
use chess_db::{Database, GameId, SortSpec, sorted_subset};
use chess_jobs::JobManager;

use crate::dto::ImportReportDto;
use crate::engines::Analysis;
use crate::error::{AppError, AppResult};
use crate::settings::Settings;

pub type SharedDb = Arc<Mutex<Database>>;

pub struct OpenDb {
    pub db: SharedDb,
    pub path: PathBuf,
    /// Shown in the game list (until `close_db`).
    pub listed: bool,
    /// The reference database (DB-13), until another one is chosen.
    pub reference: bool,
}

/// What a database is open for. It stays open while anything uses it: the
/// reference database is often the one in the list too, and a folder can
/// only be opened once (it's locked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbUse {
    List,
    Reference,
}

impl OpenDb {
    fn flag(&mut self, usage: DbUse) -> &mut bool {
        match usage {
            DbUse::List => &mut self.listed,
            DbUse::Reference => &mut self.reference,
        }
    }
}

/// Search results the game list is limited to (SRCH-14).
pub struct ActiveFilter {
    /// Ascending game ids.
    pub ids: Arc<Vec<GameId>>,
    pub description: String,
    /// The ids in each sort order used so far, valid for `epoch`.
    orders: HashMap<SortSpec, Arc<Vec<GameId>>>,
    epoch: u64,
}

impl ActiveFilter {
    pub fn new(ids: Vec<GameId>, description: String) -> ActiveFilter {
        ActiveFilter { ids: Arc::new(ids), description, orders: HashMap::new(), epoch: 0 }
    }

    /// The filtered ids in `spec` order (fails with `NotSorted` like the
    /// full list when the sort order isn't built).
    pub fn ordered(&mut self, db: &Database, spec: SortSpec) -> Result<Arc<Vec<GameId>>, chess_db::DbError> {
        if self.epoch != db.sort_epoch() {
            self.orders.clear();
            self.epoch = db.sort_epoch();
        }
        if let Some(order) = self.orders.get(&spec) {
            return Ok(order.clone());
        }
        let order = Arc::new(sorted_subset(db, spec, &self.ids)?);
        self.orders.insert(spec, order.clone());
        Ok(order)
    }
}

/// An editing session for one game shown in the UI.
pub struct Session {
    pub editor: GameEditor,
    /// Database handle and game id the game was loaded from / saved to.
    pub source: Option<(u32, GameId)>,
    /// Editor revision whose notation the frontend already has.
    pub sent_revision: Option<u64>,
}

impl Session {
    pub fn new(editor: GameEditor, source: Option<(u32, GameId)>) -> Session {
        Session { editor, source, sent_revision: None }
    }
}

/// Lock order: `sessions` before `analysis`; `settings` is never held
/// while taking another lock.
#[derive(Default)]
pub struct AppState {
    dbs: Mutex<HashMap<u32, OpenDb>>,
    /// Databases closed while a job (the position index, a compaction)
    /// still held them: opening the folder again reuses the same
    /// `Database` instead of failing on its lock.
    released: Mutex<Vec<(PathBuf, Weak<Mutex<Database>>)>>,
    next_db: AtomicU32,
    /// Lock order: `filters` before a database.
    pub filters: Mutex<HashMap<u32, ActiveFilter>>,
    sessions: Mutex<HashMap<u32, Session>>,
    next_session: AtomicU32,
    pub jobs: JobManager,
    pub settings: Mutex<Settings>,
    config_dir: OnceLock<PathBuf>,
    pub analysis: Mutex<Option<Analysis>>,
    /// The last import into each database (by handle), while it had warnings.
    pub import_reports: Mutex<HashMap<u32, ImportReportDto>>,
}

impl AppState {
    /// Loads the settings from `dir` (called once at start-up).
    pub fn init_settings(&self, dir: PathBuf) {
        *self.settings.lock().expect("settings lock") = Settings::load(&dir);
        let _ = self.config_dir.set(dir);
    }

    /// Changes the settings and saves them.
    pub fn update_settings<T>(&self, f: impl FnOnce(&mut Settings) -> T) -> T {
        let mut settings = self.settings.lock().expect("settings lock");
        let out = f(&mut settings);
        if let Some(dir) = self.config_dir.get()
            && let Err(e) = settings.save(dir)
        {
            log::warn!("couldn't save settings in {}: {e}", dir.display());
        }
        out
    }

    pub fn db(&self, handle: u32) -> AppResult<SharedDb> {
        self.dbs
            .lock()
            .expect("dbs lock")
            .get(&handle)
            .map(|o| o.db.clone())
            .ok_or_else(|| AppError::not_found(format!("database {handle}")))
    }

    pub fn find_db(&self, path: &Path) -> Option<u32> {
        let dbs = self.dbs.lock().expect("dbs lock");
        dbs.iter().find(|(_, o)| o.path == path).map(|(&h, _)| h)
    }

    /// Adds an opened database for `usage`. Databases opened for the list
    /// become the most recent one.
    pub fn add_db(&self, db: Database, usage: DbUse) -> u32 {
        // Canonical, as `find_db` is given: a folder can only be opened once.
        let path = db.path().canonicalize().unwrap_or_else(|_| db.path().to_path_buf());
        self.add_shared(Arc::new(Mutex::new(db)), path, usage)
    }

    /// A database closed at `path` (canonical) that a job still holds.
    pub fn revive_db(&self, path: &Path) -> Option<SharedDb> {
        let mut released = self.released.lock().expect("released lock");
        released.retain(|(_, weak)| weak.strong_count() > 0);
        released.iter().find(|(p, _)| p == path).and_then(|(_, weak)| weak.upgrade())
    }

    /// Adds a database that's already open, from [`revive_db`](Self::revive_db).
    pub fn add_shared(&self, db: SharedDb, path: PathBuf, usage: DbUse) -> u32 {
        let handle = self.next_db.fetch_add(1, Ordering::Relaxed) + 1;
        if usage == DbUse::List {
            self.update_settings(|s| s.note_recent(&path));
        }
        let mut open = OpenDb { db, path, listed: false, reference: false };
        *open.flag(usage) = true;
        self.dbs.lock().expect("dbs lock").insert(handle, open);
        handle
    }

    /// Marks an open database as used for `usage` as well.
    pub fn claim_db(&self, handle: u32, usage: DbUse) {
        let path = {
            let mut dbs = self.dbs.lock().expect("dbs lock");
            let Some(open) = dbs.get_mut(&handle) else { return };
            *open.flag(usage) = true;
            open.path.clone()
        };
        if usage == DbUse::List {
            self.update_settings(|s| s.note_recent(&path));
        }
    }

    /// Stops using a database for `usage`; it's closed once nothing uses it.
    /// Leaving the list drops its filter and import report either way.
    /// False if `handle` isn't open.
    pub fn release_db(&self, handle: u32, usage: DbUse) -> bool {
        if usage == DbUse::List {
            self.filters.lock().expect("filters lock").remove(&handle);
            self.import_reports.lock().expect("import reports lock").remove(&handle);
        }
        let mut dbs = self.dbs.lock().expect("dbs lock");
        let Some(open) = dbs.get_mut(&handle) else { return false };
        *open.flag(usage) = false;
        if !open.listed
            && !open.reference
            && let Some(open) = dbs.remove(&handle)
            && Arc::strong_count(&open.db) > 1
        {
            let mut released = self.released.lock().expect("released lock");
            released.retain(|(_, weak)| weak.strong_count() > 0);
            released.push((open.path, Arc::downgrade(&open.db)));
        }
        true
    }

    /// Handle of the open reference database.
    pub fn reference_db(&self) -> Option<u32> {
        self.dbs.lock().expect("dbs lock").iter().find(|(_, o)| o.reference).map(|(&h, _)| h)
    }

    /// Adds a session. The newest session is the one on screen, so a running
    /// analysis switches to it.
    pub fn add_session(&self, session: Session) -> u32 {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        self.sessions.lock().expect("sessions lock").insert(id, session);
        if let Some(a) = self.analysis.lock().expect("analysis lock").as_mut() {
            a.attach(id);
        }
        id
    }

    pub fn remove_session(&self, id: u32) {
        self.sessions.lock().expect("sessions lock").remove(&id);
    }

    /// After a compaction renumbered the games of `handle`: sessions and
    /// import warnings follow their game's new id; a game that was removed
    /// becomes unsaved.
    pub fn remap_sources(&self, handle: u32, mapping: &[Option<GameId>]) {
        if let Some(report) = self.import_reports.lock().expect("import reports lock").get_mut(&handle) {
            for w in &mut report.warnings {
                w.game = w.game.and_then(|id| mapping.get(id as usize).copied().flatten());
            }
        }
        for session in self.sessions.lock().expect("sessions lock").values_mut() {
            if let Some((h, id)) = session.source
                && h == handle
            {
                session.source = mapping.get(id as usize).copied().flatten().map(|new| (h, new));
            }
        }
    }

    /// Runs `f` on a session while holding the sessions lock. Afterwards a
    /// running analysis of this session picks up a changed position.
    pub fn with_session<T>(&self, id: u32, f: impl FnOnce(&mut Session) -> AppResult<T>) -> AppResult<T> {
        let mut sessions = self.sessions.lock().expect("sessions lock");
        let session = sessions.get_mut(&id).ok_or_else(|| AppError::not_found(format!("session {id}")))?;
        let out = f(session)?;
        if let Some(a) = self.analysis.lock().expect("analysis lock").as_mut() {
            a.follow(id, session);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_stays_open_while_anything_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let db = Database::create(&dir.path().join("Ref")).unwrap();
        let path = db.path().to_path_buf();
        let handle = state.add_db(db, DbUse::List);
        state.claim_db(handle, DbUse::Reference);
        assert_eq!(state.reference_db(), Some(handle));

        // Leaving the list keeps the reference open, without the list's filter.
        state.filters.lock().unwrap().insert(handle, ActiveFilter::new(vec![0], "f".into()));
        assert!(state.release_db(handle, DbUse::List));
        assert!(state.db(handle).is_ok());
        assert!(state.filters.lock().unwrap().is_empty());

        assert!(state.release_db(handle, DbUse::Reference));
        assert!(state.db(handle).is_err(), "closed once nothing uses it");
        assert!(!state.release_db(handle, DbUse::List));
        // The folder isn't locked any more.
        Database::open(&path).unwrap();
    }

    #[test]
    fn a_database_closed_while_a_job_holds_it_can_be_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let db = Database::create(&dir.path().join("Busy")).unwrap();
        let path = db.path().canonicalize().unwrap();
        let handle = state.add_db(db, DbUse::List);
        let job = state.db(handle).unwrap();
        assert!(state.release_db(handle, DbUse::List));
        assert!(state.db(handle).is_err(), "closed for the app");
        assert!(Database::open(&path).is_err(), "still locked: the job has it");

        let again = state.revive_db(&path).expect("the job's database");
        assert!(Arc::ptr_eq(&again, &job));
        let handle = state.add_shared(again, path.clone(), DbUse::List);
        drop(job);
        assert!(state.db(handle).is_ok(), "open again under a new handle");

        // Once nothing holds it, it closes for real and isn't revived.
        assert!(state.release_db(handle, DbUse::List));
        assert!(state.revive_db(&path).is_none());
        Database::open(&path).unwrap();
    }
}
