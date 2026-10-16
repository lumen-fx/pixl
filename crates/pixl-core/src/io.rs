//! PNG import and export, and sprite sheets with Aseprite-compatible JSON.

use std::io::Cursor;
use std::ops::Range;

use serde_json::{Map, Value, json};

use crate::color::{ColorMode, Palette, Rgba};
use crate::composite::{CompositeOptions, composite_frame};
use crate::document::{
    BlendMode, Document, Layer, LayerId, LayerKind, LoopDirection, NewLayer, Placement,
};
use crate::error::Error;
use crate::geom::Rect;
use crate::image::Raster;

fn png_err(e: impl std::fmt::Display) -> Error {
    Error::Image(e.to_string())
}

/// A decoded PNG: color pixels, or palette indices when the file is indexed.
#[derive(Clone, Debug, PartialEq)]
pub enum DecodedPng {
    Rgba {
        width: u32,
        height: u32,
        pixels: Vec<Rgba>,
    },
    Indexed {
        width: u32,
        height: u32,
        indices: Vec<u8>,
        /// Palette entries with their tRNS alpha.
        palette: Vec<Rgba>,
    },
}

impl DecodedPng {
    pub fn size(&self) -> (u32, u32) {
        match self {
            DecodedPng::Rgba { width, height, .. } | DecodedPng::Indexed { width, height, .. } => {
                (*width, *height)
            }
        }
    }

    /// The pixels as colors whatever the file's format.
    pub fn to_rgba(&self) -> Vec<Rgba> {
        match self {
            DecodedPng::Rgba { pixels, .. } => pixels.clone(),
            DecodedPng::Indexed {
                indices, palette, ..
            } => indices
                .iter()
                .map(|i| {
                    palette
                        .get(*i as usize)
                        .copied()
                        .unwrap_or(Rgba::TRANSPARENT)
                })
                .collect(),
        }
    }
}

/// Decodes a PNG of any color type and bit depth. Indexed files keep their
/// indices and palette; everything else becomes 8-bit RGBA.
pub fn decode_png(bytes: &[u8]) -> Result<DecodedPng, Error> {
    let probe = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(png_err)?;
    let info = probe.info();
    if info.color_type == png::ColorType::Indexed {
        let (width, height) = (info.width, info.height);
        let depth = info.bit_depth as u8;
        let rgb = info
            .palette
            .as_deref()
            .ok_or_else(|| png_err("indexed PNG without a palette"))?;
        let trns = info.trns.as_deref().unwrap_or(&[]);
        let palette: Vec<Rgba> = rgb
            .chunks_exact(3)
            .enumerate()
            .map(|(i, c)| Rgba::new(c[0], c[1], c[2], trns.get(i).copied().unwrap_or(255)))
            .collect();
        let mut reader = probe;
        let mut buf = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or_else(|| png_err("image too large"))?
        ];
        let frame = reader.next_frame(&mut buf).map_err(png_err)?;
        let mut indices = Vec::with_capacity(width as usize * height as usize);
        let per_byte = 8 / depth as usize;
        let mask = ((1u16 << depth) - 1) as u8;
        for y in 0..height as usize {
            let row = &buf[y * frame.line_size..(y + 1) * frame.line_size];
            for x in 0..width as usize {
                let byte = row[x / per_byte];
                let shift = 8 - depth as usize * (x % per_byte + 1);
                indices.push((byte >> shift) & mask);
            }
        }
        return Ok(DecodedPng::Indexed {
            width,
            height,
            indices,
            palette,
        });
    }
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(png_err)?;
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| png_err("image too large"))?
    ];
    let frame = reader.next_frame(&mut buf).map_err(png_err)?;
    let (width, height) = (frame.width, frame.height);
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height as usize {
        let row = &buf[y * frame.line_size..(y + 1) * frame.line_size];
        match frame.color_type {
            png::ColorType::Grayscale => pixels.extend(
                row.iter()
                    .take(width as usize)
                    .map(|&v| Rgba::opaque(v, v, v)),
            ),
            png::ColorType::GrayscaleAlpha => pixels.extend(
                row.chunks_exact(2)
                    .take(width as usize)
                    .map(|c| Rgba::new(c[0], c[0], c[0], c[1])),
            ),
            png::ColorType::Rgb => pixels.extend(
                row.chunks_exact(3)
                    .take(width as usize)
                    .map(|c| Rgba::opaque(c[0], c[1], c[2])),
            ),
            png::ColorType::Rgba => pixels.extend(
                row.chunks_exact(4)
                    .take(width as usize)
                    .map(|c| Rgba::new(c[0], c[1], c[2], c[3])),
            ),
            png::ColorType::Indexed => return Err(png_err("unexpected indexed output")),
        }
    }
    Ok(DecodedPng::Rgba {
        width,
        height,
        pixels,
    })
}

