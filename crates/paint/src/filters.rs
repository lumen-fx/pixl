//! Adjustments and filters over one image.
//!
//! A filter reads a source image and writes a new value for every selected
//! pixel; each output pixel depends only on the source, never on other
//! output pixels, so any rectangle can be rendered on its own. That is what
//! lets the editor cut a large image into tiles, render them on worker
//! threads, show each as it finishes and stop at any tile.
//!
//! Indexed images stay indexed: a color filter works on each pixel's palette
//! color and writes the nearest palette entry, and the shape filters
//! (outline, shadow, pixelate) only ever write colors the image already has
//! or the one color they are given.

use crate::tools::fill::Connectivity;
use pixl_document::color::{Palette, Rgba};
use pixl_document::geom::{Point, Rect};
use pixl_document::image::Raster;
use pixl_document::selection::Selection;

/// An adjustment or a filter, with its settings.
#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    /// Turns the hue by `hue` degrees; `saturation` and `lightness` run from
    /// -100 (none, black) through 0 (unchanged) to 100 (double, white).
    HueSaturation {
        hue: f32,
        saturation: f32,
        lightness: f32,
    },
    /// Both from -100 to 100; 0 is unchanged.
    BrightnessContrast {
        brightness: f32,
        contrast: f32,
    },
    /// Maps `input` (black and white points) to `output`, with `gamma` for
    /// the midtones (1 is linear, above 1 brightens).
    Levels {
        input: (u8, u8),
        gamma: f32,
        output: (u8, u8),
    },
    /// A tone curve through `points` (input, output), applied to red, green
    /// and blue. The curve is a monotone cubic through the points, with
    /// (0, 0) and (255, 255) added when no point sits at an end.
    Curves {
        points: Vec<(u8, u8)>,
    },
    /// Shifts toward red, green and blue (negative: cyan, magenta, yellow)
    /// in the shadows, midtones and highlights, each -100 to 100.
    ColorBalance {
        shadows: [f32; 3],
        midtones: [f32; 3],
        highlights: [f32; 3],
    },
    /// Keeps `levels` values per channel (2 to 255).
    Posterize {
        levels: u8,
    },
    Invert,
    /// Gray by Rec. 601 luma.
    Desaturate,
    /// Maps each pixel's luma onto a gradient through `stops`, darkest
    /// first, evenly spaced; alpha is kept.
    GradientMap {
        stops: Vec<Rgba>,
    },
    /// Draws `color` along the edge of the opaque pixels: on the transparent
    /// pixels around them (`outside`) or on their own edge pixels.
    Outline {
        color: Rgba,
        outside: bool,
        connectivity: Connectivity,
    },
    /// Fills transparent pixels under the opaque ones moved by `offset`
    /// with `color`: a hard shadow behind the art.
    DropShadow {
        offset: Point,
        color: Rgba,
    },
    /// Adds noise of up to `amount` to each channel of opaque pixels,
    /// the same on all three channels when `monochrome`. `seed` picks the
    /// pattern; the same seed gives the same noise.
    Noise {
        amount: u8,
        monochrome: bool,
        seed: u32,
    },
    /// Blocks of `size` pixels, anchored to the canvas corner, each taking
    /// its most common color (transparent when most of it is).
    Pixelate {
        size: u32,
    },
}

/// What a filter reads: the image, the palette its indexed pixels name, and
/// the selection it is limited to.
#[derive(Clone, Debug)]
pub struct FilterInput {
    pub raster: Raster,
    pub palette: Palette,
    pub transparent_index: u8,
    pub selection: Option<Selection>,
}

impl FilterInput {
    fn decode(&self, raw: u32) -> Rgba {
        match &self.raster {
            Raster::Rgba(_) => Rgba::from_u32(raw),
            Raster::Indexed(_) => {
                if raw == self.transparent_index as u32 {
                    Rgba::TRANSPARENT
                } else {
                    self.palette.get(raw as u8)
                }
            }
            Raster::Mask(_) => {
                let v = raw as u8;
                Rgba::new(v, v, v, 255)
            }
        }
    }

