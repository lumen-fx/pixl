//! Gradients in hard steps: linear, radial and conic, between two or more
//! colors, with the steps broken up by an ordered dither, blue noise, a
//! custom pattern, or not at all. No pixel ever takes a blend of two colors.

use std::sync::{Arc, OnceLock};

use crate::color::Color;
use crate::geom::{Point, Rect};
use crate::tools::{Bayer, Canvas, Ink};

/// The shape a gradient runs in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradientShape {
    /// Along the line from the start to the end point.
    #[default]
    Linear,
    /// Out from the start point; the end point sets the radius.
    Radial,
    /// Around the start point, from the direction of the end point.
    Conic,
}

/// How a gradient breaks up the boundary between two of its colors.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum DitherPattern {
    /// Hard bands: each pixel takes the nearer color.
    #[default]
    None,
    Bayer(Bayer),
    /// A blue noise threshold map: even, patternless dots.
    BlueNoise,
    /// A threshold map of `width x height`, each 0 to 255, tiled over the
    /// canvas (made from a pattern image's lightness).
    Custom {
        width: u32,
        height: u32,
        thresholds: Arc<Vec<u8>>,
    },
}

impl DitherPattern {
    /// The threshold at a canvas position, from 0 (exclusive) to 1.
    fn threshold(&self, x: i32, y: i32) -> f64 {
        match self {
            DitherPattern::None => 0.5,
            DitherPattern::Bayer(m) => {
                let cells = (m.size() * m.size()) as f64;
                (m.threshold(x, y) as f64 + 0.5) / cells
            }
            DitherPattern::BlueNoise => {
                let n = BLUE_NOISE_SIZE as i32;
                let map = blue_noise();
                let i = (y.rem_euclid(n) * n + x.rem_euclid(n)) as usize;
                (map[i] as f64 + 0.5) / map.len() as f64
            }
            DitherPattern::Custom {
                width,
                height,
                thresholds,
            } => {
                let (w, h) = ((*width).max(1) as i32, (*height).max(1) as i32);
                let i = (y.rem_euclid(h) * w + x.rem_euclid(w)) as usize;
                (thresholds.get(i).copied().unwrap_or(128) as f64 + 0.5) / 256.0
            }
        }
    }
}

/// Edge length of the blue noise map.
pub const BLUE_NOISE_SIZE: u32 = 32;

/// A blue noise ranking of a 32x32 torus, made once by Ulichney's
/// void-and-cluster method (1993): each pixel's rank is the order it was
/// picked in, filling the largest void first, so any threshold of the ranks
/// gives evenly spread dots.
pub fn blue_noise() -> &'static [u16] {
    static MAP: OnceLock<Vec<u16>> = OnceLock::new();
    MAP.get_or_init(|| void_and_cluster(BLUE_NOISE_SIZE as usize, 1.5))
}

fn void_and_cluster(n: usize, sigma: f64) -> Vec<u16> {
    let len = n * n;
    // The Gaussian energy kernel on the torus.
    let mut kernel = vec![0f64; len];
    for dy in 0..n {
        for dx in 0..n {
            let wx = dx.min(n - dx) as f64;
            let wy = dy.min(n - dy) as f64;
            kernel[dy * n + dx] = (-(wx * wx + wy * wy) / (2.0 * sigma * sigma)).exp();
        }
    }
    let mut energy = vec![0f64; len];
    let mut on = vec![false; len];
    let add = |energy: &mut [f64], i: usize, sign: f64| {
        let (x, y) = (i % n, i / n);
        for (j, e) in energy.iter_mut().enumerate() {
            let (jx, jy) = (j % n, j / n);
            let dx = (jx + n - x) % n;
            let dy = (jy + n - y) % n;
            *e += sign * kernel[dy * n + dx];
        }
    };
    // A fixed scattered start: one pixel in eight, by a simple hash.
    let mut seeds = 0;
    for (i, slot) in on.iter_mut().enumerate() {
        let h = (i as u32).wrapping_mul(2_654_435_761).rotate_left(13) % 8;
        if h == 0 {
            *slot = true;
            add(&mut energy, i, 1.0);
            seeds += 1;
        }
    }
    let tightest = |on: &[bool], energy: &[f64]| {
        (0..len)
            .filter(|&i| on[i])
            .max_by(|&a, &b| energy[a].total_cmp(&energy[b]).then(b.cmp(&a)))
    };
    let largest_void = |on: &[bool], energy: &[f64]| {
        (0..len)
            .filter(|&i| !on[i])
            .min_by(|&a, &b| energy[a].total_cmp(&energy[b]).then(a.cmp(&b)))
    };
    // Relax the start until moving the tightest cluster fills the largest void.
    for _ in 0..len {
        let (Some(c), Some(v)) = (tightest(&on, &energy), largest_void(&on, &energy)) else {
            break;
        };
        on[c] = false;
        add(&mut energy, c, -1.0);
        let v2 = largest_void(&on, &energy).unwrap_or(v);
        if v2 == c {
            on[c] = true;
            add(&mut energy, c, 1.0);
            break;
        }
        on[v2] = true;
        add(&mut energy, v2, 1.0);
    }
    let mut rank = vec![0u16; len];
    // Ranks below the start: take clusters out one by one.
    let (mut pattern, mut e) = (on.clone(), energy.clone());
    for r in (0..seeds).rev() {
        let Some(c) = tightest(&pattern, &e) else {
            break;
        };
        pattern[c] = false;
        add(&mut e, c, -1.0);
        rank[c] = r as u16;
    }
    // Ranks above it: fill voids one by one.
    let (mut pattern, mut e) = (on, energy);
    for r in seeds..len {
        let Some(v) = largest_void(&pattern, &e) else {
            break;
        };
        pattern[v] = true;
        add(&mut e, v, 1.0);
        rank[v] = r as u16;
    }
    rank
}