/// Encodes 8-bit RGBA pixels as a PNG.
pub fn encode_png_rgba(width: u32, height: u32, pixels: &[Rgba]) -> Result<Vec<u8>, Error> {
    if pixels.len() != width as usize * height as usize {
        return Err(Error::InvalidSize { width, height });
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(png_err)?;
        let data: Vec<u8> = pixels.iter().flat_map(|c| [c.r, c.g, c.b, c.a]).collect();
        w.write_image_data(&data).map_err(png_err)?;
        w.finish().map_err(png_err)?;
    }
    Ok(out)
}

/// Encodes palette indices as an 8-bit indexed PNG with a tRNS chunk for
/// palette alpha.
pub fn encode_png_indexed(
    width: u32,
    height: u32,
    indices: &[u8],
    palette: &[Rgba],
) -> Result<Vec<u8>, Error> {
    if indices.len() != width as usize * height as usize
        || palette.is_empty()
        || palette.len() > 256
    {
        return Err(Error::InvalidSize { width, height });
    }
    let rgb: Vec<u8> = palette.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
    let mut trns: Vec<u8> = palette.iter().map(|c| c.a).collect();
    while trns.last() == Some(&255) {
        trns.pop();
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(rgb);
        if !trns.is_empty() {
            enc.set_trns(trns);
        }
        let mut w = enc.write_header().map_err(png_err)?;
        w.write_image_data(indices).map_err(png_err)?;
        w.finish().map_err(png_err)?;
    }
    Ok(out)
}

/// A new document from a PNG. An indexed PNG opens as an indexed document
/// with its palette; the first fully transparent entry becomes the
/// transparent index, or a transparent entry is appended when there is room.
pub fn import_png(bytes: &[u8]) -> Result<Document, Error> {
    let decoded = decode_png(bytes)?;
    let (width, height) = decoded.size();
    match decoded {
        DecodedPng::Indexed {
            indices, palette, ..
        } => {
            let mut palette = palette;
            let ti = match palette.iter().position(|c| c.a == 0) {
                Some(i) => Some(i as u8),
                None if palette.len() < Palette::MAX => {
                    palette.push(Rgba::TRANSPARENT);
                    Some((palette.len() - 1) as u8)
                }
                None => None,
            };
            if let Some(ti) = ti {
                let mut doc = Document::new(width, height, ColorMode::Indexed)?;
                doc.set_palette(Palette::new(palette)?);
                doc.set_transparent_index(ti);
                let layer = doc.layers()[0].id;
                let target = doc.target(layer, 0)?;
                *target.raster = Raster::Indexed(crate::image::TiledImage::from_pixels(
                    width, height, ti, &indices,
                ));
                return Ok(doc);
            }
            // A full palette with no transparent entry: open as RGBA.
            let pixels = DecodedPng::Indexed {
                width,
                height,
                indices,
                palette,
            }
            .to_rgba();
            rgba_document(width, height, &pixels)
        }
        DecodedPng::Rgba { pixels, .. } => rgba_document(width, height, &pixels),
    }
}

fn rgba_document(width: u32, height: u32, pixels: &[Rgba]) -> Result<Document, Error> {
    let mut doc = Document::new(width, height, ColorMode::Rgba)?;
    let layer = doc.layers()[0].id;
    let target = doc.target(layer, 0)?;
    *target.raster = Raster::Rgba(crate::image::TiledImage::from_pixels(
        width,
        height,
        Rgba::TRANSPARENT,
        pixels,
    ));
    Ok(doc)
}

