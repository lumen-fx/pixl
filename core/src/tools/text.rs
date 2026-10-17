//! Text set in bitmap fonts: every glyph is a set of whole pixels, so text
//! never has anti-aliasing. pixl ships one small font of its own and reads
//! BDF fonts, the plain-text bitmap font format most pixel fonts are
//! published in.

use std::collections::HashMap;

use crate::error::Error;
use crate::geom::Point;

/// One glyph: its pixels and where they sit against the pen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub width: u32,
    pub height: u32,
    /// From the pen to the glyph's left edge.
    pub x_offset: i32,
    /// From the baseline to the glyph's top row (negative is above).
    pub y_offset: i32,
    /// How far the pen moves after the glyph.
    pub advance: i32,
    /// Row-major, `width x height`.
    pub bits: Vec<bool>,
}

/// A bitmap font.
#[derive(Clone, Debug, PartialEq)]
pub struct BitmapFont {
    pub name: String,
    /// Rows from the top of a line to the baseline.
    pub ascent: i32,
    /// Rows from one line's top to the next.
    pub line_height: i32,
    glyphs: HashMap<char, Glyph>,
}

/// pixl's own font: 5 pixels wide at most, capitals 7 tall, one row of
/// descender. `#` is a pixel. Each glyph is its character on a line of its
/// own, then eight rows.
const BUILTIN: &str = include_str!("pixl5x7.txt");

impl BitmapFont {
    /// pixl's own 5x7 font, ASCII 32 to 126.
    pub fn builtin() -> Self {
        let mut glyphs = HashMap::new();
        let mut lines = BUILTIN.lines();
        while let Some(head) = lines.next() {
            let Some(c) = head.chars().next() else {
                continue;
            };
            let rows: Vec<&str> = lines.by_ref().take(8).collect();
            let width = rows
                .iter()
                .filter_map(|r| r.rfind('#'))
                .max()
                .map_or(0, |i| i + 1) as u32;
            let mut bits = Vec::with_capacity(width as usize * 8);
            for r in &rows {
                for x in 0..width as usize {
                    bits.push(r.as_bytes().get(x) == Some(&b'#'));
                }
            }
            let advance = if c == ' ' { 3 } else { width as i32 + 1 };
            glyphs.insert(
                c,
                Glyph {
                    width,
                    height: 8,
                    x_offset: 0,
                    y_offset: -7,
                    advance,
                    bits,
                },
            );
        }
        Self {
            name: "pixl 5x7".to_string(),
            ascent: 7,
            line_height: 9,
            glyphs,
        }
    }

