//! Game blob codec (`games.blob` entries).
//!
//! ```text
//! blob     := version:u8 flags:u8 [fen:str] date:str round:str eco:str
//!             n_extra:varint (name:str value:str)*
//!             moves END_GAME
//!             n_annotations:varint annotation*
//! moves    := (move variation* )*        -- iterative main line
//! variation:= BEGIN_VAR move moves END_VAR
//! move     := u8 index into the position's legal moves sorted by
//!             (from, to, promotion)
//! annotation := node:varint kind:u8 payload
//! ```
//!
//! Clock and eval commands (one per ply in online games) get compact binary
//! kinds, but only when re-rendering reproduces the exact original text;
//! anything else is stored verbatim.
//!
//! Node numbers (for annotations) follow emission order: the root is 0,
//! then each move in the order it appears in the byte stream. The legal-move
//! ordering is defined here, not by shakmaty's generator, so the format stays
//! stable across library versions.

use chess_core::game::{Game, Nag, NodeId, Shape, ShapeColor};
use chess_core::position;
use chess_core::shakmaty::{Chess, Move, Position, Square};

use crate::DbError;
use crate::bytes::{ByteReader, put_str, put_varint};

pub const BLOB_VERSION: u8 = 1;

const BEGIN_VAR: u8 = 0xFF;
const END_VAR: u8 = 0xFE;
const END_GAME: u8 = 0xFD;

const FLAG_SETUP: u8 = 1;

const ANN_COMMENT_AFTER: u8 = 1;
const ANN_COMMENT_BEFORE: u8 = 2;
const ANN_NAGS: u8 = 3;
const ANN_SHAPES: u8 = 4;
const ANN_COMMAND: u8 = 5;
/// `[%clk]`/`[%emt]` as deciseconds: `which:u8 (0 clk, 1 emt) value:varint`.
const ANN_TIME: u8 = 6;
/// `[%eval]`: `kind:u8 (0 centipawns, 1 mate) value:zigzag-varint`.
const ANN_EVAL: u8 = 7;

const NO_SQUARE: u8 = 0xFF;

fn move_key(m: &Move) -> (u8, u8, u8) {
    let from = m.from().map_or(NO_SQUARE, |s| s as u8);
    (from, m.to() as u8, m.promotion().map_or(0, |r| r as u8))
}

fn encode_move(pos: &Chess, m: Move) -> u8 {
    let key = move_key(&m);
    let rank = pos.legal_moves().iter().filter(|x| move_key(x) < key).count();
    debug_assert!(rank < END_GAME as usize);
    rank as u8
}

fn decode_move(pos: &Chess, index: u8) -> Result<Move, DbError> {
    let mut legal = pos.legal_moves();
    legal.sort_unstable_by_key(move_key);
    legal
        .get(index as usize)
        .copied()
        .ok_or_else(|| DbError::Corrupt(format!("move index {index} out of range")))
}

/// The legal move of `pos` with this index (see the module docs), or `None`
/// if there are fewer moves.
pub fn move_at(pos: &Chess, index: u8) -> Option<Move> {
    let mut legal = pos.legal_moves();
    if usize::from(index) >= legal.len() {
        return None;
    }
    let (_, m, _) = legal.select_nth_unstable_by_key(usize::from(index), move_key);
    Some(*m)
}

fn skip_str(r: &mut ByteReader<'_>) -> Result<(), DbError> {
    let len = r.varint()? as usize;
    r.bytes(len)?;
    Ok(())
}

/// Reads a blob's header, leaving `r` at the moves; the set-up FEN, if any.
fn skip_header(r: &mut ByteReader<'_>) -> Result<Option<String>, DbError> {
    let version = r.u8()?;
    if version != BLOB_VERSION {
        return Err(DbError::Corrupt(format!("unknown blob version {version}")));
    }
    let fen = if r.u8()? & FLAG_SETUP != 0 { Some(r.str()?) } else { None };
    for _ in 0..3 {
        skip_str(r)?;
    }
    for _ in 0..extra_strings(r)? {
        skip_str(r)?;
    }
    Ok(fen)
}