/// A gradient across the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub shape: GradientShape,
    pub from: Point,
    pub to: Point,
    /// The colors, evenly spaced from start to end; at least one.
    pub colors: Vec<Color>,
    pub dither: DitherPattern,
    /// Run back and forth over the colors instead of stopping at the last.
    pub repeat: bool,
}

impl Gradient {
    /// Where pixel `(x, y)` falls along the gradient, 0 to 1.
    pub fn position(&self, x: i32, y: i32) -> f64 {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        let (ax, ay) = (self.from.x as f64 + 0.5, self.from.y as f64 + 0.5);
        let (bx, by) = (self.to.x as f64 + 0.5, self.to.y as f64 + 0.5);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let t = match self.shape {
            GradientShape::Linear => {
                if len2 == 0.0 {
                    return 0.0;
                }
                ((px - ax) * dx + (py - ay) * dy) / len2
            }
            GradientShape::Radial => {
                if len2 == 0.0 {
                    return 0.0;
                }
                ((px - ax).powi(2) + (py - ay).powi(2)).sqrt() / len2.sqrt()
            }
            GradientShape::Conic => {
                let start = dy.atan2(dx);
                let a = (py - ay).atan2(px - ax) - start;
                a.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU
            }
        };
        if self.repeat {
            let m = t.rem_euclid(2.0);
            if m > 1.0 { 2.0 - m } else { m }
        } else {
            t.clamp(0.0, 1.0)
        }
    }

    /// The color pixel `(x, y)` takes.
    pub fn color_at(&self, x: i32, y: i32) -> Option<Color> {
        let n = self.colors.len();
        if n == 0 {
            return None;
        }
        if n == 1 {
            return Some(self.colors[0]);
        }
        let s = self.position(x, y) * (n - 1) as f64;
        let i = (s.floor() as usize).min(n - 2);
        let f = s - i as f64;
        let next = f >= self.dither.threshold(x, y);
        Some(self.colors[if next { i + 1 } else { i }])
    }
}

/// Paints `gradient` over every selected pixel of the canvas (with the
/// canvas's symmetry, lock and selection, as every tool).
pub fn gradient(canvas: &mut Canvas, gradient: &Gradient) {
    let all = canvas.bounds();
    gradient_in(canvas, gradient, all);
}

