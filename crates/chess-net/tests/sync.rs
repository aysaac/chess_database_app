//! NET-13's sync end to end against the stand-in Lichess (`standin/mod.rs`),
//! which fills in tags as lichess.org does (`fills`) and adds its result
//! comment: a database linked to a new study and to an existing one (a
//! static copy upgraded, by its ChapterURLs or its moves), new games here
//! going to the study, edits going each way, a clean merge, a conflict left
//! for the user and its three answers, chapters removed on each side,
//! following only, the login and its scopes, a 429 waited out, Lichess's
//! cache, games open in a tab, a compaction, running it again.

mod standin;

use std::sync::Mutex;
use std::time::Duration;

use chess_core::game::Game;
use chess_core::pgn::{read_pgn_str, write_game};
use chess_db::{Database, GameId, NoProgress, PreparedGame, SyncMode};
use chess_net::oauth::Token;
use chess_net::study::sync::{
    self, CONFLICT_COPY_TAG, Choice, Options, PendingKind, Plan, Step, Target, link_state,
};
use chess_net::study::{ExportTarget, StudyRef, Visibility, download_study_into, export_to_studies};
use chess_net::{Client, NetError};
use standin::Lichess;

const TOKEN: &str = "lio_sync";

fn server() -> Lichess {
    let s = Lichess::start(0);
    s.add_token("Owner", TOKEN, "study:read,study:write");
    {
        let mut st = s.state.lock().unwrap();
        st.fills = true;
        st.rewrite = Some(with_result_comment);
    }
    s
}

/// What lichess.org does to a game coming in that the stand-in doesn't:
/// its result comment on the main line's last move of a finished game.
fn with_result_comment(pgn: &str) -> String {
    let Some(mut g) = read_pgn_str(pgn).into_iter().next() else { return pgn.to_owned() };
    let last = g.line_end(chess_core::game::NodeId::ROOT);
    let text = match g.headers.result.as_str() {
        "1-0" => "1-0 White wins.",
        "0-1" => "0-1 Black wins.",
        "1/2-1/2" => "1/2-1/2 The game is a draw.",
        _ => "",
    };
    if !text.is_empty() && g.node(last).comment_after.is_empty() {
        g.node_mut(last).comment_after = text.to_owned();
    }
    write_game(&g)
}

fn client(server: &Lichess) -> Client {
    Client::with_servers(&server.url, &server.url)
        .with_retry_wait(Duration::from_millis(20))
        .with_fresh_wait(Duration::from_millis(20))
}

fn token() -> Token {
    Token::new(TOKEN)
}

fn new_db() -> (tempfile::TempDir, Mutex<Database>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(&dir.path().join("Synced")).unwrap();
    (dir, Mutex::new(db))
}

const GAMES: [&str; 3] = [
    "[Event \"Club\"]\n[White \"Ann\"]\n[Black \"Bob\"]\n[Result \"1-0\"]\n\n\
     1. e4 { [%eval 0.3] } e5 2. Nf3 Nc6 3. Bb5 a6 1-0",
    "[White \"Cid\"]\n[Black \"Dee\"]\n[Result \"*\"]\n[ECO \"D06\"]\n\n1. d4 d5 2. c4 { [%clk 0:04:57.5] } *",
    "{ A position to study } 1. c4 e5 2. Nc3 Nf6 *",
];

fn add_games(db: &Mutex<Database>, pgns: &[&str]) {
    let mut db = db.lock().unwrap();
    for p in pgns {
        db.append(&read_pgn_str(p).remove(0)).unwrap();
    }
}

fn no_open() -> Vec<GameId> {
    Vec::new()
}

fn plan(server: &Lichess, db: &Mutex<Database>, target: &Target) -> Plan {
    sync::plan(&client(server), Some(&token()), db, target, &no_open, &NoProgress).unwrap()
}

