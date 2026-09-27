//! Little-endian byte encoding helpers with LEB128 varints.

use crate::DbError;

pub fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

pub fn put_str(buf: &mut Vec<u8>, s: &str) {
    put_varint(buf, s.len() as u64);
    buf.extend_from_slice(s.as_bytes());
}

pub struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(data: &'a [u8]) -> ByteReader<'a> {
        ByteReader { data, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn corrupt(&self, what: &str) -> DbError {
        DbError::Corrupt(format!("{what} at byte {}", self.pos))
    }

    pub fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    pub fn u8(&mut self) -> Result<u8, DbError> {
        let b = self.peek().ok_or_else(|| self.corrupt("unexpected end"))?;
        self.pos += 1;
        Ok(b)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DbError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.ok_or_else(|| self.corrupt("truncated data"))?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub fn varint(&mut self) -> Result<u64, DbError> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(self.corrupt("varint too long"))
    }

    pub fn str(&mut self) -> Result<String, DbError> {
        let len = self.varint()? as usize;
        let bytes = self.bytes(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| self.corrupt("invalid UTF-8"))
    }
}

pub fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

pub fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().expect("4 bytes"))
}

pub fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().expect("8 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_and_strings() {
        let mut buf = Vec::new();
        for v in [0, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            put_varint(&mut buf, v);
        }
        put_str(&mut buf, "Réti");
        let mut r = ByteReader::new(&buf);
        for v in [0, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            assert_eq!(r.varint().unwrap(), v);
        }
        assert_eq!(r.str().unwrap(), "Réti");
        assert!(r.is_empty());
        assert!(r.u8().is_err());
    }
}
