//! The tools beyond the pencil, the eraser, the plain shapes, the bucket,
//! the rectangle selection and move: the soft brush, the spray, shading and
//! dither brushes, the clone and pattern stamps, smudge, blur and sharpen,
//! the replace color brush, curves, polygons and contours, gradients, the
//! ellipse, lasso, polygon lasso and magic wand selections, crop, text,
//! measure and slices. Each tool's work is done by pixl-paint; this module
//! turns pointer input into its calls, and keeps the options bar in step.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lumenui::ecs::system::SystemParam;
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{PointerButton, PointerData, PointerState};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_document::history::Transaction;
use pixl_document::shapes;
use pixl_document::transform::Pixels;
use pixl_document::{
    Bayer, Color, CompositeOptions, Point, Raster, Rect, Rgba, SelectOp, Selection, Target,
    composite_frame,
};
use pixl_paint::filters::read_values;
use pixl_paint::tools::brushes::{Rng, SmudgeStroke, SoftBrush, SoftInk, SoftStroke, Stabilizer};
use pixl_paint::tools::fill::{self, Connectivity, FillOptions, Sampler};
use pixl_paint::tools::gradient::{self, DitherPattern, Gradient, GradientShape};
use pixl_paint::tools::paint::PencilStroke;
use pixl_paint::tools::text::BitmapFont;
use pixl_paint::{Brush, Canvas, Ink, SymmetryMode, ToolContext};

use crate::editor::{BrushShape, Editor, Tool};
use crate::ids::ElementIds;
use crate::settings::Settings;
use crate::tools::dynamics::Live;
use crate::tools::{Place, context, paint_color, refuse};
use crate::view::Preview;
use crate::view::input::{Pointer, PointerInput};
use crate::view::jobs::{PaintJob, WRITE_BUDGET, tiles_of};

/// Whether the size slider sizes this tool's brush.
pub fn sized(tool: Tool) -> bool {
    use Tool::*;
    matches!(
        tool,
        Brush
            | Spray
            | Shade
            | DitherBrush
            | CloneStamp
            | PatternStamp
            | Smudge
            | Blur
            | ReplaceColor
            | Contour
    )
}

/// The freehand tools with a pixel-perfect choice of their own.
pub fn pixel_perfect_tool(tool: Tool) -> bool {
    use Tool::*;
    matches!(
        tool,
        Shade | DitherBrush | CloneStamp | PatternStamp | ReplaceColor | Line | Curve | Contour
    )
}

/// The freehand painting tools, which take the stabilizer.
fn freehand(tool: Tool) -> bool {
    use Tool::*;
    matches!(
        tool,
        Pencil
            | Eraser
            | Brush
            | Spray
            | Shade
            | DitherBrush
            | CloneStamp
            | PatternStamp
            | Smudge
            | Blur
            | ReplaceColor
            | Contour
    )
}

/// The painting tools, which honor symmetry.
pub fn paints(tool: Tool) -> bool {
    freehand(tool)
        || matches!(
            tool,
            Tool::Line
                | Tool::Curve
                | Tool::Rectangle
                | Tool::Ellipse
                | Tool::Polygon
                | Tool::Bucket
                | Tool::Gradient
                | Tool::Text
        )
}

fn selects(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Select | Tool::EllipseSelect | Tool::Lasso | Tool::PolygonLasso | Tool::Wand
    )
}

/// The line the options bar shows to explain a tool.
pub fn hint(tool: Tool) -> &'static str {
    use Tool::*;
    match tool {
        Brush => "Soft edges by opacity, or dithered whole pixels",
        Spray => "Hold to keep spraying",
        Shade => "Steps pixels along the palette; right-click steps back",
        DitherBrush => "Dithers the foreground over the background",
        CloneStamp => "Alt+click picks where to copy from",
        PatternStamp => "Paints the pattern, anchored to the canvas",
        Smudge => "Drags the pixels under the brush",
        Blur => "Softens edges; Sharpen hardens them",
        ReplaceColor => "Paints the foreground over the background color",
        Curve => "Drag the ends, then click twice to bend",
        Polygon => "Click the corners; double-click or Enter closes",
        Contour => "Draw a closed shape; it fills on release",
        Gradient => "Drag from the start to the end",
        EllipseSelect | Lasso => "Shift adds, Alt subtracts, both intersect",
        PolygonLasso => "Click the corners; double-click or Enter closes",
        Wand => "Click a color; Shift adds, Alt subtracts",
        Crop => "Drag the area to keep",
        Text => "Click where the text starts",
        Measure => "Drag to measure",
        Slice => "Drag a new slice; click one to edit it",
        _ => "",
    }
}

/// The options of the toolbox's tools, as the options bar sets them.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxOptions {
    /// Brush opacity in percent.
    pub opacity: u8,
    /// Brush hardness in percent.
    pub hardness: u8,
    pub dithered: bool,
    pub spray_radius: u32,
    /// Pixels each spray dab scatters.
    pub spray_density: u32,
    /// Dither brush amount in percent.
    pub dither_amount: u8,
    pub dither_matrix: Bayer,
    /// Smudge and blur strength in percent.
    pub strength: u8,
    pub keep_colors: bool,
    pub sharpen: bool,
    /// The magic wand samples every layer.
    pub merged: bool,
    /// The magic wand grows across diagonals.
    pub eight: bool,
    pub select_op: SelectOp,
    pub gradient_shape: GradientShape,
    /// The gradient's dither by its menu name.
    pub gradient_dither: String,
    /// The gradient runs over the palette between the two colors.
    pub gradient_palette: bool,
    pub gradient_repeat: bool,
    pub text: String,
    pub text_font: String,
    pub text_scale: u32,
    /// Stabilizer string length in pixels; 0 is off.
    pub stabilizer: u32,
}

impl Default for BoxOptions {
    fn default() -> Self {
        Self {
            opacity: 100,
            hardness: 50,
            dithered: false,
            spray_radius: 8,
            spray_density: 12,
            dither_amount: 50,
            dither_matrix: Bayer::B4,
            strength: 50,
            keep_colors: false,
            sharpen: false,
            merged: false,
            eight: false,
            select_op: SelectOp::Replace,
            gradient_shape: GradientShape::Linear,
            gradient_dither: "Bayer 4x4".to_string(),
            gradient_palette: false,
            gradient_repeat: false,
            text: "pixl".to_string(),
            text_font: "pixl 5x7".to_string(),
            text_scale: 1,
            stabilizer: 0,
        }
    }
}

/// The toolbox's state: its options, the gesture in progress, and what
/// tools keep between gestures.
#[derive(Resource)]
pub struct Toolbox {
    pub options: BoxOptions,
    session: Option<Session>,
    /// Where the clone stamp copies from, set by Alt+click.
    pub clone_source: Option<Point>,
    /// The pattern stamp's pattern, raw values in the document's format,
    /// and where it is anchored.
    pub pattern: Option<(Arc<Pixels>, Point)>,
    /// Pixel-perfect per tool, as each tool last had it.
    pixel_perfect: HashMap<Tool, bool>,
    /// The fonts the text tool can set, pixl's own first.
    pub fonts: Vec<BitmapFont>,
    /// The last press: when, where, so a second one makes a double-click.
    last_press: Option<(Instant, Point)>,
    /// When the spray last sprayed while held still.
    last_spray: Option<Instant>,
    /// A slice the slice tool clicked, for its dialog to open.
    pub edit_slice: Option<usize>,
    /// The theme's color for outlines that are not paint: selections being
    /// drawn, crops, slices and measures.
    accent: Rgba,
    /// A gradient still being painted on worker threads.
    fill: Option<PaintJob>,
    /// Whether the preview shows the text tool's text under the pointer.
    text_hover: bool,
}

/// What the tools keep of one document between gestures.
#[derive(Default)]
pub struct ToolboxDocument {
    clone_source: Option<Point>,
    pattern: Option<(Arc<Pixels>, Point)>,
}

