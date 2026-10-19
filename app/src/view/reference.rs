//! Reference images on the canvas, at screen resolution.
//!
//! A reference layer keeps its image at full resolution, and the view draws
//! it smoothly with its own transform (its place over the document, then the
//! view's zoom, pan and turn), independent of the document's pixel grid.
//! Renderers differ in how they sample a buffer drawn smaller than its size:
//! some average it, others (Windows' software adapter) take single pixels,
//! which shimmer once an image shrinks past half its size. So each image is
//! kept as a pyramid of levels, each half the one before, and drawn from the
//! level nearest its size on screen, the same on every renderer. A level
//! wider or taller than a canvas buffer may be is never made; the file keeps
//! the full image either way.
//!
//! The levels are made on a worker thread when a reference appears, the
//! smallest first, and written into their buffers a few rows per tick, so
//! adding a large photo never stalls a frame. Until a level is in, the
//! nearest one that is stands in for it.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use lumenui_canvas::{CompositeOperation, Context2d, PixelBuffer};
use pixl_document::{Affine, Document, ImageId, Raster, ReferencePlace, Rgba, TiledImage};

use crate::view::View;

/// Levels stop halving once the longer side is this small.
const SMALLEST: u32 = 32;

/// Rows of a level written in one go before the deadline is looked at.
const ROWS_PER_WRITE: u32 = 64;

/// One level of an image's pyramid: the image halved `k` times.
struct Level {
    size: (u32, u32),
    buffer: Option<PixelBuffer>,
    /// Every row is in the buffer.
    ready: bool,
}

/// A level the worker finished, being written into its buffer.
struct Writing {
    k: usize,
    rgba: Vec<u8>,
    buffer: PixelBuffer,
    row: u32,
}

/// A level the worker made: its index and its straight RGBA8 rows.
type MadeLevel = (usize, Vec<u8>);

/// The pyramid of one reference image.
struct Pyramid {
    image: ImageId,
    /// The image the levels were made from, to see when it changes. Its
    /// tiles are shared with the document, so this costs a pointer a tile.
    source: TiledImage<Rgba>,
    levels: Vec<Level>,
    /// The first level that fits a canvas buffer.
    first: usize,
    /// Levels as the worker finishes them. Behind a lock only so the view's
    /// resource can be shared; the main thread alone reads it.
    from_worker: Option<Mutex<Receiver<MadeLevel>>>,
    writing: Option<Writing>,
    /// Whether the last sync still wanted it.
    wanted: bool,
}

/// The size of each level of an image of `size`: halved, rounding up, down
/// to [`SMALLEST`].
fn level_sizes(size: (u32, u32)) -> Vec<(u32, u32)> {
    let mut out = vec![size];
    let mut s = size;
    while s.0.max(s.1) > SMALLEST {
        s = (s.0.div_ceil(2), s.1.div_ceil(2));
        out.push(s);
    }
    out
}

/// Halves straight RGBA8 pixels, averaging each 2x2 block in premultiplied
/// color so transparent pixels do not darken the edges they border.
pub fn halve(rgba: &[u8], size: (u32, u32)) -> Vec<u8> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let rows = [2 * y, (2 * y + 1).min(h - 1)];
        for x in 0..nw {
            let cols = [2 * x, (2 * x + 1).min(w - 1)];
            let mut sum = [0u32; 4];
            for &sy in &rows {
                for &sx in &cols {
                    let p = &rgba[(sy * w + sx) * 4..(sy * w + sx) * 4 + 4];
                    let a = u32::from(p[3]);
                    sum[0] += u32::from(p[0]) * a;
                    sum[1] += u32::from(p[1]) * a;
                    sum[2] += u32::from(p[2]) * a;
                    sum[3] += a;
                }
            }
            let o = &mut out[(y * nw + x) * 4..(y * nw + x) * 4 + 4];
            let alpha = sum[3];
            for c in 0..3 {
                if let Some(v) = (sum[c] + alpha / 2).checked_div(alpha) {
                    o[c] = v as u8;
                }
            }
            o[3] = ((alpha + 2) / 4) as u8;
        }
    }
    out
}

