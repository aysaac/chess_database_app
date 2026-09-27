//! Streaming PGN reader built on `pgn_reader`, producing [`Game`] trees.
//!
//! Tolerance policy:
//! - An illegal or unparseable move ends the line it occurs in: the rest of
//!   that variation (or of the whole game, on the main line) is dropped and a
//!   warning is recorded.
//! - Games with an unsupported `Variant` or an invalid `FEN` tag are skipped
//!   with a warning.

use std::io::{self, Read};
use std::ops::ControlFlow;

use pgn_reader::{Nag as PgnNag, Outcome, RawComment, RawTag, Reader, SanPlus, Skip, Visitor};
use shakmaty::san::SanError;
use shakmaty::{Chess, Position};

use super::{comment, decode_text};
use crate::game::{Game, GameResult, Headers, Nag, NodeId};
use crate::{Error, position};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgnWarning {
    /// Zero-based index of the game in the input.
    pub game_index: usize,
    pub message: String,
}

#[derive(Debug)]
pub struct ParsedGame {
    pub index: usize,
    /// `None` if the game was skipped (see `warnings`).
    pub game: Option<Game>,
    pub warnings: Vec<PgnWarning>,
}

/// Iterates over the games of a PGN stream.
pub struct PgnReader<R: Read> {
    reader: Reader<R>,
    builder: Builder,
}

impl<R: Read> PgnReader<R> {
    pub fn new(input: R) -> PgnReader<R> {
        PgnReader {
            reader: Reader::build(input)
                .set_supported_tag_line_length(4096)
                .set_supported_comment_length(16 * 1024)
                .finish(),
            builder: Builder { index: 0 },
        }
    }
}