/// Adds a PNG as a new top raster layer at frame `frame`, placed at the
/// top-left and cropped to the canvas. Colors map to the nearest palette entry
/// in indexed documents.
pub fn import_png_layer(
    doc: &mut Document,
    bytes: &[u8],
    name: &str,
    frame: usize,
) -> Result<LayerId, Error> {
    let decoded = decode_png(bytes)?;
    let (w, h) = decoded.size();
    let pixels = decoded.to_rgba();
    let layer = doc.add_layer(NewLayer::Raster, name, Placement::default())?;
    let target = doc.target(layer, frame)?;
    let ti = target.transparent_index;
    let palette = target.palette;
    for y in 0..h.min(target.raster.height()) as i32 {
        for x in 0..w.min(target.raster.width()) as i32 {
            let c = pixels[(y as u32 * w + x as u32) as usize];
            let raw = match target.raster {
                Raster::Rgba(_) => c.to_u32(),
                Raster::Indexed(_) if c.a == 0 => ti as u32,
                Raster::Indexed(_) => palette.nearest(c, Some(ti)) as u32,
                Raster::Mask(_) => c.luma() as u32,
            };
            target.raster.set_raw(x, y, raw);
        }
    }
    Ok(layer)
}

fn scale_pixels(width: u32, height: u32, pixels: &[Rgba], scale: u32) -> Vec<Rgba> {
    if scale <= 1 {
        return pixels.to_vec();
    }
    let (sw, sh) = (width * scale, height * scale);
    let mut out = Vec::with_capacity(sw as usize * sh as usize);
    for y in 0..sh {
        for x in 0..sw {
            out.push(pixels[((y / scale) * width + x / scale) as usize]);
        }
    }
    out
}

/// Exports one frame as a PNG, enlarged by an integer `scale` (nearest).
/// An indexed document whose composite uses only palette colors exports as
/// an indexed PNG; otherwise the file is RGBA.
pub fn export_frame_png(doc: &Document, frame: usize, scale: u32) -> Result<Vec<u8>, Error> {
    if frame >= doc.frame_count() {
        return Err(Error::FrameOutOfRange(frame));
    }
    let scale = scale.max(1);
    let flat = composite_frame(doc, frame, &CompositeOptions::EXPORT);
    let pixels = scale_pixels(doc.width(), doc.height(), &flat, scale);
    let (w, h) = (doc.width() * scale, doc.height() * scale);
    if doc.mode() == ColorMode::Indexed
        && let Some(indices) = to_indices(doc, &pixels)
    {
        let mut palette = doc.palette().colors().to_vec();
        let ti = doc.transparent_index() as usize;
        if ti < palette.len() {
            palette[ti] = Rgba::TRANSPARENT;
        }
        return encode_png_indexed(w, h, &indices, &palette);
    }
    encode_png_rgba(w, h, &pixels)
}

fn to_indices(doc: &Document, pixels: &[Rgba]) -> Option<Vec<u8>> {
    let ti = doc.transparent_index();
    if ti as usize >= doc.palette().len() {
        return None;
    }
    let lookup: std::collections::HashMap<Rgba, u8> = doc
        .palette()
        .colors()
        .iter()
        .enumerate()
        .rev()
        .filter(|(i, _)| *i != ti as usize)
        .map(|(i, c)| (*c, i as u8))
        .collect();
    pixels
        .iter()
        .map(|c| {
            if c.a == 0 {
                Some(ti)
            } else {
                lookup.get(c).copied()
            }
        })
        .collect()
}

/// How frames are arranged on a sprite sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SheetLayout {
    /// One row.
    #[default]
    Horizontal,
    /// One column.
    Vertical,
    /// Rows of `columns` frames.
    Grid { columns: u32 },
}

/// The `frames` shape of the JSON, as Aseprite offers it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JsonFrames {
    /// An object keyed by frame name.
    #[default]
    Hash,
    /// An array of frames, each with a `filename`.
    Array,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetOptions {
    pub layout: SheetLayout,
    /// Frames to export; `None` exports all.
    pub frames: Option<Range<usize>>,
    /// Transparent pixels between frames (Aseprite's shape padding).
    pub padding: u32,
    /// Integer nearest-neighbor enlargement.
    pub scale: u32,
    /// Frame names are `"{title} {frame}"`.
    pub title: String,
    /// The image file name written into `meta.image`.
    pub image_name: String,
    pub json_frames: JsonFrames,
}

