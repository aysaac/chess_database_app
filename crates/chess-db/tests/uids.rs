//! Permanent game ids (NET-13 groundwork): the upgrade of databases written
//! before them, crash safety, copies and compaction, and a model of a
//! database's ids through any mix of writes.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chess_core::game::Game;
use chess_core::pgn::read_pgn_str;
use chess_db::{Database, GameId, GameUid, NoProgress, copy_games, export_pgn, import_pgn};
use proptest::prelude::*;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

/// `testdata/db/format-v2`, written by the code before permanent ids
/// (format 2): variations.pgn, setup.pgn and lichess.pgn, game 1 deleted and
/// game 3 replaced with White "Edited, White". Copied, since opening it
/// upgrades it.
fn format_2_copy(dir: &Path) -> PathBuf {
    let to = dir.join("Old.chessdb");
    std::fs::create_dir(&to).unwrap();
    for entry in std::fs::read_dir(root().join("db/format-v2")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
    to
}

fn fixture_games() -> Vec<Game> {
    ["variations.pgn", "setup.pgn", "lichess.pgn"]
        .iter()
        .flat_map(|f| read_pgn_str(&std::fs::read_to_string(root().join("pgn").join(f)).unwrap()))
        .collect()
}

fn meta_version(path: &Path) -> String {
    std::fs::read_to_string(path.join("db.meta")).unwrap().lines().next().unwrap().to_owned()
}

fn all_uids(db: &Database) -> Vec<GameUid> {
    db.uids(0, db.count()).unwrap()
}

fn assert_distinct(uids: &[GameUid]) {
    assert_eq!(uids.iter().collect::<HashSet<_>>().len(), uids.len(), "{uids:?}");
}

fn append(path: &Path, bytes: &[u8]) {
    OpenOptions::new().append(true).open(path).unwrap().write_all(bytes).unwrap();
}

fn game(white: &str) -> Game {
    let mut g = read_pgn_str("1. e4 e5 *").remove(0);
    g.headers.white = white.into();
    g
}

#[test]
fn a_format_2_database_gets_permanent_ids_on_first_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = format_2_copy(dir.path());
    let before: Vec<(String, Vec<u8>)> = ["games.idx", "games.blob", "players.names"]
        .iter()
        .map(|f| (f.to_string(), std::fs::read(path.join(f)).unwrap()))
        .collect();
    assert_eq!(meta_version(&path), "chessdb 2");
    // Reading its count needs no upgrade, and makes none.
    assert_eq!(Database::summary(&path).unwrap().games, 6);
    assert!(!path.join("games.ids").exists());

    let mut db = Database::open(&path).unwrap();
    assert_eq!(meta_version(&path), "chessdb 3");
    assert_eq!(db.count(), 6);
    let uids = all_uids(&db);
    assert_eq!(uids.len(), 6);
    assert_distinct(&uids);
    for (f, bytes) in &before {
        assert_eq!(&std::fs::read(path.join(f)).unwrap(), bytes, "{f} is unchanged");
    }
    // The games are as they were: one deleted, one replaced.
    let games = fixture_games();
    for id in 0..6 {
        let mut expected = games[id as usize].clone();
        if id == 3 {
            expected.headers.white = "Edited, White".into();
        }
        assert_eq!(db.read_game(id).unwrap(), expected, "game {id}");
        assert_eq!(db.row(id).unwrap().deleted, id == 1);
        assert_eq!(db.game_by_uid(uids[id as usize]).unwrap(), Some(id));
        assert_eq!(db.origin(id).unwrap(), None);
    }
    drop(db);

    // The ids stay, and the database works as any other.
    let mut db = Database::open(&path).unwrap();
    assert_eq!(all_uids(&db), uids);
    let new = db.append(&game("New")).unwrap();
    assert_eq!(db.compact(&NoProgress).unwrap().mapping[1], None);
    assert_eq!(all_uids(&db)[..4], [uids[0], uids[2], uids[3], uids[4]]);
    assert_eq!(db.game_by_uid(uids[1]).unwrap(), None, "compacted away");
    assert_eq!(db.game_by_uid(uids[5]).unwrap(), Some(4));
    assert_eq!(db.read_game(5).unwrap().headers.white, "New");
    assert_eq!(new, 6);
}

