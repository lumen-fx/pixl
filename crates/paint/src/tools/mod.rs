//! Raster tools as functions over a [`Canvas`].
//!
//! A tool turns its input geometry into pixels; the canvas then applies the
//! brush, the ink, symmetry, the selection and the alpha lock to each one, in
//! that order. Every tool goes through [`Canvas::stamp`] or
//! [`Canvas::plot`], so every tool honors all of them.

pub mod brushes;
pub mod fill;
pub mod gradient;
pub mod paint;
pub mod text;

use std::collections::HashSet;
use std::sync::Arc;

use pixl_document::Bayer;
use pixl_document::color::{Color, Palette, Rgba};
use pixl_document::document::{Document, LayerId, Target};
use pixl_document::error::Error;
use pixl_document::geom::{Point, Rect};
use pixl_document::history::{History, Scope};
use pixl_document::image::Raster;
use pixl_document::selection::Selection;
use pixl_document::shapes;
use pixl_document::transform::Pixels;

/// What a tool writes at each pixel it touches.
#[derive(Clone, Debug, PartialEq)]
pub enum Ink {
    /// Writes a color.
    Paint(Color),
    /// Writes transparency.
    Erase,
    /// An ordered dither between two colors; `second: None` leaves those
    /// pixels untouched. The pattern is anchored to the canvas, so strokes
    /// line up with each other.
    Dither {
        first: Color,
        second: Option<Color>,
        matrix: Bayer,
        amount: u8,
    },
    /// Moves a pixel `step` entries along a ramp (negative is darker by the
    /// ramp's order). Pixels not on the ramp stay. Each pixel shades once per
    /// stroke.
    Shade { ramp: Vec<Color>, step: i32 },
    /// Writes `to` over pixels within `tolerance` of `from`.
    Replace {
        from: Color,
        to: Color,
        tolerance: u8,
    },
    /// Copies pixels from `source` (the image as the stroke started) at
    /// `offset` from the pixel painted: the clone stamp. Pixels whose source
    /// is off the image stay.
    Clone { source: Arc<Raster>, offset: Point },
    /// Tiles `pattern` (raw pixel values in the image's format) over the
    /// canvas from `origin`; values equal to `skip` leave the pixel.
    Pattern {
        pattern: Arc<Pixels>,
        origin: Point,
        skip: Option<u32>,
    },
    /// Softens (or with `sharpen`, hardens) each pixel against its eight
    /// neighbors in `source`, by `amount` (255 is the full effect). With
    /// `keep_colors` the result takes the nearest color already in that
    /// neighborhood, so no new color appears; indexed images always take a
    /// palette color. Each pixel changes once per stroke.
    Blur {
        source: Arc<Raster>,
        sharpen: bool,
        amount: u8,
        keep_colors: bool,
    },
}

/// The shape a brush dab covers.
#[derive(Clone, Debug, PartialEq)]
pub enum BrushShape {
    Square,
    Circle,
    /// A custom mask, row-major, `width x height`, centered on the dab.
    Custom {
        width: u32,
        height: u32,
        mask: Arc<Vec<bool>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Brush {
    pub shape: BrushShape,
    /// Edge length in pixels for square and circle brushes.
    pub size: u32,
}

impl Default for Brush {
    fn default() -> Self {
        Self::PIXEL
    }
}

impl Brush {
    /// The one-pixel brush.
    pub const PIXEL: Brush = Brush {
        shape: BrushShape::Square,
        size: 1,
    };

    pub fn square(size: u32) -> Self {
        Self {
            shape: BrushShape::Square,
            size: size.max(1),
        }
    }

    pub fn circle(size: u32) -> Self {
        Self {
            shape: BrushShape::Circle,
            size: size.max(1),
        }
    }

    /// A brush from the selected pixels of `selection`.
    pub fn from_selection(selection: &Selection) -> Self {
        let b = selection.bounds();
        let mask = b.points().map(|p| selection.contains(p.x, p.y)).collect();
        Self {
            shape: BrushShape::Custom {
                width: b.w.max(0) as u32,
                height: b.h.max(0) as u32,
                mask: Arc::new(mask),
            },
            size: b.w.max(b.h).max(1) as u32,
        }
    }

    /// Whether this brush covers exactly one pixel.
    pub fn is_pixel(&self) -> bool {
        self.offsets() == [Point::new(0, 0)]
    }

    /// The pixels a dab covers, relative to the dab point. Even sizes extend
    /// one more pixel right and down than left and up.
    pub fn offsets(&self) -> Vec<Point> {
        match &self.shape {
            BrushShape::Square => {
                let s = self.size.max(1) as i32;
                let o = -(s - 1) / 2;
                Rect::new(o, o, s, s).points().collect()
            }
            BrushShape::Circle => {
                let s = self.size.max(1) as i32;
                let o = -(s - 1) / 2;
                shapes::ellipse_filled(Point::new(o, o), Point::new(o + s - 1, o + s - 1))
            }
            BrushShape::Custom {
                width,
                height,
                mask,
            } => {
                let (w, h) = (*width as i32, *height as i32);
                let (ox, oy) = (-(w - 1) / 2, -(h - 1) / 2);
                Rect::new(0, 0, w, h)
                    .points()
                    .filter(|p| mask[(p.y * w + p.x) as usize])
                    .map(|p| Point::new(p.x + ox, p.y + oy))
                    .collect()
            }
        }
    }
}

/// Mirror or rotational symmetry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SymmetryMode {
    #[default]
    None,
    /// Mirrors left and right across a vertical axis.
    Horizontal,
    /// Mirrors top and bottom across a horizontal axis.
    Vertical,
    Both,
    /// N copies rotated evenly around the axes' crossing point.
    Radial(u8),
}

/// Symmetry with movable axes. Axes are in half pixels: `axis_x2 = 2 * x`
/// for an axis at pixel edge `x`, odd values run through pixel centers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Symmetry {
    pub mode: SymmetryMode,
    pub axis_x2: i32,
    pub axis_y2: i32,
}

