//! NET-13's study round-trip check (`chess_net::study::roundtrip`) end to
//! end against the stand-in Lichess (`standin/mod.rs`), never the real
//! one: the fixture `testdata/pgn/study-roundtrip.pgn` to one new private
//! study and back, then a chapter's moves replaced and fetched back. What
//! the report says when the server keeps everything, when it drops clocks
//! and evals, and when replacing the moves fails.

mod standin;

use std::path::PathBuf;
use std::time::Duration;

use chess_core::game::Game;
use chess_core::pgn::read_pgn_str;
use chess_db::NoProgress;
use chess_net::oauth::Token;
use chess_net::study::roundtrip::{Feature, round_trip};
use chess_net::{Client, NetError, Site};
use standin::Lichess;

fn fixture_text() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/study-roundtrip.pgn");
    std::fs::read_to_string(path).unwrap()
}

fn fixture() -> Vec<Game> {
    read_pgn_str(&fixture_text())
}

fn client(server: &Lichess) -> Client {
    Client::with_servers(&server.url, &server.url).with_retry_wait(Duration::from_millis(20))
}

fn writer(server: &Lichess) -> Token {
    server.add_token("Owner", "lio_owner", "study:read,study:write");
    Token::new("lio_owner")
}

#[test]
fn the_whole_check_against_a_server_that_keeps_everything() {
    let server = Lichess::start(0);
    let token = writer(&server);
    let games = fixture();
    assert_eq!(games.len(), 6);
    let r = round_trip(&client(&server), &token, &games, "Round-trip check", &NoProgress).unwrap();

    // One private study, a chapter per game.
    let made = server.studies_of("Owner");
    assert_eq!(made.len(), 1, "one study, whatever happens");
    assert_eq!(made[0].visibility, "private");
    assert_eq!(made[0].chapters.len(), 6);
    let study = r.study.as_ref().expect("a study");
    assert_eq!((study.id.as_str(), study.sent, study.chapters, r.chapters), (made[0].id.as_str(), 6, 6, 6));

    // A study made, the games sent, the study fetched, the moves replaced,
    // the chapter fetched: one request each, all with the token.
    let reqs = server.requests();
    let what: Vec<String> = reqs.iter().map(|r| r.split('?').next().unwrap_or(r).to_owned()).collect();
    let id = &study.id;
    let chapter = &made[0].chapters[0].0;
    assert_eq!(
        what,
        [
            "POST /api/study [token]".to_owned(),
            format!("POST /api/study/{id}/import-pgn [token]"),
            format!("GET /api/study/{id}.pgn"),
            format!("POST /api/study/{id}/{chapter}/moves [token]"),
            format!("GET /api/study/{id}/{chapter}.pgn"),
        ]
    );
    assert!(reqs.iter().all(|r| r.ends_with("[token]")), "{reqs:?}");
    assert!(reqs[2].contains("clocks=true") && reqs[2].contains("comments=true"), "the full export");

    // The stand-in keeps what it's sent: only the Orientation it gives the
    // games without one differs. And the fixture has some of everything.
    for f in Feature::ALL {
        let t = r.total.get(f);
        assert!(t.same > 0, "{}", f.name());
        if f == Feature::Orientation {
            assert_eq!((t.changed, t.dropped, t.added), (0, 0, 3), "{:?}", t.examples);
        } else {
            assert_eq!(t.differences(), 0, "{}: {:?}", f.name(), t.examples);
        }
    }
    assert!(r.games.iter().all(|g| g.chapter.is_some() && g.diff.is_some()), "{:?}", r.games);
    assert_eq!(r.games[2].name, "Torneo Ciudad de Córdoba, Ñúñez, José María - Øvergård, Åse");
    assert!(r.notes.is_empty(), "{:?}", r.notes);

    // The first game's chapter, edited, replaced in place and fetched back
    // as it was sent.
    assert!(r.push_error.is_none(), "{:?}", r.push_error);
    let push = r.push.as_ref().expect("the moves replaced");
    assert_eq!((push.game, &push.chapter, push.kept_id, push.edits.len()), (1, chapter, true, 8));
    assert_eq!(push.diff.differences(), 0, "{:?}", push.diff);
    assert!(push.diff.get(Feature::Comments).same > 0 && push.diff.get(Feature::Clocks).same > 0);
    let now = server.study(id).unwrap();
    assert_eq!(now.chapters.len(), 6, "replaced in place");
    assert!(
        now.chapters[0].1.contains("Added by the round-trip check.")
            && now.chapters[0].1.contains("[ChapterURL")
    );
    assert!(
        now.chapters[1..].iter().all(|(_, pgn)| !pgn.contains("round-trip check")),
        "the others untouched"
    );

    // The texts of each step.
    assert!(r.texts.sent.contains("[White \"Ñúñez, José María\"]"));
    assert!(r.texts.back.contains("[StudyName \"Round-trip check\"]"));
    assert!(r.texts.pushed.contains("(edited)") && r.texts.pushed_back.contains("(edited)"));

    // The report.
    let text = r.to_string();
    for part in [
        format!("Study: {}/study/{id} (private, \"Round-trip check\")", server.url),
        "6 games sent, 6 chapters made, 6 chapters fetched back.".to_owned(),
        "game 3, white added (the game had none)".to_owned(),
        format!("game 1: Round-trip: every annotation, Student - Coach (chapter {chapter}): nothing differs"),
        "game 3: Torneo Ciudad de Córdoba".to_owned(),
        "The chapter kept its id: yes.".to_owned(),
        "Nothing differs.".to_owned(),
        "delete it on Lichess".to_owned(),
    ] {
        assert!(text.contains(&part), "{part:?} in:\n{text}");
    }
}

