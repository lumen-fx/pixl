//! Flattening a frame's layers into RGBA pixels.
//!
//! Colors stay straight (not premultiplied) and are blended in floating point
//! per the W3C Compositing and Blending Level 1 model: the blend function
//! `B(Cb, Cs)` mixes source and backdrop where both are present,
//!
//! ```text
//! Cs' = (1 - ab) * Cs + ab * B(Cb, Cs)
//! ao  = as + ab * (1 - as)
//! Co  = (as * Cs' + ab * (1 - as) * Cb) / ao
//! ```
//!
//! then source-over places the result. Groups composite their children into
//! a transparent buffer first (isolation) and blend that buffer as one layer.
//! A clipped layer shows only where the nearest unclipped layer below it in the
//! same group has coverage (its pixel alpha times cel opacity and mask).

use crate::color::{Palette, Rgba};
use crate::document::{Background, BlendMode, Document, Layer, LayerKind};
use crate::geom::Rect;
use crate::image::{Raster, TILE_SIZE, TiledImage};

/// What a composite includes.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositeOptions {
    /// Show reference layers. The editor view does; exports do not.
    pub include_reference: bool,
    /// Start from the document background color instead of transparency.
    pub background: bool,
    /// Ghosts of neighboring frames.
    pub onion: Option<OnionSkin>,
}

impl CompositeOptions {
    /// What an export writes: no reference layers, no onion skin.
    pub const EXPORT: CompositeOptions = CompositeOptions {
        include_reference: false,
        background: true,
        onion: None,
    };
}

impl Default for CompositeOptions {
    fn default() -> Self {
        Self::EXPORT
    }
}

/// Onion skin settings.
#[derive(Clone, Debug, PartialEq)]
pub struct OnionSkin {
    /// Frames shown before the current one.
    pub before: usize,
    /// Frames shown after the current one.
    pub after: usize,
    /// Opacity of the nearest ghost frame.
    pub opacity: u8,
    /// Each further frame keeps this percentage of the previous one's opacity.
    pub falloff_percent: u8,
    /// Tint for earlier frames; its alpha is the tint strength.
    pub tint_before: Option<Rgba>,
    /// Tint for later frames; its alpha is the tint strength.
    pub tint_after: Option<Rgba>,
    /// Draw ghosts over the current frame instead of under it.
    pub above: bool,
}

