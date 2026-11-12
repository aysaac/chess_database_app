//! Permanent game ids (the groundwork for NET-13 and the server's phase 4).
//!
//! Every game has one: 16 random bytes ([`GameUid`]), unique across
//! databases, never reused, and kept through edits, deletion, undeletion and
//! compaction. A game's slot ([`GameId`](crate::GameId), its place in
//! `games.idx`) is what lists, sorts, the position index and the Reference
//! tab use, and a compaction changes it; the permanent id is for the edges
//! (links to a study's chapters, sync state, a server's API), which find the
//! slot through [`UidIndex`].
//!
//! Files (`docs/file-format.md`):
//!
//! - `games.ids`: a 16-byte header, then the permanent id of each slot.
//! - `games.origins`: a 16-byte header, then `(id, origin id)` for each game
//!   copied from another database: the id the game has there.
//!
//! `games.origins` is the first table keyed by permanent id; NET-13's links
//! and sync state can be kept the same way.

use std::collections::HashMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::DbError;
use crate::io::{read_exact_at, write_all_at};

pub(crate) const IDS_FILE: &str = "games.ids";
pub(crate) const ORIGINS_FILE: &str = "games.origins";

const IDS_MAGIC: &[u8; 8] = b"CHDBUID1";
const ORIGINS_MAGIC: &[u8; 8] = b"CHDBORG1";
/// Version in the headers of `games.ids` and `games.origins`.
const VERSION: u32 = 1;
const HEADER: u64 = 16;
const UID_LEN: u64 = 16;
/// An entry of `games.origins`: the game's id, then its origin's.
const ORIGIN_LEN: u64 = 32;
/// Ids read, minted or written at a time (1 MB).
const CHUNK: u32 = 64 * 1024;

/// A game's permanent id: 16 random bytes laid out as a UUID version 4
/// (RFC 9562), so never all zero. In text it's written as one, lowercase
/// (`1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633b`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GameUid([u8; 16]);

impl GameUid {
    pub const fn from_bytes(bytes: [u8; 16]) -> GameUid {
        GameUid(bytes)
    }

    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }

    /// The first 4 bytes, which [`UidIndex`] sorts by (random in a minted id).
    fn prefix(&self) -> u32 {
        u32::from_be_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
}

impl fmt::Display for GameUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for GameUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GameUid({self})")
    }
}

impl FromStr for GameUid {
    type Err = DbError;

    /// Takes the hyphenated form (as [`Display`](fmt::Display) writes it)
    /// or the 32 hex digits alone, in either case.
    fn from_str(s: &str) -> Result<GameUid, DbError> {
        let bad = || DbError::Invalid("not a game id".into());
        let hyphens = [8, 13, 18, 23];
        let digits: Vec<u8> = match s.len() {
            32 => s.bytes().collect(),
            36 if hyphens.iter().all(|&i| s.as_bytes()[i] == b'-') => {
                s.bytes().enumerate().filter(|(i, _)| !hyphens.contains(i)).map(|(_, b)| b).collect()
            }
            _ => return Err(bad()),
        };
        let mut out = [0u8; 16];
        for (byte, [hi, lo]) in out.iter_mut().zip(digits.as_chunks::<2>().0) {
            let hex = |c: u8| (c as char).to_digit(16).ok_or_else(bad);
            *byte = (hex(*hi)? * 16 + hex(*lo)?) as u8;
        }
        Ok(GameUid(out))
    }
}

/// `n` new permanent ids from the OS's random source.
pub(crate) fn mint(n: usize) -> Result<Vec<GameUid>, DbError> {
    let mut bytes = vec![0u8; n * UID_LEN as usize];
    getrandom::fill(&mut bytes)
        .map_err(|e| std::io::Error::other(format!("no random bytes for new game ids: {e}")))?;
    Ok(bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| {
            let mut b = *c;
            b[6] = (b[6] & 0x0f) | 0x40; // version 4
            b[8] = (b[8] & 0x3f) | 0x80; // RFC 9562 variant
            GameUid(b)
        })
        .collect())
}

fn header(magic: &[u8; 8]) -> [u8; HEADER as usize] {
    let mut h = [0u8; HEADER as usize];
    h[..8].copy_from_slice(magic);
    h[8..12].copy_from_slice(&VERSION.to_le_bytes());
    h
}