impl Default for Toolbox {
    fn default() -> Self {
        Self {
            options: BoxOptions::default(),
            session: None,
            clone_source: None,
            pattern: None,
            pixel_perfect: HashMap::new(),
            fonts: vec![BitmapFont::builtin()],
            last_press: None,
            last_spray: None,
            edit_slice: None,
            accent: Rgba::opaque(0x31, 0xa2, 0xf2),
            fill: None,
            text_hover: false,
        }
    }
}

impl Toolbox {
    /// Hands over what the tools keep of the document in front: the clone
    /// stamp's source and the pattern stamp's pattern.
    pub fn take_document(&mut self) -> ToolboxDocument {
        self.last_press = None;
        self.edit_slice = None;
        ToolboxDocument {
            clone_source: self.clone_source.take(),
            pattern: self.pattern.take(),
        }
    }

    /// Puts back what the tools kept of the document coming to the front.
    pub fn restore_document(&mut self, saved: ToolboxDocument) {
        self.clone_source = saved.clone_source;
        self.pattern = saved.pattern;
    }

    /// Whether a gesture of the toolbox is in progress, or the paint it
    /// left is still being drawn.
    pub fn active(&self) -> bool {
        self.session.is_some() || self.fill.is_some()
    }

    /// Whether a gradient is still being painted on worker threads.
    pub fn painting(&self) -> bool {
        self.fill.is_some()
    }

    /// The font the text tool sets.
    pub fn font(&self) -> &BitmapFont {
        self.fonts
            .iter()
            .find(|f| f.name == self.options.text_font)
            .unwrap_or(&self.fonts[0])
    }
}

/// The largest brush the size keys step to: the size slider's own most.
const MAX_SIZE: u32 = 32;

/// How long two presses may be apart to make a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(450);

/// How often a held spray sprays without moving.
const SPRAY_EVERY: Duration = Duration::from_millis(40);

/// A gesture of the toolbox.
enum Session {
    /// A freehand stroke.
    Stroke(Box<StrokeSession>),
    /// Two ends dragged, then the two control points clicked.
    Curve {
        button: PointerButton,
        ends: (Point, Point),
        /// The control points clicked so far.
        controls: Vec<Point>,
        dragging: bool,
        hover: Point,
    },
    /// Corners clicked one by one: a polygon, or a polygon selection.
    Corners {
        button: PointerButton,
        select: Option<(SelectOp, Option<Selection>)>,
        points: Vec<Point>,
        hover: Point,
    },
    /// A freehand path: a contour fill or a lasso selection.
    Path {
        button: PointerButton,
        select: Option<(SelectOp, Option<Selection>)>,
        points: Vec<Point>,
    },
    /// A drag from one point to another: gradient, ellipse selection, crop,
    /// measure, slice.
    Drag {
        button: PointerButton,
        tool: Tool,
        from: Point,
        to: Point,
        select: Option<(SelectOp, Option<Selection>)>,
    },
}

struct StrokeSession {
    button: PointerButton,
    ctx: ToolContext,
    tx: Option<Transaction>,
    stroke: Stroke,
    stabilizer: Stabilizer,
    at: Point,
    /// The stroke's dynamics, when something follows an input.
    live: Option<Live>,
}

enum Stroke {
    Soft(SoftStroke),
    Inked(PencilStroke),
    Smudge(SmudgeStroke),
    Spray { ink: Ink, rng: Rng },
}

/// The selection operation a press makes: Shift adds, Alt subtracts, both
/// intersect, and otherwise the options bar's choice.
fn select_op(options: &BoxOptions, shift: bool, alt: bool) -> SelectOp {
    match (shift, alt) {
        (true, true) => SelectOp::Intersect,
        (true, false) => SelectOp::Add,
        (false, true) => SelectOp::Subtract,
        _ => options.select_op,
    }
}

/// `shape` combined with what was selected before.
fn combined(before: Option<&Selection>, op: SelectOp, shape: Selection) -> Option<Selection> {
    let out = match (op, before) {
        (SelectOp::Replace, _) | (_, None) if op != SelectOp::Intersect => shape,
        (_, None) => return None,
        (_, Some(prev)) => {
            let mut s = prev.clone();
            s.combine(&shape, op);
            s
        }
    };
    (!out.is_empty()).then_some(out)
}

/// The colors a shading stroke steps through: the palette's colors in
/// order, without its clear entries.
fn shade_ramp(editor: &Editor) -> Vec<Color> {
    let indexed = editor.doc.mode() == pixl_document::ColorMode::Indexed;
    editor
        .doc
        .palette()
        .colors()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.a > 0)
        .map(|(i, c)| {
            if indexed {
                Color::Index(i as u8)
            } else {
                Color::Rgba(*c)
            }
        })
        .collect()
}

/// The raw value the active cel stores for a clear pixel.
fn clear_raw(editor: &Editor) -> u32 {
    match editor.doc.mode() {
        pixl_document::ColorMode::Indexed => u32::from(editor.doc.transparent_index()),
        pixl_document::ColorMode::Rgba => 0,
    }
}

/// The active cel's pixels as they are, or an empty image of the canvas.
fn snapshot(editor: &Editor) -> Raster {
    match editor.doc.cel_raster(editor.layer, editor.frame) {
        Ok(Some(r)) => r.clone(),
        _ => Raster::empty(
            editor.doc.mode(),
            editor.doc.width(),
            editor.doc.height(),
            editor.doc.transparent_index(),
        ),
    }
}

/// The pattern the pattern stamp paints: the defined one, or a checker of
/// the foreground color and clear pixels.
fn pattern_of(editor: &Editor, toolbox: &Toolbox, button: PointerButton) -> (Arc<Pixels>, Point) {
    if let Some(p) = &toolbox.pattern {
        return p.clone();
    }
    let ctx = ToolContext::default();
    let mut raster = Raster::empty(editor.doc.mode(), 1, 1, editor.doc.transparent_index());
    let palette = editor.doc.palette().clone();
    let target = pixl_document::Target {
        raster: &mut raster,
        palette: &palette,
        transparent_index: editor.doc.transparent_index(),
        alpha_locked: false,
    };
    let canvas = Canvas::new(target, &ctx);
    let ink = canvas.resolve(Color::Rgba(paint_color(editor, button)));
    let clear = canvas.transparent_raw();
    let px = Pixels {
        width: 2,
        height: 2,
        data: vec![ink, clear, clear, ink],
    };
    (Arc::new(px), Point::new(0, 0))
}

