//! The canvas view: the document drawn through Lumen's canvas, zoomed and
//! panned, over a checkerboard and under a pixel grid.
//!
//! Three canvases are stacked in the stage. `checker` shows the
//! checkerboard where the document is transparent, `art` shows the layers,
//! and `overlay` shows the pixel grid, the selection and tool previews. The
//! art canvas is its own picture, so a layer's blend mode composites with
//! the layers below it and never with the checkerboard.
//!
//! Each layer is a pixel buffer, drawn with its blend mode
//! (`globalCompositeOperation`) and opacity by Lumen's renderer, on the GPU
//! when the app renders there. What Lumen cannot composite the way pixl-core
//! does (a blend mode the canvas lacks, a clipping chain, a layer mask, a
//! group) is composited by pixl-core on the CPU into one buffer instead; see
//! [`plan`]. A buffer is rewritten only where the document's tiles changed,
//! which the view finds by comparing tile pointers with the copy it last
//! showed (tiles are copy-on-write, so an untouched tile keeps its pointer).
//! Large rewrites run on worker threads and appear tile by tile
//! ([`crate::progressive`]).

use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use lumenui::glam::Vec2;
use lumenui::lumen_core::components::Transform;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{Canvas, CompositeOperation, Context2d, PixelBuffer};
use pixl_core::{
    Background, BlendMode, CompositeOptions, Document, LayerId, LayerKind, Palette, Point, Raster,
    Rect, Rgba, TILE_SIZE,
};

use crate::editor::Editor;
use crate::progressive::{self, Pool};
use crate::settings::Look;

/// The zoom levels the wheel and the zoom keys step through, as scale
/// factors (1.0 is 100 percent).
pub const ZOOM_STEPS: [f64; 26] = [
    0.01,
    0.02,
    0.03,
    0.05,
    0.08,
    0.125,
    1.0 / 6.0,
    0.25,
    1.0 / 3.0,
    0.5,
    2.0 / 3.0,
    1.0,
    1.5,
    2.0,
    3.0,
    4.0,
    6.0,
    8.0,
    12.0,
    16.0,
    24.0,
    32.0,
    40.0,
    48.0,
    56.0,
    64.0,
];

/// The smallest zoom that shows the pixel grid.
pub const GRID_FROM_ZOOM: f64 = 8.0;

/// Screen pixels per checkerboard cell.
const CHECKER_CELL: f64 = 8.0;

/// Dirty tiles a tick rewrites itself; more go to the workers.
const SYNC_TILES: usize = 24;

/// Time a tick spends writing finished tiles into buffers.
const DRAIN_BUDGET: Duration = Duration::from_millis(6);

/// Where the document sits in the stage.
#[derive(Resource, Clone, Debug)]
pub struct View {
    /// Screen pixels per document pixel.
    pub zoom: f64,
    /// The stage position of the document's top-left corner.
    pub origin: (f64, f64),
    /// The stage in window coordinates: top-left and size.
    pub stage: (Vec2, Vec2),
    /// Fit the document into the stage at the next chance.
    pub fit_pending: bool,
    /// The document generation the view was fitted to.
    fitted_generation: u64,
    /// Bumped whenever zoom, pan or the stage size change.
    pub revision: u64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            origin: (0.0, 0.0),
            stage: (Vec2::ZERO, Vec2::ZERO),
            fit_pending: true,
            fitted_generation: 0,
            revision: 1,
        }
    }
}

impl View {
    /// A window position in stage coordinates.
    pub fn stage_point(&self, window: Vec2) -> (f64, f64) {
        (
            f64::from(window.x - self.stage.0.x),
            f64::from(window.y - self.stage.0.y),
        )
    }

    /// Whether a window position is over the stage.
    pub fn contains(&self, window: Vec2) -> bool {
        let (x, y) = self.stage_point(window);
        x >= 0.0 && y >= 0.0 && x < f64::from(self.stage.1.x) && y < f64::from(self.stage.1.y)
    }

    /// The document position under a stage position, in pixels (fractional).
    pub fn to_doc(&self, stage: (f64, f64)) -> (f64, f64) {
        (
            (stage.0 - self.origin.0) / self.zoom,
            (stage.1 - self.origin.1) / self.zoom,
        )
    }

    /// The document pixel under a window position.
    pub fn doc_pixel(&self, window: Vec2) -> Point {
        let (x, y) = self.to_doc(self.stage_point(window));
        Point::new(x.floor() as i32, y.floor() as i32)
    }

    /// Zooms to `zoom`, keeping the document point under `anchor` (stage
    /// coordinates) where it is.
    pub fn zoom_at(&mut self, zoom: f64, anchor: (f64, f64)) {
        let zoom = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        let (dx, dy) = self.to_doc(anchor);
        self.zoom = zoom;
        self.origin = (
            (anchor.0 - dx * zoom).round(),
            (anchor.1 - dy * zoom).round(),
        );
        self.revision += 1;
    }

    /// The next zoom step in or out from the current zoom.
    pub fn step(&self, inward: bool) -> f64 {
        if inward {
            ZOOM_STEPS
                .into_iter()
                .find(|&z| z > self.zoom * 1.0001)
                .unwrap_or(self.zoom)
        } else {
            ZOOM_STEPS
                .into_iter()
                .rev()
                .find(|&z| z < self.zoom / 1.0001)
                .unwrap_or(self.zoom)
        }
    }

