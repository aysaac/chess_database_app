//! Protocol tests against the fake engine (`src/bin/fake_uci.rs`), plus an
//! optional run against a real Stockfish (`STOCKFISH=path`, or the binary
//! fetched by `scripts/fetch-stockfish.sh`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use chess_engine::{
    AnalysisRequest, Analyzer, AnalyzerEvent, Engine, EngineError, Limits, Score, SearchPosition, UciOption,
};

fn fake() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_fake-uci"))
}

fn pos(fen: Option<&str>, moves: &[&str]) -> SearchPosition {
    SearchPosition { fen: fen.map(str::to_owned), moves: moves.iter().map(|m| m.to_string()).collect() }
}

#[test]
fn handshake_reads_id_and_options() {
    let engine = Engine::start(fake()).unwrap();
    assert_eq!(engine.id().name, "Fake UCI 1.0");
    assert_eq!(engine.id().author, "The Tests");
    assert_eq!(
        engine.option("multipv"),
        Some(&UciOption::Spin { name: "MultiPV".into(), default: 1, min: 1, max: 500 })
    );
    assert!(engine.has_option("Clear Hash"));
}

#[test]
fn searches_to_a_depth_and_finds_the_capture() {
    let mut engine = Engine::start(fake()).unwrap();
    engine.set_option("MultiPV", "2").unwrap();
    engine.new_game().unwrap();
    // The rook on d2 wins the undefended queen.
    let mut infos = 0;
    let result = engine
        .search(
            &pos(Some("4k3/8/8/3q4/8/8/3R4/4K3 w - - 0 1"), &[]),
            &Limits { depth: Some(3), ..Limits::default() },
            &mut |_| infos += 1,
            &|| false,
        )
        .unwrap();
    assert_eq!(result.best.as_deref(), Some("d2d5"));
    assert_eq!(result.lines.len(), 2);
    assert_eq!(result.lines[0].pv[0], "d2d5");
    assert_eq!(result.lines[0].depth, Some(3));
    assert_eq!(result.lines[0].score, Some(Score::Cp(500)));
    assert_eq!(infos, 6, "two lines at three depths");
}

#[test]
fn rejects_unknown_options_and_bad_values() {
    let mut engine = Engine::start(fake()).unwrap();
    assert!(matches!(engine.set_option("Nope", "1"), Err(EngineError::UnknownOption(_))));
    assert!(matches!(engine.set_option("Hash", "0"), Err(EngineError::BadOptionValue { .. })));
    engine.set_option("Clear Hash", "").unwrap();
    engine.sync().unwrap();
}

#[test]
fn infinite_search_stops_and_cancels() {
    let mut engine = Engine::start(fake()).unwrap();
    engine.go(&pos(None, &[]), &Limits::infinite()).unwrap();
    let mut seen = 0;
    while seen < 3 {
        if engine.next_event(Duration::from_secs(2)).unwrap().is_some() {
            seen += 1;
        }
    }
    engine.stop().unwrap();
    assert!(!engine.is_searching());
    engine.sync().unwrap();

    // A cancelled search still returns what it found.
    let calls = std::cell::Cell::new(0);
    let result = engine
        .search(&pos(None, &[]), &Limits::infinite(), &mut |_| {}, &|| {
            calls.set(calls.get() + 1);
            calls.get() > 3
        })
        .unwrap();
    assert!(result.best.is_some());
}

#[test]
fn crash_and_missing_binary_are_errors() {
    let mut engine = Engine::start(fake()).unwrap();
    engine.set_option("CrashOnGo", "true").unwrap();
    let err = engine.search(&pos(None, &[]), &Limits::movetime(100), &mut |_| {}, &|| false).unwrap_err();
    assert!(matches!(err, EngineError::Exited(_)), "{err}");

    let missing = PathBuf::from("/definitely/not/an/engine");
    assert!(matches!(Engine::start(&missing), Err(EngineError::Spawn { .. })));
}

