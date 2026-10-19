//! Flips, quarter turns, nearest-neighbor scaling and RotSprite rotation.
//!
//! Transforms work on [`Pixels`], a dense buffer of raw pixel values, so the
//! same code serves RGBA, indexed and mask images: none of them ever blends
//! two pixels into a new value.

use crate::document::{Document, LayerId};
use crate::error::Error;
use crate::geom::{Point, Rect};
use crate::image::Raster;
use crate::reference::ReferencePlace;
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
        let (scx, scy) = (self.width as f64 / 2.0, self.height as f64 / 2.0);
        let (dcx, dcy) = (width as f64 / 2.0, height as f64 / 2.0);
        let m = Affine::translate(-scx, -scy)
            .then(&Affine::rotate(degrees))
            .then(&Affine::translate(dcx, dcy));
        self.affine(
            &m,
            Rect::new(0, 0, width as i32, height as i32),
            fill,
            Resample::RotSprite,
        )
    }
}

/// How a transform picks each output pixel. None of them blends pixels:
/// every output pixel is one of the source's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Resample {
    /// The source pixel under the output pixel's center.
    #[default]
    Nearest,
    /// RotSprite: sample an 8x Scale2x enlargement, which rounds diagonal
    /// steps first (see [`Pixels::rotsprite`]).
    RotSprite,
    /// Cuts the corners of diagonal steps at the sample point, in the
    /// manner of torcado's cleanEdge: where two neighbors that agree with
    /// each other (and not with the pixel) meet at a corner, the part of
    /// the pixel beyond the line between the midpoints of its two edges
    /// takes their color. It is Scale2x's rule evaluated at any resolution,
    /// so it rounds 45 degree steps without the 8x enlargement.
    CleanEdge,
}

/// An affine map of the plane: `(x, y)` goes to `(a x + c y + e,
/// b x + d y + f)`. Pixel coordinates run right and down, so a positive
/// rotation turns clockwise on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(x: f64, y: f64) -> Self {
        Self {
            e: x,
            f: y,
            ..Self::IDENTITY
        }
    }

    /// A scale; a negative factor flips that axis.
    pub fn scale(x: f64, y: f64) -> Self {
        Self {
            a: x,
            d: y,
            ..Self::IDENTITY
        }
    }

    /// A clockwise rotation by `degrees`.
    pub fn rotate(degrees: f64) -> Self {
        let (s, c) = degrees.to_radians().sin_cos();
        Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            ..Self::IDENTITY
        }
    }

    /// A shear: `x` moves by `tan(x_degrees) * y` and `y` by
    /// `tan(y_degrees) * x`.
    pub fn skew(x_degrees: f64, y_degrees: f64) -> Self {
        Self {
            c: x_degrees.to_radians().tan(),
            b: y_degrees.to_radians().tan(),
            ..Self::IDENTITY
        }
    }

    /// This map, then `next`.
    pub fn then(&self, next: &Affine) -> Affine {
        Affine {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Affine {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        })
    }

    /// A free transform about `pivot`: scale (negative flips), then skew,
    /// then rotate, all about the pivot, then move by `offset`.
    pub fn about(
        pivot: (f64, f64),
        scale: (f64, f64),
        skew_degrees: (f64, f64),
        rotate_degrees: f64,
        offset: (f64, f64),
    ) -> Affine {
        Affine::translate(-pivot.0, -pivot.1)
            .then(&Affine::scale(scale.0, scale.1))
            .then(&Affine::skew(skew_degrees.0, skew_degrees.1))
            .then(&Affine::rotate(rotate_degrees))
            .then(&Affine::translate(pivot.0 + offset.0, pivot.1 + offset.1))
    }

    /// The pixels that hold the image of `rect` under this map.
    pub fn bounds_of(&self, rect: Rect) -> Rect {
        let corners = [
            (rect.x as f64, rect.y as f64),
            (rect.right() as f64, rect.y as f64),
            (rect.x as f64, rect.bottom() as f64),
            (rect.right() as f64, rect.bottom() as f64),
        ]
        .map(|(x, y)| self.apply(x, y));
        let x0 = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        let y0 = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
        let x1 = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let y1 = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let (x0, y0) = ((x0 + 1e-6).floor() as i32, (y0 + 1e-6).floor() as i32);
        let (x1, y1) = ((x1 - 1e-6).ceil() as i32, (y1 - 1e-6).ceil() as i32);
        Rect::new(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
    }
}

