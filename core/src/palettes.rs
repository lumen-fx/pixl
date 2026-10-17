//! Palette work: reading and writing palette files, sorting, building ramps,
//! making a palette from an image, and putting an image on a palette with a
//! choice of dithering.
//!
//! The files pixl reads and writes are GIMP's `.gpl`, JASC `.pal` (and reads
//! the binary RIFF `.pal` too), `.hex` (one `rrggbb` per line, as Lospec
//! serves them) and Adobe Swatch Exchange `.ase`.

use std::collections::HashMap;

use crate::color::{Hsl, Palette, Rgba};
use crate::document::{Document, ImageId};
use crate::error::Error;
use crate::geom::Rect;
use crate::image::{Raster, TiledImage};
use crate::progress::Progress;
use crate::tools::Bayer;

/// A palette file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteFormat {
    /// GIMP palette.
    Gpl,
    /// JASC (Paint Shop Pro) palette; RIFF palettes read too.
    Pal,
    /// One `rrggbb` per line.
    Hex,
    /// Adobe Swatch Exchange.
    Ase,
}

impl PaletteFormat {
    pub const ALL: [PaletteFormat; 4] = [
        PaletteFormat::Gpl,
        PaletteFormat::Pal,
        PaletteFormat::Hex,
        PaletteFormat::Ase,
    ];

    /// The format a file extension names.
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "gpl" => PaletteFormat::Gpl,
            "pal" => PaletteFormat::Pal,
            "hex" | "txt" => PaletteFormat::Hex,
            "ase" | "aco" => PaletteFormat::Ase,
            _ => return None,
        })
    }

    pub fn extension(self) -> &'static str {
        match self {
            PaletteFormat::Gpl => "gpl",
            PaletteFormat::Pal => "pal",
            PaletteFormat::Hex => "hex",
            PaletteFormat::Ase => "ase",
        }
    }
}

fn bad(msg: impl Into<String>) -> Error {
    Error::Format(msg.into())
}

/// Reads a palette file, telling the format from its content where the
/// content says (`GIMP Palette`, `JASC-PAL`, `RIFF`, `ASEF`) and from
/// `hint` otherwise.
pub fn read_palette(bytes: &[u8], hint: Option<PaletteFormat>) -> Result<Palette, Error> {
    let colors = if bytes.starts_with(b"ASEF") {
        read_ase(bytes)?
    } else if bytes.starts_with(b"RIFF") {
        read_riff_pal(bytes)?
    } else {
        let text = std::str::from_utf8(bytes).map_err(|_| bad("the palette is not text"))?;
        let first = text.lines().next().unwrap_or("").trim();
        if first.starts_with("GIMP Palette") {
            read_gpl(text)?
        } else if first.starts_with("JASC-PAL") {
            read_jasc(text)?
        } else {
            match hint {
                Some(PaletteFormat::Gpl) => read_gpl(text)?,
                Some(PaletteFormat::Pal) => read_jasc(text)?,
                _ => read_hex(text)?,
            }
        }
    };
    if colors.is_empty() {
        return Err(bad("the palette has no colors"));
    }
    colors.truncate_to_palette()
}

trait TruncateToPalette {
    fn truncate_to_palette(self) -> Result<Palette, Error>;
}

impl TruncateToPalette for Vec<Rgba> {
    fn truncate_to_palette(mut self) -> Result<Palette, Error> {
        self.truncate(Palette::MAX);
        Palette::new(self)
    }
}

fn byte(field: &str) -> Result<u8, Error> {
    field
        .parse::<u32>()
        .ok()
        .filter(|v| *v <= 255)
        .map(|v| v as u8)
        .ok_or_else(|| bad(format!("{field:?} is not a color channel")))
}

fn read_gpl(text: &str) -> Result<Vec<Rgba>, Error> {
    let mut alpha = false;
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(ch) = line.strip_prefix("Channels:") {
            alpha = ch.trim().eq_ignore_ascii_case("rgba");
            continue;
        }
        if line.contains(':') && !line.starts_with(|c: char| c.is_ascii_digit()) {
            // Name:, Columns: and other headers.
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 {
            return Err(bad(format!("{line:?} is not a palette entry")));
        }
        let a = if alpha && f.len() >= 4 {
            byte(f[3])?
        } else {
            255
        };
        out.push(Rgba::new(byte(f[0])?, byte(f[1])?, byte(f[2])?, a));
    }
    Ok(out)
}

