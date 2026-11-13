//! Links from games to Lichess study chapters (NET-13), in `games.links`.
//!
//! A linked game is kept in step with one chapter of a study. Its link
//! ([`StudyLink`]) names the study and the chapter, how the game follows
//! them ([`SyncMode`]), when Lichess last said the study changed, the
//! chapter as Lichess itself last exported it (the base of a later
//! three-way merge) and what only this database holds of the game (the
//! local-only layer, made by `chess_net`; opaque bytes here).
//!
//! Links are keyed by permanent id ([`GameUid`]), like `games.origins`: a
//! compaction's new slots don't touch them, and an app that doesn't know the
//! file (format version 3 from before it) can't make them wrong, only leave
//! links of games it compacted away, which nothing finds (see
//! `docs/file-format.md` › `games.links`).
//!
//! The file is a log: a 32-byte header whose committed end is written last
//! (as `games.idx`'s count is), then entries that set or remove a game's
//! link, the last for a game winning. It's read on first use, never by
//! opening the database. Once more than half of a file over 1 MB is entries
//! replaced since, it's written again whole (temp file + rename), and a
//! compaction keeps only the links of the games it keeps.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::bytes::{ByteReader, put_bytes, put_str, put_varint};
use crate::io::{read_exact_at, write_all_at};
use crate::uid::{GameUid, tmp_path};
use crate::{DbError, GameId};

pub(crate) const LINKS_FILE: &str = "games.links";

const MAGIC: &[u8; 8] = b"CHDBLNK1";
/// Version in the header. A newer one is refused by the link calls only:
/// the database still opens, and a compaction leaves the file as it is.
const VERSION: u32 = 1;
const HEADER: u64 = 32;
/// Where the header holds the committed end (u64).
const END_AT: u64 = 16;

const OP_SET: u8 = 1;
const OP_REMOVE: u8 = 2;

const MODE_FOLLOW: u8 = 1;
const MODE_TWO_WAY: u8 = 2;

/// How a base snapshot is stored: its UTF-8 bytes, or those deflated
/// (RFC 1951, raw) after the text's length.
const BASE_PLAIN: u8 = 0;
const BASE_DEFLATE: u8 = 1;

/// The longest study or chapter id taken, in bytes (Lichess's have 8).
pub const MAX_LINK_ID: usize = 64;
/// The longest base snapshot or layer taken: a chapter's export can't be
/// more than a whole study's (`chess_net` takes 64 MB).
const MAX_PART: usize = 64 << 20;
/// A file this big, half of it entries replaced since, is written again.
const REWRITE_AT: u64 = 1 << 20;

/// How a linked game follows its chapter. (A study downloaded once, NET-14's
/// static copy, has no link.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncMode {
    /// Follow only: what changes on Lichess comes here, nothing goes back.
    Follow,
    /// Two-way: changes go both ways.
    TwoWay,
}

impl SyncMode {
    fn to_u8(self) -> u8 {
        match self {
            SyncMode::Follow => MODE_FOLLOW,
            SyncMode::TwoWay => MODE_TWO_WAY,
        }
    }

    fn from_u8(b: u8) -> Option<SyncMode> {
        match b {
            MODE_FOLLOW => Some(SyncMode::Follow),
            MODE_TWO_WAY => Some(SyncMode::TwoWay),
            _ => None,
        }
    }
}

/// A game's link to a study chapter (NET-13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudyLink {
    /// The study's id on Lichess (`lichess.org/study/{study}`).
    pub study: String,
    /// The chapter's (`…/study/{study}/{chapter}`).
    pub chapter: String,
    pub mode: SyncMode,
    /// The study's `Last-Modified` when `base` was fetched, as Lichess wrote
    /// it, for `If-Modified-Since` (Lichess dates the whole study, not each
    /// chapter); `None` when it didn't say.
    pub last_modified: Option<String>,
    /// The chapter's PGN as Lichess exported it at the last sync, as it
    /// came: the base of a three-way merge. It must be Lichess's own export
    /// (fetched again after a push), never what was sent, so that what
    /// Lichess rewrites (its ECO, a result comment, rounded clocks) is in
    /// the base too and never looks like an edit made there.
    pub base: String,
    /// What only this database holds of the game, Lichess can't (evals,
    /// `%emt`, …): `chess_net`'s local-only layer, kept as it gives it.
    pub layer: Vec<u8>,
}

