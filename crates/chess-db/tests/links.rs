//! Links from games to Lichess study chapters (NET-13, `games.links`): set,
//! found, listed and removed by permanent id and by chapter; kept through
//! edits, deletion, undeletion and compaction; not copied; what an app
//! that doesn't know the file leaves behind; a newer file left alone; and a
//! model of a database's links through any mix of writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chess_core::game::Game;
use chess_core::pgn::read_pgn_str;
use chess_db::{
    Database, DbError, GameId, GameUid, LinkedGame, NoProgress, PreparedGame, StudyLink, SyncMode, copy_games,
};
use proptest::prelude::*;

fn game(white: &str) -> Game {
    let mut g = read_pgn_str("1. e4 e5 2. Nf3 { A comment. } *").remove(0);
    g.headers.white = white.into();
    g
}

fn link(study: &str, chapter: &str, base: &str) -> StudyLink {
    StudyLink {
        study: study.into(),
        chapter: chapter.into(),
        mode: SyncMode::TwoWay,
        last_modified: Some("Thu, 12 Nov 2026 21:00:00 GMT".into()),
        base: format!("[ChapterURL \"https://lichess.org/study/{study}/{chapter}\"]\n\n{base}"),
        layer: base.bytes().rev().collect(),
    }
}

fn db_with(dir: &Path, name: &str, n: usize) -> Database {
    let mut db = Database::create(&dir.join(name)).unwrap();
    for i in 0..n {
        db.append(&game(&format!("{name}{i}"))).unwrap();
    }
    db
}

fn chapters(list: &[LinkedGame]) -> Vec<(GameId, &str, &str)> {
    list.iter().map(|l| (l.game, l.study.as_str(), l.chapter.as_str())).collect()
}

fn len(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

#[test]
fn links_are_set_found_listed_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with(dir.path(), "Links", 4);
    let path = db.path().to_path_buf();
    assert_eq!(db.links().unwrap(), []);
    assert!(!path.join("games.links").exists(), "reading makes no file");
    let uids = db.uids(0, 4).unwrap();

    db.set_link(uids[2], &link("studyAAA", "chap0001", "1. e4 *")).unwrap();
    db.set_links(&[
        (uids[0], link("studyAAA", "chap0002", "1. d4 *")),
        (uids[3], StudyLink { mode: SyncMode::Follow, ..link("studyBBB", "chap0001", "") }),
    ])
    .unwrap();
    assert_eq!(db.link(uids[2]).unwrap(), Some(link("studyAAA", "chap0001", "1. e4 *")));
    assert_eq!(db.link(uids[1]).unwrap(), None);
    let found = db.link_by_chapter("studyBBB", "chap0001").unwrap().unwrap();
    assert_eq!((found.uid, found.game, found.mode), (uids[3], 3, SyncMode::Follow));
    assert_eq!(found.last_modified.as_deref(), Some("Thu, 12 Nov 2026 21:00:00 GMT"));
    assert_eq!(db.link_by_chapter("studyBBB", "chap0002").unwrap(), None);
    assert_eq!(db.link_by_chapter("studyXXX", "chap0001").unwrap(), None);
    assert_eq!(
        chapters(&db.links_of_study("studyAAA").unwrap()),
        [(0, "studyAAA", "chap0002"), (2, "studyAAA", "chap0001")],
        "by slot"
    );
    assert_eq!(db.links().unwrap().len(), 3);

    // A new base after a sync replaces the old one.
    let synced =
        StudyLink { last_modified: None, layer: Vec::new(), ..link("studyAAA", "chap0001", "1. e4 e5 *") };
    db.set_link(uids[2], &synced).unwrap();
    drop(db);

    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.link(uids[2]).unwrap(), Some(synced));
    assert_eq!(db.links().unwrap().len(), 3);
    assert!(db.remove_link(uids[0]).unwrap());
    assert!(!db.remove_link(uids[0]).unwrap(), "linked no more");
    assert!(db.remove_chapter_link("studyBBB", "chap0001").unwrap());
    assert!(!db.remove_chapter_link("studyBBB", "chap0001").unwrap());
    assert_eq!(db.remove_links(&[uids[1], uids[2], uids[2]]).unwrap(), 1);
    assert_eq!(db.links().unwrap(), []);
    drop(db);
    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.links().unwrap(), []);
    assert_eq!(db.count(), 4, "the games stay");
}