fn read_jasc(text: &str) -> Result<Vec<Rgba>, Error> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.next();
    lines.next(); // version
    let count: usize = lines
        .next()
        .and_then(|l| l.parse().ok())
        .ok_or_else(|| bad("the JASC palette has no color count"))?;
    let mut out = Vec::with_capacity(count);
    for line in lines.take(count) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 {
            return Err(bad(format!("{line:?} is not a palette entry")));
        }
        let a = if f.len() >= 4 { byte(f[3])? } else { 255 };
        out.push(Rgba::new(byte(f[0])?, byte(f[1])?, byte(f[2])?, a));
    }
    Ok(out)
}

fn read_hex(text: &str) -> Result<Vec<Rgba>, Error> {
    let mut out = Vec::new();
    for line in text.lines() {
        let h = line.trim().trim_start_matches('#');
        if h.is_empty() || h.starts_with(';') {
            continue;
        }
        let ch = |i: usize| {
            h.get(i..i + 2)
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or_else(|| bad(format!("{line:?} is not a hex color")))
        };
        match h.len() {
            6 => out.push(Rgba::opaque(ch(0)?, ch(2)?, ch(4)?)),
            8 => out.push(Rgba::new(ch(0)?, ch(2)?, ch(4)?, ch(6)?)),
            _ => return Err(bad(format!("{line:?} is not a hex color"))),
        }
    }
    Ok(out)
}

fn read_riff_pal(b: &[u8]) -> Result<Vec<Rgba>, Error> {
    if b.len() < 24 || &b[8..12] != b"PAL " || &b[12..16] != b"data" {
        return Err(bad("not a RIFF palette"));
    }
    let count = u16::from_le_bytes([b[22], b[23]]) as usize;
    let data = &b[24..];
    if data.len() < count * 4 {
        return Err(bad("the RIFF palette is truncated"));
    }
    Ok(data
        .chunks_exact(4)
        .take(count)
        .map(|e| Rgba::opaque(e[0], e[1], e[2]))
        .collect())
}

fn read_ase(b: &[u8]) -> Result<Vec<Rgba>, Error> {
    let mut at = 12;
    let u16_at = |i: usize| -> Result<u16, Error> {
        b.get(i..i + 2)
            .map(|s| u16::from_be_bytes([s[0], s[1]]))
            .ok_or_else(|| bad("the swatch file is truncated"))
    };
    let u32_at = |i: usize| -> Result<u32, Error> {
        b.get(i..i + 4)
            .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
            .ok_or_else(|| bad("the swatch file is truncated"))
    };
    let f32_at = |i: usize| -> Result<f32, Error> { u32_at(i).map(f32::from_bits) };
    let blocks = u32_at(8)?;
    let mut out = Vec::new();
    for _ in 0..blocks {
        let kind = u16_at(at)?;
        let len = u32_at(at + 2)? as usize;
        let body = at + 6;
        if kind == 0x0001 {
            let name_len = u16_at(body)? as usize;
            let model_at = body + 2 + name_len * 2;
            let model = b
                .get(model_at..model_at + 4)
                .ok_or_else(|| bad("the swatch file is truncated"))?;
            let v = model_at + 4;
            let unit = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
            let color = match model {
                b"RGB " => Some(Rgba::opaque(
                    unit(f32_at(v)?),
                    unit(f32_at(v + 4)?),
                    unit(f32_at(v + 8)?),
                )),
                b"Gray" => {
                    let g = unit(f32_at(v)?);
                    Some(Rgba::opaque(g, g, g))
                }
                b"CMYK" => {
                    let (c, m, y, k) =
                        (f32_at(v)?, f32_at(v + 4)?, f32_at(v + 8)?, f32_at(v + 12)?);
                    let ch = |x: f32| unit((1.0 - x) * (1.0 - k));
                    Some(Rgba::opaque(ch(c), ch(m), ch(y)))
                }
                // Lab swatches need a white point to convert; they are skipped.
                _ => None,
            };
            out.extend(color);
        }
        at = body + len;
    }
    Ok(out)
}

