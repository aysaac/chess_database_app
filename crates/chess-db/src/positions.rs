//! Position index (SRCH-04, ChessBase's "Search Booster").
//!
//! For every game, the Polyglot hash of each main-line position up to
//! `max_ply`, with the move played next and the result, sorted by hash: all
//! games that reach a position, by any move order, are one binary search
//! away. It powers position search (SRCH-03) and the opening tree (OPEN-01).
//! The same pass classifies each game's opening (OPEN-11).
//!
//! Derived data, rebuildable at any time (see `docs/file-format.md`):
//!
//! - `positions-<gen>.idx` segments, each covering a range of game ids.
//!   Imports add a segment for the new games; after a few segments, or
//!   many edited games, the next update rebuilds one segment for all.
//! - `positions.dirty`: ids of games replaced since their segment was built
//!   (appended before the game's record changes). Their entries are
//!   ignored and the games are replayed at query time instead, as are games
//!   not covered by any segment yet.
//! - `openings.idx`: the opening id of each game.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chess_core::openings::{self, Classifier};
use chess_core::position;
use chess_core::shakmaty::{Chess, Move, Position};

use crate::blob::{mainline, move_at};
use crate::bytes::{u16_at, u32_at, u64_at};
use crate::io::{read_exact_at, write_all_at};
use crate::record::IndexRecord;
use crate::{Database, DbError, GameId, GameSet, GameStat, Progress};

/// Plies indexed per game (25 moves): the opening and early middlegame.
pub const DEFAULT_MAX_PLY: u16 = 50;

const MAGIC: &[u8; 8] = b"CHDBPOS1";
const VERSION: u32 = 1;
const HEADER: u64 = 64;
const ENTRY: usize = 16;
/// One fence (the key of every n-th entry) per block, kept in memory.
const BLOCK: u64 = 512;
/// Entries sorted in memory before spilling a run to disk (64 MB).
const RUN_ENTRIES: usize = 4 << 20;
/// More segments than this, or more dirty games, and the next update
/// rebuilds the whole index.
const MAX_SEGMENTS: usize = 6;
const MAX_DIRTY: usize = 4096;
/// Games replayed at query time (dirty or not indexed yet) at most;
/// beyond that a report is marked partial.
const REPLAY_LIMIT: usize = 20_000;
const BATCH: u32 = 4096;

const DIRTY_FILE: &str = "positions.dirty";
const OPENINGS_FILE: &str = "openings.idx";
const OPENINGS_MAGIC: &[u8; 8] = b"CHDBOPN1";

/// `next` for a position where the game ends.
pub const NO_MOVE: u8 = 0xFF;

/// One position of one game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PosEntry {
    pub key: u64,
    pub game: GameId,
    pub ply: u16,
    /// Move byte (index in the sorted legal moves) played next, or [`NO_MOVE`].
    pub next: u8,
    /// Result as stored in the index record (0 unknown, 1 White, 2 Black, 3 draw).
    pub result: u8,
}

impl PosEntry {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.key.to_le_bytes());
        out.extend_from_slice(&self.game.to_le_bytes());
        out.extend_from_slice(&self.ply.to_le_bytes());
        out.push(self.next);
        out.push(self.result);
    }

    fn decode(b: &[u8]) -> PosEntry {
        PosEntry { key: u64_at(b, 0), game: u32_at(b, 8), ply: u16_at(b, 12), next: b[14], result: b[15] }
    }
}

struct Segment {
    path: PathBuf,
    file: File,
    generation: u64,
    first: GameId,
    end: GameId,
    count: u64,
    max_ply: u16,
    fences: Vec<u64>,
}

impl Segment {
    fn open(path: &Path, generation: u64) -> Result<Segment, DbError> {
        let file = File::open(path)?;
        let bad = || DbError::Corrupt(format!("{} is not a position index", path.display()));
        let mut h = [0u8; HEADER as usize];
        read_exact_at(&file, &mut h, 0).map_err(|_| bad())?;
        if &h[..8] != MAGIC || u32_at(&h, 8) != VERSION || u32_at(&h, 12) as usize != ENTRY {
            return Err(bad());
        }
        let (max_ply, first, end, count) =
            (u32_at(&h, 16) as u16, u32_at(&h, 20), u32_at(&h, 24), u64_at(&h, 28));
        let blocks = count.div_ceil(BLOCK);
        let fence_at = HEADER + count * ENTRY as u64;
        if file.metadata()?.len() != fence_at + blocks * 8 || first > end {
            return Err(bad());
        }
        let mut buf = vec![0u8; blocks as usize * 8];
        read_exact_at(&file, &mut buf, fence_at)?;
        let fences = buf.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect();
        Ok(Segment { path: path.to_path_buf(), file, generation, first, end, count, max_ply, fences })
    }

