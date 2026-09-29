//! Board diagrams (QOL-06): a position as an SVG or PNG image, drawn like the
//! app's board: the square colours, coordinates, last move and check
//! highlights, the game's arrows and circles, and the cburnett pieces.
//!
//! The SVG is self-contained: pieces and coordinate labels are paths (the
//! labels outlined from an embedded font), so it looks the same in any
//! program. PNGs are rasterized from the same SVG with resvg.
//!
//! This crate has no UI dependencies, so printing (PUB-*) or a command line
//! tool can reuse it.

mod glyphs;

use std::fmt::{self, Write as _};
use std::str::FromStr;

use chess_core::game::{Shape, ShapeColor};
use chess_core::shakmaty::{Board, Color, File, Piece, Rank, Role, Square};
use resvg::{tiny_skia, usvg};

use crate::glyphs::glyphs;

/// Smallest and largest image, in pixels per side.
pub const MIN_SIZE: u32 = 64;
pub const MAX_SIZE: u32 = 4096;

/// Side of a square in SVG user units (the viewBox is 800 × 800).
const SQ: f64 = 100.0;

#[derive(Debug, thiserror::Error)]
pub enum DiagramError {
    #[error("{0:?} is not a colour; use #rrggbb")]
    BadColor(String),
    #[error("an image must be {MIN_SIZE} to {MAX_SIZE} pixels wide, not {0}")]
    BadSize(u32),
    #[error("couldn't draw the board image: {0}")]
    Render(String),
}

