//! Engines in the app: finding the bundled Stockfish (ENG-09) and engines
//! on PATH, and the interactive analysis that follows the game on screen
//! (ENG-02), with the opponent's threat next to it (ENG-07). Updates reach
//! the frontend as `analysis` and `threat` events ([`Events`]).
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
//!
//! **Turning it on and off, and failures.** "Show threat" can change while
//! its engine starts (a double click: on, then off). The analysis keeps
//! what was asked last and counts the requests; an engine that finished
//! starting is attached only if nothing was asked since
//! (`Analysis::offer_threat`), and quits otherwise. It starts in the
//! background once the main analysis searches, and nothing waits for it,
//! so a slow one holds up neither the analysis nor the page. When the main
//! engine fails, the threat engine quits with it. A threat engine that
//! fails, or can't start, says so (a `threat` event with `error`), again to
//! each game the analysis follows next, and nothing more until "Show
//! threat" is turned off and on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

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

/// Numbers the `threat` events ([`ThreatDto::seq`]).
static NEXT_THREAT_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_threat_seq() -> u64 {
    NEXT_THREAT_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Where the analysis sends what it finds: to the app's windows as the
/// `analysis` and `threat` events, or to a test.
#[derive(Clone)]
pub struct Events(Arc<dyn Fn(EngineEvent) + Send + Sync>);

/// What [`Events`] carries.
#[derive(Debug)]
pub enum EngineEvent {
    Analysis(AnalysisDto),
    Threat(ThreatDto),
}

impl Events {
    pub fn new(send: impl Fn(EngineEvent) + Send + Sync + 'static) -> Events {
        Events(Arc::new(send))
    }

    /// To every window of the app.
    pub fn app(app: AppHandle) -> Events {
        Events::new(move |event| {
            let sent = match event {
                EngineEvent::Analysis(dto) => app.emit(ANALYSIS_EVENT, dto),
                EngineEvent::Threat(dto) => app.emit(THREAT_EVENT, dto),
            };
            if let Err(e) = sent {
                log::warn!("failed to emit an engine event: {e}");
            }
        })
    }

    fn analysis(&self, dto: AnalysisDto) {
        (self.0)(EngineEvent::Analysis(dto));
    }

    fn threat(&self, dto: ThreatDto) {
        (self.0)(EngineEvent::Threat(dto));
    }
}

/// The `threat` event for a threat engine that failed, or couldn't start.
fn failure_dto(session: u32, message: &str, start_failed: bool) -> ThreatDto {
    ThreatDto {
        seq: next_threat_seq(),
        session,
        error: Some(message.to_owned()),
        start_failed,
        ..ThreatDto::default()
    }
}

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
    /// The threat search (ENG-07). Shared with the engine's thread, which
    /// drops it when the engine fails.
    threat: Arc<Mutex<ThreatSlot>>,
    /// "Show threat" as last asked ([`Analysis::ask_threat`]).
    threat_wanted: bool,
    /// How many times "Show threat" was asked: a threat engine that
    /// finished starting is attached only if it wasn't asked again since.
    threat_asked: u64,
    /// Why the threat search stopped or couldn't start, until "Show threat"
    /// is asked again. Each session the analysis follows is told, so the
    /// page shows it whatever game is on the board.
    threat_failure: Option<ThreatFailure>,
    events: Events,
}

/// A threat search's failure, as [`Analysis`] keeps it.
struct ThreatFailure {
    message: String,
    start_failed: bool,
    /// The last session told.
    told: u32,
}

/// Where an analysis keeps its threat search.
#[derive(Default)]
struct ThreatSlot {
    search: Option<ThreatSearch>,
    /// The main engine failed: the analysis is over and takes no threat
    /// search any more.
    closed: bool,
}

