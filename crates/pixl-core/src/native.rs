//! The native `.pixl` document format.
//!
//! A file is an 8-byte signature, a version, then a sequence of chunks:
//!
//! ```text
//! signature  89 50 58 4C 0D 0A 1A 0A   ("\x89PXL\r\n\x1a\n")
//! version    u16 major, u16 minor
//! chunk*     [u8; 4] tag, u32 flags, u32 length, payload, u32 crc32
//! ```
//!
//! All integers are little-endian. The CRC covers the tag, flags, length and
//! payload. Flag bit 0 marks a chunk critical: a reader that does not know a
//! critical chunk refuses the file, and skips an unknown non-critical chunk
//! with a warning. Flag bit 1 marks a payload zlib-compressed; it then starts
//! with its u32 uncompressed length. A reader opens any file of its own major
//! version: a newer minor version only adds chunks, never changes one. The
//! last chunk is `END `.
//!
//! Chunks (payload after decompression):
//!
//! - `HEAD` size, color mode, transparent index, pixel aspect, background
//!   and the next free layer and image ids.
//! - `PLTE` the palette.
//! - `FRMS` frame durations.
//! - `TILE` a run of the tile pool. Every distinct 64x64 tile in the document
//!   is stored once, keyed by its content, so linked cels, copied frames and
//!   repeated tiles cost one copy.
//! - `IMAG` one pool image: its kind, size, fill value, and for every tile
//!   slot either 0 (empty) or 1 + its tile pool index.
//! - `LAYR` the layer tree, bottom first, a group followed by its children.
//! - `CELS` which image each raster layer shows on each frame.
//! - `MASK` layer masks.
//! - `TAGS` frame tags, `SLCE` slices.
//!
//! Compression is zlib (deflate) through `flate2`: pure Rust, already used by
//! PNG and needed for Aseprite cels, so pixl carries one codec for all three.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bytes::{Reader, Writer, corrupt, zlib_compress, zlib_decompress};
use crate::color::{ColorMode, Palette, Rgba};
use crate::document::{
    Background, BlendMode, Cel, Document, Frame, ImageId, Layer, LayerId, LayerKind, LayerMask,
    LayerProps, Locks, LoopDirection, MAX_SIZE, PixelAspect, Slice, Structure, Tag,
};
use crate::error::Error;
use crate::geom::{Point, Rect};
use crate::image::{Raster, TILE_AREA, Tile, TileSnap, TiledImage};

/// The first 8 bytes of every `.pixl` file.
pub const SIGNATURE: [u8; 8] = *b"\x89PXL\r\n\x1a\n";
/// The format version this build writes. It reads every file of the same
/// major version.
pub const VERSION: (u16, u16) = (1, 0);

const CRITICAL: u32 = 1;
const COMPRESSED: u32 = 2;
/// The largest chunk payload a reader accepts, after decompression.
const MAX_CHUNK: usize = 256 << 20;
/// Tiles per `TILE` chunk.
const TILES_PER_CHUNK: usize = 256;
/// The deepest layer nesting a reader accepts.
const MAX_DEPTH: usize = 256;

/// How a document is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveOptions {
    /// zlib level: 0 stores, 1 is fastest, 9 is smallest.
    pub compression: u32,
}

impl SaveOptions {
    /// What autosave uses: fast to write, still compressed.
    pub const FAST: SaveOptions = SaveOptions { compression: 1 };
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { compression: 6 }
    }
}

/// Something a file held that this build could not use. The document still
/// opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadWarning {
    /// A chunk this build does not know was skipped.
    UnknownChunk { tag: String },
    /// The file was written by a newer pixl of the same major version; what
    /// only the newer version knows was skipped.
    NewerVersion { minor: u16 },
}

impl fmt::Display for LoadWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadWarning::UnknownChunk { tag } => {
                write!(f, "skipped data this version does not know ({tag})")
            }
            LoadWarning::NewerVersion { minor } => write!(
                f,
                "the file comes from a newer pixl (format {}.{minor}); newer features were skipped",
                VERSION.0
            ),
        }
    }
}

/// A document read from a file, with what was skipped reading it.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub document: Document,
    pub warnings: Vec<LoadWarning>,
}

// ---- small codes ----

fn mode_code(m: ColorMode) -> u8 {
    match m {
        ColorMode::Rgba => 0,
        ColorMode::Indexed => 1,
    }
}

fn aspect_code(a: PixelAspect) -> u8 {
    match a {
        PixelAspect::Square => 0,
        PixelAspect::Wide => 1,
        PixelAspect::Tall => 2,
    }
}

fn direction_code(d: LoopDirection) -> u8 {
    match d {
        LoopDirection::Forward => 0,
        LoopDirection::Reverse => 1,
        LoopDirection::PingPong => 2,
    }
}

const RASTER_RGBA: u8 = 0;
const RASTER_INDEXED: u8 = 1;
const RASTER_MASK: u8 = 2;
const TILE_RGBA: u8 = 0;
const TILE_BYTE: u8 = 1;

const KIND_RASTER: u8 = 0;
const KIND_GROUP: u8 = 1;
const KIND_REFERENCE: u8 = 2;

const L_VISIBLE: u8 = 1;
const L_LOCK_PIXELS: u8 = 2;
const L_LOCK_ALPHA: u8 = 4;
const L_LOCK_POSITION: u8 = 8;
const L_CLIP: u8 = 16;
const L_COLOR_TAG: u8 = 32;

fn rgba(w: &mut Writer, c: Rgba) {
    w.bytes(&[c.r, c.g, c.b, c.a]);
}

fn read_rgba(r: &mut Reader) -> Result<Rgba, Error> {
    let [cr, g, b, a] = r.array()?;
    Ok(Rgba::new(cr, g, b, a))
}

fn string(w: &mut Writer, s: &str) {
    w.u32(s.len() as u32);
    w.bytes(s.as_bytes());
}

fn read_string(r: &mut Reader) -> Result<String, Error> {
    let len = r.u32()? as usize;
    r.utf8(len)
}

