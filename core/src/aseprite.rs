//! Aseprite `.ase` / `.aseprite` import and export.
//!
//! Follows the Aseprite file specification
//! (<https://github.com/aseprite/aseprite/blob/main/docs/ase-file-specs.md>).
//! Aseprite cels are positioned and sized to their content; pixl cels cover
//! the canvas. Import places each cel on a canvas-sized image (cropping what
//! hangs past the canvas); export crops each cel to its painted bounds.
//!
//! Import reads RGBA, indexed and grayscale sprites (grayscale opens as
//! RGBA), normal, group and reference layers, linked cels, the palette, tags
//! and slices, and the colors of layers, tags and slices. The text and
//! properties of user data are skipped. What pixl cannot represent (tilemap
//! layers, the divide blend mode, cel z-index, per-frame slice keys) is
//! listed in [`AsepriteImport::warnings`].
//!
//! Export writes everything Aseprite can hold; [`AsepriteExport::warnings`]
//! lists what it cannot: layer masks, clipping, alpha locks and the document
//! background color. In indexed documents a reference layer is reduced to the
//! palette, since Aseprite stores every cel in the sprite's color mode.

use std::collections::HashMap;
use std::fmt;

use crate::bytes::{Reader, Writer, corrupt, zlib_compress, zlib_decompress};
use crate::color::{ColorMode, Palette, Rgba};
use crate::document::{
    Background, BlendMode, Document, LayerId, LayerKind, LoopDirection, NewLayer, PixelAspect,
    Placement, Slice, Tag,
};
use crate::error::Error;
use crate::geom::{Point, Rect};
use crate::image::{Raster, TiledImage};

const MAGIC: u16 = 0xA5E0;
const FRAME_MAGIC: u16 = 0xF1FA;

const CHUNK_OLD_PALETTE: u16 = 0x0004;
const CHUNK_OLD_PALETTE_64: u16 = 0x0011;
const CHUNK_LAYER: u16 = 0x2004;
const CHUNK_CEL: u16 = 0x2005;
const CHUNK_CEL_EXTRA: u16 = 0x2006;
const CHUNK_COLOR_PROFILE: u16 = 0x2007;
const CHUNK_EXTERNAL_FILES: u16 = 0x2008;
const CHUNK_MASK: u16 = 0x2016;
const CHUNK_PATH: u16 = 0x2017;
const CHUNK_TAGS: u16 = 0x2018;
const CHUNK_PALETTE: u16 = 0x2019;
const CHUNK_USER_DATA: u16 = 0x2020;
const CHUNK_SLICE: u16 = 0x2022;
const CHUNK_TILESET: u16 = 0x2023;

const HEADER_OPACITY_VALID: u32 = 1;
const HEADER_GROUP_BLEND_VALID: u32 = 2;
const HEADER_LAYER_UUID: u32 = 4;

const LAYER_VISIBLE: u16 = 1;
const LAYER_EDITABLE: u16 = 2;
const LAYER_LOCK_MOVE: u16 = 4;
const LAYER_REFERENCE: u16 = 64;

const LAYER_IMAGE: u16 = 0;
const LAYER_GROUP: u16 = 1;
const LAYER_TILEMAP: u16 = 2;

const CEL_RAW: u16 = 0;
const CEL_LINKED: u16 = 1;
const CEL_COMPRESSED: u16 = 2;

const BLEND_DIVIDE: u16 = 18;

/// Something an Aseprite file held that pixl does not represent. The rest of
/// the file opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportWarning {
    /// A grayscale sprite opened as an RGBA document.
    GrayscaleConverted,
    /// A tilemap layer was left out; pixl has no tilemap layers yet.
    TilemapSkipped { layer: String },
    /// A layer used a blend mode pixl lacks and opened as normal.
    BlendModeReplaced { layer: String },
    /// Cels with a z-index opened in their layer's order.
    ZIndexIgnored,
    /// A tag played ping-pong in reverse and opened as plain ping-pong.
    PingPongReverse { tag: String },
    /// A slice changed over the animation; its first key was kept.
    SliceKeysFlattened { slice: String },
    /// A pixel ratio other than 1:1, 2:1 and 1:2 opened as square pixels.
    PixelAspectReplaced { width: u8, height: u8 },
    /// The palette held more than 256 colors; the first 256 were kept.
    PaletteTruncated { colors: usize },
    /// A cel was scaled in Aseprite; it opened at its integer position.
    CelScaleIgnored,
    /// A chunk of a kind this version does not know was skipped.
    UnknownChunk { kind: u16 },
}

impl fmt::Display for ImportWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportWarning::GrayscaleConverted => {
                f.write_str("the grayscale sprite opened in RGBA color")
            }
            ImportWarning::TilemapSkipped { layer } => {
                write!(f, "tilemap layer \"{layer}\" was left out")
            }
            ImportWarning::BlendModeReplaced { layer } => write!(
                f,
                "layer \"{layer}\" uses the divide blend mode, which pixl lacks; it opened as normal"
            ),
            ImportWarning::ZIndexIgnored => {
                f.write_str("cel z-index is not supported; cels keep their layer order")
            }
            ImportWarning::PingPongReverse { tag } => {
                write!(
                    f,
                    "tag \"{tag}\" plays reverse ping-pong; it opened as ping-pong"
                )
            }
            ImportWarning::SliceKeysFlattened { slice } => write!(
                f,
                "slice \"{slice}\" changes during the animation; only its first frame was kept"
            ),
            ImportWarning::PixelAspectReplaced { width, height } => write!(
                f,
                "pixel ratio {width}:{height} is not supported; pixels opened square"
            ),
            ImportWarning::PaletteTruncated { colors } => {
                write!(
                    f,
                    "the palette had {colors} colors; the first 256 were kept"
                )
            }
            ImportWarning::CelScaleIgnored => {
                f.write_str("scaled cels opened at their unscaled size")
            }
            ImportWarning::UnknownChunk { kind } => {
                write!(
                    f,
                    "skipped data this version does not know (chunk {kind:#06x})"
                )
            }
        }
    }
}

