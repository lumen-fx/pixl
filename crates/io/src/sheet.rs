//! Sprite sheets: laying a document's frames (and, split by layer, its
//! layers) out on one image with the JSON Aseprite's "Export Sprite Sheet"
//! writes, and splitting a sheet image back into frames.

use std::ops::Range;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::png::{decode_png, encode_png_rgba, scale_pixels};
use pixl_document::color::{ColorMode, Rgba};
use pixl_document::composite::{CompositeOptions, composite_frame};
use pixl_document::document::{BlendMode, Document, Layer, LayerId, LayerKind, LoopDirection};
use pixl_document::error::Error;
use pixl_document::geom::Rect;
use pixl_document::image::{Raster, TiledImage};

/// How frames are arranged on a sprite sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SheetLayout {
    /// One row, each image as wide as it is.
    #[default]
    Horizontal,
    /// One column, each image as tall as it is.
    Vertical,
    /// Rows of `columns` frames, filled row by row, each in a cell the size
    /// of the largest image.
    Grid { columns: u32 },
    /// Columns of `rows` frames, filled column by column, in cells like
    /// `Grid`'s.
    Columns { rows: u32 },
    /// As small an image as the frames fit in, each frame where it fits.
    /// With `trim` the frames are their trimmed sizes.
    Packed,
}

/// The `frames` shape of the JSON, as Aseprite offers it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
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
    /// Transparent pixels around the whole sheet (Aseprite's border
    /// padding).
    pub border: u32,
    /// Crop each frame to its pixels that are not fully transparent. The
    /// JSON's `spriteSourceSize` says where the crop sits in the frame.
    pub trim: bool,
    /// One image per visible layer and frame instead of one per frame.
    pub by_layer: bool,
    /// Integer nearest-neighbor enlargement.
    pub scale: u32,
    /// Frame names are `"{title} {frame}"`, or `"{title} ({layer}) {frame}"`
    /// split by layer.
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
            border: 0,
            trim: false,
            by_layer: false,
            scale: 1,
            title: "sprite".into(),
            image_name: "sprite.png".into(),
            json_frames: JsonFrames::Hash,
        }
    }
}

impl SheetOptions {
    /// The frames of the tag named `name`.
    pub fn tag_range(doc: &Document, name: &str) -> Option<Range<usize>> {
        let tag = doc.tags().iter().find(|t| t.name == name)?;
        Some(tag.from..tag.to + 1)
    }
}

/// One image on a sprite sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetCell {
    pub name: String,
    pub frame: usize,
    /// The layer it shows, split by layer.
    pub layer: Option<LayerId>,
    /// Where it sits on the sheet.
    pub rect: Rect,
    /// The part of the (scaled) frame it holds: the whole frame, or the
    /// trimmed crop.
    pub source: Rect,
}

/// A rendered sprite sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteSheet {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<Rgba>,
    pub cells: Vec<SheetCell>,
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
            // Reference and other layers are never exported.
            _ => {}
        }
    }
}

/// The bounds of the pixels that are not fully transparent, or `None`.
pub fn opaque_bounds(width: u32, height: u32, pixels: &[Rgba]) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..height {
        let row = &pixels[(y * width) as usize..((y + 1) * width) as usize];
        let Some(first) = row.iter().position(|p| p.a != 0) else {
            continue;
        };
        let last = row.iter().rposition(|p| p.a != 0).unwrap_or(first);
        x0 = x0.min(first as u32);
        x1 = x1.max(last as u32);
        y0 = y0.min(y);
        y1 = y;
    }
    (x0 != u32::MAX).then(|| {
        Rect::new(
            x0 as i32,
            y0 as i32,
            (x1 - x0 + 1) as i32,
            (y1 - y0 + 1) as i32,
        )
    })
}

fn crop(width: u32, pixels: &[Rgba], r: Rect) -> Vec<Rgba> {
    let mut out = Vec::with_capacity((r.w * r.h) as usize);
    for y in r.y..r.y + r.h {
        let start = (y as u32 * width + r.x as u32) as usize;
        out.extend_from_slice(&pixels[start..start + r.w as usize]);
    }
    out
}

