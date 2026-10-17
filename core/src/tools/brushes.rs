//! Brushes beyond the hard pencil: the soft brush (opacity and a soft edge,
//! or the same coverage as an ordered dither), the spray, the smudge, and
//! the stroke stabilizer that smooths pointer input before any tool sees it.

use std::collections::HashMap;

use crate::color::{Color, Rgba};
use crate::geom::Point;
use crate::image::Raster;
use crate::tools::shapes;
use crate::tools::{Bayer, Brush, Canvas, Ink};

/// What a soft brush lays down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoftInk {
    Paint(Rgba),
    /// Takes alpha away.
    Erase,
}

/// A soft brush's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftBrush {
    /// Diameter in pixels.
    pub size: u32,
    /// The most a stroke covers a pixel, 0 to 255.
    pub opacity: u8,
    /// How much of the radius is at full opacity, 0 (falls off from the
    /// middle) to 255 (a hard edge).
    pub hardness: u8,
    /// Lay the coverage down as an ordered dither of whole pixels instead of
    /// as alpha: no blended colors appear.
    pub dithered: bool,
    pub ink: SoftInk,
}

impl SoftBrush {
    /// The coverage at `(dx, dy)` from the dab's middle, 0 to 255.
    pub fn coverage(&self, dx: i32, dy: i32) -> u8 {
        let r = self.size.max(1) as f32 / 2.0;
        // Pixel centers: an even size has its middle between pixels.
        let off = if self.size.is_multiple_of(2) {
            0.5
        } else {
            0.0
        };
        let (x, y) = (dx as f32 + off, dy as f32 + off);
        let d = (x * x + y * y).sqrt() / r;
        if d >= 1.0 {
            return 0;
        }
        let hard = self.hardness as f32 / 255.0;
        let f = if d <= hard || hard >= 1.0 {
            1.0
        } else {
            let t = (d - hard) / (1.0 - hard);
            1.0 - t * t * (3.0 - 2.0 * t)
        };
        (f * self.opacity as f32).round() as u8
    }

    /// The pixels a dab covers, with their coverage.
    pub fn dab(&self) -> Vec<(Point, u8)> {
        let s = self.size.max(1) as i32;
        let o = -(s - 1) / 2;
        let mut out = Vec::new();
        for y in o..o + s {
            for x in o..o + s {
                let c = self.coverage(x, y);
                if c > 0 {
                    out.push((Point::new(x, y), c));
                }
            }
        }
        out
    }
}

/// Straight-alpha source-over of `top` at `alpha` over `under`.
pub fn blend_over(under: Rgba, top: Rgba, alpha: u8) -> Rgba {
    let sa = top.a as f32 / 255.0 * alpha as f32 / 255.0;
    let da = under.a as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return Rgba::TRANSPARENT;
    }
    let ch = |s: u8, d: u8| {
        ((s as f32 * sa + d as f32 * da * (1.0 - sa)) / oa)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgba::new(
        ch(top.r, under.r),
        ch(top.g, under.g),
        ch(top.b, under.b),
        (oa * 255.0).round() as u8,
    )
}

/// A soft brush stroke. Overlapping dabs never build up past the brush's
/// opacity: each pixel keeps the most coverage any dab gave it, over the
/// pixel as the stroke found it.
pub struct SoftStroke {
    brush: SoftBrush,
    last: Option<Point>,
    covered: HashMap<Point, (u8, u32)>,
}

impl SoftStroke {
    pub fn new(brush: SoftBrush) -> Self {
        Self {
            brush,
            last: None,
            covered: HashMap::new(),
        }
    }

    /// Extends the stroke to `p`.
    pub fn add(&mut self, canvas: &mut Canvas, p: Point) {
        let sym = canvas.symmetry();
        let from = self.last.unwrap_or(p);
        self.last = Some(p);
        let dab = self.brush.dab();
        for k in 0..sym.rotations() {
            for q in shapes::line(sym.rotate(from, k), sym.rotate(p, k)) {
                for &(o, cov) in &dab {
                    for m in sym.mirrors(Point::new(q.x + o.x, q.y + o.y)) {
                        self.cover(canvas, m, cov);
                    }
                }
            }
        }
    }