/// Which level draws an image shown at `screen` screen pixels per image
/// pixel: the one nearest that size among those that are in, a finer one
/// on a tie. `None` while none is.
fn pick(ready: &[bool], first: usize, screen: f64) -> Option<usize> {
    let want = if screen >= 1.0 || !screen.is_finite() {
        0.0
    } else {
        (1.0 / screen).log2().round()
    };
    let want = (want.max(0.0) as usize).clamp(first, ready.len().saturating_sub(1));
    (0..ready.len())
        .filter(|&k| k >= first && ready[k])
        .min_by_key(|&k| (k.abs_diff(want), k))
}

impl Pyramid {
    fn new(image: ImageId, source: &TiledImage<Rgba>) -> Self {
        let size = (source.width(), source.height());
        let sizes = level_sizes(size);
        let side = crate::MAX_VIEW_SIDE;
        let first = sizes
            .iter()
            .position(|s| s.0 <= side && s.1 <= side)
            .unwrap_or(sizes.len() - 1);
        let (to_main, from_worker) = channel();
        let pixels = source.clone();
        std::thread::spawn(move || {
            let px = pixels.to_pixels();
            let mut rgba = Vec::with_capacity(px.len() * 4);
            for c in &px {
                rgba.extend_from_slice(&[c.r, c.g, c.b, c.a]);
            }
            drop(px);
            let mut levels = Vec::new();
            let mut cur = rgba;
            for (k, s) in sizes.iter().enumerate() {
                let next = (k + 1 < sizes.len()).then(|| halve(&cur, *s));
                if k >= first {
                    levels.push((k, cur));
                }
                match next {
                    Some(n) => cur = n,
                    None => break,
                }
            }
            // The smallest first, so the image shows at once.
            for level in levels.into_iter().rev() {
                if to_main.send(level).is_err() {
                    return;
                }
            }
        });
        Self {
            image,
            source: source.clone(),
            levels: level_sizes(size)
                .into_iter()
                .map(|size| Level {
                    size,
                    buffer: None,
                    ready: false,
                })
                .collect(),
            first,
            from_worker: Some(Mutex::new(from_worker)),
            writing: None,
            wanted: true,
        }
    }

    /// Whether `img` is the image the levels were made from.
    fn shows(&self, img: &TiledImage<Rgba>) -> bool {
        self.source.width() == img.width()
            && self.source.height() == img.height()
            && self.source.fill() == img.fill()
            && (0..img.tile_count()).all(|t| self.source.same_tile(img, t))
    }

    /// Writes finished levels into buffers until `deadline`. Returns whether
    /// a level became ready.
    fn pump(&mut self, deadline: Instant) -> Result<bool, String> {
        let mut became_ready = false;
        loop {
            if self.writing.is_none() {
                let Some(rx) = self.from_worker.as_mut() else {
                    return Ok(became_ready);
                };
                let next = match rx.get_mut() {
                    Ok(rx) => rx.try_recv(),
                    Err(_) => Err(TryRecvError::Disconnected),
                };
                match next {
                    Ok((k, rgba)) => {
                        let (w, h) = self.levels[k].size;
                        let buffer = PixelBuffer::new(w, h).map_err(|e| e.to_string())?;
                        self.writing = Some(Writing {
                            k,
                            rgba,
                            buffer,
                            row: 0,
                        });
                    }
                    Err(TryRecvError::Empty) => return Ok(became_ready),
                    Err(TryRecvError::Disconnected) => {
                        self.from_worker = None;
                        return Ok(became_ready);
                    }
                }
            }
            let Some(job) = self.writing.as_mut() else {
                continue;
            };
            let (w, h) = self.levels[job.k].size;
            while job.row < h {
                let rows = ROWS_PER_WRITE.min(h - job.row);
                let at = job.row as usize * w as usize * 4;
                let end = at + rows as usize * w as usize * 4;
                job.buffer
                    .write_rgba8(0, i64::from(job.row), w, rows, &job.rgba[at..end])
                    .map_err(|e| e.to_string())?;
                job.row += rows;
                if Instant::now() >= deadline && job.row < h {
                    return Ok(became_ready);
                }
            }
            let job = self.writing.take().expect("checked above");
            let level = &mut self.levels[job.k];
            level.buffer = Some(job.buffer);
            level.ready = true;
            became_ready = true;
            if Instant::now() >= deadline {
                return Ok(became_ready);
            }
        }
    }

    /// Levels still to make or write.
    fn pending(&self) -> usize {
        self.levels[self.first..]
            .iter()
            .filter(|l| !l.ready)
            .count()
    }
}

