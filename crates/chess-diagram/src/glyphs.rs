//! Outlines of the coordinate labels (`a`–`h`, `1`–`8`) from the embedded
//! IBM Plex Sans Condensed SemiBold, the font of the app's board coordinates.
//! Drawing labels as paths keeps the SVG free of fonts.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::Num;

const FONT: &[u8] = include_bytes!("../data/IBMPlexSansCondensed-SemiBold-latin.ttf");
const LABELS: &str = "abcdefgh12345678";

pub(crate) struct Glyph {
    /// SVG path data in font units, y up.
    pub path: String,
    pub advance: f64,
}

pub(crate) struct Glyphs {
    pub units_per_em: f64,
    /// Height of the digits (lining figures, as tall as capitals).
    pub cap_height: f64,
    /// Depth of the deepest file letter below the baseline (`g`).
    pub descent: f64,
    glyphs: HashMap<char, Glyph>,
}

impl Glyphs {
    fn load() -> Option<Glyphs> {
        let face = ttf_parser::Face::parse(FONT, 0).ok()?;
        let units_per_em = f64::from(face.units_per_em());
        let cap_height = face.capital_height().map_or(0.7 * units_per_em, f64::from);
        let mut descent = 0.0f64;
        let mut glyphs = HashMap::new();
        for c in LABELS.chars() {
            let id = face.glyph_index(c)?;
            let mut path = PathData(String::new());
            let bbox = face.outline_glyph(id, &mut path)?;
            if c.is_ascii_lowercase() {
                descent = descent.max(-f64::from(bbox.y_min));
            }
            glyphs.insert(c, Glyph { path: path.0, advance: f64::from(face.glyph_hor_advance(id)?) });
        }
        Some(Glyphs { units_per_em, cap_height, descent, glyphs })
    }

    pub fn get(&self, c: char) -> Option<&Glyph> {
        self.glyphs.get(&c)
    }
}

pub(crate) fn glyphs() -> &'static Glyphs {
    static GLYPHS: OnceLock<Glyphs> = OnceLock::new();
    GLYPHS.get_or_init(|| Glyphs::load().expect("the embedded font has every coordinate label"))
}

struct PathData(String);

impl ttf_parser::OutlineBuilder for PathData {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "M{} {}", Num(x.into()), Num(y.into()));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "L{} {}", Num(x.into()), Num(y.into()));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.0, "Q{} {} {} {}", Num(x1.into()), Num(y1.into()), Num(x.into()), Num(y.into()));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(
            self.0,
            "C{} {} {} {} {} {}",
            Num(x1.into()),
            Num(y1.into()),
            Num(x2.into()),
            Num(y2.into()),
            Num(x.into()),
            Num(y.into())
        );
    }

    fn close(&mut self) {
        self.0.push('Z');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_has_an_outline() {
        let g = glyphs();
        assert_eq!(g.units_per_em, 1000.0);
        assert!(g.cap_height > 600.0 && g.cap_height < 750.0, "{}", g.cap_height);
        assert!(g.descent > 100.0 && g.descent < 300.0, "{}", g.descent);
        for c in LABELS.chars() {
            let glyph = g.get(c).expect("label glyph");
            assert!(glyph.path.starts_with('M') && glyph.path.ends_with('Z'), "{c}");
            assert!(glyph.advance > 0.0, "{c}");
        }
    }
}