impl Default for SheetOptions {
    fn default() -> Self {
        Self {
            layout: SheetLayout::Horizontal,
            frames: None,
            padding: 0,
            scale: 1,
            title: "sprite".into(),
            image_name: "sprite.png".into(),
            json_frames: JsonFrames::Hash,
        }
    }
}

/// A rendered sprite sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteSheet {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<Rgba>,
    /// Aseprite-compatible JSON metadata.
    pub json: String,
}

impl SpriteSheet {
    pub fn to_png(&self) -> Result<Vec<u8>, Error> {
        encode_png_rgba(self.width, self.height, &self.pixels)
    }
}

fn blend_name(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "normal",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Overlay => "overlay",
        BlendMode::Darken => "darken",
        BlendMode::Lighten => "lighten",
        BlendMode::ColorDodge => "color_dodge",
        BlendMode::ColorBurn => "color_burn",
        BlendMode::HardLight => "hard_light",
        BlendMode::SoftLight => "soft_light",
        BlendMode::Difference => "difference",
        BlendMode::Exclusion => "exclusion",
        BlendMode::Hue => "hue",
        BlendMode::Saturation => "saturation",
        BlendMode::Color => "color",
        BlendMode::Luminosity => "luminosity",
        BlendMode::Add => "addition",
        BlendMode::Subtract => "subtract",
    }
}

fn hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
}

fn rect_json(r: Rect, scale: i32) -> Value {
    json!({"x": r.x * scale, "y": r.y * scale, "w": r.w * scale, "h": r.h * scale})
}

fn layers_json(layers: &[Layer], group: Option<&str>, out: &mut Vec<Value>) {
    // Aseprite lists layers top to bottom of the stack order it stores,
    // which is bottom first, groups before their children.
    for l in layers {
        let mut v = Map::new();
        v.insert("name".into(), l.props.name.clone().into());
        if let Some(g) = group {
            v.insert("group".into(), g.into());
        }
        match &l.kind {
            LayerKind::Group { children } => {
                out.push(Value::Object(v));
                layers_json(children, Some(&l.props.name), out);
            }
            LayerKind::Raster { .. } => {
                v.insert("opacity".into(), l.props.opacity.into());
                v.insert("blendMode".into(), blend_name(l.props.blend).into());
                out.push(Value::Object(v));
            }
            // Reference layers are never exported.
            LayerKind::Reference { .. } => {}
        }
    }
}

