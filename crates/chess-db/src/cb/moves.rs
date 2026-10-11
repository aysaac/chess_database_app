//! `.cbg` — moves (`docs/io-03-cbh.md` §3.2), mode-0 ("compact"/"pre"/
//! default-order) decoding only. Any other encoding mode (including
//! Chess960, modes 10/11) is reported as unsupported rather than guessed.

use std::fs::File;
use std::path::Path;

use chess_core::game::{Game, NodeId};
use chess_core::position::{find_move, from_fen};
use chess_core::shakmaty::{Chess, Color, File as ChessFile, Move, Piece, Position, Rank, Role, Square};

use crate::DbError;
use crate::cb::bytes::{be16, be24};
use crate::io::read_exact_at;

pub const SUPPORTED_MODE: u8 = 0;

/// Encoding mode 0's translation table (`docs/io-03-cbh.md` §3.2), read
/// from the user's own licensed ChessBase 16 (`CBase16.exe`, at 0x1416480
/// in that copy) by `examples/extract_cb_key.rs`. It decodes every
/// mode-0 game of Mega Database 2019 and of the four databases that ship
/// with ChessBase 16 (Testsets and Training) to the main-line length their
/// `.cbh` declares.
const MODE0_TABLE: [u8; 256] = [
    162, 149, 67, 245, 193, 61, 74, 108, 83, 131, 204, 124, 255, 174, 104, 173, //
    209, 146, 139, 141, 53, 129, 94, 116, 38, 142, 171, 202, 253, 154, 243, 160, //
    165, 21, 252, 177, 30, 237, 48, 234, 34, 235, 167, 205, 78, 111, 46, 36, //
    50, 148, 65, 140, 110, 88, 130, 80, 187, 2, 138, 216, 250, 96, 222, 82, //
    186, 70, 172, 41, 157, 215, 223, 8, 33, 1, 102, 163, 241, 25, 39, 181, //
    145, 213, 66, 14, 180, 76, 217, 24, 95, 188, 37, 166, 150, 4, 86, 106, //
    170, 51, 28, 43, 115, 240, 221, 164, 55, 211, 197, 16, 191, 90, 35, 52, //
    117, 91, 184, 85, 210, 107, 9, 58, 87, 18, 179, 119, 72, 133, 155, 15, //
    158, 199, 200, 161, 127, 122, 192, 189, 49, 109, 246, 62, 195, 17, 113, 206, //
    125, 218, 168, 84, 144, 151, 31, 68, 64, 22, 201, 227, 44, 203, 132, 236, //
    159, 63, 92, 230, 118, 11, 60, 32, 183, 54, 0, 220, 231, 249, 79, 247, //
    175, 6, 7, 224, 26, 10, 169, 75, 12, 214, 99, 135, 137, 29, 19, 27, //
    228, 112, 5, 71, 103, 123, 47, 238, 226, 232, 152, 13, 239, 207, 196, 244, //
    251, 176, 23, 153, 100, 242, 212, 42, 3, 77, 120, 198, 254, 101, 134, 136, //
    121, 69, 59, 229, 73, 143, 45, 185, 190, 98, 147, 20, 233, 208, 56, 156, //
    178, 194, 89, 93, 182, 114, 81, 248, 40, 126, 97, 57, 225, 219, 105, 128, //
];

/// The 256-byte permutation table used to translate move bytes
/// (`docs/io-03-cbh.md` §3.2).
#[derive(Clone)]
pub struct TranslateTable(pub [u8; 256]);

impl TranslateTable {
    /// The table ChessBase uses for encoding mode 0, the only mode we read.
    pub fn mode0() -> TranslateTable {
        TranslateTable(MODE0_TABLE)
    }

    pub fn identity() -> TranslateTable {
        let mut t = [0u8; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = i as u8;
        }
        TranslateTable(t)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<TranslateTable, DbError> {
        let arr: [u8; 256] =
            bytes.try_into().map_err(|_| DbError::Invalid("translation table must be 256 bytes".into()))?;
        Ok(TranslateTable(arr))
    }

    /// "pre" modifier: subtract `n` from the raw byte before the lookup.
    fn pre(&self, byte: u8, n: usize) -> u8 {
        self.0[usize::from(byte.wrapping_sub(n as u8))]
    }

    /// The raw byte that `pre` turns into `opcode` after `n` moves: the
    /// inverse, for building test streams.
    #[cfg(test)]
    pub(crate) fn encode(&self, opcode: u8, n: usize) -> u8 {
        let index = self.0.iter().position(|&v| v == opcode).expect("the table is a permutation");
        (index as u8).wrapping_add(n as u8)
    }
}

pub struct CbgFile {
    file: File,
    header_size: u64,
}

impl CbgFile {
    pub fn open(path: &Path) -> Result<CbgFile, DbError> {
        let file = File::open(path)?;
        let mut hdr = [0u8; 2];
        read_exact_at(&file, &mut hdr, 0)?;
        let header_size = be16(&hdr, 0) as u64;
        Ok(CbgFile { file, header_size })
    }

    #[allow(dead_code)]
    pub fn header_size(&self) -> u64 {
        self.header_size
    }