    /// The stage's middle, in stage coordinates.
    pub fn middle(&self) -> (f64, f64) {
        (
            f64::from(self.stage.1.x) / 2.0,
            f64::from(self.stage.1.y) / 2.0,
        )
    }

    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.origin = ((self.origin.0 + dx).round(), (self.origin.1 + dy).round());
        self.revision += 1;
    }

    /// The largest zoom step that shows the whole document with a margin,
    /// centered.
    pub fn fit(&mut self, width: u32, height: u32) {
        let (sw, sh) = (f64::from(self.stage.1.x), f64::from(self.stage.1.y));
        if sw <= 0.0 || sh <= 0.0 {
            return;
        }
        let want = ((sw - 48.0) / f64::from(width)).min((sh - 48.0) / f64::from(height));
        let zoom = ZOOM_STEPS
            .into_iter()
            .rev()
            .find(|&z| z <= want)
            .unwrap_or(ZOOM_STEPS[0]);
        self.zoom = zoom;
        self.origin = (
            ((sw - f64::from(width) * zoom) / 2.0).round(),
            ((sh - f64::from(height) * zoom) / 2.0).round(),
        );
        self.fit_pending = false;
        self.revision += 1;
    }

    /// The document rectangle on the stage: x, y, width, height.
    pub fn doc_rect(&self, width: u32, height: u32) -> (f64, f64, f64, f64) {
        (
            self.origin.0,
            self.origin.1,
            f64::from(width) * self.zoom,
            f64::from(height) * self.zoom,
        )
    }

    /// The document pixels the stage shows, clipped to the document.
    pub fn visible_pixels(&self, width: u32, height: u32) -> Rect {
        let (x0, y0) = self.to_doc((0.0, 0.0));
        let (x1, y1) = self.to_doc((f64::from(self.stage.1.x), f64::from(self.stage.1.y)));
        let r = Rect::new(
            x0.floor() as i32,
            y0.floor() as i32,
            (x1.ceil() - x0.floor()) as i32,
            (y1.ceil() - y0.floor()) as i32,
        );
        r.intersect(&Rect::new(0, 0, width as i32, height as i32))
    }
}

/// The colors the view draws with, read from the theme's probe elements so
/// the stylesheet owns them.
#[derive(Resource, Clone, Debug, Default)]
pub struct ViewColors {
    pub checker_a: Option<Rgba>,
    pub checker_b: Option<Rgba>,
    pub grid: Option<Rgba>,
    pub marquee_a: Option<Rgba>,
    pub marquee_b: Option<Rgba>,
    pub preview: Option<Rgba>,
    /// The look these were read under.
    read_for: Option<Look>,
    /// Ticks since the look changed; the cascade lands a tick or two later.
    settle: u32,
    pub revision: u64,
}

impl ViewColors {
    fn complete(&self) -> bool {
        self.checker_a.is_some()
            && self.checker_b.is_some()
            && self.grid.is_some()
            && self.marquee_a.is_some()
            && self.marquee_b.is_some()
            && self.preview.is_some()
    }
}

/// Parses `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(text: &str) -> Option<Rgba> {
    let hex = text.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some(Rgba::opaque(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Rgba::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

fn probe(id: &str) -> Option<Rgba> {
    dom::get_by_id(id)?
        .computed_style_of("bg")
        .as_deref()
        .and_then(parse_hex)
}

/// Re-reads the view colors after the look changes.
pub fn read_view_colors(look: Res<Look>, mut colors: ResMut<ViewColors>) {
    if colors.read_for != Some(*look) {
        colors.read_for = Some(*look);
        colors.settle = 0;
    }
    // Read for a few ticks after a change, since the new classes take a
    // tick to land and another to cascade.
    if colors.settle > 4 && colors.complete() {
        return;
    }
    colors.settle += 1;
    let read = ViewColors {
        checker_a: probe("probe-checker-a"),
        checker_b: probe("probe-checker-b"),
        grid: probe("probe-grid"),
        marquee_a: probe("probe-marquee-a"),
        marquee_b: probe("probe-marquee-b"),
        preview: probe("probe-preview"),
        ..ViewColors::default()
    };
    let changed = read.checker_a != colors.checker_a
        || read.checker_b != colors.checker_b
        || read.grid != colors.grid
        || read.marquee_a != colors.marquee_a
        || read.marquee_b != colors.marquee_b
        || read.preview != colors.preview;
    if changed {
        colors.checker_a = read.checker_a;
        colors.checker_b = read.checker_b;
        colors.grid = read.grid;
        colors.marquee_a = read.marquee_a;
        colors.marquee_b = read.marquee_b;
        colors.preview = read.preview;
        colors.revision += 1;
    }
}

fn canvas_color(c: Rgba) -> lumenui_canvas::Rgba {
    lumenui_canvas::Rgba::new(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        f64::from(c.a) / 255.0,
    )
}

/// What tools draw over the document: a shape preview, in document pixels.
#[derive(Resource, Clone, Debug, Default)]
pub struct Preview {
    pub pixels: Vec<Point>,
    pub color: Rgba,
    pub revision: u64,
}

impl Preview {
    pub fn set(&mut self, pixels: Vec<Point>, color: Rgba) {
        self.pixels = pixels;
        self.color = color;
        self.revision += 1;
    }

    pub fn clear(&mut self) {
        if !self.pixels.is_empty() {
            self.pixels.clear();
            self.revision += 1;
        }
    }
}

// ---- the composition plan ----

/// Where one buffer's pixels come from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemSource {
    /// A root raster or reference layer's own pixels.
    Layer(LayerId),
    /// A root layer composited alone by pixl-core: a masked raster layer, or
    /// a group with its children.
    Isolated(LayerId),
    /// The bottom root layers, flattened by pixl-core onto the background:
    /// everything up to the topmost layer the canvas cannot composite on its
    /// own.
    Flat(Vec<LayerId>),
}

/// One buffer the art canvas draws, and how.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanItem {
    pub source: ItemSource,
    pub op: CompositeOperation,
    pub alpha: f64,
    pub visible: bool,
}