#[test]
fn mate_and_stalemate_positions() {
    let mut engine = Engine::start(fake()).unwrap();
    // Back-rank mate.
    let mate_in_one = "6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1";
    let r =
        engine.search(&pos(Some(mate_in_one), &[]), &Limits::movetime(50), &mut |_| {}, &|| false).unwrap();
    assert_eq!(r.best.as_deref(), Some("a1a8"));
    assert_eq!(r.lines[0].score, Some(Score::Mate(1)));
    // No legal moves: `bestmove (none)`.
    let stalemate = "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1";
    let r = engine.search(&pos(Some(stalemate), &[]), &Limits::movetime(50), &mut |_| {}, &|| false).unwrap();
    assert_eq!(r.best, None);
}

#[test]
fn analyzer_follows_the_latest_position() {
    let (tx, rx) = channel();
    let mut analyzer = Analyzer::start(fake(), &[("Delay".into(), "20".into())], move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    assert_eq!(analyzer.id().name, "Fake UCI 1.0");

    let request = |moves: &[&str], multipv| AnalysisRequest {
        position: pos(None, moves),
        multipv,
        limits: Limits::infinite(),
    };
    // A burst of positions: only the last one matters.
    analyzer.analyze(request(&["e2e4"], 1));
    analyzer.analyze(request(&["e2e4", "e7e5"], 1));
    let last = analyzer.analyze(request(&["e2e4", "d7d5"], 3));

    let update = loop {
        match rx.recv_timeout(Duration::from_secs(5)).expect("an update") {
            AnalyzerEvent::Update(u) if u.request == last && u.depth >= 3 => break u,
            AnalyzerEvent::Update(_) => {}
            AnalyzerEvent::Failed(e) => panic!("engine failed: {e}"),
        }
    };
    assert_eq!(update.lines.len(), 3);
    assert_eq!(update.lines[0].pv[0], "e4d5");
    assert!(!update.finished);

    // Limited searches finish with the best move.
    let limited = analyzer.analyze(AnalysisRequest {
        position: pos(None, &["e2e4", "d7d5"]),
        multipv: 1,
        limits: Limits { depth: Some(2), ..Limits::default() },
    });
    let done = loop {
        match rx.recv_timeout(Duration::from_secs(5)).expect("an update") {
            AnalyzerEvent::Update(u) if u.request == limited && u.finished => break u,
            _ => {}
        }
    };
    assert_eq!(done.best.as_deref(), Some("e4d5"));
    assert_eq!(done.lines.len(), 1);

    analyzer.stop();
    assert!(analyzer.is_alive());
}

#[test]
fn analyzer_reports_engine_failure() {
    let (tx, rx) = channel();
    let mut analyzer = Analyzer::start(fake(), &[("CrashOnGo".into(), "true".into())], move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    analyzer.analyze(AnalysisRequest { position: pos(None, &[]), multipv: 1, limits: Limits::infinite() });
    loop {
        if let AnalyzerEvent::Failed(msg) = rx.recv_timeout(Duration::from_secs(5)).expect("an event") {
            assert!(msg.contains("exited"), "{msg}");
            break;
        }
    }
    for _ in 0..100 {
        if !analyzer.is_alive() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the analyzer should stop after a failure");
}

/// 1.e4 e5 2.Bc4 Nc6 3.Qh5, Black to move: White threatens 4.Qxf7#.
const SCHOLAR_THREAT: &str = "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 3 3";

#[test]
fn threat_engine_finds_the_mate_after_a_pass() {
    use chess_engine::threat::{self, THREAT_LIMITS};

    let pos = chess_core::position::from_fen(SCHOLAR_THREAT).unwrap();
    let after = threat::null_move(&pos).unwrap();
    let declared = Engine::start(fake()).unwrap().id().options.clone();
    let options =
        threat::engine_options(&declared, &[("MultiPV".into(), "3".into()), ("Delay".into(), "1".into())]);
    assert_eq!(options, [("Delay", "1"), ("Hash", "16")].map(|(n, v)| (n.to_owned(), v.to_owned())));

    let (tx, rx) = channel();
    let mut analyzer = Analyzer::start(fake(), &options, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    let id = analyzer.analyze(AnalysisRequest {
        position: threat::search_position(&after),
        multipv: 1,
        limits: THREAT_LIMITS,
    });
    // The search is capped: it ends by itself with the threat.
    let done = loop {
        match rx.recv_timeout(Duration::from_secs(5)).expect("an update") {
            AnalyzerEvent::Update(u) if u.request == id && u.finished => break u,
            AnalyzerEvent::Update(_) => {}
            AnalyzerEvent::Failed(e) => panic!("engine failed: {e}"),
        }
    };
    assert_eq!(done.best.as_deref(), Some("h5f7"));
    assert_eq!(done.depth, 20, "searched to the depth cap");
    let found = threat::threat_line(&after, &done.lines[0], 8).unwrap();
    assert_eq!((found.moves[0].san.as_str(), found.moves[0].move_number), ("Qxf7#", 4));
    assert_eq!(found.score, Score::Mate(1), "White mates");
    assert!(analyzer.is_alive(), "the engine idles after a capped search");
}

#[test]
fn blunder_check_marks_the_hung_queen() {
    use chess_core::game::Nag;
    use chess_engine::review::{Judgement, ReviewSettings, apply_review, review_game};

    let mut game = chess_core::pgn::read_pgn_str("1. e4 d5 2. Qg4 Bxg4 *").remove(0);
    let mut engine = Engine::start(fake()).unwrap();
    let mut calls = Vec::new();
    let settings = ReviewSettings { limits: Limits { depth: Some(2), ..Limits::default() }, max_line: 4 };
    let reviews =
        review_game(&game, &mut engine, &settings, &mut |d, t| calls.push((d, t)), &|| false).unwrap();
    assert_eq!(calls.last(), Some(&(5, 5)), "five positions");
    assert_eq!(reviews.len(), 4);
    let judged: Vec<_> = reviews.iter().map(|r| r.judgement).collect();
    assert_eq!(judged, [None, None, Some(Judgement::Blunder), None]);
    let (line, _) = reviews[2].better.clone().expect("a better line for 2.Qg4");
    assert_ne!(chess_core::notation::uci(line[0]), "d1g4");
    // White's point of view: after 2.Qg4 Black is winning.
    assert!(matches!(reviews[2].eval, Score::Cp(cp) if cp < -500), "{:?}", reviews[2].eval);

    assert!(apply_review(&mut game, &reviews));
    let qg4 = reviews[2].node;
    assert_eq!(game.node(qg4).nags, vec![Nag(4)]);
    let parent = game.node(qg4).parent.unwrap();
    assert_eq!(game.node(parent).children.len(), 2, "the better line is a variation");
    assert!(game.node(qg4).commands.iter().any(|(n, _)| n == "eval"));
    assert!(!apply_review(&mut game, &reviews), "applying twice changes nothing");

    // Cancelling stops early.
    let err = review_game(&game, &mut engine, &settings, &mut |_, _| {}, &|| true).unwrap_err();
    assert!(matches!(err, EngineError::Cancelled));
}

/// Runs against a real Stockfish when one is available.
#[test]
fn real_stockfish_if_present() {
    let path = std::env::var_os("STOCKFISH").map(PathBuf::from).or_else(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src-tauri/binaries");
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("stockfish-")))
    });
    let Some(path) = path else {
        eprintln!("no Stockfish found; skipping");
        return;
    };
    let mut engine = Engine::start(&path).unwrap();
    assert!(engine.id().name.starts_with("Stockfish"), "{}", engine.id().name);
    engine.set_option("UCI_ShowWDL", "true").unwrap();
    engine.set_option("MultiPV", "2").unwrap();
    let r = engine
        .search(
            &pos(None, &["e2e4", "d7d5"]),
            &Limits { depth: Some(12), ..Limits::default() },
            &mut |_| {},
            &|| false,
        )
        .unwrap();
    assert_eq!(r.best.as_deref(), Some("e4d5"));
    assert_eq!(r.lines.len(), 2);
    assert!(r.lines[0].wdl.is_some());

    // ENG-07: had Black passed after 3.Qh5, White mates on f7.
    use chess_engine::threat;
    let after = threat::null_move(&chess_core::position::from_fen(SCHOLAR_THREAT).unwrap()).unwrap();
    engine.set_option("MultiPV", "1").unwrap();
    let r = engine
        .search(
            &threat::search_position(&after),
            &Limits { depth: Some(10), ..Limits::default() },
            &mut |_| {},
            &|| false,
        )
        .unwrap();
    let found = threat::threat_line(&after, &r.lines[0], 8).unwrap();
    assert_eq!((found.moves[0].san.as_str(), found.score), ("Qxf7#", Score::Mate(1)));
}
