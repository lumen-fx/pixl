//! The canvas view: the document drawn through Lumen's canvas, zoomed and
//! panned, over a checkerboard and under the pixel grid when it is on.
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

use lumenui::ecs::system::SystemParam;
use lumenui::glam::Vec2;
use lumenui::lumen_core::components::Transform;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{Canvas, CompositeOperation, Context2d, PixelBuffer};
use pixl_core::{
    Background, BlendMode, CompositeOptions, Document, LayerId, LayerKind, OnionSkin, Palette,
    Point, Raster, Rect, Rgba, Selection, TILE_SIZE,
};

use crate::editor::{Editor, Touched};
use crate::progressive::{self, Pool};
use crate::settings::{CanvasSettings, Look, PerformanceSettings, Settings};

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
    /// Degrees the view turns clockwise around the stage's middle.
    pub rotation: f64,
    /// The view is mirrored left to right (before it turns).
    pub flipped: bool,
    /// The document repeats left and right, and above and below.
    pub tiled: (bool, bool),
    /// What document positions snap to, when snapping is on.
    pub snap: Option<Snap>,
}

/// What positions snap to: grid lines and guides, as document pixel edges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snap {
    /// Grid cells: width, height and the offset of the first line.
    pub grid: Option<(u32, u32, i32, i32)>,
    /// Vertical lines (guides, slice edges) at these x edges.
    pub xs: Vec<i32>,
    /// Horizontal lines at these y edges.
    pub ys: Vec<i32>,
}

/// How near, in screen pixels, a position snaps to a line.
pub const SNAP_DISTANCE: f64 = 6.0;

impl Snap {
    /// `v` moved to the nearest line within `reach` along one axis.
    fn axis(v: f64, lines: &[i32], grid: Option<(u32, i32)>, reach: f64) -> f64 {
        let mut best: Option<f64> = None;
        let mut take = |t: f64| {
            if (v - t).abs() <= reach && best.is_none_or(|b| (v - t).abs() < (v - b).abs()) {
                best = Some(t);
            }
        };
        for l in lines {
            take(f64::from(*l));
        }
        if let Some((cell, off)) = grid
            && cell > 0
        {
            let c = f64::from(cell);
            let k = ((v - f64::from(off)) / c).round();
            take(f64::from(off) + k * c);
        }
        best.unwrap_or(v)
    }

    /// A document position moved onto the nearest line within `reach`.
    pub fn apply(&self, p: (f64, f64), reach: f64) -> (f64, f64) {
        let gx = self.grid.map(|(w, _, ox, _)| (w, ox));
        let gy = self.grid.map(|(_, h, _, oy)| (h, oy));
        (
            Self::axis(p.0, &self.xs, gx, reach),
            Self::axis(p.1, &self.ys, gy, reach),
        )
    }
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
            rotation: 0.0,
            flipped: false,
            tiled: (false, false),
            snap: None,
        }
    }
}

impl View {
    /// Whether the view turns or mirrors the document.
    pub fn turned(&self) -> bool {
        self.rotation.rem_euclid(360.0) != 0.0 || self.flipped
    }

    /// A stage position as it would be with no turn and no mirror: the
    /// frame the zoom and the origin are in.
    pub fn unturn(&self, stage: (f64, f64)) -> (f64, f64) {
        if !self.turned() {
            return stage;
        }
        let (cx, cy) = self.middle();
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (dx, dy) = (stage.0 - cx, stage.1 - cy);
        let (x, y) = (dx * c + dy * s, -dx * s + dy * c);
        let x = if self.flipped { -x } else { x };
        (cx + x, cy + y)
    }

    /// The canvas transform that turns and mirrors what the stage draws,
    /// from the unturned frame onto the stage.
    pub fn matrix(&self) -> [f64; 6] {
        let (cx, cy) = self.middle();
        let (s, c) = self.rotation.to_radians().sin_cos();
        let fx = if self.flipped { -1.0 } else { 1.0 };
        let (a, b, cc, d) = (c * fx, s * fx, -s, c);
        [a, b, cc, d, cx - (a * cx + cc * cy), cy - (b * cx + d * cy)]
    }

    /// Puts the view's turn and mirror on a canvas about to draw.
    pub fn apply_turn(&self, ctx: &Context2d) {
        if self.turned() {
            ctx.set_transform(self.matrix());
        }
    }