fn rect(w: &mut Writer, r: Rect) {
    for v in [r.x, r.y, r.w, r.h] {
        w.i32(v);
    }
}

fn read_rect(r: &mut Reader) -> Result<Rect, Error> {
    Ok(Rect::new(r.i32()?, r.i32()?, r.i32()?, r.i32()?))
}

// ---- writing ----

struct ChunkWriter {
    out: Vec<u8>,
    level: u32,
}

impl ChunkWriter {
    fn chunk(&mut self, tag: &[u8; 4], critical: bool, payload: Vec<u8>) {
        let mut flags = if critical { CRITICAL } else { 0 };
        let payload = if self.level > 0 && payload.len() > 64 {
            flags |= COMPRESSED;
            let mut p = (payload.len() as u32).to_le_bytes().to_vec();
            p.extend(zlib_compress(&payload, self.level));
            p
        } else {
            payload
        };
        let start = self.out.len();
        self.out.extend_from_slice(tag);
        self.out.extend_from_slice(&flags.to_le_bytes());
        self.out
            .extend_from_slice(&(payload.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&payload);
        let crc = crc32fast::hash(&self.out[start..]);
        self.out.extend_from_slice(&crc.to_le_bytes());
    }
}

/// Assigns every distinct tile one pool index. Tiles that are the same
/// allocation (linked cels, history copies) match by pointer; others match by
/// a content hash checked against the stored tile.
struct TilePool {
    by_ptr: HashMap<usize, u32>,
    by_hash: HashMap<u64, Vec<u32>>,
    tiles: Vec<TileSnap>,
    pending: Writer,
    pending_count: u32,
    pending_first: u32,
}

impl TilePool {
    fn new() -> Self {
        Self {
            by_ptr: HashMap::new(),
            by_hash: HashMap::new(),
            tiles: Vec::new(),
            pending: Writer::new(),
            pending_count: 0,
            pending_first: 0,
        }
    }

    fn intern(&mut self, snap: TileSnap, out: &mut ChunkWriter) -> u32 {
        let (ptr, hash) = match &snap {
            TileSnap::Rgba(Some(t)) => (Arc::as_ptr(t) as usize, hash_tile(TILE_RGBA, &t[..])),
            TileSnap::Byte(Some(t)) => (Arc::as_ptr(t) as usize, hash_tile(TILE_BYTE, &t[..])),
            _ => unreachable!("only present tiles are interned"),
        };
        if let Some(&i) = self.by_ptr.get(&ptr) {
            return i;
        }
        let candidates = self.by_hash.entry(hash).or_default();
        if let Some(&i) = candidates
            .iter()
            .find(|&&i| same_pixels(&self.tiles[i as usize], &snap))
        {
            self.by_ptr.insert(ptr, i);
            return i;
        }
        let index = self.tiles.len() as u32;
        candidates.push(index);
        self.by_ptr.insert(ptr, index);
        match &snap {
            TileSnap::Rgba(Some(t)) => {
                self.pending.u8(TILE_RGBA);
                for c in t.iter() {
                    rgba(&mut self.pending, *c);
                }
            }
            TileSnap::Byte(Some(t)) => {
                self.pending.u8(TILE_BYTE);
                self.pending.bytes(&t[..]);
            }
            _ => unreachable!(),
        }
        self.tiles.push(snap);
        self.pending_count += 1;
        if self.pending_count as usize >= TILES_PER_CHUNK {
            self.flush(out);
        }
        index
    }

