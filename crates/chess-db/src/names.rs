//! Append-only name tables (`players.names`, `events.names`, …).
//!
//! File: 16-byte header (`magic`, version) followed by entries of
//! `len:varint utf8-bytes`. An entry's id is its ordinal; id 0 is always the
//! empty string ("unknown"). Entries are only referenced by committed index
//! records, which are written after the names, so a torn tail entry after a
//! crash is unreferenced and gets truncated on open.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::DbError;
use crate::bytes::{ByteReader, put_str};
use crate::io::write_all_at;

const MAGIC: &[u8; 8] = b"CHDBNAME";
const VERSION: u32 = 1;
const HEADER_SIZE: u64 = 16;

pub struct NameTable {
    path: PathBuf,
    file: File,
    names: Vec<String>,
    lookup: HashMap<String, u32>,
    file_len: u64,
    /// Encoded entries not yet written.
    pending: Vec<u8>,
    /// Case- and accent-folded names for searching, filled lazily.
    folded: Mutex<Vec<String>>,
}

impl NameTable {
    pub fn create(path: &Path) -> Result<NameTable, DbError> {
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
        let mut header = Vec::with_capacity(HEADER_SIZE as usize);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.resize(HEADER_SIZE as usize, 0);
        let mut table = NameTable {
            path: path.to_path_buf(),
            file,
            names: Vec::new(),
            lookup: HashMap::new(),
            file_len: 0,
            pending: header,
            folded: Mutex::default(),
        };
        table.intern("");
        table.flush()?;
        Ok(table)
    }