/// A linked game, as the listings give it (without its base and layer:
/// [`Database::link`](crate::Database::link) has them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedGame {
    pub uid: GameUid,
    /// Its slot now (a compaction changes it).
    pub game: GameId,
    pub study: String,
    pub chapter: String,
    pub mode: SyncMode,
    pub last_modified: Option<String>,
}

/// What's kept in memory of a link: all but its base and layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Meta {
    /// Where its entry starts (its length field) and its whole length.
    offset: u64,
    len: u64,
    pub(crate) study: String,
    pub(crate) chapter: String,
    pub(crate) mode: SyncMode,
    pub(crate) last_modified: Option<String>,
}

/// `games.links`, read: where each game's link is, by permanent id and by
/// chapter.
#[derive(Debug)]
pub(crate) struct Links {
    path: PathBuf,
    /// `None` while there's no file (no link was ever set).
    file: Option<File>,
    /// The committed end: bytes past it are a commit cut short.
    end: u64,
    live: HashMap<GameUid, Meta>,
    by_chapter: HashMap<(String, String), GameUid>,
    /// The bytes of the entries that hold the links now; the rest of the
    /// file is entries replaced since.
    live_bytes: u64,
}

fn corrupt(path: &Path, what: impl std::fmt::Display) -> DbError {
    DbError::Corrupt(format!("{}: {what}", path.display()))
}

fn header(end: u64) -> [u8; HEADER as usize] {
    let mut h = [0u8; HEADER as usize];
    h[..8].copy_from_slice(MAGIC);
    h[8..12].copy_from_slice(&VERSION.to_le_bytes());
    h[16..24].copy_from_slice(&end.to_le_bytes());
    h
}

/// The committed end in a links file's header, checked.
fn read_header(file: &File, path: &Path) -> Result<u64, DbError> {
    let mut h = [0u8; 24];
    read_exact_at(file, &mut h, 0).map_err(|_| corrupt(path, "truncated header"))?;
    if &h[..8] != MAGIC {
        return Err(corrupt(path, "bad header"));
    }
    let version = u32::from_le_bytes([h[8], h[9], h[10], h[11]]);
    if version != VERSION {
        return Err(DbError::UnsupportedVersion(version));
    }
    let end = u64::from_le_bytes(h[16..24].try_into().expect("8 bytes"));
    if end < HEADER {
        return Err(corrupt(path, "bad committed length"));
    }
    Ok(end)
}

/// A study or chapter id, as a link takes it: 1 to [`MAX_LINK_ID`] bytes,
/// no spaces or control characters.
fn check_id(id: &str, what: &str) -> Result<(), DbError> {
    if id.is_empty() || id.len() > MAX_LINK_ID || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(DbError::Invalid(format!("\"{id}\" isn't a {what} id")));
    }
    Ok(())
}

/// Whether a link can be stored (checked before anything is written).
pub(crate) fn check_link(link: &StudyLink) -> Result<(), DbError> {
    check_id(&link.study, "study")?;
    check_id(&link.chapter, "chapter")?;
    if link.base.len() > MAX_PART || link.layer.len() > MAX_PART {
        return Err(DbError::Invalid("a study link's base or layer is over 64 MB".into()));
    }
    Ok(())
}

