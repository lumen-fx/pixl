//! Flips, quarter turns, nearest-neighbor scaling and RotSprite rotation.
//!
//! Transforms work on [`Pixels`], a dense buffer of raw pixel values, so the
//! same code serves RGBA, indexed and mask images: none of them ever blends
//! two pixels into a new value.

use crate::document::{Document, LayerId};
use crate::error::Error;
use crate::geom::{Point, Rect};
use crate::image::Raster;
use crate::rig::{Region, Vec2};

/// A dense row-major buffer of raw pixel values (see [`Raster::get_raw`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u32>,
}

impl Pixels {
    pub fn new(width: u32, height: u32, fill: u32) -> Self {
        Self {
            width,
            height,
            data: vec![fill; width as usize * height as usize],
        }
    }

    /// The pixels of `rect` (outside the raster reads as its fill value).
    pub fn from_raster(raster: &Raster, rect: Rect) -> Self {
        let mut out = Self::new(rect.w.max(0) as u32, rect.h.max(0) as u32, 0);
        for (i, p) in rect.points().enumerate() {
            out.data[i] = raster.get_raw(p.x, p.y);
        }
        out
    }

    pub fn get(&self, x: i32, y: i32) -> u32 {
        self.data[(y as u32 * self.width + x as u32) as usize]
    }

    fn get_clamped(&self, x: i32, y: i32) -> u32 {
        let x = x.clamp(0, self.width as i32 - 1);
        let y = y.clamp(0, self.height as i32 - 1);
        self.get(x, y)
    }

