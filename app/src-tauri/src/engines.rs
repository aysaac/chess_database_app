//! Engines in the app: finding the bundled Stockfish (ENG-09) and engines
//! on PATH, and the interactive analysis that follows the game on screen
//! (ENG-02). Updates reach the frontend as `analysis` events.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chess_core::notation::{san, uci};
use chess_core::position::{self, parse_uci};
use chess_core::shakmaty::{Chess, Color, Position};
use chess_engine::{
    AnalysisRequest, AnalysisUpdate, Analyzer, AnalyzerEvent, Bound, EngineError, Limits, SearchPosition,
};
use tauri::{AppHandle, Emitter};

use crate::dto::{AnalysisDto, EvalDto, PvLineDto, PvMoveDto};
use crate::settings::EngineConfig;
use crate::state::Session;

/// Event carrying an [`AnalysisDto`].
pub const ANALYSIS_EVENT: &str = "analysis";
/// Longest principal variation sent to the frontend, in plies.
const MAX_PV: usize = 24;

/// Engine binaries that can be used without asking: the bundled Stockfish
/// next to the executable, the one `scripts/fetch-stockfish.sh` fetched (dev
/// builds), and `stockfish` on PATH.
pub fn detect() -> Vec<PathBuf> {
    let exe = std::env::consts::EXE_SUFFIX;
    let mut found = Vec::new();
    if let Ok(me) = std::env::current_exe()
        && let Some(dir) = me.parent()
    {
        found.push(dir.join(format!("stockfish{exe}")));
    }
    if cfg!(debug_assertions)
        && let Ok(entries) = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries"))
    {
        let mut dev: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("stockfish-")))
            .collect();
        dev.sort();
        found.extend(dev);
    }
    if let Some(paths) = std::env::var_os("PATH") {
        found.extend(std::env::split_paths(&paths).map(|dir| dir.join(format!("stockfish{exe}"))));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for p in found.into_iter().filter(|p| p.is_file()) {
        let p = p.canonicalize().unwrap_or(p);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Option values from `config` that its engine still declares (a newer
/// engine version may have dropped some).
pub fn usable_options(config: &EngineConfig) -> Vec<(String, String)> {
    config
        .options
        .iter()
        .filter(|(name, value)| {
            config.declared.iter().any(|o| o.name().eq_ignore_ascii_case(name) && o.validate(value).is_some())
        })
        .cloned()
        .collect()
}

#[derive(Clone)]
struct Target {
    request: u64,
    session: u32,
    pos: Chess,
    fen: String,
}

/// The running analysis. It follows one game session: every time that
/// session's cursor moves, the new position is searched.
pub struct Analysis {
    pub engine_id: String,
    pub engine_name: String,
    pub session: u32,
    pub multipv: u32,
    analyzer: Analyzer,
    target: Arc<Mutex<Option<Target>>>,
    sent: Option<SearchPosition>,
}

impl Analysis {
    /// Starts the engine (blocking: up to the handshake timeout).
    pub fn start(
        config: &EngineConfig,
        session: u32,
        multipv: u32,
        app: AppHandle,
    ) -> Result<Analysis, EngineError> {
        let target: Arc<Mutex<Option<Target>>> = Arc::new(Mutex::new(None));
        let shared = target.clone();
        let analyzer = Analyzer::start(&config.path, &usable_options(config), move |event| {
            let dto = match event {
                AnalyzerEvent::Update(update) => {
                    let target = shared.lock().expect("analysis target lock").clone();
                    match target {
                        Some(t) if t.request == update.request => analysis_dto(&t, &update),
                        _ => return,
                    }
                }
                AnalyzerEvent::Failed(message) => {
                    AnalysisDto { error: Some(message), ..AnalysisDto::default() }
                }
            };
            if let Err(e) = app.emit(ANALYSIS_EVENT, dto) {
                log::warn!("failed to emit analysis: {e}");
            }
        })?;
        let engine_name = analyzer.id().name.clone();
        Ok(Analysis {
            engine_id: config.id.clone(),
            engine_name,
            session,
            multipv: multipv.max(1),
            analyzer,
            target,
            sent: None,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.analyzer.is_alive()
    }

    /// Follows another session from now on.
    pub fn attach(&mut self, session: u32) {
        if self.session != session {
            self.session = session;
            self.sent = None;
        }
    }

    pub fn set_multipv(&mut self, multipv: u32) {
        self.multipv = multipv.clamp(1, 10);
        self.sent = None;
    }

    pub fn set_option(&mut self, name: &str, value: &str) {
        self.analyzer.set_option(name, value);
    }

    /// Searches `session`'s current position if this analysis follows it
    /// and the position changed since the last search.
    pub fn follow(&mut self, session_id: u32, session: &Session) {
        if session_id != self.session {
            return;
        }
        let game = session.editor.game();
        let cursor = session.editor.cursor();
        let request = SearchPosition {
            fen: game.setup.as_ref().map(position::to_fen),
            moves: game
                .path(cursor)
                .iter()
                .map(|&n| uci(game.node(n).mv.expect("non-root node has a move")))
                .collect(),
        };
        if self.sent.as_ref() == Some(&request) {
            return;
        }
        self.sent = Some(request.clone());
        let pos = game.position_at(cursor);
        let mut target = self.target.lock().expect("analysis target lock");
        if pos.is_game_over() {
            *target = None;
            self.analyzer.stop();
            return;
        }
        // Set the target while holding the lock, so the first update of the
        // new search can't slip past it.
        let id = self.analyzer.analyze(AnalysisRequest {
            position: request,
            multipv: self.multipv,
            limits: Limits::infinite(),
        });
        *target = Some(Target { request: id, session: session_id, fen: position::to_fen(&pos), pos });
    }
}

fn eval_dto(score: chess_engine::Score) -> EvalDto {
    match score {
        chess_engine::Score::Cp(cp) => EvalDto { cp: Some(cp), mate: None },
        chess_engine::Score::Mate(n) => EvalDto { cp: None, mate: Some(n) },
    }
}

fn analysis_dto(t: &Target, u: &AnalysisUpdate) -> AnalysisDto {
    let white = t.pos.turn() == Color::White;
    let lines = u
        .lines
        .iter()
        .enumerate()
        .filter_map(|(i, info)| {
            let score = info.score?.for_white(white);
            let mut pos = t.pos.clone();
            let mut moves = Vec::new();
            for m in info.pv.iter().take(MAX_PV) {
                let Some(mv) = parse_uci(&pos, m) else { break };
                moves.push(PvMoveDto {
                    uci: m.clone(),
                    san: san(&pos, mv),
                    move_number: pos.fullmoves().get(),
                    white: pos.turn() == Color::White,
                });
                pos.play_unchecked(mv);
            }
            if moves.is_empty() {
                return None;
            }
            let bound = match (info.bound, white) {
                (Bound::Exact, _) => "exact",
                (Bound::Lower, true) | (Bound::Upper, false) => "lower",
                (Bound::Upper, true) | (Bound::Lower, false) => "upper",
            };
            Some(PvLineDto {
                multipv: info.multipv.unwrap_or(i as u32 + 1),
                eval: eval_dto(score),
                bound: bound.to_owned(),
                wdl: info.wdl.map(|[w, d, l]| if white { vec![w, d, l] } else { vec![l, d, w] }),
                depth: info.depth.unwrap_or(u.depth),
                moves,
            })
        })
        .collect();
    AnalysisDto {
        session: t.session,
        fen: t.fen.clone(),
        depth: u.depth,
        seldepth: u.seldepth,
        nodes: u.nodes,
        nps: u.nps,
        time_ms: u.time_ms,
        hashfull: u.hashfull,
        finished: u.finished,
        lines,
        error: None,
    }
}