/// Places boxes of `sizes` (width, height) with `gap` pixels between them
/// on as small an image as a shelf packing finds, trying every useful
/// width. Returns the top-left of each box, in the order given, and the
/// image size.
pub fn pack(sizes: &[(u32, u32)], gap: u32) -> (Vec<(u32, u32)>, (u32, u32)) {
    if sizes.is_empty() {
        return (Vec::new(), (0, 0));
    }
    // Tallest first, so each shelf is as tall as its first box.
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| {
        sizes[b]
            .1
            .cmp(&sizes[a].1)
            .then(sizes[b].0.cmp(&sizes[a].0))
    });
    let widest = sizes.iter().map(|s| s.0).max().unwrap_or(0);
    let mut candidates: Vec<u32> = vec![widest];
    let mut run = 0;
    for (k, &i) in order.iter().enumerate() {
        run += sizes[i].0 + if k > 0 { gap } else { 0 };
        if run >= widest {
            candidates.push(run);
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    let shelve = |limit: u32| {
        let mut at = vec![(0, 0); sizes.len()];
        let (mut x, mut y, mut shelf, mut used_w) = (0u32, 0u32, 0u32, 0u32);
        for &i in &order {
            let (w, h) = sizes[i];
            if x > 0 && x + w > limit {
                y += shelf + gap;
                x = 0;
                shelf = 0;
            }
            at[i] = (x, y);
            used_w = used_w.max(x + w);
            shelf = shelf.max(h);
            x += w + gap;
        }
        (at, (used_w, y + shelf))
    };
    candidates
        .into_iter()
        .map(shelve)
        .min_by_key(|(_, (w, h))| (u64::from(*w) * u64::from(*h), w.abs_diff(*h)))
        .expect("there is a candidate")
}

/// `doc` showing only the layer `only`: every other layer that is not a
/// group is hidden, and groups stay as they are.
pub fn only_layer(doc: &Document, only: LayerId) -> Document {
    let mut alone = doc.clone();
    let others: Vec<LayerId> = alone
        .walk()
        .into_iter()
        .filter(|(_, l)| l.id != only && !matches!(l.kind, LayerKind::Group { .. }))
        .map(|(_, l)| l.id)
        .collect();
    for id in others {
        let _ = alone.update_layer(id, |p| p.visible = false);
    }
    alone
}

/// A frame's pixels for the sheet: the frame composite, or one layer of it.
fn render_cell(doc: &Document, frame: usize, layer: Option<LayerId>) -> Vec<Rgba> {
    match layer {
        None => composite_frame(doc, frame, &CompositeOptions::EXPORT),
        Some(only) => composite_frame(&only_layer(doc, only), frame, &CompositeOptions::EXPORT),
    }
}

/// The layers a sheet split by layer shows: every visible raster layer,
/// bottom first, with its name.
fn sheet_layers(doc: &Document) -> Vec<(LayerId, String)> {
    doc.walk()
        .into_iter()
        .filter(|(_, l)| l.props.visible && matches!(l.kind, LayerKind::Raster { .. }))
        .map(|(_, l)| (l.id, l.props.name.clone()))
        .collect()
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
    let layers: Vec<(Option<LayerId>, Option<String>)> = if opts.by_layer {
        sheet_layers(doc)
            .into_iter()
            .map(|(id, name)| (Some(id), Some(name)))
            .collect()
    } else {
        vec![(None, None)]
    };
    // Every image of the sheet, in order: by layer, then by frame.
    struct Pending {
        name: String,
        frame: usize,
        layer: Option<LayerId>,
        source: Rect,
        pixels: Vec<Rgba>,
    }
    let mut pending = Vec::new();
    for (layer, layer_name) in &layers {
        for f in range.clone() {
            let flat = render_cell(doc, f, *layer);
            let full = scale_pixels(doc.width(), doc.height(), &flat, scale);
            let source = if opts.trim {
                opaque_bounds(fw, fh, &full).unwrap_or(Rect::new(0, 0, 1, 1))
            } else {
                Rect::new(0, 0, fw as i32, fh as i32)
            };
            let pixels = if opts.trim {
                crop(fw, &full, source)
            } else {
                full
            };
            let name = match layer_name {
                Some(l) => format!("{} ({l}) {f}", opts.title),
                None => format!("{} {f}", opts.title),
            };
            pending.push(Pending {
                name,
                frame: f,
                layer: *layer,
                source,
                pixels,
            });
        }
    }
    if pending.is_empty() {
        return Err(Error::Image("no visible layer to put on the sheet".into()));
    }
    let n = pending.len() as u32;
    let pad = opts.padding;
    let sizes: Vec<(u32, u32)> = pending
        .iter()
        .map(|p| (p.source.w as u32, p.source.h as u32))
        .collect();
    // Grid layouts give every image the largest image's cell.
    let (cw, ch) = (
        sizes.iter().map(|s| s.0).max().unwrap_or(1),
        sizes.iter().map(|s| s.1).max().unwrap_or(1),
    );
    let grid = |columns: u32, column_major: bool| {
        let columns = columns.clamp(1, n);
        let rows = n.div_ceil(columns);
        let at: Vec<(u32, u32)> = (0..n)
            .map(|i| {
                let (col, row) = if column_major {
                    (i / rows, i % rows)
                } else {
                    (i % columns, i / columns)
                };
                (col * (cw + pad), row * (ch + pad))
            })
            .collect();
        let size = (
            columns * cw + (columns - 1) * pad,
            rows * ch + (rows - 1) * pad,
        );
        (at, size)
    };
    // A strip gives every image its own size: trimmed frames sit side by
    // side with only the padding between them.
    let strip = |across: bool| {
        let mut at = Vec::with_capacity(sizes.len());
        let mut run = 0;
        for &(w, h) in &sizes {
            at.push(if across { (run, 0) } else { (0, run) });
            run += if across { w } else { h } + pad;
        }
        let run = run - pad;
        let size = if across { (run, ch) } else { (cw, run) };
        (at, size)
    };
    let (at, (inner_w, inner_h)) = match opts.layout {
        SheetLayout::Horizontal => strip(true),
        SheetLayout::Vertical => strip(false),
        SheetLayout::Grid { columns } => grid(columns, false),
        SheetLayout::Columns { rows } => {
            let rows = rows.clamp(1, n);
            grid(n.div_ceil(rows), true)
        }
        SheetLayout::Packed => pack(&sizes, pad),
    };
    let b = opts.border;
    let (width, height) = (inner_w + 2 * b, inner_h + 2 * b);
    let mut pixels = vec![Rgba::TRANSPARENT; width as usize * height as usize];
    let mut cells = Vec::with_capacity(pending.len());
    let mut frames_hash = Map::new();
    let mut frames_array = Vec::new();
    for (p, (x, y)) in pending.into_iter().zip(at) {
        let (x0, y0) = (x + b, y + b);
        let (w, h) = (p.source.w as u32, p.source.h as u32);
        for row in 0..h {
            let dst = ((y0 + row) * width + x0) as usize;
            let src = (row * w) as usize;
            pixels[dst..dst + w as usize].copy_from_slice(&p.pixels[src..src + w as usize]);
        }
        let entry = json!({
            "frame": {"x": x0, "y": y0, "w": w, "h": h},
            "rotated": false,
            "trimmed": opts.trim,
            "spriteSourceSize": {"x": p.source.x, "y": p.source.y, "w": w, "h": h},
            "sourceSize": {"w": fw, "h": fh},
            "duration": doc.frames()[p.frame].duration_ms,
        });
        match opts.json_frames {
            JsonFrames::Hash => {
                frames_hash.insert(p.name.clone(), entry);
            }
            JsonFrames::Array => {
                let mut e = Map::new();
                e.insert("filename".into(), p.name.clone().into());
                if let Value::Object(m) = entry {
                    e.extend(m);
                }
                frames_array.push(Value::Object(e));
            }
        }
        cells.push(SheetCell {
            name: p.name,
            frame: p.frame,
            layer: p.layer,
            rect: Rect::new(x0 as i32, y0 as i32, w as i32, h as i32),
            source: p.source,
        });
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
    let mut layers_meta = Vec::new();
    layers_json(doc.layers(), None, &mut layers_meta);
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
            "layers": layers_meta,
            "slices": slices,
        }
    });
    let json = serde_json::to_string_pretty(&doc_json).map_err(|e| Error::Image(e.to_string()))?;
    Ok(SpriteSheet {
        width,
        height,
        pixels,
        cells,
        json,
    })
}