    /// Reads the record at `offset` (a game's or text's offset from `.cbh`).
    pub fn read_record(&self, offset: u32) -> Result<Vec<u8>, DbError> {
        let mut prefix = [0u8; 4];
        read_exact_at(&self.file, &mut prefix, offset as u64)?;
        let size = be24(&prefix, 1) as usize;
        if size < 4 {
            return Err(DbError::Corrupt(format!(".cbg record at {offset} has size {size} < 4")));
        }
        let mut buf = vec![0u8; size];
        read_exact_at(&self.file, &mut buf, offset as u64)?;
        Ok(buf)
    }
}

fn ci(c: Color) -> usize {
    if c == Color::White { 0 } else { 1 }
}

#[derive(Clone)]
struct Slots {
    king: [Square; 2],
    queen: [Vec<Square>; 2],
    rook: [Vec<Square>; 2],
    bishop: [Vec<Square>; 2],
    knight: [Vec<Square>; 2],
    /// Indexed by original scan order (the pawn's original file, for a
    /// standard setup), never renumbered: a captured or promoted pawn's
    /// slot goes empty and is never reassigned. It must not keep its last
    /// square, or a pawn that later stands there would be found in the
    /// dead pawn's slot.
    pawn: [Vec<Option<Square>>; 2],
}

impl Slots {
    fn from_position(pos: &Chess) -> Slots {
        let mut s = Slots {
            king: [Square::A1, Square::A1],
            queen: [Vec::new(), Vec::new()],
            rook: [Vec::new(), Vec::new()],
            bishop: [Vec::new(), Vec::new()],
            knight: [Vec::new(), Vec::new()],
            pawn: [Vec::new(), Vec::new()],
        };
        // Board-scan order a1,a2,...a8,b1,...h8 (file outer, rank inner) —
        // this is the order a piece's slot number is assigned in.
        for file in 0..8u32 {
            for rank in 0..8u32 {
                let sq = Square::from_coords(ChessFile::new(file), Rank::new(rank));
                if let Some(piece) = pos.board().piece_at(sq) {
                    let c = ci(piece.color);
                    match piece.role {
                        Role::King => s.king[c] = sq,
                        Role::Pawn => s.pawn[c].push(Some(sq)),
                        role => s.pieces_mut(piece.color, role).push(sq),
                    }
                }
            }
        }
        s
    }

    /// The slots of a queen, rook, bishop or knight kind.
    fn pieces(&self, color: Color, role: Role) -> &Vec<Square> {
        let c = ci(color);
        match role {
            Role::Queen => &self.queen[c],
            Role::Rook => &self.rook[c],
            Role::Bishop => &self.bishop[c],
            Role::Knight => &self.knight[c],
            Role::King | Role::Pawn => unreachable!("kings and pawns have their own slots"),
        }
    }

    fn pieces_mut(&mut self, color: Color, role: Role) -> &mut Vec<Square> {
        let c = ci(color);
        match role {
            Role::Queen => &mut self.queen[c],
            Role::Rook => &mut self.rook[c],
            Role::Bishop => &mut self.bishop[c],
            Role::Knight => &mut self.knight[c],
            Role::King | Role::Pawn => unreachable!("kings and pawns have their own slots"),
        }
    }

    fn square_of(&self, color: Color, role: Role, piece_no: usize) -> Option<Square> {
        match role {
            Role::King => (piece_no == 0).then_some(self.king[ci(color)]),
            Role::Pawn => self.pawn[ci(color)].get(piece_no).copied().flatten(),
            role => self.pieces(color, role).get(piece_no).copied(),
        }
    }

    /// Empties the slot of `color`'s piece on `sq` (it was captured). A
    /// captured queen, rook, bishop or knight renumbers the ones after it.
    fn remove(&mut self, color: Color, role: Role, sq: Square) {
        match role {
            Role::King => {}
            Role::Pawn => {
                if let Some(slot) = self.pawn[ci(color)].iter_mut().find(|slot| **slot == Some(sq)) {
                    *slot = None;
                }
            }
            role => {
                let v = self.pieces_mut(color, role);
                if let Some(idx) = v.iter().position(|&s| s == sq) {
                    v.remove(idx);
                }
            }
        }
    }

    /// Moves `color`'s piece from `from` to `to` (`None`: it left the board
    /// by promoting).
    fn relocate(&mut self, color: Color, role: Role, from: Square, to: Option<Square>) {
        match role {
            Role::King => self.king[ci(color)] = to.unwrap_or(from),
            Role::Pawn => {
                if let Some(slot) = self.pawn[ci(color)].iter_mut().find(|slot| **slot == Some(from)) {
                    *slot = to;
                }
            }
            role => {
                let v = self.pieces_mut(color, role);
                if let (Some(slot), Some(to)) = (v.iter_mut().find(|s| **s == from), to) {
                    *slot = to;
                }
            }
        }
    }