/// The number of strings the extra tags take (a name and a value each),
/// read from their count.
fn extra_strings(r: &mut ByteReader<'_>) -> Result<u64, DbError> {
    r.varint()?.checked_mul(2).ok_or_else(|| DbError::Corrupt("too many extra tags".into()))
}

/// Feeds the main line's move bytes to `each` until it returns false or
/// the game ends, skipping variations.
fn walk_mainline(r: &mut ByteReader<'_>, mut each: impl FnMut(u8) -> bool) -> Result<(), DbError> {
    let mut depth = 0u32;
    loop {
        match r.u8()? {
            END_GAME if depth == 0 => return Ok(()),
            END_GAME => return Err(DbError::Corrupt("unterminated variation".into())),
            BEGIN_VAR => depth += 1,
            END_VAR => {
                depth = depth.checked_sub(1).ok_or_else(|| DbError::Corrupt("unbalanced variation".into()))?
            }
            b if depth == 0 && !each(b) => return Ok(()),
            _ => {}
        }
    }
}

/// The main line of a blob without building the game tree: the set-up
/// position, if any, and the main line's move bytes (decode them with
/// [`move_at`]). Much cheaper than [`decode_game`]; the position index uses it.
pub fn mainline(blob: &[u8]) -> Result<(Option<Chess>, Vec<u8>), DbError> {
    let mut r = ByteReader::new(blob);
    let setup = match skip_header(&mut r)? {
        Some(fen) => Some(position::from_fen(&fen).map_err(|e| DbError::Corrupt(e.to_string()))?),
        None => None,
    };
    let mut moves = Vec::with_capacity(blob.len());
    walk_mainline(&mut r, |b| {
        moves.push(b);
        true
    })?;
    Ok((setup, moves))
}

/// Up to `take` main-line move bytes from ply `skip` on (fewer where the
/// game ends), appended to `out`. Cheaper still than [`mainline`]: no
/// set-up position is parsed and the walk stops once it has them. The
/// Reference tab's lines (OPEN-14) read the moves after a position with it.
pub fn mainline_window(blob: &[u8], skip: usize, take: usize, out: &mut Vec<u8>) -> Result<(), DbError> {
    let mut r = ByteReader::new(blob);
    skip_header(&mut r)?;
    if take == 0 {
        return Ok(());
    }
    let mut ply = 0usize;
    let end = skip.saturating_add(take);
    walk_mainline(&mut r, |b| {
        if ply >= skip {
            out.push(b);
        }
        ply += 1;
        ply < end
    })
}

/// Extra tags that identify one game, as servers and broadcasts write them
/// (its own page, its id, when it started, the board in a team match):
/// duplicate detection (DB-08) never takes two games whose values differ
/// for one of them as copies. Names are matched ignoring case.
pub const GAME_ID_TAGS: [&str; 7] = ["Link", "GameId", "GameURL", "ChapterURL", "UTCTime", "Time", "Board"];

/// A game's main line and a count of what else its blob holds, read
/// without building the game tree: duplicate detection (DB-08) compares
/// copies of a game by their moves and keeps the best annotated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlobDigest {
    /// The set-up position's FEN, if the game doesn't start from the
    /// standard position.
    pub setup: Option<String>,
    /// The main line's move bytes (decode them with [`move_at`]).
    pub mainline: Vec<u8>,
    /// Variations at any depth.
    pub variations: u32,
    /// Comments before or after a move (or at the start).
    pub comments: u32,
    /// Symbols such as `!` or `+-` ([`Nag`]s).
    pub nags: u32,
    /// Coloured squares and arrows.
    pub shapes: u32,
    /// Commands such as `[%clk]` and `[%eval]`, which programs write.
    pub commands: u32,
    /// Tags beyond those the index record keeps (`TimeControl`, `UTCDate`…).
    pub extra_tags: u32,
    /// A hash of the moves with their variations and every annotation
    /// (comments, symbols, arrows, and commands such as clock times and
    /// evals), or 0 when the game has none: two games with the same hash
    /// are annotated alike.
    pub annotations_hash: u64,
    /// The [`GAME_ID_TAGS`] the game has (not empty, `?` or `-`): (index
    /// in that list, hash of the value), by index.
    pub game_ids: Vec<(u8, u64)>,
}

