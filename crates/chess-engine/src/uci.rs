//! UCI protocol parsing: `info`, `bestmove` and `option` lines.
//!
//! Scores are kept as the engine reports them, from the side to move's
//! point of view. [`Score::for_white`] turns them around for display.

use serde::{Deserialize, Serialize};

/// Engine evaluation from the side to move's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Score {
    /// Centipawns.
    Cp(i32),
    /// Mate in N moves; negative when the side to move gets mated.
    Mate(i32),
}

impl Score {
    /// The same score from the other side's point of view.
    pub fn flip(self) -> Score {
        match self {
            Score::Cp(cp) => Score::Cp(-cp),
            Score::Mate(n) => Score::Mate(-n),
        }
    }

    /// Converts a side-to-move score into White's point of view.
    pub fn for_white(self, white_to_move: bool) -> Score {
        if white_to_move { self } else { self.flip() }
    }

    /// A single number that orders scores: mates beyond any centipawn value,
    /// shorter mates first.
    pub fn sort_key(self) -> i32 {
        const MATE: i32 = 100_000;
        match self {
            Score::Cp(cp) => cp.clamp(-MATE + 1000, MATE - 1000),
            Score::Mate(n) if n > 0 => MATE - n,
            Score::Mate(n) if n < 0 => -MATE - n,
            // "mate 0": the side to move is already mated.
            Score::Mate(_) => -MATE,
        }
    }

    /// Winning chances in 0..=1 for the side the score belongs to, using
    /// Lichess's logistic model for centipawns.
    pub fn win_chance(self) -> f64 {
        match self {
            Score::Cp(cp) => 1.0 / (1.0 + (-0.003_682_08 * f64::from(cp)).exp()),
            Score::Mate(n) if n > 0 => 1.0,
            Score::Mate(_) => 0.0,
        }
    }

    /// Short text as chess software shows it: `+0.35`, `-1.20`, `#3`, `#-2`.
    pub fn display(self) -> String {
        match self {
            Score::Cp(cp) => format!("{}{:.2}", if cp >= 0 { "+" } else { "-" }, f64::from(cp.abs()) / 100.0),
            Score::Mate(n) => format!("#{n}"),
        }
    }

    /// The PGN `[%eval]` form: `0.35`, `-1.2`, `#-3` (White's point of view
    /// is the caller's job).
    pub fn pgn_eval(self) -> String {
        match self {
            Score::Cp(cp) => {
                let sign = if cp < 0 { "-" } else { "" };
                let (int, rem) = (cp.unsigned_abs() / 100, cp.unsigned_abs() % 100);
                if rem % 10 == 0 {
                    format!("{sign}{int}.{}", rem / 10)
                } else {
                    format!("{sign}{int}.{rem:02}")
                }
            }
            Score::Mate(n) => format!("#{n}"),
        }
    }

    /// Parses the PGN `[%eval]` form (`0.35`, `-1.2`, `#-3`, `+0.1`).
    pub fn parse_pgn_eval(s: &str) -> Option<Score> {
        let s = s.trim();
        let s = s.split(',').next()?; // `[%eval 0.36,20]` carries a depth
        if let Some(m) = s.strip_prefix('#') {
            return m.trim_start_matches('+').parse().ok().map(Score::Mate);
        }
        let v: f64 = s.parse().ok()?;
        v.is_finite().then(|| Score::Cp((v * 100.0).round() as i32))
    }
}

/// Whether a score is exact or only a bound (aspiration window fail).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Bound {
    #[default]
    Exact,
    Lower,
    Upper,
}

/// One parsed `info` line. Fields the engine didn't send are `None`/empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub depth: Option<u32>,
    pub seldepth: Option<u32>,
    pub multipv: Option<u32>,
    pub score: Option<Score>,
    pub bound: Bound,
    /// Win/draw/loss per mille for the side to move (`UCI_ShowWDL`).
    pub wdl: Option<[u32; 3]>,
    pub nodes: Option<u64>,
    pub nps: Option<u64>,
    pub time_ms: Option<u64>,
    pub hashfull: Option<u32>,
    pub tbhits: Option<u64>,
    pub currmove: Option<String>,
    pub pv: Vec<String>,
    /// Free text after `string` (always the rest of the line).
    pub string: Option<String>,
}