/// A crash in the upgrade, after `games.ids` was renamed into place but
/// before `db.meta` said 3, leaves a version 2 database with ids that an
/// older app may have made stale since (here it appended a game without
/// one): the next open makes them all again. A temp file from a crash
/// before the rename is deleted.
#[test]
fn an_upgrade_cut_short_is_made_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = format_2_copy(dir.path());
    Database::open(&path).unwrap().append(&game("Old app")).unwrap();
    // Version 2 again, its ids file one short (as an older app's append
    // leaves it), and the upgrade's temp file.
    let ids = OpenOptions::new().write(true).open(path.join("games.ids")).unwrap();
    ids.set_len(16 + 6 * 16).unwrap();
    drop(ids);
    let meta = std::fs::read_to_string(path.join("db.meta")).unwrap();
    std::fs::write(path.join("db.meta"), meta.replace("chessdb 3", "chessdb 2")).unwrap();
    let stale = std::fs::read(path.join("games.ids")).unwrap();
    std::fs::write(path.join("games.ids.tmp"), &stale[..40]).unwrap();
    let stale_uids: HashSet<&[u8]> = stale[16..].chunks(16).collect();

    let db = Database::open(&path).unwrap();
    assert_eq!(meta_version(&path), "chessdb 3");
    assert!(!path.join("games.ids.tmp").exists());
    let uids = all_uids(&db);
    assert_eq!(uids.len(), 7);
    assert_distinct(&uids);
    assert!(uids.iter().all(|u| !stale_uids.contains(&u.to_bytes()[..])), "all made again");
}

/// Crash safety: a commit cut short leaves ids and an origin past the
/// committed games (and a torn origin entry). Opening drops them; the games
/// keep their ids, and the next commit mints new ones.
#[test]
fn ids_of_a_commit_cut_short_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let source = Mutex::new(Database::create(&dir.path().join("Source")).unwrap());
    let target = Mutex::new(Database::create(&dir.path().join("Target")).unwrap());
    for w in ["A", "B"] {
        source.lock().unwrap().append(&game(w)).unwrap();
    }
    copy_games(&source, &[0], &target, &NoProgress).unwrap();
    let (path, uids, origin) = {
        let mut t = target.lock().unwrap();
        (t.path().to_path_buf(), all_uids(&t), t.origin(0).unwrap())
    };
    drop(target);
    assert_eq!(origin, Some(source.lock().unwrap().uid(0).unwrap()));

    // The next commit wrote its blob, an id, an origin entry and a torn one,
    // and its record, but not the count.
    let uncommitted = [0x42u8; 16];
    append(&path.join("games.blob"), &[0xAB; 100]);
    append(&path.join("games.ids"), &uncommitted);
    append(&path.join("games.origins"), &[uncommitted, [0x43; 16]].concat());
    append(&path.join("games.origins"), &[0x44; 20]);
    append(&path.join("games.idx"), &[0x11; 64]);

    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.count(), 1);
    assert_eq!(all_uids(&db), uids);
    assert_eq!(db.origin(0).unwrap(), origin);
    let len = |f: &str| std::fs::metadata(path.join(f)).unwrap().len();
    assert_eq!(len("games.ids"), 16 + 16, "the uncommitted id is dropped");
    assert_eq!(len("games.origins"), 16 + 2 * 32, "the torn entry is dropped, the whole one is harmless");
    let id = db.append(&game("C")).unwrap();
    let new = db.uid(id).unwrap();
    assert_ne!(new.to_bytes(), uncommitted);
    assert_eq!(db.origin(id).unwrap(), None);
    assert_eq!(db.game_by_uid(new).unwrap(), Some(id));
    assert_eq!(db.game_by_uid(GameUid::from_bytes(uncommitted)).unwrap(), None);
}

/// Ids lost anyway (a file that lost its end or is gone, which no crash
/// does): the database still opens, those games get new ids.
#[test]
fn games_without_ids_get_new_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(&dir.path().join("Lost")).unwrap();
    for w in ["A", "B", "C"] {
        db.append(&game(w)).unwrap();
    }
    let path = db.path().to_path_buf();
    let uids = all_uids(&db);
    drop(db);
    let ids = OpenOptions::new().write(true).open(path.join("games.ids")).unwrap();
    ids.set_len(16 + 16 + 5).unwrap();
    drop(ids);
    let db = Database::open(&path).unwrap();
    let now = all_uids(&db);
    assert_eq!(now[0], uids[0]);
    assert_distinct(&[&uids[..], &now[1..]].concat());
    drop(db);

    std::fs::remove_file(path.join("games.ids")).unwrap();
    std::fs::remove_file(path.join("games.origins")).unwrap();
    let db = Database::open(&path).unwrap();
    assert_eq!(all_uids(&db).len(), 3);
    assert_distinct(&[&now[..], &all_uids(&db)[..]].concat());
}