    /// The part of the unturned frame the stage shows: left, top, right,
    /// bottom.
    pub fn shown_frame(&self) -> (f64, f64, f64, f64) {
        let (w, h) = (f64::from(self.stage.1.x), f64::from(self.stage.1.y));
        let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|p| self.unturn(p));
        let x0 = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        let y0 = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
        let x1 = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let y1 = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::NEG_INFINITY, f64::max);
        (x0, y0, x1, y1)
    }
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
        let stage = self.unturn(stage);
        (
            (stage.0 - self.origin.0) / self.zoom,
            (stage.1 - self.origin.1) / self.zoom,
        )
    }

    /// The document pixel under a window position, moved onto a grid line
    /// or a guide when snapping is on and one is near.
    pub fn doc_pixel(&self, window: Vec2) -> Point {
        let (x, y) = self.to_doc(self.stage_point(window));
        let (x, y) = match &self.snap {
            Some(s) => {
                let (sx, sy) = s.apply((x, y), SNAP_DISTANCE / self.zoom);
                // A pixel snapped to an edge is the one on the pointer's
                // side of it.
                let side = |snapped: f64, raw: f64| {
                    if snapped != raw && raw < snapped {
                        snapped - 0.5
                    } else {
                        snapped
                    }
                };
                (side(sx, x), side(sy, y))
            }
            None => (x, y),
        };
        Point::new(x.floor() as i32, y.floor() as i32)
    }

    /// Zooms to `zoom`, keeping the document point under `anchor` (stage
    /// coordinates) where it is.
    pub fn zoom_at(&mut self, zoom: f64, anchor: (f64, f64)) {
        let zoom = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        let (dx, dy) = self.to_doc(anchor);
        let anchor = self.unturn(anchor);
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
        // A drag on a turned view moves the document the way the pointer
        // goes on screen.
        let (cx, cy) = self.middle();
        let (ux, uy) = self.unturn((cx + dx, cy + dy));
        let (dx, dy) = (ux - cx, uy - cy);
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
        let (fx0, fy0, fx1, fy1) = self.shown_frame();
        let doc = |x: f64, y: f64| {
            (
                (x - self.origin.0) / self.zoom,
                (y - self.origin.1) / self.zoom,
            )
        };
        let (x0, y0) = doc(fx0, fy0);
        let (x1, y1) = doc(fx1, fy1);
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
    /// The look and the canvas settings these were read under.
    read_for: Option<(Look, [Option<String>; 3])>,
    /// Ticks since the look changed; the cascade lands a tick or two later.
    settle: u32,
    pub revision: u64,
}