/// pixl data an Aseprite file cannot hold. The UI shows these before or
/// after an export so nothing is lost silently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportWarning {
    /// Aseprite has no layer masks; the layer exported unmasked.
    MaskDropped { layer: String },
    /// Aseprite has no clipping; the layer exported unclipped.
    ClippingDropped { layer: String },
    /// Aseprite has no alpha lock.
    AlphaLockDropped { layer: String },
    /// Aseprite has no document background color.
    BackgroundDropped,
    /// A reference layer in an indexed document was reduced to the palette.
    ReferenceReduced { layer: String },
    /// A frame longer than 65535 ms was shortened to 65535 ms.
    DurationClamped { frame: usize },
}

impl fmt::Display for ExportWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportWarning::MaskDropped { layer } => {
                write!(f, "the mask of layer \"{layer}\" is not saved")
            }
            ExportWarning::ClippingDropped { layer } => {
                write!(f, "layer \"{layer}\" is saved without clipping")
            }
            ExportWarning::AlphaLockDropped { layer } => {
                write!(f, "the alpha lock of layer \"{layer}\" is not saved")
            }
            ExportWarning::BackgroundDropped => {
                f.write_str("the document background color is not saved")
            }
            ExportWarning::ReferenceReduced { layer } => {
                write!(f, "reference layer \"{layer}\" is saved in palette colors")
            }
            ExportWarning::DurationClamped { frame } => {
                write!(f, "frame {frame} is saved with a duration of 65535 ms")
            }
        }
    }
}

/// A document read from an Aseprite file.
#[derive(Clone, Debug, PartialEq)]
pub struct AsepriteImport {
    pub document: Document,
    pub warnings: Vec<ImportWarning>,
}

/// An encoded Aseprite file and what it could not hold.
#[derive(Clone, Debug, PartialEq)]
pub struct AsepriteExport {
    pub bytes: Vec<u8>,
    pub warnings: Vec<ExportWarning>,
}

fn warn<W: PartialEq>(list: &mut Vec<W>, w: W) {
    if !list.contains(&w) {
        list.push(w);
    }
}

// ---- import ----

struct AseLayer {
    flags: u16,
    kind: u16,
    level: u16,
    blend: u16,
    opacity: u8,
    name: String,
    color: Option<Rgba>,
}

enum CelData {
    Image { w: u16, h: u16, pixels: Vec<u8> },
    Linked(usize),
    Tilemap,
}

struct AseCel {
    layer: usize,
    frame: usize,
    x: i16,
    y: i16,
    opacity: u8,
    data: CelData,
}

#[derive(Clone, Copy)]
enum UserDataTarget {
    None,
    Layer(usize),
    Tag(usize),
    Slice(usize),
}

/// How raw cel pixels of the sprite's depth become pixl raw values.
#[derive(Clone, Copy)]
enum Depth {
    Rgba,
    Gray,
    Indexed,
}

impl Depth {
    fn bytes(self) -> usize {
        match self {
            Depth::Rgba => 4,
            Depth::Gray => 2,
            Depth::Indexed => 1,
        }
    }

    fn rgba(self, px: &[u8]) -> Rgba {
        let c = match self {
            Depth::Rgba => Rgba::new(px[0], px[1], px[2], px[3]),
            Depth::Gray => Rgba::new(px[0], px[0], px[0], px[1]),
            Depth::Indexed => unreachable!("indexed pixels need the palette"),
        };
        if c.a == 0 { Rgba::TRANSPARENT } else { c }
    }
}

fn read_string(r: &mut Reader) -> Result<String, Error> {
    let len = r.u16()? as usize;
    // Aseprite writes UTF-8; anything else is kept readable, not refused.
    Ok(String::from_utf8_lossy(r.take(len)?).into_owned())
}

fn read_old_palette(r: &mut Reader, six_bit: bool, out: &mut Vec<Rgba>) -> Result<(), Error> {
    let packets = r.u16()?;
    let mut index = 0usize;
    for _ in 0..packets {
        index += r.u8()? as usize;
        let n = match r.u8()? {
            0 => 256,
            n => n as usize,
        };
        for _ in 0..n {
            let [cr, g, b] = r.array()?;
            let scale = |v: u8| {
                if six_bit {
                    ((v.min(63) as u32 * 255 + 31) / 63) as u8
                } else {
                    v
                }
            };
            if index < 256 {
                if out.len() <= index {
                    out.resize(index + 1, Rgba::BLACK);
                }
                out[index] = Rgba::opaque(scale(cr), scale(g), scale(b));
            }
            index += 1;
        }
    }
    Ok(())
}