/// How a sheet image splits into frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetGrid {
    /// The size of one frame.
    pub cell: (u32, u32),
    /// Where the first cell's top-left corner is.
    pub offset: (u32, u32),
    /// Pixels between two cells, across and down.
    pub padding: (u32, u32),
    /// Leave out cells with no pixel that is not fully transparent.
    pub skip_empty: bool,
}

impl SheetGrid {
    /// A grid of `cell`-sized frames from the top-left corner.
    pub fn cells(width: u32, height: u32) -> Self {
        Self {
            cell: (width.max(1), height.max(1)),
            offset: (0, 0),
            padding: (0, 0),
            skip_empty: false,
        }
    }

    /// The grid of `columns` by `rows` cells that fills a `size` sheet past
    /// `offset`, with `padding` between cells.
    pub fn count(
        size: (u32, u32),
        columns: u32,
        rows: u32,
        offset: (u32, u32),
        padding: (u32, u32),
    ) -> Self {
        let span = |total: u32, n: u32, off: u32, pad: u32| {
            let n = n.max(1);
            (total.saturating_sub(off).saturating_sub((n - 1) * pad) / n).max(1)
        };
        Self {
            cell: (
                span(size.0, columns, offset.0, padding.0),
                span(size.1, rows, offset.1, padding.1),
            ),
            offset,
            padding,
            skip_empty: false,
        }
    }

