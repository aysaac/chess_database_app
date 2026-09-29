//! A database is a directory `Name.chessdb/` holding:
//!
//! - `db.meta`: text file with format version and a random id.
//! - `games.idx`: 64-byte header + one 64-byte [`IndexRecord`] per game.
//! - `games.blob`: 16-byte header + append-only game blobs.
//! - `players.names`, `events.names`, `sites.names`, `annotators.names`.
//! - `.lock`: held exclusively while the database is open (single writer).
//! - Derived, rebuildable files: the position index and opening
//!   classification (see [`crate::positions`]).
//!
//! Commit protocol (crash safety): blobs → fsync → names → fsync → index
//! records → fsync → committed count in the index header → fsync. On open,
//! index bytes beyond the committed count are truncated; unreferenced blob
//! bytes are harmless garbage.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chess_core::game::{Game, GameResult, Headers};

use crate::blob::{decode_game, encode_game};
use crate::bytes::u64_at;
use crate::io::{read_exact_at, write_all_at};
use crate::names::NameTable;
use crate::positions::PositionIndex;
use crate::record::{
    FLAG_ANNOTATIONS, FLAG_COMMENTS, FLAG_DELETED, FLAG_SETUP, FLAG_VARIATIONS, IndexRecord, RECORD_SIZE,
    pack_date, pack_eco, pack_round, result_from_u8, result_to_u8, unpack_date, unpack_eco, unpack_round,
};
use crate::sort::SortSpec;
use crate::{DbError, GameId};

pub const EXTENSION: &str = "chessdb";
/// Version in `db.meta`. 2: games replaced are recorded in
/// `positions.dirty` (older versions didn't, so they must not open it).
const META_VERSION: u32 = 2;
/// Version in the headers of `games.idx` and `games.blob`.
const FORMAT_VERSION: u32 = 1;

const IDX_MAGIC: &[u8; 8] = b"CHDBIDX1";
const IDX_HEADER: u64 = 64;
const IDX_COUNT_OFFSET: u64 = 16;

const BLOB_MAGIC: &[u8; 8] = b"CHDBBLOB";
const BLOB_HEADER: u64 = 16;

const META_FILE: &str = "db.meta";
const IDX_FILE: &str = "games.idx";
const BLOB_FILE: &str = "games.blob";
const LOCK_FILE: &str = ".lock";
/// Present while a compaction swaps in its new files (see [`Database::compact`]).
const COMPACT_MARKER: &str = "compact.commit";
const NEW_SUFFIX: &str = ".new";

/// Which name table a field uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameKind {
    Player = 0,
    Event = 1,
    Site = 2,
    Annotator = 3,
}

const NAME_FILES: [&str; 4] = ["players.names", "events.names", "sites.names", "annotators.names"];

/// A game list row, resolved from the index record without reading the blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRow {
    pub id: GameId,
    pub white: String,
    pub white_elo: Option<u16>,
    pub black: String,
    pub black_elo: Option<u16>,
    pub result: GameResult,
    pub date: String,
    pub event: String,
    pub site: String,
    pub round: String,
    pub eco: String,
    pub annotator: String,
    pub plies: u16,
    pub deleted: bool,
    pub has_variations: bool,
    pub has_comments: bool,
    pub has_annotations: bool,
    pub has_setup: bool,
    /// Opening from the classification (OPEN-11), when the index has it.
    pub opening: Option<&'static chess_core::openings::Opening>,
}

/// A game ready to be stored: encoded blob plus what the index record
/// needs. Holding these instead of full [`Game`] trees keeps import batches
/// small.
#[derive(Debug, Clone)]
pub struct PreparedGame {
    headers: Headers,
    flags: u32,
    plies: u16,
    blob: Vec<u8>,
}

impl PreparedGame {
    pub fn new(game: &Game) -> PreparedGame {
        let mut flags = 0;
        if game.has_variations() {
            flags |= FLAG_VARIATIONS;
        }
        if game.has_comments() {
            flags |= FLAG_COMMENTS;
        }
        if game.setup.is_some() {
            flags |= FLAG_SETUP;
        }
        if game.preorder().iter().any(|&id| {
            let n = game.node(id);
            !n.nags.is_empty() || !n.shapes.is_empty()
        }) {
            flags |= FLAG_ANNOTATIONS;
        }
        // Extra tags live in the blob only.
        let headers = Headers { extra: Vec::new(), ..game.headers.clone() };
        PreparedGame {
            headers,
            flags,
            plies: game.mainline_len().min(u16::MAX as usize) as u16,
            blob: encode_game(game),
        }
    }
}

/// What the opening tree needs of each game, kept in memory once used so a
/// tree over a million games needs no record reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GameStat {
    pub white_elo: u16,
    pub black_elo: u16,
    pub year: u16,
    /// 0 unknown, 1 White won, 2 Black won, 3 draw.
    pub result: u8,
    pub deleted: bool,
}

impl GameStat {
    fn of(r: &IndexRecord) -> GameStat {
        GameStat {
            white_elo: r.white_elo,
            black_elo: r.black_elo,
            year: (r.date >> 9) as u16,
            result: r.result,
            deleted: r.is_deleted(),
        }
    }
}