impl Symmetry {
    pub const NONE: Symmetry = Symmetry {
        mode: SymmetryMode::None,
        axis_x2: 0,
        axis_y2: 0,
    };

    /// Symmetry about the center of a `width x height` canvas.
    pub fn centered(mode: SymmetryMode, width: u32, height: u32) -> Self {
        Self {
            mode,
            axis_x2: width as i32,
            axis_y2: height as i32,
        }
    }

    /// The mirror images of `p`, `p` first. Radial symmetry has no mirrors.
    pub fn mirrors(&self, p: Point) -> impl Iterator<Item = Point> + use<> {
        let mx = Point::new(self.axis_x2 - 1 - p.x, p.y);
        let my = Point::new(p.x, self.axis_y2 - 1 - p.y);
        let mxy = Point::new(self.axis_x2 - 1 - p.x, self.axis_y2 - 1 - p.y);
        let list: [Option<Point>; 4] = match self.mode {
            SymmetryMode::Horizontal => [Some(p), Some(mx), None, None],
            SymmetryMode::Vertical => [Some(p), Some(my), None, None],
            SymmetryMode::Both => [Some(p), Some(mx), Some(my), Some(mxy)],
            SymmetryMode::None | SymmetryMode::Radial(_) => [Some(p), None, None, None],
        };
        list.into_iter().flatten()
    }

    /// How many rotated copies radial symmetry draws (1 otherwise).
    pub fn rotations(&self) -> usize {
        match self.mode {
            SymmetryMode::Radial(n) => n.max(1) as usize,
            _ => 1,
        }
    }

    /// `p` rotated by copy `k` of [`Symmetry::rotations`] around the axes'
    /// crossing, rounded to the nearest pixel.
    pub fn rotate(&self, p: Point, k: usize) -> Point {
        if k == 0 {
            return p;
        }
        let n = self.rotations() as f64;
        let angle = std::f64::consts::TAU * k as f64 / n;
        let (s, c) = angle.sin_cos();
        // Pixel centers: pixel x has its center at x + 0.5 in edge units.
        let (cx, cy) = (self.axis_x2 as f64 / 2.0, self.axis_y2 as f64 / 2.0);
        let (dx, dy) = (p.x as f64 + 0.5 - cx, p.y as f64 + 0.5 - cy);
        let x = cx + dx * c - dy * s - 0.5;
        let y = cy + dx * s + dy * c - 0.5;
        Point::new(x.round() as i32, y.round() as i32)
    }
}

/// Settings every tool shares.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolContext {
    /// Limits writes to these pixels; `None` allows the whole canvas.
    pub selection: Option<Selection>,
    pub symmetry: Symmetry,
}

/// A paintable image with the tool settings applied to every write.
pub struct Canvas<'a> {
    raster: &'a mut Raster,
    palette: &'a Palette,
    transparent_index: u8,
    alpha_locked: bool,
    selection: Option<&'a Selection>,
    symmetry: Symmetry,
    dirty: Rect,
    shaded: HashSet<Point>,
}

