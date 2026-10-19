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

    #[test]
    fn palette_caps_at_256() {
        let mut p = Palette::new(vec![Rgba::BLACK; 256]).unwrap();
        assert_eq!(p.push(Rgba::WHITE), Err(Error::PaletteFull));
        assert!(Palette::new(vec![Rgba::BLACK; 257]).is_err());
    }
}