fn apply(server: &Lichess, db: &Mutex<Database>, plan: &Plan) -> sync::Outcome {
    let o = sync::apply(&client(server), Some(&token()), db, plan, Options::default(), &no_open, &NoProgress)
        .unwrap();
    assert!(o.stopped.is_none(), "{:?}", o.stopped);
    o
}

fn two_way(study: &str) -> Target {
    Target::Study { study: study.into(), mode: SyncMode::TwoWay }
}

fn steps(plan: &Plan) -> Vec<Step> {
    plan.items.iter().map(|i| i.step).filter(|s| *s != Step::Unchanged).collect()
}

/// Links the three games to a new study; its id.
fn linked(server: &Lichess, db: &Mutex<Database>) -> String {
    add_games(db, &GAMES);
    let target =
        Target::New { name: "Synced games".into(), visibility: Visibility::Private, mode: SyncMode::TwoWay };
    let p = plan(server, db, &target);
    let o = apply(server, db, &p);
    o.study
}

fn chapter_pgn(server: &Lichess, study: &str, chapter: &str) -> String {
    server.study(study).unwrap().chapters.iter().find(|(c, _)| c == chapter).unwrap().1.clone()
}

fn chapter_of(db: &Mutex<Database>, slot: GameId) -> String {
    let mut db = db.lock().unwrap();
    let uid = db.uid(slot).unwrap();
    db.link(uid).unwrap().unwrap().chapter
}

fn game(db: &Mutex<Database>, slot: GameId) -> Game {
    db.lock().unwrap().read_game(slot).unwrap()
}

fn posts(server: &Lichess) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter(|r| r.starts_with("POST /api/study/") || r.starts_with("DELETE"))
        .collect()
}

#[test]
fn a_database_linked_to_a_new_study_then_its_new_games() {
    let server = server();
    let (_dir, db) = new_db();
    add_games(&db, &GAMES);
    let target =
        Target::New { name: "Synced games".into(), visibility: Visibility::Private, mode: SyncMode::TwoWay };
    let p = plan(&server, &db, &target);
    assert_eq!(steps(&p), [Step::NewHere; 3]);
    assert_eq!((p.study.as_deref(), p.name.as_str(), p.room), (None, "Synced games", 61));
    assert!(server.studies_of("Owner").is_empty(), "the preview makes nothing");

    let o = apply(&server, &db, &p);
    assert!(o.made);
    assert_eq!(o.count(Step::NewHere), 3);
    let study = server.study(&o.study).unwrap();
    assert_eq!((study.visibility.as_str(), study.chapters.len(), study.empty_first), ("private", 3, false));
    // Only what Lichess holds went: no eval, no ECO, the clock to the second.
    let first = &study.chapters[0].1;
    assert!(!first.contains("%eval") && first.contains("1. e4"), "{first}");
    assert!(chapter_pgn(&server, &o.study, &study.chapters[1].0).contains("[%clk 0:04:58]"));
    assert!(!study.chapters[1].1.contains("D06"));
    // Each game linked to its chapter, in order; the evals stay here.
    for (slot, (ch, _)) in study.chapters.iter().enumerate() {
        assert_eq!(&chapter_of(&db, slot as GameId), ch);
    }
    assert!(write_game(&game(&db, 0)).contains("%eval 0.3"));
    assert_eq!(game(&db, 1).headers.eco, "D06");

    // Again: nothing to do, though Lichess filled in Events and Dates, and
    // added its result comment.
    let again = plan(&server, &db, &two_way(&o.study));
    assert!(steps(&again).is_empty(), "{:?}", again.items);
    let before = posts(&server).len();
    apply(&server, &db, &again);
    assert_eq!(posts(&server).len(), before, "nothing sent");

    // The user's case: new games here go to the study.
    add_games(&db, &["[White \"Eve\"]\n[Black \"Fay\"]\n[Result \"0-1\"]\n\n1. f3 e5 2. g4 Qh4# 0-1"]);
    let p = plan(&server, &db, &two_way(&o.study));
    assert_eq!(steps(&p), [Step::NewHere]);
    assert_eq!(p.items.iter().find(|i| i.step == Step::NewHere).unwrap().name, "Eve - Fay");
    let o2 = apply(&server, &db, &p);
    assert_eq!(o2.count(Step::NewHere), 1);
    let study = server.study(&o.study).unwrap();
    assert_eq!(study.chapters.len(), 4);
    assert_eq!(chapter_of(&db, 3), study.chapters[3].0);
    assert!(steps(&plan(&server, &db, &two_way(&o.study))).is_empty());
}