/// Writes a palette in `format`, named `name` where the format keeps a name.
pub fn write_palette(palette: &Palette, format: PaletteFormat, name: &str) -> Vec<u8> {
    let colors = palette.colors();
    let alpha = colors.iter().any(|c| c.a != 255);
    match format {
        PaletteFormat::Gpl => {
            let mut s = String::from("GIMP Palette\n");
            if alpha {
                s.push_str("Channels: RGBA\n");
            }
            s.push_str(&format!("Name: {name}\nColumns: 16\n#\n"));
            for c in colors {
                if alpha {
                    s.push_str(&format!("{} {} {} {}\n", c.r, c.g, c.b, c.a));
                } else {
                    s.push_str(&format!("{} {} {}\n", c.r, c.g, c.b));
                }
            }
            s.into_bytes()
        }
        PaletteFormat::Pal => {
            let mut s = format!("JASC-PAL\r\n0100\r\n{}\r\n", colors.len());
            for c in colors {
                s.push_str(&format!("{} {} {}\r\n", c.r, c.g, c.b));
            }
            s.into_bytes()
        }
        PaletteFormat::Hex => colors
            .iter()
            .map(|c| format!("{:02x}{:02x}{:02x}\n", c.r, c.g, c.b))
            .collect::<String>()
            .into_bytes(),
        PaletteFormat::Ase => {
            let mut out = Vec::new();
            out.extend_from_slice(b"ASEF");
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&(colors.len() as u32).to_be_bytes());
            for (i, c) in colors.iter().enumerate() {
                let label: Vec<u16> = format!("{name} {i}").encode_utf16().chain([0]).collect();
                let mut body = Vec::new();
                body.extend_from_slice(&(label.len() as u16).to_be_bytes());
                for u in &label {
                    body.extend_from_slice(&u.to_be_bytes());
                }
                body.extend_from_slice(b"RGB ");
                for v in [c.r, c.g, c.b] {
                    body.extend_from_slice(&(v as f32 / 255.0).to_bits().to_be_bytes());
                }
                body.extend_from_slice(&2u16.to_be_bytes());
                out.extend_from_slice(&1u16.to_be_bytes());
                out.extend_from_slice(&(body.len() as u32).to_be_bytes());
                out.extend_from_slice(&body);
            }
            out
        }
    }
}

/// What a palette sort orders by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Hue,
    Saturation,
    Lightness,
    /// Perceived brightness (Rec. 601 luma).
    Luma,
}

/// The order of `palette`'s entries sorted by `key`: entry `order[i]` moves
/// to `i`. Fully transparent entries stay in front, in their own order.
pub fn sort_order(palette: &Palette, key: SortKey) -> Vec<u8> {
    let mut order: Vec<u8> = (0..palette.len() as u16).map(|i| i as u8).collect();
    let k = |i: u8| -> (bool, i64) {
        let c = palette.get(i);
        let hsl = c.to_hsl();
        let v = match key {
            SortKey::Hue => (hsl.h * 100.0) as i64 * 1000 + (hsl.l * 999.0) as i64,
            SortKey::Saturation => (hsl.s * 1e6) as i64,
            SortKey::Lightness => (hsl.l * 1e6) as i64,
            SortKey::Luma => c.luma() as i64,
        };
        (c.a != 0, v)
    };
    order.sort_by_key(|&i| k(i));
    order
}

/// Sorts the document palette by `key`. Indexed pixels keep their colors:
/// every image is remapped to the new indices, and the transparent index
/// follows its entry.
pub fn sort_palette(doc: &mut Document, key: SortKey) -> Rect {
    let order = sort_order(doc.palette(), key);
    reorder_palette(doc, &order)
}

/// Reorders the palette so entry `order[i]` moves to `i`, remapping indexed
/// pixels so the picture does not change.
pub fn reorder_palette(doc: &mut Document, order: &[u8]) -> Rect {
    let old = doc.palette().clone();
    let colors: Vec<Rgba> = order.iter().map(|&i| old.get(i)).collect();
    let mut to_new = [0u8; 256];
    for (new, &was) in order.iter().enumerate() {
        to_new[was as usize] = new as u8;
    }
    let ti = to_new[doc.transparent_index() as usize];
    doc.s.palette = Palette::new(colors).expect("the same number of colors");
    doc.s.transparent_index = ti;
    for raster in doc.images.values_mut() {
        if let Raster::Indexed(img) = raster {
            *img = img.map(to_new[img.fill() as usize], |i| to_new[i as usize]);
        }
    }
    doc.bounds()
}