    fn lookup(&self, key: u64, out: &mut Vec<PosEntry>) -> Result<(), DbError> {
        let start = self.fences.partition_point(|&f| f < key).saturating_sub(1) as u64;
        let stop = self.fences.partition_point(|&f| f <= key) as u64;
        if stop == 0 {
            return Ok(());
        }
        let (from, to) = (start * BLOCK, (stop * BLOCK).min(self.count));
        let mut buf = vec![0u8; (to - from) as usize * ENTRY];
        read_exact_at(&self.file, &mut buf, HEADER + from * ENTRY as u64)?;
        out.extend(buf.as_chunks::<ENTRY>().0.iter().map(|c| PosEntry::decode(c)).filter(|e| e.key == key));
        Ok(())
    }
}

/// The index of one database; see the module docs.
pub struct PositionIndex {
    dir: PathBuf,
    segments: Vec<Segment>,
    dirty: Vec<GameId>,
    dirty_set: GameSet,
    /// Opening id per game (0 = none), for the ids classified so far.
    openings: Vec<u16>,
}

/// What the next update will do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPlan {
    UpToDate,
    /// Index the games from this id on.
    Append(GameId),
    Rebuild,
}

fn segment_generation(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("positions-")?.strip_suffix(".idx")?.parse().ok()
}