    fn flush(&mut self, out: &mut ChunkWriter) {
        if self.pending_count == 0 {
            return;
        }
        let mut w = Writer::new();
        w.u32(self.pending_first);
        w.u32(self.pending_count);
        w.bytes(&self.pending.buf);
        out.chunk(b"TILE", true, w.buf);
        self.pending_first += self.pending_count;
        self.pending_count = 0;
        self.pending = Writer::new();
    }
}

fn hash_tile<T: Hash>(kind: u8, pixels: &[T]) -> u64 {
    let mut h = DefaultHasher::new();
    kind.hash(&mut h);
    pixels.hash(&mut h);
    h.finish()
}

fn same_pixels(a: &TileSnap, b: &TileSnap) -> bool {
    match (a, b) {
        (TileSnap::Rgba(Some(a)), TileSnap::Rgba(Some(b))) => a[..] == b[..],
        (TileSnap::Byte(Some(a)), TileSnap::Byte(Some(b))) => a[..] == b[..],
        _ => false,
    }
}

fn write_layer(w: &mut Writer, l: &Layer) {
    let p = &l.props;
    w.u32(l.id.0);
    string(w, &p.name);
    let mut flags = 0;
    for (on, bit) in [
        (p.visible, L_VISIBLE),
        (p.locks.pixels, L_LOCK_PIXELS),
        (p.locks.alpha, L_LOCK_ALPHA),
        (p.locks.position, L_LOCK_POSITION),
        (p.clip, L_CLIP),
        (p.color_tag.is_some(), L_COLOR_TAG),
    ] {
        if on {
            flags |= bit;
        }
    }
    w.u8(flags);
    w.u8(p.opacity);
    w.u8(p.blend.code() as u8);
    if let Some(c) = p.color_tag {
        rgba(w, c);
    }
    match &l.kind {
        LayerKind::Raster { .. } => w.u8(KIND_RASTER),
        LayerKind::Group { children } => {
            w.u8(KIND_GROUP);
            w.u32(children.len() as u32);
            for c in children {
                write_layer(w, c);
            }
        }
        LayerKind::Reference { image } => {
            w.u8(KIND_REFERENCE);
            w.u32(image.0);
        }
    }
}

/// Encodes a document as a `.pixl` file.
pub fn save(doc: &Document, opts: SaveOptions) -> Vec<u8> {
    let s = &doc.s;
    let mut start = Writer::new();
    start.bytes(&SIGNATURE);
    start.u16(VERSION.0);
    start.u16(VERSION.1);
    let mut out = ChunkWriter {
        out: start.buf,
        level: opts.compression,
    };

    let mut w = Writer::new();
    w.u32(s.width);
    w.u32(s.height);
    w.u8(mode_code(s.mode));
    w.u8(s.transparent_index);
    w.u8(aspect_code(s.pixel_aspect));
    match s.background {
        Background::Transparent => w.u8(0),
        Background::Color(c) => {
            w.u8(1);
            rgba(&mut w, c);
        }
    }
    w.u32(s.next_layer);
    w.u32(s.next_image);
    out.chunk(b"HEAD", true, w.buf);

    let mut w = Writer::new();
    w.u32(s.palette.len() as u32);
    for c in s.palette.colors() {
        rgba(&mut w, *c);
    }
    out.chunk(b"PLTE", true, w.buf);

    let mut w = Writer::new();
    w.u32(s.frames.len() as u32);
    for f in &s.frames {
        w.u32(f.duration_ms);
    }
    out.chunk(b"FRMS", true, w.buf);

    let mut pool = TilePool::new();
    for (id, raster) in &doc.images {
        let mut w = Writer::new();
        w.u32(id.0);
        w.u8(match raster {
            Raster::Rgba(_) => RASTER_RGBA,
            Raster::Indexed(_) => RASTER_INDEXED,
            Raster::Mask(_) => RASTER_MASK,
        });
        w.u32(raster.width());
        w.u32(raster.height());
        w.u32(raster.fill_raw());
        for t in 0..raster.tile_count() {
            let snap = raster.tile_snap(t);
            let present = matches!(snap, TileSnap::Rgba(Some(_)) | TileSnap::Byte(Some(_)));
            w.u32(if present {
                pool.intern(snap, &mut out) + 1
            } else {
                0
            });
        }
        pool.flush(&mut out);
        out.chunk(b"IMAG", true, w.buf);
    }

    let mut w = Writer::new();
    w.u32(s.layers.len() as u32);
    for l in &s.layers {
        write_layer(&mut w, l);
    }
    out.chunk(b"LAYR", true, w.buf);

    let mut cels = Writer::new();
    let mut masks = Writer::new();
    let (mut n_cels, mut n_masks) = (0u32, 0u32);
    cels.u32(0);
    masks.u32(0);
    for (_, l) in doc.walk() {
        if let LayerKind::Raster { cels: list, mask } = &l.kind {
            for (frame, cel) in list.iter().enumerate() {
                if let Some(c) = cel {
                    cels.u32(l.id.0);
                    cels.u32(frame as u32);
                    cels.u32(c.image.0);
                    cels.u8(c.opacity);
                    n_cels += 1;
                }
            }
            if let Some(m) = mask {
                masks.u32(l.id.0);
                masks.u32(m.image.0);
                masks.u8(m.enabled as u8);
                n_masks += 1;
            }
        }
    }
    cels.patch_u32(0, n_cels);
    masks.patch_u32(0, n_masks);
    out.chunk(b"CELS", true, cels.buf);
    out.chunk(b"MASK", true, masks.buf);

    let mut w = Writer::new();
    w.u32(s.tags.len() as u32);
    for t in &s.tags {
        string(&mut w, &t.name);
        w.u32(t.from as u32);
        w.u32(t.to as u32);
        w.u8(direction_code(t.direction));
        w.u32(t.repeat);
        rgba(&mut w, t.color);
    }
    out.chunk(b"TAGS", false, w.buf);

    let mut w = Writer::new();
    w.u32(s.slices.len() as u32);
    for sl in &s.slices {
        string(&mut w, &sl.name);
        rect(&mut w, sl.bounds);
        w.u8((sl.center.is_some() as u8) | ((sl.pivot.is_some() as u8) << 1));
        if let Some(c) = sl.center {
            rect(&mut w, c);
        }
        if let Some(p) = sl.pivot {
            w.i32(p.x);
            w.i32(p.y);
        }
        rgba(&mut w, sl.color);
    }
    out.chunk(b"SLCE", false, w.buf);

    out.chunk(b"END ", true, Vec::new());
    out.out
}

// ---- reading ----

struct RawImage {
    kind: u8,
    width: u32,
    height: u32,
    fill: u32,
    slots: Vec<u32>,
}

#[derive(Default)]
struct Parts {
    head: Option<Structure>,
    palette: Option<Palette>,
    frames: Option<Vec<Frame>>,
    tiles: Vec<TileSnap>,
    images: BTreeMap<ImageId, RawImage>,
    layers: Option<Vec<Layer>>,
    cels: Option<Vec<(LayerId, usize, Cel)>>,
    masks: Option<Vec<(LayerId, LayerMask)>>,
    tags: Option<Vec<Tag>>,
    slices: Option<Vec<Slice>>,
}

fn once<T>(slot: &mut Option<T>, value: T, tag: &str) -> Result<(), Error> {
    if slot.is_some() {
        return Err(corrupt(format!("{tag} chunk appears twice")));
    }
    *slot = Some(value);
    Ok(())
}

fn read_head(r: &mut Reader) -> Result<Structure, Error> {
    let width = r.u32()?;
    let height = r.u32()?;
    if width == 0 || height == 0 || width > MAX_SIZE || height > MAX_SIZE {
        return Err(Error::InvalidSize { width, height });
    }
    let mode = match r.u8()? {
        0 => ColorMode::Rgba,
        1 => ColorMode::Indexed,
        m => return Err(corrupt(format!("unknown color mode {m}"))),
    };
    let transparent_index = r.u8()?;
    let pixel_aspect = match r.u8()? {
        0 => PixelAspect::Square,
        1 => PixelAspect::Wide,
        2 => PixelAspect::Tall,
        a => return Err(corrupt(format!("unknown pixel aspect {a}"))),
    };
    let background = match r.u8()? {
        0 => Background::Transparent,
        1 => Background::Color(read_rgba(r)?),
        b => return Err(corrupt(format!("unknown background {b}"))),
    };
    let next_layer = r.u32()?;
    let next_image = r.u32()?;
    Ok(Structure {
        width,
        height,
        mode,
        palette: Palette::default(),
        transparent_index,
        pixel_aspect,
        background,
        layers: Vec::new(),
        frames: Vec::new(),
        tags: Vec::new(),
        slices: Vec::new(),
        next_layer,
        next_image,
    })
}

fn read_tiles(r: &mut Reader, pool: &mut Vec<TileSnap>) -> Result<(), Error> {
    let first = r.u32()? as usize;
    if first != pool.len() {
        return Err(corrupt("tile chunks out of order"));
    }
    let count = r.count(1 + TILE_AREA)?;
    for _ in 0..count {
        match r.u8()? {
            TILE_RGBA => {
                let bytes = r.take(TILE_AREA * 4)?;
                let mut t: Box<Tile<Rgba>> = Box::new([Rgba::TRANSPARENT; TILE_AREA]);
                for (px, c) in t.iter_mut().zip(bytes.chunks_exact(4)) {
                    *px = Rgba::new(c[0], c[1], c[2], c[3]);
                }
                pool.push(TileSnap::Rgba(Some(Arc::from(t))));
            }
            TILE_BYTE => {
                let bytes = r.take(TILE_AREA)?;
                let mut t: Box<Tile<u8>> = Box::new([0; TILE_AREA]);
                t.copy_from_slice(bytes);
                pool.push(TileSnap::Byte(Some(Arc::from(t))));
            }
            k => return Err(corrupt(format!("unknown tile kind {k}"))),
        }
    }
    Ok(())
}

fn read_image(r: &mut Reader) -> Result<(ImageId, RawImage), Error> {
    let id = ImageId(r.u32()?);
    let kind = r.u8()?;
    if kind > RASTER_MASK {
        return Err(corrupt(format!("unknown image kind {kind}")));
    }
    let width = r.u32()?;
    let height = r.u32()?;
    if width == 0 || height == 0 || width > MAX_SIZE || height > MAX_SIZE {
        return Err(Error::InvalidSize { width, height });
    }
    let fill = r.u32()?;
    let slots = TiledImage::new(width, height, 0u8).tile_count();
    if slots * 4 > r.remaining() {
        return Err(corrupt("image tile table is truncated"));
    }
    let slots = (0..slots).map(|_| r.u32()).collect::<Result<_, _>>()?;
    Ok((
        id,
        RawImage {
            kind,
            width,
            height,
            fill,
            slots,
        },
    ))
}

fn read_layer(r: &mut Reader, depth: usize) -> Result<Layer, Error> {
    if depth > MAX_DEPTH {
        return Err(corrupt("layers nest too deep"));
    }
    let id = LayerId(r.u32()?);
    let name = read_string(r)?;
    let flags = r.u8()?;
    let opacity = r.u8()?;
    let blend_code = r.u8()?;
    let blend = BlendMode::from_code(blend_code as u16)
        .ok_or_else(|| corrupt(format!("unknown blend mode {blend_code}")))?;
    let color_tag = if flags & L_COLOR_TAG != 0 {
        Some(read_rgba(r)?)
    } else {
        None
    };
    let props = LayerProps {
        name,
        visible: flags & L_VISIBLE != 0,
        locks: Locks {
            pixels: flags & L_LOCK_PIXELS != 0,
            alpha: flags & L_LOCK_ALPHA != 0,
            position: flags & L_LOCK_POSITION != 0,
        },
        opacity,
        blend,
        color_tag,
        clip: flags & L_CLIP != 0,
    };
    let kind = match r.u8()? {
        KIND_RASTER => LayerKind::Raster {
            cels: Vec::new(),
            mask: None,
        },
        KIND_GROUP => {
            let n = r.count(12)?;
            let mut children = Vec::with_capacity(n);
            for _ in 0..n {
                children.push(read_layer(r, depth + 1)?);
            }
            LayerKind::Group { children }
        }
        KIND_REFERENCE => LayerKind::Reference {
            image: ImageId(r.u32()?),
        },
        k => return Err(corrupt(format!("unknown layer kind {k}"))),
    };
    Ok(Layer { id, props, kind })
}

fn read_chunk_payload(tag: &[u8; 4], r: &mut Reader, parts: &mut Parts) -> Result<(), Error> {
    match tag {
        b"HEAD" => once(&mut parts.head, read_head(r)?, "HEAD")?,
        b"PLTE" => {
            let n = r.count(4)?;
            let colors = (0..n).map(|_| read_rgba(r)).collect::<Result<_, _>>()?;
            once(&mut parts.palette, Palette::new(colors)?, "PLTE")?;
        }
        b"FRMS" => {
            let n = r.count(4)?;
            let frames = (0..n)
                .map(|_| r.u32().map(|duration_ms| Frame { duration_ms }))
                .collect::<Result<_, _>>()?;
            once(&mut parts.frames, frames, "FRMS")?;
        }
        b"TILE" => read_tiles(r, &mut parts.tiles)?,
        b"IMAG" => {
            let (id, img) = read_image(r)?;
            if parts.images.insert(id, img).is_some() {
                return Err(corrupt(format!("image {} appears twice", id.0)));
            }
        }
        b"LAYR" => {
            let n = r.count(12)?;
            let layers = (0..n).map(|_| read_layer(r, 0)).collect::<Result<_, _>>()?;
            once(&mut parts.layers, layers, "LAYR")?;
        }
        b"CELS" => {
            let n = r.count(13)?;
            let mut cels = Vec::with_capacity(n);
            for _ in 0..n {
                let layer = LayerId(r.u32()?);
                let frame = r.u32()? as usize;
                let image = ImageId(r.u32()?);
                let opacity = r.u8()?;
                cels.push((layer, frame, Cel { image, opacity }));
            }
            once(&mut parts.cels, cels, "CELS")?;
        }
        b"MASK" => {
            let n = r.count(9)?;
            let mut masks = Vec::with_capacity(n);
            for _ in 0..n {
                let layer = LayerId(r.u32()?);
                let image = ImageId(r.u32()?);
                let enabled = r.u8()? != 0;
                masks.push((layer, LayerMask { image, enabled }));
            }
            once(&mut parts.masks, masks, "MASK")?;
        }
        b"TAGS" => {
            let n = r.count(21)?;
            let mut tags = Vec::with_capacity(n);
            for _ in 0..n {
                let name = read_string(r)?;
                let from = r.u32()? as usize;
                let to = r.u32()? as usize;
                let direction = match r.u8()? {
                    0 => LoopDirection::Forward,
                    1 => LoopDirection::Reverse,
                    2 => LoopDirection::PingPong,
                    d => return Err(corrupt(format!("unknown tag direction {d}"))),
                };
                let repeat = r.u32()?;
                let color = read_rgba(r)?;
                tags.push(Tag {
                    name,
                    from,
                    to,
                    direction,
                    repeat,
                    color,
                });
            }
            once(&mut parts.tags, tags, "TAGS")?;
        }
        b"SLCE" => {
            let n = r.count(25)?;
            let mut slices = Vec::with_capacity(n);
            for _ in 0..n {
                let name = read_string(r)?;
                let bounds = read_rect(r)?;
                let has = r.u8()?;
                let center = if has & 1 != 0 {
                    Some(read_rect(r)?)
                } else {
                    None
                };
                let pivot = if has & 2 != 0 {
                    Some(Point::new(r.i32()?, r.i32()?))
                } else {
                    None
                };
                let color = read_rgba(r)?;
                slices.push(Slice {
                    name,
                    bounds,
                    center,
                    pivot,
                    color,
                });
            }
            once(&mut parts.slices, slices, "SLCE")?;
        }
        _ => unreachable!("only known tags reach the payload reader"),
    }
    if !r.is_empty() {
        return Err(corrupt(format!(
            "{} chunk has trailing bytes",
            String::from_utf8_lossy(tag)
        )));
    }
    Ok(())
}

const KNOWN: [&[u8; 4]; 11] = [
    b"HEAD", b"PLTE", b"FRMS", b"TILE", b"IMAG", b"LAYR", b"CELS", b"MASK", b"TAGS", b"SLCE",
    b"END ",
];

/// Decodes a `.pixl` file. A truncated or corrupt file is an error, never a
/// panic; data this build does not know is skipped and listed as a warning.
pub fn load(bytes: &[u8]) -> Result<Loaded, Error> {
    let mut r = Reader::new(bytes);
    if r.take(8).ok() != Some(&SIGNATURE[..]) {
        return Err(corrupt("not a pixl file"));
    }
    let major = r.u16()?;
    let minor = r.u16()?;
    if major != VERSION.0 {
        return Err(corrupt(format!(
            "format version {major}.{minor} is not supported (this pixl reads {}.x)",
            VERSION.0
        )));
    }
    let mut warnings = Vec::new();
    if minor > VERSION.1 {
        warnings.push(LoadWarning::NewerVersion { minor });
    }
    let mut parts = Parts::default();
    loop {
        let start = r.pos();
        let tag: [u8; 4] = r.array()?;
        let flags = r.u32()?;
        let len = r.u32()? as usize;
        let payload = r.take(len)?;
        let crc = r.u32()?;
        if crc32fast::hash(&bytes[start..start + 12 + len]) != crc {
            return Err(corrupt(format!(
                "{} chunk is damaged (checksum mismatch)",
                String::from_utf8_lossy(&tag)
            )));
        }
        if !KNOWN.contains(&&tag) {
            let name = String::from_utf8_lossy(&tag).into_owned();
            if flags & CRITICAL != 0 {
                return Err(corrupt(format!(
                    "the file needs a newer pixl (unknown required data {name})"
                )));
            }
            warnings.push(LoadWarning::UnknownChunk { tag: name });
            continue;
        }
        if &tag == b"END " {
            break;
        }
        let data;
        let mut pr = if flags & COMPRESSED != 0 {
            let mut cr = Reader::new(payload);
            let size = cr.u32()? as usize;
            if size > MAX_CHUNK {
                return Err(corrupt("chunk too large"));
            }
            data = zlib_decompress(&payload[4..], size)?;
            Reader::new(&data)
        } else {
            Reader::new(payload)
        };
        read_chunk_payload(&tag, &mut pr, &mut parts)?;
    }
    let document = assemble(parts)?;
    Ok(Loaded { document, warnings })
}

fn assemble(parts: Parts) -> Result<Document, Error> {
    let missing = |what: &str| corrupt(format!("the file has no {what} chunk"));
    let mut s = parts.head.ok_or_else(|| missing("HEAD"))?;
    s.palette = parts.palette.ok_or_else(|| missing("PLTE"))?;
    s.frames = parts.frames.ok_or_else(|| missing("FRMS"))?;
    if s.frames.is_empty() {
        return Err(Error::LastFrame);
    }
    let frames = s.frames.len();

    let mut images = BTreeMap::new();
    for (id, raw) in parts.images {
        if raw.width != s.width || raw.height != s.height {
            return Err(corrupt(format!("image {} has the wrong size", id.0)));
        }
        let tile = |slot: u32, want: u8| -> Result<Option<&TileSnap>, Error> {
            if slot == 0 {
                return Ok(None);
            }
            let t = parts
                .tiles
                .get(slot as usize - 1)
                .ok_or_else(|| corrupt("image refers to a missing tile"))?;
            let kind = match t {
                TileSnap::Rgba(_) => TILE_RGBA,
                TileSnap::Byte(_) => TILE_BYTE,
            };
            if kind != want {
                return Err(corrupt("image refers to a tile of another kind"));
            }
            Ok(Some(t))
        };
        let want = if raw.kind == RASTER_RGBA {
            TILE_RGBA
        } else {
            TILE_BYTE
        };
        let mut raster = match raw.kind {
            RASTER_RGBA => Raster::Rgba(TiledImage::new(
                raw.width,
                raw.height,
                Rgba::from_u32(raw.fill),
            )),
            RASTER_INDEXED => {
                Raster::Indexed(TiledImage::new(raw.width, raw.height, raw.fill as u8))
            }
            _ => Raster::Mask(TiledImage::new(raw.width, raw.height, raw.fill as u8)),
        };
        for (i, slot) in raw.slots.iter().enumerate() {
            if let Some(t) = tile(*slot, want)? {
                raster.restore_tile(i, t);
            }
        }
        images.insert(id, raster);
    }

    s.layers = parts.layers.ok_or_else(|| missing("LAYR"))?;
    let mut ids = BTreeSet::new();
    let mut check = |l: &Layer| -> Result<(), Error> {
        if !ids.insert(l.id) {
            return Err(corrupt(format!("layer id {} appears twice", l.id.0)));
        }
        if let LayerKind::Reference { image } = l.kind
            && !matches!(images.get(&image), Some(Raster::Rgba(_)))
        {
            return Err(corrupt("reference layer without an RGBA image"));
        }
        Ok(())
    };
    fn visit(
        layers: &mut [Layer],
        frames: usize,
        f: &mut dyn FnMut(&Layer) -> Result<(), Error>,
    ) -> Result<(), Error> {
        for l in layers {
            f(l)?;
            match &mut l.kind {
                LayerKind::Raster { cels, .. } => *cels = vec![None; frames],
                LayerKind::Group { children } => visit(children, frames, f)?,
                LayerKind::Reference { .. } => {}
            }
        }
        Ok(())
    }
    visit(&mut s.layers, frames, &mut check)?;
    let max_layer = ids.last().map_or(0, |id| id.0);
    let max_image = images.keys().last().map_or(0, |id| id.0);
    s.next_layer = s.next_layer.max(max_layer.saturating_add(1));
    s.next_image = s.next_image.max(max_image.saturating_add(1));

    let mut doc = Document { s, images };
    let cel_kind_ok = |r: &Raster, mode: ColorMode| {
        matches!(
            (r, mode),
            (Raster::Rgba(_), ColorMode::Rgba) | (Raster::Indexed(_), ColorMode::Indexed)
        )
    };
    for (layer, frame, cel) in parts.cels.ok_or_else(|| missing("CELS"))? {
        if frame >= frames {
            return Err(Error::FrameOutOfRange(frame));
        }
        let mode = doc.s.mode;
        if !doc
            .images
            .get(&cel.image)
            .is_some_and(|r| cel_kind_ok(r, mode))
        {
            return Err(corrupt(
                "cel refers to a missing image or one of another kind",
            ));
        }
        match &mut doc
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Raster { cels, .. } => {
                if cels[frame].replace(cel).is_some() {
                    return Err(corrupt("a cel appears twice"));
                }
            }
            _ => return Err(Error::NotRaster(layer)),
        }
    }
    for (layer, m) in parts.masks.ok_or_else(|| missing("MASK"))? {
        if !matches!(doc.images.get(&m.image), Some(Raster::Mask(_))) {
            return Err(corrupt(
                "mask refers to a missing image or one of another kind",
            ));
        }
        match &mut doc
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Raster { mask, .. } => {
                if mask.replace(m).is_some() {
                    return Err(corrupt("a layer has two masks"));
                }
            }
            _ => return Err(Error::NotRaster(layer)),
        }
    }
    let tags = parts.tags.unwrap_or_default();
    if tags.iter().any(|t| t.from > t.to || t.to >= frames) {
        return Err(corrupt("a tag covers frames that do not exist"));
    }
    doc.s.tags = tags;
    doc.s.slices = parts.slices.unwrap_or_default();
    Ok(doc)
}

