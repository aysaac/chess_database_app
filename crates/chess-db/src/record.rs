//! Fixed-size (64-byte) index record per game in `games.idx`.
//!
//! Layout (little-endian):
//!
//! | off | size | field |
//! |-----|------|-------|
//! | 0   | 8    | blob offset in `games.blob` |
//! | 8   | 4    | blob length |
//! | 12  | 4    | flags (see `FLAG_*`) |
//! | 16  | 4×5  | name ids: white, black, event, site, annotator |
//! | 36  | 4    | date: `year << 9 \| month << 5 \| day` (0 = unknown part) |
//! | 40  | 2+2  | round, sub-round (0 = unknown) |
//! | 44  | 2+2  | white Elo, black Elo (0 = unknown) |
//! | 48  | 1    | result: 0 unknown, 1 white wins, 2 black wins, 3 draw |
//! | 49  | 1    | reserved |
//! | 50  | 2    | ECO: letter index (A=1…E=5) × 100 + number (0 = unknown) |
//! | 52  | 2    | main-line plies (saturating) |
//! | 54  | 10   | reserved (zero) |

use chess_core::game::GameResult;

use crate::bytes::{u16_at, u32_at, u64_at};

pub const RECORD_SIZE: usize = 64;

pub const FLAG_DELETED: u32 = 1 << 0;
pub const FLAG_VARIATIONS: u32 = 1 << 1;
pub const FLAG_COMMENTS: u32 = 1 << 2;
pub const FLAG_SETUP: u32 = 1 << 3;
pub const FLAG_ANNOTATIONS: u32 = 1 << 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexRecord {
    pub blob_offset: u64,
    pub blob_len: u32,
    pub flags: u32,
    pub white: u32,
    pub black: u32,
    pub event: u32,
    pub site: u32,
    pub annotator: u32,
    pub date: u32,
    pub round: u16,
    pub subround: u16,
    pub white_elo: u16,
    pub black_elo: u16,
    pub result: u8,
    pub eco: u16,
    pub plies: u16,
}

impl IndexRecord {
    pub fn encode(&self) -> [u8; RECORD_SIZE] {
        let mut b = [0u8; RECORD_SIZE];
        b[0..8].copy_from_slice(&self.blob_offset.to_le_bytes());
        b[8..12].copy_from_slice(&self.blob_len.to_le_bytes());
        b[12..16].copy_from_slice(&self.flags.to_le_bytes());
        b[16..20].copy_from_slice(&self.white.to_le_bytes());
        b[20..24].copy_from_slice(&self.black.to_le_bytes());
        b[24..28].copy_from_slice(&self.event.to_le_bytes());
        b[28..32].copy_from_slice(&self.site.to_le_bytes());
        b[32..36].copy_from_slice(&self.annotator.to_le_bytes());
        b[36..40].copy_from_slice(&self.date.to_le_bytes());
        b[40..42].copy_from_slice(&self.round.to_le_bytes());
        b[42..44].copy_from_slice(&self.subround.to_le_bytes());
        b[44..46].copy_from_slice(&self.white_elo.to_le_bytes());
        b[46..48].copy_from_slice(&self.black_elo.to_le_bytes());
        b[48] = self.result;
        b[50..52].copy_from_slice(&self.eco.to_le_bytes());
        b[52..54].copy_from_slice(&self.plies.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8]) -> IndexRecord {
        IndexRecord {
            blob_offset: u64_at(b, 0),
            blob_len: u32_at(b, 8),
            flags: u32_at(b, 12),
            white: u32_at(b, 16),
            black: u32_at(b, 20),
            event: u32_at(b, 24),
            site: u32_at(b, 28),
            annotator: u32_at(b, 32),
            date: u32_at(b, 36),
            round: u16_at(b, 40),
            subround: u16_at(b, 42),
            white_elo: u16_at(b, 44),
            black_elo: u16_at(b, 46),
            result: b[48],
            eco: u16_at(b, 50),
            plies: u16_at(b, 52),
        }
    }

    pub fn is_deleted(&self) -> bool {
        self.flags & FLAG_DELETED != 0
    }
}