/// How [`ramp`] builds a color ramp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RampOptions {
    pub steps: usize,
    /// Lightness of the darkest and the lightest step, 0 to 1.
    pub lightness: (f32, f32),
    /// Degrees the hue turns from the darkest step to the lightest: the
    /// usual pixel art ramp cools its shadows and warms its highlights.
    pub hue_shift: f32,
    /// Added to the saturation at both ends of the ramp and nothing in its
    /// middle; negative values dull the darkest and the lightest steps.
    pub saturation_shift: f32,
}

impl Default for RampOptions {
    fn default() -> Self {
        Self {
            steps: 5,
            lightness: (0.15, 0.85),
            hue_shift: 40.0,
            saturation_shift: -0.15,
        }
    }
}

/// A ramp of colors around `base`, darkest first.
pub fn ramp(base: Rgba, o: &RampOptions) -> Vec<Rgba> {
    let hsl = base.to_hsl();
    let n = o.steps.max(1);
    (0..n)
        .map(|i| {
            let u = if n == 1 {
                0.5
            } else {
                i as f32 / (n - 1) as f32
            };
            let l = o.lightness.0 + (o.lightness.1 - o.lightness.0) * u;
            // Hues turn in the direction of the warm side for highlights:
            // toward yellow (60) from either side of the wheel.
            let toward_warm = if (hsl.h - 60.0).rem_euclid(360.0) < 180.0 {
                -1.0
            } else {
                1.0
            };
            let h = hsl.h + toward_warm * o.hue_shift * (u - 0.5);
            let s = hsl.s + o.saturation_shift * (2.0 * u - 1.0).abs();
            Rgba::from_hsl(Hsl { h, s, l }, base.a)
        })
        .collect()
}

pub(crate) type Rgb = [u8; 3];

/// Median cut (Heckbert, "Color Image Quantization for Frame Buffer
/// Display", SIGGRAPH 1982): splits the color histogram into at most `max`
/// boxes and returns each box's weighted mean. The box split next is the one
/// with the widest range on any channel; it is cut at the weighted median of
/// that channel.
pub(crate) fn median_cut(histogram: &[(Rgb, u32)], max: usize) -> Vec<Rgb> {
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

/// A palette of at most `max` colors for `pixels` (pixels with alpha below
/// 128 are left out), by median cut weighted by how often each color shows.
/// With `transparent_first` the palette starts with a transparent entry, as
/// an indexed document's palette does, and holds `max - 1` colors besides.
pub fn palette_from_pixels(
    pixels: impl IntoIterator<Item = Rgba>,
    max: usize,
    transparent_first: bool,
) -> Palette {
    let mut histogram: HashMap<Rgb, u32> = HashMap::new();
    for c in pixels {
        if c.a >= 128 {
            *histogram.entry([c.r, c.g, c.b]).or_insert(0) += 1;
        }
    }
    let mut hist: Vec<(Rgb, u32)> = histogram.into_iter().collect();
    hist.sort_unstable();
    let cap = max.clamp(1, Palette::MAX) - usize::from(transparent_first);
    let mut colors: Vec<Rgba> = Vec::new();
    if transparent_first {
        colors.push(Rgba::TRANSPARENT);
    }
    colors.extend(
        median_cut(&hist, cap.max(1))
            .into_iter()
            .map(|[r, g, b]| Rgba::opaque(r, g, b)),
    );
    Palette::new(colors).expect("at most 256 colors")
}

/// How an image is put on a palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dither {
    /// Each pixel takes the nearest palette color.
    #[default]
    None,
    /// An ordered (Bayer) dither: a pixel between two palette colors takes
    /// one or the other by the matrix, in a fixed pattern.
    Ordered(Bayer),
    /// Floyd-Steinberg error diffusion.
    FloydSteinberg,
}

/// How far an ordered dither moves a channel at most, either way.
const ORDERED_SPREAD: f32 = 24.0;

