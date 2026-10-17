//! The document model: layers, frames, cels, tags and slices.
//!
//! A document is a tree of layers crossed with a list of frames. Each raster
//! layer holds one optional cel per frame; a cel points at an image in the
//! document's image pool, and linked cels point at the same image. Every
//! mutation returns the canvas area it changed so a view repaints only that.

use std::collections::BTreeMap;

use crate::color::{ColorMode, Palette, Rgba};
use crate::error::Error;
use crate::geom::{Point, Rect};
use crate::image::{Raster, TiledImage};
use crate::rig::Rig;

/// Identifies a layer for the life of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub u32);

/// Identifies an image in a document's image pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(pub u32);

/// How a layer's colors combine with what is below it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
    Add,
    Subtract,
}

impl BlendMode {
    pub const ALL: [BlendMode; 18] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
        BlendMode::Add,
        BlendMode::Subtract,
    ];

    /// The mode's number in stored files. pixl files use the numbering of
    /// Aseprite files, so both formats share one table.
    pub const fn code(self) -> u16 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Multiply => 1,
            BlendMode::Screen => 2,
            BlendMode::Overlay => 3,
            BlendMode::Darken => 4,
            BlendMode::Lighten => 5,
            BlendMode::ColorDodge => 6,
            BlendMode::ColorBurn => 7,
            BlendMode::HardLight => 8,
            BlendMode::SoftLight => 9,
            BlendMode::Difference => 10,
            BlendMode::Exclusion => 11,
            BlendMode::Hue => 12,
            BlendMode::Saturation => 13,
            BlendMode::Color => 14,
            BlendMode::Luminosity => 15,
            BlendMode::Add => 16,
            BlendMode::Subtract => 17,
        }
    }

    /// The mode with a stored number, or `None` for one pixl does not have.
    pub fn from_code(code: u16) -> Option<BlendMode> {
        BlendMode::ALL.into_iter().find(|m| m.code() == code)
    }
}

/// What a lock forbids. A group's locks apply to everything inside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Locks {
    /// No pixel edits.
    pub pixels: bool,
    /// Edits keep each pixel's alpha (paint only where there is paint).
    pub alpha: bool,
    /// No moving the layer's content.
    pub position: bool,
}

impl Locks {
    pub const NONE: Locks = Locks {
        pixels: false,
        alpha: false,
        position: false,
    };
    pub const ALL: Locks = Locks {
        pixels: true,
        alpha: true,
        position: true,
    };

    fn or(self, o: Locks) -> Locks {
        Locks {
            pixels: self.pixels || o.pixels,
            alpha: self.alpha || o.alpha,
            position: self.position || o.position,
        }
    }
}

/// Pixel shape on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PixelAspect {
    #[default]
    Square,
    /// 2:1, pixels twice as wide as tall.
    Wide,
    /// 1:2, pixels twice as tall as wide.
    Tall,
}

/// What shows through where no layer paints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Background {
    #[default]
    Transparent,
    Color(Rgba),
}

/// The editable properties every layer has.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerProps {
    pub name: String,
    pub visible: bool,
    pub locks: Locks,
    /// 0 is invisible, 255 is fully opaque.
    pub opacity: u8,
    pub blend: BlendMode,
    /// A label color for the layer list.
    pub color_tag: Option<Rgba>,
    /// Clip to the nearest unclipped layer below it in the same group.
    pub clip: bool,
}

impl LayerProps {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visible: true,
            locks: Locks::NONE,
            opacity: 255,
            blend: BlendMode::Normal,
            color_tag: None,
            clip: false,
        }
    }
}

/// One frame's content of a raster layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cel {
    pub image: ImageId,
    pub opacity: u8,
}

/// A raster layer mask: coverage 255 shows the layer, 0 hides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerMask {
    pub image: ImageId,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayerKind {
    /// Paintable pixels, one optional cel per frame.
    Raster {
        cels: Vec<Option<Cel>>,
        mask: Option<LayerMask>,
    },
    /// Children composited in isolation, bottom first, then blended as one.
    Group { children: Vec<Layer> },
    /// An image to trace over; shown in the editor, never exported.
    Reference { image: ImageId },
    /// A rig of bones posing raster layers; draws nothing itself (the
    /// editor shows its bones as an overlay).
    Skeleton { rig: Box<Rig> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub id: LayerId,
    pub props: LayerProps,
    pub kind: LayerKind,
}

impl Layer {
    pub fn is_raster(&self) -> bool {
        matches!(self.kind, LayerKind::Raster { .. })
    }

    pub fn children(&self) -> &[Layer] {
        match &self.kind {
            LayerKind::Group { children } => children,
            _ => &[],
        }
    }
}

/// The kind of layer to create.
#[derive(Clone, Debug)]
pub enum NewLayer {
    Raster,
    Group,
    Reference(TiledImage<Rgba>),
    Skeleton,
}

/// Where a new or moved layer goes: into `parent` (the root when `None`) at
/// `index` among its children, 0 being the bottom. `None` index is the top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub parent: Option<LayerId>,
    pub index: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub duration_ms: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LoopDirection {
    #[default]
    Forward,
    Reverse,
    PingPong,
}

/// A named range of frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub name: String,
    /// First frame, included.
    pub from: usize,
    /// Last frame, included.
    pub to: usize,
    pub direction: LoopDirection,
    /// Times to play; 0 loops forever.
    pub repeat: u32,
    pub color: Rgba,
}