/// Starts a stroke session: a transaction on the active cel and the
/// stroke the tool lays down.
fn begin_stroke(
    editor: &mut Editor,
    toolbox: &mut Toolbox,
    tool: Tool,
    press: &Press,
    settings: &Settings,
) -> Option<Session> {
    let (button, p, alt) = (press.button, press.p, press.alt);
    if tool == Tool::CloneStamp && alt {
        toolbox.clone_source = Some(p);
        editor.message = Some(format!("Clone source set at {}, {}", p.x, p.y));
        return None;
    }
    let o = toolbox.options.clone();
    let color = paint_color(editor, button);
    let other = if button == PointerButton::Secondary {
        editor.fg
    } else {
        editor.bg
    };
    let brush = editor.brush(editor.options.size);
    let pixel_perfect = editor.options.pixel_perfect;
    let ink = match tool {
        Tool::Shade => Some(Ink::Shade {
            ramp: shade_ramp(editor),
            step: if button == PointerButton::Secondary {
                -1
            } else {
                1
            },
        }),
        Tool::DitherBrush => Some(Ink::Dither {
            first: Color::Rgba(color),
            second: Some(Color::Rgba(other)),
            pattern: DitherPattern::Bayer(o.dither_matrix),
            amount: (u32::from(o.dither_amount) * 255 / 100) as u8,
        }),
        Tool::PatternStamp => {
            let (pattern, origin) = pattern_of(editor, toolbox, button);
            Some(Ink::Pattern {
                pattern,
                origin,
                skip: Some(clear_raw(editor)),
            })
        }
        Tool::CloneStamp => {
            let Some(src) = toolbox.clone_source else {
                editor.message = Some("Alt+click to pick where the clone stamp copies from".into());
                return None;
            };
            Some(Ink::Clone {
                source: Arc::new(snapshot(editor)),
                offset: Point::new(src.x - p.x, src.y - p.y),
            })
        }
        Tool::Blur => Some(Ink::Blur {
            source: Arc::new(snapshot(editor)),
            sharpen: o.sharpen,
            amount: (u32::from(o.strength) * 255 / 100) as u8,
            keep_colors: o.keep_colors,
        }),
        Tool::ReplaceColor => Some(Ink::Replace {
            from: Color::Rgba(other),
            to: Color::Rgba(color),
            tolerance: editor.options.tolerance,
        }),
        _ => None,
    };
    let ctx = context(editor);
    let (_, tx) = match editor.begin_paint(tool.label()) {
        Ok(started) => started,
        Err(e) => {
            refuse(editor, e);
            return None;
        }
    };
    let target = editor.target().ok()?;
    let canvas = Canvas::new(target, &ctx);
    let stroke = match tool {
        Tool::Brush => Stroke::Soft(SoftStroke::new(SoftBrush {
            size: editor_size(&brush),
            opacity: (u32::from(o.opacity) * 255 / 100) as u8,
            hardness: (u32::from(o.hardness) * 255 / 100) as u8,
            dithered: o.dithered,
            ink: SoftInk::Paint(color),
        })),
        Tool::Smudge => Stroke::Smudge(SmudgeStroke::new(
            &canvas,
            brush,
            (u32::from(o.strength) * 255 / 100) as u8,
            o.keep_colors,
        )),
        Tool::Spray => Stroke::Spray {
            ink: Ink::Paint(Color::Rgba(color)),
            rng: Rng::new(0x9e37_79b9 ^ (p.x as u32).wrapping_mul(31) ^ p.y as u32),
        },
        _ => Stroke::Inked(PencilStroke::new(
            &canvas,
            brush,
            ink.expect("every inked tool has an ink"),
            pixel_perfect,
        )),
    };
    let live = crate::tools::dynamics::live_for(editor, toolbox, settings, tool, button);
    let mut session = StrokeSession {
        button,
        ctx,
        tx: Some(tx),
        stroke,
        stabilizer: Stabilizer::new(f64::from(o.stabilizer)),
        at: p,
        live,
    };
    stroke_to(
        editor,
        toolbox,
        &mut session,
        p,
        Some((press.at, &press.data)),
    );
    Some(Session::Stroke(Box::new(session)))
}

/// The size of a brush for the soft brush, which draws its own round dab.
fn editor_size(brush: &Brush) -> u32 {
    brush.size.max(1)
}

/// Moves a stroke to `p`, through the stabilizer, with the pointer event
/// that brought it there (none for a spray held still).
fn stroke_to(
    editor: &mut Editor,
    toolbox: &mut Toolbox,
    s: &mut StrokeSession,
    p: Point,
    event: Option<(Vec2, &PointerData)>,
) {
    if let (Some(live), Some(event)) = (&mut s.live, event) {
        let stroke = &mut s.stroke;
        crate::tools::dynamics::follow(editor, live, event, |brush, _| match stroke {
            Stroke::Soft(st) => st.set_size(brush.size),
            Stroke::Inked(st) => st.set_brush(brush),
            Stroke::Smudge(st) => st.set_brush(brush),
            Stroke::Spray { .. } => {}
        });
    }
    let (x, y) = s
        .stabilizer
        .feed(f64::from(p.x) + 0.5, f64::from(p.y) + 0.5);
    let p = Point::new(x.floor() as i32, y.floor() as i32);
    s.at = p;
    let (radius, density) = (toolbox.options.spray_radius, toolbox.options.spray_density);
    let Ok(target) = editor.target() else {
        return;
    };
    let mut canvas = Canvas::new(target, &s.ctx);
    match &mut s.stroke {
        Stroke::Soft(stroke) => stroke.add(&mut canvas, p),
        Stroke::Inked(stroke) => stroke.add(&mut canvas, p),
        Stroke::Smudge(stroke) => stroke.add(&mut canvas, p),
        Stroke::Spray { ink, rng } => {
            pixl_paint::tools::brushes::spray(&mut canvas, p, radius, density, ink, rng);
            toolbox.last_spray = Some(Instant::now());
        }
    }
    if !canvas.dirty().is_empty() {
        editor.touch_pixels();
    }
}

fn end_stroke(editor: &mut Editor, s: StrokeSession) {
    let StrokeSession {
        ctx, tx, stroke, ..
    } = s;
    if let Stroke::Inked(stroke) = stroke
        && let Ok(target) = editor.target()
    {
        let mut canvas = Canvas::new(target, &ctx);
        stroke.finish(&mut canvas);
    }
    if let Some(tx) = tx {
        editor.history.commit(&mut editor.doc, tx);
        editor.touch_pixels();
    }
}

/// The ink a shape tool paints with.
fn shape_ink(editor: &Editor, button: PointerButton) -> Ink {
    Ink::Paint(Color::Rgba(paint_color(editor, button)))
}

/// The pixels of a curve with its control points so far.
fn curve_pixels(ends: (Point, Point), controls: &[Point], hover: Point) -> Vec<Point> {
    let (a, b) = ends;
    match controls {
        [] => shapes::line(a, b),
        [c1] => shapes::bezier(a, *c1, hover, b),
        [c1, c2, ..] => shapes::bezier(a, *c1, *c2, b),
    }
}

/// The pixels a brush covers along `pixels`, for previews.
fn widen(pixels: Vec<Point>, brush: &Brush) -> Vec<Point> {
    if brush.is_pixel() {
        return pixels;
    }
    let offsets = brush.offsets();
    let mut out: Vec<Point> = pixels
        .iter()
        .flat_map(|p| {
            offsets
                .iter()
                .map(move |o| Point::new(p.x + o.x, p.y + o.y))
        })
        .collect();
    out.sort_by_key(|p| (p.y, p.x));
    out.dedup();
    out
}

/// Paints `pixels` with the brush and the shape ink as one undo step.
fn paint_pixels(editor: &mut Editor, label: &str, pixels: &[Point], brush: &Brush, ink: &Ink) {
    let ctx = context(editor);
    let done = editor.edit(label, &ctx, |canvas| canvas.stamp_all(pixels, brush, ink));
    match done {
        Ok(((), dirty)) if !dirty.is_empty() => editor.touch_pixels(),
        Ok(_) => {}
        Err(e) => refuse(editor, e),
    }
}

/// The gradient a drag from `from` to `to` makes.
fn gradient_of(
    editor: &Editor,
    toolbox: &Toolbox,
    button: PointerButton,
    from: Point,
    to: Point,
) -> Gradient {
    let o = &toolbox.options;
    let (first, last) = if button == PointerButton::Secondary {
        (editor.bg, editor.fg)
    } else {
        (editor.fg, editor.bg)
    };
    let palette = editor.doc.palette();
    let index = |c: Rgba| palette.colors().iter().position(|p| *p == c && c.a > 0);
    let colors = match (o.gradient_palette, index(first), index(last)) {
        (true, Some(i), Some(j)) => {
            let step: Box<dyn Iterator<Item = usize>> = if i <= j {
                Box::new(i..=j)
            } else {
                Box::new((j..=i).rev())
            };
            step.map(|k| Color::Rgba(palette.colors()[k])).collect()
        }
        _ => vec![Color::Rgba(first), Color::Rgba(last)],
    };
    Gradient {
        shape: o.gradient_shape,
        from,
        to,
        colors,
        dither: dither_named(editor, toolbox, &o.gradient_dither),
        repeat: o.gradient_repeat,
    }
}

