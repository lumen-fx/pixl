//! Colors, color modes and palettes.

use crate::error::Error;

/// An 8-bit RGBA color with straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba::new(0, 0, 0, 0);
    pub const BLACK: Rgba = Rgba::opaque(0, 0, 0);
    pub const WHITE: Rgba = Rgba::opaque(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    /// Packs the color as `0xAABBGGRR`, the byte order of RGBA8 in memory.
    pub const fn to_u32(self) -> u32 {
        u32::from_le_bytes([self.r, self.g, self.b, self.a])
    }

    pub const fn from_u32(v: u32) -> Self {
        let [r, g, b, a] = v.to_le_bytes();
        Self { r, g, b, a }
    }

    /// The largest per-channel difference, the measure fill tolerance uses.
    /// Two fully transparent colors are equal whatever their RGB.
    pub fn distance(self, other: Rgba) -> u8 {
        if self.a == 0 && other.a == 0 {
            return 0;
        }
        let d = |a: u8, b: u8| a.abs_diff(b);
        d(self.r, other.r)
            .max(d(self.g, other.g))
            .max(d(self.b, other.b))
            .max(d(self.a, other.a))
    }

    /// Rec. 601 luma, used to paint masks with colors.
    pub fn luma(self) -> u8 {
        ((self.r as u32 * 299 + self.g as u32 * 587 + self.b as u32 * 114 + 500) / 1000) as u8
    }

    /// The color as hue, saturation and lightness.
    pub fn to_hsl(self) -> Hsl {
        let (r, g, b) = (
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d <= f32::EPSILON {
            return Hsl { h: 0.0, s: 0.0, l };
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        Hsl { h, s, l }
    }

    /// The color as hue, saturation and value. A gray has hue 0.
    pub fn to_hsv(self) -> Hsv {
        let (r, g, b) = (
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        );
        let max = r.max(g).max(b);
        let d = max - r.min(g).min(b);
        if d <= f32::EPSILON {
            return Hsv {
                h: 0.0,
                s: 0.0,
                v: max,
            };
        }
        let h = if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        Hsv {
            h,
            s: d / max,
            v: max,
        }
    }

    /// The color of `hsv` with alpha `a`. Any hue turns around to 0 to 360.
    pub fn from_hsv(hsv: Hsv, a: u8) -> Rgba {
        let h = hsv.h.rem_euclid(360.0);
        let s = hsv.s.clamp(0.0, 1.0);
        let v = hsv.v.clamp(0.0, 1.0);
        let c = v * s;
        let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
        let m = v - c;
        let (r, g, b) = match (h / 60.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Rgba::new(byte(r), byte(g), byte(b), a)
    }

    /// The color of `hsl` with alpha `a`.
    pub fn from_hsl(hsl: Hsl, a: u8) -> Rgba {
        let h = hsl.h.rem_euclid(360.0);
        let s = hsl.s.clamp(0.0, 1.0);
        let l = hsl.l.clamp(0.0, 1.0);
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match (h / 60.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Rgba::new(byte(r), byte(g), byte(b), a)
    }
}

/// A color as hue (degrees, 0 to 360), saturation and lightness (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

/// A color as hue (degrees, 0 to 360), saturation and value (0 to 1), the
/// model of a color picker's square.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    /// The same color as hue, saturation and lightness. A color with no
    /// saturation keeps its hue, so a picker moved through gray does not
    /// lose it.
    pub fn to_hsl(self) -> Hsl {
        let s = self.s.clamp(0.0, 1.0);
        let v = self.v.clamp(0.0, 1.0);
        let l = v * (1.0 - s / 2.0);
        let sl = if l <= 0.0 || l >= 1.0 {
            0.0
        } else {
            (v - l) / l.min(1.0 - l)
        };
        Hsl {
            h: self.h,
            s: sl,
            l,
        }
    }

    /// The same color as hue, saturation and value, keeping the hue.
    pub fn from_hsl(hsl: Hsl) -> Hsv {
        let s = hsl.s.clamp(0.0, 1.0);
        let l = hsl.l.clamp(0.0, 1.0);
        let v = l + s * l.min(1.0 - l);
        let sv = if v <= 0.0 { 0.0 } else { 2.0 * (1.0 - l / v) };
        Hsv { h: hsl.h, s: sv, v }
    }
}

/// A color as a tool or a fill names it: either a direct color or a palette
/// entry. Each image resolves it to its own pixel format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Rgba(Rgba),
    Index(u8),
}

impl From<Rgba> for Color {
    fn from(c: Rgba) -> Self {
        Color::Rgba(c)
    }
}

/// How a document stores its pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorMode {
    /// Every pixel is an [`Rgba`].
    #[default]
    Rgba,
    /// Every pixel is an index into the document palette.
    Indexed,
}

/// An ordered list of up to 256 colors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    colors: Vec<Rgba>,
}