/// A rewrite for the stand-in: every `[%clk …]` and `[%eval …]` gone (the
/// PGN writer may break a line between a command's name and its value).
fn without_clocks_and_evals(pgn: &str) -> String {
    let mut out = String::new();
    let mut rest = pgn;
    while let Some(i) = rest.find("[%clk").into_iter().chain(rest.find("[%eval")).min() {
        out.push_str(&rest[..i]);
        rest = rest[i..].find(']').map_or("", |end| &rest[i + end + 1..]);
    }
    out.push_str(rest);
    out
}

#[test]
fn what_the_server_drops_is_reported_as_dropped() {
    let server = Lichess::start(0);
    let token = writer(&server);
    server.state.lock().unwrap().rewrite = Some(without_clocks_and_evals);
    let r = round_trip(&client(&server), &token, &fixture(), "Round-trip check", &NoProgress).unwrap();
    let text = fixture_text();
    let clocks = r.total.get(Feature::Clocks);
    assert_eq!((clocks.same, clocks.dropped), (0, text.matches("[%clk ").count()), "{clocks:?}");
    let evals = r.total.get(Feature::Evals);
    assert_eq!((evals.same, evals.dropped), (0, text.matches("[%eval ").count()), "{evals:?}");
    assert!(clocks.examples[0].starts_with("game 1, at 1.e4: [%clk 0:05:00] dropped"), "{clocks:?}");
    for f in Feature::ALL {
        if ![Feature::Clocks, Feature::Evals, Feature::Orientation].contains(&f) {
            assert_eq!(r.total.get(f).differences(), 0, "{}: {:?}", f.name(), r.total.get(f).examples);
        }
    }
    // The push loses the clocks and evals it sent, the edited ones too.
    let push = r.push.as_ref().expect("the moves replaced");
    let (clk, ev) = (push.diff.get(Feature::Clocks), push.diff.get(Feature::Evals));
    assert!(clk.same == 0 && clk.dropped > 0 && ev.same == 0 && ev.dropped > 0, "{clk:?} {ev:?}");
    assert!(ev.examples.iter().any(|e| e.contains("[%eval 0.42] dropped")), "the eval added: {ev:?}");
    assert_eq!(push.diff.get(Feature::Comments).differences(), 0);
    let report = r.to_string();
    assert!(report.contains("game 1, at 1.e4: [%clk 0:05:00] dropped"), "{report}");
    assert!(report.contains("… and "), "more examples than shown: {report}");
}