impl Analysis {
    /// Starts the engine (blocking: up to the handshake timeout). The
    /// threat search is asked for separately ([`Analysis::ask_threat`]),
    /// once this one searches, so a slow threat engine doesn't hold it up.
    pub fn start(
        config: &EngineConfig,
        session: u32,
        multipv: u32,
        events: Events,
    ) -> Result<Analysis, EngineError> {
        let target: Arc<Mutex<Option<Target>>> = Arc::new(Mutex::new(None));
        let threat: Arc<Mutex<ThreatSlot>> = Arc::default();
        let (shared, slot, sink) = (target.clone(), threat.clone(), events.clone());
        let analyzer = Analyzer::start(&config.path, &usable_options(config), move |event| match event {
            AnalyzerEvent::Update(update) => {
                let target = shared.lock().expect("analysis target lock").clone();
                if let Some(t) = target.filter(|t| t.request == update.request) {
                    sink.analysis(analysis_dto(&t, &update));
                }
            }
            AnalyzerEvent::Failed(message) => {
                // The analysis is over (it stays until the next start or
                // stop): its threat engine quits now, not then.
                let orphan = {
                    let mut slot = slot.lock().expect("threat slot lock");
                    slot.closed = true;
                    slot.search.take()
                };
                sink.analysis(AnalysisDto { error: Some(message), ..AnalysisDto::default() });
                drop(orphan);
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
            threat_wanted: false,
            threat_asked: 0,
            threat_failure: None,
            events,
        })
    }

    /// False once the engine failed (from the moment it said so).
    pub fn is_alive(&self) -> bool {
        self.analyzer.is_alive() && !self.slot().closed
    }

    fn slot(&self) -> MutexGuard<'_, ThreatSlot> {
        self.threat.lock().expect("threat slot lock")
    }

    /// Follows another session from now on.
    pub fn attach(&mut self, session: u32) {
        if self.session != session {
            self.session = session;
            self.sent = None;
            if let Some(t) = self.slot().search.as_mut() {
                t.attach();
            }
        }
    }

    pub fn set_multipv(&mut self, multipv: u32) {
        self.multipv = multipv.clamp(1, 10);
        self.sent = None;
    }

    pub fn set_option(&mut self, name: &str, value: &str) {
        self.analyzer.set_option(name, value);
        if threat::shares_option(name)
            && let Some(t) = &self.slot().search
        {
            t.analyzer.set_option(name, value);
        }
    }

    /// True while the threat search (ENG-07) runs next to the analysis.
    pub fn has_threat(&self) -> bool {
        self.slot().search.as_ref().is_some_and(ThreatSearch::is_alive)
    }

    /// Records "Show threat" (ENG-07) as just asked, and returns the
    /// request's number for [`Analysis::offer_threat`]. Off takes the
    /// threat search out, and so does on when it failed (so its failure
    /// isn't taken up again while a new one starts); it's returned to be
    /// dropped off the caller's locks (its engine takes a moment to quit).
    pub fn ask_threat(&mut self, on: bool) -> (u64, Option<ThreatSearch>) {
        self.threat_wanted = on;
        self.threat_asked += 1;
        self.threat_failure = None;
        let mut slot = self.slot();
        let gone = !on || slot.search.as_ref().is_some_and(|t| !t.is_alive());
        let old = if gone { slot.search.take() } else { None };
        drop(slot);
        (self.threat_asked, old)
    }

    /// Whether a threat engine has to be started: "Show threat" is on, the
    /// analysis runs and no threat search does (a failed one doesn't count).
    pub fn needs_threat_engine(&self) -> bool {
        self.threat_wanted && self.is_alive() && !self.has_threat()
    }

    /// Whether request `asked` is the last one and it was on.
    fn still_wants_threat(&self, asked: u64) -> bool {
        self.threat_wanted && asked == self.threat_asked
    }