/// The pyramids of every reference the view shows.
#[derive(Default)]
pub struct References {
    pyramids: Vec<Pyramid>,
    /// Set when a pyramid could not be made, such as a buffer over the
    /// canvas's cap.
    pub error: Option<String>,
}

impl References {
    /// Keeps a pyramid for each reference image in `images`, made anew when
    /// its pixels changed, and drops the rest.
    pub fn sync(&mut self, doc: &Document, images: &[ImageId]) {
        for p in &mut self.pyramids {
            p.wanted = false;
        }
        for &id in images {
            let Some(Raster::Rgba(img)) = doc.image(id) else {
                continue;
            };
            match self.pyramids.iter_mut().find(|p| p.image == id) {
                Some(p) if p.shows(img) => p.wanted = true,
                Some(p) => *p = Pyramid::new(id, img),
                None => self.pyramids.push(Pyramid::new(id, img)),
            }
        }
        self.pyramids.retain(|p| p.wanted);
    }

    /// Writes finished levels into their buffers until `deadline`. Returns
    /// whether one became ready, so the canvas draws again.
    pub fn pump(&mut self, deadline: Instant) -> bool {
        let mut any = false;
        for p in &mut self.pyramids {
            match p.pump(deadline) {
                Ok(ready) => any |= ready,
                Err(e) => {
                    self.error = Some(format!("a reference image cannot be shown: {e}"));
                    p.from_worker = None;
                    p.writing = None;
                }
            }
        }
        any
    }

    /// Levels still being made or written.
    pub fn pending(&self) -> usize {
        self.pyramids
            .iter()
            .filter(|p| p.from_worker.is_some() || p.writing.is_some())
            .map(Pyramid::pending)
            .sum()
    }

    /// The buffers the pyramids hold, and their memory in bytes.
    pub fn buffer_stats(&self) -> (usize, u64) {
        let mut n = 0;
        let mut bytes = 0;
        for l in self.pyramids.iter().flat_map(|p| &p.levels) {
            if l.buffer.is_some() {
                n += 1;
                bytes += u64::from(l.size.0) * u64::from(l.size.1) * 4;
            }
        }
        (n, bytes)
    }

    /// The level a reference draws from at this view, and its size.
    pub fn level_for(&self, image: ImageId, place: ReferencePlace, view: &View) -> Option<usize> {
        let p = self.pyramids.iter().find(|p| p.image == image)?;
        let ready: Vec<bool> = p.levels.iter().map(|l| l.ready).collect();
        pick(&ready, p.first, screen_scale(place, view.zoom))
    }

    /// Draws a reference image where `place` puts it in `view`, smoothly,
    /// with `alpha` and `op`. Draws nothing while no level is in, or when
    /// the image is off the stage.
    pub fn draw(
        &self,
        ctx: &Context2d,
        view: &View,
        image: ImageId,
        place: ReferencePlace,
        alpha: f64,
        op: CompositeOperation,
    ) {
        let Some(p) = self.pyramids.iter().find(|p| p.image == image) else {
            return;
        };
        let Some(k) = self.level_for(image, place, view) else {
            return;
        };
        let level = &p.levels[k];
        let Some(buffer) = &level.buffer else {
            return;
        };
        let size = p.levels[0].size;
        let m = stage_matrix(size, level.size, place, view);
        let (lw, lh) = (f64::from(level.size.0), f64::from(level.size.1));
        let corners = [(0.0, 0.0), (lw, 0.0), (lw, lh), (0.0, lh)].map(|(x, y)| m.apply(x, y));
        let (sw, sh) = (f64::from(view.stage.1.x), f64::from(view.stage.1.y));
        let off = corners.iter().all(|c| c.0 < 0.0)
            || corners.iter().all(|c| c.0 > sw)
            || corners.iter().all(|c| c.1 < 0.0)
            || corners.iter().all(|c| c.1 > sh);
        if off {
            return;
        }
        ctx.save();
        ctx.set_transform([m.a, m.b, m.c, m.d, m.e, m.f]);
        ctx.set_image_smoothing(true);
        ctx.set_global_alpha(alpha);
        ctx.set_global_composite_operation(op);
        ctx.draw_buffer(buffer, 0.0, 0.0);
        ctx.restore();
    }
}

/// Screen pixels per image pixel of a reference at `zoom`.
fn screen_scale(place: ReferencePlace, zoom: f64) -> f64 {
    (place.scale.0 * place.scale.1).abs().sqrt() * zoom
}