/// What [`Database::compact`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSummary {
    pub kept: u32,
    pub removed: u32,
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// New id of every old game id; `None` for removed (deleted) games.
    pub mapping: Vec<Option<GameId>>,
}

/// Finishes or rolls back a compaction interrupted by a crash: with the
/// commit marker, the new files replace the old ones; without it, the new
/// files are incomplete and are deleted.
fn recover_compaction(dir: &Path) -> Result<(), DbError> {
    let committed = dir.join(COMPACT_MARKER).exists();
    let files = [IDX_FILE, BLOB_FILE].into_iter().chain(NAME_FILES);
    for name in files {
        let new = dir.join(format!("{name}{NEW_SUFFIX}"));
        if new.exists() {
            if committed {
                std::fs::rename(&new, dir.join(name))?;
            } else {
                std::fs::remove_file(&new)?;
            }
        }
    }
    if committed {
        log::warn!("finished an interrupted compaction of {}", dir.display());
        PositionIndex::remove_files(dir)?;
        std::fs::remove_file(dir.join(COMPACT_MARKER))?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbInfo {
    pub name: String,
    pub path: PathBuf,
    pub id: String,
    pub games: u32,
    pub players: u32,
    pub events: u32,
    pub size_bytes: u64,
}

pub struct Database {
    dir: PathBuf,
    id: String,
    // Held for the lifetime of the database; dropping it releases the lock.
    _lock: File,
    idx: File,
    blob: File,
    names: [NameTable; 4],
    count: u32,
    blob_len: u64,
    sorts: HashMap<SortSpec, Arc<Vec<GameId>>>,
    /// Bumped when cached sort orders become invalid (game replaced).
    sort_epoch: u64,
    generation: u64,
    /// Bumped by each compaction, which renumbers the games: a position
    /// index built across one is stale (SRCH-04, DB-06).
    compactions: u64,
    positions: PositionIndex,
    stats: Option<Vec<GameStat>>,
    /// Games per name id (deleted games excluded), with the generation
    /// they were counted at.
    name_counts: HashMap<NameKind, (u64, Arc<Vec<u32>>)>,
    name_ranks: HashMap<NameKind, (u64, Arc<Vec<u32>>)>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").field("dir", &self.dir).field("count", &self.count).finish()
    }
}

fn random_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn lock_dir(dir: &Path) -> Result<File, DbError> {
    let lock =
        OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join(LOCK_FILE))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(std::fs::TryLockError::WouldBlock) => Err(DbError::Locked(dir.to_path_buf())),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Writes a whole small file atomically (temp file + rename).
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), DbError> {
    let tmp = path.with_extension("tmp");
    {
        let f = File::create(&tmp)?;
        write_all_at(&f, contents, 0)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn header(magic: &[u8; 8], size: u64) -> Vec<u8> {
    let mut h = Vec::with_capacity(size as usize);
    h.extend_from_slice(magic);
    h.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    if size >= 24 {
        h.extend_from_slice(&(RECORD_SIZE as u32).to_le_bytes());
    }
    h.resize(size as usize, 0);
    h
}

fn check_header(file: &File, magic: &[u8; 8], what: &Path) -> Result<(), DbError> {
    let mut buf = [0u8; 12];
    read_exact_at(file, &mut buf, 0)
        .map_err(|_| DbError::Corrupt(format!("{} is truncated", what.display())))?;
    if &buf[..8] != magic {
        return Err(DbError::Corrupt(format!("{} has a bad header", what.display())));
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().expect("4 bytes"));
    if version != FORMAT_VERSION {
        return Err(DbError::UnsupportedVersion(version));
    }
    Ok(())
}

impl Database {
    /// Creates a new, empty database directory. `path` should end in
    /// `.chessdb`; the extension is added if missing.
    pub fn create(path: &Path) -> Result<Database, DbError> {
        let dir = if path.extension().is_some_and(|e| e == EXTENSION) {
            path.to_path_buf()
        } else {
            let mut name = path.as_os_str().to_owned();
            name.push(".");
            name.push(EXTENSION);
            PathBuf::from(name)
        };
        if dir.exists() {
            return Err(DbError::AlreadyExists(dir));
        }
        std::fs::create_dir_all(&dir)?;
        let lock = lock_dir(&dir)?;
        let id = random_id();
        write_atomic(&dir.join(META_FILE), format!("chessdb {META_VERSION}\nid {id}\n").as_bytes())?;

        let create = |name: &str, contents: &[u8]| -> Result<File, DbError> {
            let f = OpenOptions::new().read(true).write(true).create_new(true).open(dir.join(name))?;
            write_all_at(&f, contents, 0)?;
            f.sync_all()?;
            Ok(f)
        };
        let idx = create(IDX_FILE, &header(IDX_MAGIC, IDX_HEADER))?;
        let blob = create(BLOB_FILE, &header(BLOB_MAGIC, BLOB_HEADER))?;
        let names = [
            NameTable::create(&dir.join(NAME_FILES[0]))?,
            NameTable::create(&dir.join(NAME_FILES[1]))?,
            NameTable::create(&dir.join(NAME_FILES[2]))?,
            NameTable::create(&dir.join(NAME_FILES[3]))?,
        ];
        let positions = PositionIndex::open(&dir)?;
        Ok(Database {
            dir,
            id,
            _lock: lock,
            idx,
            blob,
            names,
            count: 0,
            blob_len: BLOB_HEADER,
            sorts: HashMap::new(),
            sort_epoch: 0,
            generation: 0,
            compactions: 0,
            positions,
            stats: None,
            name_counts: HashMap::new(),
            name_ranks: HashMap::new(),
        })
    }

    pub fn open(path: &Path) -> Result<Database, DbError> {
        let dir = path.to_path_buf();
        let meta =
            std::fs::read_to_string(dir.join(META_FILE)).map_err(|_| DbError::NotADatabase(dir.clone()))?;
        let mut lines = meta.lines();
        let version = lines
            .next()
            .and_then(|l| l.strip_prefix("chessdb "))
            .and_then(|v| v.trim().parse::<u32>().ok())
            .ok_or_else(|| DbError::NotADatabase(dir.clone()))?;
        if !(1..=META_VERSION).contains(&version) {
            return Err(DbError::UnsupportedVersion(version));
        }
        let id = lines.find_map(|l| l.strip_prefix("id ")).unwrap_or_default().trim().to_owned();

        let lock = lock_dir(&dir)?;
        recover_compaction(&dir)?;
        if version < META_VERSION {
            // Version 1 databases have no position index yet: nothing else
            // changes.
            log::info!("upgrading {} to format version {META_VERSION}", dir.display());
            write_atomic(&dir.join(META_FILE), format!("chessdb {META_VERSION}\nid {id}\n").as_bytes())?;
        }
        let open = |name: &str| OpenOptions::new().read(true).write(true).open(dir.join(name));
        let idx = open(IDX_FILE)?;
        let blob = open(BLOB_FILE)?;
        check_header(&idx, IDX_MAGIC, &dir.join(IDX_FILE))?;
        check_header(&blob, BLOB_MAGIC, &dir.join(BLOB_FILE))?;

        let mut count_buf = [0u8; 8];
        read_exact_at(&idx, &mut count_buf, IDX_COUNT_OFFSET)?;
        let count = u64_at(&count_buf, 0);
        let expected_len = IDX_HEADER + count * RECORD_SIZE as u64;
        let idx_len = idx.metadata()?.len();
        if idx_len < expected_len {
            return Err(DbError::Corrupt(format!(
                "index holds {} records but header says {count}",
                (idx_len - IDX_HEADER) / RECORD_SIZE as u64
            )));
        }
        if idx_len > expected_len {
            log::warn!("dropping uncommitted index records in {}", dir.display());
            idx.set_len(expected_len)?;
        }
        let count = u32::try_from(count).map_err(|_| DbError::Corrupt("game count too large".into()))?;
        let blob_len = blob.metadata()?.len();
        for name in NAME_FILES {
            // Left over from a rename interrupted before its swap.
            let _ = std::fs::remove_file(dir.join(format!("{name}.tmp")));
        }
        let names = [
            NameTable::open(&dir.join(NAME_FILES[0]))?,
            NameTable::open(&dir.join(NAME_FILES[1]))?,
            NameTable::open(&dir.join(NAME_FILES[2]))?,
            NameTable::open(&dir.join(NAME_FILES[3]))?,
        ];
        let positions = PositionIndex::open(&dir)?;
        Ok(Database {
            dir,
            id,
            _lock: lock,
            idx,
            blob,
            names,
            count,
            blob_len,
            sorts: HashMap::new(),
            sort_epoch: 0,
            generation: 0,
            compactions: 0,
            positions,
            stats: None,
            name_counts: HashMap::new(),
            name_ranks: HashMap::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn name(&self) -> String {
        self.dir.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    /// Increases on every change (append, replace, delete flag).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// How many times this handle compacted the database (see
    /// [`update_position_index`](crate::update_position_index)).
    pub fn compactions(&self) -> u64 {
        self.compactions
    }

    pub fn names(&self, kind: NameKind) -> &NameTable {
        &self.names[kind as usize]
    }

    /// The position index (possibly not built yet).
    pub fn positions(&self) -> &PositionIndex {
        &self.positions
    }

    pub(crate) fn positions_mut(&mut self) -> &mut PositionIndex {
        &mut self.positions
    }

    pub fn info(&self) -> DbInfo {
        let size_bytes = std::fs::read_dir(&self.dir)
            .map(|rd| rd.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum())
            .unwrap_or(0);
        DbInfo {
            name: self.name(),
            path: self.dir.clone(),
            id: self.id.clone(),
            games: self.count,
            players: self.names[NameKind::Player as usize].len() as u32 - 1,
            events: self.names[NameKind::Event as usize].len() as u32 - 1,
            size_bytes,
        }
    }

    // ---- writing ----------------------------------------------------------

    fn make_record(&mut self, game: &PreparedGame, blob_offset: u64) -> IndexRecord {
        let h = &game.headers;
        let (round, subround) = pack_round(&h.round);
        IndexRecord {
            blob_offset,
            blob_len: game.blob.len() as u32,
            flags: game.flags,
            white: self.names[NameKind::Player as usize].intern(&h.white),
            black: self.names[NameKind::Player as usize].intern(&h.black),
            event: self.names[NameKind::Event as usize].intern(&h.event),
            site: self.names[NameKind::Site as usize].intern(&h.site),
            annotator: self.names[NameKind::Annotator as usize].intern(&h.annotator),
            date: pack_date(&h.date),
            round,
            subround,
            white_elo: h.white_elo.unwrap_or(0),
            black_elo: h.black_elo.unwrap_or(0),
            result: result_to_u8(h.result),
            eco: pack_eco(&h.eco),
            plies: game.plies,
        }
    }

    fn write_blobs(&mut self, blobs: &[&[u8]]) -> Result<Vec<u64>, DbError> {
        let mut offsets = Vec::with_capacity(blobs.len());
        let mut buf = Vec::with_capacity(blobs.iter().map(|b| b.len()).sum());
        for b in blobs {
            offsets.push(self.blob_len + buf.len() as u64);
            buf.extend_from_slice(b);
        }
        write_all_at(&self.blob, &buf, self.blob_len)?;
        self.blob.sync_data()?;
        self.blob_len += buf.len() as u64;
        Ok(offsets)
    }

    fn flush_names(&mut self) -> Result<(), DbError> {
        for t in &mut self.names {
            t.flush()?;
        }
        Ok(())
    }

    fn write_count(&mut self, count: u32) -> Result<(), DbError> {
        write_all_at(&self.idx, &u64::from(count).to_le_bytes(), IDX_COUNT_OFFSET)?;
        self.idx.sync_data()?;
        self.count = count;
        Ok(())
    }

    /// Appends prepared games in one crash-safe commit (the batch path used
    /// by imports).
    pub fn append_prepared(&mut self, games: &[PreparedGame]) -> Result<std::ops::Range<GameId>, DbError> {
        let first = self.count;
        if games.is_empty() {
            return Ok(first..first);
        }
        let blobs: Vec<&[u8]> = games.iter().map(|g| g.blob.as_slice()).collect();
        let offsets = self.write_blobs(&blobs)?;
        let mut records = Vec::with_capacity(games.len() * RECORD_SIZE);
        let mut new_stats = Vec::with_capacity(games.len());
        for (game, offset) in games.iter().zip(offsets) {
            let rec = self.make_record(game, offset);
            new_stats.push(GameStat::of(&rec));
            records.extend_from_slice(&rec.encode());
        }
        self.flush_names()?;
        write_all_at(&self.idx, &records, IDX_HEADER + u64::from(first) * RECORD_SIZE as u64)?;
        self.idx.sync_data()?;
        self.write_count(first + games.len() as u32)?;
        if let Some(stats) = &mut self.stats {
            stats.extend(new_stats);
        }
        self.generation += 1;
        Ok(first..self.count)
    }

    pub fn append(&mut self, game: &Game) -> Result<GameId, DbError> {
        Ok(self.append_prepared(&[PreparedGame::new(game)])?.start)
    }

    /// Replaces a game: the new blob is appended and the record repointed.
    /// The deleted flag is kept.
    pub fn replace(&mut self, id: GameId, game: &Game) -> Result<(), DbError> {
        let old = self.record(id)?;
        let prepared = PreparedGame::new(game);
        let offset = self.write_blobs(&[&prepared.blob])?[0];
        let mut rec = self.make_record(&prepared, offset);
        rec.flags |= old.flags & FLAG_DELETED;
        self.flush_names()?;
        // Before the record points at the new game: the index must never
        // trust entries of a changed game.
        self.positions.mark_dirty(id)?;
        self.write_record(id, &rec)?;
        // Header edits may change any sort order.
        self.sorts.clear();
        self.sort_epoch += 1;
        self.generation += 1;
        Ok(())
    }

    pub fn set_deleted(&mut self, id: GameId, deleted: bool) -> Result<(), DbError> {
        let mut rec = self.record(id)?;
        if rec.is_deleted() == deleted {
            return Ok(());
        }
        rec.flags ^= FLAG_DELETED;
        self.write_record(id, &rec)?;
        self.generation += 1;
        Ok(())
    }

    fn write_record(&mut self, id: GameId, rec: &IndexRecord) -> Result<(), DbError> {
        write_all_at(&self.idx, &rec.encode(), IDX_HEADER + u64::from(id) * RECORD_SIZE as u64)?;
        self.idx.sync_data()?;
        if let Some(stat) = self.stats.as_mut().and_then(|s| s.get_mut(id as usize)) {
            *stat = GameStat::of(rec);
        }
        Ok(())
    }

    // ---- reading ----------------------------------------------------------

    pub fn record(&self, id: GameId) -> Result<IndexRecord, DbError> {
        if id >= self.count {
            return Err(DbError::NotFound(id));
        }
        let mut buf = [0u8; RECORD_SIZE];
        read_exact_at(&self.idx, &mut buf, IDX_HEADER + u64::from(id) * RECORD_SIZE as u64)?;
        Ok(IndexRecord::decode(&buf))
    }

    /// Records `start..start+len` (clamped to the game count) in one read.
    pub fn records(&self, start: GameId, len: u32) -> Result<Vec<IndexRecord>, DbError> {
        let end = start.saturating_add(len).min(self.count);
        if start >= end {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; (end - start) as usize * RECORD_SIZE];
        read_exact_at(&self.idx, &mut buf, IDX_HEADER + u64::from(start) * RECORD_SIZE as u64)?;
        Ok(buf.as_chunks::<RECORD_SIZE>().0.iter().map(|c| IndexRecord::decode(c)).collect())
    }

    /// Number of (non-deleted) games per id of a name table; for players,
    /// games with either colour. Cached until the database changes.
    pub fn name_counts(&mut self, kind: NameKind) -> Result<Arc<Vec<u32>>, DbError> {
        if let Some((generation, counts)) = self.name_counts.get(&kind)
            && *generation == self.generation
        {
            return Ok(counts.clone());
        }
        let mut counts = vec![0u32; self.names[kind as usize].len()];
        let mut bump = |id: u32| {
            if let Some(c) = counts.get_mut(id as usize) {
                *c += 1;
            }
        };
        let mut start = 0;
        while start < self.count {
            for r in self.records(start, 64 * 1024)?.iter().filter(|r| !r.is_deleted()) {
                match kind {
                    NameKind::Player => {
                        bump(r.white);
                        if r.black != r.white {
                            bump(r.black);
                        }
                    }
                    NameKind::Event => bump(r.event),
                    NameKind::Site => bump(r.site),
                    NameKind::Annotator => bump(r.annotator),
                }
            }
            start += 64 * 1024;
        }
        let counts = Arc::new(counts);
        self.name_counts.insert(kind, (self.generation, counts.clone()));
        Ok(counts)
    }

    /// Alphabetical rank of every name id (see [`NameTable::ranks`]), cached
    /// until the database changes.
    pub fn name_ranks(&mut self, kind: NameKind) -> Arc<Vec<u32>> {
        if let Some((generation, ranks)) = self.name_ranks.get(&kind)
            && *generation == self.generation
            && ranks.len() == self.names[kind as usize].len()
        {
            return ranks.clone();
        }
        let ranks = Arc::new(self.names[kind as usize].ranks());
        self.name_ranks.insert(kind, (self.generation, ranks.clone()));
        ranks
    }

    /// Renames a player, event, site or annotator in every game at once
    /// (DB-04). If `new_name` already exists, the two merge: games using
    /// `id` are repointed to it. Returns the number of games repointed.
    pub fn rename_name(&mut self, kind: NameKind, id: u32, new_name: &str) -> Result<u64, DbError> {
        let new_name = new_name.trim();
        let table = &mut self.names[kind as usize];
        if id == 0 || id as usize >= table.len() {
            return Err(DbError::Invalid("there is no such name".into()));
        }
        if new_name.is_empty() {
            return Err(DbError::Invalid("the new name is empty".into()));
        }
        let mut repointed = 0;
        match table.id_of(new_name) {
            Some(existing) if existing == id => return Ok(0),
            Some(existing) => {
                // Merge: rewrite the records that use `id`, chunk by chunk.
                const CHUNK: u32 = 64 * 1024;
                let mut start = 0;
                while start < self.count {
                    let mut recs = self.records(start, CHUNK)?;
                    let mut changed = false;
                    for r in &mut recs {
                        let fields: &mut [&mut u32] = match kind {
                            NameKind::Player => &mut [&mut r.white, &mut r.black],
                            NameKind::Event => &mut [&mut r.event],
                            NameKind::Site => &mut [&mut r.site],
                            NameKind::Annotator => &mut [&mut r.annotator],
                        };
                        for f in fields.iter_mut().filter(|f| ***f == id) {
                            **f = existing;
                            changed = true;
                            repointed += 1;
                        }
                    }
                    if changed {
                        let bytes: Vec<u8> = recs.iter().flat_map(|r| r.encode()).collect();
                        write_all_at(&self.idx, &bytes, IDX_HEADER + u64::from(start) * RECORD_SIZE as u64)?;
                    }
                    start += CHUNK;
                }
                self.idx.sync_data()?;
            }
            None => table.rename(id, new_name)?,
        }
        // Name order and cached lists changed.
        self.sorts.clear();
        self.sort_epoch += 1;
        self.generation += 1;
        Ok(repointed)
    }

    /// Compacts the database (DB-06): drops deleted games, the space old
    /// versions of edited games took, and names no game uses any more.
    /// Game ids change (see the mapping in the summary); the position index
    /// is deleted and needs rebuilding.
    ///
    /// Crash safety: the new files are written next to the old ones as
    /// `*.new` and fsynced; then `compact.commit` is written, the `.new`
    /// files are renamed over the old ones and the marker is removed. Opening
    /// a database finishes the renames if the marker exists, and deletes
    /// `.new` files otherwise.
    pub fn compact(&mut self, progress: &dyn crate::Progress) -> Result<CompactSummary, DbError> {
        const CHUNK: u32 = 16 * 1024;
        self.flush_names()?;
        recover_compaction(&self.dir)?;
        let bytes_before = self.info().size_bytes;
        let new_path = |name: &str| self.dir.join(format!("{name}{NEW_SUFFIX}"));
        let create = |path: &Path, contents: &[u8]| -> Result<File, DbError> {
            let f = OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
            write_all_at(&f, contents, 0)?;
            Ok(f)
        };
        let new_idx = create(&new_path(IDX_FILE), &header(IDX_MAGIC, IDX_HEADER))?;
        let new_blob = create(&new_path(BLOB_FILE), &header(BLOB_MAGIC, BLOB_HEADER))?;
        let mut new_names = [
            NameTable::create(&new_path(NAME_FILES[0]))?,
            NameTable::create(&new_path(NAME_FILES[1]))?,
            NameTable::create(&new_path(NAME_FILES[2]))?,
            NameTable::create(&new_path(NAME_FILES[3]))?,
        ];

        let mut mapping = Vec::with_capacity(self.count as usize);
        let mut blob_len = BLOB_HEADER;
        let mut kept = 0u32;
        let mut start = 0;
        let result = (|| -> Result<(), DbError> {
            while start < self.count {
                progress.check_cancelled()?;
                let recs = self.records(start, CHUNK)?;
                let mut blobs = Vec::new();
                let mut out_recs = Vec::new();
                for r in &recs {
                    if r.is_deleted() {
                        mapping.push(None);
                        continue;
                    }
                    let mut blob = vec![0u8; r.blob_len as usize];
                    read_exact_at(&self.blob, &mut blob, r.blob_offset)?;
                    let mut n = *r;
                    n.blob_offset = blob_len + blobs.len() as u64;
                    blobs.extend_from_slice(&blob);
                    let remap = |t: &mut NameTable, old: &NameTable, id: u32| t.intern(old.get(id));
                    n.white = remap(&mut new_names[0], &self.names[0], r.white);
                    n.black = remap(&mut new_names[0], &self.names[0], r.black);
                    n.event = remap(&mut new_names[1], &self.names[1], r.event);
                    n.site = remap(&mut new_names[2], &self.names[2], r.site);
                    n.annotator = remap(&mut new_names[3], &self.names[3], r.annotator);
                    out_recs.extend_from_slice(&n.encode());
                    mapping.push(Some(kept));
                    kept += 1;
                }
                write_all_at(&new_blob, &blobs, blob_len)?;
                blob_len += blobs.len() as u64;
                let first_new = kept - (out_recs.len() / RECORD_SIZE) as u32;
                write_all_at(&new_idx, &out_recs, IDX_HEADER + u64::from(first_new) * RECORD_SIZE as u64)?;
                start = start.saturating_add(CHUNK);
                progress.report(u64::from(start.min(self.count)), u64::from(self.count));
            }
            write_all_at(&new_idx, &u64::from(kept).to_le_bytes(), IDX_COUNT_OFFSET)?;
            new_idx.sync_all()?;
            new_blob.sync_all()?;
            for t in &mut new_names {
                t.flush()?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            drop((new_idx, new_blob, new_names));
            recover_compaction(&self.dir)?;
            return Err(e);
        }

        // Commit: from here on, opening the database finishes the swap.
        write_atomic(&self.dir.join(COMPACT_MARKER), b"compact\n")?;
        // Swap handles first (closing the old files: Windows can't replace
        // open files), then the files.
        self.idx = new_idx;
        self.blob = new_blob;
        self.names = new_names;
        for name in [IDX_FILE, BLOB_FILE].into_iter().chain(NAME_FILES) {
            std::fs::rename(new_path(name), self.dir.join(name))?;
        }
        for (table, name) in self.names.iter_mut().zip(NAME_FILES) {
            table.set_path(&self.dir.join(name));
        }
        // The index refers to old game ids: close its files, then delete them.
        self.positions = PositionIndex::empty(&self.dir);
        self.compactions += 1;
        PositionIndex::remove_files(&self.dir)?;
        std::fs::remove_file(self.dir.join(COMPACT_MARKER))?;

        let removed = self.count - kept;
        self.count = kept;
        self.blob_len = blob_len;
        self.sorts.clear();
        self.sort_epoch += 1;
        self.generation += 1;
        self.stats = None;
        self.name_counts.clear();
        self.name_ranks.clear();
        let bytes_after = self.info().size_bytes;
        Ok(CompactSummary { kept, removed, bytes_before, bytes_after, mapping })
    }

    /// Per-game stats for every game, loaded on first use.
    pub fn game_stats(&mut self) -> Result<&[GameStat], DbError> {
        if self.stats.is_none() {
            let mut stats = Vec::with_capacity(self.count as usize);
            let mut start = 0;
            while start < self.count {
                stats.extend(self.records(start, 64 * 1024)?.iter().map(GameStat::of));
                start += 64 * 1024;
            }
            self.stats = Some(stats);
        }
        Ok(self.stats.as_deref().expect("just loaded"))
    }

    /// The encoded game (see [`crate::blob`]).
    pub fn read_blob(&self, id: GameId) -> Result<Vec<u8>, DbError> {
        let rec = self.record(id)?;
        let mut blob = vec![0u8; rec.blob_len as usize];
        read_exact_at(&self.blob, &mut blob, rec.blob_offset)?;
        Ok(blob)
    }

    pub fn read_game(&self, id: GameId) -> Result<Game, DbError> {
        let rec = self.record(id)?;
        let blob = self.read_blob(id)?;
        let mut game = decode_game(&blob)?;
        let h = &mut game.headers;
        h.white = self.names[NameKind::Player as usize].get(rec.white).to_owned();
        h.black = self.names[NameKind::Player as usize].get(rec.black).to_owned();
        h.event = self.names[NameKind::Event as usize].get(rec.event).to_owned();
        h.site = self.names[NameKind::Site as usize].get(rec.site).to_owned();
        h.annotator = self.names[NameKind::Annotator as usize].get(rec.annotator).to_owned();
        h.white_elo = (rec.white_elo != 0).then_some(rec.white_elo);
        h.black_elo = (rec.black_elo != 0).then_some(rec.black_elo);
        h.result = result_from_u8(rec.result);
        Ok(game)
    }

    pub fn row_from_record(&self, id: GameId, rec: &IndexRecord) -> GameRow {
        let player = &self.names[NameKind::Player as usize];
        GameRow {
            id,
            white: player.get(rec.white).to_owned(),
            white_elo: (rec.white_elo != 0).then_some(rec.white_elo),
            black: player.get(rec.black).to_owned(),
            black_elo: (rec.black_elo != 0).then_some(rec.black_elo),
            result: result_from_u8(rec.result),
            date: unpack_date(rec.date),
            event: self.names[NameKind::Event as usize].get(rec.event).to_owned(),
            site: self.names[NameKind::Site as usize].get(rec.site).to_owned(),
            round: unpack_round(rec.round, rec.subround),
            eco: unpack_eco(rec.eco),
            annotator: self.names[NameKind::Annotator as usize].get(rec.annotator).to_owned(),
            plies: rec.plies,
            deleted: rec.is_deleted(),
            has_variations: rec.flags & FLAG_VARIATIONS != 0,
            has_comments: rec.flags & FLAG_COMMENTS != 0,
            has_annotations: rec.flags & FLAG_ANNOTATIONS != 0,
            has_setup: rec.flags & FLAG_SETUP != 0,
            opening: chess_core::openings::by_id(self.positions.opening_of(id)),
        }
    }

    pub fn row(&self, id: GameId) -> Result<GameRow, DbError> {
        Ok(self.row_from_record(id, &self.record(id)?))
    }

    /// Rows for arbitrary ids (e.g. a page of a sorted list).
    pub fn rows(&self, ids: &[GameId]) -> Result<Vec<GameRow>, DbError> {
        // Contiguous ascending ids: one read.
        if let (Some(&first), Some(&last)) = (ids.first(), ids.last())
            && last >= first
            && (last - first) as usize + 1 == ids.len()
            && ids.windows(2).all(|w| w[1] == w[0] + 1)
        {
            let recs = self.records(first, ids.len() as u32)?;
            return Ok(recs.iter().zip(first..).map(|(r, id)| self.row_from_record(id, r)).collect());
        }
        ids.iter().map(|&id| self.row(id)).collect()
    }

    // ---- sorting ----------------------------------------------------------

    /// Changes whenever cached sort orders are dropped (a game was replaced).
    pub fn sort_epoch(&self) -> u64 {
        self.sort_epoch
    }

    /// True if the permutation for `spec` is built (or not needed).
    pub fn has_sort(&self, spec: SortSpec) -> bool {
        spec.is_natural_order() || self.sorts.contains_key(&spec)
    }

    pub(crate) fn cached_sort(&self, spec: SortSpec) -> Option<Arc<Vec<GameId>>> {
        self.sorts.get(&spec).cloned()
    }

    pub(crate) fn store_sort(&mut self, spec: SortSpec, perm: Arc<Vec<GameId>>) {
        self.sorts.insert(spec, perm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::pgn::read_pgn_str;

    fn sample_games() -> Vec<Game> {
        read_pgn_str(
            r#"[Event "Club"]
[White "Alice"]
[Black "Bob"]
[Result "1-0"]
[Date "2024.05.06"]
[Round "2.1"]
[WhiteElo "1800"]
[ECO "C20"]

1. e4 e5 (1... c5) 2. Qh5 {threat} Nc6 3. Bc4 Nf6 4. Qxf7# 1-0

[Event "Club"]
[White "Bob"]
[Black "Carol"]
[Result "1/2-1/2"]

1. d4 d5 1/2-1/2
"#,
        )
    }

    #[test]
    fn create_append_read_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let games = sample_games();
        let path;
        {
            let mut db = Database::create(&dir.path().join("Test")).unwrap();
            path = db.path().to_path_buf();
            assert_eq!(path.extension().unwrap(), "chessdb");
            for g in &games {
                db.append(g).unwrap();
            }
            assert_eq!(db.count(), 2);
            assert_eq!(&db.read_game(0).unwrap(), &games[0]);
        }
        let db = Database::open(&path).unwrap();
        assert_eq!(db.count(), 2);
        assert_eq!(&db.read_game(0).unwrap(), &games[0]);
        assert_eq!(&db.read_game(1).unwrap(), &games[1]);
        let row = db.row(0).unwrap();
        assert_eq!(row.white, "Alice");
        assert_eq!(row.white_elo, Some(1800));
        assert_eq!(row.date, "2024.05.06");
        assert_eq!(row.round, "2.1");
        assert_eq!(row.eco, "C20");
        assert_eq!(row.plies, 7);
        assert!(row.has_variations && row.has_comments);
        assert_eq!(db.info().players, 3);
        assert!(matches!(db.read_game(2), Err(DbError::NotFound(2))));
    }

    #[test]
    fn lock_prevents_second_open() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(&dir.path().join("Locked.chessdb")).unwrap();
        assert!(matches!(Database::open(db.path()), Err(DbError::Locked(_))));
        let path = db.path().to_path_buf();
        drop(db);
        assert!(Database::open(&path).is_ok());
    }

    #[test]
    fn replace_and_soft_delete() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Edit")).unwrap();
        let games = sample_games();
        db.append(&games[0]).unwrap();
        db.set_deleted(0, true).unwrap();
        let mut edited = games[1].clone();
        edited.headers.white = "Dave".into();
        db.replace(0, &edited).unwrap();
        let path = db.path().to_path_buf();
        drop(db);

        let db = Database::open(&path).unwrap();
        assert_eq!(db.read_game(0).unwrap(), edited);
        let row = db.row(0).unwrap();
        assert!(row.deleted, "replace keeps the deleted flag");
        assert_eq!(row.white, "Dave");
    }

    #[test]
    fn rename_and_merge_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Names")).unwrap();
        for g in sample_games() {
            db.append(&g).unwrap();
        }
        let bob = db.names(NameKind::Player).id_of("Bob").unwrap();
        assert_eq!(db.name_counts(NameKind::Player).unwrap()[bob as usize], 2);
        // A plain rename: one table entry changes, every game follows.
        assert_eq!(db.rename_name(NameKind::Player, bob, "Bob Smith").unwrap(), 0);
        assert_eq!(db.row(0).unwrap().black, "Bob Smith");
        assert_eq!(db.row(1).unwrap().white, "Bob Smith");
        // Renaming onto an existing name merges the two.
        let carol = db.names(NameKind::Player).id_of("Carol").unwrap();
        assert_eq!(db.rename_name(NameKind::Player, carol, "Bob Smith").unwrap(), 1);
        assert_eq!(db.row(1).unwrap().black, "Bob Smith");
        let counts = db.name_counts(NameKind::Player).unwrap();
        assert_eq!((counts[bob as usize], counts[carol as usize]), (2, 0));
        assert!(matches!(db.rename_name(NameKind::Player, 0, "x"), Err(DbError::Invalid(_))));
        let path = db.path().to_path_buf();
        drop(db);
        let db = Database::open(&path).unwrap();
        assert_eq!(db.row(1).unwrap().black, "Bob Smith");
        assert_eq!(db.row(0).unwrap().black, "Bob Smith");
    }

    #[test]
    fn compaction_drops_deleted_games_and_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Pack")).unwrap();
        let games = sample_games();
        for g in &games {
            db.append(g).unwrap();
        }
        db.append(&games[0]).unwrap();
        db.set_deleted(1, true).unwrap();
        let mut edited = games[0].clone();
        edited.headers.white = "Zoe".into();
        db.replace(0, &edited).unwrap(); // leaves the old blob behind
        let before = std::fs::metadata(db.path().join(BLOB_FILE)).unwrap().len();
        let s = db.compact(&crate::NoProgress).unwrap();
        assert_eq!((s.kept, s.removed), (2, 1));
        assert_eq!(s.mapping, [Some(0), None, Some(1)]);
        assert!(std::fs::metadata(db.path().join(BLOB_FILE)).unwrap().len() < before);
        assert_eq!(db.count(), 2);
        assert_eq!(db.read_game(0).unwrap(), edited);
        assert_eq!(db.read_game(1).unwrap(), games[0]);
        // "Carol" only played in the deleted game: the name is gone.
        assert_eq!(db.names(NameKind::Player).id_of("Carol"), None);
        // Writing after a compaction works, and it all survives a reopen.
        db.append(&games[1]).unwrap();
        let path = db.path().to_path_buf();
        drop(db);
        let db = Database::open(&path).unwrap();
        assert_eq!(db.count(), 3);
        assert_eq!(db.read_game(2).unwrap(), games[1]);
        assert!(!path.join(COMPACT_MARKER).exists());
    }

    #[test]
    fn interrupted_compaction_is_finished_or_undone_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Crash")).unwrap();
        for g in sample_games() {
            db.append(&g).unwrap();
        }
        let path = db.path().to_path_buf();
        drop(db);
        // Crash before the commit: stray .new files are deleted.
        std::fs::write(path.join("games.idx.new"), b"partial").unwrap();
        let db = Database::open(&path).unwrap();
        assert_eq!(db.count(), 2);
        assert!(!path.join("games.idx.new").exists());
        drop(db);
        // Crash after the commit: the .new files replace the old ones.
        let files = [IDX_FILE, BLOB_FILE].into_iter().chain(NAME_FILES);
        let copy = tempfile::tempdir().unwrap();
        {
            let mut other = Database::create(&copy.path().join("Other")).unwrap();
            other.append(&sample_games()[1]).unwrap();
            for name in files.clone() {
                std::fs::copy(other.path().join(name), path.join(format!("{name}{NEW_SUFFIX}"))).unwrap();
            }
        }
        std::fs::write(path.join(COMPACT_MARKER), b"compact\n").unwrap();
        let db = Database::open(&path).unwrap();
        assert_eq!(db.count(), 1);
        assert_eq!(db.row(0).unwrap().white, "Bob");
        assert!(!path.join(COMPACT_MARKER).exists());
    }

    #[test]
    fn create_refuses_existing_and_open_refuses_non_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::create(&dir.path().join("A")).unwrap();
        let path = db.path().to_path_buf();
        drop(db);
        assert!(matches!(Database::create(&path), Err(DbError::AlreadyExists(_))));
        assert!(matches!(Database::open(dir.path()), Err(DbError::NotADatabase(_))));
    }
}