#[test]
fn an_edit_here_goes_to_lichess_and_one_there_comes_here() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    let ch = chapter_of(&db, 0);

    // Here: a comment and a NAG, and an eval only this database holds.
    let mut g = game(&db, 0);
    let e5 = g.node(g.root().children[0]).children[0];
    g.node_mut(e5).comment_after = "The classical reply".into();
    g.node_mut(e5).commands.push(("eval".into(), "0.25".into()));
    db.lock().unwrap().replace(0, &g).unwrap();
    let p = plan(&server, &db, &two_way(&study));
    assert_eq!(steps(&p), [Step::Push]);
    let o = apply(&server, &db, &p);
    assert_eq!(o.count(Step::Push), 1);
    let there = chapter_pgn(&server, &study, &ch);
    assert!(there.contains("The classical reply") && !there.contains("0.25"), "{there}");
    assert!(there.contains("[ChapterURL"), "its tags stay");
    assert!(steps(&plan(&server, &db, &two_way(&study))).is_empty(), "the base is what Lichess has now");

    // There: a new line and another comment; here the evals stay.
    server.edit_chapter(&study, &ch, |pgn| {
        pgn.replace("2. Nf3", "2. Nf3 { Develops }").replace("3. Bb5", "3. Bb5 (3. Bc4 Bc5)")
    });
    let p = plan(&server, &db, &two_way(&study));
    assert_eq!(steps(&p), [Step::Pull]);
    let before = posts(&server).len();
    apply(&server, &db, &p);
    assert_eq!(posts(&server).len(), before, "a pull sends nothing");
    let here = write_game(&game(&db, 0));
    assert!(here.contains("Develops") && here.contains("3. Bc4"), "{here}");
    assert!(here.contains("%eval 0.3") && here.contains("%eval 0.25"), "{here}");
    assert_eq!(game(&db, 0).headers.event, "Club");
    assert!(steps(&plan(&server, &db, &two_way(&study))).is_empty());
}

#[test]
fn edits_on_both_sides_merge_and_go_both_ways() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    let ch = chapter_of(&db, 0);
    // Here: a comment on 1.e4. There: a line at move 3.
    let mut g = game(&db, 0);
    let e4 = g.root().children[0];
    g.node_mut(e4).comment_after = "Best by test".into();
    db.lock().unwrap().replace(0, &g).unwrap();
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("3. Bb5", "3. Bb5 (3. Bc4)"));
    let p = plan(&server, &db, &two_way(&study));
    assert_eq!(steps(&p), [Step::Merge]);
    apply(&server, &db, &p);
    let here = write_game(&game(&db, 0));
    let there = chapter_pgn(&server, &study, &ch);
    for side in [&here, &there] {
        assert!(side.contains("Best by test") && side.contains("3. Bc4"), "{side}");
    }
    assert!(here.contains("%eval 0.3") && !there.contains("%eval"));
    assert!(steps(&plan(&server, &db, &two_way(&study))).is_empty());
}