#[test]
fn a_game_or_a_chapter_takes_one_link() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with(dir.path(), "One", 3);
    let uids = db.uids(0, 3).unwrap();
    db.set_link(uids[0], &link("studyAAA", "chap0001", "")).unwrap();
    db.set_link(uids[1], &link("studyAAA", "chap0002", "")).unwrap();
    let refused = |r: Result<(), DbError>| assert!(matches!(r, Err(DbError::Invalid(_))), "{r:?}");
    // Another game's chapter, two games for one chapter, one game twice.
    refused(db.set_link(uids[2], &link("studyAAA", "chap0001", "")));
    refused(db.set_links(&[(uids[2], link("studyAAA", "x", "")), (uids[0], link("studyAAA", "x", ""))]));
    refused(db.set_links(&[(uids[2], link("studyAAA", "x", "")), (uids[2], link("studyAAA", "y", ""))]));
    // A game this database doesn't have, a bad id.
    let other = db_with(dir.path(), "Other", 1).uid(0).unwrap();
    refused(db.set_link(other, &link("studyAAA", "chap0003", "")));
    refused(db.set_link(uids[2], &link("study AAA", "chap0003", "")));
    assert_eq!(chapters(&db.links().unwrap()), [(0, "studyAAA", "chap0001"), (1, "studyAAA", "chap0002")]);

    // Two games' chapters swapped at once; a game moved to a free chapter.
    db.set_links(&[(uids[0], link("studyAAA", "chap0002", "")), (uids[1], link("studyAAA", "chap0001", ""))])
        .unwrap();
    db.set_link(uids[0], &link("studyAAA", "chap0003", "")).unwrap();
    assert_eq!(chapters(&db.links().unwrap()), [(0, "studyAAA", "chap0003"), (1, "studyAAA", "chap0001")]);
    // Once unlinked, its chapter can go to another game.
    db.remove_link(uids[1]).unwrap();
    db.set_link(uids[2], &link("studyAAA", "chap0001", "")).unwrap();
    assert_eq!(chapters(&db.links().unwrap()), [(0, "studyAAA", "chap0003"), (2, "studyAAA", "chap0001")]);
}

/// Edits, deletion and undeletion keep a game's link; a compaction keeps
/// the links of the games it keeps, at their new slots, and drops those of
/// the games it removes; the file shrinks.
#[test]
fn links_stay_through_edits_deletion_and_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with(dir.path(), "Keep", 5);
    let path = db.path().to_path_buf();
    let uids = db.uids(0, 5).unwrap();
    for (i, uid) in uids.iter().enumerate() {
        db.set_link(*uid, &link("studyAAA", &format!("chap000{i}"), &"1. e4 e5 ".repeat(100))).unwrap();
    }
    db.replace(1, &game("Edited")).unwrap();
    db.set_deleted_many(&[0, 3], true).unwrap();
    db.set_deleted(3, false).unwrap();
    assert_eq!(db.links().unwrap().len(), 5, "deleted games keep their links");
    assert_eq!(db.link_by_chapter("studyAAA", "chap0000").unwrap().unwrap().game, 0);
    let before = len(&path.join("games.links"));

    let summary = db.compact(&NoProgress).unwrap();
    assert_eq!(summary.mapping, [None, Some(0), Some(1), Some(2), Some(3)]);
    assert!(len(&path.join("games.links")) < before);
    let check = |db: &mut Database| {
        assert_eq!(
            chapters(&db.links().unwrap()),
            [
                (0, "studyAAA", "chap0001"),
                (1, "studyAAA", "chap0002"),
                (2, "studyAAA", "chap0003"),
                (3, "studyAAA", "chap0004")
            ]
        );
        assert_eq!(db.link_by_chapter("studyAAA", "chap0000").unwrap(), None);
        assert_eq!(db.link(uids[0]).unwrap(), None);
        assert_eq!(db.link(uids[4]).unwrap(), Some(link("studyAAA", "chap0004", &"1. e4 e5 ".repeat(100))));
        assert_eq!(db.read_game(0).unwrap().headers.white, "Edited");
    };
    check(&mut db);
    // Linking after a compaction, and after reopening.
    db.set_link(uids[1], &link("studyBBB", "chap0001", "")).unwrap();
    drop(db);
    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.link_by_chapter("studyBBB", "chap0001").unwrap().unwrap().game, 0);
    db.set_link(uids[1], &link("studyAAA", "chap0001", "")).unwrap();
    check(&mut db);
    assert!(!path.join("games.links.new").exists());
}