    fn encode(&self, c: Rgba) -> u32 {
        match &self.raster {
            Raster::Rgba(_) => c.to_u32(),
            Raster::Indexed(_) => {
                if c.a == 0 {
                    self.transparent_index as u32
                } else {
                    let ti = self.transparent_index;
                    self.palette
                        .nearest(Rgba::new(c.r, c.g, c.b, 255), Some(ti)) as u32
                }
            }
            Raster::Mask(_) => c.luma() as u32,
        }
    }

    fn opaque(&self, x: i32, y: i32) -> bool {
        if !self.raster.bounds().contains(x, y) {
            return false;
        }
        let raw = self.raster.get_raw(x, y);
        match &self.raster {
            Raster::Rgba(_) => Rgba::from_u32(raw).a > 0,
            Raster::Indexed(_) => raw != self.transparent_index as u32,
            Raster::Mask(_) => raw > 0,
        }
    }

    fn selected(&self, x: i32, y: i32) -> bool {
        self.selection.as_ref().is_none_or(|s| s.contains(x, y))
    }
}

/// A filter ready to run: its lookup tables built once.
#[derive(Clone, Debug)]
pub struct Prepared {
    filter: Filter,
    lut: Option<[u8; 256]>,
}

impl Filter {
    /// Builds what the filter needs before it renders any pixel.
    pub fn prepare(&self) -> Prepared {
        let lut = match self {
            Filter::BrightnessContrast {
                brightness,
                contrast,
            } => {
                let c = ((100.0 + contrast.clamp(-100.0, 100.0)) / 100.0).powi(2);
                let b = brightness.clamp(-100.0, 100.0) * 1.28;
                Some(table(|v| (v - 127.5) * c + 127.5 + b))
            }
            Filter::Levels {
                input,
                gamma,
                output,
            } => {
                let (lo, hi) = (input.0 as f32, (input.1 as f32).max(input.0 as f32 + 1.0));
                let g = gamma.max(0.01);
                let (olo, ohi) = (output.0 as f32, output.1 as f32);
                Some(table(|v| {
                    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0).powf(1.0 / g);
                    olo + t * (ohi - olo)
                }))
            }
            Filter::Curves { points } => Some(curve_table(points)),
            Filter::Posterize { levels } => {
                let n = (*levels).max(2) as f32 - 1.0;
                Some(table(|v| (v / 255.0 * n).round() * 255.0 / n))
            }
            Filter::Invert => Some(table(|v| 255.0 - v)),
            _ => None,
        };
        Prepared {
            filter: self.clone(),
            lut,
        }
    }

    /// Whether the filter looks at neighbouring pixels (and so needs the
    /// source around a tile, not just inside it).
    pub fn reads_neighbours(&self) -> bool {
        matches!(
            self,
            Filter::Outline { .. } | Filter::DropShadow { .. } | Filter::Pixelate { .. }
        )
    }
}

fn table(f: impl Fn(f32) -> f32) -> [u8; 256] {
    let mut t = [0u8; 256];
    for (i, v) in t.iter_mut().enumerate() {
        *v = f(i as f32).round().clamp(0.0, 255.0) as u8;
    }
    t
}