/// The dither a menu name stands for: a Bayer matrix, blue noise, hard
/// steps, or the pattern stamp's pattern by its lightness (a Bayer 4x4
/// while there is no pattern).
pub fn dither_named(editor: &Editor, toolbox: &Toolbox, name: &str) -> DitherPattern {
    match name {
        "Bayer 2x2" => DitherPattern::Bayer(Bayer::B2),
        "Bayer 8x8" => DitherPattern::Bayer(Bayer::B8),
        "Blue noise" => DitherPattern::BlueNoise,
        "Hard steps" => DitherPattern::None,
        "Pattern" => match &toolbox.pattern {
            Some((px, _)) => {
                let canvas_palette = editor.doc.palette();
                let ti = editor.doc.transparent_index();
                let luma: Vec<u8> = px
                    .data
                    .iter()
                    .map(|v| match editor.doc.mode() {
                        pixl_document::ColorMode::Rgba => Rgba::from_u32(*v).luma(),
                        pixl_document::ColorMode::Indexed => {
                            if *v as u8 == ti {
                                0
                            } else {
                                canvas_palette.get(*v as u8).luma()
                            }
                        }
                    })
                    .collect();
                DitherPattern::Custom {
                    width: px.width,
                    height: px.height,
                    thresholds: Arc::new(luma),
                }
            }
            None => DitherPattern::Bayer(Bayer::B4),
        },
        _ => DitherPattern::Bayer(Bayer::B4),
    }
}

/// The selection the magic wand makes at `p`.
fn wand(editor: &Editor, toolbox: &Toolbox, p: Point) -> Option<Selection> {
    let opts = FillOptions {
        connectivity: if toolbox.options.eight {
            Connectivity::Eight
        } else {
            Connectivity::Four
        },
        tolerance: editor.options.tolerance,
        contiguous: editor.options.contiguous,
    };
    if toolbox.options.merged {
        let flat = composite_frame(&editor.doc, editor.frame, &CompositeOptions::EXPORT);
        let sampler = Sampler::Merged {
            pixels: &flat,
            width: editor.doc.width(),
            height: editor.doc.height(),
        };
        Some(fill::magic_wand(&sampler, p, &opts))
    } else {
        let sampler = Sampler::cel(&editor.doc, editor.layer, editor.frame).ok()?;
        Some(fill::magic_wand(&sampler, p, &opts))
    }
}

/// What a press on the stage carries.
struct Press {
    p: Point,
    /// The window position, and the pointer's state.
    at: Vec2,
    data: PointerData,
    button: PointerButton,
    shift: bool,
    alt: bool,
    double: bool,
}

fn begin(
    editor: &mut Editor,
    toolbox: &mut Toolbox,
    preview: &mut Preview,
    press: Press,
    settings: &Settings,
) {
    let Press {
        p,
        button,
        shift,
        alt,
        double,
        ..
    } = press;
    let tool = editor.tool;
    // A gesture that spans several presses takes this one.
    match toolbox.session.take() {
        Some(Session::Curve {
            button: b,
            ends,
            mut controls,
            ..
        }) => {
            controls.push(p);
            if controls.len() >= 2 {
                preview.clear();
                let pixels = curve_pixels(ends, &controls, p);
                let pixels = if editor.options.pixel_perfect {
                    shapes::pixel_perfect(&pixels)
                } else {
                    pixels
                };
                let brush = editor.brush(editor.options.size);
                let ink = shape_ink(editor, b);
                paint_pixels(editor, "Curve", &pixels, &brush, &ink);
            } else {
                toolbox.session = Some(Session::Curve {
                    button: b,
                    ends,
                    controls,
                    dragging: false,
                    hover: p,
                });
            }
            return;
        }
        Some(Session::Corners {
            button: b,
            select,
            mut points,
            ..
        }) => {
            let closes = double
                || points
                    .first()
                    .is_some_and(|f| (f.x - p.x).abs() <= 1 && (f.y - p.y).abs() <= 1);
            if !closes && points.last() != Some(&p) {
                points.push(p);
            }
            if closes && points.len() >= 3 {
                finish_corners(editor, preview, b, select, points);
            } else {
                toolbox.session = Some(Session::Corners {
                    button: b,
                    select,
                    points,
                    hover: p,
                });
            }
            return;
        }
        other => toolbox.session = other,
    }
    if toolbox.session.is_some() {
        return;
    }
    let before = editor.selection.clone();
    let op = select_op(&toolbox.options, shift, alt);
    toolbox.session = match tool {
        t if sized(t) && t != Tool::Contour => begin_stroke(editor, toolbox, t, &press, settings),
        Tool::Curve => Some(Session::Curve {
            button,
            ends: (p, p),
            controls: Vec::new(),
            dragging: true,
            hover: p,
        }),
        Tool::Polygon => Some(Session::Corners {
            button,
            select: None,
            points: vec![p],
            hover: p,
        }),
        Tool::PolygonLasso => Some(Session::Corners {
            button,
            select: Some((op, before)),
            points: vec![p],
            hover: p,
        }),
        Tool::Contour => Some(Session::Path {
            button,
            select: None,
            points: vec![p],
        }),
        Tool::Lasso => Some(Session::Path {
            button,
            select: Some((op, before)),
            points: vec![p],
        }),
        Tool::Wand => {
            if let Some(shape) = wand(editor, toolbox, p) {
                editor.selection = combined(before.as_ref(), op, shape);
            }
            None
        }
        Tool::Text => {
            let pixels =
                toolbox
                    .font()
                    .layout(&toolbox.options.text, p, toolbox.options.text_scale, 0, 0);
            let ink = shape_ink(editor, button);
            paint_pixels(editor, "Text", &pixels, &Brush::PIXEL, &ink);
            None
        }
        Tool::Slice => {
            if let Some(i) = editor
                .doc
                .slices()
                .iter()
                .rposition(|s| s.bounds.contains(p.x, p.y))
            {
                toolbox.edit_slice = Some(i);
                None
            } else {
                Some(Session::Drag {
                    button,
                    tool,
                    from: p,
                    to: p,
                    select: None,
                })
            }
        }
        Tool::Gradient | Tool::Crop | Tool::Measure => Some(Session::Drag {
            button,
            tool,
            from: p,
            to: p,
            select: None,
        }),
        Tool::EllipseSelect => Some(Session::Drag {
            button,
            tool,
            from: p,
            to: p,
            select: Some((op, before)),
        }),
        _ => None,
    };
    show(editor, toolbox, preview);
}

/// Ends a polygon or polygon lasso.
/// Paints `g` over the canvas on worker threads, the tiles nearest `focus`
/// first: a gradient reaches every pixel of a large canvas.
fn start_gradient(
    editor: &mut Editor,
    g: Gradient,
    focus: Point,
) -> std::result::Result<PaintJob, pixl_document::Error> {
    let ctx = Arc::new(context(editor));
    let area = match &ctx.selection {
        Some(s) => s.bounds(),
        None => editor.doc.bounds(),
    };
    let tiles = tiles_of(area, (f64::from(focus.x), f64::from(focus.y)));
    let palette = Arc::new(editor.doc.palette().clone());
    let transparent_index = editor.doc.transparent_index();
    let alpha_locked = !editor.mask_edit && editor.doc.effective_locks(editor.layer)?.alpha;
    let g = Arc::new(g);
    PaintJob::start(editor, "Gradient", tiles, move |original| {
        let original = Arc::new(original);
        move || {
            // Each worker paints a copy of its own; untouched tiles stay
            // shared with the cel.
            let mut scratch = (*original).clone();
            let (palette, ctx, g) = (Arc::clone(&palette), Arc::clone(&ctx), Arc::clone(&g));
            move |rect: Rect| {
                let target = Target {
                    raster: &mut scratch,
                    palette: &palette,
                    transparent_index,
                    alpha_locked,
                };
                gradient::gradient_in(&mut Canvas::new(target, &ctx), &g, rect);
                read_values(&scratch, rect)
            }
        }
    })
}

