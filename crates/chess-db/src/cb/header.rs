//! `.cbh` — game headers (`docs/io-03-cbh.md` §3.1).
//!
//! All big-endian. A 46-byte file header, then one 46-byte record per game
//! or guiding text in id order (1-based), no gaps.

use std::fs::File;
use std::path::Path;

use chess_core::game::Game;
use chess_core::shakmaty::{Color, Position};

use crate::DbError;
use crate::cb::bytes::{be16, be24, be32, packed_date_to_pgn};
use crate::io::read_exact_at;

pub const RECORD_SIZE: u64 = 46;
pub const HEADER_SIZE: u64 = 46;

const FLAG_GUIDING_TEXT: u8 = 1 << 1;
const FLAG_DELETED: u8 = 1 << 7;

pub struct CbhFile {
    file: File,
    /// Highest game id that has a record (`.cbh`'s own "next id" minus one,
    /// but we trust the file size over the header field).
    pub game_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CbResult {
    BlackWins,
    Draw,
    WhiteWins,
    Line,
    BlackForfeit,
    DrawForfeit,
    WhiteForfeit,
    BothLost,
}

impl CbResult {
    fn from_byte(b: u8) -> Option<CbResult> {
        Some(match b {
            0 => CbResult::BlackWins,
            1 => CbResult::Draw,
            2 => CbResult::WhiteWins,
            3 => CbResult::Line,
            4 => CbResult::BlackForfeit,
            5 => CbResult::DrawForfeit,
            6 => CbResult::WhiteForfeit,
            7 => CbResult::BothLost,
            _ => return None,
        })
    }

    pub fn pgn_str(self) -> &'static str {
        match self {
            CbResult::BlackWins | CbResult::BlackForfeit => "0-1",
            CbResult::Draw | CbResult::DrawForfeit => "1/2-1/2",
            CbResult::WhiteWins | CbResult::WhiteForfeit => "1-0",
            CbResult::Line | CbResult::BothLost => "*",
        }
    }
}

/// A decoded ECO code, or a Chess960 start position (`docs/io-03-cbh.md` §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcoOrChess960 {
    None,
    Eco { letter: u8, number: u8, sub: u8 },
    Chess960StartPos(u16),
}

fn decode_eco(value: u16) -> EcoOrChess960 {
    const CHESS960_BASE: u16 = 64576; // 65536 - 960
    if value == 0 {
        EcoOrChess960::None
    } else if value >= CHESS960_BASE {
        EcoOrChess960::Chess960StartPos(value - CHESS960_BASE)
    } else {
        let eco_no = value / 128 - 1;
        EcoOrChess960::Eco {
            letter: b'A' + (eco_no / 100) as u8,
            number: (eco_no % 100) as u8,
            sub: (value % 128) as u8,
        }
    }
}

impl EcoOrChess960 {
    pub fn pgn_str(self) -> String {
        match self {
            EcoOrChess960::Eco { letter, number, .. } => format!("{}{:02}", letter as char, number),
            _ => String::new(),
        }
    }
}

/// A `.cbh` game record (type/flags bit 1 clear).
#[derive(Debug, Clone)]
pub struct GameRecord {
    pub deleted: bool,
    pub moves_offset: u32,
    pub annotations_offset: u32,
    pub white: u32,
    pub black: u32,
    pub tournament: u32,
    pub annotator: u32,
    pub source: u32,
    pub date_pgn: String,
    pub result: Option<CbResult>,
    pub round: u8,
    pub subround: u8,
    pub white_elo: u16,
    pub black_elo: u16,
    pub eco: EcoOrChess960,
    pub flags: u32,
    /// Full moves (not plies) in the main line, 0 for none. A longer game
    /// stores 255 or the count modulo 256, depending on the writer.
    pub main_line_moves: u8,
}

impl GameRecord {
    /// Whether `game`'s main line is as long as this record declares: the
    /// check that tells a correct decode from a lucky one.
    pub fn main_line_matches(&self, game: &Game) -> bool {
        let plies = game.mainline_len();
        let black_first = game.start_position().turn() == Color::Black;
        let moves = if plies == 0 { 0 } else { (plies + usize::from(black_first)).div_ceil(2) };
        let declared = usize::from(self.main_line_moves);
        moves == declared || moves % 256 == declared || (declared == 255 && moves >= 255)
    }
}

/// A `.cbh` guiding-text record (type/flags bit 1 set).
#[derive(Debug, Clone, Copy)]
pub struct GuidingTextRecord {
    pub text_offset: u32,
    pub tournament: u32,
    pub source: u32,
    pub annotator: u32,
    pub round: u8,
    pub subround: u8,
    pub flags: u32,
}

pub enum CbEntry {
    Game(GameRecord),
    Text(GuidingTextRecord),
    /// Past the last record, or an all-zero record.
    Empty,
}