    fn cover(&mut self, canvas: &mut Canvas, p: Point, cov: u8) {
        if !canvas.bounds().contains(p.x, p.y) {
            return;
        }
        let original = canvas.get_raw(p.x, p.y);
        let entry = self.covered.entry(p).or_insert((0, original));
        if cov <= entry.0 {
            return;
        }
        entry.0 = cov;
        let original = entry.1;
        let b = self.brush;
        let value = if b.dithered {
            if !Bayer::B4.picks_first(p.x, p.y, cov) {
                return;
            }
            match b.ink {
                SoftInk::Paint(c) => canvas.resolve(Color::Rgba(c)),
                SoftInk::Erase => canvas.transparent_raw(),
            }
        } else {
            let under = canvas.raw_to_rgba(original);
            let out = match b.ink {
                SoftInk::Paint(c) => blend_over(under, c, cov),
                SoftInk::Erase => Rgba::new(
                    under.r,
                    under.g,
                    under.b,
                    (under.a as u32 * (255 - cov as u32) / 255) as u8,
                ),
            };
            if matches!(canvas.raster(), Raster::Indexed(_)) && out.a > 0 && out.a < 255 {
                // An indexed pixel is either clear or a palette color.
                if out.a < 128 {
                    canvas.transparent_raw()
                } else {
                    canvas.resolve(Color::Rgba(Rgba::new(out.r, out.g, out.b, 255)))
                }
            } else {
                canvas.resolve(Color::Rgba(out))
            }
        };
        canvas.write(p.x, p.y, value);
    }
}

/// A small deterministic random sequence (xorshift32).
#[derive(Clone, Copy, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// A number in `0.0..1.0`.
    pub fn unit(&mut self) -> f64 {
        self.next_u32() as f64 / (u32::MAX as f64 + 1.0)
    }
}

/// One spray dab: `count` pixels scattered evenly over the disc of `radius`
/// around `center`, each painted with `ink`. The same `rng` state gives the
/// same scatter.
pub fn spray(
    canvas: &mut Canvas,
    center: Point,
    radius: u32,
    count: u32,
    ink: &Ink,
    rng: &mut Rng,
) {
    let r = radius.max(1) as f64;
    let sym = canvas.symmetry();
    for _ in 0..count {
        // Uniform over the disc: the radius goes with the root.
        let d = rng.unit().sqrt() * r;
        let a = rng.unit() * std::f64::consts::TAU;
        let p = Point::new(
            center.x + (d * a.cos()).round() as i32,
            center.y + (d * a.sin()).round() as i32,
        );
        for k in 0..sym.rotations() {
            canvas.plot(sym.rotate(p, k), ink);
        }
    }
}

/// A smudge stroke: the brush picks up the pixels under it and drags them
/// along, mixing them into what it passes over by `strength` (255 drags
/// them unchanged). With `keep_colors`, a pixel takes either the dragged
/// pixel or its own, by an ordered dither of the strength, so the smudge
/// never makes a new color; indexed images always smudge this way.
pub struct SmudgeStroke {
    brush: Brush,
    strength: u8,
    keep_colors: bool,
    last: Option<Point>,
    /// Per rotated copy and per mirror, the pixels the brush carries.
    carried: Vec<Vec<Option<Vec<u32>>>>,
}

impl SmudgeStroke {
    pub fn new(canvas: &Canvas, brush: Brush, strength: u8, keep_colors: bool) -> Self {
        let sym = canvas.symmetry();
        let mirrors = sym.mirrors(Point::new(0, 0)).count();
        Self {
            brush,
            strength,
            keep_colors: keep_colors || matches!(canvas.raster(), Raster::Indexed(_)),
            last: None,
            carried: vec![vec![None; mirrors]; sym.rotations()],
        }
    }

    pub fn add(&mut self, canvas: &mut Canvas, p: Point) {
        let sym = canvas.symmetry();
        let from = self.last.unwrap_or(p);
        self.last = Some(p);
        let offsets = self.brush.offsets();
        for k in 0..sym.rotations() {
            let path = shapes::line(sym.rotate(from, k), sym.rotate(p, k));
            // The press itself only picks up; every later step drags.
            let start = usize::from(self.carried[k][0].is_some() && path.len() > 1);
            for q in &path[start..] {
                let mirrors: Vec<Point> = sym.mirrors(*q).collect();
                for (mi, m) in mirrors.into_iter().enumerate() {
                    self.step(canvas, k, mi, m, &offsets);
                }
            }
        }
    }