/// The canvas operation that composites like pixl-core's blend mode, where
/// the two agree.
pub fn canvas_blend(mode: BlendMode) -> Option<CompositeOperation> {
    use CompositeOperation as C;
    Some(match mode {
        BlendMode::Normal => C::SourceOver,
        BlendMode::Multiply => C::Multiply,
        BlendMode::Screen => C::Screen,
        BlendMode::Overlay => C::Overlay,
        BlendMode::Darken => C::Darken,
        BlendMode::Lighten => C::Lighten,
        BlendMode::ColorDodge => C::ColorDodge,
        BlendMode::ColorBurn => C::ColorBurn,
        BlendMode::HardLight => C::HardLight,
        BlendMode::SoftLight => C::SoftLight,
        BlendMode::Difference => C::Difference,
        BlendMode::Exclusion => C::Exclusion,
        BlendMode::Hue => C::Hue,
        BlendMode::Saturation => C::Saturation,
        BlendMode::Color => C::Color,
        BlendMode::Luminosity => C::Luminosity,
        // The canvas's `lighter` adds premultiplied colors and alphas, and
        // it has no subtract: pixl-core's add and subtract blend straight
        // colors inside the W3C model, so they stay on the CPU.
        BlendMode::Add | BlendMode::Subtract => return None,
    })
}

/// How a root layer reaches the canvas.
enum Class {
    Direct(CompositeOperation),
    Isolated(CompositeOperation),
    Backdrop,
}

fn classify(roots: &[pixl_core::Layer], i: usize) -> Class {
    let layer = &roots[i];
    let clips_above = roots.get(i + 1).is_some_and(|l| l.props.clip);
    if layer.props.clip || clips_above {
        return Class::Backdrop;
    }
    let Some(op) = canvas_blend(layer.props.blend) else {
        return Class::Backdrop;
    };
    match &layer.kind {
        LayerKind::Raster { mask, .. } => {
            if mask.is_some_and(|m| m.enabled) {
                Class::Isolated(op)
            } else {
                Class::Direct(op)
            }
        }
        LayerKind::Reference { .. } => Class::Direct(op),
        LayerKind::Group { .. } => Class::Isolated(op),
    }
}

/// The most buffer pixels the view hands Lumen's renderer: two buffers of
/// 4096x4096. Lumen's GPU renderer drops pixel buffers past about that much
/// image area (lumen-fx/lumen#521), so past it the bottom layers are
/// flattened by pixl-core into one buffer instead.
pub const BUFFER_PIXEL_BUDGET: u64 = 2 * 4096 * 4096;

/// The buffers that show `frame` of `doc`, bottom first, within
/// [`BUFFER_PIXEL_BUDGET`].
pub fn plan(doc: &Document, frame: usize) -> Vec<PlanItem> {
    plan_within(doc, frame, BUFFER_PIXEL_BUDGET)
}

/// [`plan`] with a buffer pixel budget of `budget`.
pub fn plan_within(doc: &Document, frame: usize, budget: u64) -> Vec<PlanItem> {
    let roots = doc.layers();
    let classes: Vec<Class> = (0..roots.len()).map(|i| classify(roots, i)).collect();
    let mut split = classes.iter().rposition(|c| matches!(c, Class::Backdrop));
    // Flatten from the bottom until the buffers fit the budget.
    let area = u64::from(doc.width()) * u64::from(doc.height());
    let fits = |split: Option<usize>| {
        let buffers = match split {
            Some(s) => 1 + roots.len() - (s + 1),
            None => roots.len(),
        } as u64;
        buffers * area <= budget
    };
    while !fits(split) && split.is_none_or(|s| s + 1 < roots.len()) {
        split = Some(split.map_or(0, |s| s + 1));
        // One root alone needs no flattening: it already is one buffer.
        if split == Some(0) && roots.len() == 1 {
            split = None;
            break;
        }
    }
    let mut items = Vec::new();
    let first = match split {
        Some(s) => {
            items.push(PlanItem {
                source: ItemSource::Flat(roots[..=s].iter().map(|l| l.id).collect()),
                op: CompositeOperation::SourceOver,
                alpha: 1.0,
                visible: true,
            });
            s + 1
        }
        None => 0,
    };
    for (layer, class) in roots.iter().zip(&classes).skip(first) {
        let opacity = f64::from(layer.props.opacity) / 255.0;
        let item = match class {
            Class::Direct(op) => {
                let cel = match &layer.kind {
                    LayerKind::Raster { cels, .. } => cels
                        .get(frame)
                        .copied()
                        .flatten()
                        .map_or(1.0, |c| f64::from(c.opacity) / 255.0),
                    _ => 1.0,
                };
                PlanItem {
                    source: ItemSource::Layer(layer.id),
                    op: *op,
                    alpha: opacity * cel,
                    visible: layer.props.visible,
                }
            }
            Class::Isolated(op) => PlanItem {
                source: ItemSource::Isolated(layer.id),
                op: *op,
                alpha: opacity,
                visible: layer.props.visible,
            },
            Class::Backdrop => unreachable!("backdrop layers are all at or below the split"),
        };
        items.push(item);
    }
    items
}

