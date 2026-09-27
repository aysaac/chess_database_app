use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chess_core::game::GameEditor;
use chess_db::{Database, GameId, SortSpec, sorted_subset};
use chess_jobs::JobManager;

use crate::engines::Analysis;
use crate::error::{AppError, AppResult};
use crate::settings::Settings;

pub type SharedDb = Arc<Mutex<Database>>;

pub struct OpenDb {
    pub db: SharedDb,
    pub path: PathBuf,
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
    next_db: AtomicU32,
    /// Lock order: `filters` before a database.
    pub filters: Mutex<HashMap<u32, ActiveFilter>>,
    sessions: Mutex<HashMap<u32, Session>>,
    next_session: AtomicU32,
    pub jobs: JobManager,
    pub settings: Mutex<Settings>,
    config_dir: OnceLock<PathBuf>,
    pub analysis: Mutex<Option<Analysis>>,
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

    pub fn add_db(&self, db: Database) -> u32 {
        let handle = self.next_db.fetch_add(1, Ordering::Relaxed) + 1;
        let path = db.path().to_path_buf();
        self.update_settings(|s| s.note_recent(&path));
        self.dbs.lock().expect("dbs lock").insert(handle, OpenDb { db: Arc::new(Mutex::new(db)), path });
        handle
    }

    pub fn remove_db(&self, handle: u32) -> bool {
        self.filters.lock().expect("filters lock").remove(&handle);
        self.dbs.lock().expect("dbs lock").remove(&handle).is_some()
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

    /// After a compaction renumbered the games of `handle`: sessions follow
    /// their game's new id; a game that was removed becomes unsaved.
    pub fn remap_sources(&self, handle: u32, mapping: &[Option<GameId>]) {
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
