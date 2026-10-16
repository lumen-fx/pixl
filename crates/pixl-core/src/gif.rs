//! Animated GIF export and import.
//!
//! GIF stores up to 256 colors per frame and one-bit transparency. Export
//! writes one global palette for the whole animation, so a color looks the
//! same in every frame. When the frames use at most 256 colors (255 when
//! some pixels are transparent) the palette holds them exactly; past that it
//! is reduced with median cut (Heckbert, "Color Image Quantization for Frame
//! Buffer Display", SIGGRAPH 1982), weighted by how often each color appears,
//! and every pixel takes the nearest palette color. No dithering is applied:
//! dither noise breaks pixel art. Pixels with alpha below 128 become
//! transparent and the rest opaque.
//!
//! GIF frame delays count hundredths of a second. Export rounds each frame
//! duration to the nearest 10 ms and never writes less than 20 ms, because
//! browsers show shorter delays as 100 ms. Import reads a delay of 0 or 10 ms
//! as 100 ms for the same reason.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::ops::Range;

use ::gif::{ColorOutput, DecodeOptions, DisposalMethod, Encoder, Frame as GifFrame, Repeat};

use crate::color::{ColorMode, Palette, Rgba};
use crate::composite::{CompositeOptions, composite_frame};
use crate::document::Document;
use crate::error::Error;
use crate::geom::Rect;
use crate::image::{Raster, TiledImage};
use crate::io::scale_pixels;

/// Alpha at or above this is opaque in a GIF; below it is transparent.
const ALPHA_THRESHOLD: u8 = 128;

fn gif_err(e: impl fmt::Display) -> Error {
    Error::Image(e.to_string())
}

/// What a GIF export writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GifOptions {
    /// The frames to write; `None` writes all of them.
    pub frames: Option<Range<usize>>,
    /// Integer enlargement (nearest).
    pub scale: u32,
    /// Times to play; `None` loops forever.
    pub repeat: Option<u16>,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            frames: None,
            scale: 1,
            repeat: None,
        }
    }
}

/// How a GIF export differs from the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GifWarning {
    /// The animation used more colors than a GIF holds; they were reduced to
    /// a palette of 255 or 256.
    ColorsReduced { colors: usize },
    /// Semi-transparent pixels became fully opaque or fully transparent.
    AlphaFlattened,
}

impl fmt::Display for GifWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GifWarning::ColorsReduced { colors } => write!(
                f,
                "the animation uses {colors} colors; the GIF holds the closest 256"
            ),
            GifWarning::AlphaFlattened => {
                f.write_str("semi-transparent pixels became opaque or transparent")
            }
        }
    }
}

/// An encoded GIF and how it differs from the document.
#[derive(Clone, Debug, PartialEq)]
pub struct GifExport {
    pub bytes: Vec<u8>,
    pub warnings: Vec<GifWarning>,
}

type Rgb = [u8; 3];

/// Median cut: splits the color histogram into at most `max` boxes and
/// returns each box's weighted mean. The box split next is the one with the
/// widest range on any channel; it is cut at the weighted median of that
/// channel.
fn median_cut(histogram: &[(Rgb, u32)], max: usize) -> Vec<Rgb> {
    if histogram.len() <= max {
        return histogram.iter().map(|(c, _)| *c).collect();
    }
    let widest = |b: &[(Rgb, u32)]| {
        (0..3)
            .map(|axis| {
                let lo = b.iter().map(|(c, _)| c[axis]).min().unwrap_or(0);
                let hi = b.iter().map(|(c, _)| c[axis]).max().unwrap_or(0);
                (hi - lo, axis)
            })
            .max()
            .unwrap_or((0, 0))
    };
    let mut boxes: Vec<Vec<(Rgb, u32)>> = vec![histogram.to_vec()];
    while boxes.len() < max {
        let Some((index, axis)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| {
                let (range, axis) = widest(b);
                let weight: u64 = b.iter().map(|(_, w)| *w as u64).sum();
                ((range, weight, std::cmp::Reverse(i)), (i, axis))
            })
            .max()
            .map(|(_, pick)| pick)
        else {
            break;
        };
        let mut b = std::mem::take(&mut boxes[index]);
        b.sort_unstable_by_key(|(c, _)| (c[axis], *c));
        let total: u64 = b.iter().map(|(_, w)| *w as u64).sum();
        let mut acc = 0u64;
        let mut cut = b.len() / 2;
        for (i, (_, w)) in b.iter().enumerate() {
            acc += *w as u64;
            if acc * 2 >= total {
                cut = i + 1;
                break;
            }
        }
        let cut = cut.clamp(1, b.len() - 1);
        let right = b.split_off(cut);
        boxes[index] = b;
        boxes.push(right);
    }
    boxes
        .iter()
        .map(|b| {
            let total: u64 = b.iter().map(|(_, w)| *w as u64).sum::<u64>().max(1);
            let mean = |axis: usize| {
                let sum: u64 = b.iter().map(|(c, w)| c[axis] as u64 * *w as u64).sum();
                ((sum + total / 2) / total) as u8
            };
            [mean(0), mean(1), mean(2)]
        })
        .collect()
}