/// A copy into another database is another game, linked to nothing; the
/// original keeps its link. Copying it back doesn't take the chapter.
#[test]
fn a_copy_is_not_linked() {
    let dir = tempfile::tempdir().unwrap();
    let source = Mutex::new(db_with(dir.path(), "Source", 2));
    let target = Mutex::new(db_with(dir.path(), "Target", 0));
    let uid = source.lock().unwrap().uid(1).unwrap();
    source.lock().unwrap().set_link(uid, &link("studyAAA", "chap0001", "")).unwrap();
    copy_games(&source, &[0, 1], &target, &NoProgress).unwrap();
    let mut t = target.lock().unwrap();
    assert_eq!(t.links().unwrap(), []);
    assert_eq!(t.link_by_chapter("studyAAA", "chap0001").unwrap(), None);
    assert!(!t.path().join("games.links").exists());
    drop(t);
    copy_games(&target, &[1], &source, &NoProgress).unwrap();
    let mut s = source.lock().unwrap();
    assert_eq!(s.count(), 3);
    assert_eq!(chapters(&s.links().unwrap()), [(1, "studyAAA", "chap0001")]);
    let back = s.uid(2).unwrap();
    assert!(s.link(back).unwrap().is_none());
}

fn hide_links(path: &Path) -> PathBuf {
    let aside = path.with_extension("links-aside");
    std::fs::rename(path.join("games.links"), &aside).unwrap();
    aside
}

/// An app from before links (format version 3 too) leaves `games.links`
/// alone; its compaction moves the games, not the links, which stay right,
/// and leaves those of the games it removed, which nothing here finds: their
/// chapters are free, and the next compaction here drops them.
#[test]
fn what_an_app_without_links_leaves_behind() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with(dir.path(), "Old", 3);
    let path = db.path().to_path_buf();
    let uids = db.uids(0, 3).unwrap();
    for (i, uid) in uids.iter().enumerate() {
        db.set_link(*uid, &link("studyAAA", &format!("chap000{i}"), "")).unwrap();
    }
    let links_file = std::fs::read(path.join("games.links")).unwrap();
    // The older app: it edits, deletes, appends and compacts, and never
    // looks at `games.links` (here: the file is away meanwhile).
    db.replace(2, &game("Old app's edit")).unwrap();
    db.set_deleted(0, true).unwrap();
    drop(db);
    let aside = hide_links(&path);
    let mut old = Database::open(&path).unwrap();
    old.append(&game("Old app's game")).unwrap();
    old.compact(&NoProgress).unwrap();
    drop(old);
    std::fs::rename(&aside, path.join("games.links")).unwrap();
    assert_eq!(std::fs::read(path.join("games.links")).unwrap(), links_file, "untouched");

    let mut db = Database::open(&path).unwrap();
    assert_eq!(chapters(&db.links().unwrap()), [(0, "studyAAA", "chap0001"), (1, "studyAAA", "chap0002")]);
    assert_eq!(db.read_game(1).unwrap().headers.white, "Old app's edit");
    assert_eq!(db.link(uids[0]).unwrap(), None, "its game is gone");
    assert_eq!(db.link_by_chapter("studyAAA", "chap0000").unwrap(), None);
    // The chapter left behind can be linked again.
    let new = db.uid(2).unwrap();
    db.set_link(new, &link("studyAAA", "chap0000", "")).unwrap();
    assert_eq!(db.link_by_chapter("studyAAA", "chap0000").unwrap().unwrap().uid, new);
    db.remove_link(new).unwrap();
    let before = len(&path.join("games.links"));
    db.compact(&NoProgress).unwrap();
    assert!(len(&path.join("games.links")) < before);
    assert_eq!(db.links().unwrap().len(), 2);
}