fn check_header(file: &File, magic: &[u8; 8], path: &Path) -> Result<(), DbError> {
    let mut buf = [0u8; 12];
    read_exact_at(file, &mut buf, 0)
        .map_err(|_| DbError::Corrupt(format!("{} is truncated", path.display())))?;
    if &buf[..8] != magic {
        return Err(DbError::Corrupt(format!("{} has a bad header", path.display())));
    }
    let version = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    if version != VERSION {
        return Err(DbError::UnsupportedVersion(version));
    }
    Ok(())
}

/// `games.ids.tmp` for `games.ids`: where a whole new file is written before
/// it's renamed into place.
pub(crate) fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

fn uids_from(bytes: &[u8]) -> Vec<GameUid> {
    bytes.as_chunks::<16>().0.iter().map(|c| GameUid(*c)).collect()
}

fn bytes_of(uids: &[GameUid]) -> Vec<u8> {
    uids.iter().flat_map(|u| u.0).collect()
}

/// `games.ids`: the permanent id of every slot, in slot order. Written for
/// new games before their index records, so after a crash it holds at
/// least the committed games' ids.
#[derive(Debug)]
pub(crate) struct IdsFile {
    file: File,
}

fn slot_offset(slot: u32) -> u64 {
    HEADER + u64::from(slot) * UID_LEN
}

impl IdsFile {
    /// A new file without ids, at `path` (a new database's, or a
    /// compaction's `.new` one).
    pub(crate) fn create(path: &Path) -> Result<IdsFile, DbError> {
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
        write_all_at(&file, &header(IDS_MAGIC), 0)?;
        file.sync_all()?;
        Ok(IdsFile { file })
    }

