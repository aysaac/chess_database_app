/// Game result as recorded in the `Result` tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameResult {
    WhiteWins,
    BlackWins,
    Draw,
    #[default]
    Unknown,
}

impl GameResult {
    pub fn as_str(self) -> &'static str {
        match self {
            GameResult::WhiteWins => "1-0",
            GameResult::BlackWins => "0-1",
            GameResult::Draw => "1/2-1/2",
            GameResult::Unknown => "*",
        }
    }

    pub fn parse(s: &str) -> GameResult {
        match s.trim() {
            "1-0" => GameResult::WhiteWins,
            "0-1" => GameResult::BlackWins,
            "1/2-1/2" | "½-½" => GameResult::Draw,
            _ => GameResult::Unknown,
        }
    }
}

/// PGN headers. Empty strings mean "unknown" and are written as `?`
/// (`????.??.??` for the date).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    pub event: String,
    pub site: String,
    /// PGN date format `YYYY.MM.DD`, with `??` for unknown parts.
    pub date: String,
    pub round: String,
    pub white: String,
    pub black: String,
    pub result: GameResult,
    pub white_elo: Option<u16>,
    pub black_elo: Option<u16>,
    pub eco: String,
    pub annotator: String,
    /// All other tags in their original order.
    pub extra: Vec<(String, String)>,
}

impl Headers {
    /// Sets a tag by PGN name, routing known names to typed fields.
    /// `FEN`/`SetUp` are not headers here (see `Game::setup`).
    pub fn set(&mut self, name: &str, value: &str) {
        let value = value.trim();
        let unknown = matches!(value, "?" | "" | "????.??.??");
        let text = if unknown { String::new() } else { value.to_owned() };
        match name {
            "Event" => self.event = text,
            "Site" => self.site = text,
            "Date" => self.date = text,
            "Round" => self.round = text,
            "White" => self.white = text,
            "Black" => self.black = text,
            "Result" => self.result = GameResult::parse(value),
            "WhiteElo" => self.white_elo = parse_elo(value),
            "BlackElo" => self.black_elo = parse_elo(value),
            "ECO" => self.eco = text,
            "Annotator" => self.annotator = text,
            _ => {
                if let Some(slot) = self.extra.iter_mut().find(|(k, _)| k == name) {
                    slot.1 = value.to_owned();
                } else {
                    self.extra.push((name.to_owned(), value.to_owned()));
                }
            }
        }
    }

    pub fn get_extra(&self, name: &str) -> Option<&str> {
        self.extra.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Tags in PGN export order: Seven Tag Roster, then the typed optional
    /// tags, then extras. Unknown roster values are rendered as `?`.
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        fn or_q(s: &str) -> String {
            if s.is_empty() { "?".into() } else { s.into() }
        }
        let mut out = vec![
            ("Event".into(), or_q(&self.event)),
            ("Site".into(), or_q(&self.site)),
            ("Date".into(), if self.date.is_empty() { "????.??.??".into() } else { self.date.clone() }),
            ("Round".into(), or_q(&self.round)),
            ("White".into(), or_q(&self.white)),
            ("Black".into(), or_q(&self.black)),
            ("Result".into(), self.result.as_str().into()),
        ];
        if let Some(elo) = self.white_elo {
            out.push(("WhiteElo".into(), elo.to_string()));
        }
        if let Some(elo) = self.black_elo {
            out.push(("BlackElo".into(), elo.to_string()));
        }
        if !self.eco.is_empty() {
            out.push(("ECO".into(), self.eco.clone()));
        }
        if !self.annotator.is_empty() {
            out.push(("Annotator".into(), self.annotator.clone()));
        }
        out.extend(self.extra.iter().cloned());
        out
    }
}

fn parse_elo(s: &str) -> Option<u16> {
    s.trim().parse::<u16>().ok().filter(|&e| e > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_values_round_trip() {
        let mut h = Headers::default();
        h.set("Event", "?");
        h.set("Date", "????.??.??");
        h.set("WhiteElo", "?");
        h.set("TimeControl", "300+3");
        h.set("Result", "1/2-1/2");
        assert_eq!(h.event, "");
        assert_eq!(h.white_elo, None);
        assert_eq!(h.result, GameResult::Draw);
        let pairs = h.to_pairs();
        assert_eq!(pairs[0], ("Event".into(), "?".into()));
        assert_eq!(pairs[2], ("Date".into(), "????.??.??".into()));
        assert_eq!(pairs.last().unwrap(), &("TimeControl".into(), "300+3".into()));
    }
}