pub fn result_to_u8(r: GameResult) -> u8 {
    match r {
        GameResult::Unknown => 0,
        GameResult::WhiteWins => 1,
        GameResult::BlackWins => 2,
        GameResult::Draw => 3,
    }
}

pub fn result_from_u8(v: u8) -> GameResult {
    match v {
        1 => GameResult::WhiteWins,
        2 => GameResult::BlackWins,
        3 => GameResult::Draw,
        _ => GameResult::Unknown,
    }
}

/// Packs a PGN date (`YYYY.MM.DD`, `??` for unknown parts).
pub fn pack_date(date: &str) -> u32 {
    let mut parts = date.split(['.', '-', '/']);
    let mut next = |max: u32| -> u32 {
        parts.next().and_then(|p| p.trim().parse::<u32>().ok()).filter(|&v| v <= max).unwrap_or(0)
    };
    let year = next(9999);
    let month = next(12);
    let day = next(31);
    (year << 9) | (month << 5) | day
}

pub fn unpack_date(packed: u32) -> String {
    let (y, m, d) = (packed >> 9, (packed >> 5) & 0xf, packed & 0x1f);
    if y == 0 {
        return String::new();
    }
    let part = |v: u32| if v == 0 { "??".to_owned() } else { format!("{v:02}") };
    format!("{y:04}.{}.{}", part(m), part(d))
}

/// Packs `"3"`, `"3.1"`; anything else is unknown (0, 0).
pub fn pack_round(round: &str) -> (u16, u16) {
    let mut parts = round.trim().splitn(2, '.');
    let major = parts.next().and_then(|p| p.parse::<u16>().ok());
    let minor = parts.next().map(|p| p.parse::<u16>().ok());
    match (major, minor) {
        (Some(a), None) => (a, 0),
        (Some(a), Some(Some(b))) => (a, b),
        _ => (0, 0),
    }
}

pub fn unpack_round(round: u16, subround: u16) -> String {
    match (round, subround) {
        (0, _) => String::new(),
        (r, 0) => r.to_string(),
        (r, s) => format!("{r}.{s}"),
    }
}

pub fn pack_eco(eco: &str) -> u16 {
    let b = eco.trim().as_bytes();
    if b.len() != 3 || !(b'A'..=b'E').contains(&b[0]) {
        return 0;
    }
    match (b[1] as char).to_digit(10).zip((b[2] as char).to_digit(10)) {
        Some((t, u)) => u16::from(b[0] - b'A' + 1) * 100 + (t * 10 + u) as u16,
        None => 0,
    }
}

pub fn unpack_eco(eco: u16) -> String {
    if eco == 0 {
        return String::new();
    }
    let letter = (b'A' + (eco / 100) as u8 - 1) as char;
    format!("{letter}{:02}", eco % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trip() {
        let r = IndexRecord {
            blob_offset: 1 << 40,
            blob_len: 1234,
            flags: FLAG_DELETED | FLAG_SETUP,
            white: 1,
            black: 2,
            event: 3,
            site: 4,
            annotator: 5,
            date: pack_date("2024.03.01"),
            round: 3,
            subround: 1,
            white_elo: 2800,
            black_elo: 2700,
            result: 3,
            eco: pack_eco("B90"),
            plies: 99,
        };
        assert_eq!(IndexRecord::decode(&r.encode()), r);
    }

    #[test]
    fn packing_helpers() {
        assert_eq!(unpack_date(pack_date("2024.03.01")), "2024.03.01");
        assert_eq!(unpack_date(pack_date("1999.??.??")), "1999.??.??");
        assert_eq!(unpack_date(pack_date("????.??.??")), "");
        assert!(pack_date("2024.01.02") < pack_date("2024.02.01"));
        assert_eq!(pack_round("3.1"), (3, 1));
        assert_eq!(pack_round("12"), (12, 0));
        assert_eq!(pack_round("-"), (0, 0));
        assert_eq!(unpack_round(3, 1), "3.1");
        assert_eq!(unpack_eco(pack_eco("E99")), "E99");
        assert_eq!(unpack_eco(pack_eco("A00")), "A00");
        assert_eq!(pack_eco("Z12"), 0);
    }
}