impl BlobDigest {
    /// Variations, comments, symbols and arrows.
    pub fn notes(&self) -> u32 {
        self.variations.saturating_add(self.comments).saturating_add(self.nags).saturating_add(self.shapes)
    }
}

/// Reads a blob into a [`BlobDigest`].
pub fn digest(blob: &[u8]) -> Result<BlobDigest, DbError> {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut r = ByteReader::new(blob);
    let version = r.u8()?;
    if version != BLOB_VERSION {
        return Err(DbError::Corrupt(format!("unknown blob version {version}")));
    }
    let mut d = BlobDigest::default();
    if r.u8()? & FLAG_SETUP != 0 {
        d.setup = Some(r.str()?);
    }
    for _ in 0..3 {
        skip_str(&mut r)?;
    }
    let extra = extra_strings(&mut r)? / 2;
    d.extra_tags = u32::try_from(extra).unwrap_or(u32::MAX);
    for _ in 0..extra {
        let len = r.varint()? as usize;
        let name = r.bytes(len)?;
        let len = r.varint()? as usize;
        let value = r.bytes(len)?;
        if let Some(i) = GAME_ID_TAGS.iter().position(|t| t.as_bytes().eq_ignore_ascii_case(name))
            && !matches!(value, b"" | b"?" | b"-")
            && !d.game_ids.iter().any(|&(j, _)| j == i as u8)
        {
            let mut h = DefaultHasher::new();
            value.hash(&mut h);
            d.game_ids.push((i as u8, h.finish()));
        }
    }
    d.game_ids.sort_unstable();
    let moves_from = r.position();
    let mut depth = 0u32;
    loop {
        match r.u8()? {
            END_GAME if depth == 0 => break,
            END_GAME => return Err(DbError::Corrupt("unterminated variation".into())),
            BEGIN_VAR => {
                depth += 1;
                d.variations += 1;
            }
            END_VAR => {
                depth = depth.checked_sub(1).ok_or_else(|| DbError::Corrupt("unbalanced variation".into()))?
            }
            b if depth == 0 => d.mainline.push(b),
            _ => {}
        }
    }
    let mut notes = DefaultHasher::new();
    blob[moves_from..r.position()].hash(&mut notes);
    for _ in 0..r.varint()? {
        let at = r.position();
        r.varint()?;
        let kind = r.u8()?;
        match kind {
            ANN_COMMENT_AFTER | ANN_COMMENT_BEFORE => {
                skip_str(&mut r)?;
                d.comments += 1;
            }
            ANN_NAGS => {
                let n = r.varint()? as usize;
                r.bytes(n)?;
                d.nags += n as u32;
            }
            ANN_SHAPES => {
                let n = r.varint()? as usize;
                r.bytes(n.checked_mul(3).ok_or_else(|| DbError::Corrupt("too many shapes".into()))?)?;
                d.shapes += n as u32;
            }
            ANN_COMMAND => {
                skip_str(&mut r)?;
                skip_str(&mut r)?;
                d.commands += 1;
            }
            ANN_TIME | ANN_EVAL => {
                r.u8()?;
                r.varint()?;
                d.commands += 1;
            }
            other => return Err(DbError::Corrupt(format!("unknown annotation kind {other}"))),
        }
        blob[at..r.position()].hash(&mut notes);
    }
    if d.notes() > 0 || d.commands > 0 {
        d.annotations_hash = notes.finish().max(1);
    }
    Ok(d)
}

/// The Date, Round and ECO strings of a blob, read without decoding the
/// moves (copying games between databases needs them for the record).
pub fn date_round_eco(blob: &[u8]) -> Result<(String, String, String), DbError> {
    let mut r = ByteReader::new(blob);
    let version = r.u8()?;
    if version != BLOB_VERSION {
        return Err(DbError::Corrupt(format!("unknown blob version {version}")));
    }
    if r.u8()? & FLAG_SETUP != 0 {
        skip_str(&mut r)?;
    }
    Ok((r.str()?, r.str()?, r.str()?))
}