impl<R: Read> Iterator for PgnReader<R> {
    type Item = io::Result<ParsedGame>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.reader.read_game(&mut self.builder) {
            Ok(Some(parsed)) => {
                self.builder.index += 1;
                Some(Ok(parsed))
            }
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

/// Parses all games in a string; skipped games are dropped.
pub fn read_pgn_str(pgn: &str) -> Vec<Game> {
    PgnReader::new(pgn.as_bytes()).filter_map(|r| r.ok().and_then(|p| p.game)).collect()
}

struct Builder {
    index: usize,
}

struct Tags {
    headers: Headers,
    setup: Option<Chess>,
    skip: Option<String>,
}

enum Frame {
    /// A variation we descended into; restore this line state on `)`.
    Line(Box<LineState>),
    /// A variation we skipped (or entered while the line was dead).
    Skipped,
}

struct LineState {
    current: NodeId,
    pos: Chess,
    pos_before: Option<Chess>,
}

struct Movetext {
    game: Game,
    index: usize,
    warnings: Vec<PgnWarning>,
    /// Last node of the line being read.
    current: NodeId,
    /// Position after `current`.
    pos: Chess,
    /// Position before `current`'s move; `None` at the start of a line.
    pos_before: Option<Chess>,
    frames: Vec<Frame>,
    /// Depth at which an illegal move ended the line.
    dead_at: Option<usize>,
    /// Comment seen at the start of a variation, before its first move.
    pending_before: Option<comment::ParsedComment>,
    partial_comment: Vec<u8>,
    termination: GameResult,
}

impl Movetext {
    fn warn(&mut self, message: String) {
        self.warnings.push(PgnWarning { game_index: self.index, message });
    }

    fn is_dead(&self) -> bool {
        self.dead_at.is_some()
    }

    fn apply_comment(&mut self, parsed: comment::ParsedComment) {
        let at_line_start = self.pos_before.is_none();
        if at_line_start && !self.frames.is_empty() {
            // `( {comment} 12. Nf3 … )`: belongs before the next move.
            match &mut self.pending_before {
                Some(p) => {
                    append_text(&mut p.text, &parsed.text);
                    p.shapes.extend(parsed.shapes);
                    p.commands.extend(parsed.commands);
                }
                None => self.pending_before = Some(parsed),
            }
            return;
        }
        let node = self.game.node_mut(self.current);
        append_text(&mut node.comment_after, &parsed.text);
        node.shapes.extend(parsed.shapes);
        node.commands.extend(parsed.commands);
    }
}

fn append_text(dst: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    if !dst.is_empty() {
        dst.push(' ');
    }
    dst.push_str(text);
}

impl Visitor for Builder {
    type Tags = Tags;
    type Movetext = Movetext;
    type Output = ParsedGame;

    fn begin_tags(&mut self) -> ControlFlow<ParsedGame, Tags> {
        ControlFlow::Continue(Tags { headers: Headers::default(), setup: None, skip: None })
    }

    fn tag(&mut self, tags: &mut Tags, name: &[u8], value: RawTag<'_>) -> ControlFlow<ParsedGame> {
        let name = decode_text(name);
        let value = decode_text(&value.decode());
        // The first reason to skip a game wins.
        match name.as_str() {
            "FEN" => match position::from_fen(&value) {
                Ok(pos) => tags.setup = Some(pos),
                Err(e) => {
                    let Error::InvalidFen(why) = e;
                    tags.skip.get_or_insert(format!("The FEN tag can't be read: {why}."));
                }
            },
            "SetUp" => {}
            "Variant" => {
                let v = value.to_ascii_lowercase();
                if !matches!(v.as_str(), "" | "standard" | "chess" | "normal" | "from position") {
                    tags.skip.get_or_insert(format!("It's a {value} game, which isn't supported."));
                }
            }
            _ => tags.headers.set(&name, &value),
        }
        ControlFlow::Continue(())
    }

    fn begin_movetext(&mut self, tags: Tags) -> ControlFlow<ParsedGame, Movetext> {
        if let Some(reason) = tags.skip {
            return ControlFlow::Break(ParsedGame {
                index: self.index,
                game: None,
                warnings: vec![PgnWarning { game_index: self.index, message: reason }],
            });
        }
        let game = Game::new(tags.headers, tags.setup);
        let pos = game.start_position();
        ControlFlow::Continue(Movetext {
            game,
            index: self.index,
            warnings: Vec::new(),
            current: NodeId::ROOT,
            pos,
            pos_before: None,
            frames: Vec::new(),
            dead_at: None,
            pending_before: None,
            partial_comment: Vec::new(),
            termination: GameResult::Unknown,
        })
    }

    fn san(&mut self, mt: &mut Movetext, san_plus: SanPlus) -> ControlFlow<ParsedGame> {
        if mt.is_dead() {
            return ControlFlow::Continue(());
        }
        match san_plus.san.to_move(&mt.pos) {
            Ok(m) => {
                let parent = mt.current;
                let node = mt.game.add_child(parent, m);
                if let Some(before) = mt.pending_before.take() {
                    let n = mt.game.node_mut(node);
                    n.comment_before = before.text;
                    n.shapes = before.shapes;
                    n.commands = before.commands;
                }
                mt.pos_before = Some(mt.pos.clone());
                mt.pos.play_unchecked(m);
                mt.current = node;
            }
            Err(e) => {
                let depth = mt.frames.len();
                let where_ = if depth == 0 { "main line" } else { "variation" };
                let number = mt.pos.fullmoves().get();
                let dots = if mt.pos.turn().is_white() { "." } else { "..." };
                let why = match e {
                    SanError::AmbiguousSan => "is ambiguous (more than one piece can make it)",
                    _ => "isn't a legal move there",
                };
                mt.warn(format!("The {where_} stops at {number}{dots}{san_plus}, which {why}."));
                mt.dead_at = Some(depth);
            }
        }
        ControlFlow::Continue(())
    }

    fn nag(&mut self, mt: &mut Movetext, nag: PgnNag) -> ControlFlow<ParsedGame> {
        if !mt.is_dead() && mt.pos_before.is_some() {
            let nags = &mut mt.game.node_mut(mt.current).nags;
            if !nags.contains(&Nag(nag.0)) {
                nags.push(Nag(nag.0));
            }
        }
        ControlFlow::Continue(())
    }

    fn partial_comment(&mut self, mt: &mut Movetext, c: RawComment<'_>) -> ControlFlow<ParsedGame> {
        mt.partial_comment.extend_from_slice(c.as_bytes());
        ControlFlow::Continue(())
    }

    fn comment(&mut self, mt: &mut Movetext, c: RawComment<'_>) -> ControlFlow<ParsedGame> {
        let mut bytes = std::mem::take(&mut mt.partial_comment);
        bytes.extend_from_slice(c.as_bytes());
        if !mt.is_dead() {
            let parsed = comment::parse(&decode_text(&bytes));
            mt.apply_comment(parsed);
        }
        ControlFlow::Continue(())
    }

    fn begin_variation(&mut self, mt: &mut Movetext) -> ControlFlow<ParsedGame, Skip> {
        if mt.is_dead() || mt.pos_before.is_none() {
            // Dead line, or `(` before any move of the current line.
            mt.frames.push(Frame::Skipped);
            return ControlFlow::Continue(Skip(true));
        }
        // The variation is an alternative to `current`: continue from the
        // position before it, and restore the line state on `)`.
        let parent = mt.game.node(mt.current).parent.expect("line has a move");
        let before = mt.pos_before.take().expect("checked above");
        let after = std::mem::replace(&mut mt.pos, before.clone());
        mt.frames.push(Frame::Line(Box::new(LineState {
            current: mt.current,
            pos: after,
            pos_before: Some(before),
        })));
        mt.current = parent;
        mt.pending_before = None;
        ControlFlow::Continue(Skip(false))
    }

    fn end_variation(&mut self, mt: &mut Movetext) -> ControlFlow<ParsedGame> {
        match mt.frames.pop() {
            None => {} // unmatched `)`
            Some(Frame::Skipped) => {}
            Some(Frame::Line(line)) => {
                let LineState { current, pos, pos_before } = *line;
                mt.current = current;
                mt.pos = pos;
                mt.pos_before = pos_before;
                mt.pending_before = None;
                if mt.dead_at.is_some_and(|d| mt.frames.len() < d) {
                    mt.dead_at = None;
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn outcome(&mut self, mt: &mut Movetext, outcome: Outcome) -> ControlFlow<ParsedGame> {
        mt.termination = GameResult::parse(outcome.as_str());
        ControlFlow::Continue(())
    }

    fn end_game(&mut self, mut mt: Movetext) -> ParsedGame {
        if mt.game.headers.result == GameResult::Unknown {
            mt.game.headers.result = mt.termination;
        }
        ParsedGame { index: mt.index, game: Some(mt.game), warnings: mt.warnings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation;

    fn sans_of_line(g: &Game, from: NodeId) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = from;
        loop {
            let pos = g.position_at(cur);
            let Some(&next) = g.node(cur).children.first() else { break };
            out.push(notation::san(&pos, g.node(next).mv.unwrap()));
            cur = next;
        }
        out
    }

    #[test]
    fn reads_headers_moves_and_result() {
        let games = read_pgn_str(
            r#"[Event "Test"]
[White "Réti, Richard"]
[Black "?"]
[Result "1-0"]
[WhiteElo "2600"]
[TimeControl "40/7200"]

1. e4 e5 2. Nf3 Nc6 1-0
"#,
        );
        assert_eq!(games.len(), 1);
        let g = &games[0];
        assert_eq!(g.headers.event, "Test");
        assert_eq!(g.headers.white, "Réti, Richard");
        assert_eq!(g.headers.black, "");
        assert_eq!(g.headers.result, GameResult::WhiteWins);
        assert_eq!(g.headers.white_elo, Some(2600));
        assert_eq!(g.headers.get_extra("TimeControl"), Some("40/7200"));
        assert_eq!(sans_of_line(g, NodeId::ROOT), ["e4", "e5", "Nf3", "Nc6"]);
    }

    #[test]
    fn nested_variations_and_annotations() {
        let g = &read_pgn_str(
            "{Intro} 1. e4 $1 e5 (1... c5 {Sicilian} 2. Nf3 (2. Nc3 Nc6) 2... d6) \
             ({French} 1... e6 2. d4) 2. Nf3 {[%cal Gf3e5] attacks} Nc6 *",
        )[0];
        assert_eq!(g.root().comment_after, "Intro");
        assert_eq!(sans_of_line(g, NodeId::ROOT), ["e4", "e5", "Nf3", "Nc6"]);
        let e4 = g.root().children[0];
        assert_eq!(g.node(e4).nags, vec![Nag(1)]);
        let alts = &g.node(e4).children;
        assert_eq!(alts.len(), 3);
        let c5 = alts[1];
        assert_eq!(g.node(c5).comment_after, "Sicilian");
        assert_eq!(sans_of_line(g, c5), ["Nf3", "d6"]);
        let c5_children = &g.node(c5).children;
        assert_eq!(c5_children.len(), 2, "2. Nc3 is an alternative to 2. Nf3");
        let e6 = alts[2];
        assert_eq!(g.node(e6).comment_before, "French");
        let nf3 = g.node(g.node(e4).children[0]).children[0];
        assert_eq!(g.node(nf3).comment_after, "attacks");
        assert_eq!(g.node(nf3).shapes.len(), 1);
    }

    #[test]
    fn illegal_move_in_variation_cuts_only_that_variation() {
        let mut r = PgnReader::new(&b"1. e4 e5 (1... c5 2. Ke3 Nc6 (2... d6)) 2. Nf3 *"[..]);
        let parsed = r.next().unwrap().unwrap();
        let g = parsed.game.unwrap();
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.warnings[0].message.contains("variation"));
        assert_eq!(sans_of_line(&g, NodeId::ROOT), ["e4", "e5", "Nf3"]);
        let c5 = g.node(g.root().children[0]).children[1];
        assert!(g.node(c5).children.is_empty());
    }

    #[test]
    fn illegal_move_in_main_line_truncates_game() {
        let mut r = PgnReader::new(&b"1. e4 e5 2. Ke3 Nc6 3. Nf3 1-0\n\n1. d4 *"[..]);
        let first = r.next().unwrap().unwrap();
        assert_eq!(first.game.unwrap().mainline_len(), 2);
        assert_eq!(first.warnings.len(), 1);
        assert_eq!(
            first.warnings[0].message,
            "The main line stops at 2.Ke3, which isn't a legal move there."
        );
        let second = r.next().unwrap().unwrap();
        assert_eq!(second.game.unwrap().mainline_len(), 1);
        assert!(r.next().is_none());
    }

    #[test]
    fn setup_position_and_skipped_variants() {
        let pgn = "[Variant \"Chess960\"]\n[FEN \"bqnb1rkr/pp3ppp/3ppn2/2p5/5P2/P2P4/NPP1P1PP/BQ1BNRKR w HFhf - 2 9\"]\n\n9. g3 *\n\n\
                   [SetUp \"1\"]\n[FEN \"4k3/8/8/8/8/8/4P3/4K3 w - - 0 1\"]\n\n1. e4 Kd7 *\n";
        let parsed: Vec<_> = PgnReader::new(pgn.as_bytes()).map(|r| r.unwrap()).collect();
        assert!(parsed[0].game.is_none());
        assert!(parsed[0].warnings[0].message.contains("Chess960"));
        let g = parsed[1].game.as_ref().unwrap();
        assert!(g.setup.is_some());
        assert_eq!(sans_of_line(g, NodeId::ROOT), ["e4", "Kd7"]);
    }

    #[test]
    fn result_from_termination_when_tag_missing() {
        let g = &read_pgn_str("1. f3 e5 2. g4 Qh4# 0-1")[0];
        assert_eq!(g.headers.result, GameResult::BlackWins);
    }
}
