//! Dev tool (not a product CLI, not shipped): finds candidate 256-byte
//! move-translation tables in a directory of binaries (a licensed ChessBase
//! install) and validates each one against real `.cbh` databases by trying
//! to decode every game in them — a wrong table fails almost immediately
//! (an illegal move or a corrupt read), so a table that decodes every game
//! in every test database is essentially certainly the right one.
//!
//! See `docs/io-03-cbh.md` §3.2 and the "Translate table" decision: this is
//! the tool that turns "how do we get this constant" into "extract it from
//! software you're licensed to run," not from a third party's copy.
//!
//! Usage: cargo run -p chess-db --example extract_cb_key -- BINARY_DIR DB_BASE [DB_BASE...]
//! (DB_BASE is a `.cbh` database's shared path without extension, e.g.
//! `testdata/cbh/nunn`.)

use std::path::{Path, PathBuf};

use chess_db::cb::header::{CbEntry, CbhFile};
use chess_db::cb::moves::{CbgFile, TranslateTable, decode_record};

fn find_permutation_tables(data: &[u8]) -> Vec<usize> {
    if data.len() < 256 {
        return Vec::new();
    }
    let mut counts = [0u32; 256];
    let mut distinct = 0u32;
    for &b in &data[0..256] {
        let c = &mut counts[b as usize];
        if *c == 0 {
            distinct += 1;
        }
        *c += 1;
    }
    let mut hits = Vec::new();
    if distinct == 256 {
        hits.push(0);
    }
    for i in 1..=data.len() - 256 {
        let out = data[i - 1] as usize;
        counts[out] -= 1;
        if counts[out] == 0 {
            distinct -= 1;
        }
        let inn = data[i + 255] as usize;
        if counts[inn] == 0 {
            distinct += 1;
        }
        counts[inn] += 1;
        if distinct == 256 {
            hits.push(i);
        }
    }
    hits
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Tries to decode every non-deleted game in `base` with `table`. A game
/// only counts as decoded correctly if every move in it is legal and its
/// main line has exactly the number of full moves `.cbh` itself declares
/// (`main_line_moves`) — "decoded without error" alone is too easy to
/// satisfy by chance on short games, so this cross-check against an
/// independent field is what makes a match mean something. Returns (games
/// matching, total games attempted).
fn validate(base: &Path, table: &TranslateTable) -> (u32, u32) {
    let (Ok(cbh), Ok(cbg)) = (CbhFile::open(&sibling(base, "cbh")), CbgFile::open(&sibling(base, "cbg")))
    else {
        return (0, 0);
    };
    let mut ok = 0;
    let mut total = 0;
    for id in 1..=cbh.game_count {
        let g = match cbh.read(id) {
            Ok(CbEntry::Game(g)) if !g.deleted => g,
            _ => continue,
        };
        total += 1;
        let Ok(record) = cbg.read_record(g.moves_offset) else { continue };
        let Ok(decoded) = decode_record(&record, table) else { continue };
        if !decoded.problems.is_empty() {
            continue;
        }
        let game = decoded.game;
        if g.main_line_matches(&game) {
            ok += 1;
        }
    }
    (ok, total)
}

fn sibling(base: &Path, ext: &str) -> PathBuf {
    let mut name = base.as_os_str().to_owned();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: extract_cb_key BINARY_DIR DB_BASE [DB_BASE...]");
        std::process::exit(2);
    }
    let binary_dir = PathBuf::from(&args[1]);
    let db_bases: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();

    let mut files = Vec::new();
    walk_files(&binary_dir, &mut files);
    files.sort();
    println!("scanning {} files under {}", files.len(), binary_dir.display());

    let mut candidates: Vec<(PathBuf, usize, [u8; 256])> = Vec::new();
    for file in &files {
        let Ok(data) = std::fs::read(file) else { continue };
        let hits = find_permutation_tables(&data);
        if !hits.is_empty() {
            println!("  {}: {} permutation table(s)", file.display(), hits.len());
        }
        for offset in hits {
            let mut table = [0u8; 256];
            table.copy_from_slice(&data[offset..offset + 256]);
            candidates.push((file.clone(), offset, table));
        }
    }
    println!("total candidates: {}", candidates.len());

    let mut grand_total = 0;
    for base in &db_bases {
        let (_, total) = validate(base, &TranslateTable::identity());
        grand_total += total;
    }
    println!("grand total games across all databases: {grand_total}");

    let mut best: Option<(u32, &Path, usize)> = None;
    let mut perfect = 0;
    for (file, offset, bytes) in &candidates {
        let table = TranslateTable(*bytes);
        let mut ok_sum = 0;
        for base in &db_bases {
            let (ok, _) = validate(base, &table);
            ok_sum += ok;
        }
        if ok_sum == grand_total {
            perfect += 1;
            println!("MATCH: {} @ {offset:#x} decodes all {grand_total} games", file.display());
            println!("  bytes: {bytes:02x?}");
        }
        if best.is_none_or(|(b, ..)| ok_sum > b) {
            best = Some((ok_sum, file, *offset));
        }
    }
    if perfect == 0 {
        let (score, file, offset) = best.unwrap();
        println!(
            "no perfect candidate; best is {score}/{grand_total} games: {} @ {offset:#x}",
            file.display()
        );
    }
}