/// A named rectangle exported as metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    pub name: String,
    pub bounds: Rect,
    /// The 9-slice center, relative to `bounds`.
    pub center: Option<Rect>,
    /// The pivot, relative to `bounds`.
    pub pivot: Option<Point>,
    pub color: Rgba,
}

/// Everything about a document except image pixels. Small to clone, which is
/// what history snapshots of structural edits rely on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Structure {
    pub width: u32,
    pub height: u32,
    pub mode: ColorMode,
    pub palette: Palette,
    pub transparent_index: u8,
    pub pixel_aspect: PixelAspect,
    pub background: Background,
    pub layers: Vec<Layer>,
    pub frames: Vec<Frame>,
    pub tags: Vec<Tag>,
    pub slices: Vec<Slice>,
    pub next_layer: u32,
    pub next_image: u32,
}

/// A paintable view of one image, with the context tools need.
pub struct Target<'a> {
    pub raster: &'a mut Raster,
    pub palette: &'a Palette,
    pub transparent_index: u8,
    /// Writes keep each pixel's alpha.
    pub alpha_locked: bool,
}

/// The largest supported width or height.
pub const MAX_SIZE: u32 = 32768;

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub(crate) s: Structure,
    pub(crate) images: BTreeMap<ImageId, Raster>,
}

fn default_palette() -> Palette {
    // Index 0 is the transparent entry of indexed documents.
    let colors = vec![
        Rgba::TRANSPARENT,
        Rgba::BLACK,
        Rgba::WHITE,
        Rgba::opaque(0x9d, 0x9d, 0x9d),
        Rgba::opaque(0xbe, 0x26, 0x33),
        Rgba::opaque(0xe0, 0x6f, 0x8b),
        Rgba::opaque(0x49, 0x3c, 0x2b),
        Rgba::opaque(0xa4, 0x64, 0x22),
        Rgba::opaque(0xeb, 0x89, 0x31),
        Rgba::opaque(0xf7, 0xe2, 0x6b),
        Rgba::opaque(0x2f, 0x48, 0x4e),
        Rgba::opaque(0x44, 0x89, 0x1a),
        Rgba::opaque(0xa3, 0xce, 0x27),
        Rgba::opaque(0x1b, 0x26, 0x32),
        Rgba::opaque(0x00, 0x57, 0x84),
        Rgba::opaque(0x31, 0xa2, 0xf2),
    ];
    Palette::new(colors).expect("16 colors fit")
}

impl Document {
    /// A document with one empty raster layer and one 100 ms frame.
    pub fn new(width: u32, height: u32, mode: ColorMode) -> Result<Self, Error> {
        if width == 0 || height == 0 || width > MAX_SIZE || height > MAX_SIZE {
            return Err(Error::InvalidSize { width, height });
        }
        let mut doc = Document {
            s: Structure {
                width,
                height,
                mode,
                palette: default_palette(),
                transparent_index: 0,
                pixel_aspect: PixelAspect::Square,
                background: Background::Transparent,
                layers: Vec::new(),
                frames: vec![Frame { duration_ms: 100 }],
                tags: Vec::new(),
                slices: Vec::new(),
                next_layer: 1,
                next_image: 1,
            },
            images: BTreeMap::new(),
        };
        doc.add_layer(NewLayer::Raster, "Layer 1", Placement::default())?;
        Ok(doc)
    }

    pub fn width(&self) -> u32 {
        self.s.width
    }

    pub fn height(&self) -> u32 {
        self.s.height
    }

