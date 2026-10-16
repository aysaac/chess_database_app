//! Engines in the app: finding the bundled Stockfish (ENG-09) and engines
//! on PATH, and the interactive analysis that follows the game on screen
//! (ENG-02), with the opponent's threat next to it (ENG-07). Updates reach
//! the frontend as `analysis` and `threat` events.
//!
//! **The threat runs on a second engine process.** The main analysis is an
//! infinite search that keeps deepening while the board stays put; sharing
//! its engine with the threat would mean stopping it for every threat
//! search and restarting it afterwards, so its depth would drop back each
//! time and its lines would flicker. A second process of the same engine
//! (`ThreatSearch`) leaves it alone: it runs on one thread with a small
//! hash (`chess_engine::threat::engine_options`), searches the position
//! after a pass up to a depth and time cap (`THREAT_LIMITS`), then idles
//! until the board moves. Its `Analyzer` coalesces bursts of moves and
//! replaces a running search like the main one does, and it quits when
//! "Show threat" or the analysis goes off. It isn't a second analysis
//! (ENG-04 kibitzers): it follows the main one's session and engine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chess_core::notation::{san, uci};
use chess_core::position::{self, parse_uci};
use chess_core::shakmaty::{Chess, Color, Position};
use chess_engine::threat::{self, NoThreat, THREAT_LIMITS};
use chess_engine::{
    AnalysisRequest, AnalysisUpdate, Analyzer, AnalyzerEvent, Bound, EngineError, Limits, SearchPosition,
};
use tauri::{AppHandle, Emitter};

use crate::dto::{AnalysisDto, EvalDto, NoThreatDto, PvLineDto, PvMoveDto, ThreatDto};
use crate::settings::EngineConfig;
use crate::state::Session;

/// Event carrying an [`AnalysisDto`].
pub const ANALYSIS_EVENT: &str = "analysis";
/// Event carrying a [`ThreatDto`] (ENG-07).
pub const THREAT_EVENT: &str = "threat";
/// Longest principal variation sent to the frontend, in plies.
const MAX_PV: usize = 24;
/// Longest threat line sent to the frontend, in plies.
const MAX_THREAT: usize = 8;

/// Numbers each analysis, so a threat engine that finished starting after
/// its analysis was replaced isn't attached to the new one.
static NEXT_ANALYSIS: AtomicU64 = AtomicU64::new(1);

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
        let p = dunce::canonicalize(&p).unwrap_or(p);
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
    /// Tells this analysis from one started later (`NEXT_ANALYSIS`).
    pub serial: u64,
    pub engine_id: String,
    pub engine_name: String,
    pub session: u32,
    pub multipv: u32,
    analyzer: Analyzer,
    target: Arc<Mutex<Option<Target>>>,
    sent: Option<SearchPosition>,
    /// The threat search (ENG-07), while "Show threat" is on.
    threat: Option<ThreatSearch>,
}

impl Analysis {
    /// Starts the engine, and a second one for the threat if `threat`
    /// (blocking: up to the handshake timeout each). A threat engine that
    /// fails to start is reported as a `threat` event; the analysis runs
    /// without it.
    pub fn start(
        config: &EngineConfig,
        session: u32,
        multipv: u32,
        threat: bool,
        app: AppHandle,
    ) -> Result<Analysis, EngineError> {
        let threat = if threat { ThreatSearch::start_or_report(config, session, &app) } else { None };
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
            serial: NEXT_ANALYSIS.fetch_add(1, Ordering::Relaxed),
            engine_id: config.id.clone(),
            engine_name,
            session,
            multipv: multipv.max(1),
            analyzer,
            target,
            sent: None,
            threat,
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
            if let Some(t) = &mut self.threat {
                t.sent = None;
            }
        }
    }

    pub fn set_multipv(&mut self, multipv: u32) {
        self.multipv = multipv.clamp(1, 10);
        self.sent = None;
    }

    pub fn set_option(&mut self, name: &str, value: &str) {
        self.analyzer.set_option(name, value);
        if let Some(t) = &self.threat
            && threat::shares_option(name)
        {
            t.analyzer.set_option(name, value);
        }
    }

    /// True while the threat search (ENG-07) runs next to the analysis.
    pub fn has_threat(&self) -> bool {
        self.threat.as_ref().is_some_and(|t| t.analyzer.is_alive())
    }

    /// Replaces the threat search; the next [`Analysis::follow`] searches
    /// the position on the board with the new one. Returns the old one, to
    /// be dropped off the caller's locks (its engine takes a moment to quit).
    pub fn set_threat(&mut self, threat: Option<ThreatSearch>) -> Option<ThreatSearch> {
        std::mem::replace(&mut self.threat, threat)
    }

    /// Searches `session`'s current position if this analysis follows it
    /// and the position changed since the last search; the threat search
    /// the same.
    pub fn follow(&mut self, session_id: u32, session: &Session) {
        // A failed analysis stays until the next start or stop; its threat
        // engine mustn't go on searching meanwhile.
        if session_id != self.session || !self.is_alive() {
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
        let moved = self.sent.as_ref() != Some(&request);
        let threat_moved = self.threat.as_ref().is_some_and(|t| t.sent.as_ref() != Some(&request));
        if !moved && !threat_moved {
            return;
        }
        let pos = game.position_at(cursor);
        let fen = position::to_fen(&pos);
        if let Some(t) = &mut self.threat
            && threat_moved
        {
            t.follow(session_id, &request, &pos, &fen);
        }
        if !moved {
            return;
        }
        self.sent = Some(request.clone());
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
        *target = Some(Target { request: id, session: session_id, fen, pos });
    }
}