fn nearest(palette: &[Rgb], c: Rgb) -> u8 {
    let d = |p: &Rgb| -> u32 {
        (0..3)
            .map(|i| (p[i] as i32 - c[i] as i32).pow(2) as u32)
            .sum()
    };
    palette
        .iter()
        .enumerate()
        .min_by_key(|(_, p)| d(p))
        .map_or(0, |(i, _)| i as u8)
}

/// Encodes frames of a document as an animated GIF.
pub fn export_gif(doc: &Document, opts: &GifOptions) -> Result<GifExport, Error> {
    let range = opts.frames.clone().unwrap_or(0..doc.frame_count());
    if range.start >= range.end || range.end > doc.frame_count() {
        return Err(Error::FrameOutOfRange(range.end));
    }
    let scale = opts.scale.max(1);
    let (w, h) = (doc.width() * scale, doc.height() * scale);
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return Err(Error::Image(format!("{w}x{h} is too large for a GIF")));
    }
    let mut warnings = Vec::new();
    let mut partial_alpha = false;
    let mut transparent = false;
    let mut order: Vec<Rgb> = Vec::new();
    let mut histogram: HashMap<Rgb, u32> = HashMap::new();
    let mut frames: Vec<Vec<Option<Rgb>>> = Vec::with_capacity(range.len());
    for f in range.clone() {
        let flat = composite_frame(doc, f, &CompositeOptions::EXPORT);
        let px = scale_pixels(doc.width(), doc.height(), &flat, scale);
        let frame: Vec<Option<Rgb>> = px
            .iter()
            .map(|c| {
                partial_alpha |= c.a != 0 && c.a != 255;
                if c.a < ALPHA_THRESHOLD {
                    transparent = true;
                    return None;
                }
                let rgb = [c.r, c.g, c.b];
                let n = histogram.entry(rgb).or_insert(0);
                if *n == 0 {
                    order.push(rgb);
                }
                *n += 1;
                Some(rgb)
            })
            .collect();
        frames.push(frame);
    }
    if partial_alpha {
        warnings.push(GifWarning::AlphaFlattened);
    }
    let cap = 256 - transparent as usize;
    let palette = if order.len() <= cap {
        order
    } else {
        warnings.push(GifWarning::ColorsReduced {
            colors: order.len(),
        });
        let mut hist: Vec<(Rgb, u32)> = histogram.into_iter().collect();
        hist.sort_unstable();
        median_cut(&hist, cap)
    };
    let index: HashMap<Rgb, u8> = palette
        .iter()
        .enumerate()
        .map(|(i, c)| (*c, i as u8))
        .collect();
    let ti = palette.len() as u8;
    let mut flat_palette: Vec<u8> = palette.iter().flatten().copied().collect();
    if transparent {
        flat_palette.extend([0, 0, 0]);
    }

    let mut out = Vec::new();
    {
        let mut enc = Encoder::new(&mut out, w as u16, h as u16, &flat_palette).map_err(gif_err)?;
        enc.set_repeat(match opts.repeat {
            None => Repeat::Infinite,
            Some(n) => Repeat::Finite(n),
        })
        .map_err(gif_err)?;
        let mut cache: HashMap<Rgb, u8> = HashMap::new();
        for (frame, f) in frames.iter().zip(range) {
            let indices: Vec<u8> = frame
                .iter()
                .map(|c| match c {
                    None => ti,
                    Some(c) => match index.get(c) {
                        Some(i) => *i,
                        None => *cache.entry(*c).or_insert_with(|| nearest(&palette, *c)),
                    },
                })
                .collect();
            let ms = doc.frames()[f].duration_ms;
            let delay = ms.saturating_add(5) / 10;
            let gf = GifFrame {
                width: w as u16,
                height: h as u16,
                buffer: Cow::Owned(indices),
                transparent: transparent.then_some(ti),
                delay: delay.clamp(2, u16::MAX as u32) as u16,
                dispose: if transparent {
                    DisposalMethod::Background
                } else {
                    DisposalMethod::Keep
                },
                ..GifFrame::default()
            };
            enc.write_frame(&gf).map_err(gif_err)?;
        }
    }
    Ok(GifExport {
        bytes: out,
        warnings,
    })
}

