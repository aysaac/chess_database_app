//! `.cba` — annotations (`docs/io-03-cbh.md` §3.3). All big-endian.
//!
//! Only the types the Reference/Notation tabs and blunder check need are
//! interpreted (comments, symbols, coloured squares/arrows); anything else
//! is skipped using its declared size.

use std::fs::File;
use std::path::Path;

use chess_core::game::{Game, Nag, NodeId, Shape, ShapeColor};

use crate::DbError;
use crate::cb::bytes::{be16, be24};
use crate::cb::moves::sqi_to_square;
use crate::io::read_exact_at;

const TYPE_COMMENT_AFTER: u8 = 0x02;
const TYPE_COMMENT_BEFORE: u8 = 0x82;
const TYPE_SYMBOLS: u8 = 0x03;
const TYPE_SQUARES: u8 = 0x04;
const TYPE_ARROWS: u8 = 0x05;

pub struct CbaFile {
    file: File,
}

impl CbaFile {
    pub fn open(path: &Path) -> Result<CbaFile, DbError> {
        Ok(CbaFile { file: File::open(path)? })
    }

    /// Reads the annotation record at `offset` (from a game's `.cbh`/`.cbj`
    /// record; 0 means "no annotations", callers check that themselves).
    pub fn read_record(&self, offset: u32) -> Result<Vec<u8>, DbError> {
        let mut prefix = [0u8; 0x0e];
        read_exact_at(&self.file, &mut prefix, offset as u64)?;
        let size = be32_at(&prefix, 0x0a) as usize;
        if size < 0x0e {
            return Err(DbError::Corrupt(format!(".cba record at {offset} has size {size} < 14")));
        }
        let mut buf = vec![0u8; size];
        read_exact_at(&self.file, &mut buf, offset as u64)?;
        Ok(buf)
    }
}