/// What a composite needs: the view's options, and a copy of the document
/// showing only the item's layers.
pub fn prepared(doc: &Document, source: &ItemSource) -> (Document, CompositeOptions) {
    let mut doc = doc.clone();
    let roots: Vec<LayerId> = doc.layers().iter().map(|l| l.id).collect();
    let mut opts = CompositeOptions {
        include_reference: true,
        background: true,
        onion: None,
    };
    match source {
        ItemSource::Layer(id) | ItemSource::Isolated(id) => {
            for r in roots.iter().filter(|r| *r != id) {
                let _ = doc.update_layer(*r, |p| p.visible = false);
            }
            let _ = doc.update_layer(*id, |p| {
                p.visible = true;
                p.opacity = 255;
                p.blend = BlendMode::Normal;
            });
            opts.background = false;
        }
        ItemSource::Flat(ids) => {
            for r in roots.iter().filter(|r| !ids.contains(r)) {
                let _ = doc.update_layer(*r, |p| p.visible = false);
            }
        }
    }
    (doc, opts)
}

/// The pixels an item shows, as of its last write, for finding what changed.
#[derive(Clone, Debug, Default)]
struct Shown {
    /// `(layer, cel or reference image, mask)` for each layer the item
    /// covers.
    images: Vec<(LayerId, Option<Raster>, Option<Raster>)>,
    /// Everything else that changes the item's pixels.
    signature: u64,
}

fn layer_images(
    doc: &Document,
    layer: &pixl_core::Layer,
    frame: usize,
) -> (Option<Raster>, Option<Raster>) {
    match &layer.kind {
        LayerKind::Raster { cels, mask } => {
            let cel = cels
                .get(frame)
                .copied()
                .flatten()
                .and_then(|c| doc.image(c.image).cloned());
            let mask = mask.and_then(|m| doc.image(m.image).cloned());
            (cel, mask)
        }
        LayerKind::Reference { image } => (doc.image(*image).cloned(), None),
        LayerKind::Group { .. } => (None, None),
    }
}

fn subtree<'a>(layer: &'a pixl_core::Layer, out: &mut Vec<&'a pixl_core::Layer>) {
    out.push(layer);
    for c in layer.children() {
        subtree(c, out);
    }
}

fn shown_for(doc: &Document, source: &ItemSource, frame: usize) -> Shown {
    let roots: Vec<&pixl_core::Layer> = match source {
        ItemSource::Layer(id) | ItemSource::Isolated(id) => {
            doc.layers().iter().filter(|l| l.id == *id).collect()
        }
        ItemSource::Flat(ids) => doc
            .layers()
            .iter()
            .filter(|l| ids.contains(&l.id))
            .collect(),
    };
    let mut layers = Vec::new();
    for r in roots {
        subtree(r, &mut layers);
    }
    let mut h = DefaultHasher::new();
    doc.width().hash(&mut h);
    doc.height().hash(&mut h);
    let indexed = doc.mode() == pixl_core::ColorMode::Indexed;
    if indexed {
        for c in doc.palette().colors() {
            c.hash(&mut h);
        }
        doc.transparent_index().hash(&mut h);
    }
    let composited = !matches!(source, ItemSource::Layer(_));
    if composited {
        if matches!(source, ItemSource::Flat(_))
            && let Background::Color(c) = doc.background()
        {
            c.hash(&mut h);
        }
        for l in &layers {
            l.id.hash(&mut h);
            l.props.visible.hash(&mut h);
            l.props.clip.hash(&mut h);
            if !matches!(source, ItemSource::Isolated(id) if *id == l.id) {
                l.props.opacity.hash(&mut h);
                l.props.blend.code().hash(&mut h);
            }
            if let LayerKind::Raster { cels, mask } = &l.kind {
                mask.map(|m| m.enabled).hash(&mut h);
                cels.get(frame)
                    .copied()
                    .flatten()
                    .map(|c| c.opacity)
                    .hash(&mut h);
            }
        }
    }
    Shown {
        images: layers
            .iter()
            .map(|l| {
                let (img, mask) = layer_images(doc, l, frame);
                (l.id, img, mask)
            })
            .collect(),
        signature: h.finish(),
    }
}

/// Whether tile `t` of a raster holds pixels.
fn present(r: &Raster, t: usize) -> bool {
    match r.tile_snap(t) {
        pixl_core::image::TileSnap::Rgba(p) => p.is_some(),
        pixl_core::image::TileSnap::Byte(p) => p.is_some(),
    }
}

/// The tiles that differ between two versions of one image.
fn changed_tiles(old: Option<&Raster>, new: Option<&Raster>, out: &mut BTreeSet<usize>) {
    match (old, new) {
        (None, None) => {}
        (Some(a), Some(b)) if a.same_shape(b) => {
            out.extend((0..b.tile_count()).filter(|&t| !a.same_tile(b, t)));
        }
        (Some(a), Some(b)) => {
            out.extend((0..a.tile_count()).filter(|&t| present(a, t)));
            out.extend((0..b.tile_count()).filter(|&t| present(b, t)));
        }
        (Some(r), None) | (None, Some(r)) => {
            out.extend((0..r.tile_count()).filter(|&t| present(r, t)));
        }
    }
}

