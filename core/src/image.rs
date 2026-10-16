//! Tiled pixel storage.
//!
//! An image is a grid of 64x64 tiles. A tile that was never written is absent
//! and reads as the image's fill value, so an empty layer costs almost
//! nothing. Present tiles sit behind an [`Arc`]: cloning an image copies only
//! the tile pointers, and the first write to a shared tile copies that tile
//! alone. History snapshots and linked cels rely on this.

use std::fmt::Debug;
use std::sync::Arc;

use crate::color::{ColorMode, Rgba};
use crate::geom::Rect;

/// Tile edge length in pixels.
pub const TILE_SIZE: i32 = 64;
/// Pixels per tile.
pub const TILE_AREA: usize = (TILE_SIZE * TILE_SIZE) as usize;

/// A pixel format a [`TiledImage`] can store.
pub trait Pixel: Copy + PartialEq + Debug + Send + Sync + 'static {}

impl Pixel for Rgba {}
impl Pixel for u8 {}
impl Pixel for bool {}

/// One tile's pixels, row-major.
pub type Tile<P> = [P; TILE_AREA];

/// A width x height image stored as copy-on-write tiles.
#[derive(Clone, Debug)]
pub struct TiledImage<P: Pixel> {
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
    fill: P,
    tiles: Vec<Option<Arc<Tile<P>>>>,
}

impl<P: Pixel> TiledImage<P> {
    /// An image where every pixel reads as `fill`.
    pub fn new(width: u32, height: u32, fill: P) -> Self {
        let cols = width.div_ceil(TILE_SIZE as u32);
        let rows = height.div_ceil(TILE_SIZE as u32);
        Self {
            width,
            height,
            cols,
            rows,
            fill,
            tiles: vec![None; (cols * rows) as usize],
        }
    }