/// A monotone cubic (Fritsch-Carlson) through the points, as a table.
fn curve_table(points: &[(u8, u8)]) -> [u8; 256] {
    let mut p: Vec<(f32, f32)> = points.iter().map(|&(x, y)| (x as f32, y as f32)).collect();
    p.sort_by(|a, b| a.0.total_cmp(&b.0));
    p.dedup_by(|a, b| a.0 == b.0);
    if p.first().is_none_or(|f| f.0 > 0.0) {
        p.insert(0, (0.0, 0.0));
    }
    if p.last().is_none_or(|l| l.0 < 255.0) {
        p.push((255.0, 255.0));
    }
    let n = p.len();
    let d: Vec<f32> = p
        .windows(2)
        .map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0))
        .collect();
    let mut m = vec![0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 {
            0.0
        } else {
            (d[i - 1] + d[i]) / 2.0
        };
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[i] = t * a * d[i];
            m[i + 1] = t * b * d[i];
        }
    }
    table(|x| {
        let i = p.windows(2).position(|w| x <= w[1].0).unwrap_or(n - 2);
        let (x0, y0) = p[i];
        let (x1, y1) = p[i + 1];
        let h = x1 - x0;
        let t = ((x - x0) / h).clamp(0.0, 1.0);
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * h * m[i]
            + (-2.0 * t3 + 3.0 * t2) * y1
            + (t3 - t2) * h * m[i + 1]
    })
}