/// Reads an Aseprite file into a new document.
pub fn import_aseprite(bytes: &[u8]) -> Result<AsepriteImport, Error> {
    let mut warnings = Vec::new();
    let mut r = Reader::new(bytes);
    r.u32()?;
    if r.u16()? != MAGIC {
        return Err(corrupt("not an Aseprite file"));
    }
    let frame_count = r.u16()? as usize;
    let width = r.u16()? as u32;
    let height = r.u16()? as u32;
    let depth = match r.u16()? {
        32 => Depth::Rgba,
        16 => Depth::Gray,
        8 => Depth::Indexed,
        d => return Err(corrupt(format!("unsupported color depth {d}"))),
    };
    let header_flags = r.u32()?;
    let speed = r.u16()?;
    r.skip(8)?;
    let transparent_index = r.u8()?;
    r.skip(3)?;
    r.u16()?;
    let pixel_w = r.u8()?;
    let pixel_h = r.u8()?;
    r.skip(8 + 84)?;
    if frame_count == 0 {
        return Err(Error::LastFrame);
    }

    let mut durations = Vec::with_capacity(frame_count.min(r.remaining() / 16));
    let mut layers: Vec<AseLayer> = Vec::new();
    let mut cels: Vec<AseCel> = Vec::new();
    let mut palette: Option<Vec<Rgba>> = None;
    let mut old_palette: Vec<Rgba> = Vec::new();
    let mut tags: Vec<(Tag, bool)> = Vec::new();
    let mut slices: Vec<Slice> = Vec::new();
    let mut target = UserDataTarget::None;

    for frame in 0..frame_count {
        let frame_bytes = r.u32()? as usize;
        if frame_bytes < 16 {
            return Err(corrupt("frame header is too short"));
        }
        if r.u16()? != FRAME_MAGIC {
            return Err(corrupt("frame header is damaged"));
        }
        let old_chunks = r.u16()? as usize;
        let duration = r.u16()?;
        r.skip(2)?;
        let new_chunks = r.u32()? as usize;
        let chunks = if new_chunks == 0 {
            old_chunks
        } else {
            new_chunks
        };
        let mut fr = Reader::new(r.take(frame_bytes - 16)?);
        durations.push(match (duration, speed) {
            (0, 0) => 100,
            (0, s) => s as u32,
            (d, _) => d as u32,
        });
        for _ in 0..chunks {
            let size = fr.u32()? as usize;
            if size < 6 {
                return Err(corrupt("chunk is too short"));
            }
            let kind = fr.u16()?;
            let mut c = Reader::new(fr.take(size - 6)?);
            match kind {
                CHUNK_LAYER => {
                    let flags = c.u16()?;
                    let kind = c.u16()?;
                    let level = c.u16()?;
                    c.skip(4)?;
                    let blend = c.u16()?;
                    let opacity = c.u8()?;
                    c.skip(3)?;
                    let name = read_string(&mut c)?;
                    if kind == LAYER_TILEMAP {
                        c.u32()?;
                    }
                    if header_flags & HEADER_LAYER_UUID != 0 {
                        c.skip(16)?;
                    }
                    target = UserDataTarget::Layer(layers.len());
                    layers.push(AseLayer {
                        flags,
                        kind,
                        level,
                        blend,
                        opacity,
                        name,
                        color: None,
                    });
                }
                CHUNK_CEL => {
                    let layer = c.u16()? as usize;
                    let x = c.i16()?;
                    let y = c.i16()?;
                    let opacity = c.u8()?;
                    let kind = c.u16()?;
                    let z = c.i16()?;
                    c.skip(5)?;
                    if z != 0 {
                        warn(&mut warnings, ImportWarning::ZIndexIgnored);
                    }
                    let data = match kind {
                        CEL_RAW | CEL_COMPRESSED => {
                            let w = c.u16()?;
                            let h = c.u16()?;
                            let size = w as usize * h as usize * depth.bytes();
                            let pixels = if kind == CEL_RAW {
                                c.take(size)?.to_vec()
                            } else {
                                zlib_decompress(c.take(c.remaining())?, size)?
                            };
                            CelData::Image { w, h, pixels }
                        }
                        CEL_LINKED => CelData::Linked(c.u16()? as usize),
                        _ => CelData::Tilemap,
                    };
                    target = UserDataTarget::None;
                    cels.push(AseCel {
                        layer,
                        frame,
                        x,
                        y,
                        opacity,
                        data,
                    });
                }
                CHUNK_CEL_EXTRA => {
                    if c.u32()? & 1 != 0 {
                        warn(&mut warnings, ImportWarning::CelScaleIgnored);
                    }
                }
                CHUNK_TAGS => {
                    let n = c.u16()?;
                    c.skip(8)?;
                    target = UserDataTarget::Tag(tags.len());
                    for _ in 0..n {
                        let from = c.u16()? as usize;
                        let to = c.u16()? as usize;
                        let dir = c.u8()?;
                        let repeat = c.u16()? as u32;
                        c.skip(6)?;
                        let [cr, g, b] = c.array()?;
                        c.skip(1)?;
                        let name = read_string(&mut c)?;
                        let direction = match dir {
                            1 => LoopDirection::Reverse,
                            2 | 3 => LoopDirection::PingPong,
                            _ => LoopDirection::Forward,
                        };
                        tags.push((
                            Tag {
                                name,
                                from,
                                to,
                                direction,
                                repeat,
                                color: Rgba::opaque(cr, g, b),
                            },
                            dir == 3,
                        ));
                    }
                }
                CHUNK_PALETTE => {
                    let size = c.u32()? as usize;
                    let first = c.u32()? as usize;
                    let last = c.u32()? as usize;
                    c.skip(8)?;
                    if first > last || last >= size || size > 1 << 16 {
                        return Err(corrupt("palette chunk is damaged"));
                    }
                    let colors = palette.get_or_insert_with(Vec::new);
                    colors.resize(size, Rgba::BLACK);
                    for entry in colors.iter_mut().take(last + 1).skip(first) {
                        let flags = c.u16()?;
                        *entry = Rgba::from_u32(u32::from_le_bytes(c.array()?));
                        if flags & 1 != 0 {
                            read_string(&mut c)?;
                        }
                    }
                    target = UserDataTarget::None;
                }
                CHUNK_OLD_PALETTE | CHUNK_OLD_PALETTE_64 => {
                    read_old_palette(&mut c, kind == CHUNK_OLD_PALETTE_64, &mut old_palette)?;
                }
                CHUNK_USER_DATA => {
                    let flags = c.u32()?;
                    if flags & 1 != 0 {
                        read_string(&mut c)?;
                    }
                    let color = if flags & 2 != 0 {
                        Some(Rgba::from_u32(u32::from_le_bytes(c.array()?)))
                    } else {
                        None
                    };
                    match target {
                        UserDataTarget::Layer(i) => {
                            layers[i].color = color;
                            target = UserDataTarget::None;
                        }
                        UserDataTarget::Tag(i) => {
                            if let (Some(t), Some(color)) = (tags.get_mut(i), color) {
                                t.0.color = color;
                            }
                            target = UserDataTarget::Tag(i + 1);
                        }
                        UserDataTarget::Slice(i) => {
                            if let Some(color) = color {
                                slices[i].color = color;
                            }
                            target = UserDataTarget::None;
                        }
                        UserDataTarget::None => {}
                    }
                }
                CHUNK_SLICE => {
                    let keys = c.count(16)?;
                    let flags = c.u32()?;
                    c.u32()?;
                    let name = read_string(&mut c)?;
                    let mut first: Option<(Rect, Option<Rect>, Option<Point>)> = None;
                    let mut changes = false;
                    for _ in 0..keys {
                        c.u32()?;
                        let bounds =
                            Rect::new(c.i32()?, c.i32()?, c.u32()? as i32, c.u32()? as i32);
                        let center = if flags & 1 != 0 {
                            Some(Rect::new(
                                c.i32()?,
                                c.i32()?,
                                c.u32()? as i32,
                                c.u32()? as i32,
                            ))
                        } else {
                            None
                        };
                        let pivot = if flags & 2 != 0 {
                            Some(Point::new(c.i32()?, c.i32()?))
                        } else {
                            None
                        };
                        match first {
                            None => first = Some((bounds, center, pivot)),
                            Some(k) => changes |= k != (bounds, center, pivot),
                        }
                    }
                    let Some((bounds, center, pivot)) = first else {
                        continue;
                    };
                    if changes {
                        warn(
                            &mut warnings,
                            ImportWarning::SliceKeysFlattened {
                                slice: name.clone(),
                            },
                        );
                    }
                    target = UserDataTarget::Slice(slices.len());
                    slices.push(Slice {
                        name,
                        bounds,
                        center,
                        pivot,
                        color: Rgba::opaque(0, 0, 255),
                    });
                }
                CHUNK_COLOR_PROFILE | CHUNK_EXTERNAL_FILES | CHUNK_MASK | CHUNK_PATH
                | CHUNK_TILESET => {}
                kind => warn(&mut warnings, ImportWarning::UnknownChunk { kind }),
            }
        }
    }

    let mode = match depth {
        Depth::Indexed => ColorMode::Indexed,
        Depth::Rgba => ColorMode::Rgba,
        Depth::Gray => {
            warn(&mut warnings, ImportWarning::GrayscaleConverted);
            ColorMode::Rgba
        }
    };
    let mut doc = Document::new(width, height, mode)?;
    let mut colors = palette.unwrap_or(old_palette);
    if colors.len() > Palette::MAX {
        warn(
            &mut warnings,
            ImportWarning::PaletteTruncated {
                colors: colors.len(),
            },
        );
        colors.truncate(Palette::MAX);
    }
    if !colors.is_empty() {
        doc.set_palette(Palette::new(colors)?);
    }
    doc.set_transparent_index(transparent_index);
    let palette = doc.palette().clone();
    match (pixel_w, pixel_h) {
        (0, _) | (_, 0) => {}
        (w, h) if w == h => {}
        (2, 1) => {
            doc.set_pixel_aspect(PixelAspect::Wide);
        }
        (1, 2) => {
            doc.set_pixel_aspect(PixelAspect::Tall);
        }
        (width, height) => warn(
            &mut warnings,
            ImportWarning::PixelAspectReplaced { width, height },
        ),
    }
    doc.set_frame_duration(0, durations[0])?;
    for (i, d) in durations.iter().enumerate().skip(1) {
        doc.insert_frame(i, *d)?;
    }

    // Pixel value conversion for this sprite's depth.
    let raw_at = |px: &[u8], into: &Raster| -> u32 {
        match (depth, into) {
            (Depth::Indexed, Raster::Rgba(_)) => {
                if px[0] == transparent_index {
                    0
                } else {
                    palette.get(px[0]).to_u32()
                }
            }
            (Depth::Indexed, _) => px[0] as u32,
            (d, _) => d.rgba(px).to_u32(),
        }
    };
    let paint = |raster: &mut Raster, cel: &AseCel| {
        let CelData::Image { w, h, pixels } = &cel.data else {
            return;
        };
        let bpp = depth.bytes();
        let (cw, ch) = (raster.width() as i32, raster.height() as i32);
        for row in 0..*h as i32 {
            let y = cel.y as i32 + row;
            if y < 0 || y >= ch {
                continue;
            }
            for col in 0..*w as i32 {
                let x = cel.x as i32 + col;
                if x < 0 || x >= cw {
                    continue;
                }
                let at = (row as usize * *w as usize + col as usize) * bpp;
                let raw = raw_at(&pixels[at..at + bpp], raster);
                raster.set_raw(x, y, raw);
            }
        }
    };
    let image_cel = |layer: usize| {
        let mut found = cels.iter().find(|c| c.layer == layer)?;
        // Follow a link to the cel holding the pixels.
        for _ in 0..2 {
            if let CelData::Linked(f) = found.data {
                found = cels.iter().find(|c| c.layer == layer && c.frame == f)?;
            }
        }
        matches!(found.data, CelData::Image { .. }).then_some(found)
    };

    let default_layer = doc.layers()[0].id;
    let mut ids: Vec<Option<LayerId>> = Vec::with_capacity(layers.len());
    let mut groups: Vec<LayerId> = Vec::new();
    for (i, l) in layers.iter().enumerate() {
        let level = l.level as usize;
        let parent = match level {
            0 => None,
            _ => groups.get(level - 1).or(groups.last()).copied(),
        };
        groups.truncate(level);
        let kind = match l.kind {
            LAYER_GROUP => NewLayer::Group,
            LAYER_IMAGE if l.flags & LAYER_REFERENCE != 0 => {
                let mut raster = Raster::Rgba(TiledImage::new(width, height, Rgba::TRANSPARENT));
                if let Some(cel) = image_cel(i) {
                    paint(&mut raster, cel);
                }
                let Raster::Rgba(img) = raster else {
                    unreachable!()
                };
                NewLayer::Reference(img)
            }
            LAYER_IMAGE => NewLayer::Raster,
            _ => {
                warn(
                    &mut warnings,
                    ImportWarning::TilemapSkipped {
                        layer: l.name.clone(),
                    },
                );
                ids.push(None);
                continue;
            }
        };
        let is_group = matches!(kind, NewLayer::Group);
        let id = doc.add_layer(
            kind,
            l.name.clone(),
            Placement {
                parent,
                index: None,
            },
        )?;
        if is_group {
            groups.push(id);
        }
        let blend = match BlendMode::from_code(l.blend) {
            Some(b) => b,
            None => {
                if l.blend == BLEND_DIVIDE {
                    warn(
                        &mut warnings,
                        ImportWarning::BlendModeReplaced {
                            layer: l.name.clone(),
                        },
                    );
                }
                BlendMode::Normal
            }
        };
        let blend_valid = !is_group || header_flags & HEADER_GROUP_BLEND_VALID != 0;
        let opacity_valid = blend_valid && header_flags & HEADER_OPACITY_VALID != 0;
        doc.update_layer(id, |p| {
            p.visible = l.flags & LAYER_VISIBLE != 0;
            p.opacity = if opacity_valid { l.opacity } else { 255 };
            p.blend = if blend_valid {
                blend
            } else {
                BlendMode::Normal
            };
            p.color_tag = l.color;
        })?;
        ids.push(Some(id));
    }

    let raster_layer = |doc: &Document, layer: usize| {
        let id = ids.get(layer).copied().flatten()?;
        doc.layer(id)?.is_raster().then_some(id)
    };
    for cel in &cels {
        if cel.frame >= frame_count || !matches!(cel.data, CelData::Image { .. }) {
            continue;
        }
        let Some(id) = raster_layer(&doc, cel.layer) else {
            continue;
        };
        paint(doc.target(id, cel.frame)?.raster, cel);
        if cel.opacity != 255 {
            doc.set_cel_opacity(id, cel.frame, cel.opacity)?;
        }
    }
    for cel in &cels {
        let CelData::Linked(from) = cel.data else {
            continue;
        };
        let Some(id) = raster_layer(&doc, cel.layer) else {
            continue;
        };
        if from >= frame_count || from == cel.frame || doc.cel(id, from)?.is_none() {
            continue;
        }
        doc.link_cel(id, from, cel.frame)?;
        if cel.opacity != 255 {
            doc.set_cel_opacity(id, cel.frame, cel.opacity)?;
        }
    }
    // Locks go on last: they would refuse the cel writes above.
    for (l, id) in layers.iter().zip(&ids) {
        if let Some(id) = id {
            doc.update_layer(*id, |p| {
                p.locks.pixels = l.flags & LAYER_EDITABLE == 0;
                p.locks.position = l.flags & LAYER_LOCK_MOVE != 0;
            })?;
        }
    }
    if ids.iter().any(Option::is_some) {
        doc.remove_layer(default_layer)?;
    }

    for (tag, reverse) in tags {
        if tag.from > tag.to || tag.to >= frame_count {
            return Err(corrupt(format!(
                "tag \"{}\" covers missing frames",
                tag.name
            )));
        }
        if reverse {
            warn(
                &mut warnings,
                ImportWarning::PingPongReverse {
                    tag: tag.name.clone(),
                },
            );
        }
        doc.tags_mut().push(tag);
    }
    doc.slices_mut().extend(slices);
    Ok(AsepriteImport {
        document: doc,
        warnings,
    })
}