/// Puts an RGBA image on `palette`: pixels with alpha 0 become `ti`, the
/// rest the palette entry `dither` picks (never `ti`).
pub fn index_image(
    img: &TiledImage<Rgba>,
    palette: &Palette,
    ti: u8,
    dither: Dither,
) -> TiledImage<u8> {
    let mut cache: HashMap<Rgba, u8> = HashMap::new();
    let mut nearest = |c: Rgba| -> u8 {
        *cache
            .entry(c)
            .or_insert_with(|| palette.nearest(Rgba::new(c.r, c.g, c.b, 255), Some(ti)))
    };
    match dither {
        Dither::None => img.map(ti, |c| if c.a == 0 { ti } else { nearest(c) }),
        Dither::Ordered(m) => {
            let (w, h) = (img.width() as i32, img.height() as i32);
            let cells = (m.size() * m.size()) as f32;
            let mut out = TiledImage::new(img.width(), img.height(), ti);
            for y in 0..h {
                for x in 0..w {
                    let c = img.get(x, y);
                    if c.a == 0 {
                        continue;
                    }
                    let t = (m.threshold(x, y) as f32 + 0.5) / cells - 0.5;
                    let d = t * 2.0 * ORDERED_SPREAD;
                    let ch = |v: u8| (v as f32 + d).round().clamp(0.0, 255.0) as u8;
                    out.set(x, y, nearest(Rgba::new(ch(c.r), ch(c.g), ch(c.b), 255)));
                }
            }
            out
        }
        Dither::FloydSteinberg => {
            let (w, h) = (img.width() as usize, img.height() as usize);
            let mut out = TiledImage::new(img.width(), img.height(), ti);
            let mut cur = vec![[0f32; 3]; w + 2];
            let mut next = vec![[0f32; 3]; w + 2];
            for y in 0..h {
                for x in 0..w {
                    let c = img.get(x as i32, y as i32);
                    if c.a == 0 {
                        continue;
                    }
                    let e = cur[x + 1];
                    let want = [
                        (c.r as f32 + e[0]).clamp(0.0, 255.0),
                        (c.g as f32 + e[1]).clamp(0.0, 255.0),
                        (c.b as f32 + e[2]).clamp(0.0, 255.0),
                    ];
                    let q = Rgba::opaque(
                        want[0].round() as u8,
                        want[1].round() as u8,
                        want[2].round() as u8,
                    );
                    let i = nearest(q);
                    out.set(x as i32, y as i32, i);
                    let got = palette.get(i);
                    let err = [
                        want[0] - got.r as f32,
                        want[1] - got.g as f32,
                        want[2] - got.b as f32,
                    ];
                    for k in 0..3 {
                        cur[x + 2][k] += err[k] * 7.0 / 16.0;
                        next[x][k] += err[k] * 3.0 / 16.0;
                        next[x + 1][k] += err[k] * 5.0 / 16.0;
                        next[x + 2][k] += err[k] / 16.0;
                    }
                }
                std::mem::swap(&mut cur, &mut next);
                next.iter_mut().for_each(|e| *e = [0.0; 3]);
            }
            out
        }
    }
}

/// Puts the whole document on `palette` as an indexed document: RGBA images
/// are indexed with `dither`, and an indexed document's images are remapped
/// from its old palette by color. The transparent index becomes the
/// palette's first fully transparent entry, or a new transparent entry at
/// the end when there is none and room for one.
pub fn reduce(doc: &mut Document, palette: Palette, dither: Dither) -> Result<Rect, Error> {
    let never = Progress::default();
    reduce_with(doc, palette, dither, 1, &never).map(|r| r.unwrap_or_else(|| doc.bounds()))
}

