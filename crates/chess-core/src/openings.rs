//! Opening names and ECO codes (OPEN-11) from lichess-org/chess-openings
//! (CC0), embedded in `data/openings.tsv`.
//!
//! Openings are keyed by the Polyglot hash of their final position, so a
//! game is classified by position, whatever the move order. A game gets the
//! deepest named position its main line reaches.

use std::collections::HashMap;
use std::sync::OnceLock;

use shakmaty::san::San;
use shakmaty::{Chess, Position};

use crate::game::{Game, NodeId};
use crate::position;

const DATA: &str = include_str!("../data/openings.tsv");

/// Plies of a game's main line searched for a named position.
pub const CLASSIFY_PLIES: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opening {
    /// Stable within one version of the data (see [`data_version`]); 1-based.
    pub id: u16,
    pub eco: &'static str,
    pub name: &'static str,
    /// Plies from the start position in the reference line.
    pub plies: u16,
}

struct Table {
    openings: Vec<Opening>,
    by_key: HashMap<u64, u16>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut openings = Vec::new();
        let mut by_key = HashMap::new();
        for line in DATA.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let mut cols = line.split('\t');
            let (Some(eco), Some(name), Some(pgn)) = (cols.next(), cols.next(), cols.next()) else {
                continue;
            };
            let mut pos = Chess::default();
            let mut plies = 0u16;
            let mut ok = true;
            for tok in pgn.split_whitespace().filter(|t| !t.ends_with('.')) {
                match tok.parse::<San>().ok().and_then(|san| san.to_move(&pos).ok()) {
                    Some(m) => {
                        pos.play_unchecked(m);
                        plies += 1;
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            let id = openings.len() as u16 + 1;
            openings.push(Opening { id, eco, name, plies });
            // The first (more general) name wins for a repeated position.
            by_key.entry(position::hash(&pos)).or_insert(id);
        }
        Table { openings, by_key }
    })
}

/// Changes whenever the embedded data does; derived files that store
/// opening ids record it.
pub fn data_version() -> u32 {
    // FNV-1a over the data.
    DATA.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

pub fn count() -> usize {
    table().openings.len()
}

pub fn by_id(id: u16) -> Option<&'static Opening> {
    id.checked_sub(1).and_then(|i| table().openings.get(i as usize))
}

/// The opening named exactly at the position with this Polyglot hash.
pub fn by_key(key: u64) -> Option<&'static Opening> {
    table().by_key.get(&key).and_then(|&id| by_id(id))
}

pub fn at_position(pos: &Chess) -> Option<&'static Opening> {
    by_key(position::hash(pos))
}

/// Keeps the deepest named position seen while walking a line.
#[derive(Debug, Default, Clone, Copy)]
pub struct Classifier {
    best: Option<&'static Opening>,
}

impl Classifier {
    pub fn see(&mut self, key: u64) {
        if let Some(o) = by_key(key) {
            self.best = Some(o);
        }
    }

    pub fn result(self) -> Option<&'static Opening> {
        self.best
    }
}

/// The opening of a game: the last named position among the first
/// [`CLASSIFY_PLIES`] plies of its main line.
pub fn classify(game: &Game) -> Option<&'static Opening> {
    let mut pos = game.start_position();
    let mut classifier = Classifier::default();
    classifier.see(position::hash(&pos));
    let mut cur = NodeId::ROOT;
    for _ in 0..CLASSIFY_PLIES {
        let Some(&next) = game.node(cur).children.first() else { break };
        pos.play_unchecked(game.node(next).mv.expect("non-root node has a move"));
        classifier.see(position::hash(&pos));
        cur = next;
    }
    classifier.result()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgn::read_pgn_str;

    #[test]
    fn all_lines_parse() {
        let lines = DATA.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).count();
        assert_eq!(count(), lines, "every opening line is legal");
        assert!(count() > 3000);
        assert_eq!(by_id(1).map(|o| o.eco), Some("A00"));
        assert!(by_id(0).is_none());
    }

    #[test]
    fn classifies_by_position_and_keeps_the_deepest_name() {
        let najdorf =
            read_pgn_str("1. e4 c5 2. Nf3 d6 3. d4 cxd4 4. Nxd4 Nf6 5. Nc3 a6 6. Be3 e5 *").remove(0);
        let o = classify(&najdorf).unwrap();
        assert!(o.eco.starts_with("B9"), "{o:?}");
        assert!(o.name.starts_with("Sicilian Defense: Najdorf Variation"), "{o:?}");

        // Queen's Gambit Declined reached by a different move order.
        let qgd = read_pgn_str("1. c4 e6 2. d4 d5 *").remove(0);
        let o = classify(&qgd).unwrap();
        assert!(o.name.starts_with("Queen's Gambit Declined"), "{o:?}");

        assert!(classify(&read_pgn_str("*").remove(0)).is_none());
        assert_eq!(at_position(&Chess::default()), None);
    }
}
