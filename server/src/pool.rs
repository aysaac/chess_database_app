//! The databases this server has open, as `dbs/<id>.chessdb/` in the data
//! directory. Each is opened on first use and stays open (it's locked
//! against other processes while open), behind one mutex: queries on one
//! database run one at a time, though chess-db's long scans take the lock
//! a chunk at a time, so they interleave (see `docs/server.md`, Limits).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chess_db::{Database, DbError, EXTENSION, GameId, HeaderQuery, SortSpec};
use chess_jobs::{JobId, JobManager};

use crate::secret::is_db_id;
use crate::wire::{DeletedFilter, LinesDto};

/// Search results kept per database, and orders of them.
const SEARCHES_CACHED: usize = 8;
/// Lines (OPEN-14) kept per database.
const LINES_CACHED: usize = 32;

/// What lines were counted for: a position (its hash) and the state of the
/// database then. New games, or more of them indexed, make a new key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinesKey {
    pub position: u64,
    pub generation: u64,
    pub indexed: GameId,
}

/// What a list shows, sort order and paging aside: header criteria, the
/// position its games reach (by hash), and the games marked as deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListKey {
    pub header: HeaderQuery,
    pub position: Option<u64>,
    pub deleted: DeletedFilter,
}

/// A search's results in a sort order: (list, order, generation, sort
/// epoch, ids).
type Ordered = (ListKey, SortSpec, u64, u64, Arc<Vec<GameId>>);

/// Results kept per database, valid for its generation (every change to
/// the games bumps it).
#[derive(Default)]
pub struct Cache {
    searches: VecDeque<(ListKey, u64, Arc<Vec<GameId>>)>,
    orders: VecDeque<Ordered>,
    /// Whether some game is marked as deleted, at a generation.
    any_deleted: Option<(u64, bool)>,
    lines: VecDeque<(LinesKey, LinesDto)>,
    /// Jobs counting lines, with the games each reads.
    lines_jobs: Vec<(LinesKey, JobId, u32)>,
}

fn put<T>(queue: &mut VecDeque<T>, item: T, cap: usize) {
    queue.push_front(item);
    queue.truncate(cap);
}

impl Cache {
    pub fn search(&self, q: &ListKey, generation: u64) -> Option<Arc<Vec<GameId>>> {
        self.searches.iter().find(|(k, g, _)| k == q && *g == generation).map(|(_, _, ids)| ids.clone())
    }

    pub fn put_search(&mut self, q: ListKey, generation: u64, ids: Arc<Vec<GameId>>) {
        put(&mut self.searches, (q, generation, ids), SEARCHES_CACHED);
    }

    pub fn any_deleted(&self, generation: u64) -> Option<bool> {
        self.any_deleted.filter(|(g, _)| *g == generation).map(|(_, any)| any)
    }

    pub fn put_any_deleted(&mut self, generation: u64, any: bool) {
        self.any_deleted = Some((generation, any));
    }

    pub fn order(
        &self,
        q: &ListKey,
        spec: SortSpec,
        generation: u64,
        epoch: u64,
    ) -> Option<Arc<Vec<GameId>>> {
        self.orders
            .iter()
            .find(|(k, s, g, e, _)| k == q && *s == spec && *g == generation && *e == epoch)
            .map(|(.., ids)| ids.clone())
    }

    pub fn put_order(
        &mut self,
        q: ListKey,
        spec: SortSpec,
        generation: u64,
        epoch: u64,
        ids: Arc<Vec<GameId>>,
    ) {
        put(&mut self.orders, (q, spec, generation, epoch, ids), SEARCHES_CACHED);
    }

    pub fn lines(&self, key: &LinesKey) -> Option<LinesDto> {
        self.lines.iter().find(|(k, _)| k == key).map(|(_, l)| l.clone())
    }

    pub fn put_lines(&mut self, key: LinesKey, lines: LinesDto) {
        self.lines.retain(|(k, _)| *k != key);
        put(&mut self.lines, (key, lines), LINES_CACHED);
    }

    /// The job still counting the lines for `key`, and the games it reads.
    pub fn lines_job(&mut self, key: &LinesKey, jobs: &JobManager) -> Option<(JobId, u32)> {
        self.lines_jobs.retain(|(_, job, _)| jobs.status(*job).is_some_and(|s| !s.state.is_finished()));
        self.lines_jobs.iter().find(|(k, _, _)| k == key).map(|&(_, job, games)| (job, games))
    }