impl Default for OnionSkin {
    fn default() -> Self {
        Self {
            before: 1,
            after: 1,
            opacity: 96,
            falloff_percent: 50,
            tint_before: Some(Rgba::new(255, 0, 0, 128)),
            tint_after: Some(Rgba::new(0, 0, 255, 128)),
            above: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Px {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

const INV255: f32 = 1.0 / 255.0;

impl Px {
    const CLEAR: Px = Px {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    fn from_rgba(c: Rgba) -> Px {
        Px {
            r: c.r as f32 * INV255,
            g: c.g as f32 * INV255,
            b: c.b as f32 * INV255,
            a: c.a as f32 * INV255,
        }
    }

    fn to_rgba(self) -> Rgba {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        if self.a <= 0.0 {
            return Rgba::TRANSPARENT;
        }
        Rgba::new(q(self.r), q(self.g), q(self.b), q(self.a))
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        for v in &mut out {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 {
        for v in &mut out {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&a, &b| c[a].total_cmp(&c[b]));
    let (lo, mid, hi) = (idx[0], idx[1], idx[2]);
    let mut out = [0.0; 3];
    if c[hi] > c[lo] {
        out[mid] = (c[mid] - c[lo]) * s / (c[hi] - c[lo]);
        out[hi] = s;
    }
    out[lo] = 0.0;
    out
}

fn separable(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    let multiply = |b: f32, s: f32| b * s;
    let screen = |b: f32, s: f32| b + s - b * s;
    let hard_light = |b: f32, s: f32| {
        if s <= 0.5 {
            multiply(b, 2.0 * s)
        } else {
            screen(b, 2.0 * s - 1.0)
        }
    };
    match mode {
        BlendMode::Normal => cs,
        BlendMode::Multiply => multiply(cb, cs),
        BlendMode::Screen => screen(cb, cs),
        BlendMode::Overlay => hard_light(cs, cb),
        BlendMode::Darken => cb.min(cs),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::ColorDodge => {
            if cb <= 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        BlendMode::ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        BlendMode::HardLight => hard_light(cb, cs),
        BlendMode::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 {
                    ((16.0 * cb - 12.0) * cb + 4.0) * cb
                } else {
                    cb.sqrt()
                };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        BlendMode::Difference => (cb - cs).abs(),
        BlendMode::Exclusion => cb + cs - 2.0 * cb * cs,
        BlendMode::Add => (cb + cs).min(1.0),
        BlendMode::Subtract => (cb - cs).max(0.0),
        BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity => {
            unreachable!("non-separable modes blend whole colors")
        }
    }
}

/// `B(Cb, Cs)` for one pixel.
fn blend_fn(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        BlendMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        BlendMode::Color => set_lum(cs, lum(cb)),
        BlendMode::Luminosity => set_lum(cb, lum(cs)),
        _ => [
            separable(mode, cb[0], cs[0]),
            separable(mode, cb[1], cs[1]),
            separable(mode, cb[2], cs[2]),
        ],
    }
}

/// Composites `src` (straight color, alpha already scaled by opacity) over
/// `dst` with `mode`.
#[inline]
fn blend_px(dst: &mut Px, src: Px, mode: BlendMode) {
    let as_ = src.a;
    if as_ <= 0.0 {
        return;
    }
    let ab = dst.a;
    let cs = [src.r, src.g, src.b];
    let cb = [dst.r, dst.g, dst.b];
    let mixed = if mode == BlendMode::Normal || ab <= 0.0 {
        cs
    } else {
        let b = blend_fn(mode, cb, cs);
        [
            (1.0 - ab) * cs[0] + ab * b[0],
            (1.0 - ab) * cs[1] + ab * b[1],
            (1.0 - ab) * cs[2] + ab * b[2],
        ]
    };
    let k = ab * (1.0 - as_);
    let ao = as_ + k;
    let inv = 1.0 / ao;
    dst.r = (as_ * mixed[0] + k * cb[0]) * inv;
    dst.g = (as_ * mixed[1] + k * cb[1]) * inv;
    dst.b = (as_ * mixed[2] + k * cb[2]) * inv;
    dst.a = ao;
}

/// Blends two colors the way a layer with `mode` and `opacity` lands on a
/// backdrop. Exposed for tools and tests that need one pixel.
pub fn blend(backdrop: Rgba, source: Rgba, mode: BlendMode, opacity: u8) -> Rgba {
    let mut d = Px::from_rgba(backdrop);
    let mut s = Px::from_rgba(source);
    s.a *= opacity as f32 * INV255;
    blend_px(&mut d, s, mode);
    d.to_rgba()
}

struct Ctx<'a> {
    doc: &'a Document,
    frame: usize,
    rect: Rect,
    include_reference: bool,
}

/// How a layer's pixels reach the band: color plus coverage per pixel.
fn sample_image(
    img: &Raster,
    palette: &Palette,
    ti: u8,
    rect: Rect,
    mut f: impl FnMut(usize, Rgba),
) {
    fn walk<P: crate::image::Pixel>(
        img: &TiledImage<P>,
        rect: Rect,
        transparent_fill: bool,
        to: &dyn Fn(P) -> Rgba,
        f: &mut dyn FnMut(usize, Rgba),
    ) {
        let r = rect.intersect(&img.bounds());
        let tiles: Vec<usize> = img.tiles_in(&r).collect();
        for t in tiles {
            let tr = img.tile_rect(t).intersect(&r);
            let tile = img.tile(t);
            if tile.is_none() && transparent_fill {
                continue;
            }
            let fill = to(img.fill());
            let ox = tr.x - tr.x % TILE_SIZE;
            let oy = tr.y - tr.y % TILE_SIZE;
            for y in tr.y..tr.bottom() {
                let row = (y - rect.y) * rect.w - rect.x;
                let trow = (y - oy) * TILE_SIZE - ox;
                for x in tr.x..tr.right() {
                    let c = match tile {
                        Some(t) => to(t[(trow + x) as usize]),
                        None => fill,
                    };
                    if c.a != 0 {
                        f((row + x) as usize, c);
                    }
                }
            }
        }
    }
    match img {
        Raster::Rgba(i) => walk(i, rect, i.fill().a == 0, &|c| c, &mut f),
        Raster::Indexed(i) => {
            let to = |v: u8| {
                if v == ti {
                    Rgba::TRANSPARENT
                } else {
                    palette.get(v)
                }
            };
            walk(i, rect, to(i.fill()).a == 0, &to, &mut f)
        }
        Raster::Mask(i) => walk(i, rect, i.fill() == 0, &|v| Rgba::new(v, v, v, v), &mut f),
    }
}

fn mask_at(doc: &Document, layer: &Layer, x: i32, y: i32) -> f32 {
    if let LayerKind::Raster { mask: Some(m), .. } = &layer.kind
        && m.enabled
        && let Some(img) = doc.image(m.image)
    {
        return img.get_raw(x, y) as f32 * INV255;
    }
    1.0
}

/// Composites `layers` onto `buf` (one entry per pixel of `ctx.rect`).
fn composite_layers(ctx: &Ctx, layers: &[Layer], buf: &mut [Px]) {
    let doc = ctx.doc;
    let rect = ctx.rect;
    let palette = doc.palette();
    let ti = doc.transparent_index();
    let n = buf.len();
    // Coverage of the current clip base, when a clipped layer follows it.
    let mut clip_base: Option<Vec<f32>> = None;
    for (i, layer) in layers.iter().enumerate() {
        let next_clips = layers.get(i + 1).is_some_and(|l| l.props.clip);
        let p = &layer.props;
        let clip = if p.clip { clip_base.as_deref() } else { None };
        if p.clip && clip.is_none() {
            // A clipped layer with no base below it shows nothing.
            continue;
        }
        let shown = p.visible && p.opacity > 0;
        let opacity = p.opacity as f32 * INV255;
        let mut coverage = if !p.clip && next_clips {
            Some(vec![0.0f32; n])
        } else {
            None
        };
        match &layer.kind {
            LayerKind::Raster { cels, .. } => {
                let cel = cels.get(ctx.frame).copied().flatten();
                if let (true, Some(cel)) = (shown, cel)
                    && let Some(img) = doc.image(cel.image)
                {
                    let has_mask =
                        matches!(&layer.kind, LayerKind::Raster { mask: Some(m), .. } if m.enabled);
                    let cel_op = cel.opacity as f32 * INV255;
                    sample_image(img, palette, ti, rect, |idx, c| {
                        let mut s = Px::from_rgba(c);
                        let mut a = s.a * cel_op;
                        if has_mask {
                            let x = rect.x + (idx % rect.w as usize) as i32;
                            let y = rect.y + (idx / rect.w as usize) as i32;
                            a *= mask_at(doc, layer, x, y);
                        }
                        if let Some(cov) = coverage.as_mut() {
                            cov[idx] = a;
                        }
                        if let Some(base) = clip {
                            a *= base[idx];
                        }
                        s.a = a * opacity;
                        blend_px(&mut buf[idx], s, p.blend);
                    });
                }
            }
            LayerKind::Group { children } => {
                if shown {
                    let mut inner = vec![Px::CLEAR; n];
                    composite_layers(ctx, children, &mut inner);
                    for (idx, s) in inner.into_iter().enumerate() {
                        if s.a <= 0.0 {
                            continue;
                        }
                        if let Some(cov) = coverage.as_mut() {
                            cov[idx] = s.a;
                        }
                        let mut s = s;
                        s.a *= opacity * clip.map_or(1.0, |b| b[idx]);
                        blend_px(&mut buf[idx], s, p.blend);
                    }
                }
            }
            LayerKind::Reference { image } => {
                if ctx.include_reference
                    && shown
                    && let Some(img) = doc.image(*image)
                {
                    sample_image(img, palette, ti, rect, |idx, c| {
                        let mut s = Px::from_rgba(c);
                        s.a *= opacity * clip.map_or(1.0, |b| b[idx]);
                        blend_px(&mut buf[idx], s, p.blend);
                    });
                }
            }
        }
        if !p.clip {
            clip_base = coverage.take();
        }
    }
}

fn background_px(doc: &Document, opts: &CompositeOptions) -> Px {
    match (opts.background, doc.background()) {
        (true, Background::Color(c)) => Px::from_rgba(c),
        _ => Px::CLEAR,
    }
}

fn composite_band(doc: &Document, frame: usize, rect: Rect, opts: &CompositeOptions) -> Vec<Px> {
    let n = rect.area() as usize;
    let ctx = Ctx {
        doc,
        frame,
        rect,
        include_reference: opts.include_reference,
    };
    let bg = background_px(doc, opts);
    let Some(onion) = &opts.onion else {
        let mut buf = vec![bg; n];
        composite_layers(&ctx, doc.layers(), &mut buf);
        return buf;
    };
    let mut current = vec![Px::CLEAR; n];
    composite_layers(&ctx, doc.layers(), &mut current);
    // Ghost frames, farthest first so nearer ones land on top.
    let mut ghosts: Vec<(usize, usize, Option<Rgba>)> = Vec::new();
    for k in (1..=onion.before).rev() {
        if let Some(f) = frame.checked_sub(k) {
            ghosts.push((f, k, onion.tint_before));
        }
    }
    for k in (1..=onion.after).rev() {
        if frame + k < doc.frame_count() {
            ghosts.push((frame + k, k, onion.tint_after));
        }
    }
    let mut ghost_buf = vec![Px::CLEAR; n];
    for (f, k, tint) in ghosts {
        let mut g = vec![Px::CLEAR; n];
        let gctx = Ctx { frame: f, ..ctx };
        composite_layers(&gctx, doc.layers(), &mut g);
        let mut op = onion.opacity as f32 * INV255;
        for _ in 1..k {
            op *= onion.falloff_percent as f32 / 100.0;
        }
        let tint = tint.map(|t| (Px::from_rgba(t), t.a as f32 * INV255));
        for (dst, mut s) in ghost_buf.iter_mut().zip(g) {
            if s.a <= 0.0 {
                continue;
            }
            if let Some((t, amount)) = tint {
                s.r += (t.r - s.r) * amount;
                s.g += (t.g - s.g) * amount;
                s.b += (t.b - s.b) * amount;
            }
            s.a *= op;
            blend_px(dst, s, BlendMode::Normal);
        }
    }
    let mut out = vec![bg; n];
    let (first, second) = if onion.above {
        (&current, &ghost_buf)
    } else {
        (&ghost_buf, &current)
    };
    for ((dst, a), b) in out.iter_mut().zip(first).zip(second) {
        blend_px(dst, *a, BlendMode::Normal);
        blend_px(dst, *b, BlendMode::Normal);
    }
    out
}

/// Composites `rect` of `frame` into row-major RGBA pixels, splitting the
/// work across threads by bands of rows.
pub fn composite(doc: &Document, frame: usize, rect: Rect, opts: &CompositeOptions) -> Vec<Rgba> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    composite_with_threads(doc, frame, rect, opts, threads)
}

/// [`composite`] with an explicit thread count.
pub fn composite_with_threads(
    doc: &Document,
    frame: usize,
    rect: Rect,
    opts: &CompositeOptions,
    threads: usize,
) -> Vec<Rgba> {
    if rect.is_empty() {
        return Vec::new();
    }
    let w = rect.w as usize;
    let mut out = vec![Rgba::TRANSPARENT; rect.area() as usize];
    // Bands are whole tile rows so each band touches each tile once.
    let rows_per_band = {
        let want = (rect.h as usize).div_ceil(threads.max(1)).max(1);
        want.div_ceil(TILE_SIZE as usize) * TILE_SIZE as usize
    };
    let run = |y0: usize, chunk: &mut [Rgba]| {
        let h = chunk.len() / w;
        let band = Rect::new(rect.x, rect.y + y0 as i32, rect.w, h as i32);
        for (dst, px) in chunk.iter_mut().zip(composite_band(doc, frame, band, opts)) {
            *dst = px.to_rgba();
        }
    };
    if threads <= 1 || rect.h as usize <= rows_per_band {
        run(0, &mut out);
        return out;
    }
    std::thread::scope(|s| {
        for (i, chunk) in out.chunks_mut(rows_per_band * w).enumerate() {
            let run = &run;
            s.spawn(move || run(i * rows_per_band, chunk));
        }
    });
    out
}

/// The whole canvas of `frame`.
pub fn composite_frame(doc: &Document, frame: usize, opts: &CompositeOptions) -> Vec<Rgba> {
    composite(doc, frame, doc.bounds(), opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorMode;
    use crate::document::{LayerId, NewLayer, Placement};

    const RED: Rgba = Rgba::opaque(255, 0, 0);
    const BLUE: Rgba = Rgba::opaque(0, 0, 255);

    fn px(doc: &Document, x: i32, y: i32) -> Rgba {
        composite(doc, 0, Rect::pixel(x, y), &CompositeOptions::EXPORT)[0]
    }

    fn paint(doc: &mut Document, layer: LayerId, frame: usize, x: i32, y: i32, c: Rgba) {
        doc.target(layer, frame)
            .unwrap()
            .raster
            .set_raw(x, y, c.to_u32());
    }

    fn two_layers() -> (Document, LayerId, LayerId) {
        let mut d = Document::new(8, 8, ColorMode::Rgba).unwrap();
        let bottom = d.layers()[0].id;
        let top = d
            .add_layer(NewLayer::Raster, "top", Placement::default())
            .unwrap();
        (d, bottom, top)
    }

    #[test]
    fn normal_straight_alpha_known_values() {
        assert_eq!(blend(BLUE, RED, BlendMode::Normal, 255), RED);
        assert_eq!(
            blend(BLUE, Rgba::new(255, 0, 0, 128), BlendMode::Normal, 255),
            Rgba::new(128, 0, 127, 255)
        );
        // Half red over half blue: ao = 0.752, r = 0.502 / ao, b = 0.25 / ao.
        assert_eq!(
            blend(
                Rgba::new(0, 0, 255, 128),
                Rgba::new(255, 0, 0, 128),
                BlendMode::Normal,
                255
            ),
            Rgba::new(170, 0, 85, 192)
        );
        assert_eq!(blend(BLUE, RED, BlendMode::Normal, 0), BLUE);
        assert_eq!(blend(Rgba::TRANSPARENT, RED, BlendMode::Multiply, 255), RED);
    }

    #[test]
    fn separable_modes_known_values() {
        let gray = Rgba::opaque(128, 128, 128);
        let o = |mode, b: Rgba, s: Rgba| blend(b, s, mode, 255);
        assert_eq!(
            o(BlendMode::Multiply, gray, Rgba::opaque(255, 128, 0)),
            Rgba::opaque(128, 64, 0)
        );
        assert_eq!(
            o(BlendMode::Screen, gray, gray),
            Rgba::opaque(192, 192, 192)
        );
        assert_eq!(
            o(
                BlendMode::Difference,
                Rgba::opaque(50, 200, 0),
                Rgba::opaque(200, 50, 0)
            ),
            Rgba::opaque(150, 150, 0)
        );
        assert_eq!(
            o(BlendMode::Exclusion, gray, Rgba::opaque(255, 0, 128)),
            Rgba::opaque(127, 128, 127)
        );
        assert_eq!(
            o(
                BlendMode::Add,
                Rgba::opaque(200, 10, 0),
                Rgba::opaque(100, 20, 0)
            ),
            Rgba::opaque(255, 30, 0)
        );
        assert_eq!(
            o(
                BlendMode::Subtract,
                Rgba::opaque(100, 50, 0),
                Rgba::opaque(200, 20, 0)
            ),
            Rgba::opaque(0, 30, 0)
        );
        assert_eq!(
            o(
                BlendMode::Darken,
                Rgba::opaque(100, 50, 0),
                Rgba::opaque(200, 20, 9)
            ),
            Rgba::opaque(100, 20, 0)
        );
        assert_eq!(
            o(
                BlendMode::Lighten,
                Rgba::opaque(100, 50, 0),
                Rgba::opaque(200, 20, 9)
            ),
            Rgba::opaque(200, 50, 9)
        );
        // Overlay picks multiply or screen by the backdrop.
        assert_eq!(
            o(BlendMode::Overlay, Rgba::opaque(64, 192, 0), gray),
            Rgba::opaque(64, 192, 0)
        );
        assert_eq!(
            o(
                BlendMode::HardLight,
                Rgba::opaque(64, 192, 0),
                Rgba::opaque(0, 255, 0)
            ),
            Rgba::opaque(0, 255, 0)
        );
        // Dodge by half doubles; burn by half: 1 - (1 - 0.753) / 0.502.
        assert_eq!(
            o(BlendMode::ColorDodge, Rgba::opaque(64, 0, 255), gray),
            Rgba::opaque(129, 0, 255)
        );
        assert_eq!(
            o(BlendMode::ColorBurn, Rgba::opaque(192, 255, 0), gray),
            Rgba::opaque(129, 255, 0)
        );
        // Soft light with a neutral source leaves the backdrop alone.
        assert_eq!(
            o(
                BlendMode::SoftLight,
                Rgba::opaque(30, 200, 90),
                Rgba::opaque(128, 128, 128)
            ),
            Rgba::opaque(30, 200, 90)
        );
        assert_eq!(
            o(BlendMode::SoftLight, Rgba::opaque(64, 64, 64), Rgba::WHITE),
            Rgba::opaque(128, 128, 128)
        );
    }

    #[test]
    fn non_separable_modes_known_values() {
        let gray = Rgba::opaque(128, 128, 128);
        let o = |mode, b: Rgba, s: Rgba| blend(b, s, mode, 255);
        // Hue and color of a saturated source over gray: gray has no
        // saturation, so hue keeps gray; color takes red at gray's luma.
        assert_eq!(o(BlendMode::Hue, gray, RED), gray);
        assert_eq!(
            o(BlendMode::Saturation, RED, gray),
            Rgba::opaque(77, 77, 77)
        );
        let c = o(BlendMode::Color, gray, RED);
        assert!(c.r > c.g && c.g == c.b, "{c:?}");
        let l = |c: Rgba| 0.3 * c.r as f32 + 0.59 * c.g as f32 + 0.11 * c.b as f32;
        assert!((l(c) - 128.0).abs() < 1.5);
        let lu = o(BlendMode::Luminosity, RED, gray);
        assert!((l(lu) - 128.0).abs() < 1.5);
        assert!(lu.r > lu.g, "keeps the backdrop's hue: {lu:?}");
        for mode in BlendMode::ALL {
            assert_eq!(o(mode, RED, Rgba::TRANSPARENT), RED, "{mode:?}");
        }
    }

    #[test]
    fn layer_opacity_and_visibility() {
        let (mut d, bottom, top) = two_layers();
        paint(&mut d, bottom, 0, 1, 1, BLUE);
        paint(&mut d, top, 0, 1, 1, RED);
        assert_eq!(px(&d, 1, 1), RED);
        d.update_layer(top, |p| p.opacity = 128).unwrap();
        assert_eq!(px(&d, 1, 1), Rgba::new(128, 0, 127, 255));
        d.update_layer(top, |p| p.visible = false).unwrap();
        assert_eq!(px(&d, 1, 1), BLUE);
        d.set_cel_opacity(bottom, 0, 0).unwrap();
        assert_eq!(px(&d, 1, 1), Rgba::TRANSPARENT);
    }

    #[test]
    fn groups_isolate_their_blend_modes() {
        let (mut d, bottom, top) = two_layers();
        paint(&mut d, bottom, 0, 0, 0, Rgba::opaque(128, 128, 128));
        paint(&mut d, top, 0, 0, 0, Rgba::opaque(255, 128, 0));
        d.update_layer(top, |p| p.blend = BlendMode::Multiply)
            .unwrap();
        assert_eq!(px(&d, 0, 0), Rgba::opaque(128, 64, 0));
        let g = d
            .add_layer(NewLayer::Group, "g", Placement::default())
            .unwrap();
        d.move_layer(
            top,
            Placement {
                parent: Some(g),
                index: None,
            },
        )
        .unwrap();
        assert_eq!(
            px(&d, 0, 0),
            Rgba::opaque(255, 128, 0),
            "multiply had nothing below it in the group"
        );
        d.update_layer(g, |p| p.blend = BlendMode::Multiply)
            .unwrap();
        assert_eq!(px(&d, 0, 0), Rgba::opaque(128, 64, 0));
    }

    #[test]
    fn clipping_follows_base_coverage() {
        let (mut d, bottom, top) = two_layers();
        paint(&mut d, bottom, 0, 2, 2, BLUE);
        paint(&mut d, bottom, 0, 3, 2, Rgba::new(0, 0, 255, 128));
        for x in 0..5 {
            paint(&mut d, top, 0, x, 2, RED);
        }
        d.update_layer(top, |p| p.clip = true).unwrap();
        assert_eq!(px(&d, 1, 2), Rgba::TRANSPARENT);
        assert_eq!(px(&d, 2, 2), RED);
        // Half-covered base: red lands at half alpha over the half blue.
        assert_eq!(px(&d, 3, 2), Rgba::new(170, 0, 85, 192));
        d.update_layer(bottom, |p| p.visible = false).unwrap();
        assert_eq!(px(&d, 2, 2), Rgba::TRANSPARENT, "a hidden base still clips");
    }

    #[test]
    fn masks_hide_pixels() {
        let (mut d, bottom, _) = two_layers();
        paint(&mut d, bottom, 0, 4, 4, RED);
        paint(&mut d, bottom, 0, 5, 4, RED);
        d.add_layer_mask(bottom).unwrap();
        d.mask_target(bottom).unwrap().raster.set_raw(4, 4, 0);
        d.mask_target(bottom).unwrap().raster.set_raw(5, 4, 128);
        assert_eq!(px(&d, 4, 4), Rgba::TRANSPARENT);
        assert_eq!(px(&d, 5, 4), Rgba::new(255, 0, 0, 128));
        d.set_mask_enabled(bottom, false).unwrap();
        assert_eq!(px(&d, 4, 4), RED);
    }

    #[test]
    fn background_reference_and_indexed() {
        let mut d = Document::new(4, 4, ColorMode::Indexed).unwrap();
        let l = d.layers()[0].id;
        d.target(l, 0).unwrap().raster.set_raw(0, 0, 4);
        assert_eq!(px(&d, 0, 0), d.palette().get(4));
        assert_eq!(px(&d, 1, 0), Rgba::TRANSPARENT);
        d.set_background(Background::Color(Rgba::WHITE));
        assert_eq!(px(&d, 1, 0), Rgba::WHITE);
        let opts = CompositeOptions {
            background: false,
            ..CompositeOptions::EXPORT
        };
        assert_eq!(
            composite(&d, 0, Rect::pixel(1, 0), &opts)[0],
            Rgba::TRANSPARENT
        );
        d.add_layer(
            NewLayer::Reference(TiledImage::new(4, 4, BLUE)),
            "ref",
            Placement::default(),
        )
        .unwrap();
        assert_eq!(px(&d, 1, 0), Rgba::WHITE, "exports skip references");
        let view = CompositeOptions {
            include_reference: true,
            ..CompositeOptions::EXPORT
        };
        assert_eq!(composite(&d, 0, Rect::pixel(1, 0), &view)[0], BLUE);
    }

    #[test]
    fn onion_skin_ghosts_neighbors() {
        let mut d = Document::new(4, 4, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        d.insert_frame(1, 100).unwrap();
        d.insert_frame(2, 100).unwrap();
        d.insert_frame(3, 100).unwrap();
        paint(&mut d, l, 0, 0, 0, Rgba::WHITE);
        paint(&mut d, l, 1, 1, 0, Rgba::WHITE);
        paint(&mut d, l, 2, 2, 0, Rgba::WHITE);
        paint(&mut d, l, 3, 3, 0, Rgba::WHITE);
        let opts = CompositeOptions {
            onion: Some(OnionSkin {
                before: 2,
                after: 1,
                opacity: 200,
                falloff_percent: 50,
                tint_before: None,
                tint_after: Some(Rgba::new(0, 0, 255, 255)),
                above: false,
            }),
            ..CompositeOptions::EXPORT
        };
        let row = composite(&d, 2, Rect::new(0, 0, 4, 1), &opts);
        assert_eq!(
            row[0],
            Rgba::new(255, 255, 255, 100),
            "two back: half opacity"
        );
        assert_eq!(row[1], Rgba::new(255, 255, 255, 200));
        assert_eq!(row[2], Rgba::WHITE, "the current frame");
        assert_eq!(row[3], Rgba::new(0, 0, 255, 200), "tinted later frame");
    }

    #[test]
    fn threaded_bands_match_single_thread() {
        let mut d = Document::new(300, 260, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let t = d
            .add_layer(NewLayer::Raster, "t", Placement::default())
            .unwrap();
        d.update_layer(t, |p| p.blend = BlendMode::Overlay).unwrap();
        for y in 0..260 {
            for x in 0..300 {
                let c = Rgba::new((x * 7) as u8, (y * 3) as u8, (x ^ y) as u8, (x + y) as u8);
                paint(&mut d, l, 0, x, y, c);
                if (x + y) % 3 == 0 {
                    paint(&mut d, t, 0, x, y, Rgba::new(y as u8, x as u8, 90, 200));
                }
            }
        }
        let opts = CompositeOptions::EXPORT;
        let one = composite_with_threads(&d, 0, d.bounds(), &opts, 1);
        let many = composite_with_threads(&d, 0, d.bounds(), &opts, 7);
        assert_eq!(one, many);
        let part = composite_with_threads(&d, 0, Rect::new(50, 70, 100, 90), &opts, 3);
        assert_eq!(part[0], one[70 * 300 + 50]);
        assert_eq!(part[89 * 100 + 99], one[159 * 300 + 149]);
    }
}