// ---- files ----

/// Writes `bytes` to `path` so that a crash leaves either the old file or the
/// new one, never a partial file: the data goes to a temporary file in the
/// same directory, is flushed to disk, then renamed over `path`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| Error::Io(format!("{} is not a file path", path.display())))?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        // Make the rename itself durable. Directories cannot be opened for
        // syncing on every platform, so a failure here is not an error.
        if let Ok(d) = std::fs::File::open(&dir) {
            let _ = d.sync_all();
        }
        Ok::<_, std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Saves a document to `path` atomically (see [`write_atomic`]).
pub fn save_file(doc: &Document, path: &Path, opts: SaveOptions) -> Result<(), Error> {
    write_atomic(path, &save(doc, opts))
}

/// Opens a `.pixl` file.
pub fn load_file(path: &Path) -> Result<Loaded, Error> {
    load(&std::fs::read(path)?)
}

/// Where the recovery copy of the document at `path` lives: a hidden file
/// beside it, `.<name>.recovery`.
pub fn recovery_path(path: &Path) -> PathBuf {
    let name = path.file_name().map_or_else(
        || "untitled.pixl".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!(".{name}.recovery"))
}

/// Writes the recovery copy of a document whose file is `path`. Autosave
/// calls this; the document file itself is untouched until the user saves.
pub fn write_recovery(doc: &Document, path: &Path) -> Result<(), Error> {
    write_atomic(&recovery_path(path), &save(doc, SaveOptions::FAST))
}