// ---- export ----

fn chunk(out: &mut Vec<Vec<u8>>, kind: u16, payload: Writer) {
    let mut c = Writer::new();
    c.u32(6 + payload.buf.len() as u32);
    c.u16(kind);
    c.bytes(&payload.buf);
    out.push(c.buf);
}

fn ase_string(w: &mut Writer, s: &str) {
    let mut end = s.len().min(u16::MAX as usize);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    w.u16(end as u16);
    w.bytes(&s.as_bytes()[..end]);
}

fn color_user_data(out: &mut Vec<Vec<u8>>, color: Option<Rgba>) {
    let mut w = Writer::new();
    match color {
        Some(c) => {
            w.u32(2);
            w.bytes(&[c.r, c.g, c.b, c.a]);
        }
        None => w.u32(0),
    }
    chunk(out, CHUNK_USER_DATA, w);
}

/// One cel's pixels cropped to `bounds`, in the sprite's pixel format.
fn cel_pixels(raster: &Raster, bounds: Rect, to_index: Option<&dyn Fn(Rgba) -> u8>) -> Vec<u8> {
    let mut out = Vec::new();
    for p in bounds.points() {
        let raw = raster.get_raw(p.x, p.y);
        match (raster, to_index) {
            (Raster::Rgba(_), Some(f)) => out.push(f(Rgba::from_u32(raw))),
            (Raster::Rgba(_), None) => out.extend_from_slice(&raw.to_le_bytes()),
            _ => out.push(raw as u8),
        }
    }
    out
}

