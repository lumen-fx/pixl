//! Tilesets and tilemap layers, on the model of Aseprite 1.3.
//!
//! A tileset is a numbered list of tiles of one size; tile 0 is always the
//! empty tile. A tilemap layer is a raster layer that names a tileset: each
//! of its cels has a [`TileGrid`] of [`TileRef`]s, one per cell, each a tile
//! index with flip flags. Tilesets live in the document and any number of
//! tilemap layers share one.
//!
//! A tilemap cel's image holds its grid drawn out, so compositing, the view,
//! the exports and every tool read a tilemap cel like any other cel. The grid
//! and the tileset are what the file saves; the image is made from them.
//!
//! A pixel edit to a tilemap cel (any tool, filter or paste) lands in the
//! tileset when its undo step closes ([`crate::History::commit`]), the way
//! the history's [`TileMode`] says:
//!
//! - [`TileMode::Manual`] edits the tiles under the edit, so every place the
//!   tile shows changes with it; pixels drawn on empty cells are dropped.
//! - [`TileMode::Auto`] reuses a tile that already looks like the edited
//!   cell, edits a tile in place when that cell is its only use, makes a new
//!   tile otherwise, and removes the tiles the edit left unused.
//! - [`TileMode::Stack`] reuses a matching tile or makes a new one, and never
//!   changes or removes a tile.
//!
//! The tile tools edit grids directly: [`Document::place_tiles`],
//! [`Document::fill_tiles`] and [`Document::fill_tile_rect`].
//!
//! Flips follow Aseprite: a cell shows pixel `(u, v)` of its tile after
//! mirroring `u` for an X flip and `v` for a Y flip, then swapping the two
//! for a diagonal flip. A diagonal flip of a tile that is not square shows
//! only the part that fits.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use crate::color::{ColorMode, Rgba};
use crate::document::{Document, ImageId, LayerId, LayerKind};
use crate::error::Error;
use crate::geom::{Point, Rect};
use crate::image::{Pixel, Raster, TILE_AREA, TILE_SIZE, Tile, TiledImage};

/// The largest tile width or height.
pub const MAX_TILE_SIZE: u32 = 1024;
/// The most tiles one tileset holds, the empty tile included.
pub const MAX_TILES: usize = TileRef::INDEX_MASK as usize + 1;

/// Identifies a tileset for the life of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TilesetId(pub u32);

/// How a cell shows its tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileFlags {
    /// Mirrored left to right.
    pub x: bool,
    /// Mirrored top to bottom.
    pub y: bool,
    /// Mirrored across the diagonal from the top-left (x and y swapped).
    pub d: bool,
}

impl TileFlags {
    pub const NONE: TileFlags = TileFlags {
        x: false,
        y: false,
        d: false,
    };

    /// Every combination, no flip first.
    pub const ALL: [TileFlags; 8] = {
        let mut out = [TileFlags::NONE; 8];
        let mut i = 0;
        while i < 8 {
            out[i] = TileFlags {
                x: i & 1 != 0,
                y: i & 2 != 0,
                d: i & 4 != 0,
            };
            i += 1;
        }
        out
    };

    /// The flags as the bits of a [`TileRef`].
    pub const fn bits(self) -> u32 {
        (if self.x { TileRef::FLIP_X } else { 0 })
            | (if self.y { TileRef::FLIP_Y } else { 0 })
            | (if self.d { TileRef::FLIP_D } else { 0 })
    }

    pub const fn from_bits(bits: u32) -> Self {
        TileFlags {
            x: bits & TileRef::FLIP_X != 0,
            y: bits & TileRef::FLIP_Y != 0,
            d: bits & TileRef::FLIP_D != 0,
        }
    }
}

/// One cell of a tilemap: a tile index and its flip flags, packed in 32 bits
/// as Aseprite packs them, so `.pixl` and `.aseprite` files store the same
/// value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileRef(pub u32);

impl TileRef {
    /// The empty cell.
    pub const EMPTY: TileRef = TileRef(0);
    pub const INDEX_MASK: u32 = 0x1fff_ffff;
    pub const FLIP_X: u32 = 0x8000_0000;
    pub const FLIP_Y: u32 = 0x4000_0000;
    pub const FLIP_D: u32 = 0x2000_0000;

    pub const fn new(index: u32, flags: TileFlags) -> Self {
        TileRef((index & Self::INDEX_MASK) | flags.bits())
    }

    pub const fn index(self) -> u32 {
        self.0 & Self::INDEX_MASK
    }

    pub const fn flags(self) -> TileFlags {
        TileFlags::from_bits(self.0)
    }

    pub const fn is_empty(self) -> bool {
        self.index() == 0
    }

    /// The same flags on another tile.
    pub const fn with_index(self, index: u32) -> Self {
        TileRef::new(index, self.flags())
    }
}

impl Pixel for TileRef {}

/// How pixel edits to a tilemap cel change its tileset (see the module
/// documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TileMode {
    Manual,
    #[default]
    Auto,
    Stack,
}

/// A list of tiles of one size. Tile 0 is the empty tile and always there.
#[derive(Clone, Debug, PartialEq)]
pub struct Tileset {
    pub id: TilesetId,
    pub name: String,
    /// The number the UI shows for tile 1 (Aseprite's base index); tile `i`
    /// shows as `i - 1 + base_index`. It never changes what is stored.
    pub base_index: i32,
    /// The flips under which Auto and Stack reuse an existing tile instead
    /// of making a new one.
    pub match_flips: TileFlags,
    size: (u32, u32),
    /// Raw pixel values ([`Raster::get_raw`]) in the document's color mode,
    /// row-major; tile 0 holds the empty value.
    tiles: Vec<Arc<[u32]>>,
}

impl Tileset {
    fn new(id: TilesetId, name: String, size: (u32, u32), empty: u32) -> Self {
        let mut ts = Tileset {
            id,
            name,
            base_index: 1,
            match_flips: TileFlags::NONE,
            size,
            tiles: Vec::new(),
        };
        ts.tiles.push(ts.blank(empty));
        ts
    }

    fn blank(&self, empty: u32) -> Arc<[u32]> {
        vec![empty; self.area()].into()
    }

    fn area(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize
    }

    /// Tile width and height in pixels.
    pub fn tile_size(&self) -> (u32, u32) {
        self.size
    }

    /// The number of tiles, the empty tile included.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Always false: the empty tile is always there.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// A tile's raw pixels, row-major.
    pub fn tile(&self, index: u32) -> Option<&[u32]> {
        self.tiles.get(index as usize).map(|t| &t[..])
    }

    /// The tiles' pixels, tile 0 first.
    pub fn tiles(&self) -> impl Iterator<Item = &[u32]> {
        self.tiles.iter().map(|t| &t[..])
    }

    /// The number the UI shows for tile `index` (0 stays 0, the empty tile).
    pub fn display_index(&self, index: u32) -> i64 {
        if index == 0 {
            0
        } else {
            index as i64 - 1 + self.base_index as i64
        }
    }

    /// A tileset read from a file: its tiles after the empty one.
    pub fn from_parts(
        id: TilesetId,
        name: String,
        size: (u32, u32),
        empty: u32,
        tiles: Vec<Vec<u32>>,
    ) -> Result<Self, Error> {
        check_tile_size(size)?;
        let mut ts = Tileset::new(id, name, size, empty);
        if tiles.len() + 1 > MAX_TILES {
            return Err(Error::TooManyTiles);
        }
        for t in tiles {
            if t.len() != ts.area() {
                return Err(Error::InvalidTileSize {
                    width: size.0,
                    height: size.1,
                });
            }
            ts.tiles.push(t.into());
        }
        Ok(ts)
    }

    /// The raw value the cell's display pixel `(u, v)` shows, or `None`
    /// where a diagonal flip of a tile that is not square leaves no pixel.
    fn sample(&self, tile: &[u32], u: u32, v: u32, flags: TileFlags) -> Option<u32> {
        orient(u, v, self.size, flags).map(|(sx, sy)| tile[(sy * self.size.0 + sx) as usize])
    }

    /// What a cell showing `tile` with `flags` looks like, row-major.
    pub fn oriented(&self, index: u32, flags: TileFlags, empty: u32) -> Vec<u32> {
        let (w, h) = self.size;
        let tile = self.tile(index).unwrap_or(&[]);
        let mut out = Vec::with_capacity(self.area());
        for v in 0..h {
            for u in 0..w {
                out.push(if index == 0 || tile.is_empty() {
                    empty
                } else {
                    self.sample(tile, u, v, flags).unwrap_or(empty)
                });
            }
        }
        out
    }
}

fn check_tile_size((w, h): (u32, u32)) -> Result<(), Error> {
    if w == 0 || h == 0 || w > MAX_TILE_SIZE || h > MAX_TILE_SIZE {
        Err(Error::InvalidTileSize {
            width: w,
            height: h,
        })
    } else {
        Ok(())
    }
}

/// The tile pixel a cell's display pixel `(u, v)` shows.
fn orient(u: u32, v: u32, (w, h): (u32, u32), f: TileFlags) -> Option<(u32, u32)> {
    let sx = if f.x { w - 1 - u } else { u };
    let sy = if f.y { h - 1 - v } else { v };
    if f.d {
        (sy < w && sx < h).then_some((sy, sx))
    } else {
        Some((sx, sy))
    }
}