fn finish_corners(
    editor: &mut Editor,
    preview: &mut Preview,
    button: PointerButton,
    select: Option<(SelectOp, Option<Selection>)>,
    points: Vec<Point>,
) {
    preview.clear();
    let (w, h) = (editor.doc.width(), editor.doc.height());
    match select {
        Some((op, before)) => {
            editor.selection = combined(before.as_ref(), op, Selection::lasso(w, h, &points));
        }
        None => {
            let ink = shape_ink(editor, button);
            if editor.options.filled {
                let pixels = shapes::polygon_filled(&points);
                paint_pixels(editor, "Polygon", &pixels, &Brush::PIXEL, &ink);
            } else {
                let pixels = shapes::polygon_outline(&points);
                let brush = editor.brush(editor.options.size);
                paint_pixels(editor, "Polygon", &pixels, &brush, &ink);
            }
        }
    }
}

fn extend(
    editor: &mut Editor,
    toolbox: &mut Toolbox,
    preview: &mut Preview,
    p: Point,
    event: (Vec2, &PointerData),
) {
    let Some(mut session) = toolbox.session.take() else {
        return;
    };
    match &mut session {
        Session::Stroke(s) => stroke_to(editor, toolbox, s, p, Some(event)),
        Session::Curve {
            ends,
            dragging,
            hover,
            ..
        } => {
            if *dragging {
                ends.1 = p;
            }
            *hover = p;
        }
        Session::Corners { hover, .. } => *hover = p,
        Session::Path { points, .. } => {
            if points.last() != Some(&p) {
                points.push(p);
            }
        }
        Session::Drag { to, .. } => *to = p,
    }
    toolbox.session = Some(session);
    show(editor, toolbox, preview);
}

fn finish(editor: &mut Editor, toolbox: &mut Toolbox, preview: &mut Preview, p: Point) {
    let Some(session) = toolbox.session.take() else {
        return;
    };
    match session {
        Session::Stroke(s) => end_stroke(editor, *s),
        Session::Curve {
            button,
            mut ends,
            controls,
            dragging,
            ..
        } => {
            if dragging {
                ends.1 = p;
            }
            // The ends are set; now the control points are clicked.
            toolbox.session = Some(Session::Curve {
                button,
                ends,
                controls,
                dragging: false,
                hover: p,
            });
        }
        s @ Session::Corners { .. } => toolbox.session = Some(s),
        Session::Path {
            button,
            select,
            points,
        } => {
            preview.clear();
            let (w, h) = (editor.doc.width(), editor.doc.height());
            match select {
                Some((op, before)) => {
                    editor.selection = if points.len() < 3 && op == SelectOp::Replace {
                        None
                    } else {
                        combined(before.as_ref(), op, Selection::lasso(w, h, &points))
                    };
                }
                None => {
                    if points.len() >= 3 {
                        let ink = shape_ink(editor, button);
                        let pixels = shapes::polygon_filled(&points);
                        paint_pixels(editor, "Contour", &pixels, &Brush::PIXEL, &ink);
                    }
                }
            }
        }
        Session::Drag {
            button,
            tool,
            from,
            select,
            ..
        } => {
            preview.clear();
            let to = p;
            let rect = Rect::from_corners(from, to);
            match tool {
                Tool::Gradient => {
                    let g = gradient_of(editor, toolbox, button, from, to);
                    match start_gradient(editor, g, to) {
                        // A small canvas is painted within this tick.
                        Ok(mut job) if crate::view::jobs::finishes_at_once(job.job.total) => {
                            job.pump(editor, None);
                        }
                        Ok(job) => toolbox.fill = Some(job),
                        Err(e) => refuse(editor, e),
                    }
                }
                Tool::EllipseSelect => {
                    let (op, before) = select.unwrap_or((SelectOp::Replace, None));
                    let (w, h) = (editor.doc.width(), editor.doc.height());
                    editor.selection = if from == to && op == SelectOp::Replace {
                        None
                    } else {
                        combined(before.as_ref(), op, Selection::ellipse(w, h, rect))
                    };
                }
                Tool::Crop => {
                    if rect.w > 1 || rect.h > 1 {
                        crate::menus::image::crop_to(editor, rect);
                    }
                }
                Tool::Measure => {
                    editor.message = Some(measure_text(from, to));
                }
                Tool::Slice if rect.w > 1 || rect.h > 1 => {
                    crate::tools::slices::add(editor, rect.intersect(&editor.doc.bounds()));
                }
                _ => {}
            }
        }
    }
    show(editor, toolbox, preview);
}

/// What the measure tool says about a drag.
pub fn measure_text(from: Point, to: Point) -> String {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = f64::from(dx * dx + dy * dy).sqrt();
    let angle = -f64::from(dy).atan2(f64::from(dx)).to_degrees();
    format!(
        "{}, {} to {}, {}: {} x {} px, length {:.1}, angle {:.1} degrees",
        from.x,
        from.y,
        to.x,
        to.y,
        dx.abs() + 1,
        dy.abs() + 1,
        len,
        angle
    )
}

/// Draws the session's preview.
fn show(editor: &Editor, toolbox: &Toolbox, preview: &mut Preview) {
    let Some(session) = &toolbox.session else {
        return;
    };
    let accent = toolbox.accent;
    let brush = editor.brush(editor.options.size);
    match session {
        Session::Stroke(_) => {}
        Session::Curve {
            button,
            ends,
            controls,
            hover,
            ..
        } => {
            let px = widen(curve_pixels(*ends, controls, *hover), &brush);
            preview.set(px, paint_color(editor, *button));
        }
        Session::Corners {
            button,
            select,
            points,
            hover,
        } => {
            let mut path = points.clone();
            path.push(*hover);
            let px = shapes::polyline(&path);
            let color = if select.is_some() {
                accent
            } else {
                paint_color(editor, *button)
            };
            preview.set(px, color);
        }
        Session::Path {
            button,
            select,
            points,
        } => {
            let px = shapes::polygon_outline(points);
            let color = if select.is_some() {
                accent
            } else {
                paint_color(editor, *button)
            };
            preview.set(px, color);
        }
        Session::Drag {
            button,
            tool,
            from,
            to,
            ..
        } => {
            let px = match tool {
                Tool::EllipseSelect => shapes::ellipse_outline(*from, *to),
                Tool::Crop | Tool::Slice => shapes::rect_outline(*from, *to),
                _ => shapes::line(*from, *to),
            };
            let color = match tool {
                Tool::Gradient => paint_color(editor, *button),
                _ => accent,
            };
            preview.set(px, color);
        }
    }
}

/// The commands the toolbox answers (apply and cancel, Enter and Escape by
/// default), whether the start screen has the keys instead, and the theme's
/// outline color.
#[derive(SystemParam)]
pub struct BoxKeys<'w> {
    commands: Res<'w, crate::settings::keymap::KeyCommands>,
    home: Res<'w, crate::Home>,
    colors: Res<'w, crate::view::ViewColors>,
    pointer: Res<'w, PointerState>,
    settings: Res<'w, Settings>,
}

/// The options bar's buttons, and the file dialog the font button opens.
#[derive(SystemParam)]
pub struct BoxButtons<'w, 's> {
    chosen: Res<'w, crate::menus::Chosen>,
    clicks: MessageReader<'w, 's, ClickEvent>,
    ids: ElementIds<'w, 's>,
    commands: Commands<'w, 's>,
}