impl CbhFile {
    pub fn open(path: &Path) -> Result<CbhFile, DbError> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < HEADER_SIZE {
            return Err(DbError::NotADatabase(path.to_owned()));
        }
        let mut header = [0u8; HEADER_SIZE as usize];
        read_exact_at(&file, &mut header, 0)?;
        let record_size = be16(&header, 0x03);
        let version = header[0x05];
        if record_size as u64 != RECORD_SIZE || version != 1 {
            return Err(DbError::UnsupportedVersion(version as u32));
        }
        let game_count = ((len - HEADER_SIZE) / RECORD_SIZE) as u32;
        Ok(CbhFile { file, game_count })
    }

    /// Reads record `id` (1-based). `id` 0 or past `game_count` is an error.
    pub fn read(&self, id: u32) -> Result<CbEntry, DbError> {
        if id == 0 || id > self.game_count {
            return Err(DbError::NotFound(id));
        }
        let mut buf = [0u8; RECORD_SIZE as usize];
        let offset = HEADER_SIZE + (id as u64 - 1) * RECORD_SIZE;
        read_exact_at(&self.file, &mut buf, offset)?;
        Ok(parse_record(&buf))
    }
}

fn parse_record(b: &[u8; RECORD_SIZE as usize]) -> CbEntry {
    let type_flags = b[0];
    if type_flags == 0 {
        return CbEntry::Empty;
    }
    if type_flags & FLAG_GUIDING_TEXT != 0 {
        CbEntry::Text(GuidingTextRecord {
            text_offset: be32(b, 0x01),
            tournament: be24(b, 0x07),
            source: be24(b, 0x0a),
            annotator: be24(b, 0x0d),
            round: b[0x10],
            subround: b[0x11],
            flags: be32(b, 0x12),
        })
    } else {
        let date_packed = be24(b, 0x18);
        CbEntry::Game(GameRecord {
            deleted: type_flags & FLAG_DELETED != 0,
            moves_offset: be32(b, 0x01),
            annotations_offset: be32(b, 0x05),
            white: be24(b, 0x09),
            black: be24(b, 0x0c),
            tournament: be24(b, 0x0f),
            annotator: be24(b, 0x12),
            source: be24(b, 0x15),
            date_pgn: packed_date_to_pgn(date_packed),
            result: CbResult::from_byte(b[0x1b]),
            round: b[0x1d],
            subround: b[0x1e],
            white_elo: be16(b, 0x1f),
            black_elo: be16(b, 0x21),
            eco: decode_eco(be16(b, 0x23)),
            flags: be32(b, 0x27),
            main_line_moves: b[0x2d],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::pgn::read_pgn_str;

    fn record(fields: &[(usize, &[u8])]) -> [u8; RECORD_SIZE as usize] {
        let mut b = [0u8; RECORD_SIZE as usize];
        b[0] = 0x01;
        for &(off, bytes) in fields {
            b[off..off + bytes.len()].copy_from_slice(bytes);
        }
        b
    }

    #[test]
    fn parses_a_real_byte_for_byte_example() {
        // From the real Mega Database 2019.cbh, record 3 (the first actual game,
        // after two guiding texts), bytes verified by hand with `od`.
        let b = record(&[
            (0x01, &[0x00, 0x03, 0x32, 0x46]), // moves offset
            (0x1b, &[0x02]),                   // result: white wins
        ]);
        match parse_record(&b) {
            CbEntry::Game(g) => {
                assert_eq!(g.moves_offset, 0x0003_3246);
                assert_eq!(g.result, Some(CbResult::WhiteWins));
                assert_eq!(g.result.unwrap().pgn_str(), "1-0");
            }
            _ => panic!("expected a game record"),
        }
    }

    #[test]
    fn empty_record_is_past_the_end() {
        assert!(matches!(parse_record(&[0u8; RECORD_SIZE as usize]), CbEntry::Empty));
    }

    #[test]
    fn guiding_text_bit_selects_the_other_shape() {
        let b = record(&[(0x00, &[0x03]), (0x01, &[0x00, 0x00, 0x00, 26])]);
        match parse_record(&b) {
            CbEntry::Text(t) => assert_eq!(t.text_offset, 26),
            _ => panic!("expected a guiding text"),
        }
    }

    #[test]
    fn main_line_length_counts_full_moves() {
        let game_record = |moves: u8| match parse_record(&record(&[(0x2d, &[moves])])) {
            CbEntry::Game(g) => g,
            _ => panic!("expected a game record"),
        };
        let game = read_pgn_str("1. e4 e5 2. Nf3 *").remove(0);
        assert!(game_record(2).main_line_matches(&game));
        assert!(!game_record(3).main_line_matches(&game));
        let black_first =
            read_pgn_str("[FEN \"4k3/8/8/8/8/8/8/4K3 b - - 0 1\"]\n\n1... Kd8 2. Kd2 *").remove(0);
        assert!(game_record(2).main_line_matches(&black_first));
        assert!(game_record(0).main_line_matches(&Game::new(Default::default(), None)));
    }

    #[test]
    fn eco_decode() {
        assert_eq!(decode_eco(0), EcoOrChess960::None);
        // B00: eco_no=100 -> letter B, number 0; value = (100+1)*128 + sub
        let b00 = decode_eco(101 * 128);
        assert_eq!(b00.pgn_str(), "B00");
        assert_eq!(decode_eco(64576 + 5), EcoOrChess960::Chess960StartPos(5));
    }
}