impl PositionIndex {
    /// Loads the index files of the database in `dir`. Leftovers of an
    /// interrupted build or merge are cleaned up here.
    pub(crate) fn open(dir: &Path) -> Result<PositionIndex, DbError> {
        let mut segments = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if (name.starts_with("positions") || name.starts_with("openings")) && name.ends_with(".tmp") {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if let Some(generation) = segment_generation(&path) {
                match Segment::open(&path, generation) {
                    Ok(seg) => segments.push(seg),
                    Err(e) => {
                        log::warn!("dropping unreadable {}: {e}", path.display());
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
        // Newest first: keep segments that don't overlap a newer one (a
        // rebuild supersedes what it covers), then require coverage from 0.
        segments.sort_by_key(|s| std::cmp::Reverse(s.generation));
        let mut kept: Vec<Segment> = Vec::new();
        for seg in segments {
            if kept.iter().any(|k| seg.first < k.end && k.first < seg.end) {
                let _ = std::fs::remove_file(&seg.path);
            } else {
                kept.push(seg);
            }
        }
        kept.sort_by_key(|s| s.first);
        let mut covered = 0;
        let mut contiguous = Vec::new();
        for seg in kept {
            if seg.first == covered
                && seg.max_ply == contiguous.first().map_or(seg.max_ply, |s: &Segment| s.max_ply)
            {
                covered = seg.end;
                contiguous.push(seg);
            } else {
                log::warn!("dropping position index segment {} (gap)", seg.path.display());
                let _ = std::fs::remove_file(&seg.path);
            }
        }

        let dirty: Vec<GameId> = match std::fs::read(dir.join(DIRTY_FILE)) {
            Ok(bytes) => bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect(),
            Err(_) => Vec::new(),
        };
        let openings = match std::fs::read(dir.join(OPENINGS_FILE)) {
            Ok(b)
                if b.len() >= 16
                    && &b[..8] == OPENINGS_MAGIC
                    && u32_at(&b, 8) == openings::data_version() =>
            {
                b[16..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect()
            }
            _ => Vec::new(),
        };
        Ok(PositionIndex {
            dir: dir.to_path_buf(),
            dirty_set: GameSet::from_ids(&dirty),
            dirty,
            segments: contiguous,
            openings,
        })
    }

    /// An index with nothing built yet.
    pub(crate) fn empty(dir: &Path) -> PositionIndex {
        PositionIndex {
            dir: dir.to_path_buf(),
            segments: Vec::new(),
            dirty: Vec::new(),
            dirty_set: GameSet::default(),
            openings: Vec::new(),
        }
    }

    /// Deletes every index file of the database in `dir` (after a
    /// compaction renumbered the games).
    pub(crate) fn remove_files(dir: &Path) -> Result<(), DbError> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.starts_with("positions") || name.starts_with("openings") {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    // A build running outside the lock may hold its temp files
                    // open (Windows can't delete those); it removes them when it
                    // finds it was superseded.
                    Err(_) if name.ends_with(".tmp") => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }

    /// Games `0..covered()` are in the index.
    pub fn covered(&self) -> GameId {
        self.segments.last().map_or(0, |s| s.end)
    }

    pub fn max_ply(&self) -> u16 {
        self.segments.first().map_or(DEFAULT_MAX_PLY, |s| s.max_ply)
    }

    pub fn is_built(&self) -> bool {
        !self.segments.is_empty()
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    pub fn size_bytes(&self) -> u64 {
        self.segments.iter().map(|s| HEADER + s.count * ENTRY as u64 + s.fences.len() as u64 * 8).sum()
    }

    pub fn plan(&self, games: GameId) -> IndexPlan {
        if self.segments.is_empty() && games == 0 {
            return IndexPlan::UpToDate;
        }
        if self.segments.is_empty()
            || self.segments.len() >= MAX_SEGMENTS
            || self.dirty.len() > MAX_DIRTY
            || (self.openings.len() as u32) < self.covered()
        {
            return IndexPlan::Rebuild;
        }
        if self.covered() < games { IndexPlan::Append(self.covered()) } else { IndexPlan::UpToDate }
    }

    /// Opening id of a game (0 = not classified).
    pub fn opening_of(&self, game: GameId) -> u16 {
        self.openings.get(game as usize).copied().unwrap_or(0)
    }

    /// Records that `game` is about to change: its entries become stale.
    pub(crate) fn mark_dirty(&mut self, game: GameId) -> Result<(), DbError> {
        self.mark_dirty_many(&[game])
    }

    /// [`mark_dirty`](Self::mark_dirty) for several games, with one fsync.
    pub(crate) fn mark_dirty_many(&mut self, games: &[GameId]) -> Result<(), DbError> {
        let mut new: Vec<GameId> =
            games.iter().copied().filter(|&g| g < self.covered() && !self.dirty_set.contains(g)).collect();
        new.sort_unstable();
        new.dedup();
        if new.is_empty() {
            return Ok(());
        }
        let mut f = OpenOptions::new().create(true).append(true).open(self.dir.join(DIRTY_FILE))?;
        let bytes: Vec<u8> = new.iter().flat_map(|g| g.to_le_bytes()).collect();
        std::io::Write::write_all(&mut f, &bytes)?;
        f.sync_data()?;
        for game in new {
            self.dirty.push(game);
            self.dirty_set.insert(game);
        }
        Ok(())
    }

    /// Entries for `key` from the segments, without dirty games.
    fn lookup(&self, key: u64) -> Result<Vec<PosEntry>, DbError> {
        let mut out = Vec::new();
        for seg in &self.segments {
            seg.lookup(key, &mut out)?;
        }
        out.retain(|e| !self.dirty_set.contains(e.game));
        Ok(out)
    }
}

/// The positions of one game's main line, up to `max_ply` (all plies if
/// `None`), and the game's opening.
fn game_entries(
    id: GameId,
    rec: &IndexRecord,
    blob: &[u8],
    max_ply: Option<u16>,
    out: &mut Vec<PosEntry>,
) -> Result<u16, DbError> {
    let (setup, bytes) = mainline(blob)?;
    let mut pos = setup.unwrap_or_default();
    let mut classifier = Classifier::default();
    let last = max_ply.map_or(bytes.len(), |m| bytes.len().min(usize::from(m)));
    for ply in 0..=last {
        let key = position::hash(&pos);
        if ply <= openings::CLASSIFY_PLIES {
            classifier.see(key);
        }
        let next = bytes.get(ply).copied().unwrap_or(NO_MOVE);
        out.push(PosEntry { key, game: id, ply: ply as u16, next, result: rec.result });
        if ply == last {
            break;
        }
        let m = move_at(&pos, next).ok_or_else(|| DbError::Corrupt(format!("game {id}: bad move")))?;
        pos.play_unchecked(m);
    }
    Ok(classifier.result().map_or(0, |o| o.id))
}

/// Reads games `ids` in batches and runs `work` on each one in parallel.
/// `sink` gets each batch's results in id order.
pub(crate) fn for_each_game<T: Send>(
    db: &Mutex<Database>,
    ids: &[GameId],
    progress: &dyn Progress,
    work: &(dyn Fn(GameId, &IndexRecord, &[u8]) -> T + Sync),
    sink: &mut dyn FnMut(Vec<T>) -> Result<(), DbError>,
) -> Result<(), DbError> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let total = ids.len() as u64;
    for (i, batch) in ids.chunks(BATCH as usize).enumerate() {
        progress.check_cancelled()?;
        let raw: Vec<(GameId, IndexRecord, Vec<u8>)> = {
            let db = db.lock().expect("db lock");
            batch
                .iter()
                .map(|&id| {
                    let rec = db.record(id)?;
                    Ok((id, rec, db.blob_of(&rec)?))
                })
                .collect::<Result<_, DbError>>()?
        };
        let chunk = raw.len().div_ceil(threads).max(1);
        let results: Vec<Vec<T>> = std::thread::scope(|s| {
            let handles: Vec<_> = raw
                .chunks(chunk)
                .map(|part| {
                    s.spawn(move || part.iter().map(|(id, rec, blob)| work(*id, rec, blob)).collect())
                })
                .collect();
            handles.into_iter().map(|h| h.join().expect("index worker panicked")).collect()
        });
        sink(results.into_iter().flatten().collect())?;
        progress.report(((i as u64 + 1) * u64::from(BATCH)).min(total), total);
    }
    Ok(())
}

fn write_run(path: &Path, entries: &mut [PosEntry]) -> Result<(), DbError> {
    entries.sort_unstable();
    let mut buf = Vec::with_capacity(entries.len() * ENTRY);
    for e in entries.iter() {
        e.encode(&mut buf);
    }
    std::fs::write(path, buf)?;
    Ok(())
}

/// Sorted runs → one segment file: header, entries, fences.
fn merge_runs(
    runs: &[PathBuf],
    last: &mut [PosEntry],
    out: &Path,
    first: GameId,
    end: GameId,
    max_ply: u16,
) -> Result<(), DbError> {
    use std::io::{BufReader, BufWriter, Read, Write};
    last.sort_unstable();
    let mut readers: Vec<BufReader<File>> = runs
        .iter()
        .map(|p| File::open(p).map(|f| BufReader::with_capacity(1 << 20, f)))
        .collect::<Result<_, _>>()?;
    let next_of = |r: &mut BufReader<File>| -> Result<Option<PosEntry>, DbError> {
        let mut b = [0u8; ENTRY];
        match r.read_exact(&mut b) {
            Ok(()) => Ok(Some(PosEntry::decode(&b))),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e.into()),
        }
    };
    let mut heads: Vec<Option<PosEntry>> = readers.iter_mut().map(next_of).collect::<Result<_, _>>()?;
    let mut mem = last.iter().copied().peekable();

    let file = File::create(out)?;
    let mut w = BufWriter::with_capacity(1 << 20, &file);
    w.write_all(&[0u8; HEADER as usize])?;
    let mut count = 0u64;
    let mut fences = Vec::new();
    let mut buf = Vec::with_capacity(ENTRY);
    loop {
        // The smallest head among the runs and the in-memory rest.
        let mut best: Option<(usize, PosEntry)> = mem.peek().map(|&e| (usize::MAX, e));
        for (i, h) in heads.iter().enumerate() {
            if let Some(e) = h
                && best.is_none_or(|(_, b)| *e < b)
            {
                best = Some((i, *e));
            }
        }
        let Some((src, entry)) = best else { break };
        if src == usize::MAX {
            mem.next();
        } else {
            heads[src] = next_of(&mut readers[src])?;
        }
        if count.is_multiple_of(BLOCK) {
            fences.push(entry.key);
        }
        buf.clear();
        entry.encode(&mut buf);
        w.write_all(&buf)?;
        count += 1;
    }
    for f in &fences {
        w.write_all(&f.to_le_bytes())?;
    }
    w.flush()?;
    drop(w);
    let mut h = Vec::with_capacity(HEADER as usize);
    h.extend_from_slice(MAGIC);
    h.extend_from_slice(&VERSION.to_le_bytes());
    h.extend_from_slice(&(ENTRY as u32).to_le_bytes());
    h.extend_from_slice(&u32::from(max_ply).to_le_bytes());
    h.extend_from_slice(&first.to_le_bytes());
    h.extend_from_slice(&end.to_le_bytes());
    h.extend_from_slice(&count.to_le_bytes());
    h.resize(HEADER as usize, 0);
    write_all_at(&file, &h, 0)?;
    file.sync_all()?;
    Ok(())
}

/// Result of an index update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexSummary {
    pub plan: IndexPlan,
    pub games: u32,
    pub positions: u64,
    /// The database was compacted while this ran, so nothing was installed;
    /// the caller should run it again.
    pub superseded: bool,
}

/// Brings the position index up to date: indexes games added since the last
/// update, or rebuilds everything when needed (see [`IndexPlan`]). Runs
/// outside the database lock except while reading games and installing.
pub fn update_position_index(db: &Mutex<Database>, progress: &dyn Progress) -> Result<IndexSummary, DbError> {
    let (plan, dir, count, dirty_before, max_ply, generation, compactions) = {
        let db = db.lock().expect("db lock");
        let index = db.positions();
        let generation = index.segments.iter().map(|s| s.generation).max().unwrap_or(0) + 1;
        let max_ply = if index.is_built() { index.max_ply() } else { DEFAULT_MAX_PLY };
        let plan = index.plan(db.count());
        (plan, db.path().to_path_buf(), db.count(), index.dirty.len(), max_ply, generation, db.compactions())
    };
    // A compaction renumbers the games and deletes the index files, ours
    // included: whatever this run built is then stale, and any error it hit
    // on the way (a deleted temp file, a game id past the end) is moot.
    let superseded = |db: &Database| db.compactions() != compactions;
    let stale = || IndexSummary { plan, games: 0, positions: 0, superseded: true };
    let first = match plan {
        IndexPlan::UpToDate => return Ok(IndexSummary { plan, games: 0, positions: 0, superseded: false }),
        IndexPlan::Append(from) => from,
        IndexPlan::Rebuild => 0,
    };
    let max_ply = if plan == IndexPlan::Rebuild { DEFAULT_MAX_PLY } else { max_ply };
    let ids: Vec<GameId> = (first..count).collect();

    let mut runs: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<PosEntry> = Vec::with_capacity(RUN_ENTRIES.min(ids.len() * 40 + 16));
    let mut game_openings: Vec<u16> = Vec::with_capacity(ids.len());
    let mut positions = 0u64;
    let cleanup = |runs: &[PathBuf]| {
        for r in runs {
            let _ = std::fs::remove_file(r);
        }
    };
    let work = |id: GameId, rec: &IndexRecord, blob: &[u8]| {
        let mut entries = Vec::with_capacity(usize::from(max_ply) + 1);
        match game_entries(id, rec, blob, Some(max_ply), &mut entries) {
            Ok(opening) => (entries, opening),
            Err(e) => {
                log::warn!("position index: skipping game {id}: {e}");
                (Vec::new(), 0)
            }
        }
    };
    let result = for_each_game(db, &ids, progress, &work, &mut |batch| {
        for (entries, opening) in batch {
            positions += entries.len() as u64;
            pending.extend(entries);
            game_openings.push(opening);
        }
        if pending.len() >= RUN_ENTRIES {
            let path = dir.join(format!("positions.run{}.tmp", runs.len()));
            write_run(&path, &mut pending)?;
            pending.clear();
            runs.push(path);
        }
        Ok(())
    });
    if let Err(e) = result {
        cleanup(&runs);
        return if superseded(&db.lock().expect("db lock")) { Ok(stale()) } else { Err(e) };
    }
    let tmp = dir.join(format!("positions-{generation}.idx.tmp"));
    let merged = merge_runs(&runs, &mut pending, &tmp, first, count, max_ply);
    cleanup(&runs);
    if let Err(e) = merged {
        let _ = std::fs::remove_file(&tmp);
        return if superseded(&db.lock().expect("db lock")) { Ok(stale()) } else { Err(e) };
    }

    // Openings: a rebuild rewrites the file, an append extends it.
    let openings_tmp = dir.join(format!("{OPENINGS_FILE}.tmp"));
    let mut db = db.lock().expect("db lock");
    if superseded(&db) {
        let _ = std::fs::remove_file(&tmp);
        return Ok(stale());
    }
    let index = db.positions_mut();
    let mut all_openings = if plan == IndexPlan::Rebuild { Vec::new() } else { index.openings.clone() };
    all_openings.truncate(first as usize);
    all_openings.resize(first as usize, 0);
    all_openings.extend_from_slice(&game_openings);
    let mut bytes = Vec::with_capacity(16 + all_openings.len() * 2);
    bytes.extend_from_slice(OPENINGS_MAGIC);
    bytes.extend_from_slice(&openings::data_version().to_le_bytes());
    bytes.resize(16, 0);
    for o in &all_openings {
        bytes.extend_from_slice(&o.to_le_bytes());
    }
    {
        // Syncing needs a writable handle on Windows (FlushFileBuffers).
        let f = File::create(&openings_tmp)?;
        write_all_at(&f, &bytes, 0)?;
        f.sync_all()?;
    }

    // Install: drop superseded segments (closing their files first).
    let final_path = dir.join(format!("positions-{generation}.idx"));
    std::fs::rename(&tmp, &final_path)?;
    std::fs::rename(&openings_tmp, dir.join(OPENINGS_FILE))?;
    if plan == IndexPlan::Rebuild {
        for seg in std::mem::take(&mut index.segments) {
            let path = seg.path.clone();
            drop(seg);
            let _ = std::fs::remove_file(path);
        }
        // Games edited while this build ran stay dirty.
        let still: Vec<GameId> = index.dirty[dirty_before.min(index.dirty.len())..].to_vec();
        let dirty_tmp = dir.join(format!("{DIRTY_FILE}.tmp"));
        std::fs::write(&dirty_tmp, still.iter().flat_map(|g| g.to_le_bytes()).collect::<Vec<u8>>())?;
        std::fs::rename(&dirty_tmp, dir.join(DIRTY_FILE))?;
        index.dirty_set = GameSet::from_ids(&still);
        index.dirty = still;
    }
    index.segments.push(Segment::open(&final_path, generation)?);
    index.openings = all_openings;
    Ok(IndexSummary { plan, games: ids.len() as u32, positions, superseded: false })
}

/// Games (all of them, deleted ones included) whose main line reaches `pos`
/// within the indexed plies, one entry per game (its first visit).
#[derive(Debug, Clone, Default)]
pub struct PositionHits {
    pub entries: Vec<PosEntry>,
    /// Some games couldn't be checked (too many not indexed yet).
    pub partial: bool,
    pub max_ply: u16,
}

/// Looks `pos` up in the index, replaying games it doesn't cover (edited
/// or new) when there are few enough.
pub fn find_position(db: &Mutex<Database>, pos: &Chess) -> Result<PositionHits, DbError> {
    let key = position::hash(pos);
    let db = db.lock().expect("db lock");
    let index = db.positions();
    let max_ply = index.max_ply();
    let mut entries = index.lookup(key)?;
    let mut replay: Vec<GameId> = index.dirty.clone();
    let covered = index.covered();
    let uncovered = db.count().saturating_sub(covered) as usize;
    let partial = replay.len() + uncovered > REPLAY_LIMIT;
    // Segments cover ascending, disjoint game ranges and are sorted by
    // (key, game, ply): without replayed games the hits are already in order.
    let sorted = partial || replay.is_empty() && uncovered == 0;
    if !partial {
        replay.extend(covered..db.count());
        let mut scratch = Vec::new();
        for id in replay {
            let rec = db.record(id)?;
            scratch.clear();
            if game_entries(id, &rec, &db.read_blob(id)?, Some(max_ply), &mut scratch).is_ok() {
                entries.extend(scratch.iter().filter(|e| e.key == key));
            }
        }
    }
    if !sorted {
        entries.sort_unstable_by_key(|e| (e.game, e.ply));
    }
    entries.dedup_by_key(|e| e.game);
    Ok(PositionHits { entries, partial, max_ply })
}

/// Finds the games reaching `pos` at any ply by reading every game (for
/// positions deeper than the index goes). Slow: run it as a job.
pub fn scan_position(
    db: &Mutex<Database>,
    pos: &Chess,
    progress: &dyn Progress,
) -> Result<Vec<GameId>, DbError> {
    let key = position::hash(pos);
    let count = db.lock().expect("db lock").count();
    let ids: Vec<GameId> = (0..count).collect();
    let mut found = Vec::new();
    let work = |id: GameId, rec: &IndexRecord, blob: &[u8]| {
        let mut entries = Vec::new();
        game_entries(id, rec, blob, None, &mut entries).is_ok() && entries.iter().any(|e| e.key == key)
    };
    let mut next = 0usize;
    for_each_game(db, &ids, progress, &work, &mut |batch| {
        for hit in batch {
            if hit {
                found.push(ids[next]);
            }
            next += 1;
        }
        Ok(())
    })?;
    Ok(found)
}

/// Statistics of one move (or of all games) in the opening tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeStats {
    pub games: u32,
    pub white_wins: u32,
    pub draws: u32,
    pub black_wins: u32,
    /// Elo of the player who made the move (the side to move), summed.
    pub elo_sum: u64,
    pub elo_count: u32,
    pub opp_elo_sum: u64,
    pub opp_elo_count: u32,
    /// Latest year a game with this move was played (0 = unknown).
    pub last_year: u16,
}

impl TreeStats {
    pub(crate) fn add(&mut self, g: &GameStat, white_to_move: bool) {
        self.games += 1;
        match g.result {
            1 => self.white_wins += 1,
            2 => self.black_wins += 1,
            3 => self.draws += 1,
            _ => {}
        }
        let (own, opp) = if white_to_move { (g.white_elo, g.black_elo) } else { (g.black_elo, g.white_elo) };
        if own != 0 {
            self.elo_sum += u64::from(own);
            self.elo_count += 1;
        }
        if opp != 0 {
            self.opp_elo_sum += u64::from(opp);
            self.opp_elo_count += 1;
        }
        self.last_year = self.last_year.max(g.year);
    }
}

/// The opening tree at a position (OPEN-01).
#[derive(Debug, Clone, Default)]
pub struct PositionTree {
    pub total: TreeStats,
    /// Moves played from the position, most games first. `None`: games that
    /// ended here.
    pub moves: Vec<(Option<Move>, TreeStats)>,
    /// All games reaching the position (deleted ones included), ascending.
    pub games: Vec<GameId>,
    pub partial: bool,
    pub max_ply: u16,
}

/// Builds the opening tree for `pos`. Deleted games don't count.
pub fn position_tree(db: &Mutex<Database>, pos: &Chess) -> Result<PositionTree, DbError> {
    let hits = find_position(db, pos)?;
    let white = pos.turn().is_white();
    let mut total = TreeStats::default();
    let mut by_move: [TreeStats; 256] = std::array::from_fn(|_| TreeStats::default());
    {
        let mut db = db.lock().expect("db lock");
        let stats = db.game_stats()?;
        for e in &hits.entries {
            let Some(g) = stats.get(e.game as usize) else { continue };
            if g.deleted {
                continue;
            }
            total.add(g, white);
            by_move[usize::from(e.next)].add(g, white);
        }
    }
    let moves = ranked_moves(pos, &by_move).into_iter().map(|(next, m)| (m, by_move[next].clone())).collect();
    let games = hits.entries.iter().map(|e| e.game).collect();
    Ok(PositionTree { total, moves, games, partial: hits.partial, max_ply: hits.max_ply })
}

/// The moves of a tally by move byte (`by_move[next]`), most games first,
/// then the most recently played: each move's byte and the move (`None`
/// for games that ended in the position). The opening tree and the
/// Reference tab (OPEN-14) list them in this order.
pub(crate) fn ranked_moves(pos: &Chess, by_move: &[TreeStats; 256]) -> Vec<(usize, Option<Move>)> {
    let mut moves: Vec<(usize, Option<Move>)> = by_move
        .iter()
        .enumerate()
        .filter(|(_, s)| s.games > 0)
        .filter_map(|(next, _)| {
            if next == usize::from(NO_MOVE) {
                Some((next, None))
            } else {
                // A hash collision could give an impossible move; skip it.
                move_at(pos, next as u8).map(|m| (next, Some(m)))
            }
        })
        .collect();
    moves.sort_by(|a, b| {
        let (a, b) = (&by_move[a.0], &by_move[b.0]);
        b.games.cmp(&a.games).then(a.last_year.cmp(&b.last_year).reverse())
    });
    moves
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoProgress;
    use chess_core::pgn::read_pgn_str;
    use chess_core::position::parse_uci;

    const PGN: &str = r#"[White "A"][Black "B"][Result "1-0"][WhiteElo "2000"][BlackElo "1800"][Date "2001.01.01"]
1. e4 e5 2. Nf3 Nc6 3. Bb5 1-0

[White "C"][Black "D"][Result "0-1"][WhiteElo "2200"][Date "2010.01.01"]
1. e4 c5 2. Nf3 d6 0-1

[White "E"][Black "F"][Result "1/2-1/2"]
1. Nf3 Nc6 2. e4 e5 1/2-1/2

[White "G"][Black "H"][Result "*"]
1. d4 d5 *
"#;

    fn db() -> (tempfile::TempDir, Mutex<Database>) {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Database::create(&dir.path().join("Pos")).unwrap();
        for g in read_pgn_str(PGN) {
            db.append(&g).unwrap();
        }
        (dir, Mutex::new(db))
    }

    fn after(ucis: &[&str]) -> Chess {
        let mut pos = Chess::default();
        for u in ucis {
            let m = parse_uci(&pos, u).unwrap();
            pos.play_unchecked(m);
        }
        pos
    }

    fn games(db: &Mutex<Database>, ucis: &[&str]) -> Vec<GameId> {
        find_position(db, &after(ucis)).unwrap().entries.iter().map(|e| e.game).collect()
    }

    #[test]
    fn finds_transpositions_and_builds_the_tree() {
        let (_d, db) = db();
        let summary = update_position_index(&db, &NoProgress).unwrap();
        assert_eq!(summary.plan, IndexPlan::Rebuild);
        assert_eq!(summary.games, 4);
        assert_eq!(summary.positions, 6 + 5 + 5 + 3);
        assert_eq!(db.lock().unwrap().positions().plan(4), IndexPlan::UpToDate);

        // 1.e4 e5 2.Nf3 Nc6 and 1.Nf3 Nc6 2.e4 e5 reach the same position.
        assert_eq!(games(&db, &["e2e4", "e7e5", "g1f3", "b8c6"]), [0, 2]);
        assert_eq!(games(&db, &["e2e4"]), [0, 1]);
        assert_eq!(games(&db, &["a2a4"]), Vec::<GameId>::new());

        let tree = position_tree(&db, &Chess::default()).unwrap();
        assert_eq!(tree.total.games, 4);
        assert_eq!((tree.total.white_wins, tree.total.draws, tree.total.black_wins), (1, 1, 1));
        let first = &tree.moves[0];
        assert_eq!(first.0.map(chess_core::notation::uci).as_deref(), Some("e2e4"));
        assert_eq!(first.1.games, 2);
        assert_eq!(first.1.elo_sum, 4200);
        assert_eq!(first.1.last_year, 2010);

        // Games that end in the position appear as a `None` move.
        let tree = position_tree(&db, &after(&["e2e4", "e7e5", "g1f3", "b8c6"])).unwrap();
        assert_eq!(tree.moves.len(), 2);
        assert!(tree.moves.iter().any(|(m, s)| m.is_none() && s.games == 1));

        // Openings are classified in the same pass.
        let o = db.lock().unwrap().positions().opening_of(0);
        assert!(chess_core::openings::by_id(o).unwrap().name.starts_with("Ruy Lopez"));
    }

    #[test]
    fn new_and_edited_games_are_found_before_and_after_updates() {
        let (dir, db) = db();
        update_position_index(&db, &NoProgress).unwrap();
        // A new game is found by replay, then indexed in a second segment.
        let extra = read_pgn_str("1. e4 e5 2. Nf3 Nc6 3. Bc4 *").remove(0);
        db.lock().unwrap().append(&extra).unwrap();
        assert_eq!(db.lock().unwrap().positions().plan(5), IndexPlan::Append(4));
        assert_eq!(games(&db, &["e2e4", "e7e5", "g1f3", "b8c6", "f1c4"]), [4]);
        let s = update_position_index(&db, &NoProgress).unwrap();
        assert_eq!((s.plan, s.games), (IndexPlan::Append(4), 1));
        assert_eq!(games(&db, &["e2e4", "e7e5", "g1f3", "b8c6", "f1c4"]), [4]);

        // An edited game no longer matches its old positions.
        let other = read_pgn_str("1. c4 *").remove(0);
        db.lock().unwrap().replace(1, &other).unwrap();
        assert_eq!(games(&db, &["e2e4"]), [0, 4]);
        assert_eq!(games(&db, &["c2c4"]), [1]);

        // Everything survives a reopen, and a rebuild folds it together.
        let path = db.lock().unwrap().path().to_path_buf();
        drop(db);
        let db = Mutex::new(Database::open(&path).unwrap());
        assert_eq!(db.lock().unwrap().positions().dirty_count(), 1);
        assert_eq!(games(&db, &["c2c4"]), [1]);
        let db_path = db.lock().unwrap().path().to_path_buf();
        // Imports add segments until there are too many; then it rebuilds.
        for round in 0.. {
            assert!(round <= MAX_SEGMENTS, "never rebuilt");
            db.lock().unwrap().append(&other).unwrap();
            if update_position_index(&db, &NoProgress).unwrap().plan == IndexPlan::Rebuild {
                break;
            }
        }
        assert_eq!(db.lock().unwrap().positions().segments.len(), 1, "rebuilt into one segment");
        assert_eq!(db.lock().unwrap().positions().dirty_count(), 0);
        assert_eq!(games(&db, &["e2e4"]), [0, 4]);
        let files = std::fs::read_dir(&db_path).unwrap().filter_map(Result::ok);
        let segs = files.filter(|e| e.file_name().to_string_lossy().starts_with("positions-")).count();
        assert_eq!(segs, 1, "old segments deleted");
        drop(dir);
    }

    /// Compacts the database the first time the build reports progress,
    /// that is after it read the games and before it installs the index.
    struct CompactMidway<'a> {
        db: &'a Mutex<Database>,
        done: std::sync::atomic::AtomicBool,
    }

    impl Progress for CompactMidway<'_> {
        fn report(&self, _done: u64, _total: u64) {
            if !self.done.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let mut db = self.db.lock().unwrap();
                db.set_deleted(1, true).unwrap();
                db.compact(&NoProgress).unwrap();
            }
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_compaction_during_a_build_supersedes_it() {
        let (_d, db) = db();
        let progress = CompactMidway { db: &db, done: Default::default() };
        let s = update_position_index(&db, &progress).unwrap();
        assert!(s.superseded, "the build noticed the compaction");
        assert!(!db.lock().unwrap().positions().is_built(), "nothing stale was installed");
        let path = db.lock().unwrap().path().to_path_buf();
        let leftovers: Vec<String> = std::fs::read_dir(&path)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("positions") || n.starts_with("openings"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");

        // Running it again indexes the renumbered games: C-D (1.e4 c5) is
        // gone and E-F, the transposition, is game 1 now.
        let s = update_position_index(&db, &NoProgress).unwrap();
        assert_eq!((s.plan, s.games, s.superseded), (IndexPlan::Rebuild, 3, false));
        assert_eq!(games(&db, &["e2e4", "e7e5", "g1f3", "b8c6"]), [0, 1]);
        assert_eq!(games(&db, &["e2e4", "c7c5"]), Vec::<GameId>::new());
    }

    #[test]
    fn scan_finds_deep_positions() {
        let (_d, db) = db();
        let found = scan_position(&db, &after(&["d2d4", "d7d5"]), &NoProgress).unwrap();
        assert_eq!(found, [3]);
    }

    #[test]
    fn overlapping_segments_after_a_crash_keep_the_newest() {
        let (_d, db) = db();
        update_position_index(&db, &NoProgress).unwrap();
        let path = db.lock().unwrap().path().to_path_buf();
        drop(db);
        // Simulate a crash after a rebuild wrote its segment but before the
        // old one was deleted: an older, overlapping segment.
        let newest = std::fs::read_dir(&path)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| segment_generation(p).is_some())
            .unwrap();
        std::fs::copy(&newest, path.join("positions-0.idx")).unwrap();
        std::fs::write(path.join("positions.run0.tmp"), b"junk").unwrap();
        let db = Mutex::new(Database::open(&path).unwrap());
        assert_eq!(db.lock().unwrap().positions().segments.len(), 1);
        assert!(!path.join("positions-0.idx").exists());
        assert!(!path.join("positions.run0.tmp").exists());
        assert_eq!(games(&db, &["e2e4"]), [0, 1]);
    }
}
