//! Splits PGN comments into plain text, graphical annotations
//! (`[%csl …]`, `[%cal …]`) and other embedded commands (`[%clk …]`, …).

use shakmaty::Square;

use crate::game::{Shape, ShapeColor};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParsedComment {
    pub text: String,
    pub shapes: Vec<Shape>,
    pub commands: Vec<(String, String)>,
}

pub fn parse(raw: &str) -> ParsedComment {
    let mut out = ParsedComment::default();
    let mut text = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("[%") {
        let Some(len) = rest[start..].find(']') else { break };
        text.push_str(&rest[..start]);
        let body = rest[start + 2..start + len].trim();
        let (name, value) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
        let value = value.trim();
        match name {
            "csl" | "cal" => {
                for item in value.split(',') {
                    if let Some(shape) = parse_shape(item.trim()) {
                        out.shapes.push(shape);
                    }
                }
            }
            _ => out.commands.push((name.to_owned(), value.to_owned())),
        }
        rest = &rest[start + len + 1..];
    }
    text.push_str(rest);
    out.text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    out
}

fn parse_shape(item: &str) -> Option<Shape> {
    let mut chars = item.chars();
    let color = ShapeColor::from_letter(chars.next()?)?;
    let squares = chars.as_str().as_bytes();
    match squares.len() {
        2 => Some(Shape { color, from: Square::from_ascii(squares).ok()?, to: None }),
        4 => Some(Shape {
            color,
            from: Square::from_ascii(&squares[..2]).ok()?,
            to: Some(Square::from_ascii(&squares[2..]).ok()?),
        }),
        _ => None,
    }
}

/// Renders the command part of a comment (`[%csl …][%cal …][%clk …]`).
pub fn render_commands(shapes: &[Shape], commands: &[(String, String)]) -> String {
    let mut out = String::new();
    let squares: Vec<String> =
        shapes.iter().filter(|s| s.to.is_none()).map(|s| format!("{}{}", s.color.letter(), s.from)).collect();
    let arrows: Vec<String> = shapes
        .iter()
        .filter_map(|s| s.to.map(|to| format!("{}{}{}", s.color.letter(), s.from, to)))
        .collect();
    if !squares.is_empty() {
        out.push_str(&format!("[%csl {}]", squares.join(",")));
    }
    if !arrows.is_empty() {
        out.push_str(&format!("[%cal {}]", arrows.join(",")));
    }
    for (name, value) in commands {
        if value.is_empty() {
            out.push_str(&format!("[%{name}]"));
        } else {
            out.push_str(&format!("[%{name} {value}]"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_shapes_and_commands() {
        let c = parse("Good  move [%clk 0:03:00] [%cal Ge2e4,Rd1h5][%csl Ya4] really");
        assert_eq!(c.text, "Good move really");
        assert_eq!(c.commands, vec![("clk".into(), "0:03:00".into())]);
        assert_eq!(c.shapes.len(), 3);
        assert_eq!(c.shapes[0], Shape { color: ShapeColor::Green, from: Square::E2, to: Some(Square::E4) });
        assert_eq!(c.shapes[2], Shape { color: ShapeColor::Yellow, from: Square::A4, to: None });
        assert_eq!(render_commands(&c.shapes, &c.commands), "[%csl Ya4][%cal Ge2e4,Rd1h5][%clk 0:03:00]");
    }

    #[test]
    fn unterminated_command_is_text() {
        assert_eq!(parse("odd [%clk 1:00").text, "odd [%clk 1:00");
    }
}