impl<'a> Canvas<'a> {
    pub fn new(target: Target<'a>, ctx: &'a ToolContext) -> Self {
        Self {
            raster: target.raster,
            palette: target.palette,
            transparent_index: target.transparent_index,
            alpha_locked: target.alpha_locked,
            selection: ctx.selection.as_ref(),
            symmetry: ctx.symmetry,
            dirty: Rect::EMPTY,
            shaded: HashSet::new(),
        }
    }

    /// The area changed since the canvas was created.
    pub fn dirty(&self) -> Rect {
        self.dirty
    }

    pub fn symmetry(&self) -> Symmetry {
        self.symmetry
    }

    pub fn bounds(&self) -> Rect {
        self.raster.bounds()
    }

    pub fn raster(&self) -> &Raster {
        self.raster
    }

    pub fn palette(&self) -> &Palette {
        self.palette
    }

    pub fn transparent_index(&self) -> u8 {
        self.transparent_index
    }

    /// Whether writes at `(x, y)` are allowed by the selection.
    pub fn selected(&self, x: i32, y: i32) -> bool {
        self.selection.is_none_or(|s| s.contains(x, y))
    }

    /// The selection limiting writes, if any.
    pub fn selection(&self) -> Option<&Selection> {
        self.selection
    }

    /// `color` in this image's pixel format.
    pub fn resolve(&self, color: Color) -> u32 {
        let ti = self.transparent_index;
        match (&*self.raster, color) {
            (Raster::Rgba(_), Color::Rgba(c)) => c.to_u32(),
            (Raster::Rgba(_), Color::Index(i)) => self.palette.get(i).to_u32(),
            (Raster::Indexed(_), Color::Rgba(c)) => {
                if c.a == 0 {
                    ti as u32
                } else {
                    self.palette.nearest(c, Some(ti)) as u32
                }
            }
            (Raster::Indexed(_), Color::Index(i)) => i as u32,
            (Raster::Mask(_), Color::Rgba(c)) => (c.luma() as u32 * c.a as u32 + 127) / 255,
            (Raster::Mask(_), Color::Index(i)) => i as u32,
        }
    }

    /// The raw value of a transparent pixel.
    pub fn transparent_raw(&self) -> u32 {
        match &*self.raster {
            Raster::Rgba(_) | Raster::Mask(_) => 0,
            Raster::Indexed(_) => self.transparent_index as u32,
        }
    }

