//! Duplicate games (DB-08) end to end: importing a file twice, copies from
//! another source, deleting the copies, restoring them and compacting.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use chess_core::game::GameResult;
use chess_core::pgn::{read_pgn_str, write_game};
use chess_core::shakmaty::Chess;
use chess_db::blob::with_date_round_eco;
use chess_db::record::{FLAG_DELETED, IndexRecord};
use chess_db::{
    Database, DuplicateOptions, Duplicates, GameId, HeaderQuery, NoProgress, SortKey, SortSpec, build_sort,
    delete_copies, export_pgn, fill_kept_games, find_duplicates, find_position, import_pgn, search_headers,
    unfill, update_position_index,
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

/// `testdata/pgn/duplicates.pgn`: an annotated game (0, round 1) with a copy
/// without the ratings (1) and one from another source that stops early,
/// with the board (2: round 1.3); another game of the same players (3); a
/// game with two exact copies (4–6); two online games with the same moves
/// on their own pages (7, 8). Deleting the copies, game 0 takes game 2's
/// board (R4-1).
#[test]
fn the_duplicates_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let db = Mutex::new(Database::create(&dir.path().join("Fixture")).unwrap());
    let summary = import_pgn(&db, &fixtures().join("duplicates.pgn"), &NoProgress).unwrap();
    assert_eq!((summary.imported, summary.warning_count), (9, 0), "{:?}", summary.warnings);
    let groups = |fuzzy| groups_of(&find(&db, fuzzy));
    assert_eq!(groups(false), [(4, vec![5, 6], true)]);
    assert_eq!(groups(true), [(0, vec![1, 2], false), (4, vec![5, 6], true)]);

    let before = snapshot(&db);
    let found = find(&db, true);
    let done = delete_copies(&mut db.lock().unwrap(), &found.groups).unwrap();
    assert_eq!((done.marked.as_slice(), done.filled.len()), ([1, 2, 5, 6].as_slice(), 1));
    assert_eq!(db.lock().unwrap().row(0).unwrap().round, "1.3");
    let mut d = db.lock().unwrap();
    d.set_deleted_many(&done.marked, false).unwrap();
    assert_eq!(unfill(&mut d, &done.filled).unwrap(), [0]);
    drop(d);
    assert_eq!(snapshot(&db), before);
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

/// R4-1: an annotated copy (0) from a source without the ratings, board,
/// day, ECO or result, the same game from TWIC (1) and from a third source
/// (5); another game (2); exact copies (3, 4). The kept game 0 takes what
/// it lacks from 1 and 5 before they're deleted.
const SOURCES: &str = r#"[Event "Tata Steel Masters"]
[Site "Wijk aan Zee"]
[Date "2024.01.??"]
[Round "7"]
[White "Carlsen, Magnus"]
[Black "Caruana, Fabiano"]
[Result "*"]
[Source "Magazine"]

1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3
O-O 9. h3 Nb8 {The Breyer.} 10. d4 Nbd7 11. Nbd2 Bb7 12. Bc2 Re8 13. Nf1 Bf8 14.
Ng3 g6 15. a4 (15. b3 c6) 15... c5 $1 16. d5 c4 17. Bg5 h6 18. Be3 Nc5 19. Qd2 h5
20. Bg5 Be7 21. Ra3 Nfd7 22. Bh6 Bf6 *

[Event "Tata Steel Masters 2024"]
[Site "Wijk aan Zee NED"]
[Date "2024.01.20"]
[Round "7.3"]
[White "Carlsen,M"]
[Black "Caruana,F"]
[Result "1/2-1/2"]
[WhiteElo "2830"]
[BlackElo "2804"]
[ECO "C95"]
[WhiteFideId "1503014"]

1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3
O-O 9. h3 Nb8 10. d4 Nbd7 11. Nbd2 Bb7 12. Bc2 Re8 13. Nf1 Bf8 14. Ng3 g6 15. a4
c5 16. d5 c4 17. Bg5 h6 18. Be3 Nc5 19. Qd2 h5 20. Bg5 Be7 21. Ra3 Nfd7 22. Bh6
Bf6 1/2-1/2

[Event "Wijk aan Zee"]
[Site "Netherlands"]
[Date "2024.01.20"]
[Round "3"]
[White "Gukesh, D"]
[Black "Ding, Liren"]
[Result "1-0"]
[WhiteElo "2725"]
[BlackElo "2780"]

1. d4 Nf6 2. c4 e6 3. Nc3 Bb4 4. e3 O-O 5. Bd3 d5 6. Nf3 c5 7. O-O Nc6 1-0

[Event "Tata Steel Challengers"]
[Site "Wijk aan Zee NED"]
[Date "2024.01.21"]
[Round "8.1"]
[White "Berg, Anna"]
[Black "Holm, Sara"]
[Result "1/2-1/2"]
[WhiteElo "2610"]
[BlackElo "2595"]

1. d4 Nf6 2. c4 e6 3. Nc3 Bb4 4. e3 O-O 5. Bd3 d5 6. Nf3 c5 7. O-O Nc6 8. a3
Bxc3 9. bxc3 dxc4 10. Bxc4 Qc7 11. Bd3 e5 12. Qc2 Re8 1/2-1/2

[Event "Tata Steel Challengers"]
[Site "Wijk aan Zee NED"]
[Date "2024.01.21"]
[Round "8.1"]
[White "Berg, Anna"]
[Black "Holm, Sara"]
[Result "1/2-1/2"]
[WhiteElo "2610"]
[BlackElo "2595"]

1. d4 Nf6 2. c4 e6 3. Nc3 Bb4 4. e3 O-O 5. Bd3 d5 6. Nf3 c5 7. O-O Nc6 8. a3
Bxc3 9. bxc3 dxc4 10. Bxc4 Qc7 11. Bd3 e5 12. Qc2 Re8 1/2-1/2

[Event "Tata Steel"]
[Site "?"]
[Date "2024.01.20"]
[Round "7.3"]
[White "Magnus Carlsen"]
[Black "Fabiano Caruana"]
[Result "1/2-1/2"]
[WhiteElo "2830"]
[BlackElo "2804"]

1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 4. Ba4 Nf6 5. O-O Be7 6. Re1 b5 7. Bb3 d6 8. c3
O-O 9. h3 Nb8 10. d4 Nbd7 11. Nbd2 Bb7 12. Bc2 Re8 13. Nf1 Bf8 14. Ng3 g6 15. a4
c5 16. d5 c4 17. Bg5 h6 18. Be3 Nc5 19. Qd2 h5 20. Bg5 Be7 21. Ra3 Nfd7 22. Bh6
Bf6 1/2-1/2
"#;

fn sources() -> (tempfile::TempDir, Mutex<Database>) {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(&dir.path().join("Sources")).unwrap();
    for g in read_pgn_str(SOURCES) {
        db.append(&g).unwrap();
    }
    (dir, Mutex::new(db))
}

/// Every game's index record and blob.
fn snapshot(db: &Mutex<Database>) -> Vec<(IndexRecord, Vec<u8>)> {
    let db = db.lock().unwrap();
    (0..db.count()).map(|id| (db.record(id).unwrap(), db.read_blob(id).unwrap())).collect()
}

fn blob_file_len(db: &Mutex<Database>) -> u64 {
    std::fs::metadata(db.lock().unwrap().path().join("games.blob")).unwrap().len()
}

fn groups_of(found: &Duplicates) -> Vec<(GameId, Vec<GameId>, bool)> {
    found.groups.iter().map(|g| (g.keep, g.remove.clone(), g.exact)).collect()
}

/// The result the position index gives game `id` at the start position.
fn indexed_result(db: &Mutex<Database>, id: GameId) -> u8 {
    let hits = find_position(db, &Chess::default()).unwrap();
    hits.entries.iter().find(|e| e.game == id).unwrap().result
}

/// R4-1: deleting the copies fills in what the kept game lacks first
/// (field by field), leaving its moves, annotations and extra tags and
/// every other game byte for byte; Restore puts every record and blob back
/// as it was. The position index, sorts and header searches follow.
#[test]
fn deleting_copies_fills_in_the_kept_game_and_restore_takes_it_out() {
    let (_dir, db) = sources();
    update_position_index(&db, &NoProgress).unwrap();
    let by_elo = SortSpec::new(SortKey::WhiteElo, true);
    build_sort(&db, by_elo, &NoProgress).unwrap();
    let eco = HeaderQuery { eco_from: "C95".into(), eco_to: "C95".into(), ..HeaderQuery::default() };
    assert_eq!(search_headers(&db, &eco, None, &NoProgress).unwrap(), [1]);
    let before = snapshot(&db);
    let game_before = db.lock().unwrap().read_game(0).unwrap();

    assert_eq!(groups_of(&find(&db, false)), [(3, vec![4], true)]);
    let found = find(&db, true);
    assert_eq!(groups_of(&found), [(0, vec![1, 5], false), (3, vec![4], true)]);
    let done = delete_copies(&mut db.lock().unwrap(), &found.groups).unwrap();
    assert_eq!((done.marked.as_slice(), done.left), ([1, 4, 5].as_slice(), 0));
    assert_eq!(done.filled.len(), 1);
    assert_eq!((done.filled[0].game, done.filled[0].before), (0, before[0].0));

    // The kept game, field by field: what it lacked, from TWIC's copy; its
    // own event and site stand.
    let after = snapshot(&db);
    let d = db.lock().unwrap();
    let row = d.row(0).unwrap();
    assert_eq!((row.white_elo, row.black_elo, row.eco.as_str()), (Some(2830), Some(2804), "C95"));
    assert_eq!((row.round.as_str(), row.date.as_str(), row.result), ("7.3", "2024.01.20", GameResult::Draw));
    assert_eq!((row.event.as_str(), row.site.as_str()), ("Tata Steel Masters", "Wijk aan Zee"));
    assert_eq!(
        (row.white.as_str(), row.plies, row.has_comments, row.deleted),
        ("Carlsen, Magnus", 44, true, false)
    );
    assert_eq!(done.filled[0].after, after[0].0);
    // Its blob: the same but for the Date, Round and ECO strings.
    assert_eq!(after[0].1, with_date_round_eco(&before[0].1, "2024.01.20", "7.3", "C95").unwrap());
    let game = d.read_game(0).unwrap();
    let mut expected = game_before.clone();
    expected.headers.date = "2024.01.20".into();
    expected.headers.round = "7.3".into();
    expected.headers.eco = "C95".into();
    expected.headers.white_elo = Some(2830);
    expected.headers.black_elo = Some(2804);
    expected.headers.result = GameResult::Draw;
    assert_eq!(game, expected, "moves, comments, variations and the Source tag kept");
    // Every other game: the same record (but for the deleted flag) and blob.
    for id in 1..d.count() {
        let deleted = done.marked.contains(&id);
        assert_eq!(after[id as usize].0.is_deleted(), deleted, "{id}");
        let unflagged =
            IndexRecord { flags: after[id as usize].0.flags & !FLAG_DELETED, ..after[id as usize].0 };
        assert_eq!(
            (unflagged, &after[id as usize].1),
            (before[id as usize].0, &before[id as usize].1),
            "{id}"
        );
    }
    // Its result changed: the position index replays it; sorts are rebuilt,
    // header searches see the new fields, under the same game ids.
    assert_eq!(d.positions().dirty_count(), 1);
    assert!(!d.has_sort(by_elo));
    drop(d);
    assert_eq!(indexed_result(&db, 0), 3);
    let order = build_sort(&db, by_elo, &NoProgress).unwrap();
    assert_eq!(&order[..3], [0, 1, 5], "2830, by id");
    assert_eq!(search_headers(&db, &eco, None, &NoProgress).unwrap(), [0, 1]);

    // Restore: the copies, then the fill. Every record and blob as before.
    let mut d = db.lock().unwrap();
    assert_eq!(d.set_deleted_many(&done.marked, false).unwrap(), 3);
    assert_eq!(unfill(&mut d, &done.filled).unwrap(), [0]);
    drop(d);
    assert_eq!(snapshot(&db), before);
    assert_eq!(indexed_result(&db, 0), 0);
    assert_eq!(search_headers(&db, &eco, None, &NoProgress).unwrap(), [1]);
    assert_eq!(groups_of(&find(&db, true)), groups_of(&found), "found again");
    assert_eq!(unfill(&mut db.lock().unwrap(), &done.filled).unwrap(), [], "undone already");
}

/// R4-1: a kept game that has every field of its copies is left as it is;
/// so are groups whose copies disagree on a field the kept game lacks, or
/// whose kept game is deleted (only a change since the search does that).
#[test]
fn nothing_is_written_when_the_kept_game_lacks_nothing() {
    let (_dir, db) = sources();
    let found = find(&db, true);
    let exact = found.groups[1].clone();
    let (len, generation) = (blob_file_len(&db), db.lock().unwrap().generation());
    let kept = db.lock().unwrap().record(3).unwrap();
    let done = delete_copies(&mut db.lock().unwrap(), &[exact]).unwrap();
    assert_eq!(
        (done.marked.as_slice(), done.filled.as_slice(), done.left),
        ([4].as_slice(), [].as_slice(), 0)
    );
    assert_eq!(blob_file_len(&db), len);
    assert_eq!(db.lock().unwrap().generation(), generation + 1, "the delete flag alone");
    assert_eq!(db.lock().unwrap().record(3).unwrap(), kept);

    // Copies that disagree (2 is another game, with another day and
    // ratings), or a deleted kept game: nothing written, nothing deleted.
    let before = snapshot(&db);
    let generation = db.lock().unwrap().generation();
    let odd = chess_db::DuplicateGroup { keep: 0, remove: vec![1, 2], exact: false };
    let done = delete_copies(&mut db.lock().unwrap(), &[odd]).unwrap();
    assert_eq!((done.marked.len(), done.filled.len(), done.left), (0, 0, 1));
    db.lock().unwrap().set_deleted(0, true).unwrap();
    let generation = generation + 1;
    let gone = chess_db::DuplicateGroup { keep: 0, remove: vec![1, 5], exact: false };
    let done = delete_copies(&mut db.lock().unwrap(), &[gone]).unwrap();
    assert_eq!((done.marked.len(), done.filled.len(), done.left), (0, 0, 1));
    assert_eq!(db.lock().unwrap().generation(), generation);
    db.lock().unwrap().set_deleted(0, false).unwrap();
    assert_eq!(snapshot(&db), before);
}

/// R4-1: a crash after the kept game is filled in and before its copies are
/// deleted leaves both: nothing is lost, and finding them again deletes the
/// copies with nothing more to fill in. A compaction after a delete keeps
/// what the kept game took, under its new id.
#[test]
fn a_crash_between_the_fill_and_the_delete_and_a_compaction_after() {
    let (_dir, db) = sources();
    let found = find(&db, true);
    let (filled, can_go) = fill_kept_games(&mut db.lock().unwrap(), &found.groups).unwrap();
    assert_eq!((filled.len(), can_go.as_slice()), (1, [true, true].as_slice()));
    let path = db.lock().unwrap().path().to_path_buf();
    // The process dies here, before the delete: what the fill wrote was
    // fsynced, and nothing else is (`Database` has no `Drop` that writes).
    drop(db);
    let db = Mutex::new(Database::open(&path).unwrap());
    let row = db.lock().unwrap().row(0).unwrap();
    assert_eq!((row.white_elo, row.round.as_str(), row.eco.as_str()), (Some(2830), "7.3", "C95"));
    assert!((1..6).all(|id| !db.lock().unwrap().record(id).unwrap().is_deleted()), "the copies are there");

    let again = find(&db, true);
    assert_eq!(groups_of(&again), groups_of(&found));
    let len = blob_file_len(&db);
    let done = delete_copies(&mut db.lock().unwrap(), &again.groups).unwrap();
    assert_eq!(
        (done.marked.as_slice(), done.filled.len()),
        ([1, 4, 5].as_slice(), 0),
        "nothing more to fill in"
    );
    assert_eq!(blob_file_len(&db), len);

    // Compacting: the copies go, the kept game keeps what it took, and the
    // other games are renumbered.
    let rows: Vec<_> = (0..6).map(|id| db.lock().unwrap().row(id).unwrap()).collect();
    let summary = db.lock().unwrap().compact(&NoProgress).unwrap();
    assert_eq!(summary.mapping, [Some(0), None, Some(1), Some(2), None, None]);
    let d = db.lock().unwrap();
    for (old, new) in summary.mapping.iter().enumerate() {
        if let Some(new) = *new {
            assert_eq!(
                d.row(new).unwrap(),
                chess_db::GameRow { id: new, ..rows[old].clone() },
                "{old} → {new}"
            );
        }
    }
    let game = d.read_game(0).unwrap();
    assert_eq!(
        (game.headers.date.as_str(), game.headers.round.as_str(), game.headers.eco.as_str()),
        ("2024.01.20", "7.3", "C95")
    );
    drop(d);
    assert_eq!(find(&db, true).groups, []);
}

/// R4-1: groups that overlap (not a report's): a copy that is another
/// group's kept game is filled in, then deleted with what it took.
#[test]
fn a_copy_filled_in_as_another_groups_kept_game_keeps_it() {
    let (_dir, db) = sources();
    let groups = [
        chess_db::DuplicateGroup { keep: 0, remove: vec![5], exact: false },
        chess_db::DuplicateGroup { keep: 5, remove: vec![1], exact: false },
    ];
    let done = delete_copies(&mut db.lock().unwrap(), &groups).unwrap();
    assert_eq!(done.marked, [1, 5]);
    let five = done.filled.iter().find(|f| f.game == 5).unwrap();
    let rec = db.lock().unwrap().record(5).unwrap();
    assert_eq!(IndexRecord { flags: rec.flags & !FLAG_DELETED, ..rec }, five.after);
    assert!(rec.is_deleted() && rec.eco != 0, "deleted, with the ECO it took");
}

/// R5-3: two groups with the same kept game (not a report's) would each
/// fill it in from its record as it was, the second write losing the
/// first's fields: refused before anything is written.
#[test]
fn two_groups_with_the_same_kept_game_are_refused() {
    let (_dir, db) = sources();
    let before = snapshot(&db);
    let (len, generation) = (blob_file_len(&db), db.lock().unwrap().generation());
    let groups = [
        chess_db::DuplicateGroup { keep: 0, remove: vec![1], exact: false },
        chess_db::DuplicateGroup { keep: 3, remove: vec![4], exact: true },
        chess_db::DuplicateGroup { keep: 0, remove: vec![5], exact: false },
    ];
    let deleting = delete_copies(&mut db.lock().unwrap(), &groups).unwrap_err();
    let filling = fill_kept_games(&mut db.lock().unwrap(), &groups).unwrap_err();
    for err in [deleting, filling] {
        assert!(err.to_string().contains("game 0 is the kept game of two groups"), "{err}");
    }
    assert_eq!((blob_file_len(&db), db.lock().unwrap().generation()), (len, generation));
    assert_eq!(snapshot(&db), before);
    // As one group, the copies fill it in together.
    let one = chess_db::DuplicateGroup { keep: 0, remove: vec![1, 5], exact: false };
    let done = delete_copies(&mut db.lock().unwrap(), &[one]).unwrap();
    assert_eq!((done.marked.as_slice(), done.filled.len()), ([1, 5].as_slice(), 1));
}

/// R5-5: two deletes that fill in one kept game (an exact search, then a
/// fuzzy one finding more copies of it) are undone together, and counted
/// as one game.
#[test]
fn two_fills_of_one_kept_game_are_undone_as_one() {
    let (_dir, db) = sources();
    let before = snapshot(&db);
    let first = chess_db::DuplicateGroup { keep: 0, remove: vec![5], exact: false };
    let second = chess_db::DuplicateGroup { keep: 0, remove: vec![1], exact: false };
    let mut d = db.lock().unwrap();
    let one = delete_copies(&mut d, &[first]).unwrap();
    let two = delete_copies(&mut d, &[second]).unwrap();
    let filled: Vec<_> = one.filled.iter().chain(&two.filled).copied().collect();
    assert_eq!(filled.iter().map(|f| f.game).collect::<Vec<_>>(), [0, 0], "two fills of game 0");
    d.set_deleted_many(&[1, 5], false).unwrap();
    assert_eq!(unfill(&mut d, &filled).unwrap(), [0]);
    drop(d);
    assert_eq!(snapshot(&db), before);
}

/// R4-1: Restore after the user edited the kept game brings the copies
/// back and leaves the edit (with what the game took) alone.
#[test]
fn restore_leaves_a_kept_game_edited_since() {
    let (_dir, db) = sources();
    let found = find(&db, true);
    let done = delete_copies(&mut db.lock().unwrap(), &found.groups).unwrap();
    let mut d = db.lock().unwrap();
    let mut edited = d.read_game(0).unwrap();
    edited.headers.annotator = "Me".into();
    d.replace(0, &edited).unwrap();
    let record = d.record(0).unwrap();
    assert_eq!(d.set_deleted_many(&done.marked, false).unwrap(), 3);
    assert_eq!(unfill(&mut d, &done.filled).unwrap(), []);
    assert_eq!(d.record(0).unwrap(), record);
    let row = d.row(0).unwrap();
    assert_eq!((row.annotator.as_str(), row.white_elo, row.eco.as_str()), ("Me", Some(2830), "C95"));
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
