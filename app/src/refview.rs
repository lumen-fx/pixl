//! Reference images on the canvas, at screen resolution.
//!
//! A reference layer keeps its image at full resolution, and the view draws
//! it smoothly with its own transform (its place over the document, then the
//! view's zoom, pan and turn), independent of the document's pixel grid.
//! Lumen's renderers filter a buffer drawn smaller than its size, so one
//! buffer per image serves every zoom. An image wider or taller than a
//! canvas buffer may be is shown halved until it fits; the file keeps the
//! full image either way.
//!
//! The buffer's pixels are made on a worker thread when a reference appears
//! and written in a few rows at a time, so adding a large photo never stalls
//! a frame.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use lumenui_canvas::{CompositeOperation, Context2d, PixelBuffer};
use pixl_core::{Affine, Document, ImageId, Raster, ReferencePlace, Rgba, TiledImage};

use crate::view::View;

/// Rows written in one go before the deadline is looked at.
const ROWS_PER_WRITE: u32 = 64;

/// The size an image of `size` is shown at: halved, rounding up, until
/// neither side passes `side`.
pub fn shown_size(size: (u32, u32), side: u32) -> (u32, u32) {
    let mut s = size;
    while s.0 > side || s.1 > side {
        s = (s.0.div_ceil(2), s.1.div_ceil(2));
    }
    s
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

/// The buffer of one reference image.
struct Shown {
    image: ImageId,
    /// The image the buffer was made from, to see when it changes. Its
    /// tiles are shared with the document, so this costs a pointer a tile.
    source: TiledImage<Rgba>,
    /// The buffer's size: the image's, or less for one past the cap.
    size: (u32, u32),
    /// The buffer once every row is in.
    buffer: Option<PixelBuffer>,
    /// The pixels from the worker, while they arrive. Behind a lock only so
    /// the view's resource can be shared; the main thread alone reads it.
    from_worker: Option<Mutex<Receiver<Vec<u8>>>>,
    /// Pixels being written: the rows, the buffer and the next row.
    writing: Option<(Vec<u8>, PixelBuffer, u32)>,
    /// Whether the last sync still wanted it.
    wanted: bool,
}

impl Shown {
    fn new(image: ImageId, source: &TiledImage<Rgba>) -> Self {
        let full = (source.width(), source.height());
        let size = shown_size(full, crate::MAX_VIEW_SIDE);
        let (to_main, from_worker) = channel();
        let pixels = source.clone();
        std::thread::spawn(move || {
            let px = pixels.to_pixels();
            let mut rgba = Vec::with_capacity(px.len() * 4);
            for c in &px {
                rgba.extend_from_slice(&[c.r, c.g, c.b, c.a]);
            }
            drop(px);
            let mut at = full;
            while at != size {
                rgba = halve(&rgba, at);
                at = (at.0.div_ceil(2), at.1.div_ceil(2));
            }
            // The view may have dropped it meanwhile.
            let _ = to_main.send(rgba);
        });
        Self {
            image,
            source: source.clone(),
            size,
            buffer: None,
            from_worker: Some(Mutex::new(from_worker)),
            writing: None,
            wanted: true,
        }
    }

    /// Whether `img` is the image the buffer was made from.
    fn shows(&self, img: &TiledImage<Rgba>) -> bool {
        self.source.width() == img.width()
            && self.source.height() == img.height()
            && self.source.fill() == img.fill()
            && (0..img.tile_count()).all(|t| self.source.same_tile(img, t))
    }

    /// Writes the worker's pixels into the buffer until `deadline`. Returns
    /// whether the buffer became ready.
    fn pump(&mut self, deadline: Instant) -> Result<bool, String> {
        if self.writing.is_none() {
            let Some(rx) = self.from_worker.as_mut() else {
                return Ok(false);
            };
            let next = match rx.get_mut() {
                Ok(rx) => rx.try_recv(),
                Err(_) => Err(TryRecvError::Disconnected),
            };
            match next {
                Ok(rgba) => {
                    self.from_worker = None;
                    let buffer =
                        PixelBuffer::new(self.size.0, self.size.1).map_err(|e| e.to_string())?;
                    self.writing = Some((rgba, buffer, 0));
                }
                Err(TryRecvError::Empty) => return Ok(false),
                Err(TryRecvError::Disconnected) => {
                    self.from_worker = None;
                    return Err("the image could not be prepared".into());
                }
            }
        }
        let Some((rgba, buffer, row)) = self.writing.as_mut() else {
            return Ok(false);
        };
        let (w, h) = self.size;
        while *row < h {
            let rows = ROWS_PER_WRITE.min(h - *row);
            let at = *row as usize * w as usize * 4;
            let end = at + rows as usize * w as usize * 4;
            buffer
                .write_rgba8(0, i64::from(*row), w, rows, &rgba[at..end])
                .map_err(|e| e.to_string())?;
            *row += rows;
            if *row < h && Instant::now() >= deadline {
                return Ok(false);
            }
        }
        let (_, buffer, _) = self.writing.take().expect("checked above");
        self.buffer = Some(buffer);
        Ok(true)
    }

    fn pending(&self) -> bool {
        self.from_worker.is_some() || self.writing.is_some()
    }
}

/// The buffers of every reference the view shows.
#[derive(Default)]
pub struct References {
    shown: Vec<Shown>,
    /// Set when an image could not be shown, such as a buffer over the
    /// canvas's cap.
    pub error: Option<String>,
}

impl References {
    /// Keeps a buffer for each reference image in `images`, made anew when
    /// its pixels changed, and drops the rest.
    pub fn sync(&mut self, doc: &Document, images: &[ImageId]) {
        for s in &mut self.shown {
            s.wanted = false;
        }
        for &id in images {
            let Some(Raster::Rgba(img)) = doc.image(id) else {
                continue;
            };
            match self.shown.iter_mut().find(|s| s.image == id) {
                Some(s) if s.shows(img) => s.wanted = true,
                Some(s) => *s = Shown::new(id, img),
                None => self.shown.push(Shown::new(id, img)),
            }
        }
        self.shown.retain(|s| s.wanted);
    }

    /// Writes arriving pixels into their buffers until `deadline`. Returns
    /// whether a buffer became ready, so the canvas draws again.
    pub fn pump(&mut self, deadline: Instant) -> bool {
        let mut any = false;
        for s in &mut self.shown {
            match s.pump(deadline) {
                Ok(ready) => any |= ready,
                Err(e) => {
                    self.error = Some(format!("a reference image cannot be shown: {e}"));
                    s.from_worker = None;
                    s.writing = None;
                }
            }
        }
        any
    }

    /// Images still being prepared or written.
    pub fn pending(&self) -> usize {
        self.shown.iter().filter(|s| s.pending()).count()
    }

    /// The buffers that are in, and their memory in bytes.
    pub fn buffer_stats(&self) -> (usize, u64) {
        self.shown
            .iter()
            .filter(|s| s.buffer.is_some())
            .fold((0, 0), |(n, bytes), s| {
                (n + 1, bytes + u64::from(s.size.0) * u64::from(s.size.1) * 4)
            })
    }

    /// Draws a reference image where `place` puts it in `view`, smoothly,
    /// with `alpha` and `op`. Draws nothing until its buffer is in, or when
    /// it is off the stage.
    pub fn draw(
        &self,
        ctx: &Context2d,
        view: &View,
        image: ImageId,
        place: ReferencePlace,
        alpha: f64,
        op: CompositeOperation,
    ) {
        let Some(s) = self.shown.iter().find(|s| s.image == image) else {
            return;
        };
        let Some(buffer) = &s.buffer else {
            return;
        };
        let full = (s.source.width(), s.source.height());
        let m = stage_matrix(full, s.size, place, view);
        let (bw, bh) = (f64::from(s.size.0), f64::from(s.size.1));
        let corners = [(0.0, 0.0), (bw, 0.0), (bw, bh), (0.0, bh)].map(|(x, y)| m.apply(x, y));
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

/// The map from the pixels of a buffer of `shown` size to the stage: up to
/// the image's `size`, onto the document by the reference's place, then the
/// view's zoom, pan and turn.
pub fn stage_matrix(
    size: (u32, u32),
    shown: (u32, u32),
    place: ReferencePlace,
    view: &View,
) -> Affine {
    let up = Affine::scale(
        f64::from(size.0) / f64::from(shown.0),
        f64::from(size.1) / f64::from(shown.1),
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
    fn an_image_past_the_cap_shows_halved_until_it_fits() {
        assert_eq!(shown_size((4000, 3000), 8192), (4000, 3000));
        assert_eq!(shown_size((20000, 3000), 8192), (5000, 750));
        assert_eq!(shown_size((8193, 1), 8192), (4097, 1));
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
    fn the_stage_matrix_follows_place_zoom_pan_and_turn() {
        let mut view = View::default();
        view.zoom = 8.0;
        view.origin = (100.0, 50.0);
        view.stage = (
            lumenui::glam::Vec2::ZERO,
            lumenui::glam::Vec2::new(800.0, 600.0),
        );
        let place = ReferencePlace {
            center: (16.0, 16.0),
            scale: (0.01, 0.01),
            rotation: 0.0,
        };
        // A 40000x30000 image shown from a 10000x7500 buffer.
        let m = stage_matrix((40000, 30000), (10000, 7500), place, &view);
        // The buffer's middle is the image's middle, on document (16, 16).
        let mid = m.apply(5000.0, 3750.0);
        assert!((mid.0 - 228.0).abs() < 1e-9 && (mid.1 - 178.0).abs() < 1e-9);
        // Its right edge, 20000 image pixels out: 200 document pixels.
        let right = m.apply(10000.0, 3750.0);
        assert!((right.0 - (228.0 + 1600.0)).abs() < 1e-9);
        // A turned view turns it around the stage's middle like the art.
        view.rotation = 90.0;
        let turned = stage_matrix((40000, 30000), (10000, 7500), place, &view);
        let t = view.matrix();
        let want = (
            t[0] * mid.0 + t[2] * mid.1 + t[4],
            t[1] * mid.0 + t[3] * mid.1 + t[5],
        );
        let got = turned.apply(5000.0, 3750.0);
        assert!((got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9);
    }
}