/// A buffer on the art canvas.
struct Item {
    key: u64,
    plan: PlanItem,
    buffer: PixelBuffer,
    shown: Shown,
    /// Per document tile, the stamp of the pixels the buffer holds there.
    applied: Vec<u64>,
}

/// One progressive rewrite, for the timeline.
#[derive(Clone, Debug)]
pub struct Rewrite {
    pub item: u64,
    pub started: Instant,
    pub tiles: usize,
    pub done: usize,
    /// When each finished tile reached the buffer, from `started`.
    pub arrivals: Vec<Duration>,
}

/// The buffers of the document's layers, kept in step with the document.
#[derive(Resource)]
pub struct Layers {
    items: Vec<Item>,
    pool: Pool,
    next_key: u64,
    seen_revision: u64,
    seen_generation: u64,
    seen_frame: usize,
    /// Something drawn changed since the art canvas was last drawn.
    pub art_dirty: bool,
    pub rewrites: Vec<Rewrite>,
    /// Set when a buffer could not be made, such as a document past the
    /// canvas size cap.
    pub error: Option<String>,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            pool: Pool::new(),
            next_key: 1,
            seen_revision: 0,
            seen_generation: u64::MAX,
            seen_frame: 0,
            art_dirty: true,
            rewrites: Vec::new(),
            error: None,
        }
    }
}

impl Layers {
    /// The plan the buffers currently follow.
    pub fn current_plan(&self) -> Vec<PlanItem> {
        self.items.iter().map(|i| i.plan.clone()).collect()
    }

    /// The buffer an item of the plan draws.
    pub fn buffer(&self, source: &ItemSource) -> Option<&PixelBuffer> {
        self.items
            .iter()
            .find(|i| &i.plan.source == source)
            .map(|i| &i.buffer)
    }

    /// Tiles queued for the workers, or rendered but not yet written.
    pub fn pending(&self) -> usize {
        self.pool.queued()
            + self
                .rewrites
                .iter()
                .map(|r| r.tiles.saturating_sub(r.done))
                .sum::<usize>()
    }

    /// Brings the buffers up to date with the document. `focus` is the
    /// document point in the middle of the view; progressive rewrites start
    /// there.
    pub fn sync(&mut self, editor: &Editor, focus: (f64, f64)) {
        let doc = &editor.doc;
        if editor.generation != self.seen_generation {
            for item in self.items.drain(..) {
                self.pool.cancel(item.key);
            }
            self.rewrites.clear();
            self.seen_generation = editor.generation;
            self.seen_revision = 0;
            self.error = None;
        }
        if editor.revision == self.seen_revision && editor.frame == self.seen_frame {
            return;
        }
        self.seen_revision = editor.revision;
        self.seen_frame = editor.frame;
        let frame = editor.frame;
        let wanted = plan(doc, frame);

        // Keep the buffers whose source is still wanted, in the new order.
        let mut old: Vec<Option<Item>> = self.items.drain(..).map(Some).collect();
        let mut items = Vec::with_capacity(wanted.len());
        for p in wanted {
            let found = old
                .iter_mut()
                .find(|o| o.as_ref().is_some_and(|i| i.plan.source == p.source))
                .and_then(Option::take);
            let item = match found {
                Some(mut item) => {
                    if item.plan != p {
                        self.art_dirty = true;
                    }
                    item.plan = p;
                    item
                }
                None => {
                    let buffer = match PixelBuffer::new(doc.width(), doc.height()) {
                        Ok(b) => b,
                        Err(e) => {
                            self.error = Some(format!("the canvas cannot show this document: {e}"));
                            continue;
                        }
                    };
                    self.art_dirty = true;
                    let key = self.next_key;
                    self.next_key += 1;
                    let tiles = doc.width().div_ceil(TILE_SIZE as u32) as usize
                        * doc.height().div_ceil(TILE_SIZE as u32) as usize;
                    Item {
                        key,
                        plan: p,
                        buffer,
                        shown: Shown::default(),
                        applied: vec![0; tiles],
                    }
                }
            };
            items.push(item);
        }
        for gone in old.into_iter().flatten() {
            self.pool.cancel(gone.key);
            self.art_dirty = true;
        }
        self.items = items;

        // Rewrite what changed in each buffer.
        let cols = doc.width().div_ceil(TILE_SIZE as u32) as i32;
        for i in 0..self.items.len() {
            let now = shown_for(doc, &self.items[i].plan.source, frame);
            let item = &self.items[i];
            let mut tiles = BTreeSet::new();
            let whole = now.signature != item.shown.signature
                && !(item.shown.images.is_empty()
                    && matches!(item.plan.source, ItemSource::Layer(_)));
            if whole {
                tiles.extend(0..item.applied.len());
            } else {
                for (id, img, mask) in &now.images {
                    let before = item.shown.images.iter().find(|(o, _, _)| o == id);
                    changed_tiles(before.and_then(|b| b.1.as_ref()), img.as_ref(), &mut tiles);
                    changed_tiles(before.and_then(|b| b.2.as_ref()), mask.as_ref(), &mut tiles);
                }
                for (id, img, mask) in &item.shown.images {
                    if !now.images.iter().any(|(n, _, _)| n == id) {
                        changed_tiles(img.as_ref(), None, &mut tiles);
                        changed_tiles(mask.as_ref(), None, &mut tiles);
                    }
                }
            }
            let rects: Vec<Rect> = tiles
                .iter()
                .map(|&t| {
                    let t = t as i32;
                    Rect::new(
                        (t % cols) * TILE_SIZE,
                        (t / cols) * TILE_SIZE,
                        TILE_SIZE,
                        TILE_SIZE,
                    )
                    .intersect(&doc.bounds())
                })
                .collect();
            let source = self.items[i].plan.source.clone();
            self.items[i].shown = now;
            if rects.is_empty() {
                continue;
            }
            self.art_dirty = true;
            let work = match &source {
                ItemSource::Layer(id) => {
                    let layer = doc.layer(*id).expect("planned layers exist");
                    let (img, _) = layer_images(doc, layer, frame);
                    match img {
                        Some(raster) => progressive::Source::Raster {
                            raster,
                            palette: doc.palette().clone(),
                            transparent_index: doc.transparent_index(),
                        },
                        None => progressive::Source::Raster {
                            raster: Raster::Rgba(pixl_core::TiledImage::new(
                                doc.width(),
                                doc.height(),
                                Rgba::TRANSPARENT,
                            )),
                            palette: Palette::new(Vec::new()).expect("an empty palette"),
                            transparent_index: 0,
                        },
                    }
                }
                other => {
                    let (prepared, opts) = prepared(doc, other);
                    progressive::Source::Composite {
                        doc: prepared,
                        frame,
                        opts,
                    }
                }
            };
            let item = &mut self.items[i];
            if rects.len() <= SYNC_TILES {
                let stamp = self.pool.stamp();
                for r in rects {
                    let rgba = work.render(r);
                    write_tile(&item.buffer, &mut item.applied, cols, r, stamp, &rgba);
                }
            } else {
                let mut rects = rects;
                let fx = focus.0;
                let fy = focus.1;
                rects.sort_by(|a, b| {
                    let d = |r: &Rect| {
                        let x = f64::from(r.x) + f64::from(r.w) / 2.0 - fx;
                        let y = f64::from(r.y) + f64::from(r.h) / 2.0 - fy;
                        x * x + y * y
                    };
                    d(a).total_cmp(&d(b))
                });
                self.pool.cancel(item.key);
                self.rewrites.retain(|r| r.item != item.key);
                self.rewrites.push(Rewrite {
                    item: item.key,
                    started: Instant::now(),
                    tiles: rects.len(),
                    done: 0,
                    arrivals: Vec::new(),
                });
                self.pool.submit(item.key, work, rects);
            }
        }
    }

