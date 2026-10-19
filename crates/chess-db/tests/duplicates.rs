//! Duplicate games (DB-08) end to end: importing a file twice, copies from
//! another source, deleting the copies, restoring them and compacting.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use chess_core::pgn::{read_pgn_str, write_game};
use chess_db::{
    Database, DuplicateOptions, Duplicates, GameId, NoProgress, export_pgn, find_duplicates, import_pgn,
};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn")
}

const FIXTURES: [&str; 4] = ["variations.pgn", "setup.pgn", "lichess.pgn", "reference.pgn"];

fn find(db: &Mutex<Database>, fuzzy: bool) -> Duplicates {
    find_duplicates(db, DuplicateOptions { fuzzy }, &NoProgress).unwrap()
}

fn import_fixtures(db: &Mutex<Database>) -> u32 {
    for f in FIXTURES {
        import_pgn(db, &fixtures().join(f), &NoProgress).unwrap();
    }
    db.lock().unwrap().count()
}

#[test]
fn a_file_imported_twice_is_found_deleted_restored_and_compacted_away() {
    let dir = tempfile::tempdir().unwrap();
    let db = Mutex::new(Database::create(&dir.path().join("Twice")).unwrap());
    let n = import_fixtures(&db);
    assert!(n > 50);
    assert_eq!(find(&db, true).groups, [], "the fixtures have no copies");

    // The same games again, through an export (lossless) and as the files.
    let exported = dir.path().join("all.pgn");
    export_pgn(&db, None, &exported, &NoProgress).unwrap();
    import_pgn(&db, &exported, &NoProgress).unwrap();
    assert_eq!(db.lock().unwrap().count(), 2 * n);

    // Every game is found but one: two plies, between club players, without
    // a round (one move says nothing: they could have played it twice).
    let thin: Vec<GameId> = (0..n).filter(|&id| db.lock().unwrap().record(id).unwrap().plies < 3).collect();
    assert_eq!(thin.len(), 1);
    let copied: Vec<GameId> = (0..n).filter(|id| !thin.contains(id)).collect();
    for fuzzy in [false, true] {
        let found = find(&db, fuzzy);
        assert_eq!(found.games, 2 * n);
        assert_eq!(found.groups.len(), copied.len(), "fuzzy: {fuzzy}");
        for (g, &id) in found.groups.iter().zip(&copied) {
            // Copies look alike: the first one is kept.
            assert_eq!((g.keep, g.remove.as_slice(), g.exact), (id, [id + n].as_slice(), true));
        }
        assert_eq!(found.to_remove(), copied.iter().map(|id| id + n).collect::<Vec<GameId>>());
    }

    // Deleting them leaves no copies; restoring brings them back.
    let found = find(&db, false);
    let ids = found.to_remove();
    let m = ids.len() as u32;
    assert_eq!(db.lock().unwrap().set_deleted_many(&ids, true).unwrap(), m);
    assert_eq!(find(&db, true).groups, []);
    assert_eq!(db.lock().unwrap().set_deleted_many(&ids, false).unwrap(), m);
    assert_eq!(find(&db, false).groups, found.groups);

    // Compacting removes them for good.
    db.lock().unwrap().set_deleted_many(&ids, true).unwrap();
    let summary = db.lock().unwrap().compact(&NoProgress).unwrap();
    assert_eq!((summary.kept, summary.removed), (2 * n - m, m));
    assert_eq!(find(&db, true).groups, []);
}

/// `testdata/pgn/duplicates.pgn`: an annotated game (0) with a copy without
/// the ratings (1) and one from another source that stops early (2);
/// another game of the same players (3); a game with two exact copies
/// (4–6); two online games with the same moves on their own pages (7, 8).
#[test]
fn the_duplicates_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let db = Mutex::new(Database::create(&dir.path().join("Fixture")).unwrap());
    let summary = import_pgn(&db, &fixtures().join("duplicates.pgn"), &NoProgress).unwrap();
    assert_eq!((summary.imported, summary.warning_count), (9, 0), "{:?}", summary.warnings);
    let groups =
        |fuzzy| find(&db, fuzzy).groups.into_iter().map(|g| (g.keep, g.remove, g.exact)).collect::<Vec<_>>();
    assert_eq!(groups(false), [(4, vec![5, 6], true)]);
    assert_eq!(groups(true), [(0, vec![1, 2], false), (4, vec![5, 6], true)]);
}