/// Whether every display pixel of a cell maps to its own tile pixel under
/// `flags`: always, except a diagonal flip of a tile that is not square.
fn bijective((w, h): (u32, u32), f: TileFlags) -> bool {
    !f.d || w == h
}

/// The tile a cell must hold to look like `content` under `flags`, starting
/// from `base` where the flags leave tile pixels unseen.
fn unorient(content: &[u32], size: (u32, u32), f: TileFlags, base: &[u32]) -> Vec<u32> {
    let mut out = base.to_vec();
    for v in 0..size.1 {
        for u in 0..size.0 {
            if let Some((sx, sy)) = orient(u, v, size, f) {
                out[(sy * size.0 + sx) as usize] = content[(v * size.0 + u) as usize];
            }
        }
    }
    out
}

/// The cells of one tilemap cel: a grid of [`TileRef`]s whose cell (0, 0)
/// has its top-left pixel at `origin` on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct TileGrid {
    pub origin: Point,
    cells: TiledImage<TileRef>,
}

impl TileGrid {
    /// An empty grid of `cols` by `rows` cells.
    pub fn new(origin: Point, cols: u32, rows: u32) -> Self {
        TileGrid {
            origin,
            cells: TiledImage::new(cols.max(1), rows.max(1), TileRef::EMPTY),
        }
    }

    /// The empty grid that covers a canvas, its origin moved to the top-left
    /// cell that touches the canvas.
    pub fn covering(canvas: (u32, u32), tile: (u32, u32), origin: Point) -> Self {
        let (ox, oy) = (
            normalize(origin.x, tile.0 as i32),
            normalize(origin.y, tile.1 as i32),
        );
        let cols = (canvas.0 as i32 - ox).max(1) as u32;
        let rows = (canvas.1 as i32 - oy).max(1) as u32;
        TileGrid::new(
            Point::new(ox, oy),
            cols.div_ceil(tile.0),
            rows.div_ceil(tile.1),
        )
    }

    pub fn cols(&self) -> u32 {
        self.cells.width()
    }

    pub fn rows(&self) -> u32 {
        self.cells.height()
    }

    /// The cell at column `x`, row `y`; outside the grid reads as empty.
    pub fn get(&self, x: i32, y: i32) -> TileRef {
        self.cells.get(x, y)
    }

    /// Sets a cell; returns whether it changed.
    pub fn set(&mut self, x: i32, y: i32, tile: TileRef) -> bool {
        self.cells.set(x, y, tile)
    }

    /// Every cell, row-major.
    pub fn to_cells(&self) -> Vec<TileRef> {
        self.cells.to_pixels()
    }

    /// A grid from row-major cells. Fails when the count does not match.
    pub fn from_cells(origin: Point, cols: u32, rows: u32, cells: &[TileRef]) -> Option<Self> {
        (cols > 0 && rows > 0 && cells.len() == cols as usize * rows as usize).then(|| TileGrid {
            origin,
            cells: TiledImage::from_pixels(cols, rows, TileRef::EMPTY, cells),
        })
    }

    /// The cell holding canvas pixel `p`, or `None` outside the grid.
    pub fn cell_at(&self, tile: (u32, u32), p: Point) -> Option<Point> {
        let gx = (p.x - self.origin.x).div_euclid(tile.0 as i32);
        let gy = (p.y - self.origin.y).div_euclid(tile.1 as i32);
        (gx >= 0 && gy >= 0 && gx < self.cols() as i32 && gy < self.rows() as i32)
            .then_some(Point::new(gx, gy))
    }

    /// The canvas pixels of a cell.
    pub fn cell_rect(&self, tile: (u32, u32), cell: Point) -> Rect {
        let (tw, th) = (tile.0 as i32, tile.1 as i32);
        Rect::new(
            self.origin.x + cell.x * tw,
            self.origin.y + cell.y * th,
            tw,
            th,
        )
    }

    /// The cells that overlap a canvas rectangle, clipped to the grid.
    pub fn cells_in(&self, tile: (u32, u32), r: Rect) -> Rect {
        if r.is_empty() {
            return Rect::EMPTY;
        }
        let (tw, th) = (tile.0 as i32, tile.1 as i32);
        let x0 = (r.x - self.origin.x).div_euclid(tw);
        let y0 = (r.y - self.origin.y).div_euclid(th);
        let x1 = (r.right() - 1 - self.origin.x).div_euclid(tw);
        let y1 = (r.bottom() - 1 - self.origin.y).div_euclid(th);
        Rect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1).intersect(&Rect::new(
            0,
            0,
            self.cols() as i32,
            self.rows() as i32,
        ))
    }

    /// The canvas area the whole grid covers.
    pub fn bounds(&self, tile: (u32, u32)) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            (self.cols() * tile.0) as i32,
            (self.rows() * tile.1) as i32,
        )
    }

    fn uses(&self, mut f: impl FnMut(TileRef)) {
        for y in 0..self.rows() as i32 {
            for x in 0..self.cols() as i32 {
                let t = self.get(x, y);
                if !t.is_empty() {
                    f(t);
                }
            }
        }
    }

    /// Rewrites every cell's tile index; cells mapped to 0 become empty.
    fn remap(&mut self, f: impl Fn(u32) -> u32) {
        for y in 0..self.rows() as i32 {
            for x in 0..self.cols() as i32 {
                let t = self.get(x, y);
                if !t.is_empty() {
                    let i = f(t.index());
                    self.set(
                        x,
                        y,
                        if i == 0 {
                            TileRef::EMPTY
                        } else {
                            t.with_index(i)
                        },
                    );
                }
            }
        }
    }
}

/// `v` moved by whole steps into `(-step, 0]`.
fn normalize(v: i32, step: i32) -> i32 {
    let m = v.rem_euclid(step);
    if m == 0 { 0 } else { m - step }
}

/// The tiles of a tileset by content, to find the one an edited cell
/// matches.
struct Finder {
    by_hash: HashMap<u64, Vec<u32>>,
}

fn hash_pixels(px: &[u32]) -> u64 {
    let mut h = DefaultHasher::new();
    px.hash(&mut h);
    h.finish()
}

impl Finder {
    fn new(ts: &Tileset) -> Self {
        let mut f = Finder {
            by_hash: HashMap::new(),
        };
        for (i, t) in ts.tiles.iter().enumerate().skip(1) {
            f.add(i as u32, t);
        }
        f
    }

    fn add(&mut self, index: u32, px: &[u32]) {
        self.by_hash.entry(hash_pixels(px)).or_default().push(index);
    }

    fn remove(&mut self, index: u32, px: &[u32]) {
        if let Some(v) = self.by_hash.get_mut(&hash_pixels(px)) {
            v.retain(|&i| i != index);
        }
    }

    /// A tile and flags that show exactly `content`, trying no flip first.
    fn find(&self, ts: &Tileset, content: &[u32], allow: TileFlags) -> Option<TileRef> {
        for f in TileFlags::ALL {
            if (f.x && !allow.x) || (f.y && !allow.y) || (f.d && !allow.d) {
                continue;
            }
            if !bijective(ts.size, f) {
                continue;
            }
            let t = if f == TileFlags::NONE {
                content.to_vec()
            } else {
                unorient(content, ts.size, f, content)
            };
            if let Some(list) = self.by_hash.get(&hash_pixels(&t))
                && let Some(&i) = list.iter().find(|&&i| ts.tiles[i as usize][..] == t[..])
            {
                return Some(TileRef::new(i, f));
            }
        }
        None
    }
}

/// Writes `f(x, y)` over `area` of an image, keeping the tiles whose pixels
/// do not change. Returns the area that changed.
fn paint<P: Pixel>(img: &mut TiledImage<P>, area: Rect, f: impl Fn(i32, i32) -> P) -> Rect {
    let area = area.intersect(&img.bounds());
    let fill = img.fill();
    let mut dirty = Rect::EMPTY;
    let indices: Vec<usize> = img.tiles_in(&area).collect();
    for t in indices {
        let tr = img.tile_rect(t);
        let r = tr.intersect(&area);
        let mut buf: Box<Tile<P>> = match img.tile(t) {
            Some(a) => Box::new(**a),
            None => Box::new([fill; TILE_AREA]),
        };
        let mut changed = false;
        for y in r.y..r.bottom() {
            let row = ((y - tr.y) * TILE_SIZE) as usize;
            for x in r.x..r.right() {
                let o = row + (x - tr.x) as usize;
                let v = f(x, y);
                if buf[o] != v {
                    buf[o] = v;
                    changed = true;
                }
            }
        }
        if !changed {
            continue;
        }
        dirty = dirty.union(&r);
        if buf.iter().all(|p| *p == fill) {
            img.set_tile(t, None);
        } else {
            img.set_tile(t, Some(Arc::from(buf)));
        }
    }
    dirty
}

/// The raw value a grid shows at canvas pixel `(x, y)`.
fn sample(grid: &TileGrid, ts: &Tileset, empty: u32, x: i32, y: i32) -> u32 {
    let (tw, th) = (ts.size.0 as i32, ts.size.1 as i32);
    let (gx, gy) = (x - grid.origin.x, y - grid.origin.y);
    if gx < 0 || gy < 0 {
        return empty;
    }
    let r = grid.get(gx / tw, gy / th);
    if r.is_empty() {
        return empty;
    }
    let Some(tile) = ts.tiles.get(r.index() as usize) else {
        return empty;
    };
    ts.sample(tile, (gx % tw) as u32, (gy % th) as u32, r.flags())
        .unwrap_or(empty)
}