    /// Writes finished worker tiles into their buffers, for up to the drain
    /// budget. Returns whether anything was written.
    pub fn drain(&mut self, doc_width: u32) -> bool {
        let deadline = Instant::now() + DRAIN_BUDGET;
        let cols = doc_width.div_ceil(TILE_SIZE as u32) as i32;
        let mut wrote = false;
        while Instant::now() < deadline {
            let Some(d) = self.pool.next_finished() else {
                break;
            };
            let Some(item) = self.items.iter_mut().find(|i| i.key == d.item) else {
                continue;
            };
            if write_tile(
                &item.buffer,
                &mut item.applied,
                cols,
                d.rect,
                d.stamp,
                &d.rgba,
            ) {
                wrote = true;
            }
            if let Some(r) = self.rewrites.iter_mut().find(|r| r.item == d.item) {
                r.done += 1;
                r.arrivals.push(r.started.elapsed());
            }
        }
        if wrote {
            self.art_dirty = true;
        }
        wrote
    }

    /// Draws every visible buffer onto the art canvas.
    fn draw(&self, ctx: &Context2d, doc: &Document, view: &View) {
        ctx.clear();
        ctx.set_image_smoothing(false);
        let (x, y, w, h) = view.doc_rect(doc.width(), doc.height());
        let flattened = self
            .items
            .iter()
            .any(|i| matches!(i.plan.source, ItemSource::Flat(_)));
        if !flattened && let Background::Color(c) = doc.background() {
            ctx.set_fill_color(canvas_color(c));
            ctx.fill_rect(x, y, w, h);
        }
        for item in self
            .items
            .iter()
            .filter(|i| i.plan.visible && i.plan.alpha > 0.0)
        {
            ctx.set_global_composite_operation(item.plan.op);
            ctx.set_global_alpha(item.plan.alpha);
            ctx.draw_buffer_scaled(&item.buffer, x, y, w, h);
        }
        ctx.set_global_composite_operation(CompositeOperation::SourceOver);
        ctx.set_global_alpha(1.0);
    }
}

/// Writes one tile's rows into a buffer unless a newer write got there
/// first. Returns whether it wrote.
fn write_tile(
    buffer: &PixelBuffer,
    applied: &mut [u64],
    cols: i32,
    rect: Rect,
    stamp: u64,
    rgba: &[u8],
) -> bool {
    let t = ((rect.y / TILE_SIZE) * cols + rect.x / TILE_SIZE) as usize;
    if applied.get(t).is_some_and(|&s| s > stamp) {
        return false;
    }
    if let Some(slot) = applied.get_mut(t) {
        *slot = stamp;
    }
    buffer
        .write_rgba8(
            i64::from(rect.x),
            i64::from(rect.y),
            rect.w as u32,
            rect.h as u32,
            rgba,
        )
        .is_ok()
}

/// What each stage canvas last drew, so a tick redraws only what changed.
#[derive(Resource, Default)]
pub struct Drawn {
    size: (f32, f32),
    view: u64,
    colors: u64,
    preview: u64,
    selection: Option<Rect>,
    checker: Option<(PixelBuffer, u32, u32)>,
    checker_colors: u64,
    doc_size: (u32, u32),
    background: Option<Background>,
}