/// Runs the toolbox's tools over this tick's pointer events, and Enter and
/// Escape for the tools that take several clicks.
pub fn use_toolbox(
    mut editor: ResMut<Editor>,
    mut toolbox: ResMut<Toolbox>,
    mut preview: ResMut<Preview>,
    place: Place,
    mut pointer: PointerInput,
    keys: BoxKeys,
    wake: Option<Res<WakeDeadline>>,
) {
    let editor = &mut *editor;
    let toolbox = &mut *toolbox;
    if let Some(c) = keys.colors.preview {
        toolbox.accent = c;
    }
    // A gradient being painted: its finished tiles, or Cancel.
    if let Some(mut fill) = toolbox.fill.take() {
        if keys.commands.ran("cancel", false) {
            fill.cancel(editor);
        } else if !fill.pump(editor, Some(Instant::now() + WRITE_BUDGET)) {
            toolbox.fill = Some(fill);
            if let Some(wake) = &wake {
                wake.request(Instant::now() + Duration::from_millis(4));
            }
        }
    }
    // A tool change ends what the last tool was doing.
    if toolbox.session.is_some() && !session_fits(toolbox.session.as_ref(), editor.tool) {
        cancel(editor, toolbox, &mut preview);
    }
    if !keys.home.open && toolbox.session.is_some() {
        if keys.commands.ran("cancel", false) {
            cancel(editor, toolbox, &mut preview);
        } else if keys.commands.ran("apply", false)
            && let Some(Session::Corners {
                button,
                select,
                points,
                ..
            }) = toolbox.session.take()
        {
            if points.len() >= 3 {
                finish_corners(editor, &mut preview, button, select, points);
            } else {
                preview.clear();
            }
        }
    }
    let shift = place.modifiers.0.shift;
    let alt = place.modifiers.0.alt;
    let view = &place.view;
    let mine = |t: Tool| {
        sized(t)
            || matches!(
                t,
                Tool::Curve
                    | Tool::Polygon
                    | Tool::Contour
                    | Tool::Gradient
                    | Tool::EllipseSelect
                    | Tool::Lasso
                    | Tool::PolygonLasso
                    | Tool::Wand
                    | Tool::Crop
                    | Tool::Text
                    | Tool::Measure
                    | Tool::Slice
            )
    };
    // The text under the pointer goes when the text tool does, and when the
    // pointer leaves the window.
    if toolbox.text_hover
        && (editor.tool != Tool::Text || keys.home.open || keys.pointer.position.is_none())
    {
        preview.clear();
        toolbox.text_hover = false;
    }
    // The Rig panel's modes take the stage's pointer too, and so does an
    // active reference layer.
    if !mine(editor.tool)
        || place.rig.captures_pointer()
        || crate::tools::reference::takes_pointer(editor)
    {
        pointer.events();
        return;
    }
    let over = place.over_stage();
    for event in pointer.events() {
        match event {
            Pointer::Down { at, button, data } => {
                let held_session = toolbox.session.is_some();
                let multi = matches!(
                    toolbox.session,
                    Some(Session::Curve { .. }) | Some(Session::Corners { .. })
                );
                if (held_session && !multi)
                    || toolbox.fill.is_some()
                    || place.nav.space_held
                    || place.nav.panning.is_some()
                    || !view.contains(at)
                    || !over
                    || !matches!(button, PointerButton::Primary | PointerButton::Secondary)
                {
                    continue;
                }
                let p = view.doc_pixel(at);
                let now = Instant::now();
                let double = toolbox
                    .last_press
                    .is_some_and(|(t, q)| now - t < DOUBLE_CLICK && q == p);
                toolbox.last_press = Some((now, p));
                let press = Press {
                    p,
                    at,
                    data,
                    button,
                    shift,
                    alt,
                    double,
                };
                begin(editor, toolbox, &mut preview, press, &keys.settings);
            }
            Pointer::Move { at, data } => {
                if toolbox.session.is_some() {
                    extend(
                        editor,
                        toolbox,
                        &mut preview,
                        view.doc_pixel(at),
                        (at, &data),
                    );
                } else if editor.tool == Tool::Text && view.contains(at) && over {
                    // The text shows where a click would set it.
                    let p = view.doc_pixel(at);
                    let px = toolbox.font().layout(
                        &toolbox.options.text,
                        p,
                        toolbox.options.text_scale,
                        0,
                        0,
                    );
                    preview.set(px, editor.fg);
                    toolbox.text_hover = true;
                } else if toolbox.text_hover {
                    // Off the canvas, onto a panel or the tool bar.
                    preview.clear();
                    toolbox.text_hover = false;
                }
            }
            Pointer::Up { at, button, .. } => {
                let ends = match &toolbox.session {
                    Some(Session::Stroke(s)) => s.button == button,
                    Some(Session::Path { button: b, .. }) => *b == button,
                    Some(Session::Drag { button: b, .. }) => *b == button,
                    Some(Session::Curve { dragging, .. }) => *dragging,
                    _ => false,
                };
                if ends {
                    finish(editor, toolbox, &mut preview, view.doc_pixel(at));
                }
            }
        }
    }
    // A held spray keeps spraying where it is.
    if let Some(Session::Stroke(s)) = &mut toolbox.session
        && matches!(s.stroke, Stroke::Spray { .. })
    {
        let due = toolbox
            .last_spray
            .is_none_or(|t| t.elapsed() >= SPRAY_EVERY);
        if due {
            let mut taken = toolbox.session.take();
            if let Some(Session::Stroke(s)) = &mut taken {
                let at = s.at;
                stroke_to(editor, toolbox, s, at, None);
            }
            toolbox.session = taken;
        }
        if let Some(wake) = &wake {
            wake.request(Instant::now() + SPRAY_EVERY);
        }
    }
}

/// Whether a session belongs to `tool`.
fn session_fits(session: Option<&Session>, tool: Tool) -> bool {
    match session {
        None => true,
        Some(Session::Stroke(_)) => sized(tool),
        Some(Session::Curve { .. }) => tool == Tool::Curve,
        Some(Session::Corners { select, .. }) => {
            tool == if select.is_some() {
                Tool::PolygonLasso
            } else {
                Tool::Polygon
            }
        }
        Some(Session::Path { select, .. }) => {
            tool == if select.is_some() {
                Tool::Lasso
            } else {
                Tool::Contour
            }
        }
        Some(Session::Drag { tool: t, .. }) => *t == tool,
    }
}

/// Drops the gesture in progress; a stroke keeps what it painted.
fn cancel(editor: &mut Editor, toolbox: &mut Toolbox, preview: &mut Preview) {
    match toolbox.session.take() {
        Some(Session::Stroke(s)) => end_stroke(editor, *s),
        Some(_) => preview.clear(),
        None => {}
    }
}

/// Asks for a BDF font file and adds it to the text tool's fonts.
pub fn load_font(commands: &mut Commands) {
    commands.queue(FileDialog::open().filter("BDF fonts", &["bdf"]).then(
        |world: &mut lumenui::ecs::world::World, paths| {
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| BitmapFont::parse_bdf(&t).map_err(|e| e.to_string()));
            match read {
                Ok(font) => {
                    let name = font.name.clone();
                    let mut toolbox = world.resource_mut::<Toolbox>();
                    toolbox.fonts.retain(|f| f.name != name);
                    toolbox.fonts.push(font);
                    toolbox.options.text_font = name;
                }
                Err(e) => world.resource_mut::<Editor>().message = Some(e),
            }
        },
    ));
}

/// Picks the option values a dropdown names.
fn bayer_named(name: &str) -> Option<Bayer> {
    match name {
        "Bayer 2x2" => Some(Bayer::B2),
        "Bayer 4x4" => Some(Bayer::B4),
        "Bayer 8x8" => Some(Bayer::B8),
        _ => None,
    }
}

