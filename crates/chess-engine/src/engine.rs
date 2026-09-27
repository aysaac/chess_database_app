//! One UCI engine process: start-up handshake, options, searches.
//!
//! A reader thread forwards the engine's stdout line by line; the engine
//! is driven synchronously from one thread (a job, or the [`Analyzer`]'s
//! driver thread). All engine I/O is logged at `debug` level.
//!
//! [`Analyzer`]: crate::Analyzer

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use crate::EngineError;
use crate::uci::{Info, UciOption, parse_bestmove, parse_info, parse_option};

/// How long an engine may take to answer `uci` and `isready`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an engine may take to answer `stop` with `bestmove`.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// What the engine says about itself in answer to `uci`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineId {
    pub name: String,
    pub author: String,
    pub options: Vec<UciOption>,
}

/// Search limits for `go`. All `None` (and `infinite` false) means
/// `go infinite`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    pub depth: Option<u32>,
    pub nodes: Option<u64>,
    pub movetime_ms: Option<u64>,
    pub infinite: bool,
}

impl Limits {
    pub fn infinite() -> Limits {
        Limits { infinite: true, ..Limits::default() }
    }

    pub fn movetime(ms: u64) -> Limits {
        Limits { movetime_ms: Some(ms), ..Limits::default() }
    }

    fn command(&self) -> String {
        let mut cmd = String::from("go");
        if let Some(d) = self.depth {
            cmd.push_str(&format!(" depth {d}"));
        }
        if let Some(n) = self.nodes {
            cmd.push_str(&format!(" nodes {n}"));
        }
        if let Some(t) = self.movetime_ms {
            cmd.push_str(&format!(" movetime {t}"));
        }
        if self.infinite || cmd == "go" {
            cmd.push_str(" infinite");
        }
        cmd
    }
}

/// A position to search: a start FEN (`None` = the standard start) and the
/// moves played from it, in UCI notation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchPosition {
    pub fen: Option<String>,
    pub moves: Vec<String>,
}

impl SearchPosition {
    fn command(&self) -> String {
        let mut cmd = match &self.fen {
            Some(fen) => format!("position fen {fen}"),
            None => "position startpos".to_owned(),
        };
        if !self.moves.is_empty() {
            cmd.push_str(" moves");
            for m in &self.moves {
                cmd.push(' ');
                cmd.push_str(m);
            }
        }
        cmd
    }
}

/// Something the engine said while searching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    Info(Info),
    BestMove { best: Option<String>, ponder: Option<String> },
}

/// Result of a finished search: the best move and the last `info` of each
/// principal variation (index 0 = `multipv 1`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResult {
    pub best: Option<String>,
    pub lines: Vec<Info>,
}

pub struct Engine {
    path: PathBuf,
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    id: EngineId,
    searching: bool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").field("path", &self.path).field("name", &self.id.name).finish()
    }
}

impl Engine {
    /// Starts the engine at `path` and performs the `uci` handshake.
    pub fn start(path: &Path) -> Result<Engine, EngineError> {
        let mut cmd = Command::new(path);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Engines often look for their network files next to themselves.
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child =
            cmd.spawn().map_err(|source| EngineError::Spawn { path: path.to_path_buf(), source })?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

        let (tx, lines) = channel();
        let out_label = label.clone();
        std::thread::Builder::new()
            .name(format!("engine-out-{label}"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    log::debug!("{out_label} > {line}");
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .map_err(EngineError::Io)?;
        std::thread::Builder::new()
            .name(format!("engine-err-{label}"))
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::info!("{label} (stderr): {line}");
                }
            })
            .map_err(EngineError::Io)?;

        let mut engine = Engine {
            path: path.to_path_buf(),
            child,
            stdin,
            lines,
            id: EngineId { name: String::new(), author: String::new(), options: Vec::new() },
            searching: false,
        };
        engine.send("uci")?;
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        loop {
            let line = engine.recv_until(deadline, "uci")?;
            let (word, rest) = split_word(&line);
            match word {
                "id" => {
                    let (what, value) = split_word(rest);
                    match what {
                        "name" => engine.id.name = value.to_owned(),
                        "author" => engine.id.author = value.to_owned(),
                        _ => {}
                    }
                }
                "option" => {
                    if let Some(opt) = parse_option(rest) {
                        engine.id.options.push(opt);
                    }
                }
                "uciok" => break,
                _ => {}
            }
        }
        if engine.id.name.is_empty() {
            engine.id.name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
        Ok(engine)
    }