/// The map from a level's pixels to the stage: up to the image's pixels,
/// onto the document by the reference's place, then the view's zoom, pan
/// and turn.
pub fn stage_matrix(
    size: (u32, u32),
    level: (u32, u32),
    place: ReferencePlace,
    view: &View,
) -> Affine {
    let up = Affine::scale(
        f64::from(size.0) / f64::from(level.0),
        f64::from(size.1) / f64::from(level.1),
    );
    let doc_to_stage =
        Affine::scale(view.zoom, view.zoom).then(&Affine::translate(view.origin.0, view.origin.1));
    let turn = if view.turned() {
        let [a, b, c, d, e, f] = view.matrix();
        Affine { a, b, c, d, e, f }
    } else {
        Affine::IDENTITY
    };
    up.then(&place.matrix(size)).then(&doc_to_stage).then(&turn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_halve_down_to_a_small_side() {
        let s = level_sizes((4000, 3000));
        assert_eq!(s[0], (4000, 3000));
        assert_eq!(s[1], (2000, 1500));
        assert_eq!(s[2], (1000, 750));
        assert!(s.last().unwrap().0 <= SMALLEST);
        assert_eq!(level_sizes((5, 3)), vec![(5, 3)]);
        assert_eq!(level_sizes((33, 1))[1], (17, 1));
    }

    #[test]
    fn halving_averages_in_premultiplied_color() {
        // Opaque red beside fully transparent black: the red stays red.
        let px = [255, 0, 0, 255, 0, 0, 0, 0];
        assert_eq!(halve(&px, (2, 1)), vec![255, 0, 0, 128]);
        // An odd width keeps the last column.
        let px = [10, 10, 10, 255, 20, 20, 20, 255, 30, 30, 30, 255];
        assert_eq!(halve(&px, (3, 1)), vec![15, 15, 15, 255, 30, 30, 30, 255]);
    }

    #[test]
    fn the_level_nearest_the_screen_size_draws() {
        let all = [true; 8];
        // Shown at a sixteenth of its size: four halvings.
        assert_eq!(pick(&all, 0, 1.0 / 16.0), Some(4));
        assert_eq!(pick(&all, 0, 3.0), Some(0));
        assert_eq!(pick(&all, 0, 1e-6), Some(7), "no smaller level");
        // A level too large for a buffer is never drawn.
        assert_eq!(pick(&all, 2, 1.0), Some(2));
        // Until the wanted level is in, the nearest one that is.
        let some = [false, false, true, false, false, false, true, false];
        assert_eq!(pick(&some, 0, 1.0 / 16.0), Some(2));
        assert_eq!(pick(&some, 0, 1.0 / 64.0), Some(6));
        assert_eq!(pick(&[false; 3], 0, 1.0), None);
    }

    #[test]
    fn the_stage_matrix_follows_place_zoom_pan_and_turn() {
        let mut view = View {
            zoom: 8.0,
            origin: (100.0, 50.0),
            stage: (
                lumenui::glam::Vec2::ZERO,
                lumenui::glam::Vec2::new(800.0, 600.0),
            ),
            ..View::default()
        };
        let place = ReferencePlace {
            center: (16.0, 16.0),
            scale: (0.01, 0.01),
            rotation: 0.0,
        };
        // A 4000x3000 image drawn from its 1000x750 level.
        let m = stage_matrix((4000, 3000), (1000, 750), place, &view);
        // The level's middle is the image's middle, on document (16, 16).
        let mid = m.apply(500.0, 375.0);
        assert!((mid.0 - 228.0).abs() < 1e-9 && (mid.1 - 178.0).abs() < 1e-9);
        // Its right edge, 2000 image pixels out: 20 document pixels.
        let right = m.apply(1000.0, 375.0);
        assert!((right.0 - (228.0 + 160.0)).abs() < 1e-9);
        // A turned view turns it around the stage's middle like the art.
        view.rotation = 90.0;
        let turned = stage_matrix((4000, 3000), (1000, 750), place, &view);
        let t = view.matrix();
        let want = (
            t[0] * mid.0 + t[2] * mid.1 + t[4],
            t[1] * mid.0 + t[3] * mid.1 + t[5],
        );
        let got = turned.apply(500.0, 375.0);
        assert!((got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9);
    }
}