    fn from_fn(width: u32, height: u32, mut f: impl FnMut(i32, i32) -> u32) -> Self {
        let mut data = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                data.push(f(x, y));
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// Writes every pixel into `raster` with the top-left at `at`, skipping
    /// pixels equal to `skip` when given. Returns the changed area.
    pub fn write_to(&self, raster: &mut Raster, at: Point, skip: Option<u32>) -> Rect {
        let mut dirty = Rect::EMPTY;
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let v = self.get(x, y);
                if Some(v) == skip {
                    continue;
                }
                if raster.set_raw(at.x + x, at.y + y, v) {
                    dirty.include(at.x + x, at.y + y);
                }
            }
        }
        dirty
    }

    /// Mirrored left to right.
    pub fn flip_h(&self) -> Self {
        let w = self.width as i32;
        Self::from_fn(self.width, self.height, |x, y| self.get(w - 1 - x, y))
    }

    /// Mirrored top to bottom.
    pub fn flip_v(&self) -> Self {
        let h = self.height as i32;
        Self::from_fn(self.width, self.height, |x, y| self.get(x, h - 1 - y))
    }

    /// Turned `quarters` times 90 degrees clockwise.
    pub fn rotate_quarters(&self, quarters: i32) -> Self {
        let (w, h) = (self.width as i32, self.height as i32);
        match quarters.rem_euclid(4) {
            0 => self.clone(),
            1 => Self::from_fn(self.height, self.width, |x, y| self.get(y, h - 1 - x)),
            2 => Self::from_fn(self.width, self.height, |x, y| {
                self.get(w - 1 - x, h - 1 - y)
            }),
            _ => Self::from_fn(self.height, self.width, |x, y| self.get(w - 1 - y, x)),
        }
    }

    /// Nearest-neighbor resize to `width x height`; integer factors repeat
    /// each pixel exactly.
    pub fn scale_nearest(&self, width: u32, height: u32) -> Self {
        if self.width == 0 || self.height == 0 {
            return Self::new(width, height, 0);
        }
        Self::from_fn(width, height, |x, y| {
            let sx = (x as u64 * self.width as u64 / width as u64) as i32;
            let sy = (y as u64 * self.height as u64 / height as u64) as i32;
            self.get(sx, sy)
        })
    }

    /// Scale2x (Andrea Mazzoleni's EPX variant): doubles the size and rounds
    /// diagonal edges by copying a neighbor where two neighbors agree, without
    /// creating new colors. Edges repeat their outermost pixels.
    pub fn scale2x(&self) -> Self {
        let mut out = Self::new(self.width * 2, self.height * 2, 0);
        let ow = out.width as usize;
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let p = self.get(x, y);
                let a = self.get_clamped(x, y - 1);
                let b = self.get_clamped(x + 1, y);
                let c = self.get_clamped(x - 1, y);
                let d = self.get_clamped(x, y + 1);
                let e0 = if c == a && c != d && a != b { a } else { p };
                let e1 = if a == b && a != c && b != d { b } else { p };
                let e2 = if d == c && d != b && c != a { c } else { p };
                let e3 = if b == d && b != a && d != c { d } else { p };
                let (ox, oy) = (2 * x as usize, 2 * y as usize);
                out.data[oy * ow + ox] = e0;
                out.data[oy * ow + ox + 1] = e1;
                out.data[(oy + 1) * ow + ox] = e2;
                out.data[(oy + 1) * ow + ox + 1] = e3;
            }
        }
        out
    }

    /// The 8x upscale RotSprite samples: three passes of
    /// [`Pixels::scale2x`].
    pub fn upscale8(&self) -> Self {
        self.scale2x().scale2x().scale2x()
    }

    /// The size a rotation by `degrees` needs to hold the whole image.
    pub fn rotated_size(&self, degrees: f64) -> (u32, u32) {
        let (s, c) = degrees.to_radians().sin_cos();
        let (w, h) = (self.width as f64, self.height as f64);
        let rw = (w * c.abs() + h * s.abs() - 1e-9).ceil().max(1.0) as u32;
        let rh = (w * s.abs() + h * c.abs() - 1e-9).ceil().max(1.0) as u32;
        (rw, rh)
    }

    /// Rotates by `degrees` clockwise about the center into a
    /// `width x height` buffer centered on the same point, filling uncovered
    /// pixels with `fill`.
    ///
    /// This is RotSprite, Xenowhirl's pixel art rotation (2007, first
    /// published on the Sonic Retro forums), in the form Aseprite implements
    /// it (`src/doc/algorithm/rotsprite.cpp`): the sprite is upscaled 8x with
    /// three passes of [`Pixels::scale2x`], which rounds its diagonal edges
    /// without inventing colors, and each output pixel then takes the
    /// upscaled pixel under its rotated center (nearest neighbor). Sampling
    /// the smoothed 8x image instead of the original keeps single-pixel lines
    /// connected and outlines clean where plain nearest-neighbor rotation
    /// breaks them. Quarter turns skip the upscale and are exact.
    pub fn rotsprite(&self, degrees: f64, width: u32, height: u32, fill: u32) -> Self {
        let turns = degrees / 90.0;
        if (turns - turns.round()).abs() < 1e-9 {
            let q = self.rotate_quarters(turns.round() as i32);
            let ox = (width as i32 - q.width as i32).div_euclid(2);
            let oy = (height as i32 - q.height as i32).div_euclid(2);
            return Self::from_fn(width, height, |x, y| {
                let (sx, sy) = (x - ox, y - oy);
                if sx >= 0 && sy >= 0 && sx < q.width as i32 && sy < q.height as i32 {
                    q.get(sx, sy)
                } else {
                    fill
                }
            });
        }
        let big = self.upscale8();
        let (s, c) = degrees.to_radians().sin_cos();
        let (scx, scy) = (self.width as f64 / 2.0, self.height as f64 / 2.0);
        let (dcx, dcy) = (width as f64 / 2.0, height as f64 / 2.0);
        Self::from_fn(width, height, |x, y| {
            let (px, py) = (x as f64 + 0.5 - dcx, y as f64 + 0.5 - dcy);
            // Inverse rotation maps the output center back into the source.
            let sx = px * c + py * s + scx;
            let sy = -px * s + py * c + scy;
            let (bx, by) = ((sx * 8.0).floor(), (sy * 8.0).floor());
            if bx < 0.0 || by < 0.0 || bx >= big.width as f64 || by >= big.height as f64 {
                fill
            } else {
                big.get(bx as i32, by as i32)
            }
        })
    }
}

/// A transform of the pixels in a rectangle of one cel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CelTransform {
    FlipH,
    FlipV,
    /// Quarter turns clockwise.
    Rotate90(i32),
    /// RotSprite rotation, degrees clockwise.
    Rotate(f64),
    /// Nearest-neighbor resize to this size.
    Scale(u32, u32),
}