    fn step(&mut self, canvas: &mut Canvas, k: usize, mi: usize, at: Point, offsets: &[Point]) {
        let here: Vec<u32> = offsets
            .iter()
            .map(|o| canvas.get_raw(at.x + o.x, at.y + o.y))
            .collect();
        let Some(carried) = self.carried[k][mi].take() else {
            self.carried[k][mi] = Some(here);
            return;
        };
        let mut next = Vec::with_capacity(offsets.len());
        for (i, o) in offsets.iter().enumerate() {
            let (x, y) = (at.x + o.x, at.y + o.y);
            let (cur, drag) = (here[i], carried[i]);
            let v = if cur == drag {
                cur
            } else if self.keep_colors {
                if Bayer::B4.picks_first(x, y, self.strength) {
                    drag
                } else {
                    cur
                }
            } else {
                let (c, d) = (canvas.raw_to_rgba(cur), canvas.raw_to_rgba(drag));
                let t = self.strength as f32 / 255.0;
                let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
                canvas.resolve(Color::Rgba(Rgba::new(
                    mix(c.r, d.r),
                    mix(c.g, d.g),
                    mix(c.b, d.b),
                    mix(c.a, d.a),
                )))
            };
            if canvas.bounds().contains(x, y) && v != cur {
                canvas.write(x, y, v);
            }
            next.push(canvas.get_raw(x, y));
        }
        self.carried[k][mi] = Some(next);
    }
}

/// Smooths pointer input like a string pulled behind the pointer: the
/// brush only moves when the pointer gets further than `strength` pixels
/// away, and then follows it at that distance. Strength 0 passes every
/// point through.
#[derive(Clone, Debug, Default)]
pub struct Stabilizer {
    strength: f64,
    at: Option<(f64, f64)>,
}

impl Stabilizer {
    pub fn new(strength: f64) -> Self {
        Self {
            strength: strength.max(0.0),
            at: None,
        }
    }

    /// Feeds a pointer position (document pixels, fractional) and returns
    /// where the brush is now.
    pub fn feed(&mut self, x: f64, y: f64) -> (f64, f64) {
        let Some((bx, by)) = self.at else {
            self.at = Some((x, y));
            return (x, y);
        };
        let (dx, dy) = (x - bx, y - by);
        let d = (dx * dx + dy * dy).sqrt();
        if d <= self.strength {
            return (bx, by);
        }
        let k = (d - self.strength) / d;
        let next = (bx + dx * k, by + dy * k);
        self.at = Some(next);
        next
    }