pub fn encode_game(game: &Game) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + game.mainline_len() * 2);
    out.push(BLOB_VERSION);
    out.push(if game.setup.is_some() { FLAG_SETUP } else { 0 });
    if let Some(setup) = &game.setup {
        put_str(&mut out, &position::to_fen(setup));
    }
    let h = &game.headers;
    put_str(&mut out, &h.date);
    put_str(&mut out, &h.round);
    put_str(&mut out, &h.eco);
    put_varint(&mut out, h.extra.len() as u64);
    for (name, value) in &h.extra {
        put_str(&mut out, name);
        put_str(&mut out, value);
    }

    let mut order = vec![NodeId::ROOT];
    encode_line(game, &mut out, NodeId::ROOT, game.start_position(), &mut order);
    out.push(END_GAME);

    let annotated: Vec<(usize, NodeId)> = order
        .iter()
        .enumerate()
        .filter(|&(_, &id)| game.node(id).has_annotations())
        .map(|(i, &id)| (i, id))
        .collect();
    let mut entries = Vec::new();
    let mut count = 0u64;
    for (index, id) in annotated {
        let node = game.node(id);
        let mut entry = |kind: u8, payload: &dyn Fn(&mut Vec<u8>)| {
            put_varint(&mut entries, index as u64);
            entries.push(kind);
            payload(&mut entries);
            count += 1;
        };
        if !node.comment_after.is_empty() {
            entry(ANN_COMMENT_AFTER, &|b| put_str(b, &node.comment_after));
        }
        if !node.comment_before.is_empty() {
            entry(ANN_COMMENT_BEFORE, &|b| put_str(b, &node.comment_before));
        }
        if !node.nags.is_empty() {
            entry(ANN_NAGS, &|b| {
                put_varint(b, node.nags.len() as u64);
                b.extend(node.nags.iter().map(|n| n.0));
            });
        }
        if !node.shapes.is_empty() {
            entry(ANN_SHAPES, &|b| {
                put_varint(b, node.shapes.len() as u64);
                for s in &node.shapes {
                    b.push(s.color.letter() as u8);
                    b.push(s.from as u8);
                    b.push(s.to.map_or(NO_SQUARE, |t| t as u8));
                }
            });
        }
        for (name, value) in &node.commands {
            if let Some((which, ds)) = compact_time(name, value) {
                entry(ANN_TIME, &|b| {
                    b.push(which);
                    put_varint(b, ds);
                });
            } else if let Some((kind, v)) = compact_eval(name, value) {
                entry(ANN_EVAL, &|b| {
                    b.push(kind);
                    put_varint(b, zigzag(v));
                });
            } else {
                entry(ANN_COMMAND, &|b| {
                    put_str(b, name);
                    put_str(b, value);
                });
            }
        }
    }
    put_varint(&mut out, count);
    out.extend_from_slice(&entries);
    out
}

fn encode_line(game: &Game, out: &mut Vec<u8>, mut parent: NodeId, mut pos: Chess, order: &mut Vec<NodeId>) {
    loop {
        let children = &game.node(parent).children;
        let Some(&main) = children.first() else { return };
        let main_move = game.node(main).mv.expect("non-root node has a move");
        out.push(encode_move(&pos, main_move));
        order.push(main);
        for &alt in &children[1..] {
            let alt_move = game.node(alt).mv.expect("non-root node has a move");
            out.push(BEGIN_VAR);
            out.push(encode_move(&pos, alt_move));
            order.push(alt);
            let mut alt_pos = pos.clone();
            alt_pos.play_unchecked(alt_move);
            encode_line(game, out, alt, alt_pos, order);
            out.push(END_VAR);
        }
        pos.play_unchecked(main_move);
        parent = main;
    }
}