/// A fixed hash of a pixel position and a seed, for noise that does not
/// depend on how the image was cut into tiles.
fn hash(x: i32, y: i32, seed: u32, k: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ seed.wrapping_mul(0xC2B2_AE3D)
        ^ k.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

impl Prepared {
    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    /// One pixel of a color filter.
    fn color(&self, c: Rgba, x: i32, y: i32) -> Rgba {
        if c.a == 0 {
            return c;
        }
        if let Some(t) = &self.lut {
            return Rgba::new(t[c.r as usize], t[c.g as usize], t[c.b as usize], c.a);
        }
        match &self.filter {
            Filter::HueSaturation {
                hue,
                saturation,
                lightness,
            } => {
                let mut hsl = c.to_hsl();
                hsl.h += hue;
                hsl.s *= 1.0 + saturation.clamp(-100.0, 100.0) / 100.0;
                let l = lightness.clamp(-100.0, 100.0) / 100.0;
                hsl.l = if l >= 0.0 {
                    hsl.l + (1.0 - hsl.l) * l
                } else {
                    hsl.l * (1.0 + l)
                };
                Rgba::from_hsl(hsl, c.a)
            }
            Filter::ColorBalance {
                shadows,
                midtones,
                highlights,
            } => {
                let l = c.luma() as f32 / 255.0;
                let (ws, wh) = ((1.0 - l) * (1.0 - l), l * l);
                let wm = 1.0 - ws - wh;
                let ch = |v: u8, k: usize| {
                    let d = (shadows[k] * ws + midtones[k] * wm + highlights[k] * wh) * 0.64;
                    (v as f32 + d).round().clamp(0.0, 255.0) as u8
                };
                Rgba::new(ch(c.r, 0), ch(c.g, 1), ch(c.b, 2), c.a)
            }
            Filter::Desaturate => {
                let v = c.luma();
                Rgba::new(v, v, v, c.a)
            }
            Filter::GradientMap { stops } => {
                if stops.is_empty() {
                    return c;
                }
                let s = c.luma() as f32 / 255.0 * (stops.len() - 1) as f32;
                let i = (s.floor() as usize).min(stops.len() - 1);
                let j = (i + 1).min(stops.len() - 1);
                let f = s - i as f32;
                let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f).round() as u8;
                let (a, b) = (stops[i], stops[j]);
                Rgba::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), c.a)
            }
            Filter::Noise {
                amount,
                monochrome,
                seed,
            } => {
                let a = *amount as i32;
                if a == 0 {
                    return c;
                }
                let n = |k: u32| (hash(x, y, *seed, k) % (2 * a as u32 + 1)) as i32 - a;
                let (dr, dg, db) = if *monochrome {
                    let v = n(0);
                    (v, v, v)
                } else {
                    (n(0), n(1), n(2))
                };
                let ch = |v: u8, d: i32| (v as i32 + d).clamp(0, 255) as u8;
                Rgba::new(ch(c.r, dr), ch(c.g, dg), ch(c.b, db), c.a)
            }
            _ => c,
        }
    }

    /// The new raw values of `rect`, row-major. Pixels outside the
    /// selection, and outside the image, keep their source values.
    pub fn render(&self, input: &FilterInput, rect: Rect) -> Vec<u32> {
        let rect = rect.intersect(&input.raster.bounds());
        let mut out = Vec::with_capacity(rect.area().max(0) as usize);
        match &self.filter {
            Filter::Pixelate { size } => return self.pixelate(input, rect, (*size).max(1)),
            Filter::Outline {
                color,
                outside,
                connectivity,
            } => {
                let value = input.encode(*color);
                let near: &[(i32, i32)] = match connectivity {
                    Connectivity::Four => &[(1, 0), (-1, 0), (0, 1), (0, -1)],
                    Connectivity::Eight => &[
                        (1, 0),
                        (-1, 0),
                        (0, 1),
                        (0, -1),
                        (1, 1),
                        (-1, -1),
                        (1, -1),
                        (-1, 1),
                    ],
                };
                for p in rect.points() {
                    let raw = input.raster.get_raw(p.x, p.y);
                    let here = input.opaque(p.x, p.y);
                    let edge = if *outside {
                        !here && near.iter().any(|(dx, dy)| input.opaque(p.x + dx, p.y + dy))
                    } else {
                        here && near
                            .iter()
                            .any(|(dx, dy)| !input.opaque(p.x + dx, p.y + dy))
                    };
                    out.push(if edge && input.selected(p.x, p.y) {
                        value
                    } else {
                        raw
                    });
                }
            }
            Filter::DropShadow { offset, color } => {
                let value = input.encode(*color);
                for p in rect.points() {
                    let raw = input.raster.get_raw(p.x, p.y);
                    let cast = !input.opaque(p.x, p.y)
                        && input.opaque(p.x - offset.x, p.y - offset.y)
                        && input.selected(p.x, p.y);
                    out.push(if cast { value } else { raw });
                }
            }
            _ => {
                for p in rect.points() {
                    let raw = input.raster.get_raw(p.x, p.y);
                    if !input.selected(p.x, p.y) {
                        out.push(raw);
                        continue;
                    }
                    let c = input.decode(raw);
                    let n = self.color(c, p.x, p.y);
                    out.push(if n == c { raw } else { input.encode(n) });
                }
            }
        }
        out
    }

    fn pixelate(&self, input: &FilterInput, rect: Rect, size: u32) -> Vec<u32> {
        let s = size as i32;
        let w = rect.w.max(0) as usize;
        let mut out: Vec<u32> = rect
            .points()
            .map(|p| input.raster.get_raw(p.x, p.y))
            .collect();
        let bounds = input.raster.bounds();
        let bx0 = rect.x.div_euclid(s);
        let by0 = rect.y.div_euclid(s);
        let bx1 = (rect.right() - 1).div_euclid(s);
        let by1 = (rect.bottom() - 1).div_euclid(s);
        let transparent = input.encode(Rgba::TRANSPARENT);
        for by in by0..=by1 {
            for bx in bx0..=bx1 {
                let block = Rect::new(bx * s, by * s, s, s).intersect(&bounds);
                // The most common opaque value, first seen on ties.
                let mut counts: Vec<(u32, u32)> = Vec::new();
                let mut opaque = 0;
                for p in block.points() {
                    if !input.opaque(p.x, p.y) {
                        continue;
                    }
                    opaque += 1;
                    let raw = input.raster.get_raw(p.x, p.y);
                    match counts.iter_mut().find(|(v, _)| *v == raw) {
                        Some(e) => e.1 += 1,
                        None => counts.push((raw, 1)),
                    }
                }
                let value = if opaque * 2 < block.area() as u32 {
                    transparent
                } else {
                    let mut best = (0u32, 0u32);
                    for &(v, n) in &counts {
                        if n > best.1 {
                            best = (v, n);
                        }
                    }
                    best.0
                };
                for p in block.intersect(&rect).points() {
                    if input.selected(p.x, p.y) {
                        let i = (p.y - rect.y) as usize * w + (p.x - rect.x) as usize;
                        out[i] = value;
                    }
                }
            }
        }
        out
    }
}