impl Palette {
    pub const MAX: usize = 256;

    /// A palette from a list of colors; extra colors past 256 are an error.
    pub fn new(colors: Vec<Rgba>) -> Result<Self, Error> {
        if colors.len() > Self::MAX {
            return Err(Error::PaletteFull);
        }
        Ok(Self { colors })
    }

    pub fn len(&self) -> usize {
        self.colors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    pub fn colors(&self) -> &[Rgba] {
        &self.colors
    }

    /// The color at `index`, or transparent past the end.
    pub fn get(&self, index: u8) -> Rgba {
        self.colors
            .get(index as usize)
            .copied()
            .unwrap_or(Rgba::TRANSPARENT)
    }

    pub fn set(&mut self, index: u8, color: Rgba) -> Result<(), Error> {
        let slot = self
            .colors
            .get_mut(index as usize)
            .ok_or(Error::PaletteIndex(index))?;
        *slot = color;
        Ok(())
    }

    /// Appends a color and returns its index.
    pub fn push(&mut self, color: Rgba) -> Result<u8, Error> {
        if self.colors.len() >= Self::MAX {
            return Err(Error::PaletteFull);
        }
        self.colors.push(color);
        Ok((self.colors.len() - 1) as u8)
    }

    /// The entry an imported color lands on: the transparent index `ti`
    /// for a clear color, otherwise the nearest other entry.
    pub fn import_index(&self, color: Rgba, ti: u8) -> u8 {
        if color.a == 0 {
            ti
        } else {
            self.nearest(color, Some(ti))
        }
    }

    /// The index of the entry closest to `color` (squared RGBA distance),
    /// skipping `skip` (the transparent index, when matching an opaque color).
    /// Returns 0 for an empty palette.
    pub fn nearest(&self, color: Rgba, skip: Option<u8>) -> u8 {
        let mut best = (u32::MAX, 0u8);
        for (i, c) in self.colors.iter().enumerate() {
            if Some(i as u8) == skip {
                continue;
            }
            let d = |a: u8, b: u8| (a as i32 - b as i32).unsigned_abs().pow(2);
            let dist = d(c.r, color.r) + d(c.g, color.g) + d(c.b, color.b) + d(c.a, color.a);
            if dist < best.0 {
                best = (dist, i as u8);
                if dist == 0 {
                    break;
                }
            }
        }
        best.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_in_memory_order() {
        let c = Rgba::new(1, 2, 3, 4);
        assert_eq!(c.to_u32(), 0x0403_0201);
        assert_eq!(Rgba::from_u32(c.to_u32()), c);
    }

    #[test]
    fn distance_ignores_rgb_of_transparent() {
        assert_eq!(Rgba::new(9, 9, 9, 0).distance(Rgba::TRANSPARENT), 0);
        assert_eq!(
            Rgba::opaque(10, 20, 30).distance(Rgba::opaque(12, 10, 30)),
            10
        );
    }

    #[test]
    fn nearest_skips_transparent_index() {
        let p = Palette::new(vec![
            Rgba::TRANSPARENT,
            Rgba::BLACK,
            Rgba::opaque(250, 0, 0),
        ])
        .unwrap();
        assert_eq!(p.nearest(Rgba::opaque(200, 10, 0), None), 2);
        assert_eq!(p.nearest(Rgba::new(0, 0, 0, 10), Some(0)), 1);
        assert_eq!(p.nearest(Rgba::new(0, 0, 0, 10), None), 0);
    }

    #[test]
    fn an_imported_color_is_clear_or_off_the_transparent_index() {
        let p = Palette::new(vec![Rgba::TRANSPARENT, Rgba::BLACK]).unwrap();
        assert_eq!(p.import_index(Rgba::new(9, 9, 9, 0), 0), 0);
        assert_eq!(p.import_index(Rgba::new(0, 0, 0, 10), 0), 1);
    }

    #[test]
    fn hsl_round_trips() {
        for c in [
            Rgba::opaque(255, 0, 0),
            Rgba::opaque(12, 200, 99),
            Rgba::opaque(40, 40, 40),
            Rgba::opaque(250, 240, 3),
            Rgba::opaque(8, 17, 230),
        ] {
            assert_eq!(Rgba::from_hsl(c.to_hsl(), 255), c);
        }
        let red = Rgba::opaque(255, 0, 0).to_hsl();
        assert_eq!((red.h, red.s, red.l), (0.0, 1.0, 0.5));
        assert_eq!(Rgba::opaque(0, 0, 255).to_hsl().h, 240.0);
    }

    /// A spread of colors: each channel at both ends, one step in from
    /// them, and between.
    fn samples() -> impl Iterator<Item = Rgba> {
        let steps = [0u8, 1, 17, 64, 127, 128, 200, 254, 255];
        steps.into_iter().flat_map(move |r| {
            steps
                .into_iter()
                .flat_map(move |g| steps.into_iter().map(move |b| Rgba::new(r, g, b, 77)))
        })
    }

    #[test]
    fn hsv_round_trips_every_sample() {
        for c in samples() {
            assert_eq!(Rgba::from_hsv(c.to_hsv(), c.a), c, "{c:?}");
            assert_eq!(Rgba::from_hsl(c.to_hsl(), c.a), c, "{c:?}");
            // Through the other model and back.
            let via = Rgba::from_hsl(c.to_hsv().to_hsl(), c.a);
            assert_eq!(via, c, "{c:?} through HSL");
            let via = Rgba::from_hsv(Hsv::from_hsl(c.to_hsl()), c.a);
            assert_eq!(via, c, "{c:?} through HSV");
        }
    }

    #[test]
    fn edge_hues_land_on_the_primaries() {
        let at = |h: f32| Rgba::from_hsv(Hsv { h, s: 1.0, v: 1.0 }, 255);
        assert_eq!(at(0.0), Rgba::opaque(255, 0, 0));
        assert_eq!(at(360.0), Rgba::opaque(255, 0, 0), "a full turn is red");
        assert_eq!(at(-120.0), Rgba::opaque(0, 0, 255), "turns around");
        assert_eq!(at(720.0 + 120.0), Rgba::opaque(0, 255, 0));
        assert_eq!(at(60.0), Rgba::opaque(255, 255, 0));
        assert_eq!(at(180.0), Rgba::opaque(0, 255, 255));
        assert_eq!(at(300.0), Rgba::opaque(255, 0, 255));
        assert_eq!(at(359.9), Rgba::opaque(255, 0, 0));
        let just_short = Rgba::opaque(255, 0, 1).to_hsv().h;
        assert!(just_short > 359.0 && just_short < 360.0, "{just_short}");
        assert_eq!(Rgba::opaque(0, 0, 255).to_hsv().h, 240.0);
        assert_eq!(Rgba::opaque(255, 0, 255).to_hsv().h, 300.0);
    }

    #[test]
    fn grays_have_no_saturation_and_keep_a_given_hue() {
        for v in [0u8, 1, 128, 255] {
            let gray = Rgba::opaque(v, v, v);
            let hsv = gray.to_hsv();
            assert_eq!((hsv.h, hsv.s), (0.0, 0.0));
            assert!((hsv.v - f32::from(v) / 255.0).abs() < 1e-6);
            let hsl = gray.to_hsl();
            assert_eq!(hsl.s, 0.0);
        }
        // Any hue at no saturation is the same gray, and the conversion
        // between the models keeps the hue it was given.
        for h in [0.0, 90.0, 210.0, 359.0] {
            let hsv = Hsv { h, s: 0.0, v: 0.5 };
            assert_eq!(Rgba::from_hsv(hsv, 255), Rgba::opaque(128, 128, 128));
            assert_eq!(hsv.to_hsl().h, h);
            assert_eq!(Hsv::from_hsl(hsv.to_hsl()).h, h);
        }
        // Black and white have no saturation in either model.
        let black = Hsv {
            h: 40.0,
            s: 1.0,
            v: 0.0,
        };
        assert_eq!(black.to_hsl().s, 0.0);
        let white = Hsl {
            h: 40.0,
            s: 1.0,
            l: 1.0,
        };
        assert_eq!(Hsv::from_hsl(white).s, 0.0);
        assert_eq!(Hsv::from_hsl(white).v, 1.0);
    }

    #[test]
    fn palette_caps_at_256() {
        let mut p = Palette::new(vec![Rgba::BLACK; 256]).unwrap();
        assert_eq!(p.push(Rgba::WHITE), Err(Error::PaletteFull));
        assert!(Palette::new(vec![Rgba::BLACK; 257]).is_err());
    }
}