#[test]
fn a_conflict_is_left_for_the_user_never_overwritten() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    let ch = chapter_of(&db, 0);
    let mut g = game(&db, 0);
    let e4 = g.root().children[0];
    g.node_mut(e4).comment_after = "Mine".into();
    db.lock().unwrap().replace(0, &g).unwrap();
    let mine = game(&db, 0);
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("1. e4", "1. e4 { Theirs }"));
    let theirs_text = chapter_pgn(&server, &study, &ch);

    let p = plan(&server, &db, &two_way(&study));
    assert_eq!(steps(&p), [Step::Conflict]);
    let item = p.items.iter().find(|i| i.step == Step::Conflict).unwrap();
    assert_eq!(item.note.as_deref(), Some("1.e4: comment"));
    let before = posts(&server).len();
    let o = apply(&server, &db, &p);
    assert_eq!(o.count(Step::Conflict), 1);
    assert_eq!(posts(&server).len(), before, "nothing sent");
    assert_eq!(game(&db, 0), mine, "nothing changed here");
    assert_eq!(chapter_pgn(&server, &study, &ch), theirs_text, "nor there");
    // It waits, and the next sync leaves it.
    let uid = db.lock().unwrap().uid(0).unwrap();
    let w = sync::waiting(&mut db.lock().unwrap(), uid).unwrap().unwrap();
    assert_eq!((w.pending.kind, w.questions.len()), (PendingKind::Both, 1));
    assert_eq!((w.questions[0].mine.as_str(), w.questions[0].theirs.as_str()), ("Mine", "Theirs"));
    assert_eq!(steps(&plan(&server, &db, &two_way(&study))), [Step::Pending]);
    let studies = sync::linked_studies(&mut db.lock().unwrap()).unwrap();
    assert_eq!((studies.len(), studies[0].games, studies[0].pending.len()), (1, 3, 1));

    // Keep both: Lichess's here, ours as a marked copy, which no sync sends.
    let c = client(&server);
    let msg = sync::resolve(&c, Some(&token()), &db, uid, Choice::Both, &no_open, &NoProgress).unwrap();
    assert!(msg.contains("copy"), "{msg}");
    assert!(write_game(&game(&db, 0)).contains("Theirs") && !write_game(&game(&db, 0)).contains("Mine"));
    assert!(write_game(&game(&db, 0)).contains("%eval 0.3"), "what only we hold stays");
    let copy = game(&db, 3);
    assert!(write_game(&copy).contains("Mine"));
    assert!(copy.headers.get_extra(CONFLICT_COPY_TAG).unwrap().ends_with(&ch));
    let p = plan(&server, &db, &two_way(&study));
    assert_eq!(steps(&p), [Step::NotSent]);
    assert_eq!(
        p.items.iter().find(|i| i.step == Step::NotSent).unwrap().note.as_deref(),
        Some("a copy kept from a conflict")
    );

    // Another conflict, settled with Mine: it goes to Lichess.
    let mut g = game(&db, 0);
    let e4 = g.root().children[0];
    g.node_mut(e4).comment_after = "Mine again".into();
    db.lock().unwrap().replace(0, &g).unwrap();
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("Theirs", "Theirs again"));
    apply(&server, &db, &plan(&server, &db, &two_way(&study)));
    // Changed on Lichess once more before the answer: it waits again.
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("Theirs again", "Theirs thrice"));
    let e = sync::resolve(&c, Some(&token()), &db, uid, Choice::Mine, &no_open, &NoProgress).unwrap_err();
    assert!(e.to_string().contains("changed on Lichess again"), "{e}");
    assert!(chapter_pgn(&server, &study, &ch).contains("Theirs thrice"), "not overwritten");
    sync::resolve(&c, Some(&token()), &db, uid, Choice::Mine, &no_open, &NoProgress).unwrap();
    let there = chapter_pgn(&server, &study, &ch);
    assert!(there.contains("Mine again") && !there.contains("Theirs"), "{there}");
    assert_eq!(steps(&plan(&server, &db, &two_way(&study))), [Step::NotSent], "only the copy");

    // Theirs.
    let mut g = game(&db, 0);
    let e4 = g.root().children[0];
    g.node_mut(e4).comment_after = "Mine once more".into();
    db.lock().unwrap().replace(0, &g).unwrap();
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("Mine again", "Theirs at last"));
    apply(&server, &db, &plan(&server, &db, &two_way(&study)));
    sync::resolve(&c, Some(&token()), &db, uid, Choice::Theirs, &no_open, &NoProgress).unwrap();
    assert!(write_game(&game(&db, 0)).contains("Theirs at last"));
    assert_eq!(db.lock().unwrap().count(), 4, "no copy");
    assert_eq!(steps(&plan(&server, &db, &two_way(&study))), [Step::NotSent]);
}