/// Runs `filter` over `rect` of `raster` in one go and returns the changed
/// area. The editor renders tiles itself; this is the whole job at once.
pub fn apply_filter(
    raster: &mut Raster,
    palette: &Palette,
    transparent_index: u8,
    selection: Option<&Selection>,
    filter: &Filter,
    rect: Rect,
) -> Rect {
    let input = FilterInput {
        raster: raster.clone(),
        palette: palette.clone(),
        transparent_index,
        selection: selection.cloned(),
    };
    let rect = rect.intersect(&raster.bounds());
    let values = filter.prepare().render(&input, rect);
    write_values(raster, rect, &values)
}

/// The raw values of `rect` of `raster`, row by row: what
/// [`write_values`] writes back.
pub fn read_values(raster: &Raster, rect: Rect) -> Vec<u32> {
    rect.points().map(|p| raster.get_raw(p.x, p.y)).collect()
}

/// Writes rendered values back into `raster`; returns the changed area.
pub fn write_values(raster: &mut Raster, rect: Rect, values: &[u32]) -> Rect {
    let mut dirty = Rect::EMPTY;
    for (p, v) in rect.points().zip(values) {
        if raster.set_raw(p.x, p.y, *v) {
            dirty.include(p.x, p.y);
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_document::color::ColorMode;
    use pixl_document::image::TiledImage;

    fn rgba(w: u32, h: u32, px: &[(i32, i32, Rgba)]) -> Raster {
        let mut img = TiledImage::new(w, h, Rgba::TRANSPARENT);
        for &(x, y, c) in px {
            img.set(x, y, c);
        }
        Raster::Rgba(img)
    }

    fn run(r: &mut Raster, f: Filter) -> Rect {
        let b = r.bounds();
        apply_filter(r, &Palette::default(), 0, None, &f, b)
    }

    fn at(r: &Raster, x: i32, y: i32) -> Rgba {
        Rgba::from_u32(r.get_raw(x, y))
    }

    #[test]
    fn color_adjustments() {
        let red = Rgba::opaque(200, 40, 40);
        let mut r = rgba(2, 1, &[(0, 0, red)]);
        run(&mut r, Filter::Invert);
        assert_eq!(at(&r, 0, 0), Rgba::opaque(55, 215, 215));
        assert_eq!(at(&r, 1, 0), Rgba::TRANSPARENT, "clear pixels stay clear");
        let mut r = rgba(1, 1, &[(0, 0, red)]);
        run(&mut r, Filter::Desaturate);
        let v = red.luma();
        assert_eq!(at(&r, 0, 0), Rgba::opaque(v, v, v));
        let mut r = rgba(1, 1, &[(0, 0, red)]);
        run(
            &mut r,
            Filter::HueSaturation {
                hue: 120.0,
                saturation: 0.0,
                lightness: 0.0,
            },
        );
        assert_eq!(at(&r, 0, 0), Rgba::opaque(40, 200, 40));
        let mut r = rgba(1, 1, &[(0, 0, Rgba::opaque(10, 100, 250))]);
        run(&mut r, Filter::Posterize { levels: 2 });
        assert_eq!(at(&r, 0, 0), Rgba::opaque(0, 0, 255));
    }

    #[test]
    fn levels_and_curves() {
        let grey = |v| Rgba::opaque(v, v, v);
        let mut r = rgba(
            3,
            1,
            &[(0, 0, grey(50)), (1, 0, grey(128)), (2, 0, grey(200))],
        );
        run(
            &mut r,
            Filter::Levels {
                input: (50, 200),
                gamma: 1.0,
                output: (0, 255),
            },
        );
        assert_eq!(at(&r, 0, 0), grey(0));
        assert_eq!(at(&r, 2, 0), grey(255));
        let identity = curve_table(&[]);
        assert!(identity.iter().enumerate().all(|(i, v)| *v as usize == i));
        let s = curve_table(&[(64, 32), (192, 224)]);
        assert!(s[64] == 32 && s[192] == 224);
        assert!(s.windows(2).all(|w| w[0] <= w[1]), "monotone");
        let flat_top = curve_table(&[(128, 255)]);
        assert_eq!(flat_top[200], 255);
    }

    #[test]
    fn brightness_contrast_and_balance_move_the_right_way() {
        let grey = Rgba::opaque(100, 100, 100);
        let mut r = rgba(1, 1, &[(0, 0, grey)]);
        run(
            &mut r,
            Filter::BrightnessContrast {
                brightness: 50.0,
                contrast: 0.0,
            },
        );
        assert!(at(&r, 0, 0).r > 100);
        let mut r = rgba(1, 1, &[(0, 0, grey)]);
        run(
            &mut r,
            Filter::ColorBalance {
                shadows: [0.0; 3],
                midtones: [100.0, 0.0, -100.0],
                highlights: [0.0; 3],
            },
        );
        let c = at(&r, 0, 0);
        assert!(c.r > 100 && c.g == 100 && c.b < 100, "{c:?}");
    }

    #[test]
    fn gradient_map_and_noise() {
        let black = Rgba::BLACK;
        let mut r = rgba(2, 1, &[(0, 0, black), (1, 0, Rgba::WHITE)]);
        let stops = vec![Rgba::opaque(0, 0, 80), Rgba::opaque(255, 200, 0)];
        run(&mut r, Filter::GradientMap { stops });
        assert_eq!(at(&r, 0, 0), Rgba::opaque(0, 0, 80));
        assert_eq!(at(&r, 1, 0), Rgba::opaque(255, 200, 0));
        let mut a = rgba(8, 8, &[]);
        let mut b = rgba(8, 8, &[]);
        for p in Rect::new(0, 0, 8, 8).points() {
            for r in [&mut a, &mut b] {
                r.set_raw(p.x, p.y, Rgba::opaque(128, 128, 128).to_u32());
            }
        }
        let noise = Filter::Noise {
            amount: 20,
            monochrome: true,
            seed: 7,
        };
        run(&mut a, noise.clone());
        // The same noise, rendered tile by tile.
        let prep = noise.prepare();
        let input = FilterInput {
            raster: b.clone(),
            palette: Palette::default(),
            transparent_index: 0,
            selection: None,
        };
        for t in [Rect::new(0, 0, 4, 8), Rect::new(4, 0, 4, 8)] {
            let v = prep.render(&input, t);
            write_values(&mut b, t, &v);
        }
        assert_eq!(a, b);
        let c = at(&a, 3, 3);
        assert!(c.r == c.g && c.g == c.b && c.r.abs_diff(128) <= 20);
        assert!(
            Rect::new(0, 0, 8, 8)
                .points()
                .any(|p| at(&a, p.x, p.y).r != 128)
        );
    }

    #[test]
    fn outline_inside_outside_and_connectivity() {
        let w = Rgba::WHITE;
        let ink = Rgba::opaque(255, 0, 0);
        let dot = || rgba(5, 5, &[(2, 2, w)]);
        let mut four = dot();
        run(
            &mut four,
            Filter::Outline {
                color: ink,
                outside: true,
                connectivity: Connectivity::Four,
            },
        );
        let red = |r: &Raster| {
            Rect::new(0, 0, 5, 5)
                .points()
                .filter(|p| at(r, p.x, p.y) == ink)
                .count()
        };
        assert_eq!(red(&four), 4);
        let mut eight = dot();
        run(
            &mut eight,
            Filter::Outline {
                color: ink,
                outside: true,
                connectivity: Connectivity::Eight,
            },
        );
        assert_eq!(red(&eight), 8);
        let mut block = rgba(5, 5, &[]);
        for p in Rect::new(1, 1, 3, 3).points() {
            block.set_raw(p.x, p.y, w.to_u32());
        }
        run(
            &mut block,
            Filter::Outline {
                color: ink,
                outside: false,
                connectivity: Connectivity::Four,
            },
        );
        assert_eq!(red(&block), 8, "the ring inside, the middle stays");
        assert_eq!(at(&block, 2, 2), w);
    }

    #[test]
    fn drop_shadow_goes_behind() {
        let w = Rgba::WHITE;
        let s = Rgba::opaque(0, 0, 0);
        let mut r = rgba(4, 4, &[(1, 1, w), (2, 1, w)]);
        run(
            &mut r,
            Filter::DropShadow {
                offset: Point::new(1, 1),
                color: s,
            },
        );
        assert_eq!(at(&r, 2, 2), s);
        assert_eq!(at(&r, 3, 2), s);
        assert_eq!(at(&r, 2, 1), w, "the art stays on top");
    }

    #[test]
    fn pixelate_takes_the_most_common_color_per_block() {
        let (a, b) = (Rgba::opaque(9, 9, 9), Rgba::opaque(200, 0, 0));
        let mut r = rgba(
            4,
            2,
            &[(0, 0, a), (1, 0, a), (0, 1, b), (1, 1, a), (2, 0, b)],
        );
        run(&mut r, Filter::Pixelate { size: 2 });
        for p in Rect::new(0, 0, 2, 2).points() {
            assert_eq!(at(&r, p.x, p.y), a);
        }
        // Three of four clear: the block goes clear.
        for p in Rect::new(2, 0, 2, 2).points() {
            assert_eq!(at(&r, p.x, p.y), Rgba::TRANSPARENT);
        }
        // Tiles that cut a block give the same result.
        let mut whole = rgba(6, 6, &[]);
        for p in Rect::new(0, 0, 6, 6).points() {
            let c = Rgba::opaque((p.x * 40) as u8, (p.y * 40) as u8, 0);
            whole.set_raw(p.x, p.y, c.to_u32());
        }
        let mut tiled = whole.clone();
        run(&mut whole, Filter::Pixelate { size: 3 });
        let f = Filter::Pixelate { size: 3 }.prepare();
        let input = FilterInput {
            raster: tiled.clone(),
            palette: Palette::default(),
            transparent_index: 0,
            selection: None,
        };
        for t in [Rect::new(0, 0, 2, 6), Rect::new(2, 0, 4, 6)] {
            let v = f.render(&input, t);
            write_values(&mut tiled, t, &v);
        }
        assert_eq!(whole, tiled);
    }

    #[test]
    fn selection_limits_and_indexed_stays_on_the_palette() {
        let mut r = rgba(2, 1, &[(0, 0, Rgba::WHITE), (1, 0, Rgba::WHITE)]);
        let sel = Selection::rect(2, 1, Rect::pixel(1, 0));
        let b = r.bounds();
        apply_filter(
            &mut r,
            &Palette::default(),
            0,
            Some(&sel),
            &Filter::Invert,
            b,
        );
        assert_eq!(at(&r, 0, 0), Rgba::WHITE);
        assert_eq!(at(&r, 1, 0), Rgba::opaque(0, 0, 0));

        let mut d = pixl_document::Document::new(2, 1, ColorMode::Indexed).unwrap();
        let l = d.layers()[0].id;
        let pal = d.palette().clone();
        let t = d.target(l, 0).unwrap();
        t.raster.set_raw(0, 0, 2); // white
        let mut raster = t.raster.clone();
        apply_filter(
            &mut raster,
            &pal,
            0,
            None,
            &Filter::Invert,
            Rect::new(0, 0, 2, 1),
        );
        assert_eq!(raster.get_raw(0, 0), 1, "white inverts to black's index");
        assert_eq!(raster.get_raw(1, 0), 0, "transparent stays");
    }
}
