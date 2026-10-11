//! Fixed-offset byte reading for the ChessBase classic format (see
//! `docs/io-03-cbh.md`). Byte order varies by file and even by field, so
//! every helper here is explicit about which it reads; there is no default.

pub fn be16(b: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([b[off], b[off + 1]])
}

pub fn be24(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([0, b[off], b[off + 1], b[off + 2]])
}

pub fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

pub fn le32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Latin-1, zero-terminated within a fixed-width field; bytes after the
/// terminator (if any) are leftovers and ignored.
pub fn fixed_latin1(b: &[u8], off: usize, len: usize) -> String {
    let raw = &b[off..off + len];
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    raw[..end].iter().map(|&c| c as char).collect()
}

/// A packed ChessBase date: `day (0-4) | month (5-8) | year (9-20)`, each
/// part 0 if unset. Returns PGN-style `YYYY.MM.DD` (`??` for an unset part),
/// or an empty string if every part is unset.
pub fn packed_date_to_pgn(packed: u32) -> String {
    let day = packed & 0x1f;
    let month = (packed >> 5) & 0xf;
    let year = (packed >> 9) & 0xfff;
    if day == 0 && month == 0 && year == 0 {
        return String::new();
    }
    let year = if year == 0 { "????".to_owned() } else { format!("{year:04}") };
    let month = if month == 0 { "??".to_owned() } else { format!("{month:02}") };
    let day = if day == 0 { "??".to_owned() } else { format!("{day:02}") };
    format!("{year}.{month}.{day}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_match_endianness() {
        let b = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(be16(&b, 0), 0x1234);
        assert_eq!(be32(&b, 0), 0x1234_5678);
        assert_eq!(le32(&b, 0), 0x7856_3412);
        assert_eq!(be24(&[0x00, 0x12, 0x34, 0x56], 1), 0x0012_3456);
    }

    #[test]
    fn latin1_stops_at_nul() {
        let mut buf = b"R\xe9ti".to_vec();
        buf.extend_from_slice(&[0, 0xff, 0xff]);
        assert_eq!(fixed_latin1(&buf, 0, buf.len()), "Réti");
    }

    #[test]
    fn packed_date_parts() {
        assert_eq!(packed_date_to_pgn(0), "");
        // 2018-10-22: year=2018 (bits 9-20), month=10 (bits 5-8), day=22 (bits 0-4)
        let packed = (2018 << 9) | (10 << 5) | 22;
        assert_eq!(packed_date_to_pgn(packed), "2018.10.22");
        // Year only.
        assert_eq!(packed_date_to_pgn(2018 << 9), "2018.??.??");
    }
}