#[test]
fn a_large_removal_on_lichess_waits_for_the_user() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    let ch = chapter_of(&db, 0);
    let mut g = game(&db, 0);
    let e4 = g.root().children[0];
    g.node_mut(e4).comment_after = "Here".into();
    db.lock().unwrap().replace(0, &g).unwrap();
    // Lichess cut the main line after 2.Nf3.
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("Nc6 3. Bb5 a6", ""));
    let p = plan(&server, &db, &two_way(&study));
    assert_eq!(steps(&p), [Step::Conflict]);
    let note = p.items.iter().find(|i| i.step == Step::Conflict).unwrap().note.clone().unwrap();
    assert!(note.contains("would take 3 moves of this game away"), "{note}");
}

#[test]
fn a_static_copy_upgraded_to_two_way() {
    let server = server();
    server.add_study("Shared01", "Shared lines", "Owner", "private", &GAMES);
    let (_dir, db) = new_db();
    let c = client(&server);
    let link = StudyRef { study: "Shared01".into(), chapter: None };
    download_study_into(&c, &db, &link, Some(&token()), None, &NoProgress).unwrap();
    assert!(db.lock().unwrap().links().unwrap().is_empty(), "a static copy has no links");
    // One game edited here since the import, one added.
    let mut g = game(&db, 1);
    let first = g.root().children[0];
    g.node_mut(first).comment_after = "edited here".into();
    db.lock().unwrap().replace(1, &g).unwrap();
    add_games(&db, &["1. g3 d5 *"]);

    let p = plan(&server, &db, &two_way("Shared01"));
    assert_eq!(p.name, "Shared lines");
    let mut s = steps(&p);
    s.sort_by_key(|s| format!("{s:?}"));
    assert_eq!(s, [Step::Adopt, Step::Adopt, Step::Conflict, Step::NewHere]);
    let o = apply(&server, &db, &p);
    assert_eq!((o.count(Step::Adopt), o.count(Step::Conflict), o.count(Step::NewHere)), (2, 1, 1));
    assert_eq!(server.study("Shared01").unwrap().chapters.len(), 4);
    let uid = db.lock().unwrap().uid(1).unwrap();
    let w = sync::waiting(&mut db.lock().unwrap(), uid).unwrap().unwrap();
    assert_eq!(w.pending.kind, PendingKind::NoBase);
    // Mine: the edit goes.
    sync::resolve(&c, Some(&token()), &db, uid, Choice::Mine, &no_open, &NoProgress).unwrap();
    let ch = chapter_of(&db, 1);
    assert!(chapter_pgn(&server, "Shared01", &ch).contains("edited here"));
    assert!(steps(&plan(&server, &db, &two_way("Shared01"))).is_empty());
}

#[test]
fn games_sent_without_their_links_are_linked_not_sent_twice() {
    // A sync cut short after Lichess took the games: here, NET-12's Send to
    // study, which records no links.
    let server = server();
    server.add_study("Target01", "Target", "Owner", "private", &[]);
    let (_dir, db) = new_db();
    add_games(&db, &GAMES);
    let pgns: Vec<String> = (0..3).map(|i| write_game(&game(&db, i))).collect();
    let c = client(&server);
    export_to_studies(&c, &token(), &ExportTarget::Existing("Target01".into()), &pgns, &NoProgress).unwrap();
    assert_eq!(server.study("Target01").unwrap().chapters.len(), 3);
    let p = plan(&server, &db, &two_way("Target01"));
    assert_eq!(steps(&p), [Step::Adopt; 3]);
    apply(&server, &db, &p);
    assert_eq!(server.study("Target01").unwrap().chapters.len(), 3, "none sent again");
    assert_eq!(db.lock().unwrap().links().unwrap().len(), 3);
}

