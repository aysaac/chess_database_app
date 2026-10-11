//! Entity tables: `.cbp` (players), `.cbt` (tournaments), `.cbc`
//! (annotators), `.cbs` (sources), `.cbe` (teams) — `docs/io-03-cbh.md` §3.4.
//!
//! All little-endian. A header, then fixed-size records that double as AVL
//! tree nodes; an entity's id is the 0-based position of its record, so we
//! only need direct indexing, never the tree itself.

use std::fs::File;
use std::path::Path;

use crate::DbError;
use crate::cb::bytes::{fixed_latin1, le32};
use crate::io::read_exact_at;

const NODE_PREFIX: u64 = 9;

pub struct EntityTable {
    file: File,
    header_size: u64,
    record_size: u64,
    pub count: u32,
}

impl EntityTable {
    /// `data_size` is the type's data size from `docs/io-03-cbh.md` §3.4
    /// (player 58, tournament 90, annotator 53, source 59, team 63).
    pub fn open(path: &Path, expected_data_size: u32) -> Result<EntityTable, DbError> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < 28 {
            return Err(DbError::NotADatabase(path.to_owned()));
        }
        let mut header = [0u8; 32];
        let header_len = if len >= 32 { 32 } else { 28 };
        read_exact_at(&file, &mut header[..header_len], 0)?;
        let magic = le32(&header, 0x08);
        if magic != 1_234_567_890 {
            return Err(DbError::NotADatabase(path.to_owned()));
        }
        let data_size = le32(&header, 0x0c);
        if data_size != expected_data_size {
            return Err(DbError::UnsupportedVersion(data_size));
        }
        let header_size = if header_len == 32 && le32(&header, 0x18) == 4 { 32 } else { 28 };
        let record_size = NODE_PREFIX + data_size as u64;
        // Re-derive the count from file size: the header's own "entity
        // count" field excludes deleted records, but ids run up to this.
        let record_count = ((len - header_size) / record_size) as u32;
        Ok(EntityTable { file, header_size, record_size, count: record_count })
    }

    /// The data bytes of entity `id` (0-based), node prefix excluded.
    pub fn data(&self, id: u32) -> Result<Vec<u8>, DbError> {
        if id >= self.count {
            return Err(DbError::NotFound(id));
        }
        let mut buf = vec![0u8; (self.record_size - NODE_PREFIX) as usize];
        let offset = self.header_size + id as u64 * self.record_size + NODE_PREFIX;
        read_exact_at(&self.file, &mut buf, offset)?;
        Ok(buf)
    }
}

pub fn player_name(data: &[u8]) -> String {
    let last = fixed_latin1(data, 0, 30);
    let first = fixed_latin1(data, 30, 20);
    if first.is_empty() { last } else { format!("{last}, {first}") }
}

pub fn tournament_title(data: &[u8]) -> String {
    fixed_latin1(data, 0, 40)
}

pub fn tournament_place(data: &[u8]) -> String {
    fixed_latin1(data, 40, 30)
}

pub fn annotator_name(data: &[u8]) -> String {
    fixed_latin1(data, 0, 45)
}

pub fn source_title(data: &[u8]) -> String {
    fixed_latin1(data, 0, 25)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_name_joins_last_and_first() {
        let mut data = vec![0u8; 50];
        data[0..4].copy_from_slice(b"Fox\0");
        data[30..35].copy_from_slice(b"Anna\0");
        assert_eq!(player_name(&data), "Fox, Anna");
    }

    #[test]
    fn player_name_without_first_is_just_last() {
        let mut data = vec![0u8; 50];
        data[0..4].copy_from_slice(b"Fox\0");
        assert_eq!(player_name(&data), "Fox");
    }
}