impl ViewColors {
    /// Reads the colors again, as after a change of look: a theme's tokens
    /// changed.
    pub fn reread(&mut self) {
        self.read_for = None;
    }

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

/// Re-reads the view colors after the look changes. A color the canvas
/// settings name takes the place of the theme's.
pub fn read_view_colors(look: Res<Look>, settings: Res<Settings>, mut colors: ResMut<ViewColors>) {
    let c = &settings.canvas;
    let overrides = [
        c.checker_a.clone(),
        c.checker_b.clone(),
        c.grid_color.clone(),
    ];
    let key = (*look, overrides);
    if colors.read_for.as_ref() != Some(&key) {
        colors.read_for = Some(key);
        colors.settle = 0;
    }
    // Read for a few ticks after a change, since the new classes take a
    // tick to land and another to cascade.
    if colors.settle > 4 && colors.complete() {
        return;
    }
    colors.settle += 1;
    let pick =
        |set: &Option<String>, id: &str| set.as_deref().and_then(parse_hex).or_else(|| probe(id));
    let read = ViewColors {
        checker_a: pick(&c.checker_a, "probe-checker-a"),
        checker_b: pick(&c.checker_b, "probe-checker-b"),
        grid: pick(&c.grid_color, "probe-grid"),
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
    /// The onion skin: the ghosts of the frames around the current one,
    /// composited by pixl-core, under or over the layers.
    Ghosts,
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
        // A skeleton paints nothing; its bones are an overlay.
        LayerKind::Reference { .. } | LayerKind::Skeleton { .. } => Class::Direct(op),
        LayerKind::Group { .. } => Class::Isolated(op),
    }
}

/// The layer buffer memory the view hands Lumen's renderer unless the
/// performance settings say otherwise, in MiB: eight buffers of 4096x4096.
/// Past it the bottom layers are flattened by pixl-core into one buffer,
/// which costs a CPU composite of that buffer on every stroke below the top
/// layers. Each buffer the renderer draws makes every frame dearer
/// (lumen-fx/lumen#535), so more is not better: on the view bench's 32
/// layers, eight buffers halve a bottom-layer stroke against two and keep a
/// pan frame within a 60 Hz frame on the GPU.
pub const DEFAULT_BUFFER_BUDGET_MIB: u64 = 512;

/// The buffer pixels `mib` of RGBA8 buffer memory holds.
pub fn budget_pixels(mib: u64) -> u64 {
    mib.saturating_mul(1024 * 1024) / 4
}

/// The buffers that show `frame` of `doc`, bottom first, within the
/// default budget.
pub fn plan(doc: &Document, frame: usize) -> Vec<PlanItem> {
    plan_within(doc, frame, budget_pixels(DEFAULT_BUFFER_BUDGET_MIB))
}

/// [`plan_within`] with the onion skin's buffer under or over the layers.
pub fn plan_with_onion(
    doc: &Document,
    frame: usize,
    budget: u64,
    onion: Option<&OnionSkin>,
) -> Vec<PlanItem> {
    let mut items = plan_within(doc, frame, budget);
    if let Some(onion) = onion {
        let ghosts = PlanItem {
            source: ItemSource::Ghosts,
            op: CompositeOperation::SourceOver,
            alpha: 1.0,
            visible: true,
        };
        if onion.above {
            items.push(ghosts);
        } else {
            items.insert(0, ghosts);
        }
    }
    items
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
        // A skeleton paints nothing; its bones are drawn by the rig.
        if matches!(layer.kind, LayerKind::Skeleton { .. }) {
            continue;
        }
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
/// showing only the item's layers. With the onion skin on, the flattened
/// layers leave the background out, as pixl-core does: the background goes
/// under the ghosts and the layers over them.
pub fn prepared(doc: &Document, source: &ItemSource, onion: bool) -> (Document, CompositeOptions) {
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
            opts.background = !onion;
        }
        ItemSource::Ghosts => opts.include_reference = false,
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
        LayerKind::Group { .. } | LayerKind::Skeleton { .. } => (None, None),
    }
}

fn subtree<'a>(layer: &'a pixl_core::Layer, out: &mut Vec<&'a pixl_core::Layer>) {
    out.push(layer);
    for c in layer.children() {
        subtree(c, out);
    }
}

/// The frames the onion skin ghosts around `frame`, farthest before first.
pub fn ghost_frames(doc: &Document, frame: usize, onion: &OnionSkin) -> Vec<usize> {
    let before = (1..=onion.before)
        .rev()
        .filter_map(|k| frame.checked_sub(k));
    let after = (1..=onion.after)
        .map(|k| frame + k)
        .filter(|&f| f < doc.frame_count());
    before.chain(after).collect()
}

/// What the ghost buffer shows: every non-reference layer's cels in the
/// ghost frames, in a fixed order, and what else changes their look.
fn shown_ghosts(doc: &Document, frame: usize, onion: &OnionSkin) -> Shown {
    let frames = ghost_frames(doc, frame, onion);
    let mut h = DefaultHasher::new();
    doc.width().hash(&mut h);
    doc.height().hash(&mut h);
    format!("{onion:?}").hash(&mut h);
    frames.hash(&mut h);
    if doc.mode() == pixl_core::ColorMode::Indexed {
        for c in doc.palette().colors() {
            c.hash(&mut h);
        }
        doc.transparent_index().hash(&mut h);
    }
    let walk = doc.walk();
    let layers: Vec<&pixl_core::Layer> = walk
        .iter()
        .map(|(_, l)| *l)
        .filter(|l| !matches!(l.kind, LayerKind::Reference { .. }))
        .collect();
    let mut images = Vec::new();
    for l in &layers {
        l.id.hash(&mut h);
        l.props.visible.hash(&mut h);
        l.props.clip.hash(&mut h);
        l.props.opacity.hash(&mut h);
        l.props.blend.code().hash(&mut h);
        if let LayerKind::Raster { cels, mask } = &l.kind {
            mask.map(|m| m.enabled).hash(&mut h);
            for &f in &frames {
                cels.get(f)
                    .copied()
                    .flatten()
                    .map(|c| c.opacity)
                    .hash(&mut h);
                let (img, mask) = layer_images(doc, l, f);
                images.push((l.id, img, mask));
            }
        }
    }
    Shown {
        images,
        signature: h.finish(),
    }
}

