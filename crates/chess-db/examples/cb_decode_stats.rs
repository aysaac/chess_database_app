//! Dev tool (not shipped): decodes the moves of every `step`-th game in a
//! `.cbh` database with the built-in table and tallies the outcomes, so
//! decoder gaps show up as counts by message rather than one failure at a
//! time.
//!
//! A game counts as read in full only when every move in it was read and
//! its main line has the number of full moves `.cbh` declares.
//!
//! Usage: cargo run --release -p chess-db --example cb_decode_stats -- DB_BASE [STEP | show ID]
//! (DB_BASE is the database's path without extension; `show ID` prints one
//! game's moves as PGN instead.)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chess_db::cb::header::{CbEntry, CbhFile};
use chess_db::cb::moves::{CbgFile, TranslateTable, decode_record};

fn sibling(base: &Path, ext: &str) -> PathBuf {
    let mut name = base.as_os_str().to_owned();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

/// The error message with its numbers and squares blanked, so games that
/// fail the same way land in the same bucket.
fn bucket(message: &str) -> String {
    let mut out = String::new();
    for word in message.split(' ') {
        if !out.is_empty() {
            out.push(' ');
        }
        let squareish = word.len() >= 2 && word.chars().any(|c| c.is_ascii_digit());
        out.push_str(if squareish { "_" } else { word });
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: cb_decode_stats DB_BASE [STEP]");
        std::process::exit(2);
    }
    let table = TranslateTable::mode0();
    let base = PathBuf::from(&args[1]);
    let cbh = CbhFile::open(&sibling(&base, "cbh")).expect("open .cbh");
    let cbg = CbgFile::open(&sibling(&base, "cbg")).expect("open .cbg");
    if args.get(2).is_some_and(|a| a == "show") {
        let id: u32 = args.get(3).and_then(|s| s.parse().ok()).expect("show takes a game id");
        let Ok(CbEntry::Game(g)) = cbh.read(id) else { panic!("game {id} isn't a game") };
        println!("declared: {} moves", g.main_line_moves);
        let decoded = decode_record(&cbg.read_record(g.moves_offset).expect("read"), &table).expect("decode");
        println!("{}\nproblems: {:?}", chess_core::pgn::write_movetext(&decoded.game), decoded.problems);
        return;
    }
    let step: u32 = args.get(2).map_or(1, |s| s.parse().expect("STEP is a number"));

    let mut ok = 0u32;
    let mut total = 0u32;
    let mut failures: BTreeMap<String, (u32, Vec<u32>)> = BTreeMap::new();
    let mut fail = |key: String, id: u32| {
        let entry = failures.entry(key).or_default();
        entry.0 += 1;
        if entry.1.len() < 5 {
            entry.1.push(id);
        }
    };
    let mut id = 1;
    while id <= cbh.game_count {
        let this = id;
        id += step;
        let g = match cbh.read(this) {
            Ok(CbEntry::Game(g)) if !g.deleted => g,
            _ => continue,
        };
        total += 1;
        let record = match cbg.read_record(g.moves_offset) {
            Ok(r) => r,
            Err(e) => {
                fail(format!("read: {}", bucket(&e.to_string())), this);
                continue;
            }
        };
        let game = match decode_record(&record, &table) {
            Ok(decoded) if decoded.problems.is_empty() => decoded.game,
            Ok(decoded) => {
                for problem in &decoded.problems {
                    fail(format!("kept, cut short: {}", bucket(problem)), this);
                }
                continue;
            }
            Err(e) => {
                fail(format!("skipped: {}", bucket(&e.to_string())), this);
                continue;
            }
        };
        if g.main_line_matches(&game) {
            ok += 1;
        } else {
            let plies = game.mainline_len();
            fail(
                format!(
                    "main line length differs (declared {} moves, decoded {plies} plies)",
                    g.main_line_moves
                ),
                this,
            );
        }
    }

    println!("{ok}/{total} games read in full, to their declared length");
    let mut sorted: Vec<_> = failures.into_iter().collect();
    sorted.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    for (key, (count, ids)) in sorted.iter().take(40) {
        println!("{count:>8}  {key}  e.g. {ids:?}");
    }
}