    /// A raw value as a color.
    pub fn raw_to_rgba(&self, raw: u32) -> Rgba {
        match &*self.raster {
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

    pub fn get_raw(&self, x: i32, y: i32) -> u32 {
        self.raster.get_raw(x, y)
    }

    pub fn get_rgba(&self, x: i32, y: i32) -> Rgba {
        self.raw_to_rgba(self.get_raw(x, y))
    }

    /// Whether two raw values match within `tolerance` (0 is an exact match
    /// of the stored value, so duplicate palette entries stay distinct).
    pub fn matches(&self, a: u32, b: u32, tolerance: u8) -> bool {
        if tolerance == 0 {
            return a == b;
        }
        self.raw_to_rgba(a).distance(self.raw_to_rgba(b)) <= tolerance
    }

    /// Writes a raw value, honoring the selection and the alpha lock.
    pub(crate) fn write(&mut self, x: i32, y: i32, value: u32) {
        if !self.bounds().contains(x, y) || !self.selected(x, y) {
            return;
        }
        let mut value = value;
        if self.alpha_locked {
            let current = self.raster.get_raw(x, y);
            match &*self.raster {
                Raster::Rgba(_) => {
                    let (c, v) = (Rgba::from_u32(current), Rgba::from_u32(value));
                    if c.a == 0 || v.a == 0 {
                        return;
                    }
                    value = Rgba::new(v.r, v.g, v.b, c.a).to_u32();
                }
                Raster::Indexed(_) => {
                    let ti = self.transparent_index as u32;
                    if current == ti || value == ti {
                        return;
                    }
                }
                Raster::Mask(_) => {}
            }
        }
        if self.raster.set_raw(x, y, value) {
            self.dirty.include(x, y);
        }
    }

    /// The value `ink` writes over the pixel at `(x, y)`, or `None` to leave
    /// it.
    pub(crate) fn ink_value(&mut self, ink: &Ink, x: i32, y: i32) -> Option<u32> {
        match ink {
            Ink::Paint(c) => Some(self.resolve(*c)),
            Ink::Erase => Some(self.transparent_raw()),
            Ink::Dither {
                first,
                second,
                matrix,
                amount,
            } => {
                if matrix.picks_first(x, y, *amount) {
                    Some(self.resolve(*first))
                } else {
                    second.map(|c| self.resolve(c))
                }
            }
            Ink::Shade { ramp, step } => {
                if !self.shaded.insert(Point::new(x, y)) {
                    return None;
                }
                let current = self.get_raw(x, y);
                let ramp: Vec<u32> = ramp.iter().map(|c| self.resolve(*c)).collect();
                let i = ramp.iter().position(|v| *v == current)?;
                let j = (i as i64 + *step as i64).clamp(0, ramp.len() as i64 - 1) as usize;
                Some(ramp[j])
            }
            Ink::Replace {
                from,
                to,
                tolerance,
            } => {
                let current = self.get_raw(x, y);
                let from = self.resolve(*from);
                self.matches(current, from, *tolerance)
                    .then(|| self.resolve(*to))
            }
            Ink::Clone { source, offset } => {
                let (sx, sy) = (x + offset.x, y + offset.y);
                source
                    .bounds()
                    .contains(sx, sy)
                    .then(|| source.get_raw(sx, sy))
            }
            Ink::Pattern {
                pattern,
                origin,
                skip,
            } => {
                if pattern.width == 0 || pattern.height == 0 {
                    return None;
                }
                let px = (x - origin.x).rem_euclid(pattern.width as i32);
                let py = (y - origin.y).rem_euclid(pattern.height as i32);
                let v = pattern.get(px, py);
                (Some(v) != *skip).then_some(v)
            }
            Ink::Blur {
                source,
                sharpen,
                amount,
                keep_colors,
            } => {
                if !self.shaded.insert(Point::new(x, y)) {
                    return None;
                }
                self.blurred(source, x, y, *sharpen, *amount, *keep_colors)
            }
        }
    }

    /// The blur ink's value at `(x, y)`.
    fn blurred(
        &self,
        source: &Raster,
        x: i32,
        y: i32,
        sharpen: bool,
        amount: u8,
        keep_colors: bool,
    ) -> Option<u32> {
        let b = source.bounds();
        if !b.contains(x, y) {
            return None;
        }
        let here_raw = source.get_raw(x, y);
        let here = self.raw_to_rgba(here_raw);
        // The alpha-weighted mean of the 3x3 neighborhood.
        let (mut r, mut g, mut bl, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
        let mut near: Vec<u32> = Vec::with_capacity(9);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if !b.contains(nx, ny) {
                    continue;
                }
                let raw = source.get_raw(nx, ny);
                if !near.contains(&raw) {
                    near.push(raw);
                }
                let c = self.raw_to_rgba(raw);
                let w = c.a as u32;
                r += c.r as u32 * w;
                g += c.g as u32 * w;
                bl += c.b as u32 * w;
                a += w;
                n += 1;
            }
        }
        let mean = match (r + a / 2).checked_div(a) {
            None => Rgba::TRANSPARENT,
            Some(mr) => Rgba::new(
                mr as u8,
                ((g + a / 2) / a) as u8,
                ((bl + a / 2) / a) as u8,
                ((a + n / 2) / n) as u8,
            ),
        };
        let k = amount as f32 / 255.0;
        let mix = |p: u8, m: u8| {
            let (p, m) = (p as f32, m as f32);
            let v = if sharpen {
                p + (p - m) * k
            } else {
                p + (m - p) * k
            };
            v.round().clamp(0.0, 255.0) as u8
        };
        let want = Rgba::new(
            mix(here.r, mean.r),
            mix(here.g, mean.g),
            mix(here.b, mean.b),
            mix(here.a, mean.a),
        );
        if want == here {
            return None;
        }
        let value = match &*self.raster {
            Raster::Indexed(_) => self.resolve(Color::Rgba(want)),
            _ if keep_colors => *near
                .iter()
                .min_by_key(|v| {
                    let c = self.raw_to_rgba(**v);
                    let d = |p: u8, q: u8| (p as i32 - q as i32).pow(2);
                    d(c.r, want.r) + d(c.g, want.g) + d(c.b, want.b) + d(c.a, want.a)
                })
                .expect("the pixel itself is near"),
            _ => self.resolve(Color::Rgba(want)),
        };
        (value != here_raw).then_some(value)
    }

    /// Applies `ink` at one pixel and at its mirror images.
    pub fn plot(&mut self, p: Point, ink: &Ink) {
        for q in self.symmetry.mirrors(p) {
            if let Some(v) = self.ink_value(ink, q.x, q.y) {
                self.write(q.x, q.y, v);
            }
        }
    }

    /// Applies `ink` under a brush dab at `p`, with mirror images.
    pub fn stamp(&mut self, p: Point, brush: &Brush, ink: &Ink) {
        if brush.is_pixel() {
            self.plot(p, ink);
            return;
        }
        for o in brush.offsets() {
            self.plot(Point::new(p.x + o.x, p.y + o.y), ink);
        }
    }

    /// Applies `ink` to every pixel of `pixels` with brush dabs, adding the
    /// rotated copies radial symmetry asks for (rotated pixel by pixel).
    pub fn stamp_all(&mut self, pixels: &[Point], brush: &Brush, ink: &Ink) {
        for k in 0..self.symmetry.rotations() {
            for &p in pixels {
                let q = self.symmetry.rotate(p, k);
                self.stamp(q, brush, ink);
            }
        }
    }
}

/// Runs a tool on one cel as a single undo step and returns its result and
/// the area it changed.
pub fn edit_cel<R>(
    doc: &mut Document,
    history: &mut History,
    label: &str,
    layer: LayerId,
    frame: usize,
    ctx: &ToolContext,
    tool: impl FnOnce(&mut Canvas) -> R,
) -> Result<(R, Rect), Error> {
    if doc.effective_locks(layer)?.pixels {
        return Err(Error::Locked(layer));
    }
    // Begin before the cel exists so undo also removes a cel the edit created.
    let mut tx = history.begin(doc, label, Scope::Images(Vec::new()));
    let image = doc.target_image(layer, frame)?;
    tx.watch(doc, image);
    let target = doc.target(layer, frame)?;
    let mut canvas = Canvas::new(target, ctx);
    let out = tool(&mut canvas);
    let dirty = canvas.dirty();
    history.commit(doc, tx);
    Ok((out, dirty))
}

/// Runs a tool on a layer's mask as a single undo step and returns its
/// result and the area it changed. Painting reads each color's lightness:
/// white shows the layer, black hides it.
pub fn edit_mask<R>(
    doc: &mut Document,
    history: &mut History,
    label: &str,
    layer: LayerId,
    ctx: &ToolContext,
    tool: impl FnOnce(&mut Canvas) -> R,
) -> Result<(R, Rect), Error> {
    let image = doc.mask_image(layer)?;
    let mut tx = history.begin(doc, label, Scope::Images(vec![image]));
    tx.watch(doc, image);
    let target = doc.mask_target(layer)?;
    let mut canvas = Canvas::new(target, ctx);
    let out = tool(&mut canvas);
    let dirty = canvas.dirty();
    history.commit(doc, tx);
    Ok((out, dirty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_document::color::ColorMode;

    #[test]
    fn brush_shapes() {
        assert_eq!(Brush::square(3).offsets().len(), 9);
        assert_eq!(Brush::square(2).offsets()[0], Point::new(0, 0));
        assert_eq!(Brush::circle(3).offsets().len(), 5);
        assert_eq!(Brush::circle(1).offsets(), vec![Point::new(0, 0)]);
        let c5 = Brush::circle(5).offsets();
        assert!(c5.contains(&Point::new(-2, 0)) && !c5.contains(&Point::new(-2, -2)));
        let sel = Selection::from_points(10, 10, [Point::new(3, 3), Point::new(5, 3)]);
        let b = Brush::from_selection(&sel);
        assert_eq!(b.offsets(), vec![Point::new(-1, 0), Point::new(1, 0)]);
    }

    #[test]
    fn symmetry_mirrors_and_rotates() {
        let s = Symmetry::centered(SymmetryMode::Both, 10, 8);
        let m: Vec<Point> = s.mirrors(Point::new(1, 2)).collect();
        assert_eq!(
            m,
            vec![
                Point::new(1, 2),
                Point::new(8, 2),
                Point::new(1, 5),
                Point::new(8, 5)
            ]
        );
        let odd = Symmetry {
            mode: SymmetryMode::Horizontal,
            axis_x2: 5,
            axis_y2: 0,
        };
        assert_eq!(odd.mirrors(Point::new(2, 0)).nth(1), Some(Point::new(2, 0)));
        let r = Symmetry::centered(SymmetryMode::Radial(4), 10, 10);
        assert_eq!(r.rotations(), 4);
        assert_eq!(r.rotate(Point::new(9, 4), 1), Point::new(5, 9));
        assert_eq!(r.rotate(Point::new(9, 4), 2), Point::new(0, 5));
    }

    fn rgba_doc() -> (Document, LayerId) {
        let d = Document::new(16, 16, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        (d, l)
    }

    #[test]
    fn writes_honor_selection_and_alpha_lock() {
        let (mut d, l) = rgba_doc();
        let mut h = History::default();
        let ctx = ToolContext {
            selection: Some(Selection::rect(16, 16, Rect::new(0, 0, 4, 16))),
            ..Default::default()
        };
        let ink = Ink::Paint(Rgba::WHITE.into());
        let (_, dirty) = edit_cel(&mut d, &mut h, "dot", l, 0, &ctx, |c| {
            c.plot(Point::new(2, 2), &ink);
            c.plot(Point::new(6, 2), &ink);
        })
        .unwrap();
        assert_eq!(dirty, Rect::pixel(2, 2));
        d.update_layer(l, |p| p.locks.alpha = true).unwrap();
        let red = Ink::Paint(Rgba::new(255, 0, 0, 255).into());
        let free = ToolContext::default();
        edit_cel(&mut d, &mut h, "dot", l, 0, &free, |c| {
            c.plot(Point::new(2, 2), &red);
            c.plot(Point::new(3, 3), &red);
            c.plot(Point::new(2, 2), &Ink::Erase);
        })
        .unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.get_raw(2, 2), Rgba::opaque(255, 0, 0).to_u32());
        assert_eq!(r.get_raw(3, 3), 0, "alpha lock skips empty pixels");
        d.update_layer(l, |p| p.locks.pixels = true).unwrap();
        assert!(edit_cel(&mut d, &mut h, "x", l, 0, &free, |_| ()).is_err());
    }

    #[test]
    fn edit_cel_is_one_undo_step_including_the_new_cel() {
        let (mut d, l) = rgba_doc();
        let before = d.clone();
        let mut h = History::default();
        let ink = Ink::Paint(Rgba::WHITE.into());
        edit_cel(&mut d, &mut h, "dot", l, 0, &ToolContext::default(), |c| {
            c.plot(Point::new(1, 1), &ink)
        })
        .unwrap();
        assert!(d.cel(l, 0).unwrap().is_some());
        h.undo(&mut d);
        assert_eq!(d, before);
        h.redo(&mut d);
        assert_eq!(
            d.cel_raster(l, 0).unwrap().unwrap().get_raw(1, 1),
            Rgba::WHITE.to_u32()
        );
    }

    #[test]
    fn inks() {
        let mut d = Document::new(8, 8, ColorMode::Indexed).unwrap();
        let l = d.layers()[0].id;
        let mut h = History::default();
        let ctx = ToolContext::default();
        let ramp = vec![Color::Index(1), Color::Index(3), Color::Index(2)];
        edit_cel(&mut d, &mut h, "inks", l, 0, &ctx, |c| {
            c.plot(Point::new(0, 0), &Ink::Paint(Color::Index(3)));
            c.plot(Point::new(1, 0), &Ink::Paint(Color::Index(5)));
            let shade = Ink::Shade {
                ramp: ramp.clone(),
                step: 1,
            };
            c.plot(Point::new(0, 0), &shade);
            c.plot(Point::new(0, 0), &shade);
            c.plot(Point::new(1, 0), &shade);
            c.plot(
                Point::new(2, 0),
                &Ink::Paint(Rgba::opaque(250, 250, 250).into()),
            );
            let dither = Ink::Dither {
                first: Color::Index(1),
                second: Some(Color::Index(2)),
                matrix: Bayer::B2,
                amount: 128,
            };
            for x in 0..4 {
                c.plot(Point::new(x, 4), &dither);
            }
            c.plot(
                Point::new(1, 0),
                &Ink::Replace {
                    from: Color::Index(5),
                    to: Color::Index(4),
                    tolerance: 0,
                },
            );
        })
        .unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.get_raw(0, 0), 2, "shaded one step, once per stroke");
        assert_eq!(r.get_raw(1, 0), 4, "off the ramp stays, then replaced");
        assert_eq!(r.get_raw(2, 0), 2, "rgba resolves to the nearest entry");
        let row: Vec<u32> = (0..4).map(|x| r.get_raw(x, 4)).collect();
        assert_eq!(row, vec![1, 2, 1, 2]);
    }
}
