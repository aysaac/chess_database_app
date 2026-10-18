//! A deterministic toy UCI engine for tests: a 2-ply material search.
//!
//! Options: `MultiPV`, `Hash` (ignored), `UCI_ShowWDL`, `Delay` (ms between
//! depths; searches without limits run until `stop`), `CrashOnGo` (exit
//! without answering).
//!
//! Starts can be scripted, for tests of what runs a second engine (the
//! app's threat search, ENG-07): with a file `starts.txt` next to the
//! binary (tests copy it into a folder of their own), the n-th start does
//! what the n-th word there says: `ok`, `refuse` (exit with status 3 before
//! the handshake, like an engine that allows one instance) or `slow:<ms>`
//! (wait that long first). The count is kept in `count.txt` next to it.

use std::io::{BufRead, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use chess_core::notation::uci;
use chess_core::position::{from_fen, parse_uci};
use chess_core::shakmaty::{Chess, Color, Move, Position, Role};

fn value(role: Role) -> i32 {
    match role {
        Role::Pawn => 100,
        Role::Knight => 300,
        Role::Bishop => 300,
        Role::Rook => 500,
        Role::Queen => 900,
        Role::King => 0,
    }
}

/// Material balance for the side to move.
fn material(pos: &Chess) -> i32 {
    let board = pos.board();
    let mut total = 0;
    for sq in board.occupied() {
        let piece = board.piece_at(sq).expect("occupied");
        let v = value(piece.role);
        total += if piece.color == pos.turn() { v } else { -v };
    }
    total
}

const MATE: i32 = 1_000_000;

/// Score of `pos` for the side to move after the opponent's best reply to
/// every move (plies = 1: static; 2: one reply).
fn search(pos: &Chess, plies: u32) -> (i32, Vec<Move>) {
    let moves = pos.legal_moves();
    if moves.is_empty() {
        return (if pos.is_check() { -MATE } else { 0 }, Vec::new());
    }
    if plies == 0 {
        return (material(pos), Vec::new());
    }
    let mut best = (i32::MIN, Vec::new());
    for m in sorted(moves.into_iter().collect()) {
        let mut next = pos.clone();
        next.play_unchecked(m);
        let (s, mut line) = search(&next, plies - 1);
        let s = -s;
        if s > best.0 {
            line.insert(0, m);
            best = (s, line);
        }
    }
    best
}

fn sorted(mut moves: Vec<Move>) -> Vec<Move> {
    moves.sort_by_key(|m| uci(*m));
    moves
}

/// All moves ranked best first: (score, pv).
fn ranked(pos: &Chess) -> Vec<(i32, Vec<Move>)> {
    let mut out: Vec<(i32, Vec<Move>)> = sorted(pos.legal_moves().into_iter().collect())
        .into_iter()
        .map(|m| {
            let mut next = pos.clone();
            next.play_unchecked(m);
            let (s, mut line) = search(&next, 1);
            line.insert(0, m);
            (-s, line)
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.0));
    out
}

fn score_text(s: i32) -> String {
    if s >= MATE - 10 {
        "mate 1".into()
    } else if s <= -MATE + 10 {
        "mate -1".into()
    } else {
        format!("cp {s}")
    }
}

struct State {
    pos: Chess,
    multipv: usize,
    wdl: bool,
    delay: u64,
    crash: bool,
}

fn out(line: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

fn set_position(state: &mut State, args: &str) {
    let (base, moves) = match args.split_once(" moves ") {
        Some((b, m)) => (b.trim(), m.split_whitespace().collect::<Vec<_>>()),
        None => (args.trim().trim_end_matches(" moves"), Vec::new()),
    };
    let mut pos = if let Some(fen) = base.strip_prefix("fen ") {
        from_fen(fen).unwrap_or_default()
    } else {
        Chess::default()
    };
    for m in moves {
        match parse_uci(&pos, m) {
            Some(mv) => pos.play_unchecked(mv),
            None => break,
        }
    }
    state.pos = pos;
}

fn go(state: &State, args: &str, input: &Receiver<String>) -> Option<String> {
    let mut depth_limit = None;
    let mut timed = false;
    let mut tokens = args.split_whitespace();
    while let Some(t) = tokens.next() {
        match t {
            "depth" => depth_limit = tokens.next().and_then(|d| d.parse::<u32>().ok()),
            "movetime" | "nodes" | "wtime" => timed = true,
            _ => {}
        }
    }
    // Searches without depth/time run until stopped.
    let max_depth = depth_limit.unwrap_or(if timed { 3 } else { u32::MAX });
    let lines = ranked(&state.pos);
    let start = Instant::now();
    let mut pending = None;
    let mut depth = 1;
    loop {
        for (i, (score, pv)) in lines.iter().take(state.multipv).enumerate() {
            let pv: Vec<String> = pv.iter().map(|m| uci(*m)).collect();
            let wdl = if state.wdl {
                let w = (500 + score / 2).clamp(0, 1000) as u32;
                format!(" wdl {w} 0 {}", 1000 - w)
            } else {
                String::new()
            };
            out(&format!(
                "info depth {depth} seldepth {depth} multipv {} score {}{wdl} nodes {} nps 1000 time {} pv {}",
                i + 1,
                score_text(*score),
                depth * 100,
                start.elapsed().as_millis(),
                pv.join(" ")
            ));
        }
        if lines.is_empty() {
            out("info depth 0 score cp 0");
        }
        if depth >= max_depth || lines.is_empty() {
            break;
        }
        depth += 1;
        match input.recv_timeout(Duration::from_millis(state.delay.max(1))) {
            Ok(line) if line.trim() == "stop" => break,
            Ok(line) => {
                // Anything else ends the search; handle it afterwards.
                pending = Some(line);
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::process::exit(0),
        }
    }
    let best = lines.first().map_or("(none)".to_owned(), |(_, pv)| uci(pv[0]));
    out(&format!("bestmove {best}"));
    pending
}

/// Follows `starts.txt` next to the binary, if there is one (see the top).
fn scripted_start() {
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.to_path_buf())) else {
        return;
    };
    let Ok(script) = std::fs::read_to_string(dir.join("starts.txt")) else { return };
    let count = dir.join("count.txt");
    let n =
        std::fs::read_to_string(&count).ok().and_then(|c| c.trim().parse::<usize>().ok()).unwrap_or(0) + 1;
    let _ = std::fs::write(&count, n.to_string());
    match script.split_whitespace().nth(n - 1) {
        Some("refuse") => {
            eprintln!("start {n} refused");
            std::process::exit(3);
        }
        Some(word) => {
            if let Some(ms) = word.strip_prefix("slow:").and_then(|ms| ms.parse().ok()) {
                std::thread::sleep(Duration::from_millis(ms));
            }
        }
        None => {}
    }
}

fn main() {
    scripted_start();
    let (tx, input) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut state = State { pos: Chess::default(), multipv: 1, wdl: false, delay: 5, crash: false };
    let mut pending: Option<String> = None;
    loop {
        let line = match pending.take() {
            Some(l) => l,
            None => match input.recv() {
                Ok(l) => l,
                Err(_) => return,
            },
        };
        let line = line.trim();
        let (cmd, args) = line.split_once(' ').unwrap_or((line, ""));
        match cmd {
            "uci" => {
                out("id name Fake UCI 1.0");
                out("id author The Tests");
                out("option name MultiPV type spin default 1 min 1 max 500");
                out("option name Hash type spin default 16 min 1 max 1024");
                out("option name UCI_ShowWDL type check default false");
                out("option name Delay type spin default 5 min 0 max 100000");
                out("option name CrashOnGo type check default false");
                out("option name Clear Hash type button");
                out("uciok");
            }
            "isready" => out("readyok"),
            "ucinewgame" | "stop" => {}
            "setoption" => {
                let (name, value) = match args.strip_prefix("name ").and_then(|a| a.split_once(" value ")) {
                    Some((n, v)) => (n.trim(), v.trim()),
                    None => (args.trim_start_matches("name ").trim(), ""),
                };
                match name {
                    "MultiPV" => state.multipv = value.parse().unwrap_or(1),
                    "UCI_ShowWDL" => state.wdl = value == "true",
                    "Delay" => state.delay = value.parse().unwrap_or(5),
                    "CrashOnGo" => state.crash = value == "true",
                    _ => {}
                }
            }
            "position" => set_position(&mut state, args),
            "go" => {
                if state.crash {
                    std::process::exit(3);
                }
                pending = go(&state, args, &input);
            }
            "quit" => return,
            "d" => out(&format!("turn {}", if state.pos.turn() == Color::White { "white" } else { "black" })),
            _ => {}
        }
    }
}
