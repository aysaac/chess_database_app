//! Interactive analysis (ENG-02): an engine on its own driver thread that
//! follows the position shown in the UI.
//!
//! Every [`Analyzer::analyze`] call replaces the running search; bursts of
//! calls (stepping through a game with the arrow keys) are coalesced so
//! only the last position is searched. Updates are throttled to about ten
//! per second.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::EngineError;
use crate::engine::{Engine, EngineId, Limits, SearchEvent, SearchPosition};
use crate::uci::Info;

const UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const POLL: Duration = Duration::from_millis(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisRequest {
    pub position: SearchPosition,
    /// Number of principal variations (1 = best move only).
    pub multipv: u32,
    pub limits: Limits,
}

/// The current state of a search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisUpdate {
    /// The id [`Analyzer::analyze`] returned for this search.
    pub request: u64,
    /// Latest line per principal variation, best first.
    pub lines: Vec<Info>,
    pub depth: u32,
    pub seldepth: u32,
    pub nodes: u64,
    pub nps: u64,
    pub time_ms: u64,
    pub hashfull: u32,
    /// True once the engine sent `bestmove` (limited searches only).
    pub finished: bool,
    pub best: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalyzerEvent {
    Update(AnalysisUpdate),
    /// The engine failed (crashed, stopped answering); the analyzer is done.
    Failed(String),
}

enum Cmd {
    Analyze(u64, AnalysisRequest),
    Stop,
    SetOption(String, String),
    Quit,
}

pub struct Analyzer {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    id: EngineId,
    next_request: u64,
}

impl Analyzer {
    /// Starts the engine, applies `options` (name, value) and returns once
    /// it is ready. Errors in `options` fail the start.
    pub fn start(
        path: &Path,
        options: &[(String, String)],
        on_event: impl Fn(AnalyzerEvent) + Send + 'static,
    ) -> Result<Analyzer, EngineError> {
        let mut engine = Engine::start(path)?;
        for (name, value) in options {
            engine.set_option(name, value)?;
        }
        if engine.has_option("UCI_ShowWDL") {
            engine.set_option("UCI_ShowWDL", "true")?;
        }
        engine.sync()?;
        let id = engine.id().clone();
        let (tx, rx) = channel();
        let thread = std::thread::Builder::new()
            .name(format!("analysis-{}", id.name))
            .spawn(move || drive(engine, rx, &on_event))
            .map_err(EngineError::Io)?;
        Ok(Analyzer { tx, thread: Some(thread), id, next_request: 1 })
    }

    pub fn id(&self) -> &EngineId {
        &self.id
    }

    /// Starts searching `request`, replacing any running search. Returns the
    /// id carried by this search's updates.
    pub fn analyze(&mut self, request: AnalysisRequest) -> u64 {
        let id = self.next_request;
        self.next_request += 1;
        let _ = self.tx.send(Cmd::Analyze(id, request));
        id
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    /// Changes an option; a running search restarts with it.
    pub fn set_option(&self, name: &str, value: &str) {
        let _ = self.tx.send(Cmd::SetOption(name.to_owned(), value.to_owned()));
    }

    /// False once the driver thread ended (engine failure).
    pub fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for Analyzer {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Search {
    id: u64,
    request: AnalysisRequest,
    update: AnalysisUpdate,
    dirty: bool,
    emitted: bool,
    last_emit: Instant,
}

impl Search {
    fn new(id: u64, request: AnalysisRequest) -> Search {
        Search {
            id,
            request,
            update: AnalysisUpdate { request: id, ..AnalysisUpdate::default() },
            dirty: false,
            emitted: false,
            last_emit: Instant::now(),
        }
    }

    fn absorb(&mut self, info: Info) {
        let u = &mut self.update;
        if let Some(d) = info.depth {
            u.depth = u.depth.max(d);
        }
        if let Some(d) = info.seldepth {
            u.seldepth = u.seldepth.max(d);
        }
        u.nodes = info.nodes.unwrap_or(u.nodes);
        u.nps = info.nps.unwrap_or(u.nps);
        u.time_ms = info.time_ms.unwrap_or(u.time_ms);
        u.hashfull = info.hashfull.unwrap_or(u.hashfull);
        if info.score.is_some() && !info.pv.is_empty() {
            let slot = info.multipv.unwrap_or(1).max(1) as usize - 1;
            if slot < self.request.multipv.max(1) as usize {
                if u.lines.len() <= slot {
                    u.lines.resize(slot + 1, Info::default());
                }
                u.lines[slot] = info;
            }
        }
        self.dirty = true;
    }

    fn emit_if_due(&mut self, on_event: &dyn Fn(AnalyzerEvent), force: bool) {
        let has_lines = self.update.lines.iter().any(|l| !l.pv.is_empty());
        if !self.dirty || !has_lines {
            if force {
                on_event(AnalyzerEvent::Update(self.snapshot()));
            }
            return;
        }
        if force || !self.emitted || self.last_emit.elapsed() >= UPDATE_INTERVAL {
            on_event(AnalyzerEvent::Update(self.snapshot()));
            self.dirty = false;
            self.emitted = true;
            self.last_emit = Instant::now();
        }
    }

    fn snapshot(&self) -> AnalysisUpdate {
        let mut u = self.update.clone();
        u.lines.retain(|l| !l.pv.is_empty());
        u
    }
}

fn drive(mut engine: Engine, rx: Receiver<Cmd>, on_event: &dyn Fn(AnalyzerEvent)) {
    if let Err(e) = run(&mut engine, &rx, on_event) {
        log::warn!("analysis engine {} failed: {e}", engine.id().name);
        on_event(AnalyzerEvent::Failed(e.to_string()));
    }
    engine.quit();
}

fn run(engine: &mut Engine, rx: &Receiver<Cmd>, on_event: &dyn Fn(AnalyzerEvent)) -> Result<(), EngineError> {
    let mut search: Option<Search> = None;
    let mut multipv = 1u32;
    loop {
        // Wait for commands while idle; poll them while searching.
        let first = if engine.is_searching() {
            match rx.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Cmd::Quit),
            }
        } else {
            Some(rx.recv().unwrap_or(Cmd::Quit))
        };

        if let Some(first) = first {
            let mut cmds = vec![first];
            cmds.extend(rx.try_iter());
            let mut quit = false;
            let mut stop = false;
            let mut analyze = None;
            let mut options = Vec::new();
            for cmd in cmds {
                match cmd {
                    Cmd::Quit => quit = true,
                    Cmd::Stop => {
                        stop = true;
                        analyze = None;
                    }
                    Cmd::Analyze(id, req) => {
                        stop = false;
                        analyze = Some((id, req));
                    }
                    Cmd::SetOption(name, value) => options.push((name, value)),
                }
            }
            if quit {
                return Ok(());
            }
            let was_searching = engine.is_searching();
            engine.stop()?;
            for (name, value) in &options {
                if let Err(e) = engine.set_option(name, value) {
                    log::warn!("option {name}={value} rejected: {e}");
                }
            }
            if stop {
                search = None;
            }
            // Options changed mid-search: search the same position again.
            if analyze.is_none() && was_searching && !options.is_empty() {
                analyze = search.take().map(|s| (s.id, s.request));
            }
            if let Some((id, request)) = analyze {
                let wanted = request.multipv.max(1);
                if wanted != multipv && engine.has_option("MultiPV") {
                    engine.set_option("MultiPV", &wanted.to_string())?;
                    multipv = wanted;
                }
                engine.go(&request.position, &request.limits)?;
                search = Some(Search::new(id, request));
            }
            continue;
        }

        match engine.next_event(POLL)? {
            Some(SearchEvent::Info(info)) => {
                if let Some(s) = &mut search {
                    s.absorb(info);
                }
            }
            Some(SearchEvent::BestMove { best, .. }) => {
                if let Some(s) = &mut search {
                    s.update.finished = true;
                    s.update.best = best;
                    s.dirty = true;
                    s.emit_if_due(on_event, true);
                }
            }
            None => {}
        }
        if let Some(s) = &mut search
            && engine.is_searching()
        {
            s.emit_if_due(on_event, false);
        }
    }
}