    /// Reports that the threat engine for request `asked` couldn't start
    /// (a `threat` event), unless "Show threat" was asked again since.
    /// Returns whether it was reported.
    pub fn threat_start_failed(&mut self, asked: u64, message: String) -> bool {
        if !self.still_wants_threat(asked) {
            return false;
        }
        self.events.threat(failure_dto(self.session, &message, true));
        self.threat_failure = Some(ThreatFailure { message, start_failed: true, told: self.session });
        true
    }

    /// Attaches a threat search that finished starting for request `asked`
    /// ([`Analysis::ask_threat`]), unless "Show threat" was asked again
    /// since or the engine failed meanwhile. Returns whether it was
    /// attached, and what to drop off the caller's locks: the search itself
    /// if it wasn't, or the failed one it replaces. The next
    /// [`Analysis::follow`] searches the board with it.
    pub fn offer_threat(&mut self, asked: u64, mut search: ThreatSearch) -> (bool, Option<ThreatSearch>) {
        let mut slot = self.threat.lock().expect("threat slot lock");
        if !self.still_wants_threat(asked) || slot.closed {
            return (false, Some(search));
        }
        search.attach();
        (true, slot.search.replace(search))
    }

    /// Searches `session`'s current position if this analysis follows it
    /// and the position changed since the last search; the threat search
    /// the same.
    pub fn follow(&mut self, session_id: u32, session: &Session) {
        // A failed analysis stays until the next start or stop, and its
        // threat engine is gone: nothing is searched meanwhile.
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
        let mut slot = self.threat.lock().expect("threat slot lock");
        // A threat search that failed has said so, to the session it
        // followed: it searches and notes nothing more until "Show threat"
        // is turned off and on, and the failure is told to the sessions
        // followed next.
        if self.threat_failure.is_none()
            && let Some((message, told)) = slot.search.as_ref().and_then(ThreatSearch::failure)
        {
            self.threat_failure = Some(ThreatFailure { message, start_failed: false, told });
        }
        if let Some(f) = self.threat_failure.as_mut().filter(|f| f.told != session_id) {
            f.told = session_id;
            self.events.threat(failure_dto(session_id, &f.message, f.start_failed));
        }
        let threat = slot.search.as_mut().filter(|t| t.sent.as_ref() != Some(&request) && t.is_alive());
        if !moved && threat.is_none() {
            return;
        }
        let pos = game.position_at(cursor);
        let fen = position::to_fen(&pos);
        if let Some(t) = threat {
            t.follow(session_id, &request, &pos, &fen);
        }
        drop(slot);
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

/// What the threat engine's thread shares with its [`ThreatSearch`].
struct ThreatShared {
    /// The session followed. Kept when a note clears the target, so a
    /// failure names the game it was told to.
    session: u32,
    /// What the engine is searching, while it searches.
    target: Option<ThreatTarget>,
    /// The engine failed and said so: nothing more is searched or said.
    failure: Option<String>,
}

/// What the threat engine is searching: the position on the board and the
/// one after the pass.
struct ThreatTarget {
    request: u64,
    /// Its events' [`ThreatDto::seq`].
    seq: u64,
    fen: String,
    after_pass: Chess,
}

/// The threat search (ENG-07): a second engine process that searches the
/// position on the board as if the side to move had passed (the module
/// comment says why it's a process of its own).
pub struct ThreatSearch {
    analyzer: Analyzer,
    shared: Arc<Mutex<ThreatShared>>,
    events: Events,
    /// The position last followed, as the main analysis keeps it.
    sent: Option<SearchPosition>,
}

impl ThreatSearch {
    /// Starts the engine of `config` for threats in `session` (blocking: up
    /// to the handshake timeout).
    pub fn start(config: &EngineConfig, session: u32, events: Events) -> Result<ThreatSearch, EngineError> {
        let shared = Arc::new(Mutex::new(ThreatShared { session, target: None, failure: None }));
        let (state, sink) = (shared.clone(), events.clone());
        let options = threat::engine_options(&config.declared, &usable_options(config));
        let analyzer = Analyzer::start(&config.path, &options, move |event| {
            // Put together under the lock, so its number fits in with the
            // notes `follow` numbers.
            let mut s = state.lock().expect("threat lock");
            let dto = match event {
                AnalyzerEvent::Update(update) => match &s.target {
                    Some(t) if t.request == update.request => threat_dto(s.session, t, &update),
                    _ => return,
                },
                AnalyzerEvent::Failed(message) => {
                    s.target = None;
                    let dto = failure_dto(s.session, &message, false);
                    s.failure = Some(message);
                    dto
                }
            };
            drop(s);
            sink.threat(dto);
        })?;
        Ok(ThreatSearch { analyzer, shared, events, sent: None })
    }

    /// False once its engine failed.
    pub fn is_alive(&self) -> bool {
        self.analyzer.is_alive() && self.shared.lock().expect("threat lock").failure.is_none()
    }

    /// Why its engine failed, and the session that was told.
    fn failure(&self) -> Option<(String, u32)> {
        let shared = self.shared.lock().expect("threat lock");
        shared.failure.clone().map(|m| (m, shared.session))
    }

    /// Follows the analysis to another session: the next follow searches.
    fn attach(&mut self) {
        self.sent = None;
    }

    /// Searches the threat in `pos` (`request` on the board), or says why
    /// there's none.
    fn follow(&mut self, session: u32, request: &SearchPosition, pos: &Chess, fen: &str) {
        self.sent = Some(request.clone());
        let mut shared = self.shared.lock().expect("threat lock");
        if shared.failure.is_some() {
            return;
        }
        shared.session = session;
        match threat::null_move(pos) {
            Ok(after_pass) => {
                // The target is set under the lock, as for the main search.
                let id = self.analyzer.analyze(AnalysisRequest {
                    position: threat::search_position(&after_pass),
                    multipv: 1,
                    limits: THREAT_LIMITS,
                });
                let seq = next_threat_seq();
                shared.target = Some(ThreatTarget { request: id, seq, fen: fen.to_owned(), after_pass });
            }
            Err(reason) => {
                shared.target = None;
                // Numbered under the lock: an update of the last search that
                // is still on its way has an older number.
                let seq = next_threat_seq();
                drop(shared);
                self.analyzer.stop();
                let unavailable = match reason {
                    NoThreat::GameOver => NoThreatDto::GameOver,
                    NoThreat::InCheck => NoThreatDto::InCheck,
                    NoThreat::NoMoves => NoThreatDto::NoMoves,
                };
                self.events.threat(ThreatDto {
                    seq,
                    session,
                    fen: fen.to_owned(),
                    finished: true,
                    unavailable: Some(unavailable),
                    ..ThreatDto::default()
                });
            }
        }
    }
}

fn threat_dto(session: u32, t: &ThreatTarget, u: &AnalysisUpdate) -> ThreatDto {
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
        seq: t.seq,
        session,
        fen: t.fen.clone(),
        depth: u.depth,
        finished: u.finished,
        line,
        ..ThreatDto::default()
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

/// Helpers for tests that run the toy engine of `chess-engine`.
#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, channel};
    use std::time::{Duration, Instant};

    use chess_core::game::{Game, GameEditor};
    use chess_core::position;

    use super::{EngineEvent, Events};
    use crate::dto::{AnalysisDto, ThreatDto};
    use crate::settings::EngineConfig;
    use crate::state::Session;

    /// 1.e4 e5 2.Bc4 Nc6 3.Qh5, Black to move: White threatens 4.Qxf7#.
    pub const SCHOLAR: &str = "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 3 3";
    /// 1.e4 f5 2.Qh5+, Black in check: no threat.
    pub const CHECK: &str = "rnbqkbnr/ppppp1pp/8/5p1Q/4P3/8/PPPP1PPP/RNB1KBNR b KQkq - 1 2";
    pub const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

    /// The toy engine (`crates/chess-engine/src/bin/fake_uci.rs`), which
    /// `cargo test --workspace` builds next to the folder of this test.
    pub fn fake_uci() -> PathBuf {
        let exe = std::env::current_exe().expect("the test's path");
        let dir = exe.parent().and_then(Path::parent).expect("the target folder");
        let path = dir.join(format!("fake-uci{}", std::env::consts::EXE_SUFFIX));
        assert!(path.is_file(), "{} is missing: `cargo test --workspace` builds it", path.display());
        path
    }

    /// A copy of the toy engine in `dir` whose starts follow `script`
    /// (`ok`, `refuse`, `slow:<ms>`, one word per start; see fake_uci.rs).
    pub fn scripted_fake_uci(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join(format!("fake-uci{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(fake_uci(), &path).expect("copy the toy engine");
        std::fs::write(dir.join("starts.txt"), script).expect("write its script");
        path
    }

    /// The toy engine at `path` as the user would have it set up, with the
    /// option values `options`.
    pub fn config(path: PathBuf, options: &[(&str, &str)]) -> EngineConfig {
        let probe = chess_engine::Engine::start(&fake_uci()).expect("the toy engine starts");
        EngineConfig {
            id: "fake".into(),
            name: probe.id().name.clone(),
            path,
            detected: false,
            declared: probe.id().options.clone(),
            options: options.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
        }
    }

    /// Events into a channel.
    pub fn events() -> (Events, Receiver<EngineEvent>) {
        let (tx, rx) = channel();
        let events = Events::new(move |e| {
            let _ = tx.send(e);
        });
        (events, rx)
    }

    /// A game at `fen`.
    pub fn session_at(fen: &str) -> Session {
        let pos = position::from_fen(fen).expect("a FEN");
        Session::new(GameEditor::new(Game::new(Default::default(), Some(pos))), None)
    }

    /// The threat events that come within `wait`.
    pub fn threats_within(rx: &Receiver<EngineEvent>, wait: Duration) -> Vec<ThreatDto> {
        let end = Instant::now() + wait;
        let mut out = Vec::new();
        while let Ok(event) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            if let EngineEvent::Threat(t) = event {
                out.push(t);
            }
            if Instant::now() >= end {
                break;
            }
        }
        out
    }

    /// The first threat event `wanted` accepts, within 5 s.
    pub fn threat_where(rx: &Receiver<EngineEvent>, wanted: impl Fn(&ThreatDto) -> bool) -> ThreatDto {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(EngineEvent::Threat(t)) if wanted(&t) => return t,
                Ok(_) => {}
                Err(_) => panic!("no such threat event within 5 s"),
            }
        }
    }

    /// The first analysis event `wanted` accepts, within `wait`.
    pub fn analysis_where(
        rx: &Receiver<EngineEvent>,
        wait: Duration,
        wanted: impl Fn(&AnalysisDto) -> bool,
    ) -> Option<AnalysisDto> {
        let end = Instant::now() + wait;
        loop {
            match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(EngineEvent::Analysis(a)) if wanted(&a) => return Some(a),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::testing::*;
    use super::*;

    /// An analysis of the toy engine with the options `main`, with a threat
    /// search of it with the options `threat` attached.
    fn with_threat(main: &[(&str, &str)], threat: &[(&str, &str)], events: &Events) -> Analysis {
        let mut a =
            Analysis::start(&config(fake_uci(), main), 1, 1, events.clone()).expect("the engine starts");
        let (asked, _) = a.ask_threat(true);
        assert!(a.needs_threat_engine());
        let search = ThreatSearch::start(&config(fake_uci(), threat), 1, events.clone())
            .expect("the threat engine starts");
        let (attached, replaced) = a.offer_threat(asked, search);
        assert!(attached && replaced.is_none() && a.has_threat());
        a
    }

    #[test]
    fn the_threat_engine_quits_when_the_main_engine_fails() {
        let (events, rx) = events();
        let mut a = with_threat(&[("CrashOnGo", "true")], &[], &events);
        // The main engine exits on its first search.
        a.follow(1, &session_at(SCHOLAR));
        let failed = analysis_where(&rx, Duration::from_secs(5), |d| d.error.is_some());
        assert!(failed.is_some(), "the analysis reports the failure");
        // Its threat engine went with it, not at the next start or stop.
        assert!(!a.has_threat(), "the threat engine quit with the main one");
        // And one that finishes starting now isn't attached.
        let (asked, _) = a.ask_threat(true);
        assert!(!a.needs_threat_engine());
        let late = ThreatSearch::start(&config(fake_uci(), &[]), 1, events.clone()).unwrap();
        let (attached, unwanted) = a.offer_threat(asked, late);
        assert!(!attached && unwanted.is_some(), "a failed analysis takes no threat engine");
    }

    #[test]
    fn a_failed_threat_search_says_so_once_and_nothing_more() {
        let (events, rx) = events();
        let mut a = with_threat(&[], &[("CrashOnGo", "true")], &events);
        // The threat engine exits on its first search.
        a.follow(1, &session_at(SCHOLAR));
        let failed = threat_where(&rx, |t| t.error.is_some());
        assert_eq!((failed.session, failed.start_failed), (1, false), "names its game");
        assert!(!a.has_threat());
        // Neither a note in check nor a search after it: the page keeps
        // the failure and its hint to retry, not "Looking…".
        a.follow(1, &session_at(CHECK));
        a.follow(1, &session_at(START));
        assert_eq!(threats_within(&rx, Duration::from_millis(600)).len(), 0, "nothing after the failure");
        assert!(a.is_alive(), "the analysis goes on");
        // Another game (a new tab): the analysis follows it, and the
        // failure is told to it once, so its page shows it too.
        a.attach(2);
        a.follow(2, &session_at(SCHOLAR));
        let told = threats_within(&rx, Duration::from_millis(300));
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!((told[0].session, &told[0].error, told[0].start_failed), (2, &failed.error, false));
        assert!(told[0].seq > failed.seq);
        a.follow(2, &session_at(CHECK));
        assert_eq!(threats_within(&rx, Duration::from_millis(300)).len(), 0, "told once");
        // On again, even without off first (another window): the failed
        // search goes, so its failure isn't taken up again while a new one
        // starts, and a new one is needed.
        let (_, dead) = a.ask_threat(true);
        assert!(dead.is_some(), "the failed search goes");
        assert!(a.needs_threat_engine());
        a.attach(3);
        a.follow(3, &session_at(SCHOLAR));
        assert_eq!(threats_within(&rx, Duration::from_millis(300)).len(), 0, "the old failure isn't told");
        let (_, none) = a.ask_threat(false);
        assert!(none.is_none());
    }

    #[test]
    fn threat_events_are_numbered_in_the_order_they_were_decided() {
        let (events, rx) = events();
        let mut a = with_threat(&[], &[], &events);
        a.follow(1, &session_at(SCHOLAR));
        let line = threat_where(&rx, |t| t.finished && t.line.is_some());
        assert_eq!(line.line.as_ref().unwrap().moves[0].san, "Qxf7#");
        // An update of the search before was on its way when this note was
        // decided: it has a lower number, so the page keeps the note.
        a.follow(1, &session_at(CHECK));
        let note = threat_where(&rx, |t| t.unavailable.is_some());
        assert!(note.seq > line.seq);
        a.follow(1, &session_at(SCHOLAR));
        let again = threat_where(&rx, |t| t.line.is_some());
        assert!(again.seq > note.seq);
        // Another session: the threat search follows the analysis there.
        a.attach(2);
        a.follow(2, &session_at(SCHOLAR));
        assert_eq!(threat_where(&rx, |t| t.line.is_some() && t.seq > again.seq).session, 2);
    }
}
