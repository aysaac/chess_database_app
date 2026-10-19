use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use chess_core::game::GameEditor;
use chess_db::{Database, GameId, SortSpec, sorted_subset};
use chess_jobs::{JobId, JobManager};

use crate::commands::duplicates::DbDuplicates;
use crate::dto::{ImportReportDto, LinesDto};
use crate::engines::Analysis;
use crate::error::{AppError, AppResult};
use crate::settings::{DbView, FilterStep, SavedFilter, Settings};

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

/// Running position index builds (SRCH-04), by database. A database closed
/// while its build runs and opened again gets a new handle for the same
/// `Database` (A5); a second build there would write the same temporary
/// files and fail when the first one cleans up.
#[derive(Default)]
pub struct IndexBuilds(Vec<(Weak<Mutex<Database>>, JobId)>);

impl IndexBuilds {
    /// The build of `db` that's still running, if any.
    pub fn running(&mut self, db: &SharedDb, jobs: &JobManager) -> Option<JobId> {
        self.0.retain(|(weak, job)| {
            weak.strong_count() > 0 && jobs.status(*job).is_some_and(|s| !s.state.is_finished())
        });
        self.0.iter().find(|(weak, _)| std::ptr::eq(weak.as_ptr(), Arc::as_ptr(db))).map(|(_, job)| *job)
    }

    pub fn add(&mut self, db: Weak<Mutex<Database>>, job: JobId) {
        self.0.push((db, job));
    }
}

/// What a Reference tab result (OPEN-14) was counted for: a database handle
/// (handles are never reused), a position (its hash) and the state of the
/// database then. Any change to the games, or more of them indexed, makes a
/// new key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinesKey {
    pub handle: u32,
    pub position: u64,
    pub generation: u64,
    pub indexed: GameId,
}

impl LinesKey {
    pub fn new(handle: u32, position: u64, db: &Database) -> LinesKey {
        LinesKey { handle, position, generation: db.generation(), indexed: db.positions().covered() }
    }
}

/// Lines cached at most.
const LINES_CACHED: usize = 32;

/// The Reference tab's lines (OPEN-14): the most recently counted, and the
/// jobs counting them for common positions.
#[derive(Default)]
pub struct LinesCache {
    /// Most recently used first.
    cached: VecDeque<(LinesKey, LinesDto)>,
    /// Running jobs, with the number of games each reads.
    jobs: Vec<(LinesKey, JobId, u32)>,
}

impl LinesCache {
    pub fn get(&mut self, key: &LinesKey) -> Option<LinesDto> {
        let i = self.cached.iter().position(|(k, _)| k == key)?;
        let entry = self.cached.remove(i)?;
        let lines = entry.1.clone();
        self.cached.push_front(entry);
        Some(lines)
    }

    pub fn put(&mut self, key: LinesKey, lines: LinesDto) {
        self.cached.retain(|(k, _)| *k != key);
        self.cached.push_front((key, lines));
        self.cached.truncate(LINES_CACHED);
    }

    /// The job still counting the lines for `key`, and how many games it reads.
    pub fn running(&mut self, key: &LinesKey, jobs: &JobManager) -> Option<(JobId, u32)> {
        self.jobs.retain(|(_, job, _)| jobs.status(*job).is_some_and(|s| !s.state.is_finished()));
        self.jobs.iter().find(|(k, _, _)| k == key).map(|&(_, job, games)| (job, games))
    }

    pub fn add_job(&mut self, key: LinesKey, job: JobId, games: u32) {
        self.jobs.push((key, job, games));
    }
}

/// Search results the game list is limited to (SRCH-14).
pub struct ActiveFilter {
    /// Ascending game ids; a duplicates filter (DB-08) lists each kept game
    /// before its copies instead, so the list's natural order shows them
    /// together.
    pub ids: Arc<Vec<GameId>>,
    pub description: String,
    /// How the filter was made, to make it again when the database is
    /// reopened (QOL-07); empty for one that can't be made again.
    pub steps: Vec<FilterStep>,
    /// The ids in each sort order used so far, valid for `epoch`.
    orders: HashMap<SortSpec, Arc<Vec<GameId>>>,
    epoch: u64,
}

impl ActiveFilter {
    pub fn new(ids: Vec<GameId>, description: String, steps: Vec<FilterStep>) -> ActiveFilter {
        ActiveFilter { ids: Arc::new(ids), description, steps, orders: HashMap::new(), epoch: 0 }
    }