/// What the threat engine is searching: the position on the board and the
/// one after the pass.
#[derive(Clone)]
struct ThreatTarget {
    request: u64,
    session: u32,
    fen: String,
    after_pass: Chess,
}

/// The threat search (ENG-07): a second engine process that searches the
/// position on the board as if the side to move had passed (the module
/// comment says why it's a process of its own).
pub struct ThreatSearch {
    analyzer: Analyzer,
    target: Arc<Mutex<Option<ThreatTarget>>>,
    app: AppHandle,
    /// The position last followed, as the main analysis keeps it.
    sent: Option<SearchPosition>,
}

impl ThreatSearch {
    /// Starts the engine of `config` for threats (blocking: up to the
    /// handshake timeout).
    pub fn start(config: &EngineConfig, app: AppHandle) -> Result<ThreatSearch, EngineError> {
        let target: Arc<Mutex<Option<ThreatTarget>>> = Arc::new(Mutex::new(None));
        let shared = target.clone();
        let events = app.clone();
        let options = threat::engine_options(&config.declared, &usable_options(config));
        let analyzer = Analyzer::start(&config.path, &options, move |event| {
            let target = shared.lock().expect("threat target lock").clone();
            let dto = match event {
                AnalyzerEvent::Update(update) => match target {
                    Some(t) if t.request == update.request => threat_dto(&t, &update),
                    _ => return,
                },
                AnalyzerEvent::Failed(message) => ThreatDto {
                    session: target.as_ref().map_or(0, |t| t.session),
                    fen: target.map(|t| t.fen).unwrap_or_default(),
                    error: Some(message),
                    ..ThreatDto::default()
                },
            };
            emit_threat(&events, dto);
        })?;
        Ok(ThreatSearch { analyzer, target, app, sent: None })
    }

    /// [`ThreatSearch::start`], reporting a failure as a `threat` event.
    pub fn start_or_report(config: &EngineConfig, session: u32, app: &AppHandle) -> Option<ThreatSearch> {
        match ThreatSearch::start(config, app.clone()) {
            Ok(t) => Some(t),
            Err(e) => {
                log::warn!("couldn't start the threat engine: {e}");
                emit_threat(app, ThreatDto { session, error: Some(e.to_string()), ..ThreatDto::default() });
                None
            }
        }
    }

    /// Searches the threat in `pos` (`request` on the board), or says why
    /// there's none.
    fn follow(&mut self, session: u32, request: &SearchPosition, pos: &Chess, fen: &str) {
        self.sent = Some(request.clone());
        let mut target = self.target.lock().expect("threat target lock");
        match threat::null_move(pos) {
            Ok(after_pass) => {
                // The target is set under the lock, as for the main search.
                let id = self.analyzer.analyze(AnalysisRequest {
                    position: threat::search_position(&after_pass),
                    multipv: 1,
                    limits: THREAT_LIMITS,
                });
                *target = Some(ThreatTarget { request: id, session, fen: fen.to_owned(), after_pass });
            }
            Err(reason) => {
                *target = None;
                drop(target);
                self.analyzer.stop();
                let unavailable = match reason {
                    NoThreat::GameOver => NoThreatDto::GameOver,
                    NoThreat::InCheck => NoThreatDto::InCheck,
                    NoThreat::NoMoves => NoThreatDto::NoMoves,
                };
                emit_threat(
                    &self.app,
                    ThreatDto {
                        session,
                        fen: fen.to_owned(),
                        finished: true,
                        unavailable: Some(unavailable),
                        ..ThreatDto::default()
                    },
                );
            }
        }
    }
}

fn emit_threat(app: &AppHandle, dto: ThreatDto) {
    if let Err(e) = app.emit(THREAT_EVENT, dto) {
        log::warn!("failed to emit the threat: {e}");
    }
}

fn threat_dto(t: &ThreatTarget, u: &AnalysisUpdate) -> ThreatDto {
    let line =
        u.lines.first().and_then(|info| threat::threat_line(&t.after_pass, info, MAX_THREAT)).map(|found| {
            PvLineDto {
                multipv: 1,
                eval: eval_dto(found.score),
                bound: bound_name(found.bound).to_owned(),
                wdl: None,
                depth: if found.depth > 0 { found.depth } else { u.depth },
                moves: found
                    .moves
                    .into_iter()
                    .map(|m| PvMoveDto { uci: m.uci, san: m.san, move_number: m.move_number, white: m.white })
                    .collect(),
            }
        });
    ThreatDto {
        session: t.session,
        fen: t.fen.clone(),
        depth: u.depth,
        finished: u.finished,
        line,
        unavailable: None,
        error: None,
    }
}

fn bound_name(bound: Bound) -> &'static str {
    match bound {
        Bound::Exact => "exact",
        Bound::Lower => "lower",
        Bound::Upper => "upper",
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