    pub fn add_lines_job(&mut self, key: LinesKey, job: JobId, games: u32) {
        self.lines_jobs.push((key, job, games));
    }
}

pub struct OpenDb {
    pub id: String,
    pub db: Mutex<Database>,
    pub cache: Mutex<Cache>,
    /// The import or index job writing to the database: one at a time (two
    /// position index builds would share temporary files).
    pub writer: Mutex<Option<JobId>>,
    /// A compaction holds the database's lock for its whole run: requests
    /// answer "busy" meanwhile instead of waiting for it.
    pub compacting: AtomicBool,
    /// When a request or job last took it ([`Pool::get`]): idle ones close.
    last_used: Mutex<Instant>,
}

impl OpenDb {
    fn new(id: &str, db: Database) -> OpenDb {
        OpenDb {
            id: id.to_owned(),
            db: Mutex::new(db),
            cache: Mutex::default(),
            writer: Mutex::new(None),
            compacting: AtomicBool::new(false),
            last_used: Mutex::new(Instant::now()),
        }
    }

    pub fn is_compacting(&self) -> bool {
        self.compacting.load(Ordering::Acquire)
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Database> {
        self.db.lock().expect("db lock")
    }
}

/// Whether the writer slot's job still runs.
pub fn writer_busy(writer: &Option<JobId>, jobs: &JobManager) -> bool {
    writer.is_some_and(|job| jobs.status(job).is_some_and(|s| !s.state.is_finished()))
}

type Slot = Arc<Mutex<Option<Arc<OpenDb>>>>;

pub struct Pool {
    dir: PathBuf,
    slots: Mutex<HashMap<String, Slot>>,
    /// Ids deleted since start: never opened again.
    deleted: Mutex<HashSet<String>>,
}

/// The total size of the files in `dir`.
pub fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

impl Pool {
    pub fn new(dir: PathBuf) -> Pool {
        Pool { dir, slots: Mutex::default(), deleted: Mutex::default() }
    }

    /// The folder of database `id`. Only ever called with a checked id.
    pub fn path(&self, id: &str) -> PathBuf {
        assert!(is_db_id(id), "database ids are checked before a path is built");
        self.dir.join(format!("{id}.{EXTENSION}"))
    }

    fn slot(&self, id: &str) -> Slot {
        self.slots.lock().expect("pool lock").entry(id.to_owned()).or_default().clone()
    }

    fn check_not_deleted(&self, id: &str) -> Result<(), DbError> {
        if self.deleted.lock().expect("pool lock").contains(id) {
            return Err(DbError::Invalid("the database was deleted".into()));
        }
        Ok(())
    }

    pub fn create(&self, id: &str) -> Result<Arc<OpenDb>, DbError> {
        self.check_not_deleted(id)?;
        let slot = self.slot(id);
        let mut slot = slot.lock().expect("pool slot lock");
        let open = Arc::new(OpenDb::new(id, Database::create(&self.path(id))?));
        *slot = Some(open.clone());
        Ok(open)
    }

    /// The open database, opening it on first use. Opening a big one takes
    /// a moment (half a second for a million games); only requests for that
    /// database wait.
    pub fn get(&self, id: &str) -> Result<Arc<OpenDb>, DbError> {
        self.check_not_deleted(id)?;
        let slot = self.slot(id);
        let mut slot = slot.lock().expect("pool slot lock");
        if let Some(open) = &*slot {
            *open.last_used.lock().expect("last used lock") = Instant::now();
            return Ok(open.clone());
        }
        let open = Arc::new(OpenDb::new(id, Database::open(&self.path(id))?));
        log::info!("opened database {id} ({} games)", open.lock().count());
        *slot = Some(open.clone());
        Ok(open)
    }