fn shown_for(
    doc: &Document,
    source: &ItemSource,
    frame: usize,
    onion: Option<&OnionSkin>,
) -> Shown {
    let roots: Vec<&pixl_core::Layer> = match source {
        ItemSource::Ghosts => {
            return match onion {
                Some(o) => shown_ghosts(doc, frame, o),
                None => Shown::default(),
            };
        }
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
            onion.is_some().hash(&mut h);
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

/// The ghost tiles to rewrite: where either version shows a ghost, all of
/// them when anything but the pixels changed, else where a pixel changed.
fn diff_ghosts(old: &Shown, now: &Shown, whole: bool, out: &mut BTreeSet<usize>) {
    if whole || old.images.len() != now.images.len() {
        for (_, img, _) in old.images.iter().chain(&now.images) {
            changed_tiles(img.as_ref(), None, out);
        }
        // A mask hides parts of a ghost, so its tiles hold ghost pixels only
        // where the cel does; the cel's tiles above cover them.
        return;
    }
    for ((_, a, ma), (_, b, mb)) in old.images.iter().zip(&now.images) {
        changed_tiles(a.as_ref(), b.as_ref(), out);
        changed_tiles(ma.as_ref(), mb.as_ref(), out);
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
    /// The canvas size the buffers were made for.
    seen_size: (u32, u32),
    /// Something drawn changed since the art canvas was last drawn.
    pub art_dirty: bool,
    /// Bumped each time the buffers' pixels or plan reach the art canvas.
    pub art_revision: u64,
    pub rewrites: Vec<Rewrite>,
    /// Set when a buffer could not be made, such as a document past the
    /// canvas size cap.
    pub error: Option<String>,
    /// The most buffer pixels the plan hands the renderer.
    pub budget: u64,
    /// Dirty tiles a tick rewrites itself; more go to the workers.
    pub sync_tiles: usize,
    /// The worker count the performance settings asked for; 0 is one per
    /// core.
    workers_asked: usize,
    /// The onion skin to show around the current frame, if any.
    pub onion: Option<OnionSkin>,
    seen_onion: Option<OnionSkin>,
    /// Tiles written into buffers since the view started, for the
    /// performance overlay to count per frame.
    pub tiles_written: u64,
    /// The plan must be made again: the buffer budget changed.
    replan: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            pool: Pool::new(),
            next_key: 1,
            seen_revision: 0,
            seen_generation: u64::MAX,
            seen_size: (0, 0),
            seen_frame: 0,
            art_dirty: true,
            art_revision: 0,
            rewrites: Vec::new(),
            error: None,
            budget: budget_pixels(DEFAULT_BUFFER_BUDGET_MIB),
            sync_tiles: 24,
            workers_asked: 0,
            onion: None,
            seen_onion: None,
            tiles_written: 0,
            replan: false,
        }
    }
}

impl Layers {
    /// Follows the performance settings: the buffer budget (a change
    /// re-plans the buffers), the progressive threshold and the worker
    /// count. The workers are replaced only while they are idle, so no
    /// rewrite in flight is lost.
    pub fn apply(&mut self, perf: &PerformanceSettings) {
        let budget = budget_pixels(perf.buffer_budget_mib);
        if budget != self.budget {
            self.budget = budget;
            // Re-plan on the next sync, whatever changed in the document.
            self.seen_revision = 0;
            self.replan = true;
        }
        self.sync_tiles = perf.progressive_tiles.max(1);
        if perf.worker_threads != self.workers_asked && self.pending() == 0 {
            self.workers_asked = perf.worker_threads;
            self.pool = if perf.worker_threads == 0 {
                Pool::new()
            } else {
                Pool::with_workers(perf.worker_threads)
            };
        }
    }

    /// The worker threads large redraws run on.
    pub fn workers(&self) -> usize {
        self.pool.worker_count()
    }

    /// Tiles waiting for a worker.
    pub fn queued(&self) -> usize {
        self.pool.queued()
    }

    /// The buffers the art canvas draws, and their memory in bytes.
    pub fn buffer_stats(&self) -> (usize, u64) {
        let bytes = self
            .items
            .iter()
            .map(|i| u64::from(i.buffer.width()) * u64::from(i.buffer.height()) * 4)
            .sum();
        (self.items.len(), bytes)
    }

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
    /// there. `touched` says what changed since the last call (see
    /// [`Editor::take_touched`]); with only some layers' pixels touched, only
    /// the buffers that show those layers are looked at.
    pub fn sync(&mut self, editor: &Editor, touched: &Touched, focus: (f64, f64)) {
        let doc = &editor.doc;
        let size = (doc.width(), doc.height());
        // A new document, or a new canvas size (crop, canvas size, resize),
        // needs buffers of its own size.
        if editor.generation != self.seen_generation || size != self.seen_size {
            for item in self.items.drain(..) {
                self.pool.cancel(item.key);
            }
            self.rewrites.clear();
            self.seen_generation = editor.generation;
            self.seen_size = size;
            self.seen_revision = 0;
            self.error = None;
        }
        let same_view = editor.frame == self.seen_frame && self.onion == self.seen_onion;
        if editor.revision == self.seen_revision && same_view {
            return;
        }
        self.seen_revision = editor.revision;
        self.seen_frame = editor.frame;
        self.seen_onion = self.onion.clone();
        let frame = editor.frame;
        let replan = std::mem::take(&mut self.replan);
        let pixels_only = match touched {
            Touched::Layers(ids) if same_view && !replan && !self.items.is_empty() => {
                Some(ids.as_slice())
            }
            _ => None,
        };
        if let Some(ids) = pixels_only {
            self.sync_pixels(doc, ids, frame);
            return;
        }
        let onion = self.onion.clone();
        let wanted = plan_with_onion(doc, frame, self.budget, onion.as_ref());

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
            // Its tiles will not arrive; they are not waited for.
            self.rewrites.retain(|r| r.item != gone.key);
            self.art_dirty = true;
        }
        self.items = items;

        // Rewrite what changed in each buffer, the onion skin's last so the
        // workers draw the frame itself first.
        let ghosts_last: Vec<usize> = (0..self.items.len())
            .filter(|&i| self.items[i].plan.source != ItemSource::Ghosts)
            .chain(
                (0..self.items.len()).filter(|&i| self.items[i].plan.source == ItemSource::Ghosts),
            )
            .collect();
        for i in ghosts_last {
            let now = shown_for(doc, &self.items[i].plan.source, frame, onion.as_ref());
            let item = &self.items[i];
            let mut tiles = BTreeSet::new();
            let ghosts = item.plan.source == ItemSource::Ghosts;
            let whole = now.signature != item.shown.signature
                && !(item.shown.images.is_empty()
                    && matches!(item.plan.source, ItemSource::Layer(_)));
            if ghosts {
                diff_ghosts(&item.shown, &now, whole, &mut tiles);
            } else if whole {
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
            self.items[i].shown = now;
            self.rewrite(doc, i, frame, tiles, focus);
        }
    }

    /// Looks only at the pixels of `layers`: the buffers that show them
    /// rewrite the tiles those layers changed.
    fn sync_pixels(&mut self, doc: &Document, layers: &[LayerId], frame: usize) {
        for i in 0..self.items.len() {
            let mut tiles = BTreeSet::new();
            if self.items[i].plan.source == ItemSource::Ghosts {
                // A stroke on a cel linked to a ghost frame shows there too.
                if let Some(onion) = &self.onion {
                    let now = shown_ghosts(doc, frame, onion);
                    diff_ghosts(&self.items[i].shown, &now, false, &mut tiles);
                    self.items[i].shown = now;
                }
                self.rewrite(doc, i, frame, tiles, (0.0, 0.0));
                continue;
            }
            for id in layers {
                let Some(entry) = self.items[i]
                    .shown
                    .images
                    .iter_mut()
                    .find(|(l, _, _)| l == id)
                else {
                    continue;
                };
                let Some(layer) = doc.layer(*id) else {
                    continue;
                };
                let (img, mask) = layer_images(doc, layer, frame);
                changed_tiles(entry.1.as_ref(), img.as_ref(), &mut tiles);
                changed_tiles(entry.2.as_ref(), mask.as_ref(), &mut tiles);
                entry.1 = img;
                entry.2 = mask;
            }
            // A stroke is near the view; nothing to order by.
            self.rewrite(doc, i, frame, tiles, (0.0, 0.0));
        }
    }

    /// Rewrites `tiles` of item `i`: a few on the spot, many on the workers.
    fn rewrite(
        &mut self,
        doc: &Document,
        i: usize,
        frame: usize,
        tiles: BTreeSet<usize>,
        focus: (f64, f64),
    ) {
        let cols = doc.width().div_ceil(TILE_SIZE as u32) as i32;
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
        if rects.is_empty() {
            return;
        }
        self.art_dirty = true;
        let source = self.items[i].plan.source.clone();
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
            ItemSource::Ghosts => progressive::Source::Ghosts {
                doc: doc.clone(),
                frame,
                onion: self.onion.clone().unwrap_or_default(),
            },
            other => {
                let (prepared, opts) = prepared(doc, other, self.onion.is_some());
                progressive::Source::Composite {
                    doc: prepared,
                    frame,
                    opts,
                }
            }
        };
        let item = &mut self.items[i];
        if rects.len() <= self.sync_tiles {
            // Render the tiles side by side, then write them in order.
            let stamp = self.pool.stamp();
            let rendered: Vec<Vec<u8>> = if rects.len() > 1 {
                std::thread::scope(|s| {
                    let work = &work;
                    let jobs: Vec<_> = rects
                        .iter()
                        .map(|r| s.spawn(move || work.render(*r)))
                        .collect();
                    jobs.into_iter()
                        .map(|j| j.join().expect("a tile renders"))
                        .collect()
                })
            } else {
                rects.iter().map(|r| work.render(*r)).collect()
            };
            for (r, rgba) in rects.iter().zip(rendered) {
                write_tile(&item.buffer, &mut item.applied, cols, *r, stamp, &rgba);
            }
            self.tiles_written += rects.len() as u64;
        } else {
            let mut rects = rects;
            let (fx, fy) = focus;
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
                self.tiles_written += 1;
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
        view.apply_turn(ctx);
        // Tiled mode draws the copies around the document as well.
        let (tx, ty) = view.tiled;
        let (w, h) = (
            f64::from(doc.width()) * view.zoom,
            f64::from(doc.height()) * view.zoom,
        );
        let xs: &[f64] = if tx { &[-1.0, 0.0, 1.0] } else { &[0.0] };
        let ys: &[f64] = if ty { &[-1.0, 0.0, 1.0] } else { &[0.0] };
        for &j in ys {
            for &i in xs {
                if i == 0.0 && j == 0.0 {
                    self.draw_document(ctx, doc, view, true);
                } else {
                    let mut copy = view.clone();
                    copy.origin = (view.origin.0 + i * w, view.origin.1 + j * h);
                    self.draw_document(ctx, doc, &copy, false);
                }
            }
        }
    }

    /// Draws the buffers onto `ctx` where `view` places the document, with
    /// the onion skin or without it. The art canvas and the preview both
    /// draw this way, so the preview costs no compositing of its own.
    pub fn draw_document(&self, ctx: &Context2d, doc: &Document, view: &View, ghosts: bool) {
        ctx.set_image_smoothing(false);
        let (x, y, w, h) = view.doc_rect(doc.width(), doc.height());
        // The flattened buffer holds the background, unless the onion skin
        // goes between the two.
        let flattened = self
            .items
            .iter()
            .any(|i| matches!(i.plan.source, ItemSource::Flat(_)));
        if (!flattened || self.seen_onion.is_some())
            && let Background::Color(c) = doc.background()
        {
            ctx.set_fill_color(canvas_color(c));
            ctx.fill_rect(x, y, w, h);
        }
        for item in self.items.iter().filter(|i| {
            i.plan.visible && i.plan.alpha > 0.0 && (ghosts || i.plan.source != ItemSource::Ghosts)
        }) {
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
    selection: Option<Selection>,
    /// The view aids' revision the overlay last drew.
    aids: u64,
    checker: Option<(PixelBuffer, u32, u32)>,
    checker_colors: u64,
    doc_size: (u32, u32),
    background: Option<Background>,
    /// The canvas settings the checkerboard, the grid and the stage were
    /// drawn with.
    canvas: Option<CanvasSettings>,
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
    mut editor: ResMut<Editor>,
    view: Res<View>,
    settings: Res<Settings>,
    mut layers: ResMut<Layers>,
    wake: Option<Res<WakeDeadline>>,
) {
    layers.apply(&settings.performance);
    crate::jobs::configure(&settings.performance);
    let mid = view.to_doc(view.middle());
    let touched = if editor.revision == layers.seen_revision {
        Touched::Layers(Vec::new())
    } else {
        editor.take_touched()
    };
    layers.sync(&editor, &touched, mid);
    layers.drain(editor.doc.width());
    if layers.pending() > 0
        && let Some(wake) = wake
    {
        // Keep ticking while workers are rendering, so each finished tile
        // shows on the next frame.
        wake.request(Instant::now() + Duration::from_millis(4));
    }
}

/// What the stage draws over the document: the theme's colors, the tool
/// preview and the view aids.
#[derive(SystemParam)]
pub struct OverStage<'w> {
    colors: Res<'w, ViewColors>,
    preview: Res<'w, Preview>,
    aids: Res<'w, crate::aids::Aids>,
}

/// Draws the three stage canvases where something they show changed.
pub fn draw_stage(
    editor: Res<Editor>,
    view: Res<View>,
    over: OverStage,
    settings: Res<Settings>,
    mut layers: ResMut<Layers>,
    mut drawn: ResMut<Drawn>,
) {
    let OverStage {
        colors,
        preview,
        aids,
    } = over;
    let size = (view.stage.1.x, view.stage.1.y);
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return;
    }
    let canvas = &settings.canvas;
    let canvas_changed = drawn.canvas.as_ref() != Some(canvas);
    if canvas_changed {
        // The color around the document is the user's choice, so it rides
        // on the element; with none the theme's shows.
        if let Some(stage) = dom::get_by_id("stage") {
            match canvas.background.as_deref().and_then(parse_hex) {
                Some(c) => {
                    stage.set_style("bg", &crate::tools::hex(c));
                }
                None => {
                    stage.remove_style("bg");
                }
            }
        }
        // A new cell size needs a new pattern.
        if drawn.canvas.as_ref().map(|c| c.checker_size) != Some(canvas.checker_size) {
            drawn.checker = None;
        }
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
    let view_changed =
        drawn.view != view.revision || resized || drawn.doc_size != doc_size || canvas_changed;
    let cell = f64::from(canvas.checker_size.max(1));
    // The pixel grid is off until View > Pixel grid turns it on.
    let grid_from = canvas.pixel_grid.then_some(canvas.grid_from_zoom / 100.0);

    if view_changed || drawn.colors != colors.revision || drawn.background != Some(doc.background())
    {
        draw_checker(&checker, &mut drawn, &view, &colors, doc_size, cell);
    }
    if view_changed || layers.art_dirty {
        if layers.art_dirty {
            layers.art_revision += 1;
        }
        layers.draw(&art, doc, &view);
        layers.art_dirty = false;
    }
    if view_changed
        || drawn.colors != colors.revision
        || drawn.preview != preview.revision
        || drawn.selection != editor.selection
        || drawn.aids != aids.revision
    {
        draw_overlay(
            &overlay,
            &view,
            &colors,
            &preview,
            editor.selection.as_ref(),
            doc_size,
            grid_from,
        );
        crate::aids::draw(&overlay, &view, &aids, &editor);
        drawn.preview = preview.revision;
        drawn.selection = editor.selection.clone();
        drawn.aids = aids.revision;
    }
    drawn.view = view.revision;
    drawn.colors = colors.revision;
    drawn.doc_size = doc_size;
    drawn.background = Some(doc.background());
    if canvas_changed {
        drawn.canvas = Some(canvas.clone());
    }
}

/// The checkerboard where the document is, anchored to its corner.
fn draw_checker(
    ctx: &Context2d,
    drawn: &mut Drawn,
    view: &View,
    colors: &ViewColors,
    doc: (u32, u32),
    cell: f64,
) {
    ctx.clear();
    let (Some(a), Some(b)) = (colors.checker_a, colors.checker_b) else {
        return;
    };
    view.apply_turn(ctx);
    let (mut dx, mut dy, mut dw, mut dh) = view.doc_rect(doc.0, doc.1);
    // Tiled copies have the checkerboard under them too.
    if view.tiled.0 {
        dx -= dw;
        dw *= 3.0;
    }
    if view.tiled.1 {
        dy -= dh;
        dh *= 3.0;
    }
    let (fx0, fy0, fx1, fy1) = view.shown_frame();
    let x0 = dx.max(fx0);
    let y0 = dy.max(fy0);
    let x1 = (dx + dw).min(fx1);
    let y1 = (dy + dh).min(fy1);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // Two cells of the pattern repeat, so start on an even cell at or left
    // of the visible edge.
    let pair = 2.0 * cell;
    let sx = dx + ((x0 - dx) / pair).floor() * pair;
    let sy = dy + ((y0 - dy) / pair).floor() * pair;
    let cols = ((x1 - sx) / cell).ceil() as u32 + 2;
    let rows = ((y1 - sy) / cell).ceil() as u32 + 2;
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
    ctx.draw_buffer_scaled(buf, sx, sy, f64::from(*c) * cell, f64::from(*r) * cell);
    ctx.restore();
}

/// The pixel grid, from the zoom `grid_from` on while it is on, the
/// selection outline and the tool preview.
fn draw_overlay(
    ctx: &Context2d,
    view: &View,
    colors: &ViewColors,
    preview: &Preview,
    selection: Option<&Selection>,
    doc: (u32, u32),
    grid_from: Option<f64>,
) {
    ctx.clear();
    view.apply_turn(ctx);
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
    if grid_from.is_some_and(|from| z >= from - 1e-9)
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
    let (Some(sel), Some(a), Some(b)) = (selection, colors.marquee_a, colors.marquee_b) else {
        return;
    };
    let r = sel.bounds();
    if r.is_empty() {
        return;
    }
    let shown = r.intersect(&view.visible_pixels(doc.0, doc.1));
    // Past this many pixels the outline is the bounding box: tracing every
    // edge of a huge selection on each redraw costs more than it shows.
    const TRACE_MOST: i64 = 1 << 20;
    if shown.area() > TRACE_MOST || shown.is_empty() {
        let x = ox + f64::from(r.x) * z;
        let y = oy + f64::from(r.y) * z;
        let w = f64::from(r.w) * z;
        let h = f64::from(r.h) * z;
        ctx.set_line_width(1.0);
        ctx.set_stroke_color(canvas_color(b));
        ctx.stroke_rect(x - 0.5, y - 0.5, w + 1.0, h + 1.0);
        ctx.set_stroke_color(canvas_color(a));
        ctx.stroke_rect(x + 0.5, y + 0.5, w - 1.0, h - 1.0);
        return;
    }
    // Every edge between a selected and an unselected pixel, as runs.
    let edges = selection_edges(sel, shown);
    for (color, inset) in [(b, -1.0), (a, 0.0)] {
        ctx.set_fill_color(canvas_color(color));
        for &(x, y, len, horizontal) in &edges {
            let (px, py) = (ox + f64::from(x) * z, oy + f64::from(y) * z);
            if horizontal {
                ctx.fill_rect(px, py + inset, f64::from(len) * z, 1.0);
            } else {
                ctx.fill_rect(px + inset, py, 1.0, f64::from(len) * z);
            }
        }
    }
}

/// The edges of a selection inside `area`, merged into runs: `(x, y, len,
/// horizontal)` in pixel edge coordinates.
pub fn selection_edges(sel: &Selection, area: Rect) -> Vec<(i32, i32, i32, bool)> {
    let mut out = Vec::new();
    let at = |x: i32, y: i32| sel.contains(x, y);
    // Horizontal edges: above row y, between y - 1 and y.
    for y in area.y..=area.bottom() {
        let mut start: Option<i32> = None;
        for x in area.x..=area.right() {
            let edge = x < area.right() && at(x, y) != at(x, y - 1);
            match (start, edge) {
                (None, true) => start = Some(x),
                (Some(s), false) => {
                    out.push((s, y, x - s, true));
                    start = None;
                }
                _ => {}
            }
        }
    }
    for x in area.x..=area.right() {
        let mut start: Option<i32> = None;
        for y in area.y..=area.bottom() {
            let edge = y < area.bottom() && at(x, y) != at(x - 1, y);
            match (start, edge) {
                (None, true) => start = Some(y),
                (Some(s), false) => {
                    out.push((x, s, y - s, false));
                    start = None;
                }
                _ => {}
            }
        }
    }
    out
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
