//! IO-03 on real ChessBase databases. They aren't committed (they ship with
//! ChessBase), so these tests are ignored and skip themselves when the data
//! is missing:
//!
//! ```sh
//! # every .cbh in a folder, e.g. the Testsets and Training databases
//! CBH_SAMPLES=path/to/folder cargo test --release -p chess-db --test cb_import -- --ignored samples
//! # the Mega Database (7.6M games, a few minutes)
//! CBH_MEGA="path/to/Mega Database 2019.cbh" cargo test --release -p chess-db --test cb_import -- --ignored mega
//! ```
//!
//! Without the variables they read `testdata/large/cbh-samples/` and
//! `testdata/large/Mega Database 2019.cbh` (git-ignored; symlinks are fine).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chess_db::cb::header::{CbEntry, CbhFile};
use chess_db::cb::moves::{CbgFile, TranslateTable, decode_record};
use chess_db::{Database, NoProgress, import_cb};

fn large() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/large")
}

fn data_path(var: &str, default: PathBuf) -> Option<PathBuf> {
    let path = std::env::var_os(var).map(PathBuf::from).unwrap_or(default);
    if path.exists() {
        Some(path)
    } else {
        eprintln!("skipping: {} not found (set {var})", path.display());
        None
    }
}

/// The games (not texts, not deleted) in a `.cbh`.
fn games(cbh_path: &Path) -> Vec<chess_db::cb::header::GameRecord> {
    let cbh = CbhFile::open(cbh_path).unwrap();
    (1..=cbh.game_count)
        .filter_map(|id| match cbh.read(id).unwrap() {
            CbEntry::Game(g) if !g.deleted => Some(g),
            _ => None,
        })
        .collect()
}

#[test]
#[ignore]
fn samples_import_every_game_in_full() {
    let Some(dir) = data_path("CBH_SAMPLES", large().join("cbh-samples")) else { return };
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cbh")))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no .cbh files in {}", dir.display());
    let tmp = tempfile::tempdir().unwrap();
    for path in paths {
        let expected = games(&path);
        // Every game decodes with every move legal, to the length `.cbh`
        // declares: what tells the right table and decoder from a lucky one.
        let cbg = CbgFile::open(&path.with_extension("cbg")).unwrap();
        for g in &expected {
            let decoded = decode_record(&cbg.read_record(g.moves_offset).unwrap(), &TranslateTable::mode0())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(decoded.problems.is_empty(), "{}: {:?}", path.display(), decoded.problems);
            assert!(g.main_line_matches(&decoded.game), "{}: main line length", path.display());
        }

        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let db = Mutex::new(Database::create(&tmp.path().join(format!("{name}.chessdb"))).unwrap());
        let summary = import_cb(&db, &path, &NoProgress).unwrap();
        assert_eq!(
            (summary.imported, summary.skipped, summary.warning_count),
            (expected.len() as u64, 0, 0),
            "{name}: {:?}",
            summary.warnings
        );
        let db = db.lock().unwrap();
        assert_eq!(db.count() as usize, expected.len());
        if expected.iter().any(|g| g.annotations_offset != 0) {
            let annotated = (0..db.count()).any(|id| {
                let game = db.read_game(id).unwrap();
                game.preorder().iter().any(|&n| {
                    let node = game.node(n);
                    !node.comment_after.is_empty() || !node.nags.is_empty() || !node.shapes.is_empty()
                })
            });
            assert!(annotated, "{name}: its annotations didn't come through");
        }
    }
}

#[test]
#[ignore]
fn mega_database_imports() {
    let Some(path) = data_path("CBH_MEGA", large().join("Mega Database 2019.cbh")) else { return };
    let expected = games(&path).len() as u64;
    // Not the tmpfs `/tmp`: the result is several GB.
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let db = Mutex::new(Database::create(&tmp.path().join("Mega.chessdb")).unwrap());
    let t = std::time::Instant::now();
    let summary = import_cb(&db, &path, &NoProgress).unwrap();
    eprintln!(
        "imported {} of {expected} games in {:.0} s; {} skipped, {} warnings",
        summary.imported,
        t.elapsed().as_secs_f64(),
        summary.skipped,
        summary.warning_count
    );
    assert_eq!(summary.imported + summary.skipped, expected);
    // Mega Database 2019 skips 166 (Chess960 and other encoding modes) and
    // cuts 1,776 lines (null moves, one truncated game); leave some slack
    // for other editions.
    assert!(summary.skipped * 10_000 < expected, "skipped {}", summary.skipped);
    assert!(summary.warning_count * 1_000 < expected, "{} warnings", summary.warning_count);
}