#[test]
fn a_push_that_fails_leaves_the_rest_of_the_report() {
    let server = Lichess::start(0);
    let token = writer(&server);
    // The stand-in numbers what it makes: the study first, then its chapters.
    server.state.lock().unwrap().fail.push(("POST /api/study/st000001/ch".into(), 500));
    let r = round_trip(&client(&server), &token, &fixture(), "Round-trip check", &NoProgress).unwrap();
    assert_eq!(r.study.as_ref().unwrap().id, "st000001");
    assert!(r.push.is_none());
    assert!(
        matches!(r.push_error, Some(NetError::Status { site: Site::Lichess, code: 500 })),
        "{:?}",
        r.push_error
    );
    assert_eq!(r.games.len(), 6);
    assert!(r.total.get(Feature::Moves).same > 100);
    let text = r.to_string();
    assert!(text.contains("Failed: Lichess answered with an error (HTTP 500)"), "{text}");
    let gets = server.requests().iter().filter(|r| r.starts_with("GET")).count();
    assert_eq!(gets, 1, "no chapter fetched after the failure");
}

/// A rewrite that stands for Lichess not taking the ending from a position
/// (the second game): the stand-in makes no chapter of it.
fn without_the_ending(pgn: &str) -> String {
    if pgn.contains("[FEN \"8/8/4k3/8/4PK2/8/8/8 b - - 0 41\"]") { String::new() } else { pgn.to_owned() }
}

/// When Lichess doesn't take a game, or keeps the empty chapter a study
/// starts with, each other game is still compared with its own chapter,
/// not with the one at its place.
#[test]
fn games_lichess_didnt_take_have_no_chapter_and_the_others_line_up() {
    for keep_empty_first in [false, true] {
        let case = if keep_empty_first { "the empty chapter kept" } else { "no empty chapter" };
        let server = Lichess::start(0);
        let token = writer(&server);
        {
            let mut st = server.state.lock().unwrap();
            st.rewrite = Some(without_the_ending);
            st.keep_empty_first = keep_empty_first;
        }
        let r = round_trip(&client(&server), &token, &fixture(), "Round-trip check", &NoProgress).unwrap();
        assert!(r.stopped.is_none(), "{case}: {:?}", r.stopped);
        assert_eq!(r.chapters, 5 + usize::from(keep_empty_first), "{case}");
        let made = &server.studies_of("Owner")[0];
        let ids: Vec<&str> = made.chapters.iter().map(|(id, _)| id.as_str()).collect();
        let got: Vec<(bool, Option<&str>)> =
            r.games.iter().map(|g| (g.in_study, g.chapter.as_deref())).collect();
        let want = [
            (true, Some(ids[0])),
            (false, None),
            (true, Some(ids[1])),
            (true, Some(ids[2])),
            (true, Some(ids[3])),
            (true, Some(ids[4])),
        ];
        assert_eq!(got, want, "{case}");
        assert!(r.games[1].diff.is_none(), "{case}");
        // The others against their own chapters: only the Orientation the
        // stand-in gives games without one differs.
        for f in Feature::ALL {
            let t = r.total.get(f);
            if f == Feature::Orientation {
                assert_eq!((t.changed, t.dropped, t.added), (0, 0, 3), "{case}: {:?}", t.examples);
            } else {
                assert_eq!(t.differences(), 0, "{case}, {}: {:?}", f.name(), t.examples);
            }
        }
        let note = |start: &str| r.notes.iter().any(|n| n.starts_with(start));
        assert!(note("Game 2: the study has no chapter of it"), "{case}: {:?}", r.notes);
        assert!(note("Lichess said about the games: A game couldn't be imported"), "{case}: {:?}", r.notes);
        assert_eq!(note("Lichess kept the empty chapter"), keep_empty_first, "{case}: {:?}", r.notes);
        assert!(note("6 games sent and the study has 5 chapters"), "{case}: {:?}", r.notes);
        let text = r.to_string();
        let line = "game 2: Round-trip: an ending from a position, Weiß - Schwarz: no chapter in the study";
        assert!(text.contains(line), "{case}:\n{text}");
        // The first game's chapter still has its moves replaced.
        let push = r.push.as_ref().unwrap_or_else(|| panic!("{case}: {:?}", r.push_error));
        assert_eq!((push.game, push.chapter.as_str(), push.diff.differences()), (1, ids[0], 0), "{case}");
    }
}