/// A copy gets a new id in its database and names the one it was copied
/// from; a copy of a copy names the copy. PGN carries no ids: exporting and
/// importing again makes new ones, and no origin.
#[test]
fn copies_get_new_ids_and_name_their_origin() {
    let dir = tempfile::tempdir().unwrap();
    let make = |name: &str| Mutex::new(Database::create(&dir.path().join(name)).unwrap());
    let (a, b, c) = (make("A"), make("B"), make("C"));
    for w in ["A0", "A1", "A2"] {
        a.lock().unwrap().append(&game(w)).unwrap();
    }
    b.lock().unwrap().append(&game("B0")).unwrap();
    copy_games(&a, &[2, 0], &b, &NoProgress).unwrap();
    copy_games(&b, &[1], &c, &NoProgress).unwrap();

    let a_uids = all_uids(&a.lock().unwrap());
    let mut bl = b.lock().unwrap();
    let b_uids = all_uids(&bl);
    assert_distinct(&[&a_uids[..], &b_uids[..]].concat());
    let origins: Vec<_> = (0..3).map(|id| bl.origin(id).unwrap()).collect();
    assert_eq!(origins, [None, Some(a_uids[2]), Some(a_uids[0])]);
    let mut cl = c.lock().unwrap();
    assert_eq!(cl.origin(0).unwrap(), Some(b_uids[1]), "the copy it was copied from");
    assert!(!a_uids.contains(&cl.uid(0).unwrap()) && !b_uids.contains(&cl.uid(0).unwrap()));

    // Compaction keeps the origins of the games kept, and only those.
    bl.set_deleted(1, true).unwrap();
    bl.compact(&NoProgress).unwrap();
    assert_eq!((bl.count(), bl.origin(1).unwrap()), (2, Some(a_uids[0])));
    let path = bl.path().to_path_buf();
    drop(bl);
    drop(b);
    let len = std::fs::metadata(path.join("games.origins")).unwrap().len();
    assert_eq!(len, 16 + 32, "the deleted copy's origin is dropped");
    let mut reopened = Database::open(&path).unwrap();
    assert_eq!(reopened.origin(1).unwrap(), Some(a_uids[0]));
    assert_eq!(reopened.game_by_uid(b_uids[2]).unwrap(), Some(1));
    drop(reopened);

    // Through PGN: no ids in the text, new ones after.
    let pgn = dir.path().join("out.pgn");
    export_pgn(&a, None, &pgn, &NoProgress).unwrap();
    let text = std::fs::read_to_string(&pgn).unwrap();
    assert!(
        a_uids
            .iter()
            .all(|u| !text.contains(&u.to_string()) && !text.contains(&u.to_string().replace('-', "")))
    );
    let again = make("Again");
    import_pgn(&again, &pgn, &NoProgress).unwrap();
    let mut again = again.lock().unwrap();
    let again_uids = all_uids(&again);
    assert_distinct(&[&a_uids[..], &again_uids[..]].concat());
    assert_eq!(again.origin(0).unwrap(), None);
    for id in 0..3 {
        assert_eq!(again.read_game(id).unwrap(), a.lock().unwrap().read_game(id).unwrap());
    }
}

/// A compaction cut short after its commit is finished on open, with the
/// ids and origins of the compacted games.
#[test]
fn a_compaction_finished_on_open_brings_its_ids() {
    let dir = tempfile::tempdir().unwrap();
    let source = Mutex::new(Database::create(&dir.path().join("Source")).unwrap());
    source.lock().unwrap().append(&game("S")).unwrap();
    let target = Mutex::new(Database::create(&dir.path().join("Target")).unwrap());
    target.lock().unwrap().append(&game("T0")).unwrap();
    copy_games(&source, &[0], &target, &NoProgress).unwrap();
    target.lock().unwrap().set_deleted(0, true).unwrap();
    let (path, uids) = {
        let t = target.lock().unwrap();
        (t.path().to_path_buf(), all_uids(&t))
    };
    drop(target);
    // What a compaction writes before its commit marker, made here by a
    // compaction of a copy of the database.
    let copy = dir.path().join("Copy.chessdb");
    std::fs::create_dir(&copy).unwrap();
    for entry in std::fs::read_dir(&path).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
    }
    Database::open(&copy).unwrap().compact(&NoProgress).unwrap();
    for f in [
        "games.idx",
        "games.blob",
        "games.ids",
        "games.origins",
        "players.names",
        "events.names",
        "sites.names",
        "annotators.names",
    ] {
        std::fs::copy(copy.join(f), path.join(format!("{f}.new"))).unwrap();
    }
    std::fs::write(path.join("compact.commit"), b"compact\n").unwrap();

    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.count(), 1);
    assert_eq!(all_uids(&db), [uids[1]]);
    assert_eq!(db.origin(0).unwrap(), Some(source.lock().unwrap().uid(0).unwrap()));
    assert!(!path.join("games.ids.new").exists() && !path.join("compact.commit").exists());
}

/// A database's ids in a model: what each slot's game is.
#[derive(Debug, Clone)]
struct Slot {
    uid: GameUid,
    origin: Option<GameUid>,
    deleted: bool,
}