/// Output tile edge for [`Pixels::affine`].
const AFFINE_TILE: i32 = 64;

impl Pixels {
    /// The image of these pixels under `m` (from source pixel coordinates,
    /// the top-left corner at 0, 0, to output coordinates), over the output
    /// rectangle `out`. Output pixels whose source falls outside get
    /// `fill`. The output is cut into tiles rendered side by side.
    pub fn affine(&self, m: &Affine, out: Rect, fill: u32, resample: Resample) -> Pixels {
        let mut result = Pixels::new(out.w.max(0) as u32, out.h.max(0) as u32, fill);
        let Some(inv) = m.inverse() else {
            return result;
        };
        if self.width == 0 || self.height == 0 || out.is_empty() {
            return result;
        }
        let mut tiles = Vec::new();
        let mut y = out.y;
        while y < out.bottom() {
            let mut x = out.x;
            while x < out.right() {
                tiles.push(Rect::new(x, y, AFFINE_TILE, AFFINE_TILE).intersect(&out));
                x += AFFINE_TILE;
            }
            y += AFFINE_TILE;
        }
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(tiles.len());
        let rendered: Vec<(Rect, Vec<u32>)> = if threads <= 1 {
            tiles
                .iter()
                .map(|t| (*t, self.affine_tile(&inv, *t, fill, resample)))
                .collect()
        } else {
            let per = tiles.len().div_ceil(threads);
            std::thread::scope(|s| {
                let jobs: Vec<_> = tiles
                    .chunks(per)
                    .map(|chunk| {
                        s.spawn(move || {
                            chunk
                                .iter()
                                .map(|t| (*t, self.affine_tile(&inv, *t, fill, resample)))
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                jobs.into_iter()
                    .flat_map(|j| j.join().expect("a transform tile renders"))
                    .collect()
            })
        };
        let w = result.width as usize;
        for (t, values) in rendered {
            for (i, v) in values.into_iter().enumerate() {
                let (tx, ty) = (i as i32 % t.w, i as i32 / t.w);
                let (ox, oy) = ((t.x - out.x + tx) as usize, (t.y - out.y + ty) as usize);
                result.data[oy * w + ox] = v;
            }
        }
        result
    }

    /// One output tile of [`Pixels::affine`]; `inv` maps output to source.
    fn affine_tile(&self, inv: &Affine, tile: Rect, fill: u32, resample: Resample) -> Vec<u32> {
        let (sw, sh) = (self.width as f64, self.height as f64);
        let mut out = Vec::with_capacity(tile.area() as usize);
        match resample {
            Resample::Nearest | Resample::CleanEdge => {
                for p in tile.points() {
                    let (sx, sy) = inv.apply(p.x as f64 + 0.5, p.y as f64 + 0.5);
                    if sx < 0.0 || sy < 0.0 || sx >= sw || sy >= sh {
                        out.push(fill);
                    } else if resample == Resample::Nearest {
                        out.push(self.get(sx as i32, sy as i32));
                    } else {
                        out.push(self.clean_edge(sx, sy));
                    }
                }
            }
            Resample::RotSprite => {
                // Enlarge only the source this tile reads, with a margin for
                // Scale2x's neighbors, so a large image never needs a whole
                // 8x copy.
                let src = inv.bounds_of(tile).inflate(3).intersect(&Rect::new(
                    0,
                    0,
                    self.width as i32,
                    self.height as i32,
                ));
                if src.is_empty() {
                    return vec![fill; tile.area() as usize];
                }
                let mut part = Pixels::new(src.w as u32, src.h as u32, 0);
                for (i, q) in src.points().enumerate() {
                    part.data[i] = self.get(q.x, q.y);
                }
                let big = part.upscale8();
                for p in tile.points() {
                    let (sx, sy) = inv.apply(p.x as f64 + 0.5, p.y as f64 + 0.5);
                    if sx < 0.0 || sy < 0.0 || sx >= sw || sy >= sh {
                        out.push(fill);
                        continue;
                    }
                    let bx = ((sx - src.x as f64) * 8.0).floor() as i32;
                    let by = ((sy - src.y as f64) * 8.0).floor() as i32;
                    if bx < 0 || by < 0 || bx >= big.width as i32 || by >= big.height as i32 {
                        out.push(fill);
                    } else {
                        out.push(big.get(bx, by));
                    }
                }
            }
        }
        out
    }

    /// The [`Resample::CleanEdge`] sample at a source position.
    fn clean_edge(&self, sx: f64, sy: f64) -> u32 {
        let (x, y) = (sx.floor() as i32, sy.floor() as i32);
        let (fx, fy) = (sx - x as f64, sy - y as f64);
        let p = self.get(x, y);
        let hx = if fx < 0.5 { -1 } else { 1 };
        let hy = if fy < 0.5 { -1 } else { 1 };
        let h = self.get_clamped(x + hx, y);
        let v = self.get_clamped(x, y + hy);
        let h_far = self.get_clamped(x - hx, y);
        let v_far = self.get_clamped(x, y - hy);
        // Scale2x's corner rule: the two neighbors at this corner agree, and
        // neither agrees with the neighbor across from the other.
        if h == v && h != p && h != v_far && v != h_far {
            let u = if hx < 0 { fx } else { 1.0 - fx };
            let w = if hy < 0 { fy } else { 1.0 - fy };
            if u + w < 0.5 {
                return h;
            }
        }
        p
    }

    /// The pixels of `rect` that `selection` holds; the rest read `clear`.
    pub fn from_raster_masked(
        raster: &Raster,
        rect: Rect,
        selection: Option<&crate::selection::Selection>,
        clear: u32,
    ) -> Self {
        let mut out = Self::from_raster(raster, rect);
        if let Some(s) = selection {
            for (i, p) in rect.points().enumerate() {
                if !s.contains(p.x, p.y) {
                    out.data[i] = clear;
                }
            }
        }
        out
    }
}

/// Writes `lifted` (pixels whose top-left sat at `at` on the canvas)
/// through `m` (canvas to canvas) into `raster`, skipping `clear` pixels so
/// what is under stays. Returns the changed area.
pub fn draw_transformed(
    raster: &mut Raster,
    lifted: &Pixels,
    at: Point,
    m: &Affine,
    resample: Resample,
    clear: u32,
) -> Rect {
    let local = Affine::translate(at.x as f64, at.y as f64).then(m);
    let src = Rect::new(0, 0, lifted.width as i32, lifted.height as i32);
    let out = local.bounds_of(src).intersect(&raster.bounds());
    if out.is_empty() {
        return Rect::EMPTY;
    }
    let px = lifted.affine(&local, out, clear, resample);
    px.write_to(raster, Point::new(out.x, out.y), Some(clear))
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
    /// Any affine map (canvas to canvas), as free transform makes.
    Affine(Affine, Resample),
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
    if let CelTransform::Affine(m, resample) = op {
        let mut dirty = raster.fill_rect_raw(&rect, clear);
        let at = Point::new(rect.x, rect.y);
        dirty = dirty.union(&draw_transformed(raster, &src, at, &m, resample, clear));
        return Ok(dirty);
    }
    let out = match op {
        CelTransform::FlipH => src.flip_h(),
        CelTransform::FlipV => src.flip_v(),
        CelTransform::Rotate90(q) => src.rotate_quarters(q),
        CelTransform::Rotate(deg) => {
            let (w, h) = src.rotated_size(deg);
            src.rotsprite(deg, w, h, clear)
        }
        CelTransform::Scale(w, h) => src.scale_nearest(w, h),
        CelTransform::Affine(..) => unreachable!("handled above"),
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
    /// Resizes every image to this size, picking pixels by the resampling.
    Scale(u32, u32, Resample),
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
        CanvasTransform::Scale(sw, sh, _) => (sw, sh),
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
            CanvasTransform::Scale(sw, sh, _) => {
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
    // References keep their pixels: their place follows the canvas instead.
    let refs = doc.reference_images();
    for (_, raster) in doc.images.iter_mut().filter(|(id, _)| !refs.contains(id)) {
        let px = Pixels::from_raster(raster, raster.bounds());
        let out = match op {
            CanvasTransform::FlipH => px.flip_h(),
            CanvasTransform::FlipV => px.flip_v(),
            CanvasTransform::Rotate90(q) => px.rotate_quarters(q),
            CanvasTransform::Scale(sw, sh, Resample::Nearest) => px.scale_nearest(sw, sh),
            CanvasTransform::Scale(sw, sh, resample) => {
                let fill = raster.fill_raw();
                let m = Affine::scale(sw as f64 / w as f64, sh as f64 / h as f64);
                px.affine(&m, Rect::new(0, 0, sw as i32, sh as i32), fill, resample)
            }
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
            CanvasTransform::Scale(sw, sh, _) => {
                Vec2::new(p.x * sw as f64 / fw, p.y * sh as f64 / fh)
            }
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
    let canvas = match op {
        CanvasTransform::FlipH => Affine::scale(-1.0, 1.0).then(&Affine::translate(fw, 0.0)),
        CanvasTransform::FlipV => Affine::scale(1.0, -1.0).then(&Affine::translate(0.0, fh)),
        CanvasTransform::Rotate90(q) => match q.rem_euclid(4) {
            0 => Affine::IDENTITY,
            1 => Affine::rotate(90.0).then(&Affine::translate(fh, 0.0)),
            2 => Affine::rotate(180.0).then(&Affine::translate(fw, fh)),
            _ => Affine::rotate(270.0).then(&Affine::translate(0.0, fw)),
        },
        CanvasTransform::Scale(sw, sh, _) => Affine::scale(sw as f64 / fw, sh as f64 / fh),
    };
    doc.for_each_reference(|size, place| {
        let moved = place.matrix(size).then(&canvas);
        if let Some(p) = ReferencePlace::from_matrix(&moved, size) {
            *place = p;
        }
    });
    doc.s.width = nw;
    doc.s.height = nh;
    match op {
        CanvasTransform::Scale(..) => doc.rescale_tilemaps(),
        _ => doc.turn_tilemaps(
            |px| match op {
                CanvasTransform::FlipH => px.flip_h(),
                CanvasTransform::FlipV => px.flip_v(),
                CanvasTransform::Rotate90(q) => px.rotate_quarters(q),
                CanvasTransform::Scale(..) => unreachable!("scaled above"),
            },
            map_rect,
        ),
    }
    Ok(doc.bounds())
}

/// Gives the canvas a new size, with what was at the top-left corner now at
/// `offset` (negative cuts off the left or top). Images and masks move;
/// what falls outside is cut off, new area is empty. References move by
/// the offset and keep all their pixels.
/// Slices move too. Run it in a [`crate::History`] transaction with
/// [`crate::Scope::All`] to make it undoable.
pub fn resize_canvas(
    doc: &mut Document,
    width: u32,
    height: u32,
    offset: Point,
) -> Result<Rect, Error> {
    if width == 0
        || height == 0
        || width > crate::document::MAX_SIZE
        || height > crate::document::MAX_SIZE
    {
        return Err(Error::InvalidSize { width, height });
    }
    let refs = doc.reference_images();
    for (_, raster) in doc.images.iter_mut().filter(|(id, _)| !refs.contains(id)) {
        let px = Pixels::from_raster(raster, raster.bounds());
        let mut fresh = match raster {
            Raster::Rgba(i) => Raster::Rgba(crate::image::TiledImage::new(width, height, i.fill())),
            Raster::Indexed(i) => {
                Raster::Indexed(crate::image::TiledImage::new(width, height, i.fill()))
            }
            Raster::Mask(i) => Raster::Mask(crate::image::TiledImage::new(width, height, i.fill())),
        };
        let fill = fresh.fill_raw();
        px.write_to(&mut fresh, offset, Some(fill));
        *raster = fresh;
    }
    for slice in doc.s.slices.iter_mut() {
        slice.bounds.x += offset.x;
        slice.bounds.y += offset.y;
    }
    let (dx, dy) = (offset.x as f64, offset.y as f64);
    doc.for_each_reference(|_, place| {
        place.center = (place.center.0 + dx, place.center.1 + dy);
    });
    for rig in doc.rigs_mut() {
        rig.map(
            |p| Vec2::new(p.x + dx, p.y + dy),
            false,
            0.0,
            |r| {
                let b = r.bounds();
                let moved = Rect::new(b.x + offset.x, b.y + offset.y, b.w, b.h);
                Region::from_bits(moved, r.bits().to_vec())
                    .unwrap_or_else(|| Region::rect(Rect::EMPTY))
            },
        );
    }
    doc.s.width = width;
    doc.s.height = height;
    doc.move_tilemaps(offset);
    Ok(doc.bounds())
}

/// Crops the canvas to `rect`.
pub fn crop(doc: &mut Document, rect: Rect) -> Result<Rect, Error> {
    resize_canvas(
        doc,
        rect.w.max(0) as u32,
        rect.h.max(0) as u32,
        Point::new(-rect.x, -rect.y),
    )
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
    fn affine_maps_compose_and_invert() {
        let m = Affine::about((4.0, 4.0), (2.0, -1.0), (10.0, 0.0), 30.0, (1.0, 2.0));
        let inv = m.inverse().unwrap();
        let (x, y) = m.apply(3.0, 7.0);
        let (bx, by) = inv.apply(x, y);
        assert!((bx - 3.0).abs() < 1e-9 && (by - 7.0).abs() < 1e-9);
        let r = Affine::rotate(90.0);
        let (x, y) = r.apply(1.0, 0.0);
        assert!(
            x.abs() < 1e-9 && (y - 1.0).abs() < 1e-9,
            "clockwise on screen"
        );
        assert_eq!(
            Affine::scale(2.0, 3.0).bounds_of(Rect::new(1, 1, 2, 2)),
            Rect::new(2, 3, 4, 6)
        );
    }

    #[test]
    fn affine_resampling_keeps_colors_and_flips_exactly() {
        let p = grid(5, 4);
        let flip = Affine::scale(-1.0, 1.0).then(&Affine::translate(5.0, 0.0));
        for r in [Resample::Nearest, Resample::RotSprite, Resample::CleanEdge] {
            assert_eq!(
                p.affine(&flip, Rect::new(0, 0, 5, 4), 0, r),
                p.flip_h(),
                "{r:?}"
            );
            let twice = p.affine(&Affine::scale(2.0, 2.0), Rect::new(0, 0, 10, 8), 0, r);
            assert!(twice.data.iter().all(|v| p.data.contains(v)), "{r:?}");
        }
        // A 45-degree step: clean edge cuts the corner, nearest does not.
        let step = Pixels {
            width: 2,
            height: 2,
            data: vec![1, 0, 1, 1],
        };
        let up = Affine::scale(8.0, 8.0);
        let nearest = step.affine(&up, Rect::new(0, 0, 16, 16), 9, Resample::Nearest);
        let clean = step.affine(&up, Rect::new(0, 0, 16, 16), 9, Resample::CleanEdge);
        let ones = |q: &Pixels| q.data.iter().filter(|v| **v == 1).count();
        assert_eq!(ones(&nearest), 3 * 64);
        assert!(
            ones(&clean) > ones(&nearest),
            "the empty pixel's inner corner fills"
        );
        assert_eq!(clean.get(15, 0), 0, "the far corner stays");
    }

    #[test]
    fn rotsprite_through_tiles_matches_a_whole_enlargement() {
        let mut p = Pixels::new(150, 90, 0);
        for i in 0..p.data.len() {
            p.data[i] = ((i * 7919) % 5) as u32;
        }
        let (w, h) = p.rotated_size(33.0);
        let r = p.rotsprite(33.0, w, h, 0);
        // The same rotation sampled from one full 8x enlargement.
        let big = p.scale2x().scale2x().scale2x();
        let m = Affine::translate(-75.0, -45.0)
            .then(&Affine::rotate(33.0))
            .then(&Affine::translate(w as f64 / 2.0, h as f64 / 2.0));
        let inv = m.inverse().unwrap();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let (sx, sy) = inv.apply(x as f64 + 0.5, y as f64 + 0.5);
                let want = if sx < 0.0 || sy < 0.0 || sx >= 150.0 || sy >= 90.0 {
                    0
                } else {
                    big.get((sx * 8.0).floor() as i32, (sy * 8.0).floor() as i32)
                };
                assert_eq!(r.get(x, y), want, "({x}, {y})");
            }
        }
    }

    #[test]
    fn canvas_size_crop_and_resampled_resize() {
        let mut d = Document::new(8, 8, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let red = Rgba::opaque(255, 0, 0).to_u32();
        d.target(l, 0).unwrap().raster.set_raw(1, 1, red);
        resize_canvas(&mut d, 12, 10, Point::new(2, 3)).unwrap();
        assert_eq!((d.width(), d.height()), (12, 10));
        assert_eq!(d.cel_raster(l, 0).unwrap().unwrap().get_raw(3, 4), red);
        crop(&mut d, Rect::new(3, 4, 2, 2)).unwrap();
        assert_eq!((d.width(), d.height()), (2, 2));
        assert_eq!(d.cel_raster(l, 0).unwrap().unwrap().get_raw(0, 0), red);
        transform_canvas(&mut d, CanvasTransform::Scale(6, 6, Resample::CleanEdge)).unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.get_raw(0, 0), red);
        assert_eq!(r.get_raw(5, 5), 0);
        assert!(resize_canvas(&mut d, 0, 4, Point::new(0, 0)).is_err());
    }

    #[test]
    fn a_cel_takes_a_free_transform() {
        let mut d = Document::new(16, 16, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let red = Rgba::opaque(255, 0, 0).to_u32();
        for x in 2..6 {
            d.target(l, 0).unwrap().raster.set_raw(x, 2, red);
        }
        let m = Affine::about((4.0, 2.5), (2.0, 1.0), (0.0, 0.0), 0.0, (4.0, 6.0));
        transform_cel(
            &mut d,
            l,
            0,
            Rect::new(2, 2, 4, 1),
            CelTransform::Affine(m, Resample::Nearest),
        )
        .unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.content_bounds(), Rect::new(4, 8, 8, 1));
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
        transform_canvas(&mut d, CanvasTransform::Scale(12, 8, Resample::Nearest)).unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.content_bounds(), Rect::new(0, 0, 2, 2));
        assert!(transform_canvas(&mut d, CanvasTransform::Scale(0, 8, Resample::Nearest)).is_err());
    }

    #[test]
    fn canvas_edits_move_references_and_keep_their_pixels() {
        use crate::document::{NewLayer, Placement};
        let mut d = Document::new(64, 32, ColorMode::Rgba).unwrap();
        let mut img = crate::image::TiledImage::new(400, 300, Rgba::TRANSPARENT);
        img.set(399, 299, Rgba::WHITE);
        let place = ReferencePlace {
            center: (20.0, 10.0),
            scale: (0.1, 0.1),
            rotation: 30.0,
        };
        let r = d
            .add_layer(
                NewLayer::Reference { image: img, place },
                "photo",
                Placement::default(),
            )
            .unwrap();
        // Where the image's bottom-right corner lands on the canvas.
        let corner = |d: &Document| {
            let (img, p) = d.reference(r).unwrap();
            assert_eq!((img.width(), img.height()), (400, 300), "full size kept");
            assert_eq!(img.get(399, 299), Rgba::WHITE);
            p.matrix((400, 300)).apply(400.0, 300.0)
        };
        let near =
            |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6;
        let c0 = corner(&d);
        resize_canvas(&mut d, 80, 40, Point::new(5, -2)).unwrap();
        let c1 = corner(&d);
        assert!(near(c1, (c0.0 + 5.0, c0.1 - 2.0)));
        transform_canvas(&mut d, CanvasTransform::FlipH).unwrap();
        let c2 = corner(&d);
        assert!(near(c2, (80.0 - c1.0, c1.1)));
        transform_canvas(&mut d, CanvasTransform::Rotate90(1)).unwrap();
        let c3 = corner(&d);
        assert!(near(c3, (40.0 - c2.1, c2.0)));
        transform_canvas(&mut d, CanvasTransform::Scale(80, 160, Resample::Nearest)).unwrap();
        let c4 = corner(&d);
        assert!(near(c4, (c3.0 * 2.0, c3.1 * 2.0)));
        crop(&mut d, Rect::new(10, 10, 20, 20)).unwrap();
        assert!(near(corner(&d), (c4.0 - 10.0, c4.1 - 10.0)));
    }
}