    /// The whole canvas.
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.s.width as i32, self.s.height as i32)
    }

    pub fn mode(&self) -> ColorMode {
        self.s.mode
    }

    pub fn palette(&self) -> &Palette {
        &self.s.palette
    }

    /// The palette entry that is transparent in indexed mode.
    pub fn transparent_index(&self) -> u8 {
        self.s.transparent_index
    }

    pub fn pixel_aspect(&self) -> PixelAspect {
        self.s.pixel_aspect
    }

    pub fn background(&self) -> Background {
        self.s.background
    }

    /// Root layers, bottom first.
    pub fn layers(&self) -> &[Layer] {
        &self.s.layers
    }

    pub fn frames(&self) -> &[Frame] {
        &self.s.frames
    }

    pub fn frame_count(&self) -> usize {
        self.s.frames.len()
    }

    pub fn tags(&self) -> &[Tag] {
        &self.s.tags
    }

    /// Tags carry no pixels, so editing them changes no canvas area.
    pub fn tags_mut(&mut self) -> &mut Vec<Tag> {
        &mut self.s.tags
    }

    pub fn slices(&self) -> &[Slice] {
        &self.s.slices
    }

    /// Slices carry no pixels, so editing them changes no canvas area.
    pub fn slices_mut(&mut self) -> &mut Vec<Slice> {
        &mut self.s.slices
    }

    pub fn set_palette(&mut self, palette: Palette) -> Rect {
        self.s.palette = palette;
        self.palette_dirty()
    }

    pub fn set_palette_color(&mut self, index: u8, color: Rgba) -> Result<Rect, Error> {
        self.s.palette.set(index, color)?;
        Ok(self.palette_dirty())
    }

    pub fn set_transparent_index(&mut self, index: u8) -> Rect {
        self.s.transparent_index = index;
        self.palette_dirty()
    }

    fn palette_dirty(&self) -> Rect {
        match self.s.mode {
            ColorMode::Indexed => self.bounds(),
            ColorMode::Rgba => Rect::EMPTY,
        }
    }

    pub fn set_pixel_aspect(&mut self, aspect: PixelAspect) -> Rect {
        self.s.pixel_aspect = aspect;
        self.bounds()
    }

    pub fn set_background(&mut self, background: Background) -> Rect {
        self.s.background = background;
        self.bounds()
    }

    /// An image from the pool.
    pub fn image(&self, id: ImageId) -> Option<&Raster> {
        self.images.get(&id)
    }

    pub(crate) fn alloc_image(&mut self, raster: Raster) -> ImageId {
        let id = ImageId(self.s.next_image);
        self.s.next_image += 1;
        self.images.insert(id, raster);
        id
    }

    fn empty_raster(&self) -> Raster {
        Raster::empty(
            self.s.mode,
            self.s.width,
            self.s.height,
            self.s.transparent_index,
        )
    }

    // ---- layer tree ----

    /// The layer with `id`, anywhere in the tree.
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        fn find(layers: &[Layer], id: LayerId) -> Option<&Layer> {
            layers.iter().find_map(|l| {
                if l.id == id {
                    Some(l)
                } else {
                    find(l.children(), id)
                }
            })
        }
        find(&self.s.layers, id)
    }

    pub(crate) fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        fn find(layers: &mut [Layer], id: LayerId) -> Option<&mut Layer> {
            for l in layers {
                if l.id == id {
                    return Some(l);
                }
                if let LayerKind::Group { children } = &mut l.kind
                    && let Some(found) = find(children, id)
                {
                    return Some(found);
                }
            }
            None
        }
        find(&mut self.s.layers, id)
    }

    /// The group holding `id`, or `None` for a root layer or an unknown id.
    pub fn parent_of(&self, id: LayerId) -> Option<LayerId> {
        self.ancestors(id).first().copied()
    }

    /// The groups holding `id`, innermost first.
    pub fn ancestors(&self, id: LayerId) -> Vec<LayerId> {
        fn path(layers: &[Layer], id: LayerId, out: &mut Vec<LayerId>) -> bool {
            for l in layers {
                if l.id == id {
                    return true;
                }
                if path(l.children(), id, out) {
                    out.push(l.id);
                    return true;
                }
            }
            false
        }
        let mut out = Vec::new();
        path(&self.s.layers, id, &mut out);
        out
    }

    /// Every layer in paint order (bottom first, a group before its
    /// children), with its depth in the tree.
    pub fn walk(&self) -> Vec<(usize, &Layer)> {
        fn go<'a>(layers: &'a [Layer], depth: usize, out: &mut Vec<(usize, &'a Layer)>) {
            for l in layers {
                out.push((depth, l));
                go(l.children(), depth + 1, out);
            }
        }
        let mut out = Vec::new();
        go(&self.s.layers, 0, &mut out);
        out
    }

    /// The layer's locks combined with those of every group holding it.
    pub fn effective_locks(&self, id: LayerId) -> Result<Locks, Error> {
        let mut locks = self.layer(id).ok_or(Error::LayerNotFound(id))?.props.locks;
        for g in self.ancestors(id) {
            if let Some(l) = self.layer(g) {
                locks = locks.or(l.props.locks);
            }
        }
        Ok(locks)
    }

    pub(crate) fn siblings_mut(
        &mut self,
        parent: Option<LayerId>,
    ) -> Result<&mut Vec<Layer>, Error> {
        match parent {
            None => Ok(&mut self.s.layers),
            Some(p) => match &mut self.layer_mut(p).ok_or(Error::LayerNotFound(p))?.kind {
                LayerKind::Group { children } => Ok(children),
                _ => Err(Error::NotGroup(p)),
            },
        }
    }

    /// Creates a layer and returns its id.
    pub fn add_layer(
        &mut self,
        kind: NewLayer,
        name: impl Into<String>,
        at: Placement,
    ) -> Result<LayerId, Error> {
        if let Some(p) = at.parent {
            let parent = self.layer(p).ok_or(Error::LayerNotFound(p))?;
            if !matches!(parent.kind, LayerKind::Group { .. }) {
                return Err(Error::NotGroup(p));
            }
        }
        let kind = match kind {
            NewLayer::Raster => LayerKind::Raster {
                cels: vec![None; self.s.frames.len()],
                mask: None,
            },
            NewLayer::Group => LayerKind::Group {
                children: Vec::new(),
            },
            NewLayer::Skeleton => LayerKind::Skeleton {
                rig: Box::new(Rig::new(self.s.frames.len())),
            },
            NewLayer::Reference(img) => {
                if img.width() != self.s.width || img.height() != self.s.height {
                    return Err(Error::InvalidSize {
                        width: img.width(),
                        height: img.height(),
                    });
                }
                LayerKind::Reference {
                    image: self.alloc_image(Raster::Rgba(img)),
                }
            }
        };
        let id = LayerId(self.s.next_layer);
        self.s.next_layer += 1;
        let layer = Layer {
            id,
            props: LayerProps::named(name),
            kind,
        };
        let siblings = self.siblings_mut(at.parent)?;
        let index = at.index.unwrap_or(siblings.len()).min(siblings.len());
        siblings.insert(index, layer);
        Ok(id)
    }

    /// Edits a layer's properties. Returns the whole canvas when the edit
    /// changes how the document looks, and an empty area otherwise.
    pub fn update_layer(
        &mut self,
        id: LayerId,
        edit: impl FnOnce(&mut LayerProps),
    ) -> Result<Rect, Error> {
        let bounds = self.bounds();
        let layer = self.layer_mut(id).ok_or(Error::LayerNotFound(id))?;
        let before = layer.props.clone();
        edit(&mut layer.props);
        let p = &layer.props;
        let visual = p.visible != before.visible
            || p.opacity != before.opacity
            || p.blend != before.blend
            || p.clip != before.clip;
        Ok(if visual { bounds } else { Rect::EMPTY })
    }

    fn detach(&mut self, id: LayerId) -> Result<Layer, Error> {
        let parent = self.parent_of(id);
        let siblings = self.siblings_mut(parent)?;
        let pos = siblings
            .iter()
            .position(|l| l.id == id)
            .ok_or(Error::LayerNotFound(id))?;
        Ok(siblings.remove(pos))
    }

    /// Deletes a layer and everything in it.
    pub fn remove_layer(&mut self, id: LayerId) -> Result<Rect, Error> {
        self.detach(id)?;
        self.prune_rigs();
        self.collect_garbage();
        Ok(self.bounds())
    }

    /// Drops rig bindings of layers that are gone.
    pub(crate) fn prune_rigs(&mut self) {
        let present: std::collections::BTreeSet<LayerId> =
            self.walk().into_iter().map(|(_, l)| l.id).collect();
        self.for_each_rig(|rig| rig.retain_layers(|l| present.contains(&l)));
    }

    /// Moves a layer to a new place in the tree.
    pub fn move_layer(&mut self, id: LayerId, to: Placement) -> Result<Rect, Error> {
        self.layer(id).ok_or(Error::LayerNotFound(id))?;
        if let Some(p) = to.parent
            && (p == id || self.ancestors(p).contains(&id))
        {
            return Err(Error::CyclicMove(id));
        }
        if let Some(p) = to.parent {
            let parent = self.layer(p).ok_or(Error::LayerNotFound(p))?;
            if !matches!(parent.kind, LayerKind::Group { .. }) {
                return Err(Error::NotGroup(p));
            }
        }
        let layer = self.detach(id)?;
        let siblings = self.siblings_mut(to.parent)?;
        let index = to.index.unwrap_or(siblings.len()).min(siblings.len());
        siblings.insert(index, layer);
        Ok(self.bounds())
    }

    /// Drops pool images no cel, mask or reference layer points at.
    pub fn collect_garbage(&mut self) {
        let mut used = std::collections::BTreeSet::new();
        for (_, l) in self.walk() {
            match &l.kind {
                LayerKind::Raster { cels, mask } => {
                    used.extend(cels.iter().flatten().map(|c| c.image));
                    used.extend(mask.map(|m| m.image));
                }
                LayerKind::Reference { image } => {
                    used.insert(*image);
                }
                LayerKind::Group { .. } | LayerKind::Skeleton { .. } => {}
            }
        }
        self.images.retain(|id, _| used.contains(id));
    }

    // ---- skeletons ----

    /// The rig of skeleton layer `layer`.
    pub fn rig(&self, layer: LayerId) -> Result<&Rig, Error> {
        match &self.layer(layer).ok_or(Error::LayerNotFound(layer))?.kind {
            LayerKind::Skeleton { rig } => Ok(rig),
            _ => Err(Error::NotSkeleton(layer)),
        }
    }

    /// Edits a rig. Rigs paint nothing, so no canvas area changes; renders
    /// ([`Document::render_pose`]) write the pixels.
    pub fn rig_mut(&mut self, layer: LayerId) -> Result<&mut Rig, Error> {
        match &mut self
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Skeleton { rig } => Ok(rig),
            _ => Err(Error::NotSkeleton(layer)),
        }
    }

    pub(crate) fn rigs_mut(&mut self) -> Vec<&mut Rig> {
        fn go<'a>(layers: &'a mut [Layer], out: &mut Vec<&'a mut Rig>) {
            for l in layers {
                match &mut l.kind {
                    LayerKind::Skeleton { rig } => out.push(rig),
                    LayerKind::Group { children } => go(children, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        go(&mut self.s.layers, &mut out);
        out
    }

    // ---- frames ----

    fn for_each_raster(&mut self, mut f: impl FnMut(&mut Vec<Option<Cel>>)) {
        fn go(layers: &mut [Layer], f: &mut dyn FnMut(&mut Vec<Option<Cel>>)) {
            for l in layers {
                match &mut l.kind {
                    LayerKind::Raster { cels, .. } => f(cels),
                    LayerKind::Group { children } => go(children, f),
                    LayerKind::Reference { .. } | LayerKind::Skeleton { .. } => {}
                }
            }
        }
        go(&mut self.s.layers, &mut f);
    }

    fn for_each_rig(&mut self, mut f: impl FnMut(&mut Rig)) {
        fn go(layers: &mut [Layer], f: &mut dyn FnMut(&mut Rig)) {
            for l in layers {
                match &mut l.kind {
                    LayerKind::Skeleton { rig } => f(rig),
                    LayerKind::Group { children } => go(children, f),
                    _ => {}
                }
            }
        }
        go(&mut self.s.layers, &mut f);
    }

    fn check_frame(&self, frame: usize) -> Result<(), Error> {
        if frame < self.s.frames.len() {
            Ok(())
        } else {
            Err(Error::FrameOutOfRange(frame))
        }
    }

    /// Inserts an empty frame before `at` (`at == frame_count` appends).
    pub fn insert_frame(&mut self, at: usize, duration_ms: u32) -> Result<Rect, Error> {
        if at > self.s.frames.len() {
            return Err(Error::FrameOutOfRange(at));
        }
        self.s.frames.insert(at, Frame { duration_ms });
        self.for_each_raster(|cels| cels.insert(at, None));
        self.for_each_rig(|rig| rig.frame_inserted(at));
        for t in &mut self.s.tags {
            if t.from >= at {
                t.from += 1;
            }
            if t.to >= at {
                t.to += 1;
            }
        }
        Ok(self.bounds())
    }

    /// Inserts a copy of frame `at` after it and returns the new index. The
    /// copies own their pixels (they are not linked to the originals).
    pub fn duplicate_frame(&mut self, at: usize) -> Result<usize, Error> {
        self.check_frame(at)?;
        let duration = self.s.frames[at].duration_ms;
        self.insert_frame(at + 1, duration)?;
        let mut copies = Vec::new();
        self.for_each_raster(|cels| copies.push(cels[at]));
        let mut fresh = Vec::new();
        for c in copies {
            fresh.push(c.map(|c| {
                let raster = self.images[&c.image].clone();
                Cel {
                    image: self.alloc_image(raster),
                    opacity: c.opacity,
                }
            }));
        }
        let mut it = fresh.into_iter();
        self.for_each_raster(|cels| cels[at + 1] = it.next().flatten());
        self.for_each_rig(|rig| rig.frame_duplicated(at, at + 1));
        Ok(at + 1)
    }

    /// Deletes frame `at`. The last frame cannot be deleted.
    pub fn remove_frame(&mut self, at: usize) -> Result<Rect, Error> {
        self.check_frame(at)?;
        if self.s.frames.len() == 1 {
            return Err(Error::LastFrame);
        }
        self.s.frames.remove(at);
        self.for_each_raster(|cels| {
            cels.remove(at);
        });
        self.for_each_rig(|rig| rig.frame_removed(at));
        let last = self.s.frames.len() - 1;
        self.s.tags.retain_mut(|t| {
            if t.from == at && t.to == at {
                return false;
            }
            if t.from > at {
                t.from -= 1;
            }
            if t.to >= at && t.to > 0 {
                t.to -= 1;
            }
            t.to = t.to.min(last);
            true
        });
        self.collect_garbage();
        Ok(self.bounds())
    }

    /// Moves the `count` frames starting at `from` so that they start at
    /// `to` (counted after the move, so `0..=frame_count - count`). Every
    /// layer's cels move with their frames, and each tag keeps covering the
    /// frames it covered, stretched over where they went.
    pub fn move_frames(&mut self, from: usize, count: usize, to: usize) -> Result<(), Error> {
        let n = self.s.frames.len();
        if count == 0 || from + count > n {
            return Err(Error::FrameOutOfRange(from + count.max(1) - 1));
        }
        if to + count > n {
            return Err(Error::FrameOutOfRange(to + count - 1));
        }
        // order[new] = old
        let moved = from..from + count;
        let mut order: Vec<usize> = (0..n).filter(|i| !moved.contains(i)).collect();
        for (k, old) in moved.enumerate() {
            order.insert(to + k, old);
        }
        let mut new_of = vec![0; n];
        for (new, old) in order.iter().enumerate() {
            new_of[*old] = new;
        }
        let frames = self.s.frames.clone();
        self.s.frames = order.iter().map(|&o| frames[o]).collect();
        self.for_each_raster(|cels| {
            let old = cels.clone();
            *cels = order.iter().map(|&o| old[o]).collect();
        });
        self.for_each_rig(|rig| rig.frames_reordered(&order));
        for t in &mut self.s.tags {
            let spots = (t.from..=t.to.min(n - 1)).map(|f| new_of[f]);
            let (lo, hi) = spots.fold((usize::MAX, 0), |(lo, hi), f| (lo.min(f), hi.max(f)));
            if lo <= hi {
                t.from = lo;
                t.to = hi;
            }
        }
        Ok(())
    }

    pub fn set_frame_duration(&mut self, at: usize, duration_ms: u32) -> Result<(), Error> {
        self.check_frame(at)?;
        self.s.frames[at].duration_ms = duration_ms;
        Ok(())
    }

    // ---- cels ----

    fn cels(&self, layer: LayerId) -> Result<&Vec<Option<Cel>>, Error> {
        match &self.layer(layer).ok_or(Error::LayerNotFound(layer))?.kind {
            LayerKind::Raster { cels, .. } => Ok(cels),
            _ => Err(Error::NotRaster(layer)),
        }
    }

    fn cels_mut(&mut self, layer: LayerId) -> Result<&mut Vec<Option<Cel>>, Error> {
        match &mut self
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Raster { cels, .. } => Ok(cels),
            _ => Err(Error::NotRaster(layer)),
        }
    }

    pub fn cel(&self, layer: LayerId, frame: usize) -> Result<Option<Cel>, Error> {
        self.check_frame(frame)?;
        Ok(self.cels(layer)?[frame])
    }

    /// A cel's pixels, or `None` for an empty cel.
    pub fn cel_raster(&self, layer: LayerId, frame: usize) -> Result<Option<&Raster>, Error> {
        Ok(self.cel(layer, frame)?.map(|c| &self.images[&c.image]))
    }

    /// The frames whose cel on `layer` shares pixels with frame `frame`.
    pub fn linked_frames(&self, layer: LayerId, frame: usize) -> Result<Vec<usize>, Error> {
        let Some(cel) = self.cel(layer, frame)? else {
            return Ok(Vec::new());
        };
        let cels = self.cels(layer)?;
        Ok((0..cels.len())
            .filter(|&f| cels[f].is_some_and(|c| c.image == cel.image))
            .collect())
    }

    /// Makes frame `to` show the same pixels as frame `from`; edits to either
    /// show in both.
    pub fn link_cel(&mut self, layer: LayerId, from: usize, to: usize) -> Result<Rect, Error> {
        self.check_frame(from)?;
        self.check_frame(to)?;
        let cel = self.ensure_cel(layer, from)?;
        self.cels_mut(layer)?[to] = Some(cel);
        self.collect_garbage();
        Ok(self.bounds())
    }

    /// Gives frame `frame` its own copy of shared pixels.
    pub fn unlink_cel(&mut self, layer: LayerId, frame: usize) -> Result<(), Error> {
        let Some(cel) = self.cel(layer, frame)? else {
            return Ok(());
        };
        if self.linked_frames(layer, frame)?.len() < 2 {
            return Ok(());
        }
        let raster = self.images[&cel.image].clone();
        let image = self.alloc_image(raster);
        self.cels_mut(layer)?[frame] = Some(Cel { image, ..cel });
        Ok(())
    }

    /// Empties a cel.
    pub fn clear_cel(&mut self, layer: LayerId, frame: usize) -> Result<Rect, Error> {
        self.check_frame(frame)?;
        let dirty = match self.cel_raster(layer, frame)? {
            Some(r) => r.content_bounds(),
            None => Rect::EMPTY,
        };
        self.cels_mut(layer)?[frame] = None;
        self.collect_garbage();
        Ok(dirty)
    }

    pub fn set_cel_opacity(
        &mut self,
        layer: LayerId,
        frame: usize,
        opacity: u8,
    ) -> Result<Rect, Error> {
        let cel = self.ensure_cel(layer, frame)?;
        self.cels_mut(layer)?[frame] = Some(Cel { opacity, ..cel });
        Ok(self.bounds())
    }

    fn ensure_cel(&mut self, layer: LayerId, frame: usize) -> Result<Cel, Error> {
        self.check_frame(frame)?;
        if let Some(cel) = self.cels(layer)?[frame] {
            return Ok(cel);
        }
        let raster = self.empty_raster();
        let cel = Cel {
            image: self.alloc_image(raster),
            opacity: 255,
        };
        self.cels_mut(layer)?[frame] = Some(cel);
        Ok(cel)
    }

    /// The image a paint on `layer` at `frame` writes to, creating the cel if
    /// it is empty. Fails if the layer or a group holding it locks pixels.
    pub fn target(&mut self, layer: LayerId, frame: usize) -> Result<Target<'_>, Error> {
        let locks = self.effective_locks(layer)?;
        if locks.pixels {
            return Err(Error::Locked(layer));
        }
        let cel = self.ensure_cel(layer, frame)?;
        Ok(Target {
            raster: self.images.get_mut(&cel.image).expect("cel image exists"),
            palette: &self.s.palette,
            transparent_index: self.s.transparent_index,
            alpha_locked: locks.alpha,
        })
    }

    /// The image id a paint on `layer` at `frame` writes to (see
    /// [`Document::target`]); history scopes a transaction to it. Like
    /// `target`, it fails, before making any cel, if the layer or a group
    /// holding it locks pixels.
    pub fn target_image(&mut self, layer: LayerId, frame: usize) -> Result<ImageId, Error> {
        if self.effective_locks(layer)?.pixels {
            return Err(Error::Locked(layer));
        }
        Ok(self.ensure_cel(layer, frame)?.image)
    }

    // ---- masks ----

    /// Adds a mask that shows the whole layer; returns its image id.
    pub fn add_layer_mask(&mut self, layer: LayerId) -> Result<ImageId, Error> {
        let raster = Raster::full_mask(self.s.width, self.s.height);
        let image = self.alloc_image(raster);
        match &mut self
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Raster { mask, .. } => {
                *mask = Some(LayerMask {
                    image,
                    enabled: true,
                });
            }
            _ => {
                self.images.remove(&image);
                return Err(Error::NotRaster(layer));
            }
        }
        self.collect_garbage();
        Ok(image)
    }

    pub fn remove_layer_mask(&mut self, layer: LayerId) -> Result<Rect, Error> {
        match &mut self
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Raster { mask, .. } => *mask = None,
            _ => return Err(Error::NotRaster(layer)),
        }
        self.collect_garbage();
        Ok(self.bounds())
    }

    pub fn set_mask_enabled(&mut self, layer: LayerId, enabled: bool) -> Result<Rect, Error> {
        match &mut self
            .layer_mut(layer)
            .ok_or(Error::LayerNotFound(layer))?
            .kind
        {
            LayerKind::Raster { mask: Some(m), .. } => m.enabled = enabled,
            _ => return Err(Error::NotRaster(layer)),
        }
        Ok(self.bounds())
    }

    /// The image of a layer's mask, which a mask edit writes to.
    pub fn mask_image(&self, layer: LayerId) -> Result<ImageId, Error> {
        match &self.layer(layer).ok_or(Error::LayerNotFound(layer))?.kind {
            LayerKind::Raster { mask: Some(m), .. } => Ok(m.image),
            _ => Err(Error::NotRaster(layer)),
        }
    }

    /// A paintable view of a layer's mask. Masks ignore the alpha lock.
    pub fn mask_target(&mut self, layer: LayerId) -> Result<Target<'_>, Error> {
        if self.effective_locks(layer)?.pixels {
            return Err(Error::Locked(layer));
        }
        let image = match &self.layer(layer).ok_or(Error::LayerNotFound(layer))?.kind {
            LayerKind::Raster { mask: Some(m), .. } => m.image,
            _ => return Err(Error::NotRaster(layer)),
        };
        Ok(Target {
            raster: self.images.get_mut(&image).expect("mask image exists"),
            palette: &self.s.palette,
            transparent_index: self.s.transparent_index,
            alpha_locked: false,
        })
    }

    // ---- whole-document edits ----

    /// Converts every image to another color mode. RGBA to indexed maps each
    /// color to the nearest palette entry; transparent pixels become the
    /// transparent index.
    pub fn set_color_mode(&mut self, mode: ColorMode) -> Rect {
        if mode == self.s.mode {
            return Rect::EMPTY;
        }
        let palette = self.s.palette.clone();
        let ti = self.s.transparent_index;
        // Reference layers stay RGBA: they never take part in indexed editing.
        let refs = self.reference_images();
        for (id, raster) in self.images.iter_mut() {
            if refs.contains(id) {
                continue;
            }
            let converted = match (&*raster, mode) {
                (Raster::Rgba(img), ColorMode::Indexed) => Some(Raster::Indexed(
                    crate::palettes::index_image(img, &palette, ti, crate::palettes::Dither::None),
                )),
                (Raster::Indexed(img), ColorMode::Rgba) => {
                    Some(Raster::Rgba(img.map(Rgba::TRANSPARENT, |i| {
                        if i == ti {
                            Rgba::TRANSPARENT
                        } else {
                            palette.get(i)
                        }
                    })))
                }
                _ => None,
            };
            if let Some(c) = converted {
                *raster = c;
            }
        }
        self.s.mode = mode;
        self.bounds()
    }

    /// The images of reference layers.
    pub(crate) fn reference_images(&self) -> Vec<ImageId> {
        self.walk()
            .into_iter()
            .filter_map(|(_, l)| match l.kind {
                LayerKind::Reference { image } => Some(image),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::new(100, 80, ColorMode::Rgba).unwrap()
    }

    #[test]
    fn new_document_has_one_layer_and_frame() {
        let d = doc();
        assert_eq!(d.layers().len(), 1);
        assert_eq!(d.frame_count(), 1);
        assert_eq!(d.layers()[0].props.name, "Layer 1");
        assert!(Document::new(0, 5, ColorMode::Rgba).is_err());
        assert!(Document::new(5, MAX_SIZE + 1, ColorMode::Rgba).is_err());
    }

    #[test]
    fn groups_nest_and_moves_reject_cycles() {
        let mut d = doc();
        let base = d.layers()[0].id;
        let g = d
            .add_layer(NewLayer::Group, "g", Placement::default())
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
        d.move_layer(
            base,
            Placement {
                parent: Some(inner),
                index: Some(0),
            },
        )
        .unwrap();
        assert_eq!(d.ancestors(base), vec![inner, g]);
        assert_eq!(
            d.move_layer(
                g,
                Placement {
                    parent: Some(inner),
                    index: None
                }
            ),
            Err(Error::CyclicMove(g))
        );
        let depths: Vec<usize> = d.walk().iter().map(|(depth, _)| *depth).collect();
        assert_eq!(depths, vec![0, 1, 2]);
        assert_eq!(
            d.add_layer(
                NewLayer::Raster,
                "x",
                Placement {
                    parent: Some(base),
                    index: None
                }
            ),
            Err(Error::NotGroup(base))
        );
    }

    #[test]
    fn group_locks_apply_to_children() {
        let mut d = doc();
        let base = d.layers()[0].id;
        let g = d
            .add_layer(NewLayer::Group, "g", Placement::default())
            .unwrap();
        d.move_layer(
            base,
            Placement {
                parent: Some(g),
                index: None,
            },
        )
        .unwrap();
        d.update_layer(g, |p| p.locks.pixels = true).unwrap();
        assert!(matches!(d.target(base, 0), Err(Error::Locked(_))));
        // Refused before an empty cel is made for it.
        assert!(matches!(d.target_image(base, 0), Err(Error::Locked(_))));
        assert_eq!(d.cel(base, 0).unwrap(), None);
    }

    #[test]
    fn property_edits_report_visual_changes_only() {
        let mut d = doc();
        let id = d.layers()[0].id;
        assert_eq!(
            d.update_layer(id, |p| p.name = "a".into()).unwrap(),
            Rect::EMPTY
        );
        assert_eq!(d.update_layer(id, |p| p.opacity = 10).unwrap(), d.bounds());
    }

    #[test]
    fn frames_keep_cels_and_tags_aligned() {
        let mut d = doc();
        let id = d.layers()[0].id;
        d.target(id, 0)
            .unwrap()
            .raster
            .set_raw(1, 1, Rgba::BLACK.to_u32());
        d.tags_mut().push(Tag {
            name: "walk".into(),
            from: 0,
            to: 0,
            direction: LoopDirection::PingPong,
            repeat: 0,
            color: Rgba::BLACK,
        });
        d.insert_frame(0, 50).unwrap();
        assert_eq!(d.frame_count(), 2);
        assert!(d.cel(id, 0).unwrap().is_none());
        assert!(d.cel(id, 1).unwrap().is_some());
        assert_eq!((d.tags()[0].from, d.tags()[0].to), (1, 1));
        let copy = d.duplicate_frame(1).unwrap();
        assert_eq!(copy, 2);
        assert_ne!(
            d.cel(id, 1).unwrap().unwrap().image,
            d.cel(id, 2).unwrap().unwrap().image
        );
        assert_eq!(
            d.cel_raster(id, 2).unwrap().unwrap().get_raw(1, 1),
            Rgba::BLACK.to_u32()
        );
        d.remove_frame(0).unwrap();
        assert_eq!((d.tags()[0].from, d.tags()[0].to), (0, 0));
        d.remove_frame(0).unwrap();
        assert_eq!(d.remove_frame(0), Err(Error::LastFrame));
        assert_eq!(d.images.len(), 1, "removed frames free their images");
    }

    #[test]
    fn linked_cels_share_pixels_until_unlinked() {
        let mut d = doc();
        let id = d.layers()[0].id;
        d.insert_frame(1, 100).unwrap();
        d.link_cel(id, 0, 1).unwrap();
        assert_eq!(d.linked_frames(id, 1).unwrap(), vec![0, 1]);
        d.target(id, 1)
            .unwrap()
            .raster
            .set_raw(2, 2, Rgba::WHITE.to_u32());
        assert_eq!(
            d.cel_raster(id, 0).unwrap().unwrap().get_raw(2, 2),
            Rgba::WHITE.to_u32()
        );
        d.unlink_cel(id, 1).unwrap();
        d.target(id, 1).unwrap().raster.set_raw(2, 2, 0);
        assert_eq!(
            d.cel_raster(id, 0).unwrap().unwrap().get_raw(2, 2),
            Rgba::WHITE.to_u32()
        );
        assert_eq!(d.linked_frames(id, 0).unwrap(), vec![0]);
    }

    #[test]
    fn moved_frames_take_their_cels_and_tags_along() {
        let mut d = doc();
        let id = d.layers()[0].id;
        for f in 1..5 {
            d.insert_frame(f, 100 + f as u32).unwrap();
        }
        let cels: Vec<_> = (0..5).map(|f| d.target_image(id, f).unwrap()).collect();
        d.tags_mut().push(Tag {
            name: "walk".into(),
            from: 3,
            to: 4,
            direction: LoopDirection::Forward,
            repeat: 0,
            color: Rgba::WHITE,
        });
        // Frames 3 and 4 move to the front.
        d.move_frames(3, 2, 0).unwrap();
        let now: Vec<_> = (0..5)
            .map(|f| d.cel(id, f).unwrap().unwrap().image)
            .collect();
        assert_eq!(now, vec![cels[3], cels[4], cels[0], cels[1], cels[2]]);
        assert_eq!(d.frames()[0].duration_ms, 103);
        assert_eq!((d.tags()[0].from, d.tags()[0].to), (0, 1));
        // And one frame back to the end.
        d.move_frames(0, 1, 4).unwrap();
        assert_eq!(d.cel(id, 4).unwrap().unwrap().image, cels[3]);
        assert!(d.move_frames(4, 2, 0).is_err());
    }

    #[test]
    fn color_mode_round_trip_through_palette() {
        let mut d = doc();
        let id = d.layers()[0].id;
        let red = d.palette().get(4);
        d.target(id, 0).unwrap().raster.set_raw(0, 0, red.to_u32());
        d.set_color_mode(ColorMode::Indexed);
        let r = d.cel_raster(id, 0).unwrap().unwrap();
        assert_eq!(r.get_raw(0, 0), 4);
        assert_eq!(
            r.get_raw(5, 5),
            0,
            "transparent becomes the transparent index"
        );
        d.set_color_mode(ColorMode::Rgba);
        assert_eq!(
            d.cel_raster(id, 0).unwrap().unwrap().get_raw(0, 0),
            red.to_u32()
        );
    }

    #[test]
    fn masks_and_references() {
        let mut d = doc();
        let id = d.layers()[0].id;
        let m = d.add_layer_mask(id).unwrap();
        assert_eq!(d.image(m).unwrap().get_raw(3, 3), 255);
        d.mask_target(id).unwrap().raster.set_raw(3, 3, 0);
        d.remove_layer_mask(id).unwrap();
        assert!(d.image(m).is_none());
        let r = d
            .add_layer(
                NewLayer::Reference(TiledImage::new(100, 80, Rgba::WHITE)),
                "ref",
                Placement::default(),
            )
            .unwrap();
        assert!(matches!(d.target(r, 0), Err(Error::NotRaster(_))));
        assert!(
            d.add_layer(
                NewLayer::Reference(TiledImage::new(5, 5, Rgba::WHITE)),
                "bad",
                Placement::default()
            )
            .is_err()
        );
    }
}