fn bayer_name(m: Bayer) -> &'static str {
    match m {
        Bayer::B2 => "Bayer 2x2",
        Bayer::B4 => "Bayer 4x4",
        Bayer::B8 => "Bayer 8x8",
    }
}

fn op_name(op: SelectOp) -> &'static str {
    match op {
        SelectOp::Replace => "Replace",
        SelectOp::Add => "Add",
        SelectOp::Subtract => "Subtract",
        SelectOp::Intersect => "Intersect",
    }
}

fn op_named(name: &str) -> Option<SelectOp> {
    [
        SelectOp::Replace,
        SelectOp::Add,
        SelectOp::Subtract,
        SelectOp::Intersect,
    ]
    .into_iter()
    .find(|o| op_name(*o) == name)
}

fn shape_name(s: GradientShape) -> &'static str {
    match s {
        GradientShape::Linear => "Linear",
        GradientShape::Radial => "Radial",
        GradientShape::Conic => "Conic",
    }
}

fn symmetry_name(mode: SymmetryMode) -> &'static str {
    match mode {
        SymmetryMode::None => "No symmetry",
        SymmetryMode::Horizontal => "Mirror left and right",
        SymmetryMode::Vertical => "Mirror top and bottom",
        SymmetryMode::Both => "Mirror both",
        SymmetryMode::Radial(_) => "Radial",
    }
}

/// What the options bar's controls say, as the editor last showed them.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    options: BoxOptions,
    symmetry: SymmetryMode,
    shape: String,
}

fn shape_label(shape: &BrushShape) -> &'static str {
    match shape {
        BrushShape::Round => "Round",
        BrushShape::Square => "Square",
        BrushShape::Custom(_) => "Custom",
    }
}

/// Keeps the toolbox's options and their controls in step, keeps each
/// tool's pixel-perfect choice, and runs the options bar's buttons.
pub fn toolbox_options(
    mut editor: ResMut<Editor>,
    mut toolbox: ResMut<Toolbox>,
    mut shown: Local<Option<Shown>>,
    mut last_tool: Local<Option<Tool>>,
    mut signals: Signals,
    mut buttons: BoxButtons,
    wake: Option<Res<WakeDeadline>>,
) {
    let BoxButtons {
        chosen,
        clicks,
        ids,
        commands,
    } = &mut buttons;
    let editor = &mut *editor;
    // [ and ] step the brush size, within the size slider's range.
    for id in &chosen.0 {
        match id.as_str() {
            "brush-smaller" => editor.options.size = editor.options.size.saturating_sub(1).max(1),
            "brush-larger" => editor.options.size = (editor.options.size + 1).min(MAX_SIZE),
            _ => {}
        }
    }
    // Each tool keeps its own pixel-perfect choice.
    if *last_tool != Some(editor.tool) {
        if let Some(old) = *last_tool {
            toolbox
                .pixel_perfect
                .insert(old, editor.options.pixel_perfect);
        }
        let default = matches!(editor.tool, Tool::Pencil | Tool::Eraser);
        editor.options.pixel_perfect = *toolbox.pixel_perfect.get(&editor.tool).unwrap_or(&default);
        *last_tool = Some(editor.tool);
    }
    for click in clicks.read() {
        let Ok(id) = ids.of(click.entity) else {
            continue;
        };
        match id.0.as_str() {
            "opt-brush-from-selection" => match &editor.selection {
                Some(sel) if !sel.is_empty() => {
                    editor.brush_shape = BrushShape::Custom(Brush::from_selection(sel));
                }
                _ => editor.message = Some("Select the pixels to make a brush of".into()),
            },
            "opt-pattern-from-selection" => match &editor.selection {
                Some(sel) if !sel.is_empty() => {
                    let b = sel.bounds();
                    let clear = clear_raw(editor);
                    let raster = snapshot(editor);
                    let px = Pixels::from_raster_masked(&raster, b, Some(sel), clear);
                    toolbox.pattern = Some((Arc::new(px), Point::new(b.x, b.y)));
                    editor.message = Some(format!("Pattern of {} x {} px", b.w, b.h));
                }
                _ => editor.message = Some("Select the pixels to make a pattern of".into()),
            },
            "opt-text-load" => load_font(commands),
            "crop-to-selection" => match editor.selection.as_ref().map(Selection::bounds) {
                Some(b) if !b.is_empty() => crate::menus::image::crop_to(editor, b),
                _ => editor.message = Some("Select the area to keep".into()),
            },
            _ => {}
        }
    }
    let now = Shown {
        options: toolbox.options.clone(),
        symmetry: editor.symmetry.mode,
        shape: shape_label(&editor.brush_shape).to_string(),
    };
    let mut o = toolbox.options.clone();
    let mut mode = editor.symmetry.mode;
    let mut shape = editor.brush_shape.clone();
    if let Some(last) = shown.as_ref() {
        let num = |name: &str| signals.get::<f64>(name);
        let pct =
            |name: &str, was: u8| num(name).map_or(was, |v| v.round().clamp(0.0, 100.0) as u8);
        let flag = |name: &str, was: bool| signals.get::<bool>(name).unwrap_or(was);
        let text = |name: &str| signals.get::<String>(name);
        let l = &last.options;
        let changed = |a: u8, b: u8| a != b;
        let v = pct("opt_opacity", l.opacity);
        if changed(v, l.opacity) {
            o.opacity = v.max(1);
        }
        let v = pct("opt_hardness", l.hardness);
        if changed(v, l.hardness) {
            o.hardness = v;
        }
        if flag("opt_dithered", l.dithered) != l.dithered {
            o.dithered = !l.dithered;
        }
        if let Some(v) = num("opt_spray_radius").map(|v| v.round().clamp(1.0, 64.0) as u32)
            && v != l.spray_radius
        {
            o.spray_radius = v;
        }
        if let Some(v) = num("opt_spray_density").map(|v| v.round().clamp(1.0, 100.0) as u32)
            && v != l.spray_density
        {
            o.spray_density = v;
        }
        let v = pct("opt_dither_amount", l.dither_amount);
        if changed(v, l.dither_amount) {
            o.dither_amount = v;
        }
        if let Some(m) = text("opt_dither_matrix").and_then(|n| bayer_named(&n))
            && m != l.dither_matrix
        {
            o.dither_matrix = m;
        }
        let v = pct("opt_strength", l.strength);
        if changed(v, l.strength) {
            o.strength = v.max(1);
        }
        if flag("opt_keep_colors", l.keep_colors) != l.keep_colors {
            o.keep_colors = !l.keep_colors;
        }
        if flag("opt_sharpen", l.sharpen) != l.sharpen {
            o.sharpen = !l.sharpen;
        }
        if flag("opt_merged", l.merged) != l.merged {
            o.merged = !l.merged;
        }
        if flag("opt_eight", l.eight) != l.eight {
            o.eight = !l.eight;
        }
        if let Some(op) = text("opt_select_op").and_then(|n| op_named(&n))
            && op != l.select_op
        {
            o.select_op = op;
        }
        if let Some(s) = text("opt_gradient_shape").and_then(|n| {
            [
                GradientShape::Linear,
                GradientShape::Radial,
                GradientShape::Conic,
            ]
            .into_iter()
            .find(|s| shape_name(*s) == n)
        }) && s != l.gradient_shape
        {
            o.gradient_shape = s;
        }
        if let Some(d) = text("opt_gradient_dither")
            && d != l.gradient_dither
        {
            o.gradient_dither = d;
        }
        if let Some(c) = text("opt_gradient_colors") {
            let palette = c == "Palette";
            if palette != l.gradient_palette {
                o.gradient_palette = palette;
            }
        }
        if flag("opt_gradient_repeat", l.gradient_repeat) != l.gradient_repeat {
            o.gradient_repeat = !l.gradient_repeat;
        }
        if let Some(t) = text("opt_text")
            && t != l.text
        {
            o.text = t;
        }
        if let Some(f) = text("opt_text_font")
            && f != l.text_font
        {
            o.text_font = f;
        }
        if let Some(s) = text("opt_text_scale").and_then(|t| t.trim().parse::<u32>().ok())
            && s != l.text_scale
        {
            o.text_scale = s.clamp(1, 16);
        }
        if let Some(v) = num("opt_stabilizer").map(|v| v.round().clamp(0.0, 32.0) as u32)
            && v != l.stabilizer
        {
            o.stabilizer = v;
        }
        let radial = text("opt_radial")
            .and_then(|t| t.trim().parse::<u8>().ok())
            .map(|n| n.clamp(2, 32));
        if let Some(name) = text("opt_symmetry")
            && name != symmetry_name(last.symmetry)
        {
            mode = match name.as_str() {
                "Mirror left and right" => SymmetryMode::Horizontal,
                "Mirror top and bottom" => SymmetryMode::Vertical,
                "Mirror both" => SymmetryMode::Both,
                "Radial" => SymmetryMode::Radial(radial.unwrap_or(4)),
                _ => SymmetryMode::None,
            };
        }
        if let (SymmetryMode::Radial(n), Some(r)) = (mode, radial)
            && n != r
        {
            mode = SymmetryMode::Radial(r);
        }
        if let Some(name) = text("opt_brush_shape")
            && name != last.shape
        {
            shape = match name.as_str() {
                "Square" => BrushShape::Square,
                "Custom" => match &editor.selection {
                    Some(sel) if !sel.is_empty() => BrushShape::Custom(Brush::from_selection(sel)),
                    _ => editor.brush_shape.clone(),
                },
                _ => BrushShape::Round,
            };
        }
    }
    if o != toolbox.options {
        toolbox.options = o.clone();
    }
    if mode != editor.symmetry.mode {
        let fresh = editor.symmetry.mode == SymmetryMode::None;
        editor.symmetry.mode = mode;
        if fresh {
            editor.symmetry =
                pixl_paint::Symmetry::centered(mode, editor.doc.width(), editor.doc.height());
        }
    }
    if shape != editor.brush_shape {
        editor.brush_shape = shape;
    }
    let next = Shown {
        options: toolbox.options.clone(),
        symmetry: editor.symmetry.mode,
        shape: shape_label(&editor.brush_shape).to_string(),
    };
    if shown.as_ref() != Some(&next) || shown.is_none() || now != next {
        let o = &next.options;
        signals.set("opt_opacity", f64::from(o.opacity));
        signals.set("opt_opacity_text", format!("{}%", o.opacity));
        signals.set("opt_hardness", f64::from(o.hardness));
        signals.set("opt_hardness_text", format!("{}%", o.hardness));
        signals.set("opt_dithered", o.dithered);
        signals.set("opt_spray_radius", f64::from(o.spray_radius));
        signals.set("opt_spray_radius_text", format!("{} px", o.spray_radius));
        signals.set("opt_spray_density", f64::from(o.spray_density));
        signals.set("opt_spray_density_text", o.spray_density.to_string());
        signals.set("opt_dither_amount", f64::from(o.dither_amount));
        signals.set("opt_dither_amount_text", format!("{}%", o.dither_amount));
        signals.set("opt_dither_matrix", bayer_name(o.dither_matrix));
        signals.set("opt_strength", f64::from(o.strength));
        signals.set("opt_strength_text", format!("{}%", o.strength));
        signals.set("opt_keep_colors", o.keep_colors);
        signals.set("opt_sharpen", o.sharpen);
        signals.set("opt_merged", o.merged);
        signals.set("opt_eight", o.eight);
        signals.set("opt_select_op", op_name(o.select_op));
        signals.set("opt_gradient_shape", shape_name(o.gradient_shape));
        signals.set("opt_gradient_dither", o.gradient_dither.clone());
        signals.set(
            "opt_gradient_colors",
            if o.gradient_palette {
                "Palette"
            } else {
                "Two colors"
            },
        );
        signals.set("opt_gradient_repeat", o.gradient_repeat);
        signals.set("opt_text", o.text.clone());
        signals.set("opt_text_font", o.text_font.clone());
        signals.set("opt_text_scale", o.text_scale.to_string());
        signals.set("opt_stabilizer", f64::from(o.stabilizer));
        signals.set(
            "opt_stabilizer_text",
            if o.stabilizer == 0 {
                "off".to_string()
            } else {
                format!("{} px", o.stabilizer)
            },
        );
        signals.set("opt_symmetry", symmetry_name(next.symmetry));
        signals.set(
            "show_radial",
            matches!(next.symmetry, SymmetryMode::Radial(_)),
        );
        if let SymmetryMode::Radial(n) = next.symmetry {
            signals.set("opt_radial", n.to_string());
        }
        signals.set("opt_brush_shape", next.shape.clone());
        *shown = Some(next);
        crate::repaint_soon(&wake);
    }
}