    pub fn open(path: &Path) -> Result<NameTable, DbError> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let data = std::fs::read(path)?;
        if data.len() < HEADER_SIZE as usize || &data[..8] != MAGIC {
            return Err(DbError::Corrupt(format!("{} is not a name table", path.display())));
        }
        let version = u32::from_le_bytes(data[8..12].try_into().expect("4 bytes"));
        if version != VERSION {
            return Err(DbError::UnsupportedVersion(version));
        }
        let mut names = Vec::new();
        let mut lookup = HashMap::new();
        let body = &data[HEADER_SIZE as usize..];
        let mut r = ByteReader::new(body);
        let mut valid_len = 0;
        while !r.is_empty() {
            match r.str() {
                Ok(name) => {
                    lookup.entry(name.clone()).or_insert(names.len() as u32);
                    names.push(name);
                    valid_len = r.position();
                }
                Err(_) => break, // torn tail from an interrupted write
            }
        }
        let file_len = HEADER_SIZE + valid_len as u64;
        if file_len < data.len() as u64 {
            log::warn!("truncating torn tail of {}", path.display());
            file.set_len(file_len)?;
        }
        if names.first().map(String::as_str) != Some("") {
            return Err(DbError::Corrupt(format!("{} lacks the empty name", path.display())));
        }
        Ok(NameTable {
            path: path.to_path_buf(),
            file,
            names,
            lookup,
            file_len,
            pending: Vec::new(),
            folded: Mutex::default(),
        })
    }

    /// Id for `name`, adding it (unflushed) if new.
    pub fn intern(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.lookup.get(name) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(name.to_owned());
        self.lookup.insert(name.to_owned(), id);
        put_str(&mut self.pending, name);
        id
    }

    /// Points the table at its file's new name (after a rename).
    pub(crate) fn set_path(&mut self, path: &Path) {
        self.path = path.to_path_buf();
    }

    /// The id of `name`, if the table has it.
    pub fn id_of(&self, name: &str) -> Option<u32> {
        self.lookup.get(name).copied()
    }

    /// Renames entry `id` to `name`, which must not be in the table yet
    /// (merging two names is [`crate::Database::rename_name`]'s job). Ids stay;
    /// the file is rewritten (temp file + rename).
    pub fn rename(&mut self, id: u32, name: &str) -> Result<(), DbError> {
        debug_assert!(id != 0 && (id as usize) < self.names.len() && !self.lookup.contains_key(name));
        self.flush()?;
        let old = std::mem::replace(&mut self.names[id as usize], name.to_owned());
        if self.lookup.get(&old) == Some(&id) {
            self.lookup.remove(&old);
            // Another entry with the old spelling (duplicates are rare) takes over.
            if let Some(other) = self.names.iter().position(|n| *n == old) {
                self.lookup.insert(old, other as u32);
            }
        }
        self.lookup.insert(name.to_owned(), id);
        if let Some(f) = self.folded.lock().expect("folded names lock").get_mut(id as usize) {
            *f = crate::fold(name);
        }

        let mut buf =
            Vec::with_capacity(HEADER_SIZE as usize + self.names.iter().map(|n| n.len() + 2).sum::<usize>());
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&VERSION.to_le_bytes());
        buf.resize(HEADER_SIZE as usize, 0);
        for n in &self.names {
            put_str(&mut buf, n);
        }
        let mut tmp_name = self.path.as_os_str().to_owned();
        tmp_name.push(".tmp");
        let tmp = PathBuf::from(tmp_name);
        {
            let f = File::create(&tmp)?;
            write_all_at(&f, &buf, 0)?;
            f.sync_all()?;
        }
        // Close our handle before replacing the file (Windows).
        drop(std::mem::replace(&mut self.file, File::open(&tmp)?));
        std::fs::rename(&tmp, &self.path)?;
        self.file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        self.file_len = buf.len() as u64;
        Ok(())
    }

    pub fn get(&self, id: u32) -> &str {
        self.names.get(id as usize).map_or("", String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Writes pending entries and syncs.
    pub fn flush(&mut self) -> Result<(), DbError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        write_all_at(&self.file, &self.pending, self.file_len)?;
        self.file.sync_data()?;
        self.file_len += self.pending.len() as u64;
        self.pending.clear();
        Ok(())
    }

    /// For every id, whether its folded name (see [`crate::fold`]) passes
    /// `pred`. Folded names are cached, so repeated searches are cheap.
    pub fn matches(&self, pred: impl Fn(&str) -> bool) -> Vec<bool> {
        let mut folded = self.folded.lock().expect("folded names lock");
        let have = folded.len();
        folded.extend(self.names[have..].iter().map(|n| crate::fold(n)));
        folded.iter().map(|n| pred(n)).collect()
    }

    /// Rank of every id in case-insensitive alphabetical order (for sorting).
    pub fn ranks(&self) -> Vec<u32> {
        let keys: Vec<String> = self.names.iter().map(|n| n.to_lowercase()).collect();
        let mut ids: Vec<u32> = (0..self.names.len() as u32).collect();
        // Unknown ("") sorts last.
        ids.sort_by(|&a, &b| {
            let (ka, kb) = (&keys[a as usize], &keys[b as usize]);
            (ka.is_empty(), ka, a).cmp(&(kb.is_empty(), kb, b))
        });
        let mut ranks = vec![0u32; ids.len()];
        for (rank, id) in ids.into_iter().enumerate() {
            ranks[id as usize] = rank as u32;
        }
        ranks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_recovers_from_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("players.names");
        let mut t = NameTable::create(&path).unwrap();
        assert_eq!(t.intern("Carlsen, Magnus"), 1);
        assert_eq!(t.intern("Réti, Richard"), 2);
        assert_eq!(t.intern("Carlsen, Magnus"), 1);
        t.flush().unwrap();
        drop(t);

        // Simulate a torn write: a length prefix promising more bytes.
        let mut data = std::fs::read(&path).unwrap();
        let good_len = data.len();
        data.extend_from_slice(&[20, b'x', b'y']);
        std::fs::write(&path, &data).unwrap();

        let mut t = NameTable::open(&path).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t.get(2), "Réti, Richard");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), good_len as u64);
        assert_eq!(t.intern("Tal, Mikhail"), 3);
        t.flush().unwrap();
        assert_eq!(NameTable::open(&path).unwrap().get(3), "Tal, Mikhail");
    }

    #[test]
    fn rename_rewrites_the_file_and_keeps_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("players.names");
        let mut t = NameTable::create(&path).unwrap();
        let a = t.intern("Kasparov, G");
        let b = t.intern("Anand, V");
        t.flush().unwrap();
        assert!(t.matches(|n| n.contains("kasparov"))[a as usize]);
        t.rename(a, "Kasparov, Garry").unwrap();
        assert_eq!(t.id_of("Kasparov, Garry"), Some(a));
        assert_eq!(t.id_of("Kasparov, G"), None);
        assert!(t.matches(|n| n == "kasparov, garry")[a as usize], "search cache updated");
        let c = t.intern("Tal, M");
        t.flush().unwrap();
        drop(t);
        let t = NameTable::open(&path).unwrap();
        assert_eq!((t.get(a), t.get(b), t.get(c)), ("Kasparov, Garry", "Anand, V", "Tal, M"));
        assert!(!dir.path().join("players.names.tmp").exists());
    }

    #[test]
    fn ranks_sort_case_insensitively_with_unknown_last() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = NameTable::create(&dir.path().join("n")).unwrap();
        let b = t.intern("bob");
        let a = t.intern("Alice");
        let c = t.intern("Carol");
        let ranks = t.ranks();
        assert!(ranks[a as usize] < ranks[b as usize]);
        assert!(ranks[b as usize] < ranks[c as usize]);
        assert!(ranks[c as usize] < ranks[0]);
    }
}