    /// Closes the databases nobody took for `idle` and nothing holds now (no
    /// request or job has them, the writer slot is free): their files are
    /// closed and their lock released; the next request opens them again.
    /// Returns their ids.
    pub fn close_idle(&self, idle: Duration, jobs: &JobManager) -> Vec<String> {
        let slots: Vec<(String, Slot)> =
            self.slots.lock().expect("pool lock").iter().map(|(id, s)| (id.clone(), s.clone())).collect();
        let mut closed = Vec::new();
        for (id, slot) in slots {
            let mut slot = slot.lock().expect("pool slot lock");
            let Some(open) = &*slot else { continue };
            let idle_long = open.last_used.lock().expect("last used lock").elapsed() >= idle;
            let held = Arc::strong_count(open) > 1;
            let writing =
                writer_busy(&open.writer.lock().expect("writer lock"), jobs) || open.is_compacting();
            if idle_long && !held && !writing {
                *slot = None;
                closed.push(id);
            }
        }
        closed
    }

    /// How many databases are open.
    pub fn open_count(&self) -> usize {
        let slots: Vec<Slot> = self.slots.lock().expect("pool lock").values().cloned().collect();
        slots.iter().filter(|s| s.lock().expect("pool slot lock").is_some()).count()
    }

    /// The database if it's open, without opening it.
    pub fn get_open(&self, id: &str) -> Option<Arc<OpenDb>> {
        let slot = self.slots.lock().expect("pool lock").get(id).cloned()?;
        slot.lock().expect("pool slot lock").clone()
    }

    /// Games and size on disk, without opening the database if it isn't.
    pub fn stats(&self, id: &str) -> (u32, u64) {
        // Not while a long write (a compaction) holds it: from the files then.
        if let Some(open) = self.get_open(id)
            && let Ok(db) = open.db.try_lock()
        {
            let count = db.count();
            return (count, dir_size(&self.path(id)));
        }
        match Database::summary(&self.path(id)) {
            Ok(s) => (s.games, s.size_bytes),
            Err(e) => {
                log::warn!("database {id}: {e}");
                (0, dir_size(&self.path(id)))
            }
        }
    }

    /// Closes and deletes database `id`. Its row must be gone already, so
    /// no new request reaches it; requests that have it finish first (for
    /// at most 30 seconds). It's renamed to `<id>.deleted` before its files
    /// go, so a folder half deleted (Windows refuses to delete open files)
    /// is finished at the next start.
    pub fn remove(&self, id: &str) -> std::io::Result<()> {
        self.deleted.lock().expect("pool lock").insert(id.to_owned());
        let slot = self.slots.lock().expect("pool lock").remove(id);
        if let Some(open) = slot.and_then(|s| s.lock().expect("pool slot lock").take()) {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Arc::strong_count(&open) > 1 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(open);
        }
        let path = self.path(id);
        if !path.exists() {
            return Ok(());
        }
        let trash = path.with_extension("deleted");
        std::fs::rename(&path, &trash)?;
        std::fs::remove_dir_all(&trash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::new_db_id;

    #[test]
    fn create_get_remove() {
        let dir = tempfile::tempdir().unwrap();
        let pool = Pool::new(dir.path().to_path_buf());
        let id = new_db_id();
        let open = pool.create(&id).unwrap();
        assert!(Arc::ptr_eq(&open, &pool.get(&id).unwrap()));
        assert_eq!(pool.stats(&id).0, 0);
        drop(open);
        pool.remove(&id).unwrap();
        assert!(!pool.path(&id).exists());
        assert!(pool.get(&id).is_err(), "a deleted id isn't opened again");
    }

    #[test]
    #[should_panic(expected = "checked")]
    fn paths_need_checked_ids() {
        Pool::new(PathBuf::from("x")).path("../etc");
    }

    #[test]
    fn caches_follow_the_generation() {
        let mut c = Cache::default();
        let key = |white: String| ListKey {
            header: HeaderQuery { white, ..HeaderQuery::default() },
            position: None,
            deleted: DeletedFilter::Hide,
        };
        let q = key("a".into());
        c.put_search(q.clone(), 1, Arc::new(vec![1, 2]));
        assert_eq!(c.search(&q, 1).as_deref(), Some(&vec![1, 2]));
        assert!(c.search(&q, 2).is_none());
        assert!(c.search(&ListKey { position: Some(5), ..q.clone() }, 1).is_none());
        for i in 0..20 {
            c.put_search(key(i.to_string()), 1, Arc::default());
        }
        assert!(c.search(&q, 1).is_none(), "the oldest went");
    }
}