/// An sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `self` blended with `other`; `t` is `other`'s share (0 to 1).
    fn mix(self, other: Rgb, t: f64) -> Rgb {
        let m = |a: u8, b: u8| (f64::from(a) * (1.0 - t) + f64::from(b) * t).round() as u8;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// Parses `#rrggbb` or `#rgb`, the way the app's CSS writes colours.
impl FromStr for Rgb {
    type Err = DiagramError;

    fn from_str(s: &str) -> Result<Rgb, DiagramError> {
        let bad = || DiagramError::BadColor(s.to_owned());
        let hex = s.trim().strip_prefix('#').ok_or_else(bad)?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let digit = |i: usize, len: usize| u8::from_str_radix(&hex[i..i + len], 16).map_err(|_| bad());
        match hex.len() {
            6 => Ok(Rgb(digit(0, 2)?, digit(2, 2)?, digit(4, 2)?)),
            3 => Ok(Rgb(digit(0, 1)? * 17, digit(1, 1)? * 17, digit(2, 1)? * 17)),
            _ => Err(bad()),
        }
    }
}

/// Straight (not premultiplied) RGBA pixels, row by row.
#[derive(Debug, Clone)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// What to draw. The defaults are the app's "Tournament green" board.
#[derive(Debug, Clone)]
pub struct Diagram {
    pub board: Board,
    /// The side at the bottom.
    pub orientation: Color,
    /// Width and height in pixels.
    pub size: u32,
    pub light: Rgb,
    pub dark: Rgb,
    /// File and rank labels inside the edge squares, as on the app's board.
    pub coordinates: bool,
    /// Squares of the last move: from, to (the king's target when castling).
    pub last_move: Option<(Square, Square)>,
    /// The king in check.
    pub check: Option<Square>,
    /// Arrows and circled squares.
    pub shapes: Vec<Shape>,
    /// Text for the SVG's `<title>`, such as the FEN.
    pub title: Option<String>,
}

// Colours of the app's board (board-theme.css and chessground) that aren't
// options: the light-mode brass of the last move, the red glow of check, and
// chessground's default brushes for drawings.
const LAST_MOVE: Rgb = Rgb(0x9a, 0x6f, 0x1c);
const LAST_MOVE_OPACITY: f64 = 0.42;
const CHECK: Rgb = Rgb(200, 40, 20);
const SHAPES_OPACITY: f64 = 0.6;

fn brush(color: ShapeColor) -> Rgb {
    match color {
        ShapeColor::Green => Rgb(0x15, 0x78, 0x1b),
        ShapeColor::Red => Rgb(0x88, 0x20, 0x20),
        ShapeColor::Yellow => Rgb(0xe6, 0x8f, 0x00),
        ShapeColor::Blue => Rgb(0x00, 0x30, 0x88),
    }
}

impl Diagram {
    pub fn new(board: Board) -> Diagram {
        Diagram {
            board,
            orientation: Color::White,
            size: 480,
            light: Rgb(0xe6, 0xe2, 0xc8),
            dark: Rgb(0x6f, 0x8f, 0x6a),
            coordinates: true,
            last_move: None,
            check: None,
            shapes: Vec::new(),
            title: None,
        }
    }

    /// The diagram as a standalone SVG document.
    pub fn to_svg(&self) -> Result<String, DiagramError> {
        if !(MIN_SIZE..=MAX_SIZE).contains(&self.size) {
            return Err(DiagramError::BadSize(self.size));
        }
        let mut s = String::with_capacity(24 * 1024);
        let _ = write!(
            s,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{0}" height="{0}" viewBox="0 0 {1} {1}">"#,
            self.size,
            8.0 * SQ
        );
        if let Some(title) = &self.title {
            let _ = write!(s, "<title>{}</title>", escape(title));
        }
        self.write_defs(&mut s);

        // Squares: light everywhere, then the dark ones as one path.
        let _ = write!(s, r#"<rect width="{0}" height="{0}" fill="{1}"/>"#, 8.0 * SQ, self.light);
        let _ = write!(s, r#"<path fill="{}" d=""#, self.dark);
        for sq in Square::ALL.into_iter().filter(|&sq| is_dark(sq)) {
            let (x, y) = self.corner(sq);
            let _ = write!(s, "M{} {}h{2}v{2}h-{2}z", Num(x), Num(y), SQ);
        }
        s.push_str(r#""/>"#);

        if let Some((from, to)) = self.last_move {
            for sq in [Some(from), (to != from).then_some(to)].into_iter().flatten() {
                let (x, y) = self.corner(sq);
                let _ = write!(
                    s,
                    r#"<rect x="{}" y="{}" width="{SQ}" height="{SQ}" fill="{LAST_MOVE}" fill-opacity="{LAST_MOVE_OPACITY}"/>"#,
                    Num(x),
                    Num(y)
                );
            }
        }
        if let Some(king) = self.check {
            let (x, y) = self.corner(king);
            let _ = write!(
                s,
                r#"<rect x="{}" y="{}" width="{SQ}" height="{SQ}" fill="url(#check)"/>"#,
                Num(x),
                Num(y)
            );
        }

        for (sq, piece) in self.board.iter() {
            let (x, y) = self.corner(sq);
            let _ = write!(s, r##"<use xlink:href="#{}" x="{}" y="{}"/>"##, piece_id(piece), Num(x), Num(y));
        }
        if self.coordinates {
            self.write_coordinates(&mut s);
        }
        self.write_shapes(&mut s);
        s.push_str("</svg>");
        Ok(s)
    }

    /// The diagram as a PNG file.
    pub fn to_png(&self) -> Result<Vec<u8>, DiagramError> {
        self.pixmap()?.encode_png().map_err(|e| DiagramError::Render(e.to_string()))
    }

    /// The diagram as RGBA pixels (for the clipboard).
    pub fn to_rgba(&self) -> Result<RgbaImage, DiagramError> {
        let pixmap = self.pixmap()?;
        let data = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        Ok(RgbaImage { width: pixmap.width(), height: pixmap.height(), data })
    }

    fn pixmap(&self) -> Result<tiny_skia::Pixmap, DiagramError> {
        let svg = self.to_svg()?;
        let tree = usvg::Tree::from_str(&svg, &usvg::Options::default())
            .map_err(|e| DiagramError::Render(e.to_string()))?;
        let mut pixmap = tiny_skia::Pixmap::new(self.size, self.size)
            .ok_or_else(|| DiagramError::Render(format!("no memory for a {0}×{0} image", self.size)))?;
        resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
        Ok(pixmap)
    }

    /// Top-left corner of `sq` in user units.
    fn corner(&self, sq: Square) -> (f64, f64) {
        let file = f64::from(sq.file().to_u32());
        let rank = f64::from(sq.rank().to_u32());
        match self.orientation {
            Color::White => (file * SQ, (7.0 - rank) * SQ),
            Color::Black => ((7.0 - file) * SQ, rank * SQ),
        }
    }

    fn center(&self, sq: Square) -> (f64, f64) {
        let (x, y) = self.corner(sq);
        (x + SQ / 2.0, y + SQ / 2.0)
    }

    /// Pieces on the board, each drawn once and placed with `<use>`; the
    /// check glow.
    fn write_defs(&self, s: &mut String) {
        s.push_str("<defs>");
        let mut done = Vec::new();
        for (_, piece) in self.board.iter() {
            if done.contains(&piece) {
                continue;
            }
            done.push(piece);
            // The cburnett pieces are drawn on a 45 × 45 canvas.
            let _ = write!(
                s,
                r#"<g id="{}" transform="scale({:.6})">{}</g>"#,
                piece_id(piece),
                SQ / 45.0,
                svg_body(piece_svg(piece))
            );
        }
        if self.check.is_some() {
            // CSS radial-gradient(ellipse at center, …) reaches the corners:
            // its radius is half the diagonal.
            let _ = write!(
                s,
                r#"<radialGradient id="check" r="0.7071"><stop offset="0" stop-color="{CHECK}" stop-opacity="0.9"/><stop offset="0.3" stop-color="{CHECK}" stop-opacity="0.5"/><stop offset="0.7" stop-color="{CHECK}" stop-opacity="0"/></radialGradient>"#
            );
        }
        s.push_str("</defs>");
    }

    /// Rank labels at the top left of the left column's squares, file labels
    /// at the bottom right of the bottom row's, each in the colour of the
    /// other kind of square (board-theme.css).
    fn write_coordinates(&self, s: &mut String) {
        let font = glyphs();
        // 18% of a square, as on a mid-sized board; at least 9 px.
        let px_per_unit = f64::from(self.size) / (8.0 * SQ);
        let font_size = (0.18 * SQ).max(9.0 / px_per_unit);
        let scale = font_size / font.units_per_em;
        let pad = 0.04 * SQ;
        let (left, bottom) = match self.orientation {
            Color::White => (File::A, Rank::First),
            Color::Black => (File::H, Rank::Eighth),
        };
        let mut label = |sq: Square, c: char, right_aligned: bool| {
            let Some(glyph) = font.get(c) else { return };
            let (x, y) = self.corner(sq);
            let (gx, baseline) = if right_aligned {
                (x + SQ - pad - glyph.advance * scale, y + SQ - pad - font.descent * scale)
            } else {
                (x + pad, y + pad + font.cap_height * scale)
            };
            let color = if is_dark(sq) { self.light } else { self.dark.mix(Rgb(0, 0, 0), 0.2) };
            let _ = write!(
                s,
                r#"<path fill="{color}" transform="translate({} {}) scale({scale:.6} -{scale:.6})" d="{}"/>"#,
                Num(gx),
                Num(baseline),
                glyph.path
            );
        };
        for rank in Rank::ALL {
            label(Square::from_coords(left, rank), rank.char(), false);
        }
        for file in File::ALL {
            label(Square::from_coords(file, bottom), file.char(), true);
        }
    }

    /// Circles and arrows, drawn like chessground draws them (sizes in
    /// 64ths of a square).
    fn write_shapes(&self, s: &mut String) {
        if self.shapes.is_empty() {
            return;
        }
        let _ = write!(s, r#"<g opacity="{SHAPES_OPACITY}">"#);
        for shape in &self.shapes {
            let color = brush(shape.color);
            let (x1, y1) = self.center(shape.from);
            match shape.to.filter(|&to| to != shape.from) {
                None => {
                    let width = 4.0 / 64.0 * SQ;
                    let _ = write!(
                        s,
                        r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{color}" stroke-width="{}"/>"#,
                        Num(x1),
                        Num(y1),
                        Num(SQ / 2.0 - width / 2.0),
                        Num(width)
                    );
                }
                Some(to) => {
                    let (x2, y2) = self.center(to);
                    let width = 10.0 / 64.0 * SQ;
                    let margin = if self.is_short(to) { 20.0 } else { 10.0 } / 64.0 * SQ;
                    let len = (x2 - x1).hypot(y2 - y1);
                    let (ux, uy) = ((x2 - x1) / len, (y2 - y1) / len);
                    let (ex, ey) = (x2 - ux * margin, y2 - uy * margin);
                    let _ = write!(
                        s,
                        r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{color}" stroke-width="{}" stroke-linecap="round"/>"#,
                        Num(x1),
                        Num(y1),
                        Num(ex),
                        Num(ey),
                        Num(width)
                    );
                    // chessground's marker: a triangle 4 line widths across
                    // and 3 long, its point 0.95 widths past the line's end.
                    let (bx, by) = (ex - ux * 2.05 * width, ey - uy * 2.05 * width);
                    let (px, py) = (-uy * 2.0 * width, ux * 2.0 * width);
                    let _ = write!(
                        s,
                        r#"<path fill="{color}" d="M{} {}L{} {}L{} {}Z"/>"#,
                        Num(bx + px),
                        Num(by + py),
                        Num(ex + ux * 0.95 * width),
                        Num(ey + uy * 0.95 * width),
                        Num(bx - px),
                        Num(by - py)
                    );
                }
            }
        }
        s.push_str("</g>");
    }

    /// As in chessground: arrows into `dest` end further from its centre
    /// when two of them come from directions less than 90° apart (in
    /// sixteenths of a turn).
    fn is_short(&self, dest: Square) -> bool {
        let mut slots = Vec::new();
        for shape in &self.shapes {
            if shape.to != Some(dest) || shape.from == dest {
                continue;
            }
            let ((x1, y1), (x2, y2)) = (self.center(shape.from), self.center(dest));
            let angle = (y2 - y1).atan2(x2 - x1) + std::f64::consts::PI;
            let slot = ((angle * 8.0 / std::f64::consts::PI).round() as i32).rem_euclid(16);
            if !slots.contains(&slot) {
                slots.push(slot);
            }
        }
        slots.iter().any(|a| slots.iter().any(|b| (1..=3).contains(&(a - b).rem_euclid(16))))
    }
}

/// a1 is dark.
fn is_dark(sq: Square) -> bool {
    (sq.file().to_u32() + sq.rank().to_u32()).is_multiple_of(2)
}

fn piece_id(piece: Piece) -> String {
    format!("{}{}", piece.color.fold_wb('w', 'b'), piece.role.upper_char())
}

fn piece_svg(piece: Piece) -> &'static str {
    macro_rules! pieces {
        ($($color:ident $role:ident $file:literal),* $(,)?) => {
            match (piece.color, piece.role) {
                $((Color::$color, Role::$role) => include_str!(concat!("../data/pieces/cburnett/", $file)),)*
            }
        };
    }
    pieces! {
        White Pawn "wP.svg", White Knight "wN.svg", White Bishop "wB.svg",
        White Rook "wR.svg", White Queen "wQ.svg", White King "wK.svg",
        Black Pawn "bP.svg", Black Knight "bN.svg", Black Bishop "bB.svg",
        Black Rook "bR.svg", Black Queen "bQ.svg", Black King "bK.svg",
    }
}

/// The content of a standalone `<svg>` document, without the element itself.
fn svg_body(svg: &str) -> &str {
    let start = svg.find("<svg").and_then(|i| svg[i..].find('>').map(|j| i + j + 1)).unwrap_or(0);
    let end = svg.rfind("</svg>").unwrap_or(svg.len());
    &svg[start..end.max(start)]
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A number for SVG: at most two decimals, no trailing zeros.
pub(crate) struct Num(pub f64);

impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = (self.0 * 100.0).round() / 100.0;
        if v == v.trunc() { write!(f, "{}", v as i64) } else { write!(f, "{v}") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_core::shakmaty::{Chess, Position};

    fn start() -> Board {
        Chess::default().board().clone()
    }

    fn pixel(img: &RgbaImage, x: u32, y: u32) -> Rgb {
        let i = ((y * img.width + x) * 4) as usize;
        assert_eq!(img.data[i + 3], 255, "opaque at {x},{y}");
        Rgb(img.data[i], img.data[i + 1], img.data[i + 2])
    }

    /// Pixel at the centre of `sq` (squares are `size / 8` wide).
    fn at(d: &Diagram, img: &RgbaImage, sq: Square) -> Rgb {
        let (x, y) = d.center(sq);
        let k = f64::from(d.size) / (8.0 * SQ);
        pixel(img, (x * k) as u32, (y * k) as u32)
    }

    #[test]
    fn parses_css_colours() {
        assert_eq!("#e6e2c8".parse::<Rgb>().unwrap(), Rgb(0xe6, 0xe2, 0xc8));
        assert_eq!(" #E6E2C8\n".parse::<Rgb>().unwrap(), Rgb(0xe6, 0xe2, 0xc8));
        assert_eq!("#fa0".parse::<Rgb>().unwrap(), Rgb(0xff, 0xaa, 0x00));
        for bad in ["", "e6e2c8", "#e6e2c", "#ggg", "red", "#+1+2+3", "#ééé"] {
            assert!(bad.parse::<Rgb>().is_err(), "{bad:?}");
        }
        assert_eq!(Rgb(1, 0xab, 255).to_string(), "#01abff");
    }

    #[test]
    fn every_piece_is_a_bare_drawing() {
        for color in [Color::White, Color::Black] {
            for role in Role::ALL {
                let body = svg_body(piece_svg(Piece { color, role }));
                assert!(body.starts_with('<') && !body.contains("<svg") && !body.contains("</svg>"));
            }
        }
    }

    #[test]
    fn svg_has_every_piece_once_and_parses() {
        let mut d = Diagram::new(start());
        d.title = Some("a & <b>".into());
        let svg = d.to_svg().unwrap();
        assert_eq!(svg.matches("<use ").count(), 32);
        assert_eq!(svg.matches("<g id=").count(), 12);
        assert!(svg.contains(r#"width="480" height="480""#));
        assert!(svg.contains("<title>a &amp; &lt;b&gt;</title>"));
        // 16 coordinate labels.
        assert_eq!(svg.matches(r#"transform="translate("#).count(), 16);
        usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap();

        d.coordinates = false;
        assert_eq!(d.to_svg().unwrap().matches(r#"transform="translate("#).count(), 0);
    }

    #[test]
    fn size_is_checked() {
        let mut d = Diagram::new(start());
        for size in [0, MIN_SIZE - 1, MAX_SIZE + 1] {
            d.size = size;
            assert!(matches!(d.to_svg(), Err(DiagramError::BadSize(s)) if s == size));
        }
    }

    #[test]
    fn png_is_the_requested_size() {
        let mut d = Diagram::new(start());
        d.size = 200;
        let png = d.to_png().unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // IHDR: width and height, big-endian.
        assert_eq!(&png[16..24], &[0, 0, 0, 200, 0, 0, 0, 200]);
    }

    #[test]
    fn squares_follow_colours_and_orientation() {
        let mut board = Board::empty();
        board.set_piece_at(Square::A1, Piece { color: Color::White, role: Role::King });
        let mut d = Diagram::new(board);
        d.size = 160;
        d.coordinates = false;
        d.light = Rgb(250, 250, 250);
        d.dark = Rgb(10, 100, 10);
        let img = d.to_rgba().unwrap();
        assert_eq!((img.width, img.height, img.data.len()), (160, 160, 160 * 160 * 4));
        assert_eq!(at(&d, &img, Square::D4), d.dark);
        assert_eq!(at(&d, &img, Square::E4), d.light);
        // The king is at the bottom left, the top right is empty.
        assert_ne!(pixel(&img, 10, 150), d.dark);
        assert_eq!(pixel(&img, 150, 10), d.dark);

        d.orientation = Color::Black;
        let img = d.to_rgba().unwrap();
        assert_eq!(pixel(&img, 10, 150), d.dark);
        assert_ne!(pixel(&img, 150, 10), d.dark);
        assert_eq!(at(&d, &img, Square::D4), d.dark);
    }

    #[test]
    fn highlights_coordinates_and_shapes_are_drawn() {
        let mut d = Diagram::new(Board::empty());
        d.size = 160;
        d.coordinates = false;
        let plain = d.to_rgba().unwrap();

        d.last_move = Some((Square::E2, Square::E4));
        d.shapes = vec![
            Shape { color: ShapeColor::Green, from: Square::D2, to: Some(Square::D5) },
            Shape { color: ShapeColor::Red, from: Square::H8, to: None },
        ];
        d.check = Some(Square::A8);
        d.coordinates = true;
        let img = d.to_rgba().unwrap();
        for sq in [Square::E2, Square::E4, Square::D3, Square::D4, Square::A8] {
            assert_ne!(at(&d, &img, sq), at(&d, &plain, sq), "{sq}");
        }
        // Untouched: a square away from all of it, and the middle of the circle.
        assert_eq!(at(&d, &img, Square::B5), at(&d, &plain, Square::B5));
        assert_eq!(at(&d, &img, Square::H8), at(&d, &plain, Square::H8));
        // The green arrow tints d3 green; the rank label "1" darkens a1's corner.
        let Rgb(r, g, b) = at(&d, &img, Square::D3);
        assert!(g > r && g > b, "{r} {g} {b}");
        let label = (0..20).flat_map(|x| (140..160).map(move |y| (x, y)));
        assert!(label.clone().any(|(x, y)| pixel(&img, x, y) != pixel(&plain, x, y)));
    }

    #[test]
    fn close_arrows_into_one_square_end_shorter() {
        let mut d = Diagram::new(Board::empty());
        let arrow = |from| Shape { color: ShapeColor::Blue, from, to: Some(Square::E4) };
        d.shapes = vec![arrow(Square::E1)];
        assert!(!d.is_short(Square::E4));
        d.shapes.push(arrow(Square::E8));
        assert!(!d.is_short(Square::E4), "opposite directions");
        d.shapes.push(arrow(Square::F1));
        assert!(d.is_short(Square::E4), "e1 and f1 are close");
    }
}