/// A `games.links` of a newer version: the link calls say so, everything
/// else works, and a compaction leaves the file exactly as it was.
#[test]
fn a_newer_links_file_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with(dir.path(), "Newer", 2);
    let path = db.path().to_path_buf();
    let uid = db.uid(1).unwrap();
    db.set_link(uid, &link("studyAAA", "chap0001", "")).unwrap();
    drop(db);
    let mut bytes = std::fs::read(path.join("games.links")).unwrap();
    bytes[8] = 2;
    std::fs::write(path.join("games.links"), &bytes).unwrap();

    let mut db = Database::open(&path).unwrap();
    assert!(matches!(db.links(), Err(DbError::UnsupportedVersion(2))));
    assert!(matches!(db.link(uid), Err(DbError::UnsupportedVersion(2))));
    db.append(&game("New")).unwrap();
    db.set_deleted(0, true).unwrap();
    assert_eq!(db.compact(&NoProgress).unwrap().kept, 2);
    assert_eq!(std::fs::read(path.join("games.links")).unwrap(), bytes);
    assert_eq!(db.game_by_uid(uid).unwrap(), Some(0));
}

/// A database set back to version 2 by hand (to open it in an app from
/// before permanent ids) gets new ids on its next upgrade: links naming the
/// old ones are dropped with them.
#[test]
fn an_upgrade_drops_links_to_ids_it_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with(dir.path(), "Down", 2);
    let path = db.path().to_path_buf();
    db.set_link(db.uid(0).unwrap(), &link("studyAAA", "chap0001", "")).unwrap();
    drop(db);
    let meta = std::fs::read_to_string(path.join("db.meta")).unwrap();
    std::fs::write(path.join("db.meta"), meta.replace("chessdb 3", "chessdb 2")).unwrap();
    let mut db = Database::open(&path).unwrap();
    assert!(!path.join("games.links").exists());
    assert_eq!(db.links().unwrap(), []);
    // A chapter is free again.
    db.set_link(db.uid(1).unwrap(), &link("studyAAA", "chap0001", "")).unwrap();
    assert_eq!(db.link_by_chapter("studyAAA", "chap0001").unwrap().unwrap().game, 1);
}