/// [`reduce`], indexing the document's images on up to `threads` threads
/// side by side and counting each one done in `progress`. Once `progress`
/// is stopped it ends at the next image and returns `Ok(None)` with the
/// document untouched.
pub fn reduce_with(
    doc: &mut Document,
    palette: Palette,
    dither: Dither,
    threads: usize,
    progress: &Progress,
) -> Result<Option<Rect>, Error> {
    let mut palette = palette;
    let ti = match palette.colors().iter().position(|c| c.a == 0) {
        Some(i) => i as u8,
        None => palette.push(Rgba::TRANSPARENT)?,
    };
    let old = doc.palette().clone();
    let old_ti = doc.transparent_index();
    let refs = doc.reference_images();
    let work: Vec<(ImageId, TiledImage<Rgba>)> = doc
        .images
        .iter()
        .filter(|(id, _)| !refs.contains(id))
        .filter_map(|(id, raster)| {
            let rgba = match raster {
                Raster::Rgba(img) => img.clone(),
                Raster::Indexed(img) => img.map(Rgba::TRANSPARENT, |i| {
                    if i == old_ti {
                        Rgba::TRANSPARENT
                    } else {
                        old.get(i)
                    }
                }),
                Raster::Mask(_) => return None,
            };
            Some((*id, rgba))
        })
        .collect();
    progress.set_total(work.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::Mutex::new(Vec::with_capacity(work.len()));
    let index = || {
        while !progress.stopped() {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some((id, rgba)) = work.get(i) else {
                return;
            };
            let indexed = index_image(rgba, &palette, ti, dither);
            done.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((*id, indexed));
            progress.step();
        }
    };
    let threads = threads.clamp(1, work.len().max(1));
    if threads == 1 {
        index();
    } else {
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(index);
            }
        });
    }
    if progress.stopped() {
        return Ok(None);
    }
    let done = done
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (id, indexed) in done {
        doc.images.insert(id, Raster::Indexed(indexed));
    }
    doc.s.palette = palette;
    doc.s.transparent_index = ti;
    doc.s.mode = crate::color::ColorMode::Indexed;
    Ok(Some(doc.bounds()))
}