/// The recovery copy of `path` when one exists and is newer than the
/// document file (or the document file is gone): the edits a crash would
/// otherwise have lost. Open it with [`load_file`].
pub fn find_recovery(path: &Path) -> Option<PathBuf> {
    let rec = recovery_path(path);
    let rec_time = std::fs::metadata(&rec).and_then(|m| m.modified()).ok()?;
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(doc_time) if doc_time > rec_time => None,
        _ => Some(rec),
    }
}

/// Deletes the recovery copy of `path`, after a save or when the user
/// declines to recover.
pub fn discard_recovery(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(recovery_path(path)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{NewLayer, Placement};

    /// A document using every layer kind, every blend mode, masks, linked
    /// cels, tags and slices.
    fn rich(mode: ColorMode) -> Document {
        let mut d = Document::new(130, 70, mode).unwrap();
        let base = d.layers()[0].id;
        d.insert_frame(1, 40).unwrap();
        d.insert_frame(2, 250).unwrap();
        let paint = |d: &mut Document, layer: LayerId, frame: usize, seed: u32| {
            let t = d.target(layer, frame).unwrap();
            for i in 0..200 {
                let x = ((i * 37 + seed * 11) % 130) as i32;
                let y = ((i * 13 + seed * 7) % 70) as i32;
                let raw = match t.raster {
                    Raster::Rgba(_) => Rgba::new(i as u8, seed as u8, 200, 255 - i as u8).to_u32(),
                    _ => 1 + (i + seed) % 15,
                };
                t.raster.set_raw(x, y, raw);
            }
        };
        paint(&mut d, base, 0, 1);
        paint(&mut d, base, 2, 2);
        d.link_cel(base, 0, 1).unwrap();
        d.set_cel_opacity(base, 2, 77).unwrap();
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
            let parent = if i % 2 == 0 { g } else { inner };
            let id = d
                .add_layer(
                    NewLayer::Raster,
                    format!("blend {i}"),
                    Placement {
                        parent: Some(parent),
                        index: None,
                    },
                )
                .unwrap();
            d.update_layer(id, |p| {
                p.blend = mode;
                p.opacity = 255 - i as u8 * 9;
                p.visible = i % 3 != 0;
                p.clip = i % 4 == 1;
                p.locks = Locks {
                    pixels: i % 5 == 0,
                    alpha: i % 2 == 1,
                    position: i % 3 == 2,
                };
                p.color_tag = (i % 2 == 0).then_some(Rgba::new(i as u8, 2, 3, 255));
            })
            .unwrap();
            if !d.layer(id).unwrap().props.locks.pixels {
                paint(&mut d, id, i % 3, i as u32);
            }
            if i == 3 {
                d.add_layer_mask(id).unwrap();
                d.mask_target(id).unwrap().raster.set_raw(5, 5, 0);
                d.set_mask_enabled(id, false).unwrap();
            }
            if i == 4 {
                d.add_layer_mask(id).unwrap();
            }
        }
        d.update_layer(g, |p| {
            p.blend = BlendMode::Screen;
            p.opacity = 128;
        })
        .unwrap();
        let mut reference = TiledImage::new(130, 70, Rgba::TRANSPARENT);
        reference.set(9, 9, Rgba::opaque(1, 2, 3));
        d.add_layer(
            NewLayer::Reference(reference),
            "ref",
            Placement {
                parent: Some(inner),
                index: Some(0),
            },
        )
        .unwrap();
        for (i, direction) in [
            LoopDirection::Forward,
            LoopDirection::Reverse,
            LoopDirection::PingPong,
        ]
        .into_iter()
        .enumerate()
        {
            d.tags_mut().push(Tag {
                name: format!("tag {i}"),
                from: i.min(1),
                to: 2,
                direction,
                repeat: i as u32,
                color: Rgba::new(10, 20, 30, 255),
            });
        }
        d.slices_mut().push(Slice {
            name: "button".into(),
            bounds: Rect::new(-3, 4, 20, 10),
            center: Some(Rect::new(2, 2, 16, 6)),
            pivot: Some(Point::new(10, -1)),
            color: Rgba::WHITE,
        });
        d.slices_mut().push(Slice {
            name: "plain".into(),
            bounds: Rect::new(0, 0, 1, 1),
            center: None,
            pivot: None,
            color: Rgba::BLACK,
        });
        d.set_background(Background::Color(Rgba::opaque(4, 5, 6)));
        d.set_pixel_aspect(PixelAspect::Tall);
        if mode == ColorMode::Indexed {
            d.set_transparent_index(0);
        }
        d
    }

    /// Chunk tags and payload ranges of an encoded file.
    fn chunks(bytes: &[u8]) -> Vec<([u8; 4], u32, std::ops::Range<usize>)> {
        let mut out = Vec::new();
        let mut pos = 12;
        while pos < bytes.len() {
            let tag: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap();
            let flags = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap());
            let len = u32::from_le_bytes(bytes[pos + 8..pos + 12].try_into().unwrap()) as usize;
            out.push((tag, flags, pos + 12..pos + 12 + len));
            pos += 16 + len;
        }
        out
    }

    fn fix_crc(bytes: &mut [u8], payload: std::ops::Range<usize>) {
        let crc = crc32fast::hash(&bytes[payload.start - 12..payload.end]);
        bytes[payload.end..payload.end + 4].copy_from_slice(&crc.to_le_bytes());
    }

    fn chunk(tag: &[u8; 4], flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut c = tag.to_vec();
        c.extend(flags.to_le_bytes());
        c.extend((payload.len() as u32).to_le_bytes());
        c.extend(payload);
        let crc = crc32fast::hash(&c);
        c.extend(crc.to_le_bytes());
        c
    }

    #[test]
    fn round_trips_every_layer_kind_and_blend_mode() {
        for mode in [ColorMode::Rgba, ColorMode::Indexed] {
            let d = rich(mode);
            for level in [0, 1, 9] {
                let bytes = save(&d, SaveOptions { compression: level });
                let loaded = load(&bytes).unwrap();
                assert!(loaded.warnings.is_empty());
                assert_eq!(loaded.document, d, "{mode:?} at level {level}");
            }
        }
    }

    #[test]
    fn loaded_document_keeps_working() {
        let d = rich(ColorMode::Rgba);
        let mut loaded = load(&save(&d, SaveOptions::default())).unwrap().document;
        let base = loaded.layers()[0].id;
        assert_eq!(loaded.linked_frames(base, 0).unwrap(), vec![0, 1]);
        let id = loaded
            .add_layer(NewLayer::Raster, "new", Placement::default())
            .unwrap();
        assert!(id.0 >= d.s.next_layer, "fresh ids do not collide");
        loaded.target(id, 0).unwrap().raster.set_raw(0, 0, 1);
    }

    #[test]
    fn tiles_are_stored_once() {
        let mut d = Document::new(256, 256, ColorMode::Rgba).unwrap();
        let base = d.layers()[0].id;
        d.insert_frame(1, 100).unwrap();
        d.insert_frame(2, 100).unwrap();
        // The same pattern in all 16 tiles of frame 0, linked into frame 1;
        // frame 2 is an unlinked copy.
        let t = d.target(base, 0).unwrap();
        for y in 0..256 {
            for x in 0..256 {
                if (x + y) % 4 == 0 {
                    t.raster.set_raw(x, y, Rgba::opaque(9, 8, 7).to_u32());
                }
            }
        }
        d.link_cel(base, 0, 1).unwrap();
        let copy = d.cel_raster(base, 0).unwrap().unwrap().clone();
        let t = d.target(base, 2).unwrap();
        for p in copy.bounds().points() {
            t.raster.set_raw(p.x, p.y, copy.get_raw(p.x, p.y));
        }
        let bytes = save(&d, SaveOptions { compression: 0 });
        let tiles: u32 = chunks(&bytes)
            .iter()
            .filter(|(tag, ..)| tag == b"TILE")
            .map(|(_, _, p)| {
                u32::from_le_bytes(bytes[p.start + 4..p.start + 8].try_into().unwrap())
            })
            .sum();
        assert_eq!(tiles, 1, "48 tile slots, one distinct tile");
        let loaded = load(&bytes).unwrap().document;
        assert_eq!(loaded, d);
        let a = loaded.cel_raster(base, 0).unwrap().unwrap();
        let b = loaded.cel_raster(base, 2).unwrap().unwrap();
        let ptr = |r: &Raster, i: usize| match r.tile_snap(i) {
            TileSnap::Rgba(Some(t)) => Arc::as_ptr(&t),
            _ => panic!("tile {i} is empty"),
        };
        let first = ptr(a, 0);
        assert!(
            (0..16).all(|i| ptr(a, i) == first && ptr(b, i) == first),
            "loaded tiles share memory too"
        );
    }

    #[test]
    fn compression_shrinks_files() {
        let d = rich(ColorMode::Rgba);
        let stored = save(&d, SaveOptions { compression: 0 }).len();
        let packed = save(&d, SaveOptions::default()).len();
        assert!(packed * 2 < stored, "{packed} vs {stored}");
    }

    #[test]
    fn unknown_chunks_skip_or_refuse() {
        let d = rich(ColorMode::Rgba);
        let bytes = save(&d, SaveOptions::default());
        let end = chunks(&bytes).last().unwrap().2.start - 12;
        let with = |c: Vec<u8>| {
            let mut b = bytes[..end].to_vec();
            b.extend(c);
            b.extend(&bytes[end..]);
            b
        };
        let loaded = load(&with(chunk(b"ANIM", 0, b"future data"))).unwrap();
        assert_eq!(loaded.document, d);
        assert_eq!(
            loaded.warnings,
            vec![LoadWarning::UnknownChunk { tag: "ANIM".into() }]
        );
        assert!(load(&with(chunk(b"ANIM", CRITICAL, b"x"))).is_err());
    }

    #[test]
    fn versions() {
        let d = rich(ColorMode::Rgba);
        let mut bytes = save(&d, SaveOptions::default());
        bytes[10] = 3;
        let loaded = load(&bytes).unwrap();
        assert_eq!(
            loaded.warnings,
            vec![LoadWarning::NewerVersion { minor: 3 }]
        );
        bytes[8] = 2;
        assert!(load(&bytes).is_err());
        assert!(load(b"\x89PNG\r\n\x1a\n").is_err());
    }

    /// A tiny deterministic generator for the fuzz tests.
    fn rng(mut s: u64) -> impl FnMut() -> u64 {
        move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        }
    }

    #[test]
    fn truncated_files_error() {
        let bytes = save(&rich(ColorMode::Indexed), SaveOptions::default());
        for len in 0..bytes.len() {
            assert!(load(&bytes[..len]).is_err(), "prefix of {len} bytes");
        }
    }

    #[test]
    fn corrupt_files_never_panic() {
        let bytes = save(&rich(ColorMode::Rgba), SaveOptions::default());
        let mut next = rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..3000 {
            let mut b = bytes.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() % b.len() as u64) as usize;
                b[i] ^= 1 << (next() % 8);
            }
            let _ = load(&b);
        }
    }

    #[test]
    fn corrupt_payloads_with_valid_checksums_never_panic() {
        // Stored chunks with re-computed checksums, so the damage reaches
        // the chunk parsers instead of stopping at the checksum.
        for compression in [0, 6] {
            let bytes = save(&rich(ColorMode::Rgba), SaveOptions { compression });
            let list = chunks(&bytes);
            let mut next = rng(0x2545_f491_4f6c_dd1d);
            for _ in 0..4000 {
                let mut b = bytes.clone();
                let (_, _, range) = &list[(next() % list.len() as u64) as usize];
                if range.is_empty() {
                    continue;
                }
                for _ in 0..1 + next() % 3 {
                    let i = range.start + (next() % range.len() as u64) as usize;
                    b[i] = match next() % 3 {
                        0 => b[i] ^ (1 << (next() % 8)),
                        1 => 0xff,
                        _ => 0,
                    };
                }
                fix_crc(&mut b, range.clone());
                let _ = load(&b);
            }
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pixl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_save_replaces_and_leaves_no_temp_file() {
        let dir = temp_dir("save");
        let path = dir.join("art.pixl");
        let a = rich(ColorMode::Rgba);
        save_file(&a, &path, SaveOptions::default()).unwrap();
        let mut b = a.clone();
        b.set_background(Background::Transparent);
        save_file(&b, &path, SaveOptions::default()).unwrap();
        assert_eq!(load_file(&path).unwrap().document, b);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("art.pixl")]);
        assert!(save_file(&a, &dir.join("missing").join("x.pixl"), SaveOptions::FAST).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recovery_copy_lifecycle() {
        let dir = temp_dir("recovery");
        let path = dir.join("art.pixl");
        assert_eq!(recovery_path(&path), dir.join(".art.pixl.recovery"));
        let d = rich(ColorMode::Rgba);
        assert_eq!(find_recovery(&path), None);
        // Never saved: the recovery copy is all there is.
        write_recovery(&d, &path).unwrap();
        let found = find_recovery(&path).unwrap();
        assert_eq!(load_file(&found).unwrap().document, d);
        discard_recovery(&path).unwrap();
        discard_recovery(&path).unwrap();
        assert_eq!(find_recovery(&path), None);
        // A save newer than the recovery copy supersedes it.
        write_recovery(&d, &path).unwrap();
        save_file(&d, &path, SaveOptions::default()).unwrap();
        let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(recovery_path(&path))
            .unwrap()
            .set_modified(earlier)
            .unwrap();
        assert_eq!(find_recovery(&path), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