/// Lays out frames on one image and writes the JSON Aseprite's "Export
/// Sprite Sheet" produces (frames, frameTags, layers, slices), so engines and
/// importers that read Aseprite sheets read these.
pub fn sprite_sheet(doc: &Document, opts: &SheetOptions) -> Result<SpriteSheet, Error> {
    let range = opts.frames.clone().unwrap_or(0..doc.frame_count());
    if range.start >= range.end || range.end > doc.frame_count() {
        return Err(Error::FrameOutOfRange(range.end));
    }
    let scale = opts.scale.max(1);
    let (fw, fh) = (doc.width() * scale, doc.height() * scale);
    let n = (range.end - range.start) as u32;
    let columns = match opts.layout {
        SheetLayout::Horizontal => n,
        SheetLayout::Vertical => 1,
        SheetLayout::Grid { columns } => columns.clamp(1, n),
    };
    let rows = n.div_ceil(columns);
    let pad = opts.padding;
    let width = columns * fw + (columns - 1) * pad;
    let height = rows * fh + (rows - 1) * pad;
    let mut pixels = vec![Rgba::TRANSPARENT; width as usize * height as usize];
    let mut frames_hash = Map::new();
    let mut frames_array = Vec::new();
    for (i, f) in range.clone().enumerate() {
        let (col, row) = (i as u32 % columns, i as u32 / columns);
        let (x0, y0) = (col * (fw + pad), row * (fh + pad));
        let flat = composite_frame(doc, f, &CompositeOptions::EXPORT);
        let frame_px = scale_pixels(doc.width(), doc.height(), &flat, scale);
        for y in 0..fh {
            let dst = ((y0 + y) * width + x0) as usize;
            let src = (y * fw) as usize;
            pixels[dst..dst + fw as usize].copy_from_slice(&frame_px[src..src + fw as usize]);
        }
        let name = format!("{} {}", opts.title, f);
        let entry = json!({
            "frame": {"x": x0, "y": y0, "w": fw, "h": fh},
            "rotated": false,
            "trimmed": false,
            "spriteSourceSize": {"x": 0, "y": 0, "w": fw, "h": fh},
            "sourceSize": {"w": fw, "h": fh},
            "duration": doc.frames()[f].duration_ms,
        });
        match opts.json_frames {
            JsonFrames::Hash => {
                frames_hash.insert(name, entry);
            }
            JsonFrames::Array => {
                let mut e = Map::new();
                e.insert("filename".into(), name.into());
                if let Value::Object(m) = entry {
                    e.extend(m);
                }
                frames_array.push(Value::Object(e));
            }
        }
    }
    let tags: Vec<Value> = doc
        .tags()
        .iter()
        .filter(|t| t.to >= range.start && t.from < range.end)
        .map(|t| {
            let mut v = Map::new();
            v.insert("name".into(), t.name.clone().into());
            v.insert(
                "from".into(),
                (t.from.max(range.start) - range.start).into(),
            );
            v.insert("to".into(), (t.to.min(range.end - 1) - range.start).into());
            let dir = match t.direction {
                LoopDirection::Forward => "forward",
                LoopDirection::Reverse => "reverse",
                LoopDirection::PingPong => "pingpong",
            };
            v.insert("direction".into(), dir.into());
            if t.repeat > 0 {
                // Aseprite writes the repeat count as a string.
                v.insert("repeat".into(), t.repeat.to_string().into());
            }
            v.insert("color".into(), hex(t.color).into());
            Value::Object(v)
        })
        .collect();
    let mut layers = Vec::new();
    layers_json(doc.layers(), None, &mut layers);
    let s = scale as i32;
    let slices: Vec<Value> = doc
        .slices()
        .iter()
        .map(|sl| {
            let mut key = Map::new();
            key.insert("frame".into(), 0.into());
            key.insert("bounds".into(), rect_json(sl.bounds, s));
            if let Some(c) = sl.center {
                key.insert("center".into(), rect_json(c, s));
            }
            if let Some(p) = sl.pivot {
                key.insert("pivot".into(), json!({"x": p.x * s, "y": p.y * s}));
            }
            json!({"name": sl.name, "color": hex(sl.color), "keys": [Value::Object(key)]})
        })
        .collect();
    let frames = match opts.json_frames {
        JsonFrames::Hash => Value::Object(frames_hash),
        JsonFrames::Array => Value::Array(frames_array),
    };
    let doc_json = json!({
        "frames": frames,
        "meta": {
            "app": "pixl",
            "version": env!("CARGO_PKG_VERSION"),
            "image": opts.image_name,
            "format": "RGBA8888",
            "size": {"w": width, "h": height},
            "scale": scale.to_string(),
            "frameTags": tags,
            "layers": layers,
            "slices": slices,
        }
    });
    let json = serde_json::to_string_pretty(&doc_json).map_err(png_err)?;
    Ok(SpriteSheet {
        width,
        height,
        pixels,
        json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Slice, Tag};
    use crate::geom::Point;

    fn checker(doc: &mut Document, frame: usize, c: Rgba) {
        let l = doc.layers()[0].id;
        let t = doc.target(l, frame).unwrap();
        for y in 0..t.raster.height() as i32 {
            for x in 0..t.raster.width() as i32 {
                if (x + y) % 2 == 0 {
                    t.raster.set_raw(x, y, c.to_u32());
                }
            }
        }
    }

    #[test]
    fn rgba_round_trip() {
        let pixels: Vec<Rgba> = (0..12)
            .map(|i| Rgba::new(i * 20, 255 - i * 20, i, i * 21))
            .collect();
        let bytes = encode_png_rgba(4, 3, &pixels).unwrap();
        let back = decode_png(&bytes).unwrap();
        assert_eq!(
            back,
            DecodedPng::Rgba {
                width: 4,
                height: 3,
                pixels
            }
        );
        assert!(encode_png_rgba(4, 4, &[]).is_err());
        assert!(decode_png(b"not a png").is_err());
    }

    #[test]
    fn grayscale_and_rgb_decode_to_rgba() {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 2, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(png::BitDepth::Sixteen);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[0xff, 0xff, 0x80, 0x00]).unwrap();
            w.finish().unwrap();
        }
        let d = decode_png(&out).unwrap();
        assert_eq!(d.to_rgba(), vec![Rgba::WHITE, Rgba::opaque(128, 128, 128)]);
    }

    #[test]
    fn indexed_round_trip_and_low_bit_depths() {
        let pal = vec![Rgba::TRANSPARENT, Rgba::BLACK, Rgba::opaque(255, 0, 0)];
        let idx = vec![0, 1, 2, 1, 0, 2];
        let bytes = encode_png_indexed(3, 2, &idx, &pal).unwrap();
        let doc = import_png(&bytes).unwrap();
        assert_eq!(doc.mode(), ColorMode::Indexed);
        assert_eq!(doc.transparent_index(), 0);
        let l = doc.layers()[0].id;
        let r = doc.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!(r.get_raw(2, 0), 2);
        // A 2-bit file with no transparency gets a transparent entry added.
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 5, 1);
            enc.set_color(png::ColorType::Indexed);
            enc.set_depth(png::BitDepth::Two);
            enc.set_palette(vec![0, 0, 0, 255, 255, 255, 9, 9, 9, 7, 7, 7]);
            let mut w = enc.write_header().unwrap();
            // indices 0 1 2 3 1 -> 00 01 10 11 | 01 000000
            w.write_image_data(&[0b0001_1011, 0b0100_0000]).unwrap();
            w.finish().unwrap();
        }
        let decoded = decode_png(&out).unwrap();
        match &decoded {
            DecodedPng::Indexed { indices, .. } => assert_eq!(indices, &vec![0, 1, 2, 3, 1]),
            other => panic!("{other:?}"),
        }
        let doc = import_png(&out).unwrap();
        assert_eq!(doc.transparent_index(), 4);
        assert_eq!(doc.palette().len(), 5);
    }

    #[test]
    fn export_frame_scales_and_keeps_indexed() {
        let mut d = Document::new(3, 2, ColorMode::Indexed).unwrap();
        let l = d.layers()[0].id;
        d.target(l, 0).unwrap().raster.set_raw(1, 0, 4);
        let bytes = export_frame_png(&d, 0, 2).unwrap();
        match decode_png(&bytes).unwrap() {
            DecodedPng::Indexed {
                width,
                height,
                indices,
                ..
            } => {
                assert_eq!((width, height), (6, 4));
                assert_eq!(&indices[0..6], &[0, 0, 4, 4, 0, 0]);
            }
            other => panic!("{other:?}"),
        }
        d.update_layer(l, |p| p.opacity = 100).unwrap();
        assert!(matches!(
            decode_png(&export_frame_png(&d, 0, 1).unwrap()).unwrap(),
            DecodedPng::Rgba { .. }
        ));
        assert!(export_frame_png(&d, 3, 1).is_err());
    }

    #[test]
    fn import_layer_maps_to_document_mode() {
        let mut d = Document::new(2, 2, ColorMode::Indexed).unwrap();
        let red = d.palette().get(4);
        let bytes = encode_png_rgba(3, 1, &[red, Rgba::TRANSPARENT, Rgba::BLACK]).unwrap();
        let l = import_png_layer(&mut d, &bytes, "png", 0).unwrap();
        let r = d.cel_raster(l, 0).unwrap().unwrap();
        assert_eq!((r.get_raw(0, 0), r.get_raw(1, 0)), (4, 0));
    }

    #[test]
    fn sprite_sheet_layout_and_aseprite_json() {
        let mut d = Document::new(4, 3, ColorMode::Rgba).unwrap();
        for f in 1..5 {
            d.insert_frame(f, 50 * f as u32).unwrap();
        }
        for f in 0..5 {
            checker(&mut d, f, Rgba::opaque(f as u8 * 40, 0, 0));
        }
        d.tags_mut().push(Tag {
            name: "walk".into(),
            from: 1,
            to: 3,
            direction: LoopDirection::PingPong,
            repeat: 2,
            color: Rgba::opaque(0, 0, 255),
        });
        d.slices_mut().push(Slice {
            name: "hit".into(),
            bounds: Rect::new(1, 1, 2, 2),
            center: Some(Rect::new(0, 0, 1, 1)),
            pivot: Some(Point::new(1, 1)),
            color: Rgba::BLACK,
        });
        let opts = SheetOptions {
            layout: SheetLayout::Grid { columns: 2 },
            padding: 1,
            scale: 2,
            ..Default::default()
        };
        let sheet = sprite_sheet(&d, &opts).unwrap();
        assert_eq!((sheet.width, sheet.height), (2 * 8 + 1, 3 * 6 + 2));
        // Frame 3 sits at column 1, row 1; its top-left pixel is a checker cell.
        let at = |x: u32, y: u32| sheet.pixels[(y * sheet.width + x) as usize];
        assert_eq!(at(9, 7), Rgba::opaque(120, 0, 0));
        assert_eq!(at(8, 0), Rgba::TRANSPARENT, "padding column");
        let v: Value = serde_json::from_str(&sheet.json).unwrap();
        let names: Vec<&String> = v["frames"].as_object().unwrap().keys().collect();
        assert_eq!(
            names,
            ["sprite 0", "sprite 1", "sprite 2", "sprite 3", "sprite 4"]
        );
        let f3 = &v["frames"]["sprite 3"];
        assert_eq!(f3["frame"], json!({"x": 9, "y": 7, "w": 8, "h": 6}));
        assert_eq!(f3["duration"], 150);
        assert_eq!(f3["sourceSize"], json!({"w": 8, "h": 6}));
        let meta = &v["meta"];
        assert_eq!(meta["size"], json!({"w": 17, "h": 20}));
        assert_eq!(meta["scale"], "2");
        assert_eq!(
            meta["frameTags"][0],
            json!({"name": "walk", "from": 1, "to": 3, "direction": "pingpong", "repeat": "2", "color": "#0000ffff"})
        );
        assert_eq!(
            meta["layers"][0],
            json!({"name": "Layer 1", "opacity": 255, "blendMode": "normal"})
        );
        assert_eq!(
            meta["slices"][0]["keys"][0]["bounds"],
            json!({"x": 2, "y": 2, "w": 4, "h": 4})
        );
        assert_eq!(
            meta["slices"][0]["keys"][0]["pivot"],
            json!({"x": 2, "y": 2})
        );
        let png = sheet.to_png().unwrap();
        assert_eq!(decode_png(&png).unwrap().size(), (17, 20));
    }

    #[test]
    fn sprite_sheet_range_and_array_frames() {
        let mut d = Document::new(2, 2, ColorMode::Rgba).unwrap();
        d.insert_frame(1, 100).unwrap();
        d.insert_frame(2, 100).unwrap();
        d.tags_mut().push(Tag {
            name: "t".into(),
            from: 0,
            to: 2,
            direction: LoopDirection::Forward,
            repeat: 0,
            color: Rgba::BLACK,
        });
        let sheet = sprite_sheet(
            &d,
            &SheetOptions {
                layout: SheetLayout::Vertical,
                frames: Some(1..3),
                json_frames: JsonFrames::Array,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((sheet.width, sheet.height), (2, 4));
        let v: Value = serde_json::from_str(&sheet.json).unwrap();
        assert_eq!(v["frames"][1]["filename"], "sprite 2");
        assert_eq!(v["frames"][1]["frame"]["y"], 2);
        assert_eq!(v["meta"]["frameTags"][0]["from"], 0);
        assert_eq!(v["meta"]["frameTags"][0]["to"], 1);
        assert!(v["meta"]["frameTags"][0].get("repeat").is_none());
        assert!(
            sprite_sheet(
                &d,
                &SheetOptions {
                    frames: Some(2..9),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