    /// An image from row-major pixels. Panics if `pixels` has the wrong length.
    pub fn from_pixels(width: u32, height: u32, fill: P, pixels: &[P]) -> Self {
        assert_eq!(pixels.len(), width as usize * height as usize);
        let mut img = Self::new(width, height, fill);
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                img.set(x, y, pixels[(y as u32 * width + x as u32) as usize]);
            }
        }
        img
    }

    /// Every pixel, row-major.
    pub fn to_pixels(&self) -> Vec<P> {
        let mut out = Vec::with_capacity(self.width as usize * self.height as usize);
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                out.push(self.get(x, y));
            }
        }
        out
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    /// The value of pixels in absent tiles.
    pub fn fill(&self) -> P {
        self.fill
    }

    /// Tile grid columns and rows.
    pub fn tile_grid(&self) -> (u32, u32) {
        (self.cols, self.rows)
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// The pixel rectangle tile `index` covers, clipped to the image.
    pub fn tile_rect(&self, index: usize) -> Rect {
        let tx = (index as u32 % self.cols) as i32;
        let ty = (index as u32 / self.cols) as i32;
        Rect::new(tx * TILE_SIZE, ty * TILE_SIZE, TILE_SIZE, TILE_SIZE).intersect(&self.bounds())
    }

    /// The indices of tiles that overlap `rect`.
    pub fn tiles_in(&self, rect: &Rect) -> impl Iterator<Item = usize> + use<P> {
        let r = rect.intersect(&self.bounds());
        let cols = self.cols as usize;
        let (tx0, ty0, tx1, ty1) = if r.is_empty() {
            (0, 0, 0, 0)
        } else {
            (
                (r.x / TILE_SIZE) as usize,
                (r.y / TILE_SIZE) as usize,
                ((r.right() - 1) / TILE_SIZE) as usize + 1,
                ((r.bottom() - 1) / TILE_SIZE) as usize + 1,
            )
        };
        (ty0..ty1).flat_map(move |ty| (tx0..tx1).map(move |tx| ty * cols + tx))
    }

    /// The tile at `index`, or `None` if it is absent (all fill).
    pub fn tile(&self, index: usize) -> Option<&Arc<Tile<P>>> {
        self.tiles[index].as_ref()
    }

    /// Replaces the tile at `index`. History uses this to restore snapshots.
    pub fn set_tile(&mut self, index: usize, tile: Option<Arc<Tile<P>>>) {
        self.tiles[index] = tile;
    }

    /// Whether tile `index` is the same allocation in both images (both absent
    /// counts as the same).
    pub fn same_tile(&self, other: &Self, index: usize) -> bool {
        match (&self.tiles[index], &other.tiles[index]) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Writable pixels of tile `index`, allocated or unshared as needed.
    pub fn tile_mut(&mut self, index: usize) -> &mut Tile<P> {
        let fill = self.fill;
        let slot = &mut self.tiles[index];
        Arc::make_mut(slot.get_or_insert_with(|| Arc::new([fill; TILE_AREA])))
    }

    fn locate(&self, x: i32, y: i32) -> Option<(usize, usize)> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        let tile = (y / TILE_SIZE) as usize * self.cols as usize + (x / TILE_SIZE) as usize;
        let offset = ((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) as usize;
        Some((tile, offset))
    }

    /// The pixel at `(x, y)`; outside the image reads as the fill value.
    pub fn get(&self, x: i32, y: i32) -> P {
        match self.locate(x, y) {
            Some((t, o)) => match &self.tiles[t] {
                Some(tile) => tile[o],
                None => self.fill,
            },
            None => self.fill,
        }
    }

    /// Writes one pixel. Returns whether the image changed; writes outside the
    /// image and writes of the current value change nothing and copy no tile.
    pub fn set(&mut self, x: i32, y: i32, value: P) -> bool {
        let Some((t, o)) = self.locate(x, y) else {
            return false;
        };
        let current = match &self.tiles[t] {
            Some(tile) => tile[o],
            None => self.fill,
        };
        if current == value {
            return false;
        }
        self.tile_mut(t)[o] = value;
        true
    }

    /// Sets every pixel of `rect` and returns the changed area. A whole tile
    /// cleared back to the fill value reports the tile's area.
    pub fn fill_rect(&mut self, rect: &Rect, value: P) -> Rect {
        let r = rect.intersect(&self.bounds());
        let mut dirty = Rect::EMPTY;
        let tiles: Vec<usize> = self.tiles_in(&r).collect();
        for t in tiles {
            let tr = self.tile_rect(t).intersect(&r);
            let whole = tr == self.tile_rect(t);
            if whole && value == self.fill {
                if self.tiles[t].take().is_some() {
                    dirty = dirty.union(&tr);
                }
                continue;
            }
            if self.tiles[t].is_none() && value == self.fill {
                continue;
            }
            let base_x = tr.x - tr.x % TILE_SIZE;
            let base_y = tr.y - tr.y % TILE_SIZE;
            let offset = |x: i32, y: i32| ((y - base_y) * TILE_SIZE + (x - base_x)) as usize;
            if let Some(tile) = &self.tiles[t]
                && tr.points().all(|p| tile[offset(p.x, p.y)] == value)
            {
                continue;
            }
            let tile = self.tile_mut(t);
            for p in tr.points() {
                tile[offset(p.x, p.y)] = value;
            }
            dirty = dirty.union(&tr);
        }
        dirty
    }

    /// Drops tiles whose every pixel equals the fill value.
    pub fn compact(&mut self) {
        let fill = self.fill;
        for slot in &mut self.tiles {
            if slot.as_ref().is_some_and(|t| t.iter().all(|p| *p == fill)) {
                *slot = None;
            }
        }
    }

    /// The bounding box of pixels that differ from the fill value.
    pub fn content_bounds(&self) -> Rect {
        let mut r = Rect::EMPTY;
        for (i, slot) in self.tiles.iter().enumerate() {
            let Some(tile) = slot else { continue };
            let tr = self.tile_rect(i);
            for p in tr.points() {
                let o = ((p.y % TILE_SIZE) * TILE_SIZE + p.x % TILE_SIZE) as usize;
                if tile[o] != self.fill {
                    r.include(p.x, p.y);
                }
            }
        }
        r
    }

    /// Bytes held by present tiles. Shared tiles count in full.
    pub fn memory_bytes(&self) -> usize {
        self.tiles.iter().flatten().count() * std::mem::size_of::<Tile<P>>()
    }

    /// The same pixels converted pixel by pixel into another format.
    pub fn map<Q: Pixel>(&self, fill: Q, mut f: impl FnMut(P) -> Q) -> TiledImage<Q> {
        let mut out = TiledImage::new(self.width, self.height, fill);
        for (i, slot) in self.tiles.iter().enumerate() {
            if let Some(tile) = slot {
                let mut t = [fill; TILE_AREA];
                for (dst, src) in t.iter_mut().zip(tile.iter()) {
                    *dst = f(*src);
                }
                out.tiles[i] = Some(Arc::new(t));
            }
        }
        out
    }
}

impl<P: Pixel> PartialEq for TiledImage<P> {
    /// Images are equal when they have the same size and the same pixels,
    /// however their tiles are stored.
    fn eq(&self, other: &Self) -> bool {
        if self.width != other.width || self.height != other.height {
            return false;
        }
        (0..self.tiles.len()).all(|i| {
            if self.same_tile(other, i) && self.fill == other.fill {
                return true;
            }
            self.tile_rect(i)
                .points()
                .all(|p| self.get(p.x, p.y) == other.get(p.x, p.y))
        })
    }
}

/// One tile of a [`Raster`], held by pointer. History keeps these.
#[derive(Clone, Debug)]
pub enum TileSnap {
    Rgba(Option<Arc<Tile<Rgba>>>),
    Byte(Option<Arc<Tile<u8>>>),
}

impl TileSnap {
    /// Bytes the tile holds, 0 when absent.
    pub fn bytes(&self) -> usize {
        match self {
            TileSnap::Rgba(t) => t.as_ref().map_or(0, |_| std::mem::size_of::<Tile<Rgba>>()),
            TileSnap::Byte(t) => t.as_ref().map_or(0, |_| std::mem::size_of::<Tile<u8>>()),
        }
    }
}

/// The pixels of a cel, a reference image or a layer mask.
#[derive(Clone, Debug, PartialEq)]
pub enum Raster {
    /// Color pixels.
    Rgba(TiledImage<Rgba>),
    /// Palette indices.
    Indexed(TiledImage<u8>),
    /// Mask coverage: 0 hides, 255 shows.
    Mask(TiledImage<u8>),
}

impl Raster {
    /// An empty raster in a document color mode; `transparent_index` fills an
    /// indexed raster.
    pub fn empty(mode: ColorMode, width: u32, height: u32, transparent_index: u8) -> Self {
        match mode {
            ColorMode::Rgba => Raster::Rgba(TiledImage::new(width, height, Rgba::TRANSPARENT)),
            ColorMode::Indexed => {
                Raster::Indexed(TiledImage::new(width, height, transparent_index))
            }
        }
    }

    /// A mask that shows everything.
    pub fn full_mask(width: u32, height: u32) -> Self {
        Raster::Mask(TiledImage::new(width, height, 255))
    }

    pub fn width(&self) -> u32 {
        match self {
            Raster::Rgba(i) => i.width(),
            Raster::Indexed(i) | Raster::Mask(i) => i.width(),
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Raster::Rgba(i) => i.height(),
            Raster::Indexed(i) | Raster::Mask(i) => i.height(),
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width() as i32, self.height() as i32)
    }

    /// The pixel as a raw value: packed RGBA ([`Rgba::to_u32`]) or the index
    /// or coverage byte.
    pub fn get_raw(&self, x: i32, y: i32) -> u32 {
        match self {
            Raster::Rgba(i) => i.get(x, y).to_u32(),
            Raster::Indexed(i) | Raster::Mask(i) => i.get(x, y) as u32,
        }
    }

    /// Writes a raw value (see [`Raster::get_raw`]); returns whether it changed.
    pub fn set_raw(&mut self, x: i32, y: i32, value: u32) -> bool {
        match self {
            Raster::Rgba(i) => i.set(x, y, Rgba::from_u32(value)),
            Raster::Indexed(i) | Raster::Mask(i) => i.set(x, y, value as u8),
        }
    }

    /// The raw value of empty pixels.
    pub fn fill_raw(&self) -> u32 {
        match self {
            Raster::Rgba(i) => i.fill().to_u32(),
            Raster::Indexed(i) | Raster::Mask(i) => i.fill() as u32,
        }
    }

    pub fn tile_count(&self) -> usize {
        match self {
            Raster::Rgba(i) => i.tile_count(),
            Raster::Indexed(i) | Raster::Mask(i) => i.tile_count(),
        }
    }

    pub fn tile_rect(&self, index: usize) -> Rect {
        match self {
            Raster::Rgba(i) => i.tile_rect(index),
            Raster::Indexed(i) | Raster::Mask(i) => i.tile_rect(index),
        }
    }

    pub fn memory_bytes(&self) -> usize {
        match self {
            Raster::Rgba(i) => i.memory_bytes(),
            Raster::Indexed(i) | Raster::Mask(i) => i.memory_bytes(),
        }
    }

    /// Whether tile `index` is shared with `other` (see [`TiledImage::same_tile`]).
    /// Rasters of different kinds share nothing.
    pub fn same_tile(&self, other: &Raster, index: usize) -> bool {
        match (self, other) {
            (Raster::Rgba(a), Raster::Rgba(b)) => a.same_tile(b, index),
            (Raster::Indexed(a), Raster::Indexed(b)) | (Raster::Mask(a), Raster::Mask(b)) => {
                a.same_tile(b, index)
            }
            _ => false,
        }
    }

    /// A pointer copy of tile `index`.
    pub fn tile_snap(&self, index: usize) -> TileSnap {
        match self {
            Raster::Rgba(i) => TileSnap::Rgba(i.tile(index).cloned()),
            Raster::Indexed(i) | Raster::Mask(i) => TileSnap::Byte(i.tile(index).cloned()),
        }
    }

    /// Puts back a tile taken with [`Raster::tile_snap`]. Panics if the kinds
    /// differ.
    pub fn restore_tile(&mut self, index: usize, snap: &TileSnap) {
        match (self, snap) {
            (Raster::Rgba(i), TileSnap::Rgba(t)) => i.set_tile(index, t.clone()),
            (Raster::Indexed(i) | Raster::Mask(i), TileSnap::Byte(t)) => {
                i.set_tile(index, t.clone())
            }
            _ => panic!("restore_tile with a tile of another kind"),
        }
    }

    /// Whether both rasters have the same kind and size.
    pub fn same_shape(&self, other: &Raster) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
            && self.width() == other.width()
            && self.height() == other.height()
    }

    /// Sets every pixel of `rect` to a raw value; returns the changed area.
    pub fn fill_rect_raw(&mut self, rect: &Rect, value: u32) -> Rect {
        match self {
            Raster::Rgba(i) => i.fill_rect(rect, Rgba::from_u32(value)),
            Raster::Indexed(i) | Raster::Mask(i) => i.fill_rect(rect, value as u8),
        }
    }

    pub fn content_bounds(&self) -> Rect {
        match self {
            Raster::Rgba(i) => i.content_bounds(),
            Raster::Indexed(i) | Raster::Mask(i) => i.content_bounds(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_tiles_read_as_fill_and_cost_nothing() {
        let img = TiledImage::new(4096, 4096, Rgba::TRANSPARENT);
        assert_eq!(img.tile_count(), 64 * 64);
        assert_eq!(img.get(100, 100), Rgba::TRANSPARENT);
        assert_eq!(img.memory_bytes(), 0);
    }

    #[test]
    fn writes_allocate_one_tile_and_report_change() {
        let mut img = TiledImage::new(130, 70, 0u8);
        assert_eq!(img.tile_grid(), (3, 2));
        assert!(img.set(65, 3, 7));
        assert!(!img.set(65, 3, 7), "same value is not a change");
        assert!(
            !img.set(0, 0, 0),
            "fill value into an absent tile is not a change"
        );
        assert!(!img.set(130, 0, 1), "outside the image");
        assert_eq!(img.get(65, 3), 7);
        assert_eq!(img.memory_bytes(), TILE_AREA);
        assert_eq!(img.tile_rect(5), Rect::new(128, 64, 2, 6));
    }

    #[test]
    fn clone_shares_tiles_until_written() {
        let mut a = TiledImage::new(128, 128, 0u8);
        a.set(1, 1, 1);
        a.set(100, 100, 1);
        let mut b = a.clone();
        assert!((0..4).all(|i| a.same_tile(&b, i)));
        b.set(2, 2, 9);
        assert!(!a.same_tile(&b, 0), "written tile is copied");
        assert!(a.same_tile(&b, 3), "untouched tile stays shared");
        assert_eq!(a.get(2, 2), 0);
        assert_eq!(b.get(1, 1), 1);
    }

    #[test]
    fn fill_rect_reports_only_changed_area() {
        let mut img = TiledImage::new(200, 200, 0u8);
        assert_eq!(img.fill_rect(&Rect::new(10, 10, 100, 20), 0), Rect::EMPTY);
        assert_eq!(
            img.fill_rect(&Rect::new(-5, 10, 100, 20), 3),
            Rect::new(0, 10, 95, 20)
        );
        assert_eq!(img.get(94, 29), 3);
        assert_eq!(img.get(95, 29), 0);
        let whole = img.fill_rect(&img.bounds(), 0);
        assert_eq!(
            whole,
            Rect::new(0, 0, 128, 64),
            "dropped tiles report their area"
        );
        assert_eq!(img.memory_bytes(), 0, "clearing whole tiles drops them");
    }

    #[test]
    fn equality_ignores_storage() {
        let mut a = TiledImage::new(70, 70, 0u8);
        let b = a.clone();
        a.set(3, 3, 1);
        a.set(3, 3, 0);
        assert_eq!(a, b);
        a.compact();
        assert_eq!(a.memory_bytes(), 0);
    }

    #[test]
    fn content_bounds_spans_tiles() {
        let mut img = TiledImage::new(300, 300, Rgba::TRANSPARENT);
        img.set(10, 250, Rgba::BLACK);
        img.set(200, 5, Rgba::BLACK);
        assert_eq!(img.content_bounds(), Rect::new(10, 5, 191, 246));
    }

    #[test]
    fn raster_raw_round_trip() {
        let mut r = Raster::empty(ColorMode::Rgba, 8, 8, 0);
        let red = Rgba::opaque(255, 0, 0).to_u32();
        assert!(r.set_raw(1, 1, red));
        assert_eq!(r.get_raw(1, 1), red);
        let mut i = Raster::empty(ColorMode::Indexed, 8, 8, 3);
        assert_eq!(i.get_raw(0, 0), 3);
        i.set_raw(0, 0, 5);
        assert_eq!(i.get_raw(0, 0), 5);
    }
}