    pub fn saved(&self) -> SavedFilter {
        SavedFilter { steps: self.steps.clone(), description: self.description.clone() }
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
    /// Lock order: `filters` before a database. Change it through
    /// `set_filter` and `clear_filter`, which remember it (QOL-07).
    pub filters: Mutex<HashMap<u32, ActiveFilter>>,
    /// Bumped (under the `filters` lock) whenever a list filter is set or
    /// cleared, so a filter being restored doesn't replace a newer one.
    filter_generation: AtomicU64,
    sessions: Mutex<HashMap<u32, Session>>,
    next_session: AtomicU32,
    pub jobs: JobManager,
    /// Lock order: `index_builds` before a database.
    pub index_builds: Mutex<IndexBuilds>,
    pub settings: Mutex<Settings>,
    config_dir: OnceLock<PathBuf>,
    /// Where the app keeps its own data, such as the sample database (QOL-08).
    data_dir: OnceLock<PathBuf>,
    pub analysis: Mutex<Option<Analysis>>,
    /// The last import into each database (by handle), while it had warnings.
    pub import_reports: Mutex<HashMap<u32, ImportReportDto>>,
    /// The searches for duplicate games (DB-08) in each database in the
    /// list, by handle. Lock order: `duplicates` before a database.
    pub duplicates: Mutex<HashMap<u32, DbDuplicates>>,
    /// The Reference tab's counted lines (OPEN-14). Held only to read or
    /// store them (and to ask `jobs` about its jobs), never while taking a
    /// database or session lock.
    pub lines: Mutex<LinesCache>,
    next_window: AtomicU32,
    /// A database an editor window should open on start-up (the library
    /// window spawned it for this path), by the new window's label; taken
    /// (removed) once that window reads it.
    pub editor_targets: Mutex<HashMap<String, Option<PathBuf>>>,
}

impl AppState {
    /// Loads the settings from `config_dir` and notes the data folder
    /// (called once at start-up).
    pub fn init_dirs(&self, config_dir: PathBuf, data_dir: PathBuf) {
        *self.settings.lock().expect("settings lock") = Settings::load(&config_dir);
        let _ = self.config_dir.set(config_dir);
        let _ = self.data_dir.set(data_dir);
    }