/// The frame duration a GIF delay means, as browsers play it.
fn delay_ms(delay: u16) -> u32 {
    if delay <= 1 { 100 } else { delay as u32 * 10 }
}

/// Reads a GIF into a new document: one layer, one frame per GIF frame, each
/// frame the full picture as the GIF shows it (partial frames and disposal
/// applied). A GIF whose frames all use its global palette opens as an
/// indexed document with that palette; any other opens as RGBA.
pub fn import_gif(bytes: &[u8]) -> Result<Document, Error> {
    let mut opts = DecodeOptions::new();
    opts.set_color_output(ColorOutput::Indexed);
    let mut dec = opts.read_info(bytes).map_err(gif_err)?;
    let (w, h) = (dec.width() as u32, dec.height() as u32);
    let global: Option<Vec<u8>> = dec.global_palette().map(<[u8]>::to_vec);
    let mut doc = Document::new(w, h, ColorMode::Rgba)?;
    let mut canvas = TiledImage::new(w, h, Rgba::TRANSPARENT);
    let mut frames: Vec<(TiledImage<Rgba>, u32)> = Vec::new();
    let mut local_palette = false;
    let mut transparent_indices = BTreeSet::new();
    while let Some(f) = dec.read_next_frame().map_err(gif_err)? {
        let palette = match (&f.palette, &global) {
            (Some(p), _) => {
                local_palette = true;
                p.as_slice()
            }
            (None, Some(g)) => g.as_slice(),
            (None, None) => return Err(Error::Image("a GIF frame has no palette".into())),
        };
        transparent_indices.extend(f.transparent);
        let rect = Rect::new(f.left as i32, f.top as i32, f.width as i32, f.height as i32);
        let previous = (f.dispose == DisposalMethod::Previous).then(|| canvas.clone());
        for (i, &index) in f.buffer.iter().enumerate().take(rect.area() as usize) {
            if Some(index) == f.transparent {
                continue;
            }
            let at = index as usize * 3;
            let c = palette
                .get(at..at + 3)
                .map_or(Rgba::BLACK, |c| Rgba::opaque(c[0], c[1], c[2]));
            let x = rect.x + i as i32 % rect.w;
            let y = rect.y + i as i32 / rect.w;
            canvas.set(x, y, c);
        }
        frames.push((canvas.clone(), delay_ms(f.delay)));
        match (f.dispose, previous) {
            (DisposalMethod::Background, _) => {
                canvas.fill_rect(&rect, Rgba::TRANSPARENT);
            }
            (DisposalMethod::Previous, Some(p)) => canvas = p,
            _ => {}
        }
    }
    if frames.is_empty() {
        return Err(Error::Image("the GIF has no frames".into()));
    }
    let layer = doc.layers()[0].id;
    for (i, (image, ms)) in frames.into_iter().enumerate() {
        if i == 0 {
            doc.set_frame_duration(0, ms)?;
        } else {
            doc.insert_frame(i, ms)?;
        }
        *doc.target(layer, i)?.raster = Raster::Rgba(image);
    }

    if let (false, Some(g)) = (local_palette, global) {
        let mut colors: Vec<Rgba> = g
            .chunks_exact(3)
            .take(Palette::MAX)
            .map(|c| Rgba::opaque(c[0], c[1], c[2]))
            .collect();
        let ti = match transparent_indices.first() {
            Some(&t) if transparent_indices.len() == 1 && (t as usize) < colors.len() => {
                colors[t as usize] = Rgba::TRANSPARENT;
                Some(t)
            }
            _ if colors.len() < Palette::MAX => {
                colors.push(Rgba::TRANSPARENT);
                Some((colors.len() - 1) as u8)
            }
            _ => None,
        };
        if let Some(ti) = ti {
            doc.set_palette(Palette::new(colors)?);
            doc.set_transparent_index(ti);
            doc.set_color_mode(ColorMode::Indexed);
        }
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::LayerId;

    fn flat(d: &Document, f: usize) -> Vec<Rgba> {
        composite_frame(d, f, &CompositeOptions::EXPORT)
    }

    fn animation(mode: ColorMode) -> Document {
        let mut d = Document::new(20, 12, mode).unwrap();
        let layer = d.layers()[0].id;
        d.insert_frame(1, 40).unwrap();
        d.insert_frame(2, 250).unwrap();
        for f in 0..3 {
            let t = d.target(layer, f).unwrap();
            for i in 0..30 {
                let raw = match t.raster {
                    Raster::Rgba(_) => t.palette.get(1 + (i % 15) as u8).to_u32(),
                    _ => 1 + i % 15,
                };
                t.raster
                    .set_raw((i + f as u32 * 3) as i32 % 20, (i / 3) as i32, raw);
            }
        }
        d
    }

    #[test]
    fn round_trip_keeps_frames_and_durations() {
        for mode in [ColorMode::Rgba, ColorMode::Indexed] {
            let d = animation(mode);
            let out = export_gif(&d, &GifOptions::default()).unwrap();
            assert!(out.warnings.is_empty(), "{:?}", out.warnings);
            let back = import_gif(&out.bytes).unwrap();
            assert_eq!(back.frame_count(), 3);
            assert_eq!(back.mode(), ColorMode::Indexed, "one global palette");
            let ms: Vec<u32> = back.frames().iter().map(|f| f.duration_ms).collect();
            assert_eq!(ms, vec![100, 40, 250]);
            for f in 0..3 {
                assert_eq!(flat(&back, f), flat(&d, f), "{mode:?} frame {f}");
            }
        }
    }

    #[test]
    fn range_scale_and_short_delays() {
        let mut d = animation(ColorMode::Rgba);
        d.set_frame_duration(2, 4).unwrap();
        let opts = GifOptions {
            frames: Some(1..3),
            scale: 3,
            repeat: Some(2),
        };
        let back = import_gif(&export_gif(&d, &opts).unwrap().bytes).unwrap();
        assert_eq!(
            (back.width(), back.height(), back.frame_count()),
            (60, 36, 2)
        );
        assert_eq!(back.frames()[1].duration_ms, 20, "4 ms rounds up to 20 ms");
        let src = flat(&d, 1);
        let big = flat(&back, 0);
        assert_eq!(big[(7 * 60 + 5) as usize], src[(2 * 20 + 1) as usize]);
        assert!(
            export_gif(
                &d,
                &GifOptions {
                    frames: Some(2..5),
                    ..GifOptions::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn many_colors_reduce_by_median_cut() {
        let mut d = Document::new(64, 64, ColorMode::Rgba).unwrap();
        let layer: LayerId = d.layers()[0].id;
        let t = d.target(layer, 0).unwrap();
        for p in Rect::new(0, 0, 64, 64).points() {
            let c = Rgba::opaque(p.x as u8 * 4, p.y as u8 * 4, 128);
            t.raster.set_raw(p.x, p.y, c.to_u32());
        }
        let out = export_gif(&d, &GifOptions::default()).unwrap();
        assert_eq!(
            out.warnings,
            vec![GifWarning::ColorsReduced { colors: 4096 }]
        );
        let back = import_gif(&out.bytes).unwrap();
        assert!(back.palette().len() <= 256);
        let (a, b) = (flat(&d, 0), flat(&back, 0));
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| {
                x.r.abs_diff(y.r)
                    .max(x.g.abs_diff(y.g))
                    .max(x.b.abs_diff(y.b))
            })
            .max()
            .unwrap();
        assert!(worst <= 8, "worst channel error {worst}");
    }

    #[test]
    fn median_cut_is_exact_when_it_fits_and_deterministic() {
        let hist = vec![([0, 0, 0], 5), ([255, 0, 0], 1), ([0, 0, 255], 9)];
        assert_eq!(
            median_cut(&hist, 4),
            vec![[0, 0, 0], [255, 0, 0], [0, 0, 255]]
        );
        let many: Vec<(Rgb, u32)> = (0..=255u8)
            .map(|v| ([v, 255 - v, v / 2], 1 + v as u32 % 3))
            .collect();
        let a = median_cut(&many, 16);
        assert_eq!(a.len(), 16);
        assert_eq!(a, median_cut(&many, 16));
    }

    #[test]
    fn semi_transparent_pixels_flatten() {
        let mut d = Document::new(4, 4, ColorMode::Rgba).unwrap();
        let layer = d.layers()[0].id;
        let t = d.target(layer, 0).unwrap();
        t.raster.set_raw(0, 0, Rgba::new(255, 0, 0, 200).to_u32());
        t.raster.set_raw(1, 0, Rgba::new(0, 255, 0, 60).to_u32());
        let out = export_gif(&d, &GifOptions::default()).unwrap();
        assert_eq!(out.warnings, vec![GifWarning::AlphaFlattened]);
        let back = import_gif(&out.bytes).unwrap();
        let px = flat(&back, 0);
        assert_eq!(px[0], Rgba::opaque(255, 0, 0));
        assert_eq!(px[1].a, 0);
    }

    /// A GIF with a partial second frame and each disposal method.
    fn disposal_gif(dispose: DisposalMethod) -> Vec<u8> {
        let palette = [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255];
        let mut out = Vec::new();
        {
            let mut enc = Encoder::new(&mut out, 4, 4, &palette).unwrap();
            let base = GifFrame {
                width: 4,
                height: 4,
                buffer: Cow::Owned(vec![1; 16]),
                dispose,
                delay: 5,
                ..GifFrame::default()
            };
            enc.write_frame(&base).unwrap();
            let patch = GifFrame {
                left: 1,
                top: 1,
                width: 2,
                height: 2,
                buffer: Cow::Owned(vec![2, 0, 2, 2]),
                transparent: Some(0),
                delay: 0,
                ..GifFrame::default()
            };
            enc.write_frame(&patch).unwrap();
            enc.write_frame(&GifFrame {
                width: 1,
                height: 1,
                buffer: Cow::Owned(vec![3]),
                ..GifFrame::default()
            })
            .unwrap();
        }
        out
    }

    #[test]
    fn import_applies_partial_frames_and_disposal() {
        let red = Rgba::opaque(255, 0, 0);
        let green = Rgba::opaque(0, 255, 0);
        let blue = Rgba::opaque(0, 0, 255);
        let keep = import_gif(&disposal_gif(DisposalMethod::Keep)).unwrap();
        assert_eq!(keep.frames()[0].duration_ms, 50);
        assert_eq!(
            keep.frames()[1].duration_ms,
            100,
            "a 0 delay plays as 100 ms"
        );
        let f1 = flat(&keep, 1);
        assert_eq!((f1[0], f1[5], f1[6], f1[10]), (red, green, red, green));
        let f2 = flat(&keep, 2);
        assert_eq!((f2[0], f2[5], f2[15]), (blue, green, red));
        // Background disposal clears the first frame before the patch.
        let bg = import_gif(&disposal_gif(DisposalMethod::Background)).unwrap();
        let f1 = flat(&bg, 1);
        assert_eq!((f1[0].a, f1[5], f1[6].a), (0, green, 0));
        // Previous disposal of the first frame restores the empty canvas.
        let prev = import_gif(&disposal_gif(DisposalMethod::Previous)).unwrap();
        assert_eq!(flat(&prev, 1)[0].a, 0);
    }

    fn rng(mut s: u64) -> impl FnMut() -> u64 {
        move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        }
    }

    #[test]
    fn truncated_and_corrupt_gifs_never_panic() {
        let bytes = export_gif(&animation(ColorMode::Rgba), &GifOptions::default())
            .unwrap()
            .bytes;
        for len in 0..bytes.len() {
            assert!(import_gif(&bytes[..len]).is_err(), "prefix {len}");
        }
        let mut next = rng(0x0bad_cafe_f00d_1234);
        for _ in 0..3000 {
            let mut b = bytes.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() % b.len() as u64) as usize;
                b[i] ^= 1 << (next() % 8);
            }
            let _ = import_gif(&b);
        }
    }
}