fn be32_at(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// `position` is a signed 24-bit value (two's complement in the low 24
/// bits): -1 means "before the first move", otherwise a 0-based index into
/// `order` (see `apply_annotations`).
fn position_signed(raw: u32) -> i32 {
    if raw & 0x0080_0000 != 0 { (raw | 0xff00_0000) as i32 } else { raw as i32 }
}

fn color_from_code(code: u8) -> Option<ShapeColor> {
    match code {
        2 => Some(ShapeColor::Green),
        3 => Some(ShapeColor::Yellow),
        4 => Some(ShapeColor::Red),
        _ => None,
    }
}

/// A comment's language code (`docs/io-03-cbh.md` §3.3): 0 is text for
/// any language.
const LANGUAGE_ANY: u8 = 0;
const LANGUAGE_ENGLISH: u8 = 42;

struct Comment {
    node: NodeId,
    before: bool,
    language: u8,
    text: String,
}

/// Applies a `.cba` game record's annotations onto `game`. `order` maps an
/// annotation's 0-based move position to the node it refers to: it's
/// `DecodedGame::order`, the moves in the `.cbg` storage order `.cba`
/// positions count in (`docs/io-03-cbh.md` §3.3). Position -1 is the root;
/// an annotation on a move that wasn't imported (`None`) is dropped. After
/// an error the annotations read before it are still applied.
pub fn apply_annotations(game: &mut Game, order: &[Option<NodeId>], record: &[u8]) -> Result<(), DbError> {
    let mut comments = Vec::new();
    let read = read_annotations(game, order, record, &mut comments);
    apply_comments(game, &comments);
    read
}

fn read_annotations(
    game: &mut Game,
    order: &[Option<NodeId>],
    record: &[u8],
    comments: &mut Vec<Comment>,
) -> Result<(), DbError> {
    let mut off = 0x0e;
    while off + 6 <= record.len() {
        let position = position_signed(be24(record, off));
        let kind = record[off + 3];
        let size = be16(record, off + 4) as usize;
        if size < 6 || off + size > record.len() {
            return Err(DbError::Corrupt(format!("`.cba` annotation at {off} has bad size {size}")));
        }
        let data = &record[off + 6..off + size];
        off += size;
        let node = if position < 0 {
            NodeId::ROOT
        } else {
            match order.get(position as usize).copied().flatten() {
                Some(id) => id,
                None => continue,
            }
        };
        match kind {
            TYPE_COMMENT_AFTER | TYPE_COMMENT_BEFORE if data.len() >= 2 => {
                let text = decode_text(&data[2..]);
                if !text.is_empty() {
                    comments.push(Comment {
                        node,
                        before: kind == TYPE_COMMENT_BEFORE,
                        language: data[1],
                        text,
                    });
                }
            }
            _ => apply_one(game, node, kind, data),
        }
    }
    Ok(())
}

/// ChessBase keeps a comment in several languages side by side (the
/// databases it ships with have German, Spanish, French and Russian). Each
/// place keeps its text for any language and one language of the rest:
/// English if it's there, else the first.
fn apply_comments(game: &mut Game, comments: &[Comment]) {
    for (i, c) in comments.iter().enumerate() {
        let same_place = |o: &&Comment| o.node == c.node && o.before == c.before;
        if comments[..i].iter().any(|o| same_place(&o)) {
            continue; // done with its place's first comment
        }
        let place: Vec<&Comment> = comments[i..].iter().filter(same_place).collect();
        let language = if place.iter().any(|o| o.language == LANGUAGE_ENGLISH) {
            LANGUAGE_ENGLISH
        } else {
            place.iter().map(|o| o.language).find(|&l| l != LANGUAGE_ANY).unwrap_or(LANGUAGE_ANY)
        };
        let n = game.node_mut(c.node);
        let into = if c.before { &mut n.comment_before } else { &mut n.comment_after };
        for o in place.iter().filter(|o| o.language == LANGUAGE_ANY || o.language == language) {
            append_text(into, &o.text);
        }
    }
}

/// Applies an annotation other than a comment.
fn apply_one(game: &mut Game, node: NodeId, kind: u8, data: &[u8]) {
    match kind {
        TYPE_SYMBOLS => {
            let n = game.node_mut(node);
            for &b in data.iter().take(3) {
                if b != 0 {
                    n.nags.push(Nag(b));
                }
            }
        }
        TYPE_SQUARES => {
            let n = game.node_mut(node);
            for chunk in data.as_chunks::<2>().0 {
                if let Some(color) = color_from_code(chunk[0])
                    && let Ok(sq) = sqi_to_square(u16::from(chunk[1].saturating_sub(1)))
                {
                    n.shapes.push(Shape { color, from: sq, to: None });
                }
            }
        }
        TYPE_ARROWS => {
            let n = game.node_mut(node);
            for chunk in data.as_chunks::<3>().0 {
                if let Some(color) = color_from_code(chunk[0])
                    && let (Ok(from), Ok(to)) = (
                        sqi_to_square(u16::from(chunk[1].saturating_sub(1))),
                        sqi_to_square(u16::from(chunk[2].saturating_sub(1))),
                    )
                {
                    n.shapes.push(Shape { color, from, to: Some(to) });
                }
            }
        }
        _ => {}
    }
}

fn append_text(into: &mut String, text: &str) {
    if !into.is_empty() {
        into.push(' ');
    }
    into.push_str(text);
}

/// Free text has no declared encoding (`docs/io-03-cbh.md` §3): try UTF-8,
/// fall back to Windows-1252 (treated here as Latin-1, close enough for the
/// printable range this format actually uses). Line ends become `\n`, and
/// the line ends ChessBase leaves at the end go.
fn decode_text(bytes: &[u8]) -> String {
    let text: String = match std::str::from_utf8(bytes) {
        Ok(s) => s.trim_end_matches('\0').to_owned(),
        Err(_) => bytes.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect(),
    };
    text.replace("\r\n", "\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::game::Headers;
    use chess_core::shakmaty::Chess;

    fn record_with(annotations: &[(i32, u8, Vec<u8>)]) -> Vec<u8> {
        let mut buf = vec![0u8; 0x0e];
        for (position, kind, data) in annotations {
            let pos_bytes = (*position as u32).to_be_bytes();
            buf.extend_from_slice(&pos_bytes[1..4]);
            buf.push(*kind);
            let size = (6 + data.len()) as u16;
            buf.extend_from_slice(&size.to_be_bytes());
            buf.extend_from_slice(data);
        }
        let total = buf.len() as u32;
        buf[0x0a..0x0e].copy_from_slice(&total.to_be_bytes());
        buf
    }

    #[test]
    fn comment_after_attaches_to_the_root_for_position_minus_one() {
        let mut game = Game::new(Headers::default(), None);
        let record = record_with(&[(-1, TYPE_COMMENT_AFTER, [0u8, 0, b'H', b'i'].to_vec())]);
        apply_annotations(&mut game, &[], &record).unwrap();
        assert_eq!(game.root().comment_after, "Hi");
    }

    fn comment(language: u8, text: &str) -> Vec<u8> {
        [&[0, language][..], text.as_bytes()].concat()
    }

    #[test]
    fn a_comment_in_several_languages_keeps_english_and_any_language() {
        let mut game = Game::new(Headers::default(), None);
        let record = record_with(&[
            (-1, TYPE_COMMENT_AFTER, comment(53, "Aljechin-Verteidigung\r\n")),
            (-1, TYPE_COMMENT_AFTER, comment(LANGUAGE_ENGLISH, "Alekhine Defence\r\n")),
            (-1, TYPE_COMMENT_AFTER, comment(LANGUAGE_ANY, "B02")),
            (-1, TYPE_COMMENT_BEFORE, comment(49, "Défense Alekhine")),
            (-1, TYPE_COMMENT_BEFORE, comment(43, "Defensa Alekhine")),
        ]);
        apply_annotations(&mut game, &[], &record).unwrap();
        assert_eq!(game.root().comment_after, "Alekhine Defence B02");
        // No English: the first language.
        assert_eq!(game.root().comment_before, "Défense Alekhine");
    }

    #[test]
    fn annotations_before_a_bad_one_are_kept() {
        let mut game = Game::new(Headers::default(), None);
        let mut record = record_with(&[(-1, TYPE_COMMENT_AFTER, comment(0, "Kept"))]);
        record.extend_from_slice(&[0xff, 0xff, 0xff, TYPE_COMMENT_AFTER, 0, 2]); // size 2 < 6
        let total = record.len() as u32;
        record[0x0a..0x0e].copy_from_slice(&total.to_be_bytes());
        assert!(apply_annotations(&mut game, &[], &record).is_err());
        assert_eq!(game.root().comment_after, "Kept");
    }

    #[test]
    fn symbols_become_nags() {
        let mut game = Game::new(Headers::default(), None);
        let mv = chess_core::position::find_move(
            &Chess::default(),
            chess_core::shakmaty::Square::E2,
            chess_core::shakmaty::Square::E4,
            None,
        )
        .unwrap();
        let child = game.add_child(NodeId::ROOT, mv);
        let order = vec![Some(child)];
        let record = record_with(&[(0, TYPE_SYMBOLS, vec![1, 14, 0])]);
        apply_annotations(&mut game, &order, &record).unwrap();
        assert_eq!(game.node(child).nags, vec![Nag(1), Nag(14)]);
    }
}
