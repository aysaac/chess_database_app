//! Storage-level integration tests: crash recovery, PGN round trips and
//! blob codec properties.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chess_core::game::{Game, Nag, NodeId};
use chess_core::pgn::{read_pgn_str, write_game};
use chess_core::shakmaty::Position;
use chess_db::blob::{decode_game, encode_game};
use chess_db::{Database, NoProgress, copy_games, export_pgn, import_pgn};
use proptest::prelude::*;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn")
}

fn append_garbage(path: &Path, bytes: &[u8]) {
    OpenOptions::new().append(true).open(path).unwrap().write_all(bytes).unwrap();
}

#[test]
fn recovers_from_interrupted_commit() {
    let dir = tempfile::tempdir().unwrap();
    let games = read_pgn_str(&std::fs::read_to_string(fixtures().join("variations.pgn")).unwrap());
    let path = {
        let mut db = Database::create(&dir.path().join("Crash")).unwrap();
        for g in &games {
            db.append(g).unwrap();
        }
        db.path().to_path_buf()
    };

    // Simulate a crash in the middle of the next commit: blob bytes, a torn
    // name entry and index records were written, but the count wasn't bumped.
    append_garbage(&path.join("games.blob"), &[0xAB; 1000]);
    append_garbage(&path.join("players.names"), &[40, b'P', b'a']);
    append_garbage(&path.join("games.idx"), &[0x11; 64 * 3 + 17]);

    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.count(), games.len() as u32);
    for (i, g) in games.iter().enumerate() {
        assert_eq!(&db.read_game(i as u32).unwrap(), g);
    }
    // The database keeps working after recovery.
    let id = db.append(&games[0]).unwrap();
    assert_eq!(id, games.len() as u32);
    drop(db);
    let db = Database::open(&path).unwrap();
    assert_eq!(db.read_game(id).unwrap(), games[0]);
    assert_eq!(db.row(id).unwrap().white, "Kasparov, Garry");
}

#[test]
fn detects_count_larger_than_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = {
        let mut db = Database::create(&dir.path().join("Short")).unwrap();
        db.append(&read_pgn_str("1. e4 *")[0]).unwrap();
        db.path().to_path_buf()
    };
    let idx = path.join("games.idx");
    let len = std::fs::metadata(&idx).unwrap().len();
    OpenOptions::new().write(true).open(&idx).unwrap().set_len(len - 10).unwrap();
    assert!(Database::open(&path).is_err());
}

#[test]
fn import_export_reimport_is_lossless() {
    let dir = tempfile::tempdir().unwrap();
    let first = Mutex::new(Database::create(&dir.path().join("First")).unwrap());
    for f in ["variations.pgn", "setup.pgn", "lichess.pgn", "latin1-crlf.pgn", "malformed.pgn"] {
        import_pgn(&first, &fixtures().join(f), &NoProgress).unwrap();
    }
    let exported = dir.path().join("out.pgn");
    let written = export_pgn(&first, None, &exported, &NoProgress).unwrap();
    let count = first.lock().unwrap().count();
    assert_eq!(written, u64::from(count));

    let second = Mutex::new(Database::create(&dir.path().join("Second")).unwrap());
    let summary = import_pgn(&second, &exported, &NoProgress).unwrap();
    assert_eq!(summary.imported, u64::from(count));
    assert_eq!(summary.warning_count, 0);

    let (a, b) = (first.lock().unwrap(), second.lock().unwrap());
    for id in 0..count {
        let (ga, gb) = (a.read_game(id).unwrap(), b.read_game(id).unwrap());
        assert_eq!(ga, gb, "game {id}");
        assert_eq!(encode_game(&ga), encode_game(&gb), "blob of game {id}");
    }
}

/// Fidelity check on real-world files downloaded by
/// `scripts/fetch-testdata.sh`. Run with
/// `cargo test --release -p chess-db -- --ignored`
/// (`LARGE_PGN=path` picks another file).
#[test]
#[ignore]
fn large_file_round_trip() {
    let path = std::env::var_os("LARGE_PGN")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixtures().join("../large/twic1600.pgn"));
    if !path.exists() {
        eprintln!("skipping: {} not found", path.display());
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let first = Mutex::new(Database::create(&dir.path().join("First")).unwrap());
    let summary = import_pgn(&first, &path, &NoProgress).unwrap();
    let exported = dir.path().join("out.pgn");
    export_pgn(&first, None, &exported, &NoProgress).unwrap();
    let second = Mutex::new(Database::create(&dir.path().join("Second")).unwrap());
    let again = import_pgn(&second, &exported, &NoProgress).unwrap();
    assert_eq!(again.imported, summary.imported);
    assert_eq!(again.warning_count, 0);
    let (a, b) = (first.lock().unwrap(), second.lock().unwrap());
    for id in 0..a.count() {
        let (ga, gb) = (a.read_game(id).unwrap(), b.read_game(id).unwrap());
        assert_eq!(encode_game(&ga), encode_game(&gb), "game {id}");
        assert_eq!(ga, gb, "game {id}");
    }
    eprintln!("{} games round-tripped losslessly", a.count());
}