/// Draws a grid's cells over `area` of a cel image; returns what changed.
fn render(raster: &mut Raster, grid: &TileGrid, ts: &Tileset, area: Rect) -> Rect {
    let empty = raster.fill_raw();
    match raster {
        Raster::Rgba(img) => paint(img, area, |x, y| {
            Rgba::from_u32(sample(grid, ts, empty, x, y))
        }),
        Raster::Indexed(img) => paint(img, area, |x, y| sample(grid, ts, empty, x, y) as u8),
        Raster::Mask(_) => Rect::EMPTY,
    }
}

/// What a cel image shows in a cell, display orientation; pixels past the
/// canvas come from what the grid shows there.
fn cell_content(raster: &Raster, grid: &TileGrid, ts: &Tileset, cell: Point) -> Vec<u32> {
    let r = grid.cell_rect(ts.size, cell);
    let bounds = raster.bounds();
    let empty = raster.fill_raw();
    let mut out = Vec::with_capacity(ts.area());
    for p in r.points() {
        out.push(if bounds.contains(p.x, p.y) {
            raster.get_raw(p.x, p.y)
        } else {
            sample(grid, ts, empty, p.x, p.y)
        });
    }
    out
}

fn cell_shown(grid: &TileGrid, ts: &Tileset, empty: u32, cell: Point) -> Vec<u32> {
    let t = grid.get(cell.x, cell.y);
    ts.oriented(t.index(), t.flags(), empty)
}

/// Whether a cell's pixels are all transparent.
fn blank(px: &[u32], mode: ColorMode, empty: u32) -> bool {
    match mode {
        ColorMode::Rgba => px.iter().all(|&p| Rgba::from_u32(p).a == 0),
        ColorMode::Indexed => px.iter().all(|&p| p == empty),
    }
}

fn find_set(sets: &[Tileset], id: TilesetId) -> Option<&Tileset> {
    sets.iter().find(|t| t.id == id)
}

/// A cell an edit changed: its cel image, the cell and what it shows now.
struct Edited {
    image: ImageId,
    cell: Point,
    /// What the cell shows after the edit.
    content: Vec<u32>,
    /// What its tile showed there before.
    shown: Vec<u32>,
}

impl Document {
    // ---- tilesets ----

    pub fn tilesets(&self) -> &[Tileset] {
        &self.s.tilesets
    }

    pub fn tileset(&self, id: TilesetId) -> Option<&Tileset> {
        self.s.tilesets.iter().find(|t| t.id == id)
    }