/// [`gradient`] within `rect` alone. Each pixel takes its color from where
/// it lies, so the parts of a canvas can be painted apart, on several
/// threads, and come out as one gradient.
pub fn gradient_in(canvas: &mut Canvas, gradient: &Gradient, rect: Rect) {
    let area = match canvas.selection() {
        Some(s) => s.bounds(),
        None => canvas.bounds(),
    }
    .intersect(&rect);
    for p in area.points() {
        if let Some(c) = gradient.color_at(p.x, p.y) {
            let ink = Ink::Paint(c);
            if let Some(v) = canvas.ink_value(&ink, p.x, p.y) {
                canvas.write(p.x, p.y, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;

    fn two(dither: DitherPattern, shape: GradientShape) -> Gradient {
        Gradient {
            shape,
            from: Point::new(0, 0),
            to: Point::new(31, 0),
            colors: vec![Rgba::BLACK.into(), Rgba::WHITE.into()],
            dither,
            repeat: false,
        }
    }

    fn whites(g: &Gradient, xs: std::ops::Range<i32>) -> usize {
        let mut n = 0;
        for y in 0..32 {
            for x in xs.clone() {
                if g.color_at(x, y) == Some(Rgba::WHITE.into()) {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn hard_steps_split_in_the_middle() {
        let g = two(DitherPattern::None, GradientShape::Linear);
        assert_eq!(g.color_at(0, 0), Some(Rgba::BLACK.into()));
        assert_eq!(g.color_at(31, 5), Some(Rgba::WHITE.into()));
        assert_eq!(whites(&g, 0..16), 0);
        assert_eq!(whites(&g, 16..32), 16 * 32);
    }

    #[test]
    fn dithers_mix_more_white_further_along() {
        for d in [
            DitherPattern::Bayer(Bayer::B4),
            DitherPattern::BlueNoise,
            DitherPattern::Custom {
                width: 2,
                height: 1,
                thresholds: Arc::new(vec![64, 192]),
            },
        ] {
            let g = two(d.clone(), GradientShape::Linear);
            let (a, b, c) = (whites(&g, 2..8), whites(&g, 13..19), whites(&g, 24..30));
            assert!(a < b && b < c, "{d:?}: {a} {b} {c}");
            assert!(b > 0 && b < 6 * 32, "{d:?} mixes in the middle");
        }
    }

    #[test]
    fn blue_noise_ranks_every_cell_once_and_spreads_dots() {
        let map = blue_noise();
        let mut sorted = map.to_vec();
        sorted.sort();
        assert_eq!(sorted, (0..1024).collect::<Vec<u16>>());
        // The first eighth of the ranks: no two dots touch side by side.
        let n = BLUE_NOISE_SIZE as i32;
        let lit = |x: i32, y: i32| map[(y.rem_euclid(n) * n + x.rem_euclid(n)) as usize] < 128;
        let mut touching = 0;
        for p in Rect::new(0, 0, n, n).points() {
            if lit(p.x, p.y) && (lit(p.x + 1, p.y) || lit(p.x, p.y + 1)) {
                touching += 1;
            }
        }
        assert!(touching < 8, "{touching}");
    }

    #[test]
    fn radial_and_conic_positions() {
        let r = Gradient {
            to: Point::new(10, 0),
            ..two(DitherPattern::None, GradientShape::Radial)
        };
        assert!(r.position(0, 0) < 0.01);
        assert!((r.position(0, 10) - 1.0).abs() < 0.01);
        let c = two(DitherPattern::None, GradientShape::Conic);
        assert!(c.position(10, 0) < 0.01);
        assert!((c.position(0, 10) - 0.25).abs() < 0.02);
        let rep = Gradient {
            to: Point::new(9, 0),
            repeat: true,
            ..two(DitherPattern::None, GradientShape::Linear)
        };
        assert!(rep.position(14, 0) < 0.6 && rep.position(14, 0) > 0.4);
    }

    #[test]
    fn a_gradient_painted_in_parts_matches_one_painted_whole() {
        use crate::color::Palette;
        use crate::document::Target;
        use crate::image::Raster;
        use crate::tools::ToolContext;
        let g = Gradient {
            to: Point::new(90, 40),
            ..two(DitherPattern::BlueNoise, GradientShape::Radial)
        };
        let palette = Palette::default();
        let ctx = ToolContext::default();
        let paint = |rects: &[Rect]| {
            let mut raster =
                Raster::Rgba(crate::image::TiledImage::new(100, 60, Rgba::TRANSPARENT));
            for r in rects {
                let target = Target {
                    raster: &mut raster,
                    palette: &palette,
                    transparent_index: 0,
                    alpha_locked: false,
                };
                gradient_in(&mut Canvas::new(target, &ctx), &g, *r);
            }
            crate::filters::read_values(&raster, Rect::new(0, 0, 100, 60))
        };
        let whole = paint(&[Rect::new(0, 0, 100, 60)]);
        let parts = paint(&[
            Rect::new(0, 0, 64, 64),
            Rect::new(64, 0, 64, 64),
            Rect::new(0, 64, 128, 64),
        ]);
        assert_eq!(whole, parts);
        assert!(whole.iter().any(|&v| v != whole[0]));
    }
}