/// Once the study is made, a failure doesn't lose the report: it has the
/// study's link (the study stays on the account) and why it stopped.
#[test]
fn a_check_that_stops_after_making_its_study_still_reports_it() {
    // The games aren't taken.
    let server = Lichess::start(0);
    let token = writer(&server);
    server.state.lock().unwrap().fail.push(("POST /api/study/st000001/import-pgn".into(), 500));
    let r = round_trip(&client(&server), &token, &fixture(), "Round-trip check", &NoProgress).unwrap();
    assert_eq!(r.study.as_ref().map(|s| s.id.as_str()), Some("st000001"));
    assert!(
        matches!(r.stopped, Some(NetError::Status { site: Site::Lichess, code: 500 })),
        "{:?}",
        r.stopped
    );
    assert!(r.games.is_empty() && r.push.is_none() && r.push_error.is_none());
    assert!(r.texts.sent.contains("[Event \"Round-trip: promotions\"]"), "what was to be sent is kept");
    let text = r.to_string();
    for part in [
        format!("Study: {}/study/st000001 (private, \"Round-trip check\")", server.url),
        "The check stopped: Lichess answered with an error (HTTP 500)".to_owned(),
        "delete it on Lichess".to_owned(),
    ] {
        assert!(text.contains(&part), "{part:?} in:\n{text}");
    }
    assert!(!text.contains("1. The games sent"), "nothing was compared:\n{text}");
    assert_eq!(server.requests().len(), 2, "{:?}", server.requests());

    // The games went, and the study can't be fetched back.
    let server = Lichess::start(0);
    let token = writer(&server);
    server.state.lock().unwrap().fail.push(("GET /api/study/st000001.pgn".into(), 500));
    let r = round_trip(&client(&server), &token, &fixture(), "Round-trip check", &NoProgress).unwrap();
    assert_eq!(r.study.as_ref().map(|s| (s.sent, s.chapters)), Some((6, 6)));
    assert!(matches!(r.stopped, Some(NetError::Status { code: 500, .. })), "{:?}", r.stopped);
    assert!(r.games.is_empty() && r.push.is_none());
    assert!(r.to_string().contains("6 games sent, 6 chapters made, 0 chapters fetched back."), "{r}");
    assert_eq!(server.requests().len(), 3, "nothing after the failure: {:?}", server.requests());
}

#[test]
fn refused_before_asking_lichess() {
    let server = Lichess::start(0);
    let token = writer(&server);
    let client = client(&server);
    let one = fixture().into_iter().next().unwrap();
    let many = vec![one; 65];
    let e = round_trip(&client, &token, &many, "Round-trip check", &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::StudyFull { room: 64, .. }), "{e}");
    let e = round_trip(&client, &token, &many[..1], " x ", &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::BadStudyName), "{e}");
    let r = round_trip(&client, &token, &[], "Round-trip check", &NoProgress).unwrap();
    assert!(r.study.is_none() && r.push.is_none() && r.games.is_empty());
    assert!(r.to_string().contains("no games to send"), "{r}");
    assert!(server.requests().is_empty(), "{:?}", server.requests());

    // At Lichess's daily limit: no study, an error.
    server.state.lock().unwrap().daily_limit = 0;
    let e = round_trip(&client, &token, &many[..1], "Round-trip check", &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::StudyLimit), "{e}");
    assert!(server.studies_of("Owner").is_empty());
}