    /// Reads a BDF font (Adobe's Glyph Bitmap Distribution Format, 2.1).
    pub fn parse_bdf(text: &str) -> Result<Self, Error> {
        let bad = |m: &str| Error::Format(format!("BDF: {m}"));
        let mut name = String::from("BDF font");
        let (mut ascent, mut descent) = (None::<i32>, None::<i32>);
        let mut bbox = (0i32, 0i32, 0i32, 0i32);
        let mut glyphs = HashMap::new();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            let mut f = line.split_whitespace();
            match f.next() {
                Some("FONT") => name = f.collect::<Vec<_>>().join(" "),
                Some("FONT_ASCENT") => ascent = f.next().and_then(|v| v.parse().ok()),
                Some("FONT_DESCENT") => descent = f.next().and_then(|v| v.parse().ok()),
                Some("FONTBOUNDINGBOX") => {
                    let v: Vec<i32> = f.filter_map(|v| v.parse().ok()).collect();
                    if v.len() == 4 {
                        bbox = (v[0], v[1], v[2], v[3]);
                    }
                }
                Some("STARTCHAR") => {
                    let (mut code, mut advance, mut bbx) = (None::<i64>, None::<i32>, bbox);
                    let mut bits = Vec::new();
                    while let Some(line) = lines.next() {
                        let mut f = line.split_whitespace();
                        match f.next() {
                            Some("ENCODING") => code = f.next().and_then(|v| v.parse().ok()),
                            Some("DWIDTH") => advance = f.next().and_then(|v| v.parse().ok()),
                            Some("BBX") => {
                                let v: Vec<i32> = f.filter_map(|v| v.parse().ok()).collect();
                                if v.len() == 4 {
                                    bbx = (v[0], v[1], v[2], v[3]);
                                }
                            }
                            Some("BITMAP") => {
                                let (w, h) = (bbx.0.max(0) as usize, bbx.1.max(0) as usize);
                                for row in lines.by_ref().take(h) {
                                    let hex = row.trim();
                                    let mut bytes = Vec::new();
                                    for i in (0..hex.len()).step_by(2) {
                                        let b = hex
                                            .get(i..i + 2)
                                            .and_then(|s| u8::from_str_radix(s, 16).ok())
                                            .ok_or_else(|| bad("a bitmap row is not hex"))?;
                                        bytes.push(b);
                                    }
                                    for x in 0..w {
                                        let byte = bytes.get(x / 8).copied().unwrap_or(0);
                                        bits.push(byte & (0x80 >> (x % 8)) != 0);
                                    }
                                }
                            }
                            Some("ENDCHAR") => break,
                            _ => {}
                        }
                    }
                    let Some(c) = code
                        .and_then(|c| u32::try_from(c).ok())
                        .and_then(char::from_u32)
                    else {
                        continue;
                    };
                    let (w, h) = (bbx.0.max(0) as u32, bbx.1.max(0) as u32);
                    bits.resize(w as usize * h as usize, false);
                    glyphs.insert(
                        c,
                        Glyph {
                            width: w,
                            height: h,
                            x_offset: bbx.2,
                            y_offset: -(bbx.1 + bbx.3),
                            advance: advance.unwrap_or(bbx.0 + 1),
                            bits,
                        },
                    );
                }
                _ => {}
            }
        }
        if glyphs.is_empty() {
            return Err(bad("no glyphs"));
        }
        let ascent = ascent.unwrap_or(bbox.1 + bbox.3);
        let descent = descent.unwrap_or(-bbox.3);
        Ok(Self {
            name,
            ascent,
            line_height: (ascent + descent).max(1),
            glyphs,
        })
    }

    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        self.glyphs.get(&c).or_else(|| self.glyphs.get(&'?'))
    }

    /// The pixels of `text` with its first line's top-left at `at`, each
    /// font pixel drawn as a `scale x scale` block, `spacing` more pixels
    /// between letters and `line_gap` more between lines. Newlines start new
    /// lines.
    pub fn layout(
        &self,
        text: &str,
        at: Point,
        scale: u32,
        spacing: i32,
        line_gap: i32,
    ) -> Vec<Point> {
        let s = scale.max(1) as i32;
        let mut out = Vec::new();
        for (row, line) in text.split('\n').enumerate() {
            let baseline = at.y + (row as i32 * (self.line_height + line_gap) + self.ascent) * s;
            let mut pen = at.x;
            for c in line.chars() {
                let Some(g) = self.glyph(c) else {
                    continue;
                };
                for gy in 0..g.height as i32 {
                    for gx in 0..g.width as i32 {
                        if !g.bits[(gy * g.width as i32 + gx) as usize] {
                            continue;
                        }
                        let x = pen + (g.x_offset + gx) * s;
                        let y = baseline + (g.y_offset + gy) * s;
                        for by in 0..s {
                            for bx in 0..s {
                                out.push(Point::new(x + bx, y + by));
                            }
                        }
                    }
                }
                pen += (g.advance + spacing) * s;
            }
        }
        out
    }

    /// The width and height `text` takes at `scale`.
    pub fn measure(&self, text: &str, scale: u32, spacing: i32, line_gap: i32) -> (i32, i32) {
        let s = scale.max(1) as i32;
        let lines: Vec<&str> = text.split('\n').collect();
        let w = lines
            .iter()
            .map(|l| {
                l.chars()
                    .filter_map(|c| self.glyph(c))
                    .map(|g| g.advance + spacing)
                    .sum::<i32>()
            })
            .max()
            .unwrap_or(0);
        let h = lines.len() as i32 * (self.line_height + line_gap) - line_gap;
        (w * s, h * s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_font_covers_ascii() {
        let f = BitmapFont::builtin();
        for c in ' '..='~' {
            let g = f.glyphs.get(&c).unwrap_or_else(|| panic!("no glyph {c:?}"));
            assert!(g.width <= 5, "{c:?}");
            if c != ' ' {
                assert!(g.bits.iter().any(|b| *b), "{c:?} is empty");
            }
        }
        assert_eq!(f.glyphs.len(), 95);
    }

    #[test]
    fn layout_places_whole_pixels_and_scales() {
        let f = BitmapFont::builtin();
        let one = f.layout("I", Point::new(10, 10), 1, 0, 0);
        assert!(one.contains(&Point::new(10, 10)), "the I's top bar");
        assert!(one.contains(&Point::new(11, 16)));
        assert!(one.iter().all(|p| p.y >= 10 && p.y < 17));
        let big = f.layout("I", Point::new(0, 0), 2, 0, 0);
        assert_eq!(big.len(), one.len() * 4);
        let (w, h) = f.measure("ab\nc", 1, 0, 0);
        assert_eq!((w, h), (12, 18));
        let two = f.layout("a\na", Point::new(0, 0), 1, 0, 0);
        assert!(two.iter().any(|p| p.y >= 9));
    }

    #[test]
    fn bdf_fonts_read() {
        let bdf = "STARTFONT 2.1\nFONT tiny\nFONTBOUNDINGBOX 3 3 0 0\nSTARTPROPERTIES 2\nFONT_ASCENT 3\nFONT_DESCENT 0\nENDPROPERTIES\nCHARS 1\nSTARTCHAR A\nENCODING 65\nDWIDTH 4 0\nBBX 3 3 0 0\nBITMAP\n40\nE0\nA0\nENDCHAR\nENDFONT\n";
        let f = BitmapFont::parse_bdf(bdf).unwrap();
        assert_eq!(f.name, "tiny");
        let px = f.layout("A", Point::new(0, 0), 1, 0, 0);
        let mut want = vec![
            Point::new(1, 0),
            Point::new(0, 1),
            Point::new(1, 1),
            Point::new(2, 1),
            Point::new(0, 2),
            Point::new(2, 2),
        ];
        let mut got = px.clone();
        got.sort_by_key(|p| (p.y, p.x));
        want.sort_by_key(|p| (p.y, p.x));
        assert_eq!(got, want);
        assert_eq!(f.measure("AA", 1, 0, 0).0, 8);
        assert!(BitmapFont::parse_bdf("nothing").is_err());
    }
}