/// Parses the rest of an `info` line (after the `info` keyword).
pub fn parse_info(rest: &str) -> Info {
    let mut info = Info::default();
    let mut tokens = rest.split_whitespace().peekable();
    while let Some(tok) = tokens.next() {
        let mut num = || tokens.next().and_then(|t| t.parse::<u64>().ok());
        match tok {
            "depth" => info.depth = num().map(|v| v as u32),
            "seldepth" => info.seldepth = num().map(|v| v as u32),
            "multipv" => info.multipv = num().map(|v| v as u32),
            "nodes" => info.nodes = num(),
            "nps" => info.nps = num(),
            "time" => info.time_ms = num(),
            "hashfull" => info.hashfull = num().map(|v| v as u32),
            "tbhits" => info.tbhits = num(),
            "currmove" => info.currmove = tokens.next().map(str::to_owned),
            "score" => {
                let kind = tokens.next();
                let value = tokens.next().and_then(|t| t.parse::<i32>().ok());
                info.score = match (kind, value) {
                    (Some("cp"), Some(v)) => Some(Score::Cp(v)),
                    (Some("mate"), Some(v)) => Some(Score::Mate(v)),
                    _ => None,
                };
                match tokens.peek() {
                    Some(&"lowerbound") => {
                        info.bound = Bound::Lower;
                        tokens.next();
                    }
                    Some(&"upperbound") => {
                        info.bound = Bound::Upper;
                        tokens.next();
                    }
                    _ => {}
                }
            }
            "wdl" => {
                let mut w = [0u32; 3];
                let mut ok = true;
                for slot in &mut w {
                    match tokens.next().and_then(|t| t.parse().ok()) {
                        Some(v) => *slot = v,
                        None => ok = false,
                    }
                }
                info.wdl = ok.then_some(w);
            }
            "pv" => {
                // `pv` runs to the end of the line, unless another keyword
                // follows (some engines put `string` last).
                while let Some(&t) = tokens.peek() {
                    if t == "string" {
                        break;
                    }
                    info.pv.push(t.to_owned());
                    tokens.next();
                }
            }
            "string" => {
                info.string = Some(tokens.by_ref().collect::<Vec<_>>().join(" "));
            }
            _ => {}
        }
    }
    info
}

/// Parses the rest of a `bestmove` line: the move and the optional ponder
/// move. `(none)` and `0000` (no legal move) give `None`.
pub fn parse_bestmove(rest: &str) -> (Option<String>, Option<String>) {
    let mut tokens = rest.split_whitespace();
    let best = tokens.next().filter(|m| *m != "(none)" && *m != "0000").map(str::to_owned);
    let ponder = match (tokens.next(), tokens.next()) {
        (Some("ponder"), Some(m)) => Some(m.to_owned()),
        _ => None,
    };
    (best, ponder)
}

/// A UCI option as declared by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UciOption {
    Check { name: String, default: bool },
    Spin { name: String, default: i64, min: i64, max: i64 },
    Combo { name: String, default: String, vars: Vec<String> },
    Button { name: String },
    String { name: String, default: String },
}

impl UciOption {
    pub fn name(&self) -> &str {
        match self {
            UciOption::Check { name, .. }
            | UciOption::Spin { name, .. }
            | UciOption::Combo { name, .. }
            | UciOption::Button { name }
            | UciOption::String { name, .. } => name,
        }
    }

    /// The default as the text sent with `setoption` (empty for buttons).
    pub fn default_value(&self) -> String {
        match self {
            UciOption::Check { default, .. } => default.to_string(),
            UciOption::Spin { default, .. } => default.to_string(),
            UciOption::Combo { default, .. } | UciOption::String { default, .. } => default.clone(),
            UciOption::Button { .. } => String::new(),
        }
    }