    pub fn id(&self) -> &EngineId {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn has_option(&self, name: &str) -> bool {
        self.option(name).is_some()
    }

    pub fn option(&self, name: &str) -> Option<&UciOption> {
        self.id.options.iter().find(|o| o.name().eq_ignore_ascii_case(name))
    }

    fn send(&mut self, cmd: &str) -> Result<(), EngineError> {
        log::debug!("{} < {cmd}", self.id.name);
        writeln!(self.stdin, "{cmd}").and_then(|_| self.stdin.flush()).map_err(|_| self.exited())
    }

    fn exited(&mut self) -> EngineError {
        let status = self.child.try_wait().ok().flatten();
        EngineError::Exited(status.map_or_else(|| "no longer responds".into(), |s| s.to_string()))
    }

    /// Next line before `deadline`; `what` names the awaited answer in errors.
    fn recv_until(&mut self, deadline: Instant, what: &str) -> Result<String, EngineError> {
        let wait = deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(wait) {
            Ok(line) => Ok(line),
            Err(RecvTimeoutError::Timeout) => Err(EngineError::Timeout(what.to_owned())),
            Err(RecvTimeoutError::Disconnected) => Err(self.exited()),
        }
    }

    /// Sets an option (`value` is ignored for buttons). Unknown options are
    /// an error; engines silently ignore them otherwise.
    pub fn set_option(&mut self, name: &str, value: &str) -> Result<(), EngineError> {
        let Some(opt) = self.option(name).cloned() else {
            return Err(EngineError::UnknownOption(name.to_owned()));
        };
        let cmd = match &opt {
            UciOption::Button { name } => format!("setoption name {name}"),
            _ => {
                let value = opt.validate(value).ok_or_else(|| EngineError::BadOptionValue {
                    name: opt.name().to_owned(),
                    value: value.to_owned(),
                })?;
                format!("setoption name {} value {value}", opt.name())
            }
        };
        self.send(&cmd)
    }

    /// Waits until the engine has processed everything sent so far.
    pub fn sync(&mut self) -> Result<(), EngineError> {
        self.send("isready")?;
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        loop {
            if self.recv_until(deadline, "isready")? == "readyok" {
                return Ok(());
            }
        }
    }

    pub fn new_game(&mut self) -> Result<(), EngineError> {
        self.send("ucinewgame")?;
        self.sync()
    }

    /// Starts a search. Read the output with [`Engine::next_event`] until
    /// [`SearchEvent::BestMove`].
    pub fn go(&mut self, position: &SearchPosition, limits: &Limits) -> Result<(), EngineError> {
        if self.searching {
            self.stop()?;
        }
        self.send(&position.command())?;
        self.send(&limits.command())?;
        self.searching = true;
        Ok(())
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    /// Next search event, waiting at most `timeout`. `Ok(None)` on timeout.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<SearchEvent>, EngineError> {
        let deadline = Instant::now() + timeout;
        loop {
            let line = match self.recv_until(deadline, "search output") {
                Ok(line) => line,
                Err(EngineError::Timeout(_)) => return Ok(None),
                Err(e) => {
                    self.searching = false;
                    return Err(e);
                }
            };
            let (word, rest) = split_word(&line);
            match word {
                "info" => return Ok(Some(SearchEvent::Info(parse_info(rest)))),
                "bestmove" => {
                    self.searching = false;
                    let (best, ponder) = parse_bestmove(rest);
                    return Ok(Some(SearchEvent::BestMove { best, ponder }));
                }
                _ => {}
            }
        }
    }

    /// Stops the running search and discards its remaining output.
    pub fn stop(&mut self) -> Result<(), EngineError> {
        if !self.searching {
            return Ok(());
        }
        self.send("stop")?;
        let deadline = Instant::now() + STOP_TIMEOUT;
        while self.searching {
            let wait = deadline.saturating_duration_since(Instant::now());
            if self.next_event(wait)?.is_none() && Instant::now() >= deadline {
                return Err(EngineError::Timeout("stop".into()));
            }
        }
        Ok(())
    }

    /// Runs a search to completion. `on_info` sees every `info` line;
    /// `cancelled` is polled and stops the search early (the result then
    /// holds what was found so far).
    pub fn search(
        &mut self,
        position: &SearchPosition,
        limits: &Limits,
        on_info: &mut dyn FnMut(&Info),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SearchResult, EngineError> {
        self.go(position, limits)?;
        let mut result = SearchResult::default();
        let mut stop_sent = false;
        loop {
            if !stop_sent && cancelled() {
                self.send("stop")?;
                stop_sent = true;
            }
            match self.next_event(Duration::from_millis(50))? {
                None => {}
                Some(SearchEvent::Info(info)) => {
                    on_info(&info);
                    if info.score.is_some() && !info.pv.is_empty() {
                        let slot = info.multipv.unwrap_or(1).max(1) as usize - 1;
                        if result.lines.len() <= slot {
                            result.lines.resize(slot + 1, Info::default());
                        }
                        result.lines[slot] = info;
                    }
                }
                Some(SearchEvent::BestMove { best, .. }) => {
                    result.best = best;
                    // Drop placeholders for lines the engine never filled.
                    result.lines.retain(|l| !l.pv.is_empty());
                    return Ok(result);
                }
            }
        }
    }

    /// Asks the engine to quit and waits briefly; kills it otherwise.
    pub fn quit(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = self.stop();
        let _ = self.send("quit");
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.split_once(char::is_whitespace) {
        Some((w, rest)) => (w, rest.trim_start()),
        None => (s, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_commands() {
        assert_eq!(Limits::default().command(), "go infinite");
        assert_eq!(Limits::movetime(500).command(), "go movetime 500");
        assert_eq!(
            Limits { depth: Some(12), nodes: Some(1000), ..Limits::default() }.command(),
            "go depth 12 nodes 1000"
        );
        assert_eq!(SearchPosition::default().command(), "position startpos");
        let pos =
            SearchPosition { fen: Some("8/8/8/8/8/8/8/K6k w - - 0 1".into()), moves: vec!["a1a2".into()] };
        assert_eq!(pos.command(), "position fen 8/8/8/8/8/8/8/K6k w - - 0 1 moves a1a2");
    }

    #[test]
    fn splits_words() {
        assert_eq!(split_word("id name Stockfish 19"), ("id", "name Stockfish 19"));
        assert_eq!(split_word("uciok"), ("uciok", ""));
    }
}