    fn tileset_mut(&mut self, id: TilesetId) -> Result<&mut Tileset, Error> {
        self.s
            .tilesets
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Error::TilesetNotFound(id))
    }

    /// The raw value of an empty pixel in this document's color mode.
    pub(crate) fn empty_raw(&self) -> u32 {
        match self.s.mode {
            ColorMode::Rgba => Rgba::TRANSPARENT.to_u32(),
            ColorMode::Indexed => self.s.transparent_index as u32,
        }
    }

    /// Adds an empty tileset (only the empty tile) of tiles `width` by
    /// `height`, and returns its id.
    pub fn add_tileset(
        &mut self,
        name: impl Into<String>,
        width: u32,
        height: u32,
    ) -> Result<TilesetId, Error> {
        check_tile_size((width, height))?;
        let id = TilesetId(self.s.next_tileset);
        self.s.next_tileset += 1;
        let empty = self.empty_raw();
        self.s
            .tilesets
            .push(Tileset::new(id, name.into(), (width, height), empty));
        Ok(id)
    }

    /// Puts a tileset read from a file into the document, keeping its id.
    pub fn insert_tileset(&mut self, ts: Tileset) -> Result<(), Error> {
        if self.tileset(ts.id).is_some() {
            return Err(Error::Format(format!("tileset {} appears twice", ts.id.0)));
        }
        self.s.next_tileset = self.s.next_tileset.max(ts.id.0 + 1);
        self.s.tilesets.push(ts);
        Ok(())
    }

    /// Renames a tileset, sets its base index and the flips Auto and Stack
    /// match. Changes no pixels.
    pub fn update_tileset(
        &mut self,
        id: TilesetId,
        edit: impl FnOnce(&mut String, &mut i32, &mut TileFlags),
    ) -> Result<(), Error> {
        let ts = self.tileset_mut(id)?;
        edit(&mut ts.name, &mut ts.base_index, &mut ts.match_flips);
        Ok(())
    }

    /// Removes a tileset no tilemap layer uses.
    pub fn remove_tileset(&mut self, id: TilesetId) -> Result<(), Error> {
        self.tileset(id).ok_or(Error::TilesetNotFound(id))?;
        if !self.tilemap_layers(id).is_empty() {
            return Err(Error::TilesetInUse(id));
        }
        self.s.tilesets.retain(|t| t.id != id);
        Ok(())
    }

    /// The tilemap layers that use a tileset, in paint order.
    pub fn tilemap_layers(&self, id: TilesetId) -> Vec<LayerId> {
        self.walk()
            .into_iter()
            .filter(|(_, l)| l.tileset() == Some(id))
            .map(|(_, l)| l.id)
            .collect()
    }

    /// Which tileset each tilemap cel image draws from.
    fn tilemap_images(&self) -> BTreeMap<ImageId, TilesetId> {
        let mut out = BTreeMap::new();
        for (_, l) in self.walk() {
            if let LayerKind::Raster {
                cels,
                tileset: Some(ts),
                ..
            } = &l.kind
            {
                for c in cels.iter().flatten() {
                    out.insert(c.image, *ts);
                }
            }
        }
        out
    }

    /// The grids of a tileset's cels, by image.
    fn grids_of(&self, id: TilesetId) -> Vec<ImageId> {
        self.tilemap_images()
            .into_iter()
            .filter(|(_, ts)| *ts == id)
            .map(|(img, _)| img)
            .filter(|img| self.s.tilemaps.contains_key(img))
            .collect()
    }

    /// How many cells show each tile of a tileset (linked cels count once).
    pub fn tile_usage(&self, id: TilesetId) -> Result<Vec<usize>, Error> {
        let ts = self.tileset(id).ok_or(Error::TilesetNotFound(id))?;
        let mut use_count = vec![0; ts.len()];
        for img in self.grids_of(id) {
            self.s.tilemaps[&img].uses(|t| {
                if let Some(n) = use_count.get_mut(t.index() as usize) {
                    *n += 1;
                }
            });
        }
        Ok(use_count)
    }

    // ---- tilemap cels ----

    /// The tileset of a tilemap layer.
    pub fn layer_tileset(&self, layer: LayerId) -> Result<TilesetId, Error> {
        self.layer(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .tileset()
            .ok_or(Error::NotTilemap(layer))
    }

    /// The grid of a tilemap layer's cel, or `None` for an empty cel.
    pub fn tile_grid(&self, layer: LayerId, frame: usize) -> Result<Option<&TileGrid>, Error> {
        self.layer_tileset(layer)?;
        Ok(self
            .cel(layer, frame)?
            .and_then(|c| self.s.tilemaps.get(&c.image)))
    }

    /// The grid a new cel of a tilemap layer starts with: empty, aligned
    /// with the layer's other cels.
    pub(crate) fn new_grid(&self, layer: LayerId, ts: TilesetId) -> TileGrid {
        let origin = match &self.layer(layer).map(|l| &l.kind) {
            Some(LayerKind::Raster { cels, .. }) => cels
                .iter()
                .flatten()
                .find_map(|c| self.s.tilemaps.get(&c.image))
                .map_or(Point::new(0, 0), |g| g.origin),
            _ => Point::new(0, 0),
        };
        let size = self.tileset(ts).map_or((16, 16), |t| t.size);
        TileGrid::covering((self.s.width, self.s.height), size, origin)
    }

    /// The cell under canvas pixel `p` on a tilemap layer, with what it
    /// holds; `None` when the cel is empty or `p` is off the grid.
    pub fn tile_at(
        &self,
        layer: LayerId,
        frame: usize,
        p: Point,
    ) -> Result<Option<(Point, TileRef)>, Error> {
        let ts = self.layer_tileset(layer)?;
        let size = self.tileset(ts).ok_or(Error::TilesetNotFound(ts))?.size;
        Ok(self
            .tile_grid(layer, frame)?
            .and_then(|g| g.cell_at(size, p).map(|cell| (cell, g.get(cell.x, cell.y)))))
    }

    /// The cell of a tilemap layer under canvas pixel `p` on `frame`: on
    /// the cel's grid, or for an empty cel on the grid a new cel would get.
    /// `None` off the grid.
    pub fn tile_cell(
        &self,
        layer: LayerId,
        frame: usize,
        p: Point,
    ) -> Result<Option<Point>, Error> {
        let ts = self.layer_tileset(layer)?;
        let size = self.tileset(ts).ok_or(Error::TilesetNotFound(ts))?.size;
        Ok(match self.tile_grid(layer, frame)? {
            Some(g) => g.cell_at(size, p),
            None => self.new_grid(layer, ts).cell_at(size, p),
        })
    }

    /// The grid of a tilemap cel to edit, its cel made if empty. Refused
    /// when the layer's pixels are locked or `tile` is not in the tileset.
    fn edit_grid(
        &mut self,
        layer: LayerId,
        frame: usize,
        tile: TileRef,
    ) -> Result<(ImageId, TilesetId), Error> {
        let ts = self.layer_tileset(layer)?;
        let len = self.tileset(ts).ok_or(Error::TilesetNotFound(ts))?.len();
        if tile.index() as usize >= len {
            return Err(Error::TileNotFound(tile.index()));
        }
        let image = self.target_image(layer, frame)?;
        Ok((image, ts))
    }

    /// Draws a cel's cells over `cells` (in cell coordinates) again.
    fn redraw_cells(&mut self, image: ImageId, ts: TilesetId, cells: Rect) -> Rect {
        let (Some(grid), Some(set)) = (self.s.tilemaps.get(&image), find_set(&self.s.tilesets, ts))
        else {
            return Rect::EMPTY;
        };
        let area = Rect::new(
            grid.origin.x + cells.x * set.size.0 as i32,
            grid.origin.y + cells.y * set.size.1 as i32,
            cells.w * set.size.0 as i32,
            cells.h * set.size.1 as i32,
        );
        let raster = self.images.get_mut(&image).expect("tilemap cel image");
        render(raster, grid, set, area)
    }

    /// Gives a tilemap cel a whole grid, making the cel if it is empty. The
    /// grid may sit anywhere; cells wholly off the canvas are dropped.
    /// Refused when a cell names a tile the tileset lacks.
    pub fn set_tile_grid(
        &mut self,
        layer: LayerId,
        frame: usize,
        grid: TileGrid,
    ) -> Result<Rect, Error> {
        let ts = self.layer_tileset(layer)?;
        let len = self.tileset(ts).ok_or(Error::TilesetNotFound(ts))?.len() as u32;
        if let Some(t) = grid.to_cells().iter().find(|t| t.index() >= len) {
            return Err(Error::TileNotFound(t.index()));
        }
        let image = self.target_image(layer, frame)?;
        self.s.tilemaps.insert(image, grid);
        self.move_one(image, ts, Point::new(0, 0));
        Ok(self.bounds())
    }

    /// Sets cells of a tilemap cel to a tile, making the cel if it is
    /// empty. Cells off the grid are left out. Returns the canvas area that
    /// changed.
    pub fn place_tiles(
        &mut self,
        layer: LayerId,
        frame: usize,
        cells: &[Point],
        tile: TileRef,
    ) -> Result<Rect, Error> {
        let (image, ts) = self.edit_grid(layer, frame, tile)?;
        let mut box_ = Rect::EMPTY;
        let grid = self.s.tilemaps.get_mut(&image).expect("tilemap cel grid");
        for c in cells {
            if c.x >= 0
                && c.y >= 0
                && (c.x as u32) < grid.cols()
                && (c.y as u32) < grid.rows()
                && grid.set(c.x, c.y, tile)
            {
                box_ = box_.union(&Rect::pixel(c.x, c.y));
            }
        }
        Ok(self.redraw_cells(image, ts, box_))
    }

    /// Sets every cell of `cells` (a rectangle of cells) to a tile.
    pub fn fill_tile_rect(
        &mut self,
        layer: LayerId,
        frame: usize,
        cells: Rect,
        tile: TileRef,
    ) -> Result<Rect, Error> {
        let list: Vec<Point> = cells.points().collect();
        self.place_tiles(layer, frame, &list, tile)
    }

    /// Fills with a tile from cell `at`: the cells joined to it edge to edge
    /// that hold the same tile and flips, or every such cell when not
    /// `contiguous`.
    pub fn fill_tiles(
        &mut self,
        layer: LayerId,
        frame: usize,
        at: Point,
        tile: TileRef,
        contiguous: bool,
    ) -> Result<Rect, Error> {
        let (image, _) = self.edit_grid(layer, frame, tile)?;
        let grid = &self.s.tilemaps[&image];
        let (cols, rows) = (grid.cols() as i32, grid.rows() as i32);
        if at.x < 0 || at.y < 0 || at.x >= cols || at.y >= rows {
            return Ok(Rect::EMPTY);
        }
        let from = grid.get(at.x, at.y);
        if from == tile {
            return Ok(Rect::EMPTY);
        }
        let mut cells = Vec::new();
        if contiguous {
            let mut seen = vec![false; (cols * rows) as usize];
            let mut stack = vec![at];
            seen[(at.y * cols + at.x) as usize] = true;
            while let Some(p) = stack.pop() {
                cells.push(p);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let q = Point::new(p.x + dx, p.y + dy);
                    if q.x < 0 || q.y < 0 || q.x >= cols || q.y >= rows {
                        continue;
                    }
                    let k = (q.y * cols + q.x) as usize;
                    if !seen[k] && grid.get(q.x, q.y) == from {
                        seen[k] = true;
                        stack.push(q);
                    }
                }
            }
        } else {
            for y in 0..rows {
                for x in 0..cols {
                    if grid.get(x, y) == from {
                        cells.push(Point::new(x, y));
                    }
                }
            }
        }
        self.place_tiles(layer, frame, &cells, tile)
    }

    // ---- tiles ----

    /// Adds a tile at the end of a tileset and returns its index. `pixels`
    /// are raw values in the document's color mode, row-major.
    pub fn add_tile(&mut self, id: TilesetId, pixels: Vec<u32>) -> Result<u32, Error> {
        let ts = self.tileset_mut(id)?;
        if pixels.len() != ts.area() {
            return Err(Error::InvalidTileSize {
                width: ts.size.0,
                height: ts.size.1,
            });
        }
        if ts.tiles.len() >= MAX_TILES {
            return Err(Error::TooManyTiles);
        }
        ts.tiles.push(pixels.into());
        Ok(ts.tiles.len() as u32 - 1)
    }

    /// Replaces a tile's pixels; every cell showing it changes, on every
    /// frame and layer. Returns the canvas area that changed.
    pub fn set_tile_pixels(
        &mut self,
        id: TilesetId,
        index: u32,
        pixels: Vec<u32>,
    ) -> Result<Rect, Error> {
        let ts = self.tileset_mut(id)?;
        if index == 0 || index as usize >= ts.tiles.len() {
            return Err(Error::TileNotFound(index));
        }
        if pixels.len() != ts.area() {
            return Err(Error::InvalidTileSize {
                width: ts.size.0,
                height: ts.size.1,
            });
        }
        ts.tiles[index as usize] = pixels.into();
        Ok(
            self.redraw_tiles(id, &BTreeSet::from([index]), &mut BTreeMap::new(), &|_| {
                true
            }),
        )
    }

    /// Copies a tile just after itself, moving the later tiles up one, and
    /// returns the copy's index. Cells keep showing the tiles they showed.
    pub fn duplicate_tile(&mut self, id: TilesetId, index: u32) -> Result<u32, Error> {
        let ts = self.tileset_mut(id)?;
        if index == 0 || index as usize >= ts.tiles.len() {
            return Err(Error::TileNotFound(index));
        }
        if ts.tiles.len() >= MAX_TILES {
            return Err(Error::TooManyTiles);
        }
        let copy = ts.tiles[index as usize].clone();
        ts.tiles.insert(index as usize + 1, copy);
        self.remap_tiles(id, |i| if i > index { i + 1 } else { i });
        Ok(index + 1)
    }

    /// Moves tile `from` to position `to` (both past the empty tile). Cells
    /// keep showing the same pixels; only the numbers change.
    pub fn move_tile(&mut self, id: TilesetId, from: u32, to: u32) -> Result<(), Error> {
        let ts = self.tileset_mut(id)?;
        let n = ts.tiles.len() as u32;
        if from == 0 || from >= n {
            return Err(Error::TileNotFound(from));
        }
        if to == 0 || to >= n {
            return Err(Error::TileNotFound(to));
        }
        if from == to {
            return Ok(());
        }
        let t = ts.tiles.remove(from as usize);
        ts.tiles.insert(to as usize, t);
        self.remap_tiles(id, |i| {
            if i == from {
                to
            } else if from < to && i > from && i <= to {
                i - 1
            } else if to < from && i >= to && i < from {
                i + 1
            } else {
                i
            }
        });
        Ok(())
    }

    /// Removes a tile; cells that showed it become empty. Returns the canvas
    /// area that changed.
    pub fn remove_tile(&mut self, id: TilesetId, index: u32) -> Result<Rect, Error> {
        let ts = self.tileset_mut(id)?;
        if index == 0 || index as usize >= ts.tiles.len() {
            return Err(Error::TileNotFound(index));
        }
        let used = self.tile_usage(id)?[index as usize] > 0;
        self.tileset_mut(id)?.tiles.remove(index as usize);
        self.remap_tiles(id, |i| match i.cmp(&index) {
            std::cmp::Ordering::Less => i,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => i - 1,
        });
        if used {
            for img in self.grids_of(id) {
                self.redraw_whole(img, id);
            }
            Ok(self.bounds())
        } else {
            Ok(Rect::EMPTY)
        }
    }

    /// Removes every tile no cell shows; returns how many went.
    pub fn remove_unused_tiles(&mut self, id: TilesetId) -> Result<usize, Error> {
        let usage = self.tile_usage(id)?;
        let unused: BTreeSet<u32> = (1..usage.len() as u32)
            .filter(|&i| usage[i as usize] == 0)
            .collect();
        self.drop_tiles(id, &unused)?;
        Ok(unused.len())
    }

    /// Removes tiles no cell shows, renumbering the rest.
    fn drop_tiles(&mut self, id: TilesetId, gone: &BTreeSet<u32>) -> Result<(), Error> {
        if gone.is_empty() {
            return Ok(());
        }
        let ts = self.tileset_mut(id)?;
        let mut i = 0u32;
        ts.tiles.retain(|_| {
            let keep = !gone.contains(&i);
            i += 1;
            keep
        });
        let shift: Vec<u32> = gone.iter().copied().collect();
        self.remap_tiles(id, |i| {
            if gone.contains(&i) {
                0
            } else {
                i - shift.partition_point(|&g| g < i) as u32
            }
        });
        Ok(())
    }

    /// Renumbers the tiles every grid of a tileset points at.
    fn remap_tiles(&mut self, id: TilesetId, f: impl Fn(u32) -> u32) {
        for img in self.grids_of(id) {
            if let Some(g) = self.s.tilemaps.get_mut(&img) {
                g.remap(&f);
            }
        }
    }

    /// Draws again, on every cel of a tileset, the cells showing `tiles`.
    /// Each image `known` does not cover is copied into `before` first, for
    /// the history.
    fn redraw_tiles(
        &mut self,
        id: TilesetId,
        tiles: &BTreeSet<u32>,
        before: &mut BTreeMap<ImageId, Raster>,
        known: &dyn Fn(ImageId) -> bool,
    ) -> Rect {
        let Some(size) = self.tileset(id).map(|t| t.size) else {
            return Rect::EMPTY;
        };
        let mut dirty = Rect::EMPTY;
        for img in self.grids_of(id) {
            let grid = &self.s.tilemaps[&img];
            let mut area = Rect::EMPTY;
            for y in 0..grid.rows() as i32 {
                for x in 0..grid.cols() as i32 {
                    if tiles.contains(&grid.get(x, y).index()) {
                        area = area.union(&grid.cell_rect(size, Point::new(x, y)));
                    }
                }
            }
            if area.is_empty() {
                continue;
            }
            if !known(img) {
                before
                    .entry(img)
                    .or_insert_with(|| self.images[&img].clone());
            }
            let grid = &self.s.tilemaps[&img];
            let set = find_set(&self.s.tilesets, id).expect("tileset");
            let raster = self.images.get_mut(&img).expect("tilemap cel image");
            dirty = dirty.union(&render(raster, grid, set, area));
        }
        dirty
    }

    fn redraw_whole(&mut self, img: ImageId, id: TilesetId) -> Rect {
        let (Some(grid), Some(set)) = (self.s.tilemaps.get(&img), find_set(&self.s.tilesets, id))
        else {
            return Rect::EMPTY;
        };
        let raster = self.images.get_mut(&img).expect("tilemap cel image");
        let area = raster.bounds();
        render(raster, grid, set, area)
    }

    /// Draws every tilemap cel from its grid again.
    pub(crate) fn redraw_tilemaps(&mut self) {
        for (img, ts) in self.tilemap_images() {
            self.redraw_whole(img, ts);
        }
    }

    // ---- converting ----

    /// Builds a grid for a cel image from its pixels, cut into cells from
    /// `origin`: each cell takes a tile that looks the same under one of the
    /// flips `allow` permits, or a new tile. Then draws the cel from it.
    fn retile(&mut self, image: ImageId, id: TilesetId, origin: Point, allow: TileFlags) {
        let empty = self.empty_raw();
        let mode = self.s.mode;
        let canvas = (self.s.width, self.s.height);
        let Some(ts) = self.s.tilesets.iter_mut().find(|t| t.id == id) else {
            return;
        };
        let Some(raster) = self.images.get(&image) else {
            return;
        };
        let size = ts.size;
        let mut grid = TileGrid::covering(canvas, size, origin);
        let mut finder = Finder::new(ts);
        let bounds = raster.bounds();
        for cy in 0..grid.rows() as i32 {
            for cx in 0..grid.cols() as i32 {
                let r = grid.cell_rect(size, Point::new(cx, cy));
                if r.intersect(&bounds).is_empty() {
                    continue;
                }
                let content: Vec<u32> = r
                    .points()
                    .map(|p| {
                        if bounds.contains(p.x, p.y) {
                            raster.get_raw(p.x, p.y)
                        } else {
                            empty
                        }
                    })
                    .collect();
                if blank(&content, mode, empty) {
                    continue;
                }
                let t = match finder.find(ts, &content, allow) {
                    Some(t) => t,
                    None if ts.tiles.len() < MAX_TILES => {
                        let index = ts.tiles.len() as u32;
                        finder.add(index, &content);
                        ts.tiles.push(content.into());
                        TileRef::new(index, TileFlags::NONE)
                    }
                    None => continue,
                };
                grid.set(cx, cy, t);
            }
        }
        self.s.tilemaps.insert(image, grid);
        self.redraw_whole(image, id);
    }

    /// Makes a raster layer a tilemap layer of tileset `id`: each cel is cut
    /// into tiles, reusing the tiles the tileset already has and adding the
    /// rest. Returns the canvas area that changed.
    pub fn convert_to_tilemap(&mut self, layer: LayerId, id: TilesetId) -> Result<Rect, Error> {
        let allow = self
            .tileset(id)
            .ok_or(Error::TilesetNotFound(id))?
            .match_flips;
        if self.effective_locks(layer)?.pixels {
            return Err(Error::Locked(layer));
        }
        let images: Vec<ImageId> = match &mut self
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Raster {
                cels,
                tileset: tileset @ None,
                ..
            } => {
                *tileset = Some(id);
                let mut v: Vec<ImageId> = cels.iter().flatten().map(|c| c.image).collect();
                v.dedup();
                v
            }
            _ => return Err(Error::NotRaster(layer)),
        };
        let mut done = BTreeSet::new();
        for img in images {
            if done.insert(img) {
                self.retile(img, id, Point::new(0, 0), allow);
            }
        }
        Ok(self.bounds())
    }

    /// Makes a tilemap layer a plain raster layer that keeps the pixels its
    /// cels show.
    pub fn convert_to_raster(&mut self, layer: LayerId) -> Result<Rect, Error> {
        match &mut self
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Raster {
                tileset: tileset @ Some(_),
                ..
            } => *tileset = None,
            _ => return Err(Error::NotTilemap(layer)),
        }
        self.collect_garbage();
        Ok(Rect::EMPTY)
    }

    /// Gives every tilemap cel that has none (a merge or a mask applied made
    /// its pixels) a grid cut from its pixels.
    pub(crate) fn retile_missing(&mut self) {
        let missing: Vec<(ImageId, TilesetId)> = self
            .tilemap_images()
            .into_iter()
            .filter(|(img, _)| !self.s.tilemaps.contains_key(img))
            .collect();
        for (img, ts) in missing {
            let allow = self.tileset(ts).map_or(TileFlags::NONE, |t| t.match_flips);
            self.retile(img, ts, Point::new(0, 0), allow);
        }
    }

    /// After the canvas moved by `offset` and took a new size: the grids
    /// move with it, cover the new canvas, and the cels are drawn again.
    pub(crate) fn move_tilemaps(&mut self, offset: Point) {
        for (img, id) in self.tilemap_images() {
            self.move_one(img, id, offset);
        }
    }

    /// After a canvas flip or quarter turn (`op` maps a dense buffer, `rect`
    /// a canvas rectangle): each grid turns with the canvas and its cells'
    /// flips follow. A quarter turn of tiles that are not square cannot be
    /// shown with flips, so those cels are cut into tiles again.
    pub(crate) fn turn_tilemaps(
        &mut self,
        op: impl Fn(&crate::transform::Pixels) -> crate::transform::Pixels,
        rect: impl Fn(Rect) -> Rect,
    ) {
        use crate::transform::Pixels;
        for (img, id) in self.tilemap_images() {
            let (Some(old), Some(ts)) = (self.s.tilemaps.get(&img), self.tileset(id)) else {
                continue;
            };
            let size = ts.size;
            // What each flip becomes: turn a tile of distinct values shown
            // with it, and find the flip that shows the result.
            let probe = Pixels {
                width: size.0,
                height: size.1,
                data: (1..=size.0 * size.1).collect(),
            };
            let shown = |f: TileFlags| -> Pixels {
                let mut px = Pixels::new(size.0, size.1, 0);
                for v in 0..size.1 {
                    for u in 0..size.0 {
                        if let Some((sx, sy)) = orient(u, v, size, f) {
                            px.data[(v * size.0 + u) as usize] =
                                probe.data[(sy * size.0 + sx) as usize];
                        }
                    }
                }
                px
            };
            let mut table = [None; 8];
            for (k, f) in TileFlags::ALL.into_iter().enumerate() {
                if !bijective(size, f) {
                    continue;
                }
                let turned = op(&shown(f));
                table[k] = TileFlags::ALL
                    .into_iter()
                    .find(|&g| bijective(size, g) && shown(g) == turned);
            }
            let usable = table
                .iter()
                .zip(TileFlags::ALL)
                .all(|(t, f)| t.is_some() || !bijective(size, f));
            if !usable {
                self.s.tilemaps.remove(&img);
                self.retile(
                    img,
                    id,
                    Point::new(0, 0),
                    TileFlags {
                        x: true,
                        y: true,
                        d: true,
                    },
                );
                continue;
            }
            let cells = Pixels {
                width: old.cols(),
                height: old.rows(),
                data: old.to_cells().iter().map(|t| t.0).collect(),
            };
            let turned = op(&cells);
            let to = rect(old.bounds(size));
            let refs: Vec<TileRef> = turned
                .data
                .iter()
                .map(|&v| {
                    let t = TileRef(v);
                    if t.is_empty() {
                        return TileRef::EMPTY;
                    }
                    let k = TileFlags::ALL
                        .iter()
                        .position(|f| *f == t.flags())
                        .expect("eight flips");
                    TileRef::new(t.index(), table[k].unwrap_or(t.flags()))
                })
                .collect();
            let mid =
                TileGrid::from_cells(Point::new(to.x, to.y), turned.width, turned.height, &refs)
                    .expect("sized to the turned cells");
            self.s.tilemaps.insert(img, mid);
            // Trim to the canvas the way a move does, and draw.
            self.move_one(img, id, Point::new(0, 0));
        }
    }

    /// Moves one grid by `offset`, makes it cover the canvas and draws its
    /// cel again.
    fn move_one(&mut self, img: ImageId, id: TilesetId, offset: Point) {
        let canvas = (self.s.width, self.s.height);
        let (Some(old), Some(ts)) = (self.s.tilemaps.get(&img), self.tileset(id)) else {
            return;
        };
        let size = ts.size;
        let moved = Point::new(old.origin.x + offset.x, old.origin.y + offset.y);
        let mut grid = TileGrid::covering(canvas, size, moved);
        let shift = Point::new(
            (moved.x - grid.origin.x) / size.0 as i32,
            (moved.y - grid.origin.y) / size.1 as i32,
        );
        for y in 0..old.rows() as i32 {
            for x in 0..old.cols() as i32 {
                let t = old.get(x, y);
                if !t.is_empty() {
                    grid.set(x + shift.x, y + shift.y, t);
                }
            }
        }
        self.s.tilemaps.insert(img, grid);
        self.redraw_whole(img, id);
    }

    /// After a resize that scales the canvas: every tilemap cel is cut into
    /// tiles again from its scaled pixels.
    pub(crate) fn rescale_tilemaps(&mut self) {
        for (img, id) in self.tilemap_images() {
            self.s.tilemaps.remove(&img);
            self.retile(
                img,
                id,
                Point::new(0, 0),
                TileFlags {
                    x: true,
                    y: true,
                    d: true,
                },
            );
        }
    }

    /// Converts every tile to the color mode the document is changing to.
    pub(crate) fn convert_tiles(&mut self, to: ColorMode) {
        let palette = self.s.palette.clone();
        let ti = self.s.transparent_index;
        let from = self.s.mode;
        let empty = match to {
            ColorMode::Rgba => Rgba::TRANSPARENT.to_u32(),
            ColorMode::Indexed => ti as u32,
        };
        for ts in &mut self.s.tilesets {
            for (i, t) in ts.tiles.iter_mut().enumerate() {
                let px: Vec<u32> = if i == 0 {
                    vec![empty; t.len()]
                } else {
                    t.iter()
                        .map(|&v| match (from, to) {
                            (ColorMode::Rgba, ColorMode::Indexed) => {
                                let c = Rgba::from_u32(v);
                                if c.a == 0 {
                                    ti as u32
                                } else {
                                    palette.nearest(Rgba::new(c.r, c.g, c.b, 255), Some(ti)) as u32
                                }
                            }
                            (ColorMode::Indexed, ColorMode::Rgba) => {
                                if v as u8 == ti {
                                    Rgba::TRANSPARENT.to_u32()
                                } else {
                                    palette.get(v as u8).to_u32()
                                }
                            }
                            _ => v,
                        })
                        .collect()
                };
                *t = px.into();
            }
        }
    }

    // ---- pixel edits landing in tilesets ----

    /// Takes the pixel edits made to tilemap cels since a transaction began
    /// into their tilesets, the way `mode` says, and draws the cels again
    /// from their grids. `before` holds the watched images as they were and
    /// `existed` (sorted) the ids of every image then; images made since
    /// count as edited from empty. Returns, for the history, how the other
    /// cels this redrew looked before.
    pub(crate) fn sync_tilemaps(
        &mut self,
        before: &BTreeMap<ImageId, Raster>,
        existed: &[ImageId],
        mode: TileMode,
    ) -> BTreeMap<ImageId, Raster> {
        let mut redrawn = BTreeMap::new();
        if self.s.tilesets.is_empty() {
            return redrawn;
        }
        self.retile_missing();
        let owners = self.tilemap_images();
        let empty = self.empty_raw();
        let is_new = |img: &ImageId| existed.binary_search(img).is_err();
        let mut edits: BTreeMap<TilesetId, Vec<Edited>> = BTreeMap::new();
        for (&img, &id) in &owners {
            let now = &self.images[&img];
            let area = if is_new(&img) {
                now.bounds()
            } else if let Some(old) = before.get(&img) {
                if old.same_shape(now) {
                    (0..now.tile_count())
                        .filter(|&t| !old.same_tile(now, t))
                        .fold(Rect::EMPTY, |r, t| r.union(&now.tile_rect(t)))
                } else {
                    now.bounds()
                }
            } else {
                continue;
            };
            if !area.is_empty() {
                self.collect_edits(img, id, area, empty, &mut edits);
            }
        }
        let known = |img: ImageId| before.contains_key(&img) || is_new(&img);
        for (id, list) in edits {
            let mut changed = BTreeSet::new();
            match mode {
                TileMode::Manual => self.land_manual(id, &list, &mut changed),
                TileMode::Auto | TileMode::Stack => {
                    self.land_auto(id, &list, mode == TileMode::Auto, &mut changed, empty)
                }
            }
            for e in &list {
                self.redraw_cells(e.image, id, Rect::new(e.cell.x, e.cell.y, 1, 1));
            }
            if !changed.is_empty() {
                self.redraw_tiles(id, &changed, &mut redrawn, &known);
            }
        }
        redrawn
    }

    fn collect_edits(
        &self,
        img: ImageId,
        id: TilesetId,
        area: Rect,
        empty: u32,
        out: &mut BTreeMap<TilesetId, Vec<Edited>>,
    ) {
        let (Some(grid), Some(ts)) = (self.s.tilemaps.get(&img), self.tileset(id)) else {
            return;
        };
        let raster = &self.images[&img];
        let cells = grid.cells_in(ts.size, area.intersect(&raster.bounds()));
        for cell in cells.points() {
            let content = cell_content(raster, grid, ts, cell);
            let shown = cell_shown(grid, ts, empty, cell);
            if content != shown {
                out.entry(id).or_default().push(Edited {
                    image: img,
                    cell,
                    content,
                    shown,
                });
            }
        }
    }

    /// Manual: each pixel the edit changed goes into the tile under it, so
    /// edits to two places showing one tile both land.
    fn land_manual(&mut self, id: TilesetId, list: &[Edited], changed: &mut BTreeSet<u32>) {
        for e in list {
            let t = self.s.tilemaps[&e.image].get(e.cell.x, e.cell.y);
            if t.is_empty() {
                continue;
            }
            let ts = self.tileset_mut(id).expect("tileset");
            let size = ts.size;
            let Some(tile) = ts.tiles.get(t.index() as usize) else {
                continue;
            };
            let mut px = tile.to_vec();
            for v in 0..size.1 {
                for u in 0..size.0 {
                    let k = (v * size.0 + u) as usize;
                    if e.content[k] != e.shown[k]
                        && let Some((sx, sy)) = orient(u, v, size, t.flags())
                    {
                        px[(sy * size.0 + sx) as usize] = e.content[k];
                    }
                }
            }
            ts.tiles[t.index() as usize] = px.into();
            changed.insert(t.index());
        }
    }

    /// Auto and Stack: reuse a matching tile, else (Auto only) edit a tile
    /// shown nowhere else, else add a tile. Auto then removes the tiles the
    /// edit left unused.
    fn land_auto(
        &mut self,
        id: TilesetId,
        list: &[Edited],
        auto: bool,
        changed: &mut BTreeSet<u32>,
        empty: u32,
    ) {
        let mode = self.s.mode;
        let Ok(mut usage) = self.tile_usage(id) else {
            return;
        };
        let ts = self.tileset(id).expect("tileset");
        let allow = ts.match_flips;
        let mut finder = Finder::new(ts);
        let mut left = BTreeSet::new();
        for e in list {
            let old = self.s.tilemaps[&e.image].get(e.cell.x, e.cell.y);
            let ts = self.tileset(id).expect("tileset");
            let next = if blank(&e.content, mode, empty) {
                TileRef::EMPTY
            } else if let Some(t) = finder.find(ts, &e.content, allow) {
                t
            } else if auto && !old.is_empty() && usage[old.index() as usize] == 1 {
                let i = old.index();
                let base = ts.tiles[i as usize].to_vec();
                let px = unorient(&e.content, ts.size, old.flags(), &base);
                finder.remove(i, &base);
                finder.add(i, &px);
                self.tileset_mut(id).expect("tileset").tiles[i as usize] = px.into();
                changed.insert(i);
                old
            } else if ts.tiles.len() < MAX_TILES {
                let i = ts.tiles.len() as u32;
                finder.add(i, &e.content);
                self.tileset_mut(id)
                    .expect("tileset")
                    .tiles
                    .push(e.content.clone().into());
                usage.push(0);
                TileRef::new(i, TileFlags::NONE)
            } else {
                old
            };
            if next != old {
                if !old.is_empty() {
                    usage[old.index() as usize] -= 1;
                    left.insert(old.index());
                }
                if !next.is_empty() {
                    usage[next.index() as usize] += 1;
                }
                self.s
                    .tilemaps
                    .get_mut(&e.image)
                    .expect("grid")
                    .set(e.cell.x, e.cell.y, next);
            }
        }
        if auto {
            let gone: BTreeSet<u32> = left
                .into_iter()
                .filter(|&i| usage[i as usize] == 0)
                .collect();
            for i in &gone {
                changed.remove(i);
            }
            // Renumbering moves which index a redraw looks at; the pixels the
            // cells show stay the same, so only edited tiles need drawing,
            // under their new numbers.
            let renum = |i: u32| i - gone.iter().filter(|&&g| g < i).count() as u32;
            let still: BTreeSet<u32> = changed.iter().map(|&i| renum(i)).collect();
            let _ = self.drop_tiles(id, &gone);
            *changed = still;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{NewLayer, Placement};
    use crate::history::{History, Scope};

    const RED: Rgba = Rgba::opaque(255, 0, 0);

    fn doc_with_map() -> (Document, LayerId, TilesetId) {
        let mut d = Document::new(64, 48, ColorMode::Rgba).unwrap();
        let ts = d.add_tileset("tiles", 16, 16).unwrap();
        let l = d
            .add_layer(
                NewLayer::Tilemap { tileset: ts },
                "map",
                Placement::default(),
            )
            .unwrap();
        (d, l, ts)
    }

    fn solid(c: Rgba) -> Vec<u32> {
        vec![c.to_u32(); 256]
    }

    fn px(d: &Document, l: LayerId, x: i32, y: i32) -> Rgba {
        Rgba::from_u32(d.cel_raster(l, 0).unwrap().map_or(0, |r| r.get_raw(x, y)))
    }

    fn paint(h: &mut History, d: &mut Document, l: LayerId, x: i32, y: i32, c: Rgba) {
        let image = d.target_image(l, 0).unwrap();
        h.transact(d, "pencil", Scope::Images(vec![image]), |d| {
            d.target(l, 0).unwrap().raster.set_raw(x, y, c.to_u32());
        });
    }

    #[test]
    fn tile_refs_pack_like_aseprite() {
        let t = TileRef::new(
            5,
            TileFlags {
                x: true,
                y: false,
                d: true,
            },
        );
        assert_eq!(t.0, 5 | 0x8000_0000 | 0x2000_0000);
        assert_eq!(t.index(), 5);
        assert_eq!(
            t.flags(),
            TileFlags {
                x: true,
                y: false,
                d: true
            }
        );
        assert!(TileRef::EMPTY.is_empty());
    }

    #[test]
    fn placed_tiles_draw_with_flips() {
        let (mut d, l, ts) = doc_with_map();
        // A tile with one red pixel at its top-right.
        let mut t = vec![0; 256];
        t[15] = RED.to_u32();
        let i = d.add_tile(ts, t).unwrap();
        assert_eq!(i, 1);
        let dirty = d
            .place_tiles(l, 0, &[Point::new(1, 0)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        assert_eq!(dirty, Rect::new(16, 0, 16, 16), "the placed cell");
        assert_eq!(px(&d, l, 31, 0), RED);
        let fx = TileRef::new(
            i,
            TileFlags {
                x: true,
                ..TileFlags::NONE
            },
        );
        d.place_tiles(l, 0, &[Point::new(0, 1)], fx).unwrap();
        assert_eq!(px(&d, l, 0, 16), RED, "x flip mirrors the column");
        let fd = TileRef::new(
            i,
            TileFlags {
                d: true,
                ..TileFlags::NONE
            },
        );
        d.place_tiles(l, 0, &[Point::new(2, 2)], fd).unwrap();
        assert_eq!(px(&d, l, 32, 32 + 15), RED, "a diagonal flip swaps x and y");
        assert_eq!(
            d.tile_at(l, 0, Point::new(33, 40)).unwrap(),
            Some((Point::new(2, 2), fd))
        );
        assert_eq!(
            d.place_tiles(l, 0, &[Point::new(0, 0)], TileRef::new(9, TileFlags::NONE)),
            Err(Error::TileNotFound(9))
        );
    }

    #[test]
    fn manual_edits_change_every_instance() {
        let (mut d, l, ts) = doc_with_map();
        let i = d.add_tile(ts, solid(Rgba::BLACK)).unwrap();
        d.fill_tile_rect(
            l,
            0,
            Rect::new(0, 0, 4, 1),
            TileRef::new(i, TileFlags::NONE),
        )
        .unwrap();
        let mut h = History::default();
        h.set_tile_mode(TileMode::Manual);
        paint(&mut h, &mut d, l, 1, 1, RED);
        for cell in 0..4 {
            assert_eq!(px(&d, l, cell * 16 + 1, 1), RED, "cell {cell}");
        }
        assert_eq!(d.tileset(ts).unwrap().len(), 2, "no tile was made");
        // Pixels on an empty cell are dropped in manual mode.
        paint(&mut h, &mut d, l, 1, 20, RED);
        assert_eq!(px(&d, l, 1, 20), Rgba::TRANSPARENT);
        h.undo(&mut d);
        h.undo(&mut d);
        assert_eq!(
            px(&d, l, 49, 1),
            Rgba::BLACK,
            "undo restores every instance"
        );
        h.redo(&mut d);
        assert_eq!(px(&d, l, 49, 1), RED);
    }

    #[test]
    fn manual_edits_reach_other_layers_and_undo() {
        let (mut d, l, ts) = doc_with_map();
        let other = d
            .add_layer(
                NewLayer::Tilemap { tileset: ts },
                "other",
                Placement::default(),
            )
            .unwrap();
        let i = d.add_tile(ts, solid(Rgba::BLACK)).unwrap();
        d.place_tiles(l, 0, &[Point::new(0, 0)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        d.place_tiles(
            other,
            0,
            &[Point::new(2, 2)],
            TileRef::new(i, TileFlags::NONE),
        )
        .unwrap();
        let mut h = History::default();
        h.set_tile_mode(TileMode::Manual);
        let before = d.clone();
        paint(&mut h, &mut d, l, 3, 3, RED);
        assert_eq!(px(&d, other, 35, 35), RED);
        h.undo(&mut d);
        assert_eq!(d, before);
    }

    #[test]
    fn auto_mode_makes_reuses_and_removes_tiles() {
        let (mut d, l, ts) = doc_with_map();
        let mut h = History::default();
        assert_eq!(h.tile_mode(), TileMode::Auto);
        paint(&mut h, &mut d, l, 2, 2, RED);
        let set = d.tileset(ts).unwrap();
        assert_eq!(set.len(), 2, "drawing on an empty cell made a tile");
        assert_eq!(
            d.tile_at(l, 0, Point::new(2, 2))
                .unwrap()
                .unwrap()
                .1
                .index(),
            1
        );
        // The same pixel in another cell reuses that tile.
        paint(&mut h, &mut d, l, 18, 2, RED);
        assert_eq!(d.tileset(ts).unwrap().len(), 2);
        assert_eq!(
            d.tile_at(l, 0, Point::new(18, 2))
                .unwrap()
                .unwrap()
                .1
                .index(),
            1
        );
        // Changing one of two uses makes a new tile.
        paint(&mut h, &mut d, l, 19, 3, RED);
        assert_eq!(d.tileset(ts).unwrap().len(), 3);
        assert_eq!(
            px(&d, l, 3, 3),
            Rgba::TRANSPARENT,
            "the other use is unchanged"
        );
        // Changing the only use of a tile edits it in place.
        paint(&mut h, &mut d, l, 20, 4, RED);
        assert_eq!(d.tileset(ts).unwrap().len(), 3);
        // Erasing a cell empties it and removes the tile it left unused.
        let img = d.target_image(l, 0).unwrap();
        h.transact(&mut d, "erase", Scope::Images(vec![img]), |d| {
            let t = d.target(l, 0).unwrap();
            t.raster.fill_rect_raw(&Rect::new(16, 0, 16, 16), 0);
        });
        assert_eq!(
            d.tile_at(l, 0, Point::new(18, 2)).unwrap().unwrap().1,
            TileRef::EMPTY
        );
        assert_eq!(d.tileset(ts).unwrap().len(), 2);
        h.undo(&mut d);
        assert_eq!(d.tileset(ts).unwrap().len(), 3);
        assert_eq!(px(&d, l, 20, 4), RED);
    }

    #[test]
    fn stack_mode_never_changes_tiles() {
        let (mut d, l, ts) = doc_with_map();
        let i = d.add_tile(ts, solid(Rgba::BLACK)).unwrap();
        d.place_tiles(l, 0, &[Point::new(0, 0)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        let mut h = History::default();
        h.set_tile_mode(TileMode::Stack);
        paint(&mut h, &mut d, l, 1, 1, RED);
        let set = d.tileset(ts).unwrap();
        assert_eq!(set.len(), 3);
        assert_eq!(set.tile(1).unwrap(), &solid(Rgba::BLACK)[..]);
    }

    #[test]
    fn flipped_matches_when_asked() {
        let (mut d, l, ts) = doc_with_map();
        let mut t = vec![0; 256];
        t[0] = RED.to_u32();
        d.add_tile(ts, t).unwrap();
        d.update_tileset(ts, |_, _, m| m.x = true).unwrap();
        let mut h = History::default();
        paint(&mut h, &mut d, l, 15, 0, RED);
        assert_eq!(d.tileset(ts).unwrap().len(), 2, "matched tile 1 flipped");
        let r = d.tile_at(l, 0, Point::new(15, 0)).unwrap().unwrap().1;
        assert_eq!(
            r,
            TileRef::new(
                1,
                TileFlags {
                    x: true,
                    ..TileFlags::NONE
                }
            )
        );
    }

    #[test]
    fn convert_and_back() {
        let mut d = Document::new(32, 32, ColorMode::Rgba).unwrap();
        let l = d.layers()[0].id;
        {
            let t = d.target(l, 0).unwrap();
            t.raster
                .fill_rect_raw(&Rect::new(0, 0, 32, 16), RED.to_u32());
        }
        let before = d.cel_raster(l, 0).unwrap().unwrap().clone();
        let ts = d.add_tileset("t", 16, 16).unwrap();
        d.convert_to_tilemap(l, ts).unwrap();
        assert_eq!(
            d.tileset(ts).unwrap().len(),
            2,
            "two red cells share a tile"
        );
        assert_eq!(d.tile_usage(ts).unwrap(), vec![0, 2]);
        assert_eq!(d.cel_raster(l, 0).unwrap().unwrap(), &before);
        d.convert_to_raster(l).unwrap();
        assert!(d.tile_grid(l, 0).is_err());
        assert_eq!(d.cel_raster(l, 0).unwrap().unwrap(), &before);
        assert!(d.s.tilemaps.is_empty());
    }

    #[test]
    fn tileset_reorder_duplicate_and_prune() {
        let (mut d, l, ts) = doc_with_map();
        let a = d.add_tile(ts, solid(Rgba::BLACK)).unwrap();
        let b = d.add_tile(ts, solid(RED)).unwrap();
        d.place_tiles(l, 0, &[Point::new(0, 0)], TileRef::new(a, TileFlags::NONE))
            .unwrap();
        d.place_tiles(l, 0, &[Point::new(1, 0)], TileRef::new(b, TileFlags::NONE))
            .unwrap();
        let shown = d.cel_raster(l, 0).unwrap().unwrap().clone();
        d.move_tile(ts, 2, 1).unwrap();
        assert_eq!(
            d.tile_at(l, 0, Point::new(0, 0))
                .unwrap()
                .unwrap()
                .1
                .index(),
            2
        );
        assert_eq!(d.cel_raster(l, 0).unwrap().unwrap(), &shown);
        let c = d.duplicate_tile(ts, 1).unwrap();
        assert_eq!(c, 2);
        assert_eq!(
            d.tile_at(l, 0, Point::new(0, 0))
                .unwrap()
                .unwrap()
                .1
                .index(),
            3
        );
        assert_eq!(d.remove_unused_tiles(ts).unwrap(), 1);
        assert_eq!(d.tileset(ts).unwrap().len(), 3);
        assert_eq!(d.cel_raster(l, 0).unwrap().unwrap(), &shown);
        assert_eq!(d.move_tile(ts, 0, 1), Err(Error::TileNotFound(0)));
    }

    #[test]
    fn bucket_fills_cells() {
        let (mut d, l, ts) = doc_with_map();
        let a = d.add_tile(ts, solid(Rgba::BLACK)).unwrap();
        let t = TileRef::new(a, TileFlags::NONE);
        d.fill_tile_rect(l, 0, Rect::new(1, 0, 1, 3), t).unwrap();
        d.fill_tiles(l, 0, Point::new(0, 0), t, true).unwrap();
        let g = d.tile_grid(l, 0).unwrap().unwrap();
        assert_eq!(g.get(0, 2), t);
        assert_eq!(g.get(3, 2), TileRef::EMPTY, "the wall stops the fill");
        d.fill_tiles(l, 0, Point::new(3, 0), t, false).unwrap();
        assert_eq!(d.tile_grid(l, 0).unwrap().unwrap().get(3, 2), t);
    }

    #[test]
    fn canvas_edits_keep_tilemaps() {
        use crate::transform::{CanvasTransform, crop, transform_canvas};
        let (mut d, l, ts) = doc_with_map();
        let mut t = vec![0; 256];
        t[0] = RED.to_u32();
        let i = d.add_tile(ts, t).unwrap();
        d.place_tiles(l, 0, &[Point::new(0, 0)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        transform_canvas(&mut d, CanvasTransform::FlipH).unwrap();
        assert_eq!(px(&d, l, 63, 0), RED);
        assert_eq!(
            d.tileset(ts).unwrap().len(),
            2,
            "the flip is a flag, not a tile"
        );
        let r = d.tile_at(l, 0, Point::new(63, 0)).unwrap().unwrap().1;
        assert!(r.flags().x);
        transform_canvas(&mut d, CanvasTransform::Rotate90(1)).unwrap();
        assert_eq!((d.width(), d.height()), (48, 64));
        assert_eq!(px(&d, l, 47, 63), RED);
        assert_eq!(d.tileset(ts).unwrap().len(), 2);
        crop(&mut d, Rect::new(8, 8, 40, 56)).unwrap();
        assert_eq!(px(&d, l, 39, 55), RED);
        let g = d.tile_grid(l, 0).unwrap().unwrap();
        assert_eq!(g.origin, Point::new(-8, -8));
        // The rendered cel always matches its grid.
        let mut check = d.cel_raster(l, 0).unwrap().unwrap().clone();
        let set = d.tileset(ts).unwrap();
        assert_eq!(
            render(&mut check, g, set, Rect::new(0, 0, 40, 56)),
            Rect::EMPTY
        );
    }

    #[test]
    fn indexed_tilesets_convert_with_the_document() {
        let (mut d, l, ts) = doc_with_map();
        let red = d.palette().get(4);
        let i = d.add_tile(ts, solid(red)).unwrap();
        d.place_tiles(l, 0, &[Point::new(0, 0)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        d.set_color_mode(ColorMode::Indexed);
        assert_eq!(d.tileset(ts).unwrap().tile(1).unwrap()[0], 4);
        assert_eq!(d.cel_raster(l, 0).unwrap().unwrap().get_raw(3, 3), 4);
        d.set_color_mode(ColorMode::Rgba);
        assert_eq!(px(&d, l, 3, 3), red);
    }

    #[test]
    fn merging_onto_a_tilemap_cuts_the_result_into_tiles() {
        let (mut d, l, ts) = doc_with_map();
        let i = d.add_tile(ts, solid(Rgba::BLACK)).unwrap();
        d.place_tiles(l, 0, &[Point::new(0, 0)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        let top = d
            .add_layer(NewLayer::Raster, "top", Placement::default())
            .unwrap();
        d.target(top, 0)
            .unwrap()
            .raster
            .set_raw(20, 4, RED.to_u32());
        let before = d.clone();
        let mut h = History::default();
        h.transact(&mut d, "Merge down", Scope::All, |d| d.merge_down(top))
            .unwrap();
        assert_eq!(d.layer_tileset(l), Ok(ts));
        assert_eq!(px(&d, l, 20, 4), RED);
        assert_eq!(px(&d, l, 1, 1), Rgba::BLACK);
        assert_eq!(
            d.tile_at(l, 0, Point::new(1, 1))
                .unwrap()
                .unwrap()
                .1
                .index(),
            1
        );
        assert_eq!(d.tileset(ts).unwrap().len(), 3, "the red cell made a tile");
        h.undo(&mut d);
        assert_eq!(d, before);
    }

    #[test]
    fn new_frames_and_copies_keep_grids() {
        let (mut d, l, ts) = doc_with_map();
        let i = d.add_tile(ts, solid(RED)).unwrap();
        d.place_tiles(l, 0, &[Point::new(0, 0)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        let f = d.duplicate_frame(0).unwrap();
        assert!(d.tile_grid(l, f).unwrap().is_some());
        d.place_tiles(l, f, &[Point::new(1, 0)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        assert_eq!(
            d.tile_grid(l, 0).unwrap().unwrap().get(1, 0),
            TileRef::EMPTY
        );
        let copy = d.duplicate_layer(l).unwrap();
        assert_eq!(d.layer_tileset(copy).unwrap(), ts);
        assert!(d.tile_grid(copy, f).unwrap().is_some());
        d.insert_frame(2, 100).unwrap();
        d.place_tiles(l, 2, &[Point::new(3, 2)], TileRef::new(i, TileFlags::NONE))
            .unwrap();
        assert_eq!(px(&d, l, 0, 0), RED);
    }
}