#[test]
fn chapters_removed_on_each_side() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    let (ch0, ch1, ch2) = (chapter_of(&db, 0), chapter_of(&db, 1), chapter_of(&db, 2));
    // Removed on Lichess: marked as deleted here. Removed there and changed
    // here: left for the user.
    server.remove_chapter(&study, &ch0);
    server.remove_chapter(&study, &ch1);
    let mut g = game(&db, 1);
    g.headers.white = "Cid Jr".into();
    db.lock().unwrap().replace(1, &g).unwrap();
    // Deleted here: the chapter stays unless asked.
    db.lock().unwrap().set_deleted(2, true).unwrap();
    let p = plan(&server, &db, &two_way(&study));
    assert_eq!(steps(&p), [Step::RemovedThere, Step::Conflict, Step::DeletedHere]);
    apply(&server, &db, &p);
    assert!(db.lock().unwrap().record(0).unwrap().is_deleted());
    assert!(!db.lock().unwrap().record(1).unwrap().is_deleted());
    assert_eq!(server.study(&study).unwrap().chapters.len(), 1);
    let o = sync::apply(
        &client(&server),
        Some(&token()),
        &db,
        &plan(&server, &db, &two_way(&study)),
        Options { delete_chapters: true },
        &no_open,
        &NoProgress,
    )
    .unwrap();
    assert_eq!(o.count(Step::DeleteChapter), 1);
    assert!(server.study(&study).unwrap().chapters.iter().all(|(c, _)| *c != ch2));
    // The one that changed here: Mine puts it back as a new chapter.
    let uid = db.lock().unwrap().uid(1).unwrap();
    let w = sync::waiting(&mut db.lock().unwrap(), uid).unwrap().unwrap();
    assert_eq!(w.pending.kind, PendingKind::RemovedThere);
    sync::resolve(&client(&server), Some(&token()), &db, uid, Choice::Mine, &no_open, &NoProgress).unwrap();
    let chapters = server.study(&study).unwrap().chapters;
    assert!(chapters.iter().any(|(c, pgn)| *c == chapter_of(&db, 1) && pgn.contains("Cid Jr")));
    assert!(steps(&plan(&server, &db, &two_way(&study))).is_empty());
}

#[test]
fn following_only_never_sends() {
    let server = server();
    server.add_study("Follow01", "Followed", "Owner", "public", &GAMES);
    let (_dir, db) = new_db();
    let follow = Target::Study { study: "Follow01".into(), mode: SyncMode::Follow };
    // No login needed for a public study.
    let p = sync::plan(&client(&server), None, &db, &follow, &no_open, &NoProgress).unwrap();
    assert_eq!(steps(&p), [Step::NewThere; 3]);
    let o = sync::apply(&client(&server), None, &db, &p, Options::default(), &no_open, &NoProgress).unwrap();
    assert_eq!(o.count(Step::NewThere), 3);
    assert_eq!(db.lock().unwrap().count(), 3);
    assert!(db.lock().unwrap().links().unwrap().iter().all(|l| l.mode == SyncMode::Follow));
    // Edited here: kept here. A game added here: not sent.
    let mut g = game(&db, 0);
    g.headers.white = "Annie".into();
    db.lock().unwrap().replace(0, &g).unwrap();
    add_games(&db, &["1. b3 *"]);
    let ch1 = chapter_of(&db, 1);
    server.edit_chapter("Follow01", &ch1, |pgn| pgn.replace("2. c4", "2. c4 { The Queen's Gambit }"));
    let p = sync::plan(&client(&server), None, &db, &follow, &no_open, &NoProgress).unwrap();
    assert_eq!(steps(&p), [Step::KeptHere, Step::Pull, Step::NotSent]);
    sync::apply(&client(&server), None, &db, &p, Options::default(), &no_open, &NoProgress).unwrap();
    assert!(write_game(&game(&db, 1)).contains("Queen's Gambit"));
    assert_eq!(game(&db, 0).headers.white, "Annie");
    assert!(posts(&server).is_empty(), "{:?}", posts(&server));
    // Two-way from now on: the edit goes.
    assert_eq!(sync::set_mode(&mut db.lock().unwrap(), "Follow01", SyncMode::TwoWay).unwrap(), 3);
    let p = plan(&server, &db, &two_way("Follow01"));
    assert_eq!(steps(&p), [Step::Push, Step::NewHere]);
    // Unlinked: a static copy again.
    assert_eq!(sync::unlink(&mut db.lock().unwrap(), "Follow01").unwrap(), 3);
    assert!(db.lock().unwrap().links().unwrap().is_empty());
}