/// Lifts the pixels of `rect` from a cel, transforms them and puts them back
/// centered on the same point; the lifted area is cleared first and
/// transparent results do not overwrite. Fails if the layer's pixels or
/// position are locked. Returns the changed area.
pub fn transform_cel(
    doc: &mut Document,
    layer: LayerId,
    frame: usize,
    rect: Rect,
    op: CelTransform,
) -> Result<Rect, Error> {
    if doc.effective_locks(layer)?.position {
        return Err(Error::Locked(layer));
    }
    let rect = rect.intersect(&doc.bounds());
    let target = doc.target(layer, frame)?;
    let raster = target.raster;
    let clear = match raster {
        Raster::Rgba(_) => 0,
        Raster::Indexed(_) => target.transparent_index as u32,
        Raster::Mask(_) => 0,
    };
    if rect.is_empty() {
        return Ok(Rect::EMPTY);
    }
    let src = Pixels::from_raster(raster, rect);
    let out = match op {
        CelTransform::FlipH => src.flip_h(),
        CelTransform::FlipV => src.flip_v(),
        CelTransform::Rotate90(q) => src.rotate_quarters(q),
        CelTransform::Rotate(deg) => {
            let (w, h) = src.rotated_size(deg);
            src.rotsprite(deg, w, h, clear)
        }
        CelTransform::Scale(w, h) => src.scale_nearest(w, h),
    };
    let mut dirty = raster.fill_rect_raw(&rect, clear);
    let at = Point::new(
        rect.x + (rect.w - out.width as i32).div_euclid(2),
        rect.y + (rect.h - out.height as i32).div_euclid(2),
    );
    dirty = dirty.union(&out.write_to(raster, at, Some(clear)));
    Ok(dirty)
}

/// A canvas-wide transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasTransform {
    FlipH,
    FlipV,
    /// Quarter turns clockwise; odd turns swap width and height.
    Rotate90(i32),
    /// Nearest-neighbor resize of every image.
    Scale(u32, u32),
}