/// Measures the stage and fits the document into it when asked.
pub fn measure_stage(
    canvases: Query<(&Canvas, &Transform)>,
    editor: Res<Editor>,
    mut view: ResMut<View>,
) {
    let Some((_, t)) = canvases.iter().find(|(c, _)| c.id == "art") else {
        return;
    };
    if (t.absolute, t.size) != view.stage {
        view.stage = (t.absolute, t.size);
        view.revision += 1;
    }
    if editor.generation != view.fitted_generation {
        view.fitted_generation = editor.generation;
        view.fit_pending = true;
    }
    if view.fit_pending && t.size.x > 0.0 && t.size.y > 0.0 {
        view.fit(editor.doc.width(), editor.doc.height());
    }
}

/// Keeps the layer buffers in step with the document.
pub fn sync_layers(
    editor: Res<Editor>,
    view: Res<View>,
    mut layers: ResMut<Layers>,
    wake: Option<Res<WakeDeadline>>,
) {
    let mid = view.to_doc(view.middle());
    layers.sync(&editor, mid);
    layers.drain(editor.doc.width());
    if layers.pending() > 0
        && let Some(wake) = wake
    {
        // Keep ticking while workers are rendering, so each finished tile
        // shows on the next frame.
        wake.request(Instant::now() + Duration::from_millis(4));
    }
}

/// Draws the three stage canvases where something they show changed.
pub fn draw_stage(
    editor: Res<Editor>,
    view: Res<View>,
    colors: Res<ViewColors>,
    preview: Res<Preview>,
    mut layers: ResMut<Layers>,
    mut drawn: ResMut<Drawn>,
) {
    let size = (view.stage.1.x, view.stage.1.y);
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return;
    }
    let checker = Context2d::new("checker");
    let art = Context2d::new("art");
    let overlay = Context2d::new("overlay");
    let resized = drawn.size != size;
    if resized {
        for ctx in [&checker, &art, &overlay] {
            ctx.resize(size.0, size.1);
        }
        drawn.size = size;
    }
    let doc = &editor.doc;
    let doc_size = (doc.width(), doc.height());
    let view_changed = drawn.view != view.revision || resized || drawn.doc_size != doc_size;

    if view_changed || drawn.colors != colors.revision || drawn.background != Some(doc.background())
    {
        draw_checker(&checker, &mut drawn, &view, &colors, doc_size);
    }
    if view_changed || layers.art_dirty {
        layers.draw(&art, doc, &view);
        layers.art_dirty = false;
    }
    let selection = editor.selection.as_ref().map(|s| s.bounds());
    if view_changed
        || drawn.colors != colors.revision
        || drawn.preview != preview.revision
        || drawn.selection != selection
    {
        draw_overlay(&overlay, &view, &colors, &preview, selection, doc_size);
        drawn.preview = preview.revision;
        drawn.selection = selection;
    }
    drawn.view = view.revision;
    drawn.colors = colors.revision;
    drawn.doc_size = doc_size;
    drawn.background = Some(doc.background());
}

/// The checkerboard where the document is, anchored to its corner.
fn draw_checker(
    ctx: &Context2d,
    drawn: &mut Drawn,
    view: &View,
    colors: &ViewColors,
    doc: (u32, u32),
) {
    ctx.clear();
    let (Some(a), Some(b)) = (colors.checker_a, colors.checker_b) else {
        return;
    };
    let (dx, dy, dw, dh) = view.doc_rect(doc.0, doc.1);
    let (sw, sh) = (f64::from(view.stage.1.x), f64::from(view.stage.1.y));
    let x0 = dx.max(0.0);
    let y0 = dy.max(0.0);
    let x1 = (dx + dw).min(sw);
    let y1 = (dy + dh).min(sh);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // Two cells of the pattern repeat, so start on an even cell at or left
    // of the visible edge.
    let pair = 2.0 * CHECKER_CELL;
    let sx = dx + ((x0 - dx) / pair).floor() * pair;
    let sy = dy + ((y0 - dy) / pair).floor() * pair;
    let cols = ((x1 - sx) / CHECKER_CELL).ceil() as u32 + 2;
    let rows = ((y1 - sy) / CHECKER_CELL).ceil() as u32 + 2;
    let stale = match &drawn.checker {
        Some((_, c, r)) => *c < cols || *r < rows || drawn.checker_colors != colors.revision,
        None => true,
    };
    if stale {
        let (c, r) = (
            cols.max(16).next_multiple_of(16),
            rows.max(16).next_multiple_of(16),
        );
        let Ok(buf) = PixelBuffer::new(c, r) else {
            return;
        };
        let pa = [a.r, a.g, a.b, a.a];
        let pb = [b.r, b.g, b.b, b.a];
        let mut rgba = Vec::with_capacity((c * r * 4) as usize);
        for y in 0..r {
            for x in 0..c {
                rgba.extend_from_slice(if (x + y) % 2 == 0 { &pa } else { &pb });
            }
        }
        let _ = buf.write_rgba8(0, 0, c, r, &rgba);
        drawn.checker = Some((buf, c, r));
        drawn.checker_colors = colors.revision;
    }
    let Some((buf, c, r)) = &drawn.checker else {
        return;
    };
    ctx.set_image_smoothing(false);
    ctx.save();
    ctx.begin_path();
    ctx.rect(x0, y0, x1 - x0, y1 - y0);
    ctx.clip();
    ctx.draw_buffer_scaled(
        buf,
        sx,
        sy,
        f64::from(*c) * CHECKER_CELL,
        f64::from(*r) * CHECKER_CELL,
    );
    ctx.restore();
}