#[test]
fn the_login_and_its_scopes_are_checked_before_anything() {
    let server = server();
    server.add_token("Owner", "lio_read", "study:read");
    let (_dir, db) = new_db();
    add_games(&db, &GAMES);
    let target =
        Target::New { name: "Synced".into(), visibility: Visibility::Private, mode: SyncMode::TwoWay };
    let c = client(&server);
    let e = sync::plan(&c, None, &db, &target, &no_open, &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::SyncNeedsLogin), "{e}");
    let e = sync::plan(&c, Some(&Token::new("lio_read")), &db, &target, &no_open, &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::MissingScopes(ref s) if s == "study:write"), "{e}");
    let e = sync::plan(&c, Some(&Token::new("lio_gone")), &db, &target, &no_open, &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::TokenRejected), "{e}");
    let p = plan(&server, &db, &target);
    let e =
        sync::apply(&c, Some(&Token::new("lio_read")), &db, &p, Options::default(), &no_open, &NoProgress)
            .unwrap_err();
    assert!(matches!(e, NetError::MissingScopes(_)));
    assert!(server.studies_of("Owner").is_empty(), "nothing made");
    assert!(db.lock().unwrap().links().unwrap().is_empty());
}

#[test]
fn a_429_is_waited_out_and_sent_again() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    add_games(&db, &["1. a3 *"]);
    {
        let mut st = server.state.lock().unwrap();
        st.retry_after = Some(0);
        st.fail.push((format!("POST /api/study/{study}/import-pgn"), 429));
    }
    let o = apply(&server, &db, &plan(&server, &db, &two_way(&study)));
    assert_eq!(o.count(Step::NewHere), 1);
    let imports = posts(&server).iter().filter(|r| r.contains("import-pgn")).count();
    assert_eq!(imports, 3, "the link's, the refused one and the one sent again");
    assert_eq!(server.study(&study).unwrap().chapters.len(), 4);

    // One that keeps failing ends the sync, and says why.
    add_games(&db, &["1. a4 *"]);
    {
        let mut st = server.state.lock().unwrap();
        for _ in 0..4 {
            st.fail.push((format!("POST /api/study/{study}/import-pgn"), 429));
        }
    }
    let o = sync::apply(
        &client(&server),
        Some(&token()),
        &db,
        &plan(&server, &db, &two_way(&study)),
        Options::default(),
        &no_open,
        &NoProgress,
    )
    .unwrap();
    assert!(matches!(o.stopped, Some(NetError::RateLimited(_))), "{:?}", o.stopped);
    assert_eq!(server.study(&study).unwrap().chapters.len(), 4);
    assert_eq!(steps(&plan(&server, &db, &two_way(&study))), [Step::NewHere], "the next sync sends it");
}