    /// Checks and normalizes a value for this option. Returns `None` if it
    /// doesn't fit (out of range, not a listed choice…).
    pub fn validate(&self, value: &str) -> Option<String> {
        let value = value.trim();
        match self {
            UciOption::Check { .. } => match value {
                "true" | "false" => Some(value.to_owned()),
                _ => None,
            },
            UciOption::Spin { min, max, .. } => {
                value.parse::<i64>().ok().filter(|v| (*min..=*max).contains(v)).map(|v| v.to_string())
            }
            UciOption::Combo { vars, .. } => {
                vars.iter().find(|v| v.eq_ignore_ascii_case(value)).map(|v| v.to_owned())
            }
            UciOption::Button { .. } => Some(String::new()),
            UciOption::String { .. } => Some(value.to_owned()),
        }
    }
}

const OPTION_KEYWORDS: [&str; 4] = ["default", "min", "max", "var"];

/// Parses the rest of an `option` line (after `option`). Names and string
/// values may contain spaces; `<empty>` stands for an empty string.
pub fn parse_option(rest: &str) -> Option<UciOption> {
    let rest = rest.trim().strip_prefix("name ")?;
    // The name runs up to the first ` type ` (it may contain other keywords).
    let (name, rest) = rest.split_once(" type ")?;
    let name = name.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let mut tokens = rest.split_whitespace();
    let kind = tokens.next()?;
    // (keyword, value) pairs; values run until the next keyword.
    let mut fields: Vec<(&str, Vec<&str>)> = Vec::new();
    for tok in tokens {
        if OPTION_KEYWORDS.contains(&tok) {
            fields.push((tok, Vec::new()));
        } else if let Some((_, value)) = fields.last_mut() {
            value.push(tok);
        }
    }
    let get = |key: &str| fields.iter().find(|(k, _)| *k == key).map(|(_, v)| v.join(" "));
    let text = |s: String| if s == "<empty>" { String::new() } else { s };
    let option = match kind {
        "check" => UciOption::Check { name, default: get("default").is_some_and(|d| d == "true") },
        "spin" => {
            let num = |k: &str| get(k).and_then(|v| v.parse::<i64>().ok());
            let min = num("min").unwrap_or(i64::MIN);
            let max = num("max").unwrap_or(i64::MAX);
            UciOption::Spin { name, default: num("default").unwrap_or(min.max(0)).clamp(min, max), min, max }
        }
        "combo" => {
            let vars = fields.iter().filter(|(k, _)| *k == "var").map(|(_, v)| v.join(" ")).collect();
            UciOption::Combo { name, default: get("default").unwrap_or_default(), vars }
        }
        "button" => UciOption::Button { name },
        "string" => UciOption::String { name, default: text(get("default").unwrap_or_default()) },
        _ => return None,
    };
    Some(option)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stockfish_info_lines() {
        let i = parse_info(
            "depth 22 seldepth 31 multipv 2 score cp -35 upperbound wdl 30 900 70 nodes 1234567 nps 987654 \
             hashfull 412 tbhits 0 time 1250 pv e7e5 g1f3 b8c6",
        );
        assert_eq!(i.depth, Some(22));
        assert_eq!(i.seldepth, Some(31));
        assert_eq!(i.multipv, Some(2));
        assert_eq!(i.score, Some(Score::Cp(-35)));
        assert_eq!(i.bound, Bound::Upper);
        assert_eq!(i.wdl, Some([30, 900, 70]));
        assert_eq!(i.nodes, Some(1_234_567));
        assert_eq!(i.nps, Some(987_654));
        assert_eq!(i.hashfull, Some(412));
        assert_eq!(i.time_ms, Some(1250));
        assert_eq!(i.pv, ["e7e5", "g1f3", "b8c6"]);

        let m = parse_info("depth 5 score mate -2 pv h7h8");
        assert_eq!(m.score, Some(Score::Mate(-2)));
        let s = parse_info("string NNUE evaluation using nn-1111.nnue enabled");
        assert_eq!(s.string.as_deref(), Some("NNUE evaluation using nn-1111.nnue enabled"));
        let c = parse_info("depth 12 currmove e2e4 currmovenumber 1");
        assert_eq!(c.currmove.as_deref(), Some("e2e4"));
        assert!(c.pv.is_empty());
    }

    #[test]
    fn parses_bestmove() {
        assert_eq!(parse_bestmove("e2e4 ponder e7e5"), (Some("e2e4".into()), Some("e7e5".into())));
        assert_eq!(parse_bestmove("g1f3"), (Some("g1f3".into()), None));
        assert_eq!(parse_bestmove("(none)"), (None, None));
        assert_eq!(parse_bestmove("0000"), (None, None));
    }

    #[test]
    fn parses_options() {
        assert_eq!(
            parse_option("name Threads type spin default 1 min 1 max 1024"),
            Some(UciOption::Spin { name: "Threads".into(), default: 1, min: 1, max: 1024 })
        );
        assert_eq!(
            parse_option("name UCI_ShowWDL type check default false"),
            Some(UciOption::Check { name: "UCI_ShowWDL".into(), default: false })
        );
        assert_eq!(
            parse_option("name SyzygyPath type string default <empty>"),
            Some(UciOption::String { name: "SyzygyPath".into(), default: String::new() })
        );
        assert_eq!(
            parse_option("name Clear Hash type button"),
            Some(UciOption::Button { name: "Clear Hash".into() })
        );
        assert_eq!(
            parse_option("name Style type combo default Normal var Solid var Normal var Risky Play"),
            Some(UciOption::Combo {
                name: "Style".into(),
                default: "Normal".into(),
                vars: vec!["Solid".into(), "Normal".into(), "Risky Play".into()],
            })
        );
        assert_eq!(
            parse_option("name Book max weight type spin default 5 min 0 max 10"),
            Some(UciOption::Spin { name: "Book max weight".into(), default: 5, min: 0, max: 10 })
        );
        assert_eq!(parse_option("name Weird type unknown"), None);
        assert_eq!(parse_option("type spin"), None);
    }

    #[test]
    fn option_validation() {
        let spin = UciOption::Spin { name: "Hash".into(), default: 16, min: 1, max: 1024 };
        assert_eq!(spin.validate(" 64 "), Some("64".into()));
        assert_eq!(spin.validate("0"), None);
        assert_eq!(spin.validate("lots"), None);
        let combo = UciOption::Combo { name: "S".into(), default: "a".into(), vars: vec!["Alpha".into()] };
        assert_eq!(combo.validate("alpha"), Some("Alpha".into()));
        assert_eq!(combo.validate("beta"), None);
    }

    #[test]
    fn score_helpers() {
        assert_eq!(Score::Cp(35).for_white(false), Score::Cp(-35));
        assert!(Score::Mate(1).sort_key() > Score::Mate(5).sort_key());
        assert!(Score::Mate(5).sort_key() > Score::Cp(5000).sort_key());
        assert!(Score::Mate(-1).sort_key() < Score::Mate(-5).sort_key());
        assert!(Score::Mate(-5).sort_key() < Score::Cp(-5000).sort_key());
        assert!((Score::Cp(0).win_chance() - 0.5).abs() < 1e-9);
        assert_eq!(Score::Cp(35).display(), "+0.35");
        assert_eq!(Score::Cp(-120).display(), "-1.20");
        assert_eq!(Score::Mate(-2).display(), "#-2");
        assert_eq!(Score::Cp(-120).pgn_eval(), "-1.2");
        assert_eq!(Score::Cp(5).pgn_eval(), "0.05");
        assert_eq!(Score::parse_pgn_eval("-1.2"), Some(Score::Cp(-120)));
        assert_eq!(Score::parse_pgn_eval("0.36,20"), Some(Score::Cp(36)));
        assert_eq!(Score::parse_pgn_eval("#-3"), Some(Score::Mate(-3)));
        assert_eq!(Score::parse_pgn_eval("nonsense"), None);
    }
}