fn cel_chunk(out: &mut Vec<Vec<u8>>, layer: usize, pos: Point, opacity: u8, data: CelBody) {
    let mut w = Writer::new();
    w.u16(layer as u16);
    w.i16(pos.x as i16);
    w.i16(pos.y as i16);
    w.u8(opacity);
    match data {
        CelBody::Linked(frame) => {
            w.u16(CEL_LINKED);
            w.i16(0);
            w.zeros(5);
            w.u16(frame as u16);
        }
        CelBody::Image(bounds, pixels) => {
            w.u16(CEL_COMPRESSED);
            w.i16(0);
            w.zeros(5);
            w.u16(bounds.w as u16);
            w.u16(bounds.h as u16);
            w.bytes(&zlib_compress(&pixels, 6));
        }
    }
    chunk(out, CHUNK_CEL, w);
}

enum CelBody {
    Linked(usize),
    Image(Rect, Vec<u8>),
}

/// Encodes a document as an Aseprite file. Reference layers are kept:
/// Aseprite has them too.
pub fn export_aseprite(doc: &Document) -> Result<AsepriteExport, Error> {
    let frames = doc.frame_count();
    if frames > u16::MAX as usize {
        return Err(Error::Format(
            "an Aseprite file holds at most 65535 frames".into(),
        ));
    }
    let mut warnings = Vec::new();
    let indexed = doc.mode() == ColorMode::Indexed;
    let ti = doc.transparent_index();
    let palette = doc.palette();
    let to_index = |c: Rgba| {
        if c.a == 0 {
            ti
        } else {
            palette.nearest(c, Some(ti))
        }
    };
    if let Background::Color(_) = doc.background() {
        warnings.push(ExportWarning::BackgroundDropped);
    }

    let layers = doc.walk();
    let mut per_frame: Vec<Vec<Vec<u8>>> = vec![Vec::new(); frames];
    {
        let first = &mut per_frame[0];
        if !palette.is_empty() {
            let mut w = Writer::new();
            w.u32(palette.len() as u32);
            w.u32(0);
            w.u32(palette.len() as u32 - 1);
            w.zeros(8);
            for c in palette.colors() {
                w.u16(0);
                w.bytes(&[c.r, c.g, c.b, c.a]);
            }
            chunk(first, CHUNK_PALETTE, w);
        }
        for (depth, l) in &layers {
            let p = &l.props;
            let mut flags = 0;
            if p.visible {
                flags |= LAYER_VISIBLE;
            }
            if !p.locks.pixels {
                flags |= LAYER_EDITABLE;
            }
            if p.locks.position {
                flags |= LAYER_LOCK_MOVE;
            }
            let kind = match &l.kind {
                LayerKind::Group { .. } => LAYER_GROUP,
                LayerKind::Reference { .. } => {
                    flags |= LAYER_REFERENCE;
                    if indexed {
                        warnings.push(ExportWarning::ReferenceReduced {
                            layer: p.name.clone(),
                        });
                    }
                    LAYER_IMAGE
                }
                LayerKind::Raster { mask, .. } => {
                    if mask.is_some() {
                        warnings.push(ExportWarning::MaskDropped {
                            layer: p.name.clone(),
                        });
                    }
                    LAYER_IMAGE
                }
            };
            if p.clip {
                warnings.push(ExportWarning::ClippingDropped {
                    layer: p.name.clone(),
                });
            }
            if p.locks.alpha {
                warnings.push(ExportWarning::AlphaLockDropped {
                    layer: p.name.clone(),
                });
            }
            let mut w = Writer::new();
            w.u16(flags);
            w.u16(kind);
            w.u16(*depth as u16);
            w.u16(0);
            w.u16(0);
            w.u16(p.blend.code());
            w.u8(p.opacity);
            w.zeros(3);
            ase_string(&mut w, &p.name);
            chunk(first, CHUNK_LAYER, w);
            if p.color_tag.is_some() {
                color_user_data(first, p.color_tag);
            }
        }
        if !doc.tags().is_empty() {
            let mut w = Writer::new();
            w.u16(doc.tags().len().min(u16::MAX as usize) as u16);
            w.zeros(8);
            for t in doc.tags().iter().take(u16::MAX as usize) {
                w.u16(t.from as u16);
                w.u16(t.to as u16);
                w.u8(match t.direction {
                    LoopDirection::Forward => 0,
                    LoopDirection::Reverse => 1,
                    LoopDirection::PingPong => 2,
                });
                w.u16(t.repeat.min(u16::MAX as u32) as u16);
                w.zeros(6);
                w.bytes(&[t.color.r, t.color.g, t.color.b, 0]);
                ase_string(&mut w, &t.name);
            }
            chunk(first, CHUNK_TAGS, w);
            for t in doc.tags().iter().take(u16::MAX as usize) {
                color_user_data(first, Some(t.color));
            }
        }
        for s in doc.slices() {
            let mut w = Writer::new();
            w.u32(1);
            w.u32((s.center.is_some() as u32) | ((s.pivot.is_some() as u32) << 1));
            w.u32(0);
            ase_string(&mut w, &s.name);
            w.u32(0);
            w.i32(s.bounds.x);
            w.i32(s.bounds.y);
            w.u32(s.bounds.w.max(0) as u32);
            w.u32(s.bounds.h.max(0) as u32);
            if let Some(c) = s.center {
                w.i32(c.x);
                w.i32(c.y);
                w.u32(c.w.max(0) as u32);
                w.u32(c.h.max(0) as u32);
            }
            if let Some(p) = s.pivot {
                w.i32(p.x);
                w.i32(p.y);
            }
            chunk(first, CHUNK_SLICE, w);
            color_user_data(first, Some(s.color));
        }
    }

    for (index, (_, l)) in layers.iter().enumerate() {
        match &l.kind {
            LayerKind::Raster { cels, .. } => {
                // The frame each image was first written at, or None when it
                // is empty and was not written.
                let mut written: HashMap<_, Option<usize>> = HashMap::new();
                for (frame, cel) in cels.iter().enumerate() {
                    let Some(cel) = cel else { continue };
                    let out = &mut per_frame[frame];
                    match written.get(&cel.image) {
                        Some(Some(first)) => {
                            let raster = &doc.images[&cel.image];
                            let b = raster.content_bounds();
                            cel_chunk(
                                out,
                                index,
                                Point::new(b.x, b.y),
                                cel.opacity,
                                CelBody::Linked(*first),
                            );
                        }
                        Some(None) => {}
                        None => {
                            let raster = &doc.images[&cel.image];
                            let b = raster.content_bounds();
                            if b.is_empty() {
                                written.insert(cel.image, None);
                                continue;
                            }
                            let px = cel_pixels(raster, b, None);
                            cel_chunk(
                                out,
                                index,
                                Point::new(b.x, b.y),
                                cel.opacity,
                                CelBody::Image(b, px),
                            );
                            written.insert(cel.image, Some(frame));
                        }
                    }
                }
            }
            LayerKind::Reference { image } => {
                let raster = &doc.images[image];
                let b = raster.content_bounds();
                if b.is_empty() {
                    continue;
                }
                let px = if indexed {
                    cel_pixels(raster, b, Some(&to_index))
                } else {
                    cel_pixels(raster, b, None)
                };
                cel_chunk(
                    &mut per_frame[0],
                    index,
                    Point::new(b.x, b.y),
                    255,
                    CelBody::Image(b, px),
                );
                for out in per_frame.iter_mut().skip(1) {
                    cel_chunk(out, index, Point::new(b.x, b.y), 255, CelBody::Linked(0));
                }
            }
            LayerKind::Group { .. } => {}
        }
    }

    let mut w = Writer::new();
    w.u32(0);
    w.u16(MAGIC);
    w.u16(frames as u16);
    w.u16(doc.width() as u16);
    w.u16(doc.height() as u16);
    w.u16(if indexed { 8 } else { 32 });
    w.u32(HEADER_OPACITY_VALID | HEADER_GROUP_BLEND_VALID);
    w.u16(100);
    w.zeros(8);
    w.u8(ti);
    w.zeros(3);
    w.u16(palette.len() as u16);
    let (pw, ph) = match doc.pixel_aspect() {
        PixelAspect::Square => (1, 1),
        PixelAspect::Wide => (2, 1),
        PixelAspect::Tall => (1, 2),
    };
    w.u8(pw);
    w.u8(ph);
    w.i16(0);
    w.i16(0);
    w.u16(16);
    w.u16(16);
    w.zeros(84);
    for (frame, chunks) in per_frame.iter().enumerate() {
        let ms = doc.frames()[frame].duration_ms;
        if ms > u16::MAX as u32 {
            warnings.push(ExportWarning::DurationClamped { frame });
        }
        w.u32(16 + chunks.iter().map(Vec::len).sum::<usize>() as u32);
        w.u16(FRAME_MAGIC);
        w.u16(chunks.len().min(0xffff) as u16);
        w.u16(ms.min(u16::MAX as u32) as u16);
        w.zeros(2);
        w.u32(chunks.len() as u32);
        for c in chunks {
            w.bytes(c);
        }
    }
    let size = w.buf.len() as u32;
    w.patch_u32(0, size);
    Ok(AsepriteExport {
        bytes: w.buf,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite::{CompositeOptions, composite_frame};
    use crate::document::Locks;

    /// Everything about a document an Aseprite round trip keeps, without the
    /// ids it renumbers.
    fn summary(d: &Document) -> String {
        let mut out = format!(
            "{}x{} {:?} ti={} {:?} {:?}\n",
            d.width(),
            d.height(),
            d.mode(),
            d.transparent_index(),
            d.pixel_aspect(),
            d.palette()
        );
        for f in d.frames() {
            out += &format!("frame {}\n", f.duration_ms);
        }
        for (depth, l) in d.walk() {
            out += &format!("{depth} {:?}\n", l.props);
            match &l.kind {
                LayerKind::Raster { cels, .. } => {
                    for (f, c) in cels.iter().enumerate() {
                        let Some(c) = c else {
                            out += "  empty\n";
                            continue;
                        };
                        let r = &d.images[&c.image];
                        let px: Vec<u32> =
                            r.bounds().points().map(|p| r.get_raw(p.x, p.y)).collect();
                        out += &format!(
                            "  opacity {} linked {:?} pixels {:x}\n",
                            c.opacity,
                            d.linked_frames(l.id, f).unwrap(),
                            fnv(&px)
                        );
                    }
                }
                LayerKind::Reference { image } => {
                    let r = &d.images[image];
                    let px: Vec<u32> = r.bounds().points().map(|p| r.get_raw(p.x, p.y)).collect();
                    out += &format!("  reference {:x}\n", fnv(&px));
                }
                LayerKind::Group { .. } => {}
            }
        }
        out += &format!("{:?}\n{:?}\n", d.tags(), d.slices());
        for f in 0..d.frame_count() {
            out += &format!(
                "composite {:x}\n",
                fnv_rgba(&composite_frame(d, f, &CompositeOptions::EXPORT))
            );
        }
        out
    }

    fn fnv(v: &[u32]) -> u64 {
        v.iter().fold(0xcbf2_9ce4_8422_2325, |h, x| {
            (h ^ *x as u64).wrapping_mul(0x0100_0000_01b3)
        })
    }

    fn fnv_rgba(v: &[Rgba]) -> u64 {
        fnv(&v.iter().map(|c| c.to_u32()).collect::<Vec<_>>())
    }

    fn sample(mode: ColorMode) -> Document {
        let mut d = Document::new(40, 30, mode).unwrap();
        let base = d.layers()[0].id;
        d.insert_frame(1, 70).unwrap();
        d.insert_frame(2, 300).unwrap();
        let paint = |d: &mut Document, layer: LayerId, frame: usize, at: (i32, i32)| {
            let t = d.target(layer, frame).unwrap();
            for i in 0..12 {
                let raw = match t.raster {
                    Raster::Rgba(_) => Rgba::new(10 * i as u8, 3, 200, 255 - i as u8 * 3).to_u32(),
                    _ => 1 + i as u32,
                };
                t.raster.set_raw(at.0 + i % 4, at.1 + i / 4, raw);
            }
        };
        paint(&mut d, base, 0, (2, 3));
        d.link_cel(base, 0, 2).unwrap();
        paint(&mut d, base, 1, (36, 27));
        d.set_cel_opacity(base, 1, 90).unwrap();
        let g = d
            .add_layer(NewLayer::Group, "group", Placement::default())
            .unwrap();
        let inner = d
            .add_layer(
                NewLayer::Group,
                "inner",
                Placement {
                    parent: Some(g),
                    index: None,
                },
            )
            .unwrap();
        for (i, mode) in BlendMode::ALL.into_iter().enumerate() {
            let id = d
                .add_layer(
                    NewLayer::Raster,
                    format!("blend {i}"),
                    Placement {
                        parent: Some(if i % 2 == 0 { g } else { inner }),
                        index: None,
                    },
                )
                .unwrap();
            paint(&mut d, id, i % 3, (i as i32, i as i32));
            d.update_layer(id, |p| {
                p.blend = mode;
                p.opacity = 255 - 7 * i as u8;
                p.visible = i != 5;
                p.color_tag = (i % 3 == 0).then_some(Rgba::new(1, 2, 3, 255));
                p.locks = Locks {
                    pixels: i % 4 == 1,
                    alpha: false,
                    position: i % 5 == 2,
                };
            })
            .unwrap();
        }
        d.update_layer(inner, |p| {
            p.blend = BlendMode::Multiply;
            p.opacity = 200;
        })
        .unwrap();
        let mut reference = TiledImage::new(40, 30, Rgba::TRANSPARENT);
        reference.set(5, 6, d.palette().get(4));
        d.add_layer(NewLayer::Reference(reference), "ref", Placement::default())
            .unwrap();
        d.tags_mut().push(Tag {
            name: "walk".into(),
            from: 0,
            to: 2,
            direction: LoopDirection::PingPong,
            repeat: 3,
            color: Rgba::opaque(200, 10, 20),
        });
        d.tags_mut().push(Tag {
            name: "back".into(),
            from: 1,
            to: 1,
            direction: LoopDirection::Reverse,
            repeat: 0,
            color: Rgba::opaque(0, 10, 20),
        });
        d.slices_mut().push(Slice {
            name: "nine".into(),
            bounds: Rect::new(1, 2, 20, 10),
            center: Some(Rect::new(3, 3, 14, 4)),
            pivot: Some(Point::new(10, 9)),
            color: Rgba::new(0, 0, 255, 255),
        });
        d.set_pixel_aspect(PixelAspect::Wide);
        d
    }

    #[test]
    fn round_trips_layers_cels_tags_and_slices() {
        for mode in [ColorMode::Rgba, ColorMode::Indexed] {
            let d = sample(mode);
            let out = export_aseprite(&d).unwrap();
            let expected = match mode {
                ColorMode::Rgba => vec![],
                ColorMode::Indexed => vec![ExportWarning::ReferenceReduced {
                    layer: "ref".into(),
                }],
            };
            assert_eq!(out.warnings, expected);
            let back = import_aseprite(&out.bytes).unwrap();
            assert!(back.warnings.is_empty(), "{:?}", back.warnings);
            assert_eq!(summary(&back.document), summary(&d), "{mode:?}");
        }
    }

    #[test]
    fn cels_are_cropped_and_linked_on_export() {
        let d = sample(ColorMode::Rgba);
        let bytes = export_aseprite(&d).unwrap().bytes;
        // The first frame's first cel chunk belongs to the bottom layer and
        // covers only its 4x3 painted pixels.
        let mut r = Reader::new(&bytes[128..]);
        r.skip(12).unwrap();
        let chunks = r.u32().unwrap();
        let mut cels = Vec::new();
        for _ in 0..chunks {
            let size = r.u32().unwrap();
            let kind = r.u16().unwrap();
            let body = r.take(size as usize - 6).unwrap();
            if kind == CHUNK_CEL {
                cels.push(body.to_vec());
            }
        }
        let first = &cels[0];
        assert_eq!(u16::from_le_bytes([first[0], first[1]]), 0, "layer 0");
        assert_eq!(i16::from_le_bytes([first[2], first[3]]), 2, "x");
        assert_eq!(i16::from_le_bytes([first[4], first[5]]), 3, "y");
        assert_eq!(u16::from_le_bytes([first[16], first[17]]), 4, "width");
        assert_eq!(u16::from_le_bytes([first[18], first[19]]), 3, "height");
    }

    #[test]
    fn export_lists_what_aseprite_cannot_hold() {
        let mut d = sample(ColorMode::Indexed);
        let base = d.layers()[0].id;
        d.add_layer_mask(base).unwrap();
        d.update_layer(base, |p| {
            p.clip = true;
            p.locks.alpha = true;
        })
        .unwrap();
        d.set_background(Background::Color(Rgba::WHITE));
        d.set_frame_duration(1, 70_000).unwrap();
        let w = export_aseprite(&d).unwrap().warnings;
        assert_eq!(
            w,
            vec![
                ExportWarning::BackgroundDropped,
                ExportWarning::MaskDropped {
                    layer: "Layer 1".into()
                },
                ExportWarning::ClippingDropped {
                    layer: "Layer 1".into()
                },
                ExportWarning::AlphaLockDropped {
                    layer: "Layer 1".into()
                },
                ExportWarning::ReferenceReduced {
                    layer: "ref".into()
                },
                ExportWarning::DurationClamped { frame: 1 },
            ]
        );
        assert!(w.iter().all(|w| !w.to_string().is_empty()));
    }

    #[test]
    fn hand_built_file_with_positioned_cel_and_divide() {
        // A 4x4 RGBA sprite, one layer using divide, one raw cel at (-1, 2)
        // of 3x3 pixels hanging off the canvas, and a tilemap layer.
        let mut first = Vec::new();
        let mut w = Writer::new();
        w.u16(LAYER_VISIBLE | LAYER_EDITABLE);
        w.u16(LAYER_IMAGE);
        w.u16(0);
        w.zeros(4);
        w.u16(BLEND_DIVIDE);
        w.u8(255);
        w.zeros(3);
        ase_string(&mut w, "div");
        chunk(&mut first, CHUNK_LAYER, w);
        let mut w = Writer::new();
        w.u16(LAYER_VISIBLE);
        w.u16(LAYER_TILEMAP);
        w.u16(0);
        w.zeros(4);
        w.u16(0);
        w.u8(255);
        w.zeros(3);
        ase_string(&mut w, "tiles");
        w.u32(0);
        chunk(&mut first, CHUNK_LAYER, w);
        let mut w = Writer::new();
        w.u16(0);
        w.i16(-1);
        w.i16(2);
        w.u8(255);
        w.u16(CEL_RAW);
        w.i16(1);
        w.zeros(5);
        w.u16(3);
        w.u16(3);
        for i in 0..9u8 {
            w.bytes(&[i, i, i, 255]);
        }
        chunk(&mut first, CHUNK_CEL, w);
        chunk(&mut first, 0x7777, Writer::new());
        let mut f = Writer::new();
        f.u32(0);
        f.u16(MAGIC);
        f.u16(1);
        f.u16(4);
        f.u16(4);
        f.u16(32);
        f.u32(HEADER_OPACITY_VALID);
        f.u16(0);
        f.zeros(8 + 4 + 2);
        f.u8(3);
        f.u8(1);
        f.zeros(8 + 84);
        f.u32(16 + first.iter().map(Vec::len).sum::<usize>() as u32);
        f.u16(FRAME_MAGIC);
        f.u16(first.len() as u16);
        f.u16(0);
        f.zeros(2);
        f.u32(0);
        for c in &first {
            f.bytes(c);
        }
        let imp = import_aseprite(&f.buf).unwrap();
        let d = &imp.document;
        assert_eq!(d.layers().len(), 1);
        assert_eq!(d.layers()[0].props.blend, BlendMode::Normal);
        assert_eq!(d.frames()[0].duration_ms, 100);
        let r = d.cel_raster(d.layers()[0].id, 0).unwrap().unwrap();
        assert_eq!((r.width(), r.height()), (4, 4), "canvas-sized cel");
        assert_eq!(r.get_raw(0, 2), Rgba::new(1, 1, 1, 255).to_u32());
        assert_eq!(r.get_raw(1, 3), Rgba::new(5, 5, 5, 255).to_u32());
        assert_eq!(r.get_raw(0, 0), 0);
        assert_eq!(
            imp.warnings,
            vec![
                ImportWarning::ZIndexIgnored,
                ImportWarning::UnknownChunk { kind: 0x7777 },
                ImportWarning::PixelAspectReplaced {
                    width: 3,
                    height: 1
                },
                ImportWarning::BlendModeReplaced {
                    layer: "div".into()
                },
                ImportWarning::TilemapSkipped {
                    layer: "tiles".into()
                },
            ]
        );
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
    fn truncated_and_corrupt_files_never_panic() {
        let bytes = export_aseprite(&sample(ColorMode::Rgba)).unwrap().bytes;
        for len in 0..bytes.len() {
            assert!(import_aseprite(&bytes[..len]).is_err(), "prefix {len}");
        }
        let mut next = rng(0x1234_5678_9abc_def1);
        for _ in 0..4000 {
            let mut b = bytes.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() % b.len() as u64) as usize;
                b[i] = match next() % 3 {
                    0 => b[i] ^ (1 << (next() % 8)),
                    1 => 0xff,
                    _ => 0,
                };
            }
            let _ = import_aseprite(&b);
        }
    }
}