/// Shows the options of the active toolbox tool.
pub fn show_toolbox(
    editor: Res<Editor>,
    mut shown: Local<Option<Tool>>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    if *shown == Some(editor.tool) {
        return;
    }
    let tool = editor.tool;
    use Tool::*;
    let brushy = sized(tool)
        || matches!(
            tool,
            Pencil | Eraser | Line | Curve | Rectangle | Ellipse | Polygon
        );
    signals.set("show_brush_shape", brushy && tool != Spray);
    signals.set("show_opacity", tool == Brush);
    signals.set("show_spray", tool == Spray);
    signals.set("show_dither", tool == DitherBrush);
    signals.set("show_strength", matches!(tool, Smudge | Blur));
    signals.set("show_sharpen", tool == Blur);
    signals.set("show_pattern", tool == PatternStamp);
    signals.set("show_match", matches!(tool, Wand | ReplaceColor));
    signals.set("show_wand", tool == Wand);
    signals.set("show_select_op", selects(tool));
    signals.set("show_gradient", tool == Gradient);
    signals.set("show_text", tool == Text);
    signals.set("show_transform", tool == Transform);
    signals.set("show_crop", tool == Crop);
    signals.set("show_stabilizer", freehand(tool) && tool != Contour);
    signals.set("show_symmetry", paints(tool));
    *shown = Some(tool);
    crate::repaint_soon(&wake);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_ops_from_modifiers() {
        let o = BoxOptions::default();
        assert_eq!(select_op(&o, true, false), SelectOp::Add);
        assert_eq!(select_op(&o, false, true), SelectOp::Subtract);
        assert_eq!(select_op(&o, true, true), SelectOp::Intersect);
        assert_eq!(select_op(&o, false, false), SelectOp::Replace);
    }

    #[test]
    fn combining_selections() {
        let a = Selection::rect(10, 10, Rect::new(0, 0, 4, 4));
        let b = Selection::rect(10, 10, Rect::new(2, 2, 4, 4));
        let add = combined(Some(&a), SelectOp::Add, b.clone()).unwrap();
        assert_eq!(add.count(), 16 + 16 - 4);
        let int = combined(Some(&a), SelectOp::Intersect, b.clone()).unwrap();
        assert_eq!(int.count(), 4);
        assert!(combined(None, SelectOp::Intersect, b.clone()).is_none());
        assert_eq!(combined(None, SelectOp::Add, b.clone()), Some(b));
    }

    #[test]
    fn measuring() {
        let t = measure_text(Point::new(0, 0), Point::new(3, 4));
        assert!(t.contains("4 x 5 px") && t.contains("length 5.0"), "{t}");
        assert!(measure_text(Point::new(0, 0), Point::new(0, -2)).contains("angle 90.0"));
    }

    #[test]
    fn every_tool_has_a_hint_or_is_plain() {
        for t in Tool::ALL {
            if sized(t) || selects(t) && t != Tool::Select {
                assert!(!hint(t).is_empty(), "{t:?}");
            }
        }
    }
}