/// A compaction cut short: before its commit marker, the new links file is
/// deleted; after it, it's renamed in place with the others.
#[test]
fn a_compaction_cut_short_finishes_or_drops_its_links() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with(dir.path(), "Cut", 2);
    let path = db.path().to_path_buf();
    let uids = db.uids(0, 2).unwrap();
    db.set_link(uids[0], &link("studyAAA", "chap0001", "")).unwrap();
    db.set_link(uids[1], &link("studyAAA", "chap0002", "")).unwrap();
    drop(db);
    // Not committed: an empty (wrong) links file is left over, and deleted.
    std::fs::write(path.join("games.links.new"), b"partial").unwrap();
    let mut db = Database::open(&path).unwrap();
    assert!(!path.join("games.links.new").exists());
    assert_eq!(db.links().unwrap().len(), 2);
    drop(db);

    // Committed: what a compaction removing game 0 writes, made by one of a
    // copy of the database, takes the place of every file.
    let copy = dir.path().join("Copy.chessdb");
    std::fs::create_dir(&copy).unwrap();
    for entry in std::fs::read_dir(&path).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
    }
    let mut c = Database::open(&copy).unwrap();
    c.set_deleted(0, true).unwrap();
    c.compact(&NoProgress).unwrap();
    drop(c);
    for f in [
        "games.idx",
        "games.blob",
        "games.ids",
        "games.links",
        "players.names",
        "events.names",
        "sites.names",
        "annotators.names",
    ] {
        std::fs::copy(copy.join(f), path.join(format!("{f}.new"))).unwrap();
    }
    std::fs::write(path.join("compact.commit"), b"compact\n").unwrap();
    let before = len(&path.join("games.links"));
    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.count(), 1);
    assert_eq!(chapters(&db.links().unwrap()), [(0, "studyAAA", "chap0002")]);
    assert!(len(&path.join("games.links")) < before);
    assert!(!path.join("games.links.new").exists() && !path.join("compact.commit").exists());
}

/// A game pulled in from a chapter comes with its link in one commit.
#[test]
fn games_appended_linked() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = db_with(dir.path(), "Pull", 1);
    let range = db
        .append_prepared(&[
            PreparedGame::new(&game("A")).with_link(link("studyAAA", "chap0001", "a")),
            PreparedGame::new(&game("B")),
            PreparedGame::new(&game("C")).with_link(link("studyAAA", "chap0002", "c")),
        ])
        .unwrap();
    assert_eq!(range, 1..4);
    assert_eq!(chapters(&db.links().unwrap()), [(1, "studyAAA", "chap0001"), (3, "studyAAA", "chap0002")]);
    assert_eq!(db.link(db.uid(3).unwrap()).unwrap(), Some(link("studyAAA", "chap0002", "c")));
    // A chapter linked already is refused, nothing written.
    let again =
        db.append_prepared(&[PreparedGame::new(&game("D")).with_link(link("studyAAA", "chap0001", ""))]);
    assert!(matches!(again, Err(DbError::Invalid(_))));
    assert_eq!(db.count(), 4);
}

// ---- A model of a database's links through any mix of writes ---------------

#[derive(Debug, Clone)]
enum Op {
    /// Games appended, the n-th linked to chapter `n` when its flag is set.
    Append(Vec<Option<u8>>),
    /// A game linked to a chapter (refused if another game has it).
    Link(u8, u8),
    Unlink(u8),
    Delete(u8),
    Undelete(u8),
    Edit(u8),
    Compact,
    Reopen,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => prop::collection::vec(prop::option::of(0u8..6), 1..4).prop_map(Op::Append),
        3 => (any::<u8>(), 0u8..6).prop_map(|(g, c)| Op::Link(g, c)),
        1 => any::<u8>().prop_map(Op::Unlink),
        2 => any::<u8>().prop_map(Op::Delete),
        1 => any::<u8>().prop_map(Op::Undelete),
        1 => any::<u8>().prop_map(Op::Edit),
        1 => Just(Op::Compact),
        1 => Just(Op::Reopen),
    ]
}

/// A game of the model.
#[derive(Debug, Clone)]
struct Slot {
    uid: GameUid,
    deleted: bool,
    /// Its chapter and the base it was last linked with.
    link: Option<(u8, String)>,
}