#[test]
fn export_skips_deleted_games_and_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = Mutex::new(Database::create(&dir.path().join("Del")).unwrap());
    import_pgn(&db, &fixtures().join("setup.pgn"), &NoProgress).unwrap();
    db.lock().unwrap().set_deleted(1, true).unwrap();
    let out = dir.path().join("out.pgn");
    assert_eq!(export_pgn(&db, None, &out, &NoProgress).unwrap(), 2);
    assert_eq!(read_pgn_str(&std::fs::read_to_string(&out).unwrap()).len(), 2);
    assert!(!dir.path().join("out.pgn.part").exists());
}

fn random_game(choices: &[(u8, u8, u8)]) -> Game {
    let mut game = Game::default();
    let mut cursor = NodeId::ROOT;
    for &(action, pick, extra) in choices {
        match action % 5 {
            0..=2 => {
                let moves = game.position_at(cursor).legal_moves();
                if moves.is_empty() {
                    continue;
                }
                let m = moves[pick as usize % moves.len()];
                cursor = game.find_child(cursor, m).unwrap_or_else(|| game.add_child(cursor, m));
            }
            3 => {
                for _ in 0..pick % 4 {
                    cursor = game.node(cursor).parent.unwrap_or(cursor);
                }
            }
            _ => {
                let node = game.node_mut(cursor);
                match extra % 3 {
                    0 => node.comment_after = format!("c{pick}"),
                    1 => node.nags.push(Nag(pick)),
                    _ => node.commands.push(("clk".into(), format!("0:0{}", pick % 10))),
                }
            }
        }
    }
    game
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn blob_codec_round_trips(choices in prop::collection::vec(any::<(u8, u8, u8)>(), 0..200)) {
        let game = random_game(&choices);
        let decoded = decode_game(&encode_game(&game)).unwrap();
        prop_assert_eq!(&decoded, &game, "\n{}", write_game(&game));
    }
}

proptest! {
    // Each case creates two databases (with fsyncs), so fewer cases.
    #![proptest_config(ProptestConfig::with_cases(40))]

    /// Copying games between databases (DB-06) reuses the stored bytes and
    /// renames nothing: the copies read back exactly like the originals.
    #[test]
    fn copied_games_read_back_identically(
        games in prop::collection::vec(
            (prop::collection::vec(any::<(u8, u8, u8)>(), 0..60), any::<(u8, u8, u16, bool)>()),
            1..6,
        ),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let source = Mutex::new(Database::create(&dir.path().join("From")).unwrap());
        let target = Mutex::new(Database::create(&dir.path().join("To")).unwrap());
        let dates = ["", "2024.05.06", "1999.??.??", "????.12.??", "bad date"];
        let rounds = ["", "1", "3.2", "?", "-", "final"];
        let ecos = ["", "B90", "A00", "E99", "Z5"];
        for (choices, (a, b, elo, extra)) in &games {
            let mut game = random_game(choices);
            let h = &mut game.headers;
            h.white = format!("P{}", a % 3);
            h.black = format!("P{}", b % 4);
            h.event = if a % 2 == 0 { String::new() } else { format!("Event {b}") };
            h.date = dates[*a as usize % dates.len()].into();
            h.round = rounds[*b as usize % rounds.len()].into();
            h.eco = ecos[(*a as usize + *b as usize) % ecos.len()].into();
            h.white_elo = (*elo > 1000).then_some(*elo % 3000);
            if *extra {
                h.extra.push(("Opening".into(), "Sicilian".into()));
            }
            source.lock().unwrap().append(&game).unwrap();
        }
        // Names the target already has get the target's ids.
        target.lock().unwrap().append(&random_game(&[])).unwrap();
        let ids: Vec<u32> = (0..games.len() as u32).rev().collect();
        let summary = copy_games(&source, &ids, &target, &NoProgress).unwrap();
        prop_assert_eq!(&summary.copied, &ids);
        let (src, dst) = (source.lock().unwrap(), target.lock().unwrap());
        for (copy, &id) in (1..).zip(&ids) {
            prop_assert_eq!(dst.read_game(copy).unwrap(), src.read_game(id).unwrap());
            prop_assert_eq!(dst.read_blob(copy).unwrap(), src.read_blob(id).unwrap());
            prop_assert_eq!(dst.row(copy).unwrap().date, src.row(id).unwrap().date);
        }
    }
}