#[test]
fn copies_from_another_source_are_fuzzy_and_the_annotated_one_stays() {
    let dir = tempfile::tempdir().unwrap();
    let db = Mutex::new(Database::create(&dir.path().join("Sources")).unwrap());
    import_pgn(&db, &fixtures().join("variations.pgn"), &NoProgress).unwrap();
    // Kasparov–Topalov, annotated, as game 0. Another source has it with
    // other names and event, no annotations, stopping at move 30.
    let original = db.lock().unwrap().read_game(0).unwrap();
    assert!(original.has_comments() && original.has_variations());
    let mut other = read_pgn_str(&write_game(&original)).remove(0);
    other.headers.white = "Kasparov, G.".into();
    other.headers.black = "Veselin Topalov".into();
    other.headers.event = "Hoogovens".into();
    other.headers.annotator.clear();
    let pgn = write_game(&other);
    let plain = read_pgn_str(&strip_annotations_and_cut(&pgn, 30)).remove(0);
    assert!(!plain.has_comments() && plain.mainline_len() == 60);
    let copy = db.lock().unwrap().append(&plain).unwrap();

    assert_eq!(find(&db, false).groups, [], "not an exact copy");
    let found = find(&db, true);
    assert_eq!(found.groups.len(), 1);
    let g = &found.groups[0];
    assert_eq!((g.keep, g.remove.as_slice(), g.exact), (0, [copy].as_slice(), false));
}

/// The movetext of `pgn` without comments, variations or symbols, up to
/// White's move `last` (inclusive).
fn strip_annotations_and_cut(pgn: &str, last: u32) -> String {
    let (headers, moves) = pgn.split_once("\n\n").unwrap();
    let mut out = String::new();
    let (mut depth, mut comment) = (0, false);
    for c in moves.chars() {
        match c {
            '{' => comment = true,
            '}' => comment = false,
            '(' if !comment => depth += 1,
            ')' if !comment => depth -= 1,
            _ if comment || depth > 0 => {}
            c => out.push(c),
        }
    }
    let words: Vec<&str> =
        out.split_whitespace().filter(|w| !w.starts_with('$') && !w.ends_with("...")).collect();
    let mut kept = Vec::new();
    for w in words {
        if w == format!("{}.", last + 1) {
            break;
        }
        kept.push(w);
    }
    format!("{headers}\n\n{} *\n", kept.join(" "))
}

fn peak_rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// A big file imported twice: every game of the second import is found as
/// a copy, and it stays quick. `cargo test --release -p chess-db -- --ignored`
/// (`LARGE_PGN=path` picks another file, such as the 1M Lichess games).
#[test]
#[ignore]
fn large_file_imported_twice() {
    let path = std::env::var_os("LARGE_PGN")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixtures().join("../large/twic1600.pgn"));
    if !path.exists() {
        eprintln!("skipping: {} not found", path.display());
        return;
    }
    let dir = std::env::var_os("DUPLICATES_DIR").map(PathBuf::from);
    let tmp = tempfile::tempdir_in(dir.as_deref().unwrap_or(&std::env::temp_dir())).unwrap();
    let db = Mutex::new(Database::create(&tmp.path().join("Twice")).unwrap());
    let t = Instant::now();
    import_pgn(&db, &path, &NoProgress).unwrap();
    let n = db.lock().unwrap().count();
    eprintln!("imported {n} games in {:.1}s", t.elapsed().as_secs_f64());
    time_both(&db, "once");
    import_pgn(&db, Path::new(&path), &NoProgress).unwrap();
    for found in time_both(&db, "twice") {
        let removable = found.to_remove();
        // Copies within the file itself are found too.
        assert!(removable.len() >= n as usize);
        let second: Vec<GameId> = removable.iter().copied().filter(|&id| id >= n).collect();
        assert_eq!(second.len(), n as usize, "every game of the second import is a copy");
    }
}