    /// Applies `mv` (already played on the real position) to the slots.
    fn apply_move(&mut self, mover: Color, mv: chess_core::shakmaty::Move) {
        use chess_core::shakmaty::Move;
        match mv {
            Move::Castle { king, rook } => {
                self.king[ci(mover)] =
                    chess_core::position::castle_king_target(mv).expect("castle move has a king target");
                let file = if rook.file() > king.file() { ChessFile::F } else { ChessFile::D };
                self.relocate(mover, Role::Rook, rook, Some(Square::from_coords(file, rook.rank())));
            }
            Move::Normal { role, from, capture, to, promotion } => {
                if let Some(captured) = capture {
                    self.remove(mover.other(), captured, to);
                }
                self.relocate(mover, role, from, if promotion.is_some() { None } else { Some(to) });
                if let Some(promoted) = promotion {
                    self.pieces_mut(mover, promoted).push(to);
                }
            }
            Move::EnPassant { from, to } => {
                self.remove(mover.other(), Role::Pawn, Square::from_coords(to.file(), from.rank()));
                self.relocate(mover, Role::Pawn, from, Some(to));
            }
            Move::Put { .. } => {}
        }
    }
}

const OP_NULL: u8 = 0;
const OP_TWO_BYTES: u8 = 235;
const OP_IGNORE: u8 = 236;
const OP_START_VAR: u8 = 254;
const OP_END_VAR: u8 = 255;

/// One (Δfile, Δrank) per king/knight direction index, clockwise from
/// north — `docs/io-03-cbh.md` §3.2, derived from the real decoder's
/// direction tables (see the research notes, not reproduced verbatim).
const KING_DIR: [(i32, i32); 8] = [(0, 1), (1, 1), (1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1)];
const KNIGHT_DIR: [(i32, i32); 8] = [(2, 1), (1, 2), (-1, 2), (-2, 1), (-2, -1), (-1, -2), (1, -2), (2, -1)];

enum OpcodeMove {
    Null,
    Ignore,
    Unknown,
    StartVariant,
    EndVariant,
    TwoBytes,
    Single { role: Role, piece_no: usize, ofs: u8 },
}

fn classify(opcode: u8) -> OpcodeMove {
    match opcode {
        OP_NULL => OpcodeMove::Null,
        1..=10 => OpcodeMove::Single { role: Role::King, piece_no: 0, ofs: opcode - 1 },
        11..=38 => OpcodeMove::Single { role: Role::Queen, piece_no: 0, ofs: opcode - 11 },
        39..=52 => OpcodeMove::Single { role: Role::Rook, piece_no: 0, ofs: opcode - 39 },
        53..=66 => OpcodeMove::Single { role: Role::Rook, piece_no: 1, ofs: opcode - 53 },
        67..=80 => OpcodeMove::Single { role: Role::Bishop, piece_no: 0, ofs: opcode - 67 },
        81..=94 => OpcodeMove::Single { role: Role::Bishop, piece_no: 1, ofs: opcode - 81 },
        95..=102 => OpcodeMove::Single { role: Role::Knight, piece_no: 0, ofs: opcode - 95 },
        103..=110 => OpcodeMove::Single { role: Role::Knight, piece_no: 1, ofs: opcode - 103 },
        111..=142 => {
            let i = opcode - 111;
            OpcodeMove::Single { role: Role::Pawn, piece_no: (i / 4) as usize, ofs: i % 4 }
        }
        143..=170 => OpcodeMove::Single { role: Role::Queen, piece_no: 1, ofs: opcode - 143 },
        171..=198 => OpcodeMove::Single { role: Role::Queen, piece_no: 2, ofs: opcode - 171 },
        199..=212 => OpcodeMove::Single { role: Role::Rook, piece_no: 2, ofs: opcode - 199 },
        213..=226 => OpcodeMove::Single { role: Role::Bishop, piece_no: 2, ofs: opcode - 213 },
        227..=234 => OpcodeMove::Single { role: Role::Knight, piece_no: 2, ofs: opcode - 227 },
        OP_TWO_BYTES => OpcodeMove::TwoBytes,
        OP_IGNORE => OpcodeMove::Ignore,
        OP_START_VAR => OpcodeMove::StartVariant,
        OP_END_VAR => OpcodeMove::EndVariant,
        _ => OpcodeMove::Unknown,
    }
}

fn step(from: Square, d: (i32, i32)) -> Option<Square> {
    let file = from.file().to_u32() as i32 + d.0;
    let rank = from.rank().to_u32() as i32 + d.1;
    if !(0..8).contains(&file) || !(0..8).contains(&rank) {
        return None;
    }
    Some(Square::from_coords(ChessFile::new(file as u32), Rank::new(rank as u32)))
}

fn wrap8(v: i32) -> u32 {
    v.rem_euclid(8) as u32
}

fn slide_target(from: Square, ofs: u8, bishop: bool) -> Square {
    let dir = (ofs / 7) + if bishop { 2 } else { 0 };
    let stride = i32::from(ofs % 7) + 1;
    let px = from.file().to_u32() as i32;
    let py = from.rank().to_u32() as i32;
    let (nf, nr) = match dir {
        0 => (px, py + stride),
        1 => (px + stride, py),
        2 => (px + stride, py + stride),
        3 => (px + stride, py - stride),
        _ => unreachable!("dir is 0..=3"),
    };
    Square::from_coords(ChessFile::new(wrap8(nf)), Rank::new(wrap8(nr)))
}

/// Where a one-byte move code takes its piece, or `None` when the code
/// names a piece that isn't there or a step off the board.
fn single_byte_target(
    pos: &Chess,
    slots: &Slots,
    role: Role,
    piece_no: usize,
    ofs: u8,
) -> Option<(Square, Square)> {
    let mover = pos.turn();
    let from = slots.square_of(mover, role, piece_no)?;
    let to = match role {
        Role::King => match ofs {
            8 => Square::from_coords(ChessFile::G, from.rank()),
            9 => Square::from_coords(ChessFile::C, from.rank()),
            _ => step(from, KING_DIR[ofs as usize])?,
        },
        Role::Knight => step(from, KNIGHT_DIR[ofs as usize])?,
        Role::Rook => slide_target(from, ofs, false),
        Role::Bishop => slide_target(from, ofs, true),
        Role::Queen => slide_target(from, ofs, false),
        Role::Pawn => {
            let sign: i32 = if mover == Color::White { 1 } else { -1 };
            let d = match ofs {
                0 => (0, sign),
                1 => (0, 2 * sign),
                2 => (sign, sign),
                3 => (-sign, sign),
                _ => unreachable!("pawn ofs is 0..=3"),
            };
            step(from, d)?
        }
    };
    Some((from, to))
}

pub(crate) fn sqi_to_square(sqi: u16) -> Result<Square, DbError> {
    if sqi >= 64 {
        return Err(DbError::Corrupt(format!("square index {sqi} out of range")));
    }
    let file = (sqi / 8) as u32;
    let rank = (sqi % 8) as u32;
    Ok(Square::from_coords(ChessFile::new(file), Rank::new(rank)))
}

fn two_byte_target(pos: &Chess, value: u16) -> (Square, Square, Option<Role>) {
    let square = |sqi: u16| sqi_to_square(sqi & 0x3f).expect("six bits are a square");
    let from = square(value);
    let to = square(value >> 6);
    let promo_code = (value >> 12) & 0x3;
    let is_pawn = matches!(pos.board().piece_at(from), Some(Piece { role: Role::Pawn, .. }));
    let promotion = if is_pawn && (to.rank() == Rank::First || to.rank() == Rank::Eighth) {
        Some(match promo_code {
            0 => Role::Queen,
            1 => Role::Rook,
            2 => Role::Bishop,
            _ => Role::Knight,
        })
    } else {
        None
    };
    (from, to, promotion)
}

/// A move in the stream, translated but not yet played.
enum Token {
    Null,
    Single { role: Role, piece_no: usize, ofs: u8 },
    TwoBytes(u16),
}

/// Why a move in the stream can't be played.
enum Unreadable {
    Null,
    Illegal(Square, Square),
    Undecodable,
}

fn token_move(pos: &Chess, slots: &Slots, token: Token) -> Result<Move, Unreadable> {
    let (from, to, promotion) = match token {
        Token::Null => return Err(Unreadable::Null),
        Token::Single { role, piece_no, ofs } => {
            let (from, to) =
                single_byte_target(pos, slots, role, piece_no, ofs).ok_or(Unreadable::Undecodable)?;
            (from, to, None)
        }
        Token::TwoBytes(value) => two_byte_target(pos, value),
    };
    find_move(pos, from, to, promotion).ok_or(Unreadable::Illegal(from, to))
}

/// The import report's line for a line cut short at an unreadable move,
/// worded like PGN import's.
fn cut_message(pos: &Chess, main_line: bool, why: Unreadable) -> String {
    let line = if main_line { "The main line" } else { "A variation" };
    let number = pos.fullmoves().get();
    let dots = if pos.turn() == Color::White { "." } else { "..." };
    match why {
        Unreadable::Null => {
            format!("{line} stops at {number}{dots}-- (a null move), which can't be imported yet.")
        }
        Unreadable::Illegal(from, to) => {
            format!("{line} stops at {number}{dots}{from}-{to}, which isn't a legal move there.")
        }
        Unreadable::Undecodable => format!("{line} stops at move {number}{dots}, which can't be decoded."),
    }
}

const ENDS_ABRUPTLY: &str = "The moves end abruptly, so the rest of the game is missing.";

/// A game's moves as read from `.cbg`.
pub struct DecodedGame {
    pub game: Game,
    /// The node of each move in `.cbg` storage order, the order `.cba`
    /// annotation positions count in; `None` for a move in a line that
    /// was cut short.
    pub order: Vec<Option<NodeId>>,
    /// Why lines were cut short, one message per cut, for the import
    /// report.
    pub problems: Vec<String>,
}

/// Decodes a `.cbg` game record's moves (mode 0 only) into a fresh `Game`
/// starting at `start`. The record's 4-byte flags+size prefix (and, when
/// present, the 28-byte setup-position block) must already be stripped by
/// the caller — this function reads only the move stream.
///
/// A move that can't be played (a null move, which our game tree can't
/// hold, or one that isn't legal) cuts its line there, as PGN import does:
/// the rest of that line and its sub-variations are skipped, and the next
/// line is read as usual. Skipping still counts every move, so the bytes
/// after it translate correctly.
pub fn decode_mode0(
    move_bytes: &[u8],
    start: Chess,
    setup_tag: Option<Chess>,
    table: &TranslateTable,
) -> DecodedGame {
    let mut game = Game::new(Default::default(), setup_tag);
    let mut order: Vec<Option<NodeId>> = Vec::new();
    let mut problems = Vec::new();
    let mut bytes = move_bytes.iter().copied();
    let mut pos = start;
    let mut slots = Slots::from_position(&pos);
    let mut current = NodeId::ROOT;
    let mut stack: Vec<(NodeId, Chess, Slots)> = Vec::new();
    let mut main_line = true;
    // While a line is cut short: the branch points opened inside it since,
    // still to be closed.
    let mut cut: Option<usize> = None;

    loop {
        // Every move counts towards the modifier, cut or not; markers don't.
        let n = order.len();
        let Some(raw) = bytes.next() else {
            problems.push(ENDS_ABRUPTLY.to_owned());
            break;
        };
        let token = match classify(table.pre(raw, n)) {
            OpcodeMove::Ignore | OpcodeMove::Unknown => continue,
            OpcodeMove::StartVariant => {
                match cut.as_mut() {
                    Some(open) => *open += 1,
                    None => stack.push((current, pos.clone(), slots.clone())),
                }
                continue;
            }
            OpcodeMove::EndVariant => {
                if let Some(open) = cut.as_mut()
                    && *open > 0
                {
                    *open -= 1;
                    continue;
                }
                cut = None;
                let Some(frame) = stack.pop() else { break };
                (current, pos, slots) = frame;
                // Only the first line from a branch point continues the
                // main line, and it comes first.
                main_line = false;
                continue;
            }
            OpcodeMove::Null => Token::Null,
            OpcodeMove::TwoBytes => {
                let (Some(hi), Some(lo)) = (bytes.next(), bytes.next()) else {
                    problems.push(ENDS_ABRUPTLY.to_owned());
                    break;
                };
                Token::TwoBytes(u16::from_be_bytes([table.pre(hi, n), table.pre(lo, n)]))
            }
            OpcodeMove::Single { role, piece_no, ofs } => Token::Single { role, piece_no, ofs },
        };
        if cut.is_some() {
            order.push(None);
            continue;
        }
        match token_move(&pos, &slots, token) {
            Ok(mv) => {
                let mover = pos.turn();
                current = game.add_child(current, mv);
                order.push(Some(current));
                slots.apply_move(mover, mv);
                pos.play_unchecked(mv);
            }
            Err(why) => {
                problems.push(cut_message(&pos, main_line, why));
                order.push(None);
                cut = Some(0);
            }
        }
    }

    DecodedGame { game, order, problems }
}

/// Strips a `.cbg` game record's 4-byte flags+size prefix (and, when
/// present, the setup-position block) and decodes its moves. `record` is
/// the full record as read by `CbgFile::read_record`. An error means the
/// game can't be read at all: an encoding mode we don't read, or a bad
/// start position.
pub fn decode_record(record: &[u8], table: &TranslateTable) -> Result<DecodedGame, DbError> {
    if record.len() < 4 {
        return Err(DbError::Corrupt("`.cbg` record shorter than its header".into()));
    }
    let flags = record[0];
    let mode = flags & 0x3f;
    match mode {
        SUPPORTED_MODE => {}
        10 | 11 => return Err(DbError::Invalid("Chess960 games can't be imported yet.".into())),
        _ => {
            return Err(DbError::Invalid(format!(
                "The game is stored in ChessBase's encoding mode {mode}, which can't be imported yet."
            )));
        }
    }
    let mut offset = 4;
    let setup_tag = if flags & 0x40 != 0 {
        let block = record
            .get(offset..offset + 28)
            .ok_or_else(|| DbError::Invalid("The game's start position is cut off.".into()))?;
        offset += 28;
        Some(parse_setup(block)?)
    } else {
        None
    };
    let start = setup_tag.clone().unwrap_or_default();
    Ok(decode_mode0(&record[offset..], start, setup_tag, table))
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn bit(&mut self) -> Result<u8, DbError> {
        let byte = self
            .data
            .get(self.pos / 8)
            .ok_or_else(|| DbError::Invalid("The game's start position is cut off.".into()))?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(bit)
    }

    fn bits3(&mut self) -> Result<u8, DbError> {
        Ok((self.bit()? << 2) | (self.bit()? << 1) | self.bit()?)
    }
}

/// Parses the 28-byte custom start position (`docs/io-03-cbh.md` §3.2) into
/// a `Chess` by building a FEN and reusing `chess_core::position::from_fen`.
fn parse_setup(data: &[u8]) -> Result<Chess, DbError> {
    let ep_nibble = data[1] & 0x0f;
    let black_to_move = data[1] & 0x10 != 0;
    let castle_bits = data[2];
    let move_number = if data[3] == 0 { 1 } else { data[3] };

    let mut board = [[None; 8]; 8]; // [file][rank]
    let mut bits = BitReader { data: &data[4..28], pos: 0 };
    for file in board.iter_mut() {
        for square in file.iter_mut() {
            if bits.bit()? == 0 {
                continue;
            }
            let color = if bits.bit()? == 0 { Color::White } else { Color::Black };
            let role = match bits.bits3()? {
                1 => Role::King,
                2 => Role::Queen,
                3 => Role::Knight,
                4 => Role::Bishop,
                5 => Role::Rook,
                6 => Role::Pawn,
                other => {
                    return Err(DbError::Invalid(format!(
                        "The game's start position has an unknown piece code {other}."
                    )));
                }
            };
            *square = Some((color, role));
        }
    }

    let mut ranks = Vec::with_capacity(8);
    for rank in (0..8).rev() {
        let mut row = String::new();
        let mut empty = 0u32;
        for file in board.iter() {
            match file[rank] {
                None => empty += 1,
                Some((color, role)) => {
                    if empty > 0 {
                        row.push_str(&empty.to_string());
                        empty = 0;
                    }
                    let c = match role {
                        Role::King => 'K',
                        Role::Queen => 'Q',
                        Role::Rook => 'R',
                        Role::Bishop => 'B',
                        Role::Knight => 'N',
                        Role::Pawn => 'P',
                    };
                    row.push(if color == Color::White { c } else { c.to_ascii_lowercase() });
                }
            }
        }
        if empty > 0 {
            row.push_str(&empty.to_string());
        }
        ranks.push(row);
    }

    // ChessBase often stores every right (0x0f) whatever the board, so a
    // right only counts while its king and rook are on their home squares.
    let mut castling = String::new();
    for (bit, letter, color, rank, rook_file) in [
        (0b0010, 'K', Color::White, 0, 7),
        (0b0001, 'Q', Color::White, 0, 0),
        (0b1000, 'k', Color::Black, 7, 7),
        (0b0100, 'q', Color::Black, 7, 0),
    ] {
        if castle_bits & bit != 0
            && board[4][rank] == Some((color, Role::King))
            && board[rook_file][rank] == Some((color, Role::Rook))
        {
            castling.push(letter);
        }
    }
    if castling.is_empty() {
        castling.push('-');
    }

    let ep = if ep_nibble == 0 {
        "-".to_owned()
    } else {
        // The square the pawn that just moved two squares passed over.
        let file = (b'a' + ep_nibble - 1) as char;
        let rank = if black_to_move { 3 } else { 6 };
        format!("{file}{rank}")
    };

    let fen_with = |ep: &str| {
        format!(
            "{} {} {castling} {ep} 0 {move_number}",
            ranks.join("/"),
            if black_to_move { "b" } else { "w" }
        )
    };
    let fen = fen_with(&ep);
    // A few games name an en passant file no pawn can have just moved on:
    // the position stands without it.
    from_fen(&fen)
        .or_else(|e| if ep == "-" { Err(e) } else { from_fen(&fen_with("-")) })
        .map_err(|e| DbError::Invalid(format!("The game's start position isn't valid ({fen}): {e}.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a raw `.cbg` move stream from a list of post-translate
    /// opcodes (what `classify` sees): control codes (236/254/255) take the
    /// running modifier but don't advance it; everything else is a one-byte
    /// move opcode, or `(235, hi, lo)` for a two-byte move — every byte of
    /// which gets the *same* running modifier, which only then advances by
    /// one. This mirrors `decode_mode0`, it's not an independent
    /// re-derivation.
    enum Op {
        Move(u8),
        TwoByte(u8, u8),
        Control(u8),
    }

    fn encode_with(table: &TranslateTable, ops: &[Op]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut n = 0;
        for op in ops {
            match *op {
                Op::Control(c) => out.push(table.encode(c, n)),
                Op::Move(v) => {
                    out.push(table.encode(v, n));
                    n += 1;
                }
                Op::TwoByte(hi, lo) => {
                    out.extend([OP_TWO_BYTES, hi, lo].map(|b| table.encode(b, n)));
                    n += 1;
                }
            }
        }
        out
    }

    fn encode(ops: &[Op]) -> Vec<u8> {
        encode_with(&TranslateTable::identity(), ops)
    }

    fn decode(bytes: &[u8], start: Chess) -> DecodedGame {
        decode_mode0(bytes, start.clone(), Some(start), &TranslateTable::identity())
    }

    fn mainline_sans(game: &Game) -> Vec<String> {
        let mut pos = game.start_position();
        let mut out = Vec::new();
        let mut cur = NodeId::ROOT;
        while let Some(&next) = game.node(cur).children.first() {
            let mv = game.node(next).mv.expect("a move node");
            out.push(chess_core::notation::san(&pos, mv));
            pos.play_unchecked(mv);
            cur = next;
        }
        out
    }

    #[test]
    fn decodes_e4_e5_nf3() {
        // e-pawn is piece_no 4 (a,b,c,d,e -> 0,1,2,3,4): opcode = 111+4*4+1 = 128.
        let bytes = encode(&[
            Op::Move(128),     // e2-e4 (two steps)
            Op::Move(128),     // e7-e5 (two steps)
            Op::Move(103 + 2), // Ng1-f3: knight #1, Δ(file,rank)=(-1,2) = KNIGHT_DIR[2]
            Op::Control(OP_END_VAR),
        ]);
        let decoded = decode(&bytes, Chess::default());
        assert_eq!(mainline_sans(&decoded.game), ["e4", "e5", "Nf3"]);
        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.order.len(), 3);
    }

    #[test]
    fn the_mode0_table_reads_real_streams() {
        let table = TranslateTable::mode0();
        let mut seen = [false; 256];
        for &v in &table.0 {
            seen[usize::from(v)] = true;
        }
        assert!(seen.iter().all(|&s| s), "the table is a permutation");
        // Facts from Mega Database 2019 that pin the table down: a game's
        // last byte is 12 after its last move (T[12] = 255), and the most
        // common first bytes, 1.e4, 1.d4 and 1.Nf3, are 255, 11 and 254.
        assert_eq!(table.0[12], OP_END_VAR);
        assert_eq!(table.encode(128, 0), 255);
        assert_eq!(table.encode(124, 0), 11);
        assert_eq!(table.encode(105, 0), 254);
        let bytes =
            encode_with(&table, &[Op::Move(128), Op::Move(128), Op::Move(103 + 2), Op::Control(OP_END_VAR)]);
        let decoded = decode_mode0(&bytes, Chess::default(), None, &table);
        assert_eq!(mainline_sans(&decoded.game), ["e4", "e5", "Nf3"]);
    }

    #[test]
    fn decodes_castling_short() {
        let pos = from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
        let bytes = encode(&[Op::Move(9), Op::Control(OP_END_VAR)]); // king ofs 8 -> opcode 1+8=9
        let game = decode(&bytes, pos).game;
        assert_eq!(game.mainline_len(), 1);
        let mv = game.node(game.preorder()[1]).mv.unwrap();
        assert!(matches!(mv, chess_core::shakmaty::Move::Castle { .. }));
    }

    #[test]
    fn the_castled_rook_keeps_its_slot_on_its_new_square() {
        // After O-O the h1 rook stands on f1 and is still rook #0.
        let pos = from_fen("4k3/8/8/8/8/8/8/4K2R w K - 0 1").unwrap();
        let bytes = encode(&[
            Op::Move(9),      // O-O
            Op::Move(7),      // ...Ke8-d8: KING_DIR[6] = (-1,0)
            Op::Move(39 + 6), // rook #0 up 7: f1-f8
            Op::Control(OP_END_VAR),
        ]);
        let decoded = decode(&bytes, pos);
        assert_eq!(mainline_sans(&decoded.game), ["O-O", "Kd8", "Rf8+"]);
    }

    #[test]
    fn a_captured_pawn_leaves_its_slot_empty() {
        // White's c4 pawn (#0) is taken; the d3 pawn (#1) recaptures onto
        // c4 and moves on. The dead #0 must not answer for c4.
        let pos = from_fen("4k3/8/8/1p6/2P5/3P4/8/4K3 b - - 0 1").unwrap();
        let bytes = encode(&[
            Op::Move(111 + 3),     // ...bxc4: black pawn #0, capture towards +file
            Op::Move(111 + 4 + 3), // dxc4: white pawn #1, capture towards -file
            Op::Move(7),           // ...Kd8
            Op::Move(111 + 4),     // c5: pawn #1, one step
            Op::Move(3),           // ...Ke8: KING_DIR[2] = (1,0)
            Op::Move(111 + 4),     // c6
            Op::Control(OP_END_VAR),
        ]);
        let decoded = decode(&bytes, pos);
        assert_eq!(mainline_sans(&decoded.game), ["bxc4", "dxc4", "Kd8", "c5", "Ke8", "c6"]);
        assert!(decoded.problems.is_empty());
    }

    #[test]
    fn decodes_a_promotion_via_two_byte_form() {
        // a7-a8=Q: a7 sqi = file0*8+rank6 = 6, a8 sqi = file0*8+rank7 = 7.
        let pos = from_fen("4k3/P7/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        let value: u16 = 6 | (7 << 6); // promo bits 0 = queen
        let bytes = encode(&[Op::TwoByte((value >> 8) as u8, (value & 0xff) as u8), Op::Control(OP_END_VAR)]);
        let game = decode(&bytes, pos).game;
        let mv = game.node(game.preorder()[1]).mv.unwrap();
        assert_eq!(mv.promotion(), Some(Role::Queen));
    }

    #[test]
    fn a_variation_produces_two_branches() {
        // Main line first: 254 precedes every sibling but the last, so
        // root -> e4 (main, prefixed), back to the branch, -> d4 (variation).
        let bytes = encode(&[
            Op::Control(OP_START_VAR),
            Op::Move(128), // e-pawn (piece_no 4), two steps -> e4
            Op::Control(OP_END_VAR),
            Op::Move(124), // d-pawn (piece_no 3): 111+3*4+1=124, two steps -> d4
            Op::Control(OP_END_VAR),
        ]);
        let game = decode(&bytes, Chess::default()).game;
        let children = &game.root().children;
        assert_eq!(children.len(), 2);
        let main = game.node(children[0]).mv.unwrap();
        let var = game.node(children[1]).mv.unwrap();
        assert_eq!(main.to(), sqi_to_square(4 * 8 + 3).unwrap()); // e4: file 4, rank 3
        assert_eq!(var.to(), sqi_to_square(3 * 8 + 3).unwrap()); // d4: file 3, rank 3
    }

    #[test]
    fn a_captured_rook_renumbers_its_sibling() {
        // White Ra1 (slot 0), Rh1 (slot 1). Black's knight captures Ra1;
        // Rh1 must then be addressable as slot 0 ("1st rook").
        let pos = from_fen("4k3/8/8/8/8/1n6/8/R3K2R w - - 0 1").unwrap();
        let b3_sqi: u16 = 8 + 2; // file 1, rank 2
        let a1_sqi: u16 = 0; // file 0, rank 0
        let h1_sqi: u16 = 7 * 8; // file 7, rank 0
        let h8_sqi: u16 = 7 * 8 + 7;
        let nxa1 = b3_sqi | (a1_sqi << 6); // Nb3xa1, no promotion
        let bytes = encode(&[
            Op::Move(7), // Ke1-d1: KING_DIR[6] = (-1,0)
            Op::TwoByte((nxa1 >> 8) as u8, (nxa1 & 0xff) as u8),
            Op::Move(39 + 6), // Rh1-h8, now slot 0: dir 0 (vertical), stride 7
            Op::Control(OP_END_VAR),
        ]);
        let game = decode(&bytes, pos).game;
        let moves: Vec<_> = game.preorder().iter().skip(1).map(|&id| game.node(id).mv.unwrap()).collect();
        assert_eq!(moves[1].to(), sqi_to_square(a1_sqi).unwrap());
        assert_eq!(moves[2].from(), Some(sqi_to_square(h1_sqi).unwrap()));
        assert_eq!(moves[2].to(), sqi_to_square(h8_sqi).unwrap());
    }

    #[test]
    fn a_null_move_cuts_its_variation_and_the_next_line_is_read() {
        let bytes = encode(&[
            Op::Control(OP_START_VAR),
            Op::Move(128), // 1.e4 (main line)
            Op::Move(128), // 1...e5
            Op::Control(OP_END_VAR),
            Op::Move(124), // 1.d4 (variation)
            Op::Move(0),   // 1...-- cuts the variation
            Op::Control(OP_START_VAR),
            Op::Move(97), // skipped, in a sub-variation of the cut line
            Op::Control(OP_END_VAR),
            Op::Move(97), // skipped
            Op::Control(OP_END_VAR),
        ]);
        let decoded = decode(&bytes, Chess::default());
        let game = &decoded.game;
        assert_eq!(mainline_sans(game), ["e4", "e5"]);
        let variation = game.root().children[1];
        assert!(game.node(variation).children.is_empty());
        let nodes = game.preorder();
        assert_eq!(decoded.order, [Some(nodes[1]), Some(nodes[2]), Some(variation), None, None, None]);
        assert_eq!(
            decoded.problems,
            ["A variation stops at 1...-- (a null move), which can't be imported yet."]
        );
    }

    #[test]
    fn a_cut_main_line_keeps_the_variations_before_it() {
        let bytes = encode(&[
            Op::Control(OP_START_VAR),
            Op::Move(128), // 1.e4
            Op::Move(129), // 1...exd6: the e-pawn's capture, with nothing on d6
            Op::Move(128), // skipped
            Op::Control(OP_END_VAR),
            Op::Move(124), // 1.d4 (variation)
            Op::Control(OP_END_VAR),
        ]);
        let decoded = decode(&bytes, Chess::default());
        assert_eq!(mainline_sans(&decoded.game), ["e4"]);
        assert_eq!(decoded.game.root().children.len(), 2);
        assert_eq!(decoded.order.len(), 4);
        assert_eq!(decoded.problems, ["The main line stops at 1...e7-d6, which isn't a legal move there."]);
    }

    #[test]
    fn a_stream_without_its_end_keeps_what_was_read() {
        let decoded = decode(&encode(&[Op::Move(128), Op::Move(128)]), Chess::default());
        assert_eq!(mainline_sans(&decoded.game), ["e4", "e5"]);
        assert_eq!(decoded.problems, [ENDS_ABRUPTLY]);
    }

    struct BitWriter {
        bytes: Vec<u8>,
        cur: u8,
        nbits: u8,
    }

    impl BitWriter {
        fn push(&mut self, bit: u8) {
            self.cur = (self.cur << 1) | bit;
            self.nbits += 1;
            if self.nbits == 8 {
                self.bytes.push(self.cur);
                self.cur = 0;
                self.nbits = 0;
            }
        }

        fn finish(mut self) -> Vec<u8> {
            while self.nbits != 0 {
                self.push(0);
            }
            self.bytes
        }
    }

    /// Builds a 28-byte setup block the way `parse_setup` expects to read
    /// one, from a plain list of `(file, rank, color, role)` pieces.
    fn encode_setup(pieces: &[(u32, u32, Color, Role)], black_to_move: bool, castle_bits: u8) -> Vec<u8> {
        let mut out = vec![1u8, if black_to_move { 0x10 } else { 0 }, castle_bits, 1];
        let mut w = BitWriter { bytes: Vec::new(), cur: 0, nbits: 0 };
        for file in 0..8 {
            for rank in 0..8 {
                match pieces.iter().find(|&&(f, r, ..)| f == file && r == rank) {
                    None => w.push(0),
                    Some(&(_, _, color, role)) => {
                        w.push(1);
                        w.push(if color == Color::White { 0 } else { 1 });
                        let code = match role {
                            Role::King => 1,
                            Role::Queen => 2,
                            Role::Knight => 3,
                            Role::Bishop => 4,
                            Role::Rook => 5,
                            Role::Pawn => 6,
                        };
                        w.push((code >> 2) & 1);
                        w.push((code >> 1) & 1);
                        w.push(code & 1);
                    }
                }
            }
        }
        let mut board_bytes = w.finish();
        board_bytes.resize(24, 0);
        out.extend(board_bytes);
        out
    }

    #[test]
    fn parse_setup_roundtrips_a_custom_position() {
        use Color::{Black, White};
        let pieces = [
            (0, 0, White, Role::Rook),
            (4, 0, White, Role::King),
            (7, 0, White, Role::Rook),
            (0, 7, Black, Role::Rook),
            (4, 7, Black, Role::King),
            (7, 7, Black, Role::Rook),
        ];
        let block = encode_setup(&pieces, false, 0b1111);
        let pos = parse_setup(&block).unwrap();
        let expected = from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
        assert_eq!(chess_core::position::to_fen(&pos), chess_core::position::to_fen(&expected));
    }

    #[test]
    fn parse_setup_drops_castling_rights_the_board_rules_out() {
        // ChessBase often stores all four rights whatever the board.
        use Color::{Black, White};
        let pieces = [
            (0, 0, White, Role::Rook),
            (5, 0, White, Role::Rook),
            (6, 0, White, Role::King),
            (0, 7, Black, Role::Rook),
            (4, 7, Black, Role::King),
            (7, 7, Black, Role::Rook),
        ];
        let pos = parse_setup(&encode_setup(&pieces, false, 0b1111)).unwrap();
        assert_eq!(chess_core::position::to_fen(&pos), "r3k2r/8/8/8/8/8/8/R4RK1 w kq - 0 1");
    }

    #[test]
    fn parse_setup_puts_the_en_passant_square_behind_the_pawn_that_moved() {
        use Color::{Black, White};
        let pieces = [
            (4, 0, White, Role::King),
            (4, 4, White, Role::Pawn),
            (3, 4, Black, Role::Pawn),
            (4, 7, Black, Role::King),
        ];
        let mut block = encode_setup(&pieces, false, 0);
        block[1] |= 4; // the d-file: ...d7-d5 was just played
        let pos = parse_setup(&block).unwrap();
        assert_eq!(chess_core::position::to_fen(&pos), "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1");
        // No pawn can have just moved on the f-file: read it without one.
        block[1] = (block[1] & 0xf0) | 6;
        let pos = parse_setup(&block).unwrap();
        assert_eq!(chess_core::position::to_fen(&pos), "4k3/8/8/3pP3/8/8/8/4K3 w - - 0 1");
    }
}