/// Decodes a blob into a game. Only `date`, `round`, `eco` and `extra`
/// headers are stored in the blob; the caller fills the rest from the
/// index record and name tables.
pub fn decode_game(blob: &[u8]) -> Result<Game, DbError> {
    let mut r = ByteReader::new(blob);
    let version = r.u8()?;
    if version != BLOB_VERSION {
        return Err(DbError::Corrupt(format!("unknown blob version {version}")));
    }
    let flags = r.u8()?;
    let setup = if flags & FLAG_SETUP != 0 {
        let fen = r.str()?;
        Some(position::from_fen(&fen).map_err(|e| DbError::Corrupt(e.to_string()))?)
    } else {
        None
    };
    let mut game = Game::new(Default::default(), setup);
    game.headers.date = r.str()?;
    game.headers.round = r.str()?;
    game.headers.eco = r.str()?;
    let n_extra = r.varint()?;
    for _ in 0..n_extra {
        let name = r.str()?;
        let value = r.str()?;
        game.headers.extra.push((name, value));
    }

    let mut order = vec![NodeId::ROOT];
    let start = game.start_position();
    decode_line(&mut r, &mut game, NodeId::ROOT, start, &mut order)?;
    if r.u8()? != END_GAME {
        return Err(DbError::Corrupt("expected end of game".into()));
    }

    let n_ann = r.varint()?;
    for _ in 0..n_ann {
        let index = r.varint()? as usize;
        let id = *order
            .get(index)
            .ok_or_else(|| DbError::Corrupt(format!("annotation for missing node {index}")))?;
        let kind = r.u8()?;
        let node = game.node_mut(id);
        match kind {
            ANN_COMMENT_AFTER => node.comment_after = r.str()?,
            ANN_COMMENT_BEFORE => node.comment_before = r.str()?,
            ANN_NAGS => {
                let n = r.varint()? as usize;
                node.nags = r.bytes(n)?.iter().map(|&b| Nag(b)).collect();
            }
            ANN_SHAPES => {
                let n = r.varint()?;
                for _ in 0..n {
                    let color = ShapeColor::from_letter(r.u8()? as char)
                        .ok_or_else(|| DbError::Corrupt("bad shape color".into()))?;
                    let from = square(r.u8()?)?;
                    let to = match r.u8()? {
                        NO_SQUARE => None,
                        b => Some(square(b)?),
                    };
                    node.shapes.push(Shape { color, from, to });
                }
            }
            ANN_COMMAND => {
                let name = r.str()?;
                let value = r.str()?;
                node.commands.push((name, value));
            }
            ANN_TIME => {
                let name = match r.u8()? {
                    0 => "clk",
                    1 => "emt",
                    other => return Err(DbError::Corrupt(format!("bad time kind {other}"))),
                };
                node.commands.push((name.to_owned(), render_time(r.varint()?)));
            }
            ANN_EVAL => {
                let kind = r.u8()?;
                let value = unzigzag(r.varint()?);
                node.commands.push(("eval".to_owned(), render_eval(kind, value)));
            }
            other => return Err(DbError::Corrupt(format!("unknown annotation kind {other}"))),
        }
    }
    Ok(game)
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// `h:mm:ss` or `h:mm:ss.d` in deciseconds.
fn parse_time(s: &str) -> Option<u64> {
    let (hms, tenths) = match s.split_once('.') {
        Some((hms, d)) if d.len() == 1 => (hms, d.parse::<u64>().ok()?),
        Some(_) => return None,
        None => (s, 0),
    };
    let mut parts = hms.split(':');
    let h = parts.next()?.parse::<u64>().ok()?;
    let m = parts.next()?.parse::<u64>().ok()?;
    let sec = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() || m >= 60 || sec >= 60 {
        return None;
    }
    Some(((h * 60 + m) * 60 + sec) * 10 + tenths)
}

fn render_time(ds: u64) -> String {
    let (secs, tenths) = (ds / 10, ds % 10);
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if tenths == 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{h}:{m:02}:{s:02}.{tenths}") }
}

fn compact_time(name: &str, value: &str) -> Option<(u8, u64)> {
    let which = match name {
        "clk" => 0,
        "emt" => 1,
        _ => return None,
    };
    let ds = parse_time(value)?;
    (render_time(ds) == value).then_some((which, ds))
}

/// Pawn evals like `0.17`, `-1.2` (centipawns) or mates like `#-9`.
fn parse_eval(s: &str) -> Option<(u8, i64)> {
    if let Some(mate) = s.strip_prefix('#') {
        return Some((1, mate.parse().ok()?));
    }
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = digits.split_once('.')?;
    let frac_cp = match frac.len() {
        1 => frac.parse::<i64>().ok()? * 10,
        2 => frac.parse::<i64>().ok()?,
        _ => return None,
    };
    let cp = int.parse::<i64>().ok()?.checked_mul(100)?.checked_add(frac_cp)?;
    Some((0, if neg { -cp } else { cp }))
}

fn render_eval(kind: u8, v: i64) -> String {
    if kind == 1 {
        return format!("#{v}");
    }
    let sign = if v < 0 { "-" } else { "" };
    let (int, rem) = (v.unsigned_abs() / 100, v.unsigned_abs() % 100);
    if rem % 10 == 0 { format!("{sign}{int}.{}", rem / 10) } else { format!("{sign}{int}.{rem:02}") }
}

fn compact_eval(name: &str, value: &str) -> Option<(u8, i64)> {
    if name != "eval" {
        return None;
    }
    let (kind, v) = parse_eval(value)?;
    (render_eval(kind, v) == value).then_some((kind, v))
}

fn square(b: u8) -> Result<Square, DbError> {
    if b < 64 { Ok(Square::new(u32::from(b))) } else { Err(DbError::Corrupt(format!("bad square {b}"))) }
}

fn decode_line(
    r: &mut ByteReader<'_>,
    game: &mut Game,
    mut parent: NodeId,
    mut pos: Chess,
    order: &mut Vec<NodeId>,
) -> Result<(), DbError> {
    loop {
        match r.peek() {
            None => return Err(DbError::Corrupt("unterminated move list".into())),
            Some(END_VAR | END_GAME) => return Ok(()),
            Some(BEGIN_VAR) => return Err(DbError::Corrupt("variation without main move".into())),
            Some(_) => {}
        }
        let main_move = decode_move(&pos, r.u8()?)?;
        let main = game.add_child(parent, main_move);
        order.push(main);
        while r.peek() == Some(BEGIN_VAR) {
            r.u8()?;
            let alt_move = decode_move(&pos, r.u8()?)?;
            let alt = game.add_child(parent, alt_move);
            order.push(alt);
            let mut alt_pos = pos.clone();
            alt_pos.play_unchecked(alt_move);
            decode_line(r, game, alt, alt_pos, order)?;
            if r.u8()? != END_VAR {
                return Err(DbError::Corrupt("expected end of variation".into()));
            }
        }
        pos.play_unchecked(main_move);
        parent = main;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::pgn::read_pgn_str;

    #[test]
    fn round_trips_annotated_game() {
        let pgn = r#"[Date "2024.01.02"]
[Round "5"]
[ECO "C20"]
[TimeControl "180+2"]

{Intro [%csl Ge4]} 1. e4 $1 {[%clk 0:03:00] best} e5 (1... c5 {Sicilian} 2. Nf3 (2. Nc3 Nc6) 2... d6)
({French} 1... e6 2. d4) 2. Nf3 {[%cal Gf3e5]} Nc6 *"#;
        let mut game = read_pgn_str(pgn).remove(0);
        let blob = encode_game(&game);
        let decoded = decode_game(&blob).unwrap();
        // White/Black/Result etc. live in the index record, not the blob.
        game.headers.result = Default::default();
        assert_eq!(decoded, game);
    }

    #[test]
    fn one_byte_per_move() {
        let game = read_pgn_str("1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *").remove(0);
        let blob = encode_game(&game);
        // version, flags, 3 empty strings, 0 extras, 6 moves, END, 0 annotations
        assert_eq!(blob.len(), 2 + 3 + 1 + 6 + 1 + 1);
    }

    #[test]
    fn setup_positions_and_promotions() {
        let pgn = "[FEN \"r3k2r/1P6/8/8/8/8/6p1/R3K2R w KQkq - 0 30\"]\n\n30. bxa8=N Ke7 31. O-O-O gxh1=Q 32. Rxh1 *";
        let mut game = read_pgn_str(pgn).remove(0);
        game.headers.result = Default::default();
        assert_eq!(decode_game(&encode_game(&game)).unwrap(), game);
    }

    #[test]
    fn compact_clock_and_eval_round_trip_exactly() {
        for v in ["0:03:00", "1:00:00", "0:00:05.3", "12:59:59"] {
            assert_eq!(compact_time("clk", v).map(|(_, ds)| render_time(ds)).as_deref(), Some(v));
        }
        for v in ["0:3:00", "0:03:00.25", "-1:00:00", "abc"] {
            assert_eq!(compact_time("clk", v), None, "{v}");
        }
        for v in ["0.17", "0.2", "-0.05", "-1.0", "0.0", "12.34", "#-9", "#3"] {
            let (kind, cp) = compact_eval("eval", v).unwrap_or_else(|| panic!("{v}"));
            assert_eq!(render_eval(kind, cp), v);
        }
        for v in ["0.20", "1", "0.123", "0.36,20", "+0.3"] {
            assert_eq!(compact_eval("eval", v), None, "{v}");
        }

        let pgn = "1. e4 {[%eval 0.36] [%clk 0:03:00]} e5 {[%eval 0.20] [%clk 0:02:59.5] [%emt 0:00:01]} *";
        let game = read_pgn_str(pgn).remove(0);
        assert_eq!(decode_game(&encode_game(&game)).unwrap(), game);
    }

    #[test]
    fn mainline_skips_variations_and_annotations() {
        let pgn = "[FEN \"r3k2r/1P6/8/8/8/8/6p1/R3K2R w KQkq - 0 30\"]\n[Event \"x\"]\n\n\
                   30. bxa8=N {c} (30. b8=Q+ Kd7 (30... Ke7)) Ke7 $1 31. O-O-O gxh1=Q *";
        let game = read_pgn_str(pgn).remove(0);
        let (setup, bytes) = mainline(&encode_game(&game)).unwrap();
        let mut pos = setup.expect("set-up position");
        assert_eq!(pos, game.start_position());
        let mut sans = Vec::new();
        for b in bytes {
            let m = move_at(&pos, b).unwrap();
            sans.push(chess_core::notation::san(&pos, m));
            pos.play_unchecked(m);
        }
        assert_eq!(sans, ["bxa8=N", "Ke7", "O-O-O", "gxh1=Q"]);
        assert_eq!(move_at(&Chess::default(), 20), None, "20 legal moves at the start");
    }

    #[test]
    fn mainline_window_reads_a_slice_of_the_main_line() {
        let pgn = "1. e4 (1. d4 d5) e5 2. Nf3 {x} Nc6 (2... d6 3. d4) 3. Bb5 a6 *";
        let blob = encode_game(&read_pgn_str(pgn).remove(0));
        let (_, all) = mainline(&blob).unwrap();
        assert_eq!(all.len(), 6);
        let window = |skip, take| {
            let mut out = vec![0xAA];
            mainline_window(&blob, skip, take, &mut out).unwrap();
            out[1..].to_vec()
        };
        assert_eq!(window(0, 10), all, "the whole line when it's shorter");
        assert_eq!(window(2, 3), all[2..5]);
        assert_eq!(window(4, 6), all[4..], "fewer where the game ends");
        assert_eq!(window(6, 6), Vec::<u8>::new(), "nothing after the last move");
        assert_eq!(window(9, 6), Vec::<u8>::new());
        assert_eq!(window(1, 0), Vec::<u8>::new());
        assert_eq!(window(0, 0), Vec::<u8>::new());
        assert!(mainline_window(&[9, 0], 0, 1, &mut Vec::new()).is_err(), "a bad blob");
    }

    #[test]
    fn digest_counts_what_a_game_holds() {
        let pgn = r#"[TimeControl "180+2"]
[UTCDate "2024.01.02"]

{Intro} 1. e4 $1 {[%clk 0:03:00] best} e5 (1... c5 {Sicilian} 2. Nf3 (2. Nc3 Nc6 $2) 2... d6)
({French} 1... e6) 2. Nf3 {[%cal Gf3e5,Rd1h5] [%eval 0.3]} Nc6 {[%foo bar]} *"#;
        let game = read_pgn_str(pgn).remove(0);
        let blob = encode_game(&game);
        let d = digest(&blob).unwrap();
        assert_eq!(d.mainline, mainline(&blob).unwrap().1);
        assert_eq!(d.mainline.len(), 4);
        assert_eq!(d.setup, None);
        assert_eq!((d.variations, d.comments, d.nags, d.shapes), (3, 4, 2, 2));
        assert_eq!(d.commands, 3, "clk, eval and an unknown command");
        assert_eq!(d.extra_tags, 2);

        let plain = digest(&encode_game(&read_pgn_str("1. d4 d5 *").remove(0))).unwrap();
        assert_eq!(plain, BlobDigest { mainline: plain.mainline.clone(), ..BlobDigest::default() });
        let fen = "8/8/8/4k3/8/8/4P3/4K3 w - - 0 1";
        let setup = read_pgn_str(&format!("[FEN \"{fen}\"]\n\n1. Kd2 *")).remove(0);
        assert_eq!(digest(&encode_game(&setup)).unwrap().setup.as_deref(), Some(fen));
        assert!(digest(&blob[..blob.len() - 3]).is_err(), "a truncated blob");
    }

    #[test]
    fn digest_hashes_the_annotations_and_reads_the_game_ids() {
        let notes = |movetext: &str| {
            digest(&encode_game(&read_pgn_str(movetext).remove(0))).unwrap().annotations_hash
        };
        assert_eq!(notes("1. e4 e5 2. Nf3 *"), 0, "no annotations");
        let clocks = notes("1. e4 {[%clk 0:03:00]} e5 {[%eval 0.2]} *");
        assert_ne!(clocks, 0, "a clock and an eval are annotations too (DB-08 review R2-1)");
        assert_eq!(clocks, notes("1. e4 {[%clk 0:03:00]} e5 {[%eval 0.2]} *"));
        let commented = notes("1. e4 {Best by test} e5 *");
        assert_ne!(commented, 0);
        for other in [
            "1. e4 {Best by test.} e5 *",
            "1. e4 e5 {Best by test} *",
            "1. e4 $1 e5 *",
            "1. e4 (1. d4) e5 *",
            "1. e4 {[%csl Ge4]} e5 *",
            "1. e4 {Best by test} e5 {[%clk 0:01:00]} *",
            "1. e4 {[%clk 0:03:00]} e5 {[%eval 0.3]} *",
        ] {
            let other = notes(other);
            assert!(other != 0 && other != commented && other != clocks, "{other}");
        }

        let tags = "[Link \"https://www.chess.com/game/live/1\"]\n[utctime \"18:00:01\"]\n[TimeControl \"60\"]\n\
                    [Board \"?\"]\n\n1. f3 e5 *";
        let d = digest(&encode_game(&read_pgn_str(tags).remove(0))).unwrap();
        assert_eq!(
            d.game_ids.iter().map(|&(i, _)| GAME_ID_TAGS[i as usize]).collect::<Vec<_>>(),
            ["Link", "UTCTime"]
        );
        assert_eq!(d.extra_tags, 4);
    }

    #[test]
    fn a_huge_count_of_extra_tags_is_an_error() {
        // 2^63 extra tags: twice that overflows.
        let mut blob = vec![BLOB_VERSION, 0, 0, 0, 0];
        blob.extend_from_slice(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert!(digest(&blob).is_err());
        assert!(mainline(&blob).is_err());
        assert!(mainline_window(&blob, 0, 1, &mut Vec::new()).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_game(&[]).is_err());
        assert!(decode_game(&[9, 0]).is_err());
        assert!(decode_game(&[BLOB_VERSION, 0, 0, 0, 0, 0, 250, END_GAME, 0]).is_err());
    }
}