fn time_both(db: &Mutex<Database>, label: &str) -> [Duplicates; 2] {
    [false, true].map(|fuzzy| {
        let t = Instant::now();
        let found = find(db, fuzzy);
        eprintln!(
            "{label}: {} {} games: {} groups, {} to delete, {} compared, in {:.2}s (peak RSS {:.0} MB)",
            if fuzzy { "fuzzy" } else { "exact" },
            found.games,
            found.groups.len(),
            found.removable(),
            found.compared,
            t.elapsed().as_secs_f64(),
            peak_rss_mb().unwrap_or(0.0)
        );
        found
    })
}

/// `n` games of one pair of engines that all start with the same 10 plies
/// and go on at random for 40 plies, without a date.
fn engine_match(n: usize, seed: u64) -> Vec<chess_db::PreparedGame> {
    use chess_core::game::{Game, Headers, NodeId};
    use chess_core::shakmaty::san::San;
    use chess_core::shakmaty::{Chess, Position};

    let head = ["e4", "c5", "Nf3", "d6", "d4", "cxd4", "Nxd4", "Nf6", "Nc3", "a6"];
    let mut seed = seed;
    (0..n)
        .map(|_| {
            let headers = Headers {
                white: "Stockfish 16 64-bit".into(),
                black: "Komodo 14 64-bit".into(),
                event: "Engine match".into(),
                ..Headers::default()
            };
            let mut game = Game::new(headers, None);
            let mut pos = Chess::default();
            let mut moves = Vec::new();
            for san in head {
                let m = san.parse::<San>().unwrap().to_move(&pos).unwrap();
                pos.play_unchecked(m);
                moves.push(m);
            }
            for _ in 0..40 {
                let legal = pos.legal_moves();
                if legal.is_empty() {
                    break;
                }
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let m = legal[((seed >> 33) as usize) % legal.len()];
                pos.play_unchecked(m);
                moves.push(m);
            }
            game.add_line(NodeId::ROOT, &moves);
            chess_db::PreparedGame::new(&game)
        })
        .collect()
}

/// R1-9 (review of DB-08): one players' bucket of many games that share
/// their first moves stays quick (it was quadratic: 100,000 games took
/// 24 s), and Stop is heard while they're compared.
/// `DUPLICATES_N=100000 cargo test --release -p chess-db --test duplicates -- --ignored big_bucket --nocapture`
#[test]
#[ignore]
fn a_big_bucket_of_games_that_start_alike() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let n: usize = std::env::var("DUPLICATES_N").ok().and_then(|v| v.parse().ok()).unwrap_or(100_000);
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(&dir.path().join("Bucket")).unwrap();
    let games = engine_match(n, 42);
    db.append_prepared(&games).unwrap();
    db.append_prepared(&games[..10]).unwrap();
    let db = Mutex::new(db);
    for found in time_both(&db, "one bucket") {
        assert_eq!(found.removable(), 10);
    }

    /// Stop pressed as soon as the comparisons begin.
    struct Stop(AtomicBool);
    impl chess_db::Progress for Stop {
        fn report(&self, done: u64, total: u64) {
            if total == 1000 && done >= 950 {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }
    let t = Instant::now();
    let result = find_duplicates(&db, DuplicateOptions { fuzzy: true }, &Stop(AtomicBool::new(false)));
    eprintln!("stopped: {:?} in {:.2}s", result.as_ref().err(), t.elapsed().as_secs_f64());
    assert!(matches!(result, Err(chess_db::DbError::Cancelled)));
}