fn entry(body: Vec<u8>) -> Vec<u8> {
    // Both parts are at most 64 MB (`check_link`): the length fits.
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

fn encode_set(uid: GameUid, link: &StudyLink) -> Vec<u8> {
    let mut b = Vec::with_capacity(64 + link.base.len() / 3 + link.layer.len());
    b.push(OP_SET);
    b.extend_from_slice(&uid.to_bytes());
    put_str(&mut b, &link.study);
    put_str(&mut b, &link.chapter);
    b.push(link.mode.to_u8());
    put_str(&mut b, link.last_modified.as_deref().unwrap_or(""));
    let deflated = miniz_oxide::deflate::compress_to_vec(link.base.as_bytes(), 6);
    if deflated.len() < link.base.len() {
        b.push(BASE_DEFLATE);
        put_varint(&mut b, link.base.len() as u64);
        put_bytes(&mut b, &deflated);
    } else {
        b.push(BASE_PLAIN);
        put_bytes(&mut b, link.base.as_bytes());
    }
    put_bytes(&mut b, &link.layer);
    entry(b)
}

fn encode_remove(uid: GameUid) -> Vec<u8> {
    let mut b = Vec::with_capacity(17);
    b.push(OP_REMOVE);
    b.extend_from_slice(&uid.to_bytes());
    entry(b)
}

/// An entry's body up to its link's base: (op, game, its link's fields).
fn decode_head(r: &mut ByteReader<'_>) -> Result<(u8, GameUid, Option<Meta>), DbError> {
    let op = r.u8()?;
    let uid = GameUid::from_bytes(r.bytes(16)?.try_into().expect("16 bytes"));
    if op == OP_REMOVE {
        return Ok((op, uid, None));
    }
    if op != OP_SET {
        return Err(DbError::Corrupt(format!("unknown link entry kind {op}")));
    }
    let study = r.str()?;
    let chapter = r.str()?;
    let mode = r.u8()?;
    let mode =
        SyncMode::from_u8(mode).ok_or_else(|| DbError::Corrupt(format!("unknown sync mode {mode}")))?;
    let last_modified = Some(r.str()?).filter(|s| !s.is_empty());
    Ok((op, uid, Some(Meta { offset: 0, len: 0, study, chapter, mode, last_modified })))
}

/// The base and layer after [`decode_head`].
fn decode_tail(r: &mut ByteReader<'_>) -> Result<(String, Vec<u8>), DbError> {
    let base = match r.u8()? {
        BASE_PLAIN => r.len_bytes()?.to_vec(),
        BASE_DEFLATE => {
            let len = r.varint()?;
            let len = usize::try_from(len)
                .ok()
                .filter(|&l| l <= MAX_PART)
                .ok_or_else(|| DbError::Corrupt("a study link's base is too long".into()))?;
            let text = miniz_oxide::inflate::decompress_to_vec_with_limit(r.len_bytes()?, len)
                .map_err(|e| DbError::Corrupt(format!("a study link's base can't be inflated: {e}")))?;
            if text.len() != len {
                return Err(DbError::Corrupt("a study link's base has the wrong length".into()));
            }
            text
        }
        other => return Err(DbError::Corrupt(format!("unknown base encoding {other}"))),
    };
    let base =
        String::from_utf8(base).map_err(|_| DbError::Corrupt("a study link's base isn't UTF-8".into()))?;
    let layer = r.len_bytes()?.to_vec();
    if !r.is_empty() {
        return Err(DbError::Corrupt("trailing bytes in a study link".into()));
    }
    Ok((base, layer))
}

impl Links {
    fn empty(path: &Path) -> Links {
        Links {
            path: path.to_path_buf(),
            file: None,
            end: HEADER,
            live: HashMap::new(),
            by_chapter: HashMap::new(),
            live_bytes: 0,
        }
    }

    /// Reads the links at `path` (none if there's no file), dropping the
    /// bytes of a commit a crash cut short.
    pub(crate) fn open(path: &Path) -> Result<Links, DbError> {
        let mut links = Links::empty(path);
        if !path.exists() {
            return Ok(links);
        }
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let end = read_header(&file, path)?;
        let len = file.metadata()?.len();
        if len < end {
            return Err(corrupt(path, format!("{len} bytes, but its header says {end}")));
        }
        if len > end {
            log::warn!("dropping a study link commit cut short in {}", path.display());
            file.set_len(end)?;
            file.sync_data()?;
        }
        let size = usize::try_from(end - HEADER).map_err(|_| corrupt(path, "too large"))?;
        let mut buf = vec![0u8; size];
        read_exact_at(&file, &mut buf, HEADER)?;
        let mut at = 0usize;
        while at < buf.len() {
            let body_len = buf
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
                .ok_or_else(|| corrupt(path, "a torn entry before the committed end"))?;
            let body = buf
                .get(at + 4..at + 4 + body_len)
                .ok_or_else(|| corrupt(path, "a torn entry before the committed end"))?;
            let (_, uid, meta) = decode_head(&mut ByteReader::new(body)).map_err(|e| corrupt(path, e))?;
            match meta {
                Some(meta) => {
                    let len = 4 + body_len as u64;
                    links.insert(uid, Meta { offset: HEADER + at as u64, len, ..meta });
                }
                None => links.forget(uid),
            }
            at += 4 + body_len;
        }
        links.file = Some(file);
        links.end = end;
        Ok(links)
    }

    /// Whether the file has anything in it.
    pub(crate) fn has_file(&self) -> bool {
        self.file.is_some()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    pub(crate) fn meta(&self, uid: GameUid) -> Option<&Meta> {
        self.live.get(&uid)
    }

    /// The game whose link is to this chapter, if one is.
    pub(crate) fn holder(&self, study: &str, chapter: &str) -> Option<GameUid> {
        self.by_chapter.get(&(study.to_owned(), chapter.to_owned())).copied()
    }

    /// Every link, by game.
    pub(crate) fn all(&self) -> impl Iterator<Item = (GameUid, &Meta)> {
        self.live.iter().map(|(u, m)| (*u, m))
    }

    /// A link set: it takes its chapter from whatever game had it, which
    /// then has none (a batch that swaps two games' chapters sets both).
    fn insert(&mut self, uid: GameUid, meta: Meta) {
        self.forget(uid);
        let key = (meta.study.clone(), meta.chapter.clone());
        if let Some(other) = self.by_chapter.insert(key, uid)
            && let Some(old) = self.live.remove(&other)
        {
            self.live_bytes -= old.len;
        }
        self.live_bytes += meta.len;
        self.live.insert(uid, meta);
    }

    fn forget(&mut self, uid: GameUid) {
        if let Some(old) = self.live.remove(&uid) {
            self.live_bytes -= old.len;
            let key = (old.study, old.chapter);
            if self.by_chapter.get(&key) == Some(&uid) {
                self.by_chapter.remove(&key);
            }
        }
    }

    /// The link of game `uid`, base and layer read from the file.
    pub(crate) fn get(&self, uid: GameUid) -> Result<Option<StudyLink>, DbError> {
        let (Some(meta), Some(file)) = (self.live.get(&uid), &self.file) else { return Ok(None) };
        let mut buf = vec![0u8; meta.len as usize];
        read_exact_at(file, &mut buf, meta.offset)?;
        let mut r = ByteReader::new(&buf[4..]);
        let (_, _, head) = decode_head(&mut r).map_err(|e| corrupt(&self.path, e))?;
        let head = head.ok_or_else(|| corrupt(&self.path, "a link points at a removal"))?;
        let (base, layer) = decode_tail(&mut r).map_err(|e| corrupt(&self.path, e))?;
        Ok(Some(StudyLink {
            study: head.study,
            chapter: head.chapter,
            mode: head.mode,
            last_modified: head.last_modified,
            base,
            layer,
        }))
    }

    /// The open file, made (empty, temp file + rename) if there's none yet.
    fn file(&mut self) -> Result<&File, DbError> {
        if self.file.is_none() {
            let tmp = tmp_path(&self.path);
            {
                let f = File::create(&tmp)?;
                write_all_at(&f, &header(HEADER), 0)?;
                f.sync_all()?;
            }
            std::fs::rename(&tmp, &self.path)?;
            self.file = Some(OpenOptions::new().read(true).write(true).open(&self.path)?);
            self.end = HEADER;
        }
        Ok(self.file.as_ref().expect("just made"))
    }

    /// Sets (`Some`) or removes (`None`) games' links in one crash-safe
    /// commit: the entries are written past the committed end and synced,
    /// then the new end in the header, synced. Nothing in memory changes
    /// unless it all worked. The caller has checked the links
    /// ([`check_link`]).
    pub(crate) fn commit(&mut self, changes: &[(GameUid, Option<&StudyLink>)]) -> Result<(), DbError> {
        if changes.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::new();
        let mut done = Vec::with_capacity(changes.len());
        for &(uid, link) in changes {
            let offset = self.end + bytes.len() as u64;
            match link {
                Some(link) => {
                    let e = encode_set(uid, link);
                    let meta = Meta {
                        offset,
                        len: e.len() as u64,
                        study: link.study.clone(),
                        chapter: link.chapter.clone(),
                        mode: link.mode,
                        last_modified: link.last_modified.clone(),
                    };
                    done.push((uid, Some(meta)));
                    bytes.extend(e);
                }
                None => {
                    done.push((uid, None));
                    bytes.extend(encode_remove(uid));
                }
            }
        }
        let start = self.end;
        let file = self.file()?;
        write_all_at(file, &bytes, start)?;
        file.sync_data()?;
        let end = start + bytes.len() as u64;
        write_all_at(file, &end.to_le_bytes(), END_AT)?;
        file.sync_data()?;
        self.end = end;
        for (uid, meta) in done {
            match meta {
                Some(meta) => self.insert(uid, meta),
                None => self.forget(uid),
            }
        }
        Ok(())
    }

    /// Whether the file is mostly entries replaced since.
    pub(crate) fn wants_rewrite(&self) -> bool {
        let size = self.end - HEADER;
        size > REWRITE_AT && self.live_bytes < size / 2
    }

    /// Writes a whole new file at `path` with the links `keep` says yes to,
    /// their entries as they are, in their order; synced and closed.
    pub(crate) fn write_kept(
        &self,
        path: &Path,
        mut keep: impl FnMut(GameUid) -> Result<bool, DbError>,
    ) -> Result<(), DbError> {
        let mut kept: Vec<(&GameUid, &Meta)> = Vec::new();
        for (uid, meta) in &self.live {
            if keep(*uid)? {
                kept.push((uid, meta));
            }
        }
        kept.sort_by_key(|(_, m)| m.offset);
        let out = File::create(path)?;
        let mut end = HEADER;
        if let Some(file) = &self.file {
            for (_, meta) in kept {
                let mut buf = vec![0u8; meta.len as usize];
                read_exact_at(file, &mut buf, meta.offset)?;
                write_all_at(&out, &buf, end)?;
                end += meta.len;
            }
        }
        write_all_at(&out, &header(end), 0)?;
        out.sync_all()?;
        Ok(())
    }

    /// Writes the file again without the entries replaced since, as a temp
    /// file renamed over it, and reads it back. After an error the file is
    /// the old one or the new one, both whole: the caller reads it again.
    pub(crate) fn rewrite(&mut self) -> Result<(), DbError> {
        let tmp = tmp_path(&self.path);
        self.write_kept(&tmp, |_| Ok(true))?;
        // Closed first: Windows doesn't replace an open file.
        self.file = None;
        std::fs::rename(&tmp, &self.path)?;
        *self = Links::open(&self.path)?;
        Ok(())
    }
}

#[cfg(test)]
impl Links {
    pub(crate) fn file_len(&self) -> u64 {
        self.end
    }

    /// The links read, their file then reopened read-only, so that every
    /// write to it fails (a commit whose links can't be written).
    pub(crate) fn read_only(path: &Path) -> Links {
        let mut links = Links::open(path).expect("links");
        links.file = Some(File::open(path).expect("games.links"));
        links
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uid::mint;

    fn link(chapter: &str, base: &str) -> StudyLink {
        StudyLink {
            study: "abcdEFGH".into(),
            chapter: chapter.into(),
            mode: SyncMode::TwoWay,
            last_modified: Some("Thu, 12 Nov 2026 20:15:00 GMT".into()),
            base: base.into(),
            layer: vec![1, 2, 3],
        }
    }

    #[test]
    fn entries_round_trip_plain_and_deflated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LINKS_FILE);
        let uids = mint(3).unwrap();
        let long = "1. e4 { A long comment that repeats. } e5 ".repeat(50);
        let mut l = Links::open(&path).unwrap();
        assert!(!l.has_file() && !path.exists(), "no file until a link is set");
        let a = link("chapter1", "");
        let b = StudyLink {
            mode: SyncMode::Follow,
            last_modified: None,
            layer: Vec::new(),
            ..link("chapter2", &long)
        };
        let c = link("chapter3", "[Event \"Ünïcödé ♘\"]\n\n*");
        l.commit(&[(uids[0], Some(&a)), (uids[1], Some(&b)), (uids[2], Some(&c))]).unwrap();
        assert!(l.file_len() < HEADER + long.len() as u64, "the long base is deflated");
        drop(l);
        let l = Links::open(&path).unwrap();
        assert_eq!(l.get(uids[0]).unwrap(), Some(a));
        assert_eq!(l.get(uids[1]).unwrap(), Some(b));
        assert_eq!(l.get(uids[2]).unwrap(), Some(c));
        assert_eq!(l.holder("abcdEFGH", "chapter2"), Some(uids[1]));
        assert_eq!(l.get(mint(1).unwrap()[0]).unwrap(), None);
    }

    #[test]
    fn a_set_takes_its_chapter_and_the_last_entry_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LINKS_FILE);
        let [x, y] = <[GameUid; 2]>::try_from(mint(2).unwrap()).unwrap();
        let mut l = Links::open(&path).unwrap();
        l.commit(&[(x, Some(&link("one", "x1"))), (y, Some(&link("two", "y")))]).unwrap();
        // x moves to "two", taking it from y, which has no link then.
        l.commit(&[(x, Some(&link("two", "x2")))]).unwrap();
        let check = |l: &Links| {
            assert_eq!(l.holder("abcdEFGH", "one"), None);
            assert_eq!(l.holder("abcdEFGH", "two"), Some(x));
            assert_eq!(l.get(y).unwrap(), None);
            assert_eq!(l.get(x).unwrap().unwrap().base, "x2");
        };
        check(&l);
        check(&Links::open(&path).unwrap());
        // A swap in one batch.
        l.commit(&[(x, Some(&link("one", "x3"))), (y, Some(&link("two", "y2")))]).unwrap();
        l.commit(&[(x, Some(&link("two", "x4"))), (y, Some(&link("one", "y3")))]).unwrap();
        for l in [&l, &Links::open(&path).unwrap()] {
            assert_eq!((l.holder("abcdEFGH", "one"), l.holder("abcdEFGH", "two")), (Some(y), Some(x)));
        }
        l.commit(&[(y, None)]).unwrap();
        let l = Links::open(&path).unwrap();
        assert_eq!((l.holder("abcdEFGH", "one"), l.get(y).unwrap()), (None, None));
    }

    /// Bytes past the committed end (a commit cut short, its header not
    /// written) are dropped; a header that isn't one is refused; a newer
    /// version is refused as such.
    #[test]
    fn a_commit_cut_short_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LINKS_FILE);
        let uids = mint(2).unwrap();
        let mut l = Links::open(&path).unwrap();
        l.commit(&[(uids[0], Some(&link("one", "a")))]).unwrap();
        let committed = std::fs::read(&path).unwrap();
        drop(l);
        let mut torn = committed.clone();
        torn.extend(encode_set(uids[1], &link("two", "b")));
        torn.extend([7, 7, 7]);
        std::fs::write(&path, &torn).unwrap();
        let l = Links::open(&path).unwrap();
        assert_eq!(l.get(uids[1]).unwrap(), None);
        assert_eq!(l.get(uids[0]).unwrap().unwrap().base, "a");
        assert_eq!(std::fs::read(&path).unwrap(), committed, "truncated to the committed end");
        drop(l);

        let mut bad = committed.clone();
        bad[8] = 2;
        std::fs::write(&path, &bad).unwrap();
        assert!(matches!(Links::open(&path), Err(DbError::UnsupportedVersion(2))));
        bad[0] = b'X';
        std::fs::write(&path, &bad).unwrap();
        assert!(matches!(Links::open(&path), Err(DbError::Corrupt(_))));
        // A committed end past the file's end.
        let mut short = committed.clone();
        short.truncate(committed.len() - 1);
        std::fs::write(&path, &short).unwrap();
        assert!(matches!(Links::open(&path), Err(DbError::Corrupt(_))));
    }

    #[test]
    fn a_file_mostly_replaced_is_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LINKS_FILE);
        let uids = mint(2).unwrap();
        let mut l = Links::open(&path).unwrap();
        // Bases that don't deflate much: random hex, about 10 kB deflated.
        let base = || mint(600).unwrap().iter().map(|u| u.to_string()).collect::<String>();
        l.commit(&[(uids[1], Some(&link("keep", "kept")))]).unwrap();
        let mut n = 0;
        let mut last = String::new();
        while !l.wants_rewrite() {
            last = base();
            l.commit(&[(uids[0], Some(&link("often", &last)))]).unwrap();
            n += 1;
            assert!(n < 1000);
        }
        let before = l.file_len();
        l.rewrite().unwrap();
        assert!(l.file_len() < before / 2, "{} → {}", before, l.file_len());
        assert!(!tmp_path(&path).exists());
        for l in [&l, &Links::open(&path).unwrap()] {
            assert_eq!(l.get(uids[0]).unwrap().unwrap().base, last);
            assert_eq!(l.get(uids[1]).unwrap().unwrap().base, "kept");
        }
        l.commit(&[(uids[1], None)]).unwrap();
        assert_eq!(Links::open(&path).unwrap().get(uids[1]).unwrap(), None);
    }

    #[test]
    fn ids_are_checked() {
        for (study, chapter) in
            [("", "x"), ("abc", ""), ("a b", "x"), ("abc", "x\n"), (&*"x".repeat(65), "x")]
        {
            let l = StudyLink { study: study.into(), chapter: chapter.into(), ..link("c", "") };
            assert!(matches!(check_link(&l), Err(DbError::Invalid(_))), "{study:?} {chapter:?}");
        }
        assert!(check_link(&link("c", "")).is_ok());
    }
}