/// Replaces `from` with `to` (within `tolerance`) in every cel of every
/// raster layer whose pixels are not locked, over the frames in `frames`.
/// Linked cels change once. Returns the changed area of the canvas. Run it
/// in a [`crate::History`] transaction with [`crate::Scope::All`] to make it
/// one undo step.
pub fn replace_color_everywhere(
    doc: &mut Document,
    from: crate::Color,
    to: crate::Color,
    tolerance: u8,
    frames: std::ops::Range<usize>,
) -> Rect {
    let layers: Vec<crate::LayerId> = doc
        .walk()
        .into_iter()
        .filter(|(_, l)| l.is_raster())
        .map(|(_, l)| l.id)
        .collect();
    let ctx = crate::ToolContext::default();
    let mut done: Vec<crate::ImageId> = Vec::new();
    let mut dirty = Rect::EMPTY;
    for id in layers {
        if doc.effective_locks(id).is_ok_and(|l| l.pixels) {
            continue;
        }
        let count = doc.frame_count();
        for f in frames.clone().filter(|f| *f < count) {
            let Ok(Some(cel)) = doc.cel(id, f) else {
                continue;
            };
            if done.contains(&cel.image) {
                continue;
            }
            done.push(cel.image);
            let Ok(target) = doc.target(id, f) else {
                continue;
            };
            let mut canvas = crate::Canvas::new(target, &ctx);
            crate::tools::paint::replace_color(&mut canvas, from, to, tolerance);
            dirty = dirty.union(&canvas.dirty());
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorMode;
    use crate::geom::Point;

    fn pal(colors: &[(u8, u8, u8)]) -> Palette {
        Palette::new(
            colors
                .iter()
                .map(|&(r, g, b)| Rgba::opaque(r, g, b))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn replacing_everywhere_skips_locked_layers_and_other_frames() {
        let mut d = Document::new(4, 4, ColorMode::Rgba).unwrap();
        let a = d.layers()[0].id;
        let b = d
            .add_layer(crate::NewLayer::Raster, "b", crate::Placement::default())
            .unwrap();
        d.insert_frame(1, 100).unwrap();
        let red = Rgba::opaque(255, 0, 0);
        for (l, f) in [(a, 0), (b, 0), (a, 1)] {
            d.target(l, f).unwrap().raster.set_raw(1, 1, red.to_u32());
        }
        d.update_layer(b, |p| p.locks.pixels = true).unwrap();
        let dirty = replace_color_everywhere(&mut d, red.into(), Rgba::WHITE.into(), 0, 0..1);
        assert_eq!(dirty, Rect::pixel(1, 1));
        let px = |d: &Document, l, f| d.cel_raster(l, f).unwrap().unwrap().get_raw(1, 1);
        assert_eq!(px(&d, a, 0), Rgba::WHITE.to_u32());
        assert_eq!(px(&d, b, 0), red.to_u32(), "locked");
        assert_eq!(px(&d, a, 1), red.to_u32(), "outside the range");
    }

    #[test]
    fn every_format_round_trips() {
        let p = pal(&[(0, 0, 0), (255, 128, 1), (17, 34, 51)]);
        for f in PaletteFormat::ALL {
            let bytes = write_palette(&p, f, "test");
            let back = read_palette(&bytes, Some(f)).unwrap();
            assert_eq!(back, p, "{f:?}");
        }
        let alpha = Palette::new(vec![Rgba::TRANSPARENT, Rgba::WHITE]).unwrap();
        let gpl = write_palette(&alpha, PaletteFormat::Gpl, "a");
        assert_eq!(read_palette(&gpl, None).unwrap(), alpha);
    }

    #[test]
    fn foreign_files_read() {
        let gpl =
            "GIMP Palette\nName: Lospec\nColumns: 4\n#\n  0   0   0\tBlack\n255 255 255\tWhite\n";
        assert_eq!(
            read_palette(gpl.as_bytes(), None).unwrap(),
            pal(&[(0, 0, 0), (255, 255, 255)])
        );
        let hex = "#ff0000\n00FF00\n\n";
        assert_eq!(
            read_palette(hex.as_bytes(), Some(PaletteFormat::Hex)).unwrap(),
            pal(&[(255, 0, 0), (0, 255, 0)])
        );
        let mut riff = b"RIFF\0\0\0\0PAL data\0\0\0\0\0\x03\x02\0".to_vec();
        riff.extend([1, 2, 3, 0, 4, 5, 6, 0]);
        assert_eq!(
            read_palette(&riff, None).unwrap(),
            pal(&[(1, 2, 3), (4, 5, 6)])
        );
        assert!(read_palette(b"zz", Some(PaletteFormat::Hex)).is_err());
    }

    #[test]
    fn sorting_keeps_the_picture() {
        let mut d = Document::new(4, 1, ColorMode::Indexed).unwrap();
        let l = d.layers()[0].id;
        let p = Palette::new(vec![
            Rgba::TRANSPARENT,
            Rgba::WHITE,
            Rgba::BLACK,
            Rgba::opaque(128, 128, 128),
        ])
        .unwrap();
        d.set_palette(p);
        for x in 0..3 {
            d.target(l, 0).unwrap().raster.set_raw(x, 0, x as u32 + 1);
        }
        let before: Vec<Rgba> = (0..4)
            .map(|x| {
                d.palette()
                    .get(d.cel_raster(l, 0).unwrap().unwrap().get_raw(x, 0) as u8)
            })
            .collect();
        sort_palette(&mut d, SortKey::Lightness);
        assert_eq!(
            d.palette().colors(),
            &[
                Rgba::TRANSPARENT,
                Rgba::BLACK,
                Rgba::opaque(128, 128, 128),
                Rgba::WHITE
            ]
        );
        assert_eq!(d.transparent_index(), 0);
        let after: Vec<Rgba> = (0..4)
            .map(|x| {
                d.palette()
                    .get(d.cel_raster(l, 0).unwrap().unwrap().get_raw(x, 0) as u8)
            })
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn ramps_go_dark_to_light_and_turn_the_hue() {
        let base = Rgba::opaque(200, 60, 60);
        let r = ramp(base, &RampOptions::default());
        assert_eq!(r.len(), 5);
        for w in r.windows(2) {
            assert!(w[0].to_hsl().l < w[1].to_hsl().l);
        }
        let (dark, light) = (r[0].to_hsl().h, r[4].to_hsl().h);
        // Red's highlights turn toward yellow.
        let toward = |h: f32| {
            (h - 60.0)
                .rem_euclid(360.0)
                .min((60.0 - h).rem_euclid(360.0))
        };
        assert!(toward(light) < toward(dark), "{dark} {light}");
        let flat = ramp(
            base,
            &RampOptions {
                hue_shift: 0.0,
                saturation_shift: 0.0,
                ..RampOptions::default()
            },
        );
        assert!(
            flat.iter()
                .all(|c| (c.to_hsl().h - base.to_hsl().h).abs() < 2.0)
        );
    }

    #[test]
    fn a_palette_from_an_image_keeps_its_colors_when_they_fit() {
        let px = [Rgba::WHITE, Rgba::WHITE, Rgba::BLACK, Rgba::TRANSPARENT];
        let p = palette_from_pixels(px, 16, true);
        assert_eq!(p.colors(), &[Rgba::TRANSPARENT, Rgba::BLACK, Rgba::WHITE]);
        let many = (0..=255u8).map(|v| Rgba::opaque(v, v, v));
        assert_eq!(palette_from_pixels(many, 8, false).len(), 8);
    }

    fn gradient_doc() -> Document {
        let mut d = Document::new(64, 4, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        let t = d.target(l, 0).unwrap();
        for y in 0..4 {
            for x in 0..64 {
                let v = (x * 4) as u8;
                t.raster.set_raw(x, y, Rgba::opaque(v, v, v).to_u32());
            }
        }
        d.target(l, 0).unwrap().raster.set_raw(0, 0, 0);
        d
    }

    #[test]
    fn reducing_with_each_dither() {
        let two = pal(&[(0, 0, 0), (255, 255, 255)]);
        let mut counts = Vec::new();
        for dither in [
            Dither::None,
            Dither::Ordered(Bayer::B4),
            Dither::FloydSteinberg,
        ] {
            let mut d = gradient_doc();
            let l = d.layers()[0].id;
            reduce(&mut d, two.clone(), dither).unwrap();
            assert_eq!(d.mode(), ColorMode::Indexed);
            assert_eq!(d.transparent_index(), 2, "a transparent entry is added");
            let r = d.cel_raster(l, 0).unwrap().unwrap();
            assert_eq!(r.get_raw(0, 0), 2, "transparent stays transparent");
            // Count the white pixels in the middle of the ramp.
            let mid = Rect::new(24, 0, 16, 4)
                .points()
                .filter(|p: &Point| r.get_raw(p.x, p.y) == 1)
                .count();
            counts.push(mid);
        }
        // Without dithering the middle splits in two halves; dithering mixes
        // black and white across it.
        assert_eq!(counts[0], 32);
        let mixed = |d: &Document| {
            let l = d.layers()[0].id;
            let r = d.cel_raster(l, 0).unwrap().unwrap();
            (24..40)
                .filter(|&x| r.get_raw(x, 1) != r.get_raw(x + 1, 1))
                .count()
        };
        let mut ordered = gradient_doc();
        reduce(&mut ordered, two.clone(), Dither::Ordered(Bayer::B4)).unwrap();
        assert!(mixed(&ordered) > 4);
        let mut fs = gradient_doc();
        reduce(&mut fs, two, Dither::FloydSteinberg).unwrap();
        assert!(mixed(&fs) > 4);
    }

    #[test]
    fn reducing_on_several_threads_matches_one_and_a_stopped_one_changes_nothing() {
        use crate::document::{NewLayer, Placement};
        let mut doc = Document::new(64, 64, crate::color::ColorMode::Rgba).unwrap();
        for i in 0..6 {
            let id = if i == 0 {
                doc.layers()[0].id
            } else {
                doc.add_layer(NewLayer::Raster, format!("l{i}"), Placement::default())
                    .unwrap()
            };
            let t = doc.target(id, 0).unwrap();
            for y in 0..64 {
                for x in 0..64 {
                    let c = Rgba::opaque((x * 4) as u8, (y * 4) as u8, (i * 40) as u8);
                    t.raster.set_raw(x, y, c.to_u32());
                }
            }
        }
        let palette =
            Palette::new(vec![Rgba::BLACK, Rgba::WHITE, Rgba::opaque(200, 0, 0)]).unwrap();
        let mut one = doc.clone();
        reduce(&mut one, palette.clone(), Dither::FloydSteinberg).unwrap();
        let mut many = doc.clone();
        let progress = Progress::default();
        let done = reduce_with(
            &mut many,
            palette.clone(),
            Dither::FloydSteinberg,
            4,
            &progress,
        );
        assert!(done.unwrap().is_some());
        assert_eq!(progress.get(), (6, 6));
        for (_, l) in one.walk() {
            assert_eq!(
                one.cel_raster(l.id, 0).unwrap(),
                many.cel_raster(l.id, 0).unwrap()
            );
        }
        let mut stopped = doc.clone();
        let progress = Progress::default();
        progress.stop();
        let done = reduce_with(&mut stopped, palette, Dither::None, 4, &progress);
        assert!(done.unwrap().is_none());
        assert_eq!(stopped.mode(), crate::color::ColorMode::Rgba);
    }
}