    /// Where the brush is.
    pub fn position(&self) -> Option<(f64, f64)> {
        self.at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorMode;
    use crate::document::Document;
    use crate::geom::Rect;
    use crate::history::History;
    use crate::tools::{Symmetry, SymmetryMode, ToolContext, edit_cel};

    fn doc(w: u32) -> (Document, crate::LayerId) {
        let d = Document::new(w, w, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        (d, l)
    }

    fn px(d: &Document, l: crate::LayerId, x: i32, y: i32) -> Rgba {
        Rgba::from_u32(d.cel_raster(l, 0).unwrap().unwrap().get_raw(x, y))
    }

    #[test]
    fn soft_coverage_falls_off_and_never_builds_up() {
        let b = SoftBrush {
            size: 9,
            opacity: 200,
            hardness: 0,
            dithered: false,
            ink: SoftInk::Paint(Rgba::opaque(255, 0, 0)),
        };
        assert_eq!(b.coverage(0, 0), 200);
        assert!(b.coverage(3, 0) < b.coverage(1, 0));
        assert_eq!(b.coverage(5, 0), 0);
        let (mut d, l) = doc(32);
        let mut h = History::default();
        edit_cel(&mut d, &mut h, "b", l, 0, &ToolContext::default(), |c| {
            let mut s = SoftStroke::new(b);
            for x in [10, 11, 12, 11, 10] {
                s.add(c, Point::new(x, 10));
            }
        })
        .unwrap();
        assert_eq!(px(&d, l, 10, 10).a, 200, "overlaps stay at the opacity");
        assert!(px(&d, l, 13, 10).a < 200);
    }

    #[test]
    fn dithered_soft_brush_paints_whole_pixels() {
        let b = SoftBrush {
            size: 8,
            opacity: 128,
            hardness: 255,
            dithered: true,
            ink: SoftInk::Paint(Rgba::WHITE),
        };
        let (mut d, l) = doc(16);
        let mut h = History::default();
        edit_cel(&mut d, &mut h, "b", l, 0, &ToolContext::default(), |c| {
            SoftStroke::new(b).add(c, Point::new(8, 8))
        })
        .unwrap();
        let mut whole = 0;
        for p in Rect::new(0, 0, 16, 16).points() {
            let c = px(&d, l, p.x, p.y);
            assert!(c.a == 0 || c == Rgba::WHITE);
            whole += usize::from(c.a > 0);
        }
        let dab = b.dab().len();
        assert!(whole > dab / 3 && whole < dab * 2 / 3, "{whole} of {dab}");
    }

    #[test]
    fn spray_scatters_inside_the_radius_the_same_way_twice() {
        let run = || {
            let (mut d, l) = doc(64);
            let mut h = History::default();
            edit_cel(&mut d, &mut h, "s", l, 0, &ToolContext::default(), |c| {
                let mut rng = Rng::new(42);
                spray(
                    c,
                    Point::new(32, 32),
                    6,
                    30,
                    &Ink::Paint(Rgba::WHITE.into()),
                    &mut rng,
                )
            })
            .unwrap();
            d.cel_raster(l, 0).unwrap().unwrap().content_bounds()
        };
        let b = run();
        assert!(Rect::new(26, 26, 13, 13).intersect(&b) == b, "{b:?}");
        assert_eq!(run(), b);
    }

    #[test]
    fn smudge_drags_pixels_and_keeps_colors_when_asked() {
        let (mut d, l) = doc(16);
        let red = Rgba::opaque(255, 0, 0);
        d.target(l, 0).unwrap().raster.set_raw(2, 5, red.to_u32());
        let mut h = History::default();
        edit_cel(&mut d, &mut h, "s", l, 0, &ToolContext::default(), |c| {
            let mut s = SmudgeStroke::new(c, Brush::PIXEL, 255, false);
            for x in 2..=6 {
                s.add(c, Point::new(x, 5));
            }
        })
        .unwrap();
        assert_eq!(px(&d, l, 6, 5), red, "full strength drags it all the way");
        let (mut d, l) = doc(16);
        d.target(l, 0).unwrap().raster.set_raw(2, 5, red.to_u32());
        edit_cel(&mut d, &mut h, "s", l, 0, &ToolContext::default(), |c| {
            let mut s = SmudgeStroke::new(c, Brush::square(3), 140, true);
            for x in 2..=9 {
                s.add(c, Point::new(x, 5));
            }
        })
        .unwrap();
        for p in Rect::new(0, 0, 16, 16).points() {
            let c = px(&d, l, p.x, p.y);
            assert!(c == red || c == Rgba::TRANSPARENT, "no new colors");
        }
    }

    #[test]
    fn clone_pattern_and_blur_inks() {
        let (mut d, l) = doc(16);
        let red = Rgba::opaque(255, 0, 0);
        d.target(l, 0).unwrap().raster.set_raw(1, 1, red.to_u32());
        let source = std::sync::Arc::new(d.cel_raster(l, 0).unwrap().unwrap().clone());
        let mut h = History::default();
        let mirror = ToolContext {
            symmetry: Symmetry::centered(SymmetryMode::Horizontal, 16, 16),
            ..Default::default()
        };
        edit_cel(&mut d, &mut h, "c", l, 0, &mirror, |c| {
            let ink = Ink::Clone {
                source: source.clone(),
                offset: Point::new(-4, -4),
            };
            c.plot(Point::new(5, 5), &ink);
        })
        .unwrap();
        assert_eq!(px(&d, l, 5, 5), red);
        let pattern = crate::transform::Pixels {
            width: 2,
            height: 1,
            data: vec![Rgba::WHITE.to_u32(), 0],
        };
        edit_cel(&mut d, &mut h, "p", l, 0, &ToolContext::default(), |c| {
            let ink = Ink::Pattern {
                pattern: std::sync::Arc::new(pattern),
                origin: Point::new(0, 0),
                skip: Some(0),
            };
            for x in 0..4 {
                c.plot(Point::new(x, 8), &ink);
            }
        })
        .unwrap();
        let row: Vec<u8> = (0..4).map(|x| px(&d, l, x, 8).a).collect();
        assert_eq!(row, vec![255, 0, 255, 0]);
        let src = std::sync::Arc::new(d.cel_raster(l, 0).unwrap().unwrap().clone());
        edit_cel(&mut d, &mut h, "b", l, 0, &ToolContext::default(), |c| {
            let ink = Ink::Blur {
                source: src.clone(),
                sharpen: false,
                amount: 255,
                keep_colors: true,
            };
            c.plot(Point::new(1, 8), &ink);
        })
        .unwrap();
        let c = px(&d, l, 1, 8);
        assert!(c == Rgba::WHITE || c.a == 0, "kept to nearby colors: {c:?}");
    }

    #[test]
    fn the_stabilizer_trails_the_pointer() {
        let mut s = Stabilizer::new(4.0);
        assert_eq!(s.feed(0.0, 0.0), (0.0, 0.0));
        assert_eq!(s.feed(3.0, 0.0), (0.0, 0.0), "inside the string");
        assert_eq!(s.feed(10.0, 0.0), (6.0, 0.0));
        let mut off = Stabilizer::new(0.0);
        off.feed(0.0, 0.0);
        assert_eq!(off.feed(5.0, 5.0), (5.0, 5.0));
    }
}