fn chapter(c: u8) -> String {
    format!("chap000{c}")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Through any mix of writes, every game's link is where the model says,
    /// by permanent id and by chapter, at the game's slot whatever
    /// compactions did; a chapter has at most one game; links of games
    /// compacted away are gone.
    #[test]
    fn links_follow_their_games(ops in prop::collection::vec(op(), 1..14)) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Model.chessdb");
        let mut db = Some(Database::create(&path).unwrap());
        let mut model: Vec<Slot> = Vec::new();
        let mut gone: Vec<GameUid> = Vec::new();
        let mut n = 0u32;
        for op in ops {
            let d = db.as_mut().unwrap();
            let held = |model: &[Slot], c: u8| model.iter().position(|s| s.link.as_ref().is_some_and(|(k, _)| *k == c));
            let pick = |model: &[Slot], i: u8| (!model.is_empty()).then(|| i as usize % model.len());
            match op {
                Op::Append(links) => {
                    n += 1;
                    let mut distinct: Vec<u8> = links.iter().flatten().copied().collect();
                    distinct.sort_unstable();
                    distinct.dedup();
                    let free = distinct.len() == links.iter().flatten().count()
                        && distinct.iter().all(|&c| held(&model, c).is_none());
                    let batch: Vec<PreparedGame> = links
                        .iter()
                        .map(|l| {
                            let g = PreparedGame::new(&game("G"));
                            match l {
                                Some(c) => g.with_link(link("studyAAA", &chapter(*c), &format!("{n}"))),
                                None => g,
                            }
                        })
                        .collect();
                    let result = d.append_prepared(&batch);
                    prop_assert_eq!(result.is_ok(), free, "{:?}", result);
                    if let Ok(range) = result {
                        for (id, l) in range.zip(&links) {
                            let uid = d.uid(id).unwrap();
                            model.push(Slot { uid, deleted: false, link: l.map(|c| (c, format!("{n}"))) });
                        }
                    }
                }
                Op::Link(g, c) => {
                    if let Some(i) = pick(&model, g) {
                        n += 1;
                        let result = d.set_link(model[i].uid, &link("studyAAA", &chapter(c), &format!("{n}")));
                        let free = held(&model, c).is_none_or(|h| h == i);
                        prop_assert_eq!(result.is_ok(), free, "{:?}", result);
                        if free {
                            model[i].link = Some((c, format!("{n}")));
                        }
                    }
                }
                Op::Unlink(g) => {
                    if let Some(i) = pick(&model, g) {
                        prop_assert_eq!(d.remove_link(model[i].uid).unwrap(), model[i].link.is_some());
                        model[i].link = None;
                    }
                }
                Op::Delete(g) | Op::Undelete(g) => {
                    if let Some(i) = pick(&model, g) {
                        let deleted = matches!(op, Op::Delete(_));
                        d.set_deleted(i as GameId, deleted).unwrap();
                        model[i].deleted = deleted;
                    }
                }
                Op::Edit(g) => {
                    if let Some(i) = pick(&model, g) {
                        d.replace(i as GameId, &game("Edited")).unwrap();
                    }
                }
                Op::Compact => {
                    d.compact(&NoProgress).unwrap();
                    gone.extend(model.iter().filter(|s| s.deleted).map(|s| s.uid));
                    model.retain(|s| !s.deleted);
                }
                Op::Reopen => {
                    drop(db.take());
                    db = Some(Database::open(&path).unwrap());
                }
            }

            let d = db.as_mut().unwrap();
            let mut want: HashMap<String, GameId> = HashMap::new();
            for (slot, s) in model.iter().enumerate() {
                let got = d.link(s.uid).unwrap();
                prop_assert_eq!(got.as_ref().map(|l| (l.chapter.clone(), l.base.clone())),
                    s.link.as_ref().map(|(c, n)| (chapter(*c), link("studyAAA", &chapter(*c), n).base)));
                if let Some((c, _)) = &s.link {
                    want.insert(chapter(*c), slot as GameId);
                }
            }
            let listed: HashMap<String, GameId> = d.links().unwrap().into_iter().map(|l| (l.chapter, l.game)).collect();
            prop_assert_eq!(&listed, &want);
            for c in 0..6 {
                let by_chapter = d.link_by_chapter("studyAAA", &chapter(c)).unwrap().map(|l| l.game);
                prop_assert_eq!(by_chapter, want.get(&chapter(c)).copied());
            }
            for uid in &gone {
                prop_assert_eq!(d.link(*uid).unwrap(), None);
            }
        }
    }
}