    /// The app's data folder (it may not exist yet).
    pub fn data_dir(&self) -> AppResult<PathBuf> {
        self.data_dir.get().cloned().ok_or_else(|| AppError::not_found("the app's data folder"))
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

    /// A fresh, unique label for an editor window (DB-07's library window
    /// opens one of these per database).
    pub fn new_editor_label(&self) -> String {
        format!("editor-{}", self.next_window.fetch_add(1, Ordering::Relaxed) + 1)
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
        let path = dunce::canonicalize(db.path()).unwrap_or_else(|_| db.path().to_path_buf());
        self.add_shared(Arc::new(Mutex::new(db)), path, usage)
    }

    /// The database at `path` (canonical) if this process has it open: in
    /// the list, as the reference, or held by a job after it was closed.
    pub fn open_db_at(&self, path: &Path) -> Option<SharedDb> {
        self.find_db(path).and_then(|h| self.db(h).ok()).or_else(|| self.revive_db(path))
    }

    /// A database closed at `path` (canonical) that a job still holds.
    pub fn revive_db(&self, path: &Path) -> Option<SharedDb> {
        let mut released = self.released.lock().expect("released lock");
        released.retain(|(_, weak)| weak.strong_count() > 0);
        released.iter().find(|(p, _)| p == path).and_then(|(_, weak)| weak.upgrade())
    }

    /// The database at `path` for a job that writes into it (copying games,
    /// DB-06/QOL-04): the app's own if it has it open, else it's opened for
    /// the job alone, or created first with `create` if it doesn't exist.
    /// Opening it in the list while the job still runs shares the job's
    /// (see [`revive_db`](Self::revive_db)); otherwise it closes when the
    /// job lets go of it. Also says whether it was created.
    pub fn db_at(&self, path: &Path, create: bool) -> AppResult<(SharedDb, bool)> {
        let dir = if create { Database::dir_for(path) } else { path.to_path_buf() };
        let canonical = dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if let Some(handle) = self.find_db(&canonical) {
            return Ok((self.db(handle)?, false));
        }
        if let Some(db) = self.revive_db(&canonical) {
            return Ok((db, false));
        }
        let created = create && !dir.exists();
        let db = if created { Database::create(&dir)? } else { Database::open(&canonical)? };
        let canonical = dunce::canonicalize(db.path()).unwrap_or_else(|_| db.path().to_path_buf());
        let db = Arc::new(Mutex::new(db));
        let mut released = self.released.lock().expect("released lock");
        released.retain(|(_, weak)| weak.strong_count() > 0);
        released.push((canonical, Arc::downgrade(&db)));
        Ok((db, created))
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
    /// Leaving the list drops its filter, import report and searches for
    /// duplicates either way. False if `handle` isn't open.
    pub fn release_db(&self, handle: u32, usage: DbUse) -> bool {
        if usage == DbUse::List {
            self.filters.lock().expect("filters lock").remove(&handle);
            self.import_reports.lock().expect("import reports lock").remove(&handle);
            self.duplicates.lock().expect("duplicates lock").remove(&handle);
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

    /// The canonical path of `handle` while it's shown in the game list.
    fn list_path(&self, handle: u32) -> Option<PathBuf> {
        self.dbs.lock().expect("dbs lock").get(&handle).filter(|o| o.listed).map(|o| o.path.clone())
    }

    /// Whether `handle` is open in the game list.
    pub fn is_listed(&self, handle: u32) -> bool {
        self.list_path(handle).is_some()
    }

    /// Limits the list of `handle` to `filter`, and remembers it for the next
    /// time the database is opened (QOL-07). A filter without steps can't be
    /// made again (a cross-table edition's games, DB-16, or DB-08's
    /// duplicates): it's for this session, and the saved filter is forgotten.
    pub fn set_filter(&self, handle: u32, filter: ActiveFilter) {
        let saved = (!filter.steps.is_empty()).then(|| filter.saved());
        {
            let mut filters = self.filters.lock().expect("filters lock");
            filters.insert(handle, filter);
            self.filter_generation.fetch_add(1, Ordering::Relaxed);
        }
        self.remember_filter(handle, saved);
    }

    /// Shows all games of `handle` again, next time too.
    pub fn clear_filter(&self, handle: u32) {
        {
            let mut filters = self.filters.lock().expect("filters lock");
            filters.remove(&handle);
            self.filter_generation.fetch_add(1, Ordering::Relaxed);
        }
        self.remember_filter(handle, None);
    }

    /// Where a filter change stands, for [`finish_restore`](Self::finish_restore).
    pub fn filter_generation(&self) -> u64 {
        let _filters = self.filters.lock().expect("filters lock");
        self.filter_generation.load(Ordering::Relaxed)
    }

    /// Finishes restoring the saved filter of `handle`: installs `filter`,
    /// or with `None` (it couldn't be made again) forgets the saved one.
    /// Does nothing, returning false, if the list's filter changed since
    /// `generation` or the database left the list.
    pub fn finish_restore(&self, handle: u32, filter: Option<ActiveFilter>, generation: u64) -> bool {
        if self.list_path(handle).is_none() {
            return false;
        }
        {
            let mut filters = self.filters.lock().expect("filters lock");
            if self.filter_generation.load(Ordering::Relaxed) != generation {
                return false;
            }
            let Some(filter) = filter else {
                drop(filters);
                self.remember_filter(handle, None);
                return true;
            };
            filters.insert(handle, filter);
            self.filter_generation.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    fn remember_filter(&self, handle: u32, saved: Option<SavedFilter>) {
        if let Some(path) = self.list_path(handle) {
            self.update_settings(|s| s.db_view_mut(&path).filter = saved);
        }
    }

    /// The saved list view of the database `handle` (empty if it has none).
    pub fn db_view(&self, handle: u32) -> AppResult<DbView> {
        let path = self.list_path(handle).ok_or_else(|| AppError::not_found(format!("database {handle}")))?;
        let settings = self.settings.lock().expect("settings lock");
        Ok(settings.db_view(&path).cloned().unwrap_or_default())
    }

    /// Changes and saves the list view of the database `handle`.
    pub fn update_db_view(&self, handle: u32, f: impl FnOnce(&mut DbView)) -> AppResult<()> {
        let path = self.list_path(handle).ok_or_else(|| AppError::not_found(format!("database {handle}")))?;
        self.update_settings(|s| f(s.db_view_mut(&path)));
        Ok(())
    }

    /// Forgets the list view saved for the database `handle` (it was just
    /// created, maybe where an older one was).
    pub fn forget_db_view(&self, handle: u32) {
        if let Some(path) = self.list_path(handle) {
            self.update_settings(|s| s.forget_db_view(&path));
        }
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
        state.filters.lock().unwrap().insert(handle, ActiveFilter::new(vec![0], "f".into(), Vec::new()));
        assert!(state.release_db(handle, DbUse::List));
        assert!(state.db(handle).is_ok());
        assert!(state.filters.lock().unwrap().is_empty());

        assert!(state.release_db(handle, DbUse::Reference));
        assert!(state.db(handle).is_err(), "closed once nothing uses it");
        assert!(!state.release_db(handle, DbUse::List));
        // The folder isn't locked any more.
        Database::open(&path).unwrap();
    }

    fn step(text: &str) -> FilterStep {
        FilterStep::Header {
            query: crate::dto::HeaderQueryDto { text: text.into(), ..Default::default() },
            quick: true,
        }
    }

    #[test]
    fn the_list_filter_is_remembered_per_database() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let state = AppState::default();
        state.init_dirs(config.clone(), dir.path().join("data"));
        let handle = state.add_db(Database::create(&dir.path().join("A")).unwrap(), DbUse::List);
        assert_eq!(state.db_view(handle).unwrap(), DbView::default());

        state.set_filter(handle, ActiveFilter::new(vec![1, 2], "“x”".into(), vec![step("x")]));
        let saved = state.db_view(handle).unwrap().filter.expect("saved");
        assert_eq!((saved.description.as_str(), saved.steps.as_slice()), ("“x”", [step("x")].as_slice()));
        // Saved to disk, keyed by the database's folder.
        let path = state.list_path(handle).unwrap();
        assert_eq!(Settings::load(&config).db_view(&path).unwrap().filter.as_ref(), Some(&saved));

        // Closing keeps it for next time; clearing forgets it.
        assert!(state.release_db(handle, DbUse::List));
        assert!(Settings::load(&config).db_view(&path).unwrap().filter.is_some());
        let handle = state.add_db(Database::open(&path).unwrap(), DbUse::List);
        assert!(state.filters.lock().unwrap().get(&handle).is_none(), "not active until restored");
        state.clear_filter(handle);
        assert_eq!(state.db_view(handle).unwrap().filter, None);

        // The reference database (not in the list) remembers nothing.
        let other = state.add_db(Database::create(&dir.path().join("B")).unwrap(), DbUse::Reference);
        state.set_filter(other, ActiveFilter::new(vec![], "y".into(), vec![step("y")]));
        assert!(state.db_view(other).is_err());
        assert_eq!(Settings::load(&config).database_views.len(), 1);
    }

    /// R2-3 of the DB-16 review: a filter for this session only saves no
    /// filter, so reopening shows all games without a "Restoring" message.
    #[test]
    fn a_filter_without_steps_is_not_saved() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let state = AppState::default();
        state.init_dirs(config.clone(), dir.path().join("data"));
        let handle = state.add_db(Database::create(&dir.path().join("A")).unwrap(), DbUse::List);
        let path = state.list_path(handle).unwrap();
        state.set_filter(handle, ActiveFilter::new(vec![1], "“x”".into(), vec![step("x")]));
        assert!(state.db_view(handle).unwrap().filter.is_some());
        state.set_filter(handle, ActiveFilter::new(vec![2], "Event: X (2020)".into(), Vec::new()));
        assert_eq!(state.filters.lock().unwrap()[&handle].description, "Event: X (2020)", "it's the list's");
        assert_eq!(state.db_view(handle).unwrap().filter, None, "and nothing is saved");
        assert_eq!(Settings::load(&config).db_view(&path).unwrap().filter, None);
        assert_eq!(crate::dto::DbViewDto::from(&state.db_view(handle).unwrap()).filter, None);
    }

    #[test]
    fn a_restored_filter_doesnt_replace_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        state.init_dirs(dir.path().join("config"), dir.path().join("data"));
        let handle = state.add_db(Database::create(&dir.path().join("A")).unwrap(), DbUse::List);
        state.set_filter(handle, ActiveFilter::new(vec![1], "old".into(), vec![step("old")]));
        state.filters.lock().unwrap().clear();

        let generation = state.filter_generation();
        assert!(state.finish_restore(
            handle,
            Some(ActiveFilter::new(vec![1], "old".into(), vec![])),
            generation
        ));
        assert_eq!(state.filters.lock().unwrap()[&handle].description, "old");

        // The user filtered while it was being restored: theirs stays.
        let generation = state.filter_generation();
        state.set_filter(handle, ActiveFilter::new(vec![2], "new".into(), vec![step("new")]));
        assert!(!state.finish_restore(
            handle,
            Some(ActiveFilter::new(vec![1], "old".into(), vec![])),
            generation
        ));
        assert!(!state.finish_restore(handle, None, generation), "and stays saved");
        assert_eq!(state.filters.lock().unwrap()[&handle].description, "new");
        assert_eq!(state.db_view(handle).unwrap().filter.unwrap().description, "new");

        // A filter that couldn't be made again is forgotten.
        let generation = state.filter_generation();
        assert!(state.finish_restore(handle, None, generation));
        assert_eq!(state.db_view(handle).unwrap().filter, None);

        // Not after the database left the list.
        let generation = state.filter_generation();
        state.release_db(handle, DbUse::List);
        assert!(!state.finish_restore(
            handle,
            Some(ActiveFilter::new(vec![], "x".into(), vec![])),
            generation
        ));
        assert!(state.filters.lock().unwrap().is_empty());
    }

    #[test]
    fn a_reopened_database_shares_its_running_index_build() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let handle = state.add_db(Database::create(&dir.path().join("A")).unwrap(), DbUse::List);
        let other = state.add_db(Database::create(&dir.path().join("B")).unwrap(), DbUse::List);
        let db = state.db(handle).unwrap();
        let (finish, finished) = std::sync::mpsc::channel::<()>();
        let held = db.clone();
        let job = state.jobs.spawn("index", "Indexing positions", move |_| {
            let _held = held;
            let _ = finished.recv();
            Ok(String::new())
        });
        state.index_builds.lock().unwrap().add(Arc::downgrade(&db), job);
        let path = dunce::canonicalize(db.lock().unwrap().path()).unwrap();
        drop(db);

        // Closed and opened again while the build runs: the same build.
        assert!(state.release_db(handle, DbUse::List));
        let again = state.revive_db(&path).expect("the build still holds it");
        let handle = state.add_shared(again, path, DbUse::List);
        let db = state.db(handle).unwrap();
        let mut builds = state.index_builds.lock().unwrap();
        assert_eq!(builds.running(&db, &state.jobs), Some(job));
        assert_eq!(builds.running(&state.db(other).unwrap(), &state.jobs), None, "another database");
        drop(builds);

        finish.send(()).unwrap();
        state.jobs.wait(job);
        assert_eq!(state.index_builds.lock().unwrap().running(&db, &state.jobs), None, "finished");
    }

    #[test]
    fn a_copy_target_is_shared_while_a_job_writes_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let listed = state.add_db(Database::create(&dir.path().join("Listed")).unwrap(), DbUse::List);
        let path = dunce::canonicalize(dir.path().join("Listed.chessdb")).unwrap();

        // An open database is the app's own.
        let (db, created) = state.db_at(&path, false).unwrap();
        assert!(Arc::ptr_eq(&db, &state.db(listed).unwrap()) && !created);

        // A new one is created (with its extension) for the job alone.
        let (clip, created) = state.db_at(&dir.path().join("Clip"), true).unwrap();
        let clip_path = dunce::canonicalize(dir.path().join("Clip.chessdb")).unwrap();
        assert!(created && state.find_db(&clip_path).is_none());
        // Asked again (another drop) or opened in the list meanwhile: the same one.
        let (again, created) = state.db_at(&dir.path().join("Clip.chessdb"), true).unwrap();
        assert!(Arc::ptr_eq(&again, &clip) && !created);
        let revived = state.revive_db(&clip_path).expect("held by the job");
        assert!(Arc::ptr_eq(&revived, &clip));
        drop((again, revived, clip));

        // Once the job is done it's closed: opening it again works.
        let (reopened, created) = state.db_at(&clip_path, false).unwrap();
        assert!(!created);
        drop(reopened);
        Database::open(&clip_path).unwrap();
        assert!(state.db_at(&dir.path().join("Missing.chessdb"), false).is_err());
    }

    #[test]
    fn a_database_closed_while_a_job_holds_it_can_be_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::default();
        let db = Database::create(&dir.path().join("Busy")).unwrap();
        let path = dunce::canonicalize(db.path()).unwrap();
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