/// The pixel grid, the selection outline and the tool preview.
fn draw_overlay(
    ctx: &Context2d,
    view: &View,
    colors: &ViewColors,
    preview: &Preview,
    selection: Option<Rect>,
    doc: (u32, u32),
) {
    ctx.clear();
    let z = view.zoom;
    let (ox, oy) = view.origin;
    if !preview.pixels.is_empty() {
        let c = preview.color;
        ctx.set_fill_color(canvas_color(c));
        let visible = view.visible_pixels(doc.0, doc.1);
        for p in &preview.pixels {
            if visible.contains(p.x, p.y) {
                ctx.fill_rect(ox + f64::from(p.x) * z, oy + f64::from(p.y) * z, z, z);
            }
        }
    }
    if z >= GRID_FROM_ZOOM
        && let Some(grid) = colors.grid
    {
        let visible = view.visible_pixels(doc.0, doc.1);
        ctx.set_fill_color(canvas_color(grid));
        let top = oy + f64::from(visible.y) * z;
        let height = f64::from(visible.h) * z;
        for x in visible.x..=visible.right() {
            ctx.fill_rect(ox + f64::from(x) * z, top, 1.0, height);
        }
        let left = ox + f64::from(visible.x) * z;
        let width = f64::from(visible.w) * z;
        for y in visible.y..=visible.bottom() {
            ctx.fill_rect(left, oy + f64::from(y) * z, width, 1.0);
        }
    }
    if let (Some(r), Some(a), Some(b)) = (selection, colors.marquee_a, colors.marquee_b)
        && !r.is_empty()
    {
        let x = ox + f64::from(r.x) * z;
        let y = oy + f64::from(r.y) * z;
        let w = f64::from(r.w) * z;
        let h = f64::from(r.h) * z;
        ctx.set_line_width(1.0);
        ctx.set_stroke_color(canvas_color(b));
        ctx.stroke_rect(x - 0.5, y - 0.5, w + 1.0, h + 1.0);
        ctx.set_stroke_color(canvas_color(a));
        ctx.stroke_rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_core::{ColorMode, NewLayer, Placement};

    #[test]
    fn hex_colors_parse() {
        assert_eq!(parse_hex("#3c3c3c"), Some(Rgba::opaque(0x3c, 0x3c, 0x3c)));
        assert_eq!(parse_hex("#00000059"), Some(Rgba::new(0, 0, 0, 0x59)));
        assert_eq!(parse_hex("red"), None);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut v = View {
            stage: (Vec2::ZERO, Vec2::new(800.0, 600.0)),
            ..View::default()
        };
        v.fit(64, 64);
        let anchor = (300.0, 200.0);
        let before = v.to_doc(anchor);
        v.zoom_at(v.step(true), anchor);
        let after = v.to_doc(anchor);
        assert!((before.0 - after.0).abs() < 1.0 / v.zoom);
        assert!((before.1 - after.1).abs() < 1.0 / v.zoom);
    }

    #[test]
    fn plain_layers_go_to_the_canvas_one_buffer_each() {
        let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
        let top = doc
            .add_layer(NewLayer::Raster, "top", Placement::default())
            .unwrap();
        doc.update_layer(top, |p| p.blend = BlendMode::Multiply)
            .unwrap();
        let p = plan(&doc, 0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].source, ItemSource::Layer(top));
        assert_eq!(p[1].op, CompositeOperation::Multiply);
    }

    #[test]
    fn a_mode_the_canvas_lacks_flattens_what_is_below_it() {
        let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
        let bottom = doc.layers()[0].id;
        let mid = doc
            .add_layer(NewLayer::Raster, "mid", Placement::default())
            .unwrap();
        let top = doc
            .add_layer(NewLayer::Raster, "top", Placement::default())
            .unwrap();
        doc.update_layer(mid, |p| p.blend = BlendMode::Subtract)
            .unwrap();
        let p = plan(&doc, 0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].source, ItemSource::Flat(vec![bottom, mid]));
        assert_eq!(p[1].source, ItemSource::Layer(top));
    }

    #[test]
    fn past_the_buffer_budget_the_bottom_layers_flatten() {
        let mut doc = Document::new(64, 64, ColorMode::Rgba).unwrap();
        let ids: Vec<LayerId> = std::iter::once(doc.layers()[0].id)
            .chain((0..4).map(|_| {
                doc.add_layer(NewLayer::Raster, "l", Placement::default())
                    .unwrap()
            }))
            .collect();
        // Room for three buffers of 64x64.
        let p = plan_within(&doc, 0, 3 * 64 * 64);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].source, ItemSource::Flat(ids[..3].to_vec()));
        assert_eq!(p[2].source, ItemSource::Layer(ids[4]));
        assert_eq!(plan_within(&doc, 0, u64::MAX).len(), 5);
    }

    #[test]
    fn groups_composite_on_their_own() {
        let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
        let g = doc
            .add_layer(NewLayer::Group, "g", Placement::default())
            .unwrap();
        doc.add_layer(
            NewLayer::Raster,
            "in",
            Placement {
                parent: Some(g),
                index: None,
            },
        )
        .unwrap();
        let p = plan(&doc, 0);
        assert_eq!(p[1].source, ItemSource::Isolated(g));
    }
}