    /// The cells that fit a `size` sheet whole, row by row.
    pub fn rects(&self, size: (u32, u32)) -> Vec<Rect> {
        let (cw, ch) = (self.cell.0.max(1), self.cell.1.max(1));
        let fit = |total: u32, off: u32, cell: u32, pad: u32| {
            if total < off + cell {
                0
            } else {
                (total - off - cell) / (cell + pad) + 1
            }
        };
        let cols = fit(size.0, self.offset.0, cw, self.padding.0);
        let rows = fit(size.1, self.offset.1, ch, self.padding.1);
        let mut out = Vec::with_capacity((cols * rows) as usize);
        for r in 0..rows {
            for c in 0..cols {
                out.push(Rect::new(
                    (self.offset.0 + c * (cw + self.padding.0)) as i32,
                    (self.offset.1 + r * (ch + self.padding.1)) as i32,
                    cw as i32,
                    ch as i32,
                ));
            }
        }
        out
    }
}

/// Splits a sheet image of `width` by `height` into the cells of `grid`,
/// each as its own pixels.
pub fn split_sheet(width: u32, height: u32, pixels: &[Rgba], grid: &SheetGrid) -> Vec<Vec<Rgba>> {
    grid.rects((width, height))
        .into_iter()
        .map(|r| crop(width, pixels, r))
        .filter(|cell| !grid.skip_empty || cell.iter().any(|p| p.a != 0))
        .collect()
}

/// A new RGBA document from a sheet PNG: one frame per cell of `grid`, each
/// lasting `duration_ms`.
pub fn import_sheet(bytes: &[u8], grid: &SheetGrid, duration_ms: u32) -> Result<Document, Error> {
    let decoded = decode_png(bytes)?;
    let (w, h) = decoded.size();
    let cells = split_sheet(w, h, &decoded.to_rgba(), grid);
    sheet_document(grid.cell, &cells, duration_ms)
}