#[test]
fn lichess_s_cache_is_waited_out_for_the_new_base() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    let ch = chapter_of(&db, 0);
    let mut g = game(&db, 0);
    let e4 = g.root().children[0];
    g.node_mut(e4).comment_after = "Pushed".into();
    db.lock().unwrap().replace(0, &g).unwrap();
    let p = plan(&server, &db, &two_way(&study));
    // The export after the push is the cache's (before it), twice: the 2
    // exports of the apply before the push and the first one after.
    server.serve_stale(&study, 2);
    apply(&server, &db, &p);
    let uid = db.lock().unwrap().uid(0).unwrap();
    let link = db.lock().unwrap().link(uid).unwrap().unwrap();
    assert!(
        link.base.contains("Pushed") && link.base.contains("[ChapterURL"),
        "Lichess's own export: {}",
        link.base
    );
    assert!(chapter_pgn(&server, &study, &ch).contains("Pushed"));
    assert!(steps(&plan(&server, &db, &two_way(&study))).is_empty());
}

#[test]
fn games_open_in_a_tab_are_left_alone() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    let ch = chapter_of(&db, 0);
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("2. Nf3", "2. Nf3 { From Lichess }"));
    let open = || vec![0];
    let c = client(&server);
    let p = sync::plan(&c, Some(&token()), &db, &two_way(&study), &open, &NoProgress).unwrap();
    assert_eq!(steps(&p), [Step::Open]);
    let o = sync::apply(&c, Some(&token()), &db, &p, Options::default(), &open, &NoProgress).unwrap();
    assert_eq!(o.count(Step::Pull), 0);
    assert!(!write_game(&game(&db, 0)).contains("From Lichess"));
    // Closed: it comes.
    apply(&server, &db, &plan(&server, &db, &two_way(&study)));
    assert!(write_game(&game(&db, 0)).contains("From Lichess"));
}

#[test]
fn links_follow_their_games_through_a_compaction() {
    let server = server();
    let (_dir, db) = new_db();
    // A game first that's deleted and compacted away: the others move.
    add_games(&db, &["1. h4 *"]);
    let study = linked(&server, &db);
    let p = plan(&server, &db, &two_way(&study));
    assert!(steps(&p).is_empty(), "the first game is in the study too");
    let ch = chapter_of(&db, 1);
    db.lock().unwrap().set_deleted(0, true).unwrap();
    // Deleted here, kept there; then compacted: the link goes with the game.
    apply(&server, &db, &plan(&server, &db, &two_way(&study)));
    db.lock().unwrap().compact(&NoProgress).unwrap();
    assert_eq!(chapter_of(&db, 0), ch, "the first game left is the one linked to that chapter");
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("1. e4", "1. e4 { After the compaction }"));
    let p = plan(&server, &db, &two_way(&study));
    // The compacted game's chapter comes back as a new game (its link went
    // with it); Ann - Bob's edit comes to it.
    let mut s = steps(&p);
    s.sort_by_key(|s| format!("{s:?}"));
    assert_eq!(s, [Step::NewThere, Step::Pull]);
    apply(&server, &db, &p);
    assert!(write_game(&game(&db, 0)).contains("After the compaction"));
}

#[test]
fn a_pending_merge_is_kept_with_the_link() {
    let server = server();
    let (_dir, db) = new_db();
    let study = linked(&server, &db);
    let ch = chapter_of(&db, 0);
    let mut g = game(&db, 0);
    g.headers.white = "Ann B".into();
    db.lock().unwrap().replace(0, &g).unwrap();
    server.edit_chapter(&study, &ch, |pgn| pgn.replace("[White \"Ann\"]", "[White \"Ann C\"]"));
    apply(&server, &db, &plan(&server, &db, &two_way(&study)));
    let uid = db.lock().unwrap().uid(0).unwrap();
    let link = db.lock().unwrap().link(uid).unwrap().unwrap();
    let (_, pending) = link_state(&link).unwrap();
    let pending = pending.unwrap();
    assert!(pending.theirs.contains("Ann C"));
    assert!(!link.base.contains("Ann C"), "the base is still the last one both sides had");
    // A copy of the game into another database isn't linked, nor waits.
    let (_dir2, other) = new_db();
    let prepared = PreparedGame::new(&game(&db, 0));
    other.lock().unwrap().append_prepared(&[prepared]).unwrap();
    assert!(other.lock().unwrap().links().unwrap().is_empty());
}