    /// Writes a file of `count` new ids, in `path`'s temp file, then renames
    /// it over `path`: the ids of a database from before permanent ids (its
    /// first open), all at once or not at all.
    pub(crate) fn write_fresh(path: &Path, count: u32) -> Result<(), DbError> {
        let tmp = tmp_path(path);
        {
            let file = File::create(&tmp)?;
            let ids = IdsFile { file };
            write_all_at(&ids.file, &header(IDS_MAGIC), 0)?;
            ids.mint_into(0, count)?;
            ids.file.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Opens the ids of a database with `count` committed games. Ids past
    /// them (written by a commit a crash cut short) are dropped. Missing
    /// ones can't come from a crash (ids are synced before the count); if
    /// the file lost its end anyway, or is gone, those games get new ids,
    /// with a warning, rather than the database not opening.
    pub(crate) fn open(path: &Path, count: u32) -> Result<IdsFile, DbError> {
        if !path.exists() {
            log::warn!("{} is missing: new permanent ids for all {count} games", path.display());
            IdsFile::write_fresh(path, count)?;
        }
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        check_header(&file, IDS_MAGIC, path)?;
        let ids = IdsFile { file };
        let len = ids.file.metadata()?.len();
        let expected = slot_offset(count);
        if len > expected {
            ids.file.set_len(expected)?;
        } else if len < expected {
            let have = (len.saturating_sub(HEADER) / UID_LEN) as u32;
            log::warn!(
                "{} holds ids for {have} of {count} games: new permanent ids for the others",
                path.display()
            );
            ids.file.set_len(slot_offset(have))?;
            ids.mint_into(have, count)?;
            ids.file.sync_data()?;
        }
        Ok(ids)
    }

    /// Writes new ids for slots `from..to` (not synced).
    fn mint_into(&self, from: u32, to: u32) -> Result<(), DbError> {
        let mut slot = from;
        while slot < to {
            let n = (to - slot).min(CHUNK);
            write_all_at(&self.file, &bytes_of(&mint(n as usize)?), slot_offset(slot))?;
            slot += n;
        }
        Ok(())
    }

    /// The id in `slot` (the caller checks it's a committed game's).
    pub(crate) fn get(&self, slot: u32) -> Result<GameUid, DbError> {
        let mut buf = [0u8; 16];
        read_exact_at(&self.file, &mut buf, slot_offset(slot))?;
        Ok(GameUid(buf))
    }

    /// The ids in slots `start..start + n`, in one read.
    pub(crate) fn range(&self, start: u32, n: u32) -> Result<Vec<GameUid>, DbError> {
        let mut buf = vec![0u8; n as usize * UID_LEN as usize];
        read_exact_at(&self.file, &mut buf, slot_offset(start))?;
        Ok(uids_from(&buf))
    }

    /// Writes ids into slots from `first` on (not synced).
    pub(crate) fn put(&self, first: u32, uids: &[GameUid]) -> Result<(), DbError> {
        write_all_at(&self.file, &bytes_of(uids), slot_offset(first))?;
        Ok(())
    }

    pub(crate) fn sync(&self) -> Result<(), DbError> {
        self.file.sync_data()?;
        Ok(())
    }

    /// The index of every slot below `count` by id.
    pub(crate) fn index(&self, count: u32) -> Result<UidIndex, DbError> {
        let mut sorted = Vec::with_capacity(count as usize);
        let mut start = 0;
        while start < count {
            let n = (count - start).min(CHUNK);
            sorted.extend(self.range(start, n)?.iter().zip(start..).map(|(u, slot)| pack(u, slot)));
            start += n;
        }
        sorted.sort_unstable();
        Ok(UidIndex { sorted, recent: HashMap::new() })
    }
}

/// `games.origins`: `(id, origin id)` of each game copied in from another
/// database, appended (and synced) before the copies' index records. An
/// entry for an id no committed game has (a commit a crash cut short) is
/// never looked up, and a compaction drops it with those of the games it
/// removes. The last entry for an id wins (there is only one so far).
#[derive(Debug)]
pub(crate) struct Origins {
    file: File,
    entries: u32,
    /// Loaded on first use.
    index: Option<UidIndex>,
}

fn origin_offset(entry: u32) -> u64 {
    HEADER + u64::from(entry) * ORIGIN_LEN
}

impl Origins {
    /// A new, empty table at `path`.
    pub(crate) fn create(path: &Path) -> Result<Origins, DbError> {
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
        write_all_at(&file, &header(ORIGINS_MAGIC), 0)?;
        file.sync_all()?;
        Ok(Origins { file, entries: 0, index: None })
    }

    /// Replaces the table at `path` with an empty one (temp file + rename):
    /// a database from before permanent ids has no copies to name.
    pub(crate) fn write_empty(path: &Path) -> Result<(), DbError> {
        let tmp = tmp_path(path);
        {
            let file = File::create(&tmp)?;
            write_all_at(&file, &header(ORIGINS_MAGIC), 0)?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Opens the table at `path`, dropping a torn entry at its end (from a
    /// commit a crash cut short). A missing table is made again, empty,
    /// with a warning: the copies then name no origin.
    pub(crate) fn open(path: &Path) -> Result<Origins, DbError> {
        if !path.exists() {
            log::warn!("{} is missing: the games copied in no longer name their origin", path.display());
            Origins::write_empty(path)?;
        }
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        check_header(&file, ORIGINS_MAGIC, path)?;
        let len = file.metadata()?.len();
        let entries = u32::try_from(len.saturating_sub(HEADER) / ORIGIN_LEN)
            .map_err(|_| DbError::Corrupt(format!("{} is too large", path.display())))?;
        if len != origin_offset(entries) {
            file.set_len(origin_offset(entries))?;
        }
        Ok(Origins { file, entries, index: None })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries == 0
    }

    /// Appends entries and syncs them.
    pub(crate) fn append(&mut self, entries: &[(GameUid, GameUid)]) -> Result<(), DbError> {
        let bytes: Vec<u8> =
            entries.iter().flat_map(|(id, origin)| id.0.into_iter().chain(origin.0)).collect();
        write_all_at(&self.file, &bytes, origin_offset(self.entries))?;
        self.file.sync_data()?;
        for (entry, (id, _)) in (self.entries..).zip(entries) {
            if let Some(index) = &mut self.index {
                index.insert(*id, entry);
            }
        }
        self.entries += entries.len() as u32;
        Ok(())
    }

    /// Entries `start..start + n`, in one read.
    fn range(&self, start: u32, n: u32) -> Result<Vec<(GameUid, GameUid)>, DbError> {
        let mut buf = vec![0u8; n as usize * ORIGIN_LEN as usize];
        read_exact_at(&self.file, &mut buf, origin_offset(start))?;
        Ok(buf.as_chunks::<32>().0.iter().map(|c| (uid_at(c, 0), uid_at(c, 16))).collect())
    }

    /// The origin of the game with id `id`, if it was copied in.
    pub(crate) fn get(&mut self, id: GameUid) -> Result<Option<GameUid>, DbError> {
        if self.index.is_none() {
            let mut sorted = Vec::with_capacity(self.entries as usize);
            let mut start = 0;
            while start < self.entries {
                let n = (self.entries - start).min(CHUNK);
                sorted
                    .extend(self.range(start, n)?.iter().zip(start..).map(|((u, _), entry)| pack(u, entry)));
                start += n;
            }
            sorted.sort_unstable();
            self.index = Some(UidIndex { sorted, recent: HashMap::new() });
        }
        let index = self.index.as_ref().expect("just loaded");
        let Some(entry) = index.find(id, |entry| Ok(self.range(entry, 1)?[0].0))? else {
            return Ok(None);
        };
        Ok(Some(self.range(entry, 1)?[0].1))
    }

    /// Writes the entries `keep` says yes to into a new table at `path` (a
    /// compaction's `.new` file), synced.
    pub(crate) fn write_kept(
        &self,
        path: &Path,
        mut keep: impl FnMut(GameUid) -> Result<bool, DbError>,
    ) -> Result<Origins, DbError> {
        let mut out = Origins::create(path)?;
        let mut start = 0;
        while start < self.entries {
            let n = (self.entries - start).min(CHUNK);
            let mut kept = Vec::new();
            for entry in self.range(start, n)? {
                if keep(entry.0)? {
                    kept.push(entry);
                }
            }
            let bytes: Vec<u8> =
                kept.iter().flat_map(|(id, origin)| id.0.into_iter().chain(origin.0)).collect();
            write_all_at(&out.file, &bytes, origin_offset(out.entries))?;
            out.entries += kept.len() as u32;
            start += n;
        }
        out.file.sync_all()?;
        Ok(out)
    }
}

fn uid_at(b: &[u8], at: usize) -> GameUid {
    let mut out = [0u8; 16];
    out.copy_from_slice(&b[at..at + 16]);
    GameUid(out)
}

fn pack(uid: &GameUid, position: u32) -> u64 {
    u64::from(uid.prefix()) << 32 | u64::from(position)
}

/// Finds a position (a slot of `games.ids`, an entry of `games.origins`) by
/// permanent id, in 8 bytes a position: the id's first 4 bytes and the
/// position, packed into a `u64` and sorted, plus the positions added since
/// in a map until there are enough to sort them in. A lookup checks each
/// candidate's whole id where it's stored: with random ids, two share their
/// first 4 bytes about once in 4 billion, so that's one read.
#[derive(Debug, Default)]
pub(crate) struct UidIndex {
    sorted: Vec<u64>,
    recent: HashMap<GameUid, u32>,
}

/// Positions kept in [`UidIndex::recent`] before they're sorted in, at least.
const MERGE_AT: usize = 4096;

impl UidIndex {
    /// Adds a position past every one it has.
    pub(crate) fn insert(&mut self, uid: GameUid, position: u32) {
        self.recent.insert(uid, position);
        if self.recent.len() > MERGE_AT.max(self.sorted.len() / 8) {
            self.sorted.extend(self.recent.drain().map(|(u, p)| pack(&u, p)));
            self.sorted.sort_unstable();
        }
    }

    /// The last position holding `uid`, as `at` reads the id a position
    /// holds.
    pub(crate) fn find(
        &self,
        uid: GameUid,
        at: impl Fn(u32) -> Result<GameUid, DbError>,
    ) -> Result<Option<u32>, DbError> {
        if let Some(&position) = self.recent.get(&uid) {
            return Ok(Some(position));
        }
        let prefix = u64::from(uid.prefix());
        let start = self.sorted.partition_point(|&e| e >> 32 < prefix);
        let mut found = None;
        for &e in self.sorted[start..].iter().take_while(|&&e| e >> 32 == prefix) {
            // Ascending within a prefix, so the last match is the latest.
            if at(e as u32)? == uid {
                found = Some(e as u32);
            }
        }
        Ok(found)
    }

    /// Follows a compaction: `mapping[old]` is each old position's new one,
    /// `None` for those dropped. New positions keep the old ones' order, so
    /// what's sorted stays sorted.
    pub(crate) fn remap(&mut self, mapping: &[Option<u32>]) {
        let new = |p: u32| mapping.get(p as usize).copied().flatten();
        self.sorted.retain_mut(|e| match new(*e as u32) {
            Some(p) => {
                *e = (*e >> 32) << 32 | u64::from(p);
                true
            }
            None => false,
        });
        self.recent = self.recent.drain().filter_map(|(u, p)| Some((u, new(p)?))).collect();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn minted_ids_are_version_4_and_distinct() {
        let ids = mint(100_000).unwrap();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        for id in &ids[..100] {
            let text = id.to_string();
            assert_eq!(text.len(), 36);
            assert_eq!(&text[14..15], "4", "{text}");
            assert!("89ab".contains(&text[19..20]), "{text}");
            assert_ne!(id.to_bytes(), [0; 16]);
        }
    }

    #[test]
    fn ids_are_written_and_read_as_uuids() {
        let id: GameUid = "1B4E28BA-2FA1-4D3B-A3F5-EF19B5A7633B".parse().unwrap();
        assert_eq!(id.to_string(), "1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633b");
        assert_eq!("1b4e28ba2fa14d3ba3f5ef19b5a7633b".parse::<GameUid>().unwrap(), id);
        assert_eq!(id.to_bytes()[..4], [0x1b, 0x4e, 0x28, 0xba]);
        assert_eq!(format!("{id:?}"), "GameUid(1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633b)");
        for bad in [
            "",
            "1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633",
            "1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633bb",
            "1b4e28ba_2fa1-4d3b-a3f5-ef19b5a7633b",
            "1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633g",
            "+b4e28ba2fa14d3ba3f5ef19b5a7633b",
            "1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633é",
        ] {
            assert!(bad.parse::<GameUid>().is_err(), "{bad:?}");
        }
    }

    proptest! {
        #[test]
        fn text_round_trips(bytes in any::<[u8; 16]>()) {
            let id = GameUid::from_bytes(bytes);
            prop_assert_eq!(id.to_string().parse::<GameUid>().unwrap(), id);
            prop_assert_eq!(id.to_string().replace('-', "").parse::<GameUid>().unwrap(), id);
        }

        /// The index finds every id where it was put, through merges of
        /// what was added and a compaction's renumbering, and nothing else.
        #[test]
        fn the_index_finds_each_position(
            seeded in 0u32..3000,
            added in 0u32..12_000,
            dropped in prop::collection::vec(any::<bool>(), 0..64),
            shared_prefix in any::<bool>(),
        ) {
            let mut ids = mint((seeded + added) as usize).unwrap();
            if shared_prefix {
                // Ids sharing their first 4 bytes are told apart by the rest.
                for id in ids.iter_mut().step_by(3) {
                    id.0[..4].copy_from_slice(&[7, 7, 7, 7]);
                }
            }
            let at = |ids: &[GameUid]| {
                let ids = ids.to_vec();
                move |p: u32| Ok(ids[p as usize])
            };
            let mut sorted: Vec<u64> = ids[..seeded as usize].iter().zip(0..).map(|(u, p)| pack(u, p)).collect();
            sorted.sort_unstable();
            let mut index = UidIndex { sorted, recent: HashMap::new() };
            for (p, id) in ids.iter().enumerate().skip(seeded as usize) {
                index.insert(*id, p as u32);
            }
            for (p, id) in ids.iter().enumerate() {
                prop_assert_eq!(index.find(*id, at(&ids)).unwrap(), Some(p as u32));
            }
            prop_assert_eq!(index.find(mint(1).unwrap()[0], at(&ids)).unwrap(), None);

            // A compaction dropping some.
            let mut mapping = Vec::new();
            let mut kept = Vec::new();
            for (p, id) in ids.iter().enumerate() {
                if dropped.get(p % 97).copied().unwrap_or(false) {
                    mapping.push(None);
                } else {
                    mapping.push(Some(kept.len() as u32));
                    kept.push(*id);
                }
            }
            index.remap(&mapping);
            for (p, id) in ids.iter().enumerate() {
                prop_assert_eq!(index.find(*id, at(&kept)).unwrap(), mapping[p]);
            }
        }
    }

    #[test]
    fn the_last_position_of_an_id_wins() {
        let ids = mint(2).unwrap();
        let held = [ids[0], ids[1], ids[0], ids[1]];
        let at = |p: u32| Ok(held[p as usize]);
        let mut sorted: Vec<u64> = held[..3].iter().zip(0..).map(|(u, p)| pack(u, p)).collect();
        sorted.sort_unstable();
        let mut index = UidIndex { sorted, recent: HashMap::new() };
        assert_eq!(index.find(ids[0], at).unwrap(), Some(2));
        assert_eq!(index.find(ids[1], at).unwrap(), Some(1));
        index.insert(ids[1], 3);
        assert_eq!(index.find(ids[1], at).unwrap(), Some(3), "added after the sorted ones");
    }
}