/// Applies a transform to every image of the document and moves slices to
/// match. Run it inside a [`crate::History`] transaction with
/// [`crate::Scope::All`] to make it undoable.
pub fn transform_canvas(doc: &mut Document, op: CanvasTransform) -> Result<Rect, Error> {
    let (w, h) = (doc.width(), doc.height());
    let (nw, nh) = match op {
        CanvasTransform::FlipH | CanvasTransform::FlipV => (w, h),
        CanvasTransform::Rotate90(q) if q.rem_euclid(2) == 1 => (h, w),
        CanvasTransform::Rotate90(_) => (w, h),
        CanvasTransform::Scale(sw, sh) => (sw, sh),
    };
    if nw == 0 || nh == 0 || nw > crate::document::MAX_SIZE || nh > crate::document::MAX_SIZE {
        return Err(Error::InvalidSize {
            width: nw,
            height: nh,
        });
    }
    let map_rect = |r: Rect| -> Rect {
        let (w, h) = (w as i32, h as i32);
        match op {
            CanvasTransform::FlipH => Rect::new(w - r.right(), r.y, r.w, r.h),
            CanvasTransform::FlipV => Rect::new(r.x, h - r.bottom(), r.w, r.h),
            CanvasTransform::Rotate90(q) => match q.rem_euclid(4) {
                0 => r,
                1 => Rect::new(h - r.bottom(), r.x, r.h, r.w),
                2 => Rect::new(w - r.right(), h - r.bottom(), r.w, r.h),
                _ => Rect::new(r.y, w - r.right(), r.h, r.w),
            },
            CanvasTransform::Scale(sw, sh) => {
                let sx = |v: i32| (v as i64 * sw as i64 / w as i64) as i32;
                let sy = |v: i32| (v as i64 * sh as i64 / h as i64) as i32;
                Rect::new(
                    sx(r.x),
                    sy(r.y),
                    sx(r.right()) - sx(r.x),
                    sy(r.bottom()) - sy(r.y),
                )
            }
        }
    };
    for raster in doc.images.values_mut() {
        let px = Pixels::from_raster(raster, raster.bounds());
        let out = match op {
            CanvasTransform::FlipH => px.flip_h(),
            CanvasTransform::FlipV => px.flip_v(),
            CanvasTransform::Rotate90(q) => px.rotate_quarters(q),
            CanvasTransform::Scale(sw, sh) => px.scale_nearest(sw, sh),
        };
        let mut fresh = match raster {
            Raster::Rgba(i) => Raster::Rgba(crate::image::TiledImage::new(nw, nh, i.fill())),
            Raster::Indexed(i) => Raster::Indexed(crate::image::TiledImage::new(nw, nh, i.fill())),
            Raster::Mask(i) => Raster::Mask(crate::image::TiledImage::new(nw, nh, i.fill())),
        };
        let fill = fresh.fill_raw();
        out.write_to(&mut fresh, Point::new(0, 0), Some(fill));
        *raster = fresh;
    }
    for slice in doc.s.slices.iter_mut() {
        let old = slice.bounds;
        slice.bounds = map_rect(old);
        // Center and pivot are relative to the bounds: map them in canvas
        // space and take them back to the new bounds' origin.
        if let Some(c) = slice.center {
            let abs = map_rect(Rect::new(old.x + c.x, old.y + c.y, c.w, c.h));
            slice.center = Some(Rect::new(
                abs.x - slice.bounds.x,
                abs.y - slice.bounds.y,
                abs.w,
                abs.h,
            ));
        }
        if let Some(p) = slice.pivot {
            let abs = map_rect(Rect::pixel(old.x + p.x, old.y + p.y));
            slice.pivot = Some(Point::new(abs.x - slice.bounds.x, abs.y - slice.bounds.y));
        }
    }
    let (fw, fh) = (w as f64, h as f64);
    let point = |p: Vec2| -> Vec2 {
        match op {
            CanvasTransform::FlipH => Vec2::new(fw - p.x, p.y),
            CanvasTransform::FlipV => Vec2::new(p.x, fh - p.y),
            CanvasTransform::Rotate90(q) => match q.rem_euclid(4) {
                0 => p,
                1 => Vec2::new(fh - p.y, p.x),
                2 => Vec2::new(fw - p.x, fh - p.y),
                _ => Vec2::new(p.y, fw - p.x),
            },
            CanvasTransform::Scale(sw, sh) => Vec2::new(p.x * sw as f64 / fw, p.y * sh as f64 / fh),
        }
    };
    let region = |r: &Region| -> Region {
        let b = r.bounds();
        let bits = Pixels {
            width: b.w.max(0) as u32,
            height: b.h.max(0) as u32,
            data: r.bits().iter().map(|&on| on as u32).collect(),
        };
        let to = map_rect(b);
        let out = match op {
            CanvasTransform::FlipH => bits.flip_h(),
            CanvasTransform::FlipV => bits.flip_v(),
            CanvasTransform::Rotate90(q) => bits.rotate_quarters(q),
            CanvasTransform::Scale(..) => {
                bits.scale_nearest(to.w.max(0) as u32, to.h.max(0) as u32)
            }
        };
        Region::from_bits(to, out.data.iter().map(|&v| v != 0).collect())
            .unwrap_or_else(|| Region::rect(Rect::EMPTY))
    };
    let (flip, turn) = match op {
        CanvasTransform::FlipH | CanvasTransform::FlipV => (true, 0.0),
        CanvasTransform::Rotate90(q) => (false, q as f64 * 90.0),
        CanvasTransform::Scale(..) => (false, 0.0),
    };
    for rig in doc.rigs_mut() {
        rig.map(point, flip, turn, region);
    }
    doc.s.width = nw;
    doc.s.height = nh;
    Ok(doc.bounds())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{ColorMode, Rgba};
    use crate::document::Slice;
    use crate::history::{History, Scope};

    fn grid(w: u32, h: u32) -> Pixels {
        Pixels::from_fn(w, h, |x, y| (y * w as i32 + x) as u32 + 1)
    }

    #[test]
    fn flips_and_quarter_turns() {
        let p = grid(3, 2); // 1 2 3 / 4 5 6
        assert_eq!(p.flip_h().data, vec![3, 2, 1, 6, 5, 4]);
        assert_eq!(p.flip_v().data, vec![4, 5, 6, 1, 2, 3]);
        let cw = p.rotate_quarters(1);
        assert_eq!((cw.width, cw.height), (2, 3));
        assert_eq!(cw.data, vec![4, 1, 5, 2, 6, 3]);
        assert_eq!(p.rotate_quarters(-1), p.rotate_quarters(3));
        assert_eq!(p.rotate_quarters(4), p);
        assert_eq!(cw.rotate_quarters(3), p);
        assert_eq!(p.rotate_quarters(2).data, vec![6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn nearest_scaling() {
        let p = grid(2, 2);
        let s = p.scale_nearest(4, 4);
        assert_eq!(s.data, vec![1, 1, 2, 2, 1, 1, 2, 2, 3, 3, 4, 4, 3, 3, 4, 4]);
        assert_eq!(s.scale_nearest(2, 2), p);
        assert_eq!(p.scale_nearest(3, 1).data, vec![1, 1, 2]);
    }

    #[test]
    fn scale2x_rounds_diagonals() {
        // A diagonal step:
        // 1 0
        // 1 1
        let p = Pixels {
            width: 2,
            height: 2,
            data: vec![1, 0, 1, 1],
        };
        let s = p.scale2x();
        // The 0 pixel's bottom-left quarter fills in from its agreeing
        // neighbors (left and down are both 1).
        assert_eq!(s.get(2, 1), 1);
        assert_eq!(s.get(3, 0), 0);
        let flat = Pixels::new(3, 3, 7);
        assert_eq!(flat.scale2x(), Pixels::new(6, 6, 7));
    }

    #[test]
    fn rotsprite_quarter_turns_are_exact() {
        let p = grid(4, 3);
        let r = p.rotsprite(90.0, 3, 4, 0);
        assert_eq!(r, p.rotate_quarters(1));
        assert_eq!(p.rotsprite(360.0, 4, 3, 0), p);
    }

    #[test]
    fn rotsprite_keeps_colors_and_lines() {
        // A 1-pixel horizontal line, rotated 45 degrees.
        let mut p = Pixels::new(16, 16, 0);
        for x in 2..14 {
            p.data[8 * 16 + x] = 5;
        }
        p.data[8 * 16 + 2] = 9;
        let (w, h) = p.rotated_size(45.0);
        assert_eq!((w, h), (23, 23));
        let r = p.rotsprite(45.0, w, h, 0);
        assert!(
            r.data.iter().all(|v| [0, 5, 9].contains(v)),
            "no new colors"
        );
        let lit: Vec<(i32, i32)> = (0..h as i32)
            .flat_map(|y| (0..w as i32).map(move |x| (x, y)))
            .filter(|&(x, y)| r.get(x, y) != 0)
            .collect();
        assert!(lit.len() >= 8, "{}", lit.len());
        // Every lit pixel touches another (8-connected): the line stays whole.
        for &(x, y) in &lit {
            assert!(
                lit.iter()
                    .any(|&(a, b)| (a, b) != (x, y) && (a - x).abs() <= 1 && (b - y).abs() <= 1)
            );
        }
        // The line runs along the diagonal.
        for &(x, y) in &lit {
            assert!((x - y).abs() <= 2, "({x}, {y})");
        }
    }

    #[test]
    fn rotsprite_square_area_is_kept() {
        let mut p = Pixels::new(20, 20, 0);
        for y in 5..15 {
            for x in 5..15 {
                p.data[y * 20 + x] = 1;
            }
        }
        let r = p.rotsprite(30.0, 20, 20, 0);
        let n = r.data.iter().filter(|v| **v == 1).count();
        assert!((90..=110).contains(&n), "{n}");
    }

    #[test]
    fn cel_transforms() {
        let mut d = Document::new(8, 8, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let red = Rgba::opaque(255, 0, 0).to_u32();
        d.target(l, 0).unwrap().raster.set_raw(2, 2, red);
        let dirty =
            transform_cel(&mut d, l, 0, Rect::new(2, 2, 4, 1), CelTransform::FlipH).unwrap();
        assert_eq!(dirty, Rect::new(2, 2, 4, 1));
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.get_raw(5, 2), red);
        assert_eq!(r.get_raw(2, 2), 0);
        transform_cel(
            &mut d,
            l,
            0,
            Rect::new(2, 0, 4, 4),
            CelTransform::Rotate90(1),
        )
        .unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.content_bounds(), Rect::pixel(3, 3));
        d.update_layer(l, |p| p.locks.position = true).unwrap();
        assert!(transform_cel(&mut d, l, 0, Rect::new(0, 0, 8, 8), CelTransform::FlipV).is_err());
    }

    #[test]
    fn canvas_rotation_moves_pixels_slices_and_undoes() {
        let mut d = Document::new(6, 4, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let red = Rgba::opaque(255, 0, 0).to_u32();
        d.target(l, 0).unwrap().raster.set_raw(0, 0, red);
        d.slices_mut().push(Slice {
            name: "s".into(),
            bounds: Rect::new(0, 0, 2, 1),
            center: None,
            pivot: Some(Point::new(1, 0)),
            color: Rgba::BLACK,
        });
        let before = d.clone();
        let mut h = History::default();
        h.transact(&mut d, "rotate", Scope::All, |d| {
            transform_canvas(d, CanvasTransform::Rotate90(1)).unwrap()
        });
        assert_eq!((d.width(), d.height()), (4, 6));
        assert_eq!(d.cel_raster(l, 0).unwrap().unwrap().get_raw(3, 0), red);
        assert_eq!(d.slices()[0].bounds, Rect::new(3, 0, 1, 2));
        assert_eq!(d.slices()[0].pivot, Some(Point::new(0, 1)));
        h.undo(&mut d);
        assert_eq!(d, before);
        transform_canvas(&mut d, CanvasTransform::Scale(12, 8)).unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.content_bounds(), Rect::new(0, 0, 2, 2));
        assert!(transform_canvas(&mut d, CanvasTransform::Scale(0, 8)).is_err());
    }
}