/// A new RGBA document of `cell`-sized frames holding `cells`.
pub fn sheet_document(
    cell: (u32, u32),
    cells: &[Vec<Rgba>],
    duration_ms: u32,
) -> Result<Document, Error> {
    if cells.is_empty() {
        return Err(Error::Image("the sheet holds no whole cell".into()));
    }
    let mut doc = Document::new(cell.0, cell.1, ColorMode::Rgba)?;
    let layer = doc.layers()[0].id;
    for (i, pixels) in cells.iter().enumerate() {
        if i > 0 {
            doc.insert_frame(i, duration_ms)?;
        }
        let target = doc.target(layer, i)?;
        *target.raster = Raster::Rgba(TiledImage::from_pixels(
            cell.0,
            cell.1,
            Rgba::TRANSPARENT,
            pixels,
        ));
    }
    let first = doc.frames()[0].duration_ms;
    if first != duration_ms {
        doc.set_frame_duration(0, duration_ms)?;
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_document::document::{Slice, Tag};
    use pixl_document::geom::Point;

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
        assert_eq!(SheetOptions::tag_range(&d, "t"), Some(0..3));
        assert_eq!(SheetOptions::tag_range(&d, "none"), None);
    }

    /// A document whose frame `f` holds one `f + 1` pixel wide bar at
    /// (1, 1), so each frame trims to its own size.
    fn bars(frames: usize) -> Document {
        let mut d = Document::new(8, 8, ColorMode::Rgba).unwrap();
        for f in 1..frames {
            d.insert_frame(f, 100).unwrap();
        }
        let l = d.layers()[0].id;
        for f in 0..frames {
            let t = d.target(l, f).unwrap();
            t.raster.fill_rect_raw(
                &Rect::new(1, 1, f as i32 + 1, 2),
                Rgba::opaque(200, 10, 10).to_u32(),
            );
        }
        d
    }

    #[test]
    fn trim_crops_each_frame_and_says_where_the_crop_sits() {
        let d = bars(3);
        let sheet = sprite_sheet(
            &d,
            &SheetOptions {
                trim: true,
                ..Default::default()
            },
        )
        .unwrap();
        // Bars of 1, 2 and 3 pixels in a row, each 2 tall.
        assert_eq!((sheet.width, sheet.height), (6, 2));
        let v: Value = serde_json::from_str(&sheet.json).unwrap();
        let f2 = &v["frames"]["sprite 2"];
        assert_eq!(f2["trimmed"], true);
        assert_eq!(f2["frame"], json!({"x": 3, "y": 0, "w": 3, "h": 2}));
        assert_eq!(
            f2["spriteSourceSize"],
            json!({"x": 1, "y": 1, "w": 3, "h": 2})
        );
        assert_eq!(f2["sourceSize"], json!({"w": 8, "h": 8}));
        assert!(sheet.pixels.iter().all(|p| p.a == 255));
    }

    #[test]
    fn packing_places_every_box_apart_on_a_small_image() {
        let sizes = [(8, 8), (4, 4), (4, 4), (2, 8), (8, 2), (4, 4)];
        let (at, (w, h)) = pack(&sizes, 1);
        let rects: Vec<Rect> = at
            .iter()
            .zip(&sizes)
            .map(|(&(x, y), &(bw, bh))| Rect::new(x as i32, y as i32, bw as i32, bh as i32))
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(
                a.right() < w as i32 + 1 && a.bottom() < h as i32 + 1,
                "{a:?}"
            );
            for b in &rects[i + 1..] {
                // At least the gap apart.
                let grown = Rect::new(a.x, a.y, a.w + 1, a.h + 1);
                assert!(grown.intersect(b).is_empty(), "{a:?} {b:?}");
            }
        }
        let area: u32 = sizes.iter().map(|(w, h)| w * h).sum();
        assert!(w * h < area * 2, "{w}x{h} for {area} px of boxes");
        assert_eq!(pack(&[], 0), (Vec::new(), (0, 0)));
    }

    #[test]
    fn a_packed_trimmed_sheet_holds_every_frame() {
        let d = bars(5);
        let sheet = sprite_sheet(
            &d,
            &SheetOptions {
                layout: SheetLayout::Packed,
                trim: true,
                padding: 1,
                border: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(sheet.cells.len(), 5);
        for c in &sheet.cells {
            assert!(c.rect.x >= 2 && c.rect.y >= 2, "inside the border: {c:?}");
            let px = sheet.pixels[(c.rect.y as u32 * sheet.width + c.rect.x as u32) as usize];
            assert_eq!(px.a, 255, "{c:?}");
        }
        let packed = u64::from(sheet.width) * u64::from(sheet.height);
        let row = sprite_sheet(
            &d,
            &SheetOptions {
                trim: true,
                padding: 1,
                border: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(packed <= u64::from(row.width) * u64::from(row.height));
    }

    #[test]
    fn columns_fill_down_first_and_the_border_surrounds() {
        let d = bars(5);
        let sheet = sprite_sheet(
            &d,
            &SheetOptions {
                layout: SheetLayout::Columns { rows: 2 },
                border: 3,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((sheet.width, sheet.height), (3 * 8 + 6, 2 * 8 + 6));
        let at = |i: usize| (sheet.cells[i].rect.x, sheet.cells[i].rect.y);
        assert_eq!(at(0), (3, 3));
        assert_eq!(at(1), (3, 11));
        assert_eq!(at(2), (11, 3));
    }

    #[test]
    fn split_by_layer_names_each_image_for_its_layer() {
        let mut d = bars(2);
        let top = d
            .add_layer(
                pixl_document::document::NewLayer::Raster,
                "Top",
                pixl_document::document::Placement::default(),
            )
            .unwrap();
        d.target(top, 0)
            .unwrap()
            .raster
            .set_raw(7, 7, Rgba::WHITE.to_u32());
        let sheet = sprite_sheet(
            &d,
            &SheetOptions {
                by_layer: true,
                ..Default::default()
            },
        )
        .unwrap();
        let names: Vec<&str> = sheet.cells.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "sprite (Layer 1) 0",
                "sprite (Layer 1) 1",
                "sprite (Top) 0",
                "sprite (Top) 1"
            ]
        );
        // The bottom layer's image holds none of the top layer's pixel.
        let c = &sheet.cells[0];
        let px = |x: i32, y: i32| {
            sheet.pixels[((c.rect.y + y) as u32 * sheet.width + (c.rect.x + x) as u32) as usize]
        };
        assert_eq!(px(7, 7), Rgba::TRANSPARENT);
        assert_eq!(px(1, 1).a, 255);
        let t = &sheet.cells[2];
        let tp =
            sheet.pixels[((t.rect.y + 7) as u32 * sheet.width + (t.rect.x + 7) as u32) as usize];
        assert_eq!(tp, Rgba::WHITE);
    }

    #[test]
    fn a_sheet_splits_back_into_its_frames() {
        let d = bars(4);
        let sheet = sprite_sheet(
            &d,
            &SheetOptions {
                layout: SheetLayout::Grid { columns: 2 },
                padding: 2,
                border: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let grid = SheetGrid {
            cell: (8, 8),
            offset: (1, 1),
            padding: (2, 2),
            skip_empty: false,
        };
        let cells = split_sheet(sheet.width, sheet.height, &sheet.pixels, &grid);
        assert_eq!(cells.len(), 4);
        let back = sheet_document((8, 8), &cells, 80).unwrap();
        assert_eq!(back.frame_count(), 4);
        assert!(back.frames().iter().all(|f| f.duration_ms == 80));
        for f in 0..4 {
            assert_eq!(
                composite_frame(&back, f, &CompositeOptions::EXPORT),
                composite_frame(&d, f, &CompositeOptions::EXPORT),
                "frame {f}"
            );
        }
        // By count: two by two cells fill the same sheet.
        let by_count = SheetGrid::count((sheet.width, sheet.height), 2, 2, (1, 1), (2, 2));
        assert_eq!(by_count.cell, (8, 8));
        let png = sheet.to_png().unwrap();
        let imported = import_sheet(&png, &by_count, 100).unwrap();
        assert_eq!((imported.width(), imported.height()), (8, 8));
        assert_eq!(imported.frame_count(), 4);
    }

    #[test]
    fn empty_cells_can_be_left_out() {
        let mut px = vec![Rgba::TRANSPARENT; 8 * 4];
        px[1] = Rgba::BLACK;
        let grid = SheetGrid {
            skip_empty: true,
            ..SheetGrid::cells(4, 4)
        };
        assert_eq!(split_sheet(8, 4, &px, &grid).len(), 1);
        assert_eq!(split_sheet(8, 4, &px, &SheetGrid::cells(4, 4)).len(), 2);
        assert!(sheet_document((4, 4), &[], 100).is_err());
    }
}