#[derive(Debug, Clone)]
enum Op {
    Append(u8),
    Copy(Vec<u8>),
    Delete(u8),
    Undelete(u8),
    Replace(u8),
    Compact,
    Reopen,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (1u8..5).prop_map(Op::Append),
        2 => prop::collection::vec(any::<u8>(), 1..4).prop_map(Op::Copy),
        2 => any::<u8>().prop_map(Op::Delete),
        1 => any::<u8>().prop_map(Op::Undelete),
        1 => any::<u8>().prop_map(Op::Replace),
        1 => Just(Op::Compact),
        1 => Just(Op::Reopen),
    ]
}

proptest! {
    // Each case creates two databases and syncs every write, so few cases.
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Through any mix of writes, every game keeps its permanent id, found
    /// at its slot whatever compactions moved it; no id is ever given out
    /// twice, here or in the database copied from; an id compacted away
    /// finds nothing; copies name their origin.
    #[test]
    fn games_keep_their_permanent_ids(ops in prop::collection::vec(op(), 1..12)) {
        let dir = tempfile::tempdir().unwrap();
        let source = Mutex::new(Database::create(&dir.path().join("Source")).unwrap());
        for i in 0..4 {
            source.lock().unwrap().append(&game(&format!("S{i}"))).unwrap();
        }
        let source_uids = all_uids(&source.lock().unwrap());
        let path = dir.path().join("Db.chessdb");
        // `None` only while it's reopened (it's locked while open).
        let mut db = Some(Mutex::new(Database::create(&path).unwrap()));
        let mut model: Vec<Slot> = Vec::new();
        let mut minted: HashSet<GameUid> = source_uids.iter().copied().collect();
        let mut gone: Vec<GameUid> = Vec::new();
        let pick = |model: &[Slot], i: u8| (!model.is_empty()).then(|| i as usize % model.len());

        for op in ops {
            match op {
                Op::Append(n) => {
                    let games: Vec<_> =
                        (0..n).map(|i| chess_db::PreparedGame::new(&game(&format!("G{i}")))).collect();
                    let d = db.as_mut().unwrap().get_mut().unwrap();
                    for id in d.append_prepared(&games).unwrap() {
                        let uid = d.uid(id).unwrap();
                        prop_assert!(minted.insert(uid), "{} given out twice", uid);
                        model.push(Slot { uid, origin: None, deleted: false });
                    }
                }
                Op::Copy(picks) => {
                    let ids: Vec<GameId> = picks.iter().map(|&p| u32::from(p) % 4).collect();
                    let summary = copy_games(&source, &ids, db.as_ref().unwrap(), &NoProgress).unwrap();
                    let d = db.as_mut().unwrap().get_mut().unwrap();
                    let first = model.len() as u32;
                    for (i, &from) in summary.copied.iter().enumerate() {
                        let uid = d.uid(first + i as u32).unwrap();
                        prop_assert!(minted.insert(uid), "{} given out twice", uid);
                        model.push(Slot { uid, origin: Some(source_uids[from as usize]), deleted: false });
                    }
                }
                Op::Delete(i) | Op::Undelete(i) => {
                    if let Some(slot) = pick(&model, i) {
                        let deleted = matches!(op, Op::Delete(_));
                        db.as_mut().unwrap().get_mut().unwrap().set_deleted(slot as GameId, deleted).unwrap();
                        model[slot].deleted = deleted;
                    }
                }
                Op::Replace(i) => {
                    if let Some(slot) = pick(&model, i) {
                        let d = db.as_mut().unwrap().get_mut().unwrap();
                        d.replace(slot as GameId, &game("Replaced")).unwrap();
                    }
                }
                Op::Compact => {
                    let summary = db.as_mut().unwrap().get_mut().unwrap().compact(&NoProgress).unwrap();
                    prop_assert_eq!(summary.mapping.len(), model.len());
                    gone.extend(model.iter().filter(|s| s.deleted).map(|s| s.uid));
                    model.retain(|s| !s.deleted);
                }
                Op::Reopen => {
                    // The reopened handle builds its map of ids anew.
                    drop(db.take());
                    db = Some(Mutex::new(Database::open(&path).unwrap()));
                }
            }

            let d = db.as_mut().unwrap().get_mut().unwrap();
            prop_assert_eq!(d.count() as usize, model.len());
            prop_assert_eq!(all_uids(d), model.iter().map(|s| s.uid).collect::<Vec<_>>());
            for (slot, s) in model.iter().enumerate() {
                prop_assert_eq!(d.game_by_uid(s.uid).unwrap(), Some(slot as GameId));
                prop_assert_eq!(d.origin(slot as GameId).unwrap(), s.origin);
                prop_assert_eq!(d.row(slot as GameId).unwrap().deleted, s.deleted);
            }
            for uid in gone.iter().chain(&source_uids) {
                prop_assert_eq!(d.game_by_uid(*uid).unwrap(), None);
            }
        }
    }
}
