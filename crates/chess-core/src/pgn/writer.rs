//! PGN export in standard format: roster tags first, movetext wrapped at 80
//! columns, NAGs as `$n`, graphical annotations as `[%csl]`/`[%cal]`.

use shakmaty::san::SanPlus;
use shakmaty::{Chess, Color, Position};

use super::comment::render_commands;
use crate::game::{Game, Node, NodeId};
use crate::position;

const LINE_WIDTH: usize = 80;

/// Full PGN for one game, ending with a blank line.
pub fn write_game(game: &Game) -> String {
    let mut out = String::new();
    for (name, value) in game.headers.to_pairs() {
        push_tag(&mut out, &name, &value);
    }
    if let Some(setup) = &game.setup {
        push_tag(&mut out, "SetUp", "1");
        push_tag(&mut out, "FEN", &position::to_fen(setup));
    }
    out.push('\n');
    out.push_str(&write_movetext(game));
    out.push_str("\n\n");
    out
}

fn push_tag(out: &mut String, name: &str, value: &str) {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    out.push_str(&format!("[{name} \"{escaped}\"]\n"));
}

/// Movetext including the result token, without a trailing newline.
pub fn write_movetext(game: &Game) -> String {
    let mut w = Writer { out: String::new(), line_len: 0, prefix: None };
    let root = game.root();
    w.comment(&root.comment_after, root);
    let start = game.start_position();
    w.line(game, NodeId::ROOT, start, true);
    w.token(game.headers.result.as_str());
    w.out
}

struct Writer {
    out: String,
    line_len: usize,
    /// Glued to the next token (used for `(`).
    prefix: Option<&'static str>,
}

impl Writer {
    fn token(&mut self, tok: &str) {
        let prefix = self.prefix.take().unwrap_or("");
        let len = prefix.chars().count() + tok.chars().count();
        if self.line_len > 0 {
            if self.line_len + 1 + len > LINE_WIDTH {
                self.out.push('\n');
                self.line_len = 0;
            } else {
                self.out.push(' ');
                self.line_len += 1;
            }
        }
        self.out.push_str(prefix);
        self.out.push_str(tok);
        self.line_len += len;
    }

    fn close_paren(&mut self) {
        self.out.push(')');
        self.line_len += 1;
    }

    /// Writes `{commands text}` if there is anything to write. For the
    /// comment after a move, `node` supplies shapes and commands.
    fn comment(&mut self, text: &str, node: &Node) -> bool {
        self.comment_parts(&render_commands(&node.shapes, &node.commands), text)
    }

    fn comment_parts(&mut self, commands: &str, text: &str) -> bool {
        let text = text.replace('}', ")");
        let body = match (commands.is_empty(), text.is_empty()) {
            (true, true) => return false,
            (false, true) => commands.to_owned(),
            (true, false) => text,
            (false, false) => format!("{commands} {text}"),
        };
        let words: Vec<&str> = body.split_whitespace().collect();
        if words.is_empty() {
            return false;
        }
        let last = words.len() - 1;
        for (i, word) in words.iter().enumerate() {
            let mut tok = String::new();
            if i == 0 {
                tok.push('{');
            }
            tok.push_str(word);
            if i == last {
                tok.push('}');
            }
            self.token(&tok);
        }
        true
    }

    /// Writes one move with its annotations. Returns true if a comment
    /// followed it (so the next move needs its number repeated).
    fn mv(&mut self, game: &Game, id: NodeId, pos: &Chess, mut need_number: bool) -> bool {
        let node = game.node(id);
        if !node.comment_before.is_empty() {
            self.comment_parts("", &node.comment_before);
            need_number = true;
        }
        let number = pos.fullmoves().get();
        match pos.turn() {
            Color::White => self.token(&format!("{number}.")),
            Color::Black if need_number => self.token(&format!("{number}...")),
            Color::Black => {}
        }
        let mv = node.mv.expect("non-root node has a move");
        self.token(&SanPlus::from_move(pos.clone(), mv).to_string());
        for nag in &node.nags {
            self.token(&format!("${}", nag.0));
        }
        self.comment(&node.comment_after, node)
    }

    /// Writes the main continuation from `parent` with all variations.
    fn line(&mut self, game: &Game, mut parent: NodeId, mut pos: Chess, mut need_number: bool) {
        while let Some(&main) = game.node(parent).children.first() {
            let commented = self.mv(game, main, &pos, need_number);
            need_number = commented;

            for &alt in &game.node(parent).children[1..] {
                self.prefix = Some("(");
                let commented = self.mv(game, alt, &pos, true);
                let mut alt_pos = pos.clone();
                alt_pos.play_unchecked(game.node(alt).mv.expect("move"));
                self.line(game, alt, alt_pos, commented);
                self.close_paren();
                need_number = true;
            }

            pos.play_unchecked(game.node(main).mv.expect("move"));
            parent = main;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgn::read_pgn_str;

    #[test]
    fn writes_variations_comments_and_numbers() {
        let pgn = "{Intro} 1. e4 $1 e5 (1... c5 {Sicilian} 2. Nf3 (2. Nc3 Nc6) 2... d6) \
                   ({French} 1... e6 2. d4) 2. Nf3 {[%cal Gf3e5] attacks} 2... Nc6 *";
        let g = &read_pgn_str(pgn)[0];
        let text = write_movetext(g);
        assert_eq!(
            text.replace('\n', " "),
            "{Intro} 1. e4 $1 e5 (1... c5 {Sicilian} 2. Nf3 (2. Nc3 Nc6) 2... d6) \
             ({French} 1... e6 2. d4) 2. Nf3 {[%cal Gf3e5] attacks} 2... Nc6 *"
        );
        assert!(text.lines().all(|l| l.chars().count() <= LINE_WIDTH));
    }

    #[test]
    fn round_trips_full_game() {
        let pgn = r#"[Event "Casual \"blitz\""]
[Site "?"]
[Date "2024.01.02"]
[Round "?"]
[White "A"]
[Black "B"]
[Result "1/2-1/2"]
[WhiteElo "2100"]
[ECO "C20"]
[TimeControl "180+2"]

1. e4 {[%clk 0:03:00]} e5 {[%clk 0:03:00]} 2. Qh5 $2 {[%csl Rh5]} Nc6 3. Bc4 g6
4. Qf3 Nf6 5. Qb3 (5. Ne2 Bg7 6. d3) 5... Qe7 1/2-1/2
"#;
        let g = &read_pgn_str(pgn)[0];
        let written = write_game(g);
        let again = &read_pgn_str(&written)[0];
        assert_eq!(g, again);
        assert_eq!(write_game(again), written);
        assert!(written.contains(r#"[Event "Casual \"blitz\""]"#));
    }

    #[test]
    fn setup_position_starting_with_black() {
        let pgn = "[FEN \"4k3/8/8/8/8/8/4P3/4K3 b - - 0 12\"]\n\n12... Kd7 13. e4 *";
        let g = &read_pgn_str(pgn)[0];
        let written = write_game(g);
        assert!(written.contains("[SetUp \"1\"]"));
        assert!(written.trim_end().ends_with("12... Kd7 13. e4 *"));
        assert_eq!(&read_pgn_str(&written)[0], g);
    }
}
