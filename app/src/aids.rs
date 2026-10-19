//! View aids: rulers along the stage, guides dragged out of them, snapping
//! to the grid, guides and slices, a custom grid (square or isometric),
//! tiled mode, turning and mirroring the view, the slices drawn over the
//! document with their dialog, and the symmetry axes, whose marks in the
//! rulers drag them.
//!
//! Guides and the custom grid belong to the editor session; they are not
//! saved with the document.

use lumenui::glam::Vec2;
use lumenui::lumen_core::components::Transform;
use lumenui::lumen_core::input::PointerButton;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{Canvas, Context2d};
use pixl_core::{Rect, Rgba, Scope, Slice, SymmetryMode};

use crate::editor::{Editor, Tool};
use crate::input::{Pointer, PointerInput};
use crate::menus::Chosen;
use crate::toolbox::Toolbox;
use crate::view::{Snap, View};

/// Degrees a view turn steps by.
pub const TURN_STEP: f64 = 15.0;

/// How near, in screen pixels, a press in a ruler takes a mark.
const MARK_REACH: f64 = 5.0;

/// The colors the aids draw with, read from the theme's probe elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AidColors {
    pub guide: Option<Rgba>,
    pub slice: Option<Rgba>,
    pub axis: Option<Rgba>,
    pub grid: Option<Rgba>,
    pub ruler: Option<Rgba>,
    pub ruler_ink: Option<Rgba>,
}

/// A drag that started in a ruler.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// A guide: a new one (`index: None`) or an existing one, along x
    /// (`vertical`) or y, at a document edge.
    Guide {
        vertical: bool,
        index: Option<usize>,
        at: i32,
    },
    /// A symmetry axis, in half pixels.
    Axis { vertical: bool },
}

/// What of the editor the overlay's aids show: the tool, the symmetry, the
/// slices and the canvas size.
type Seen = (Tool, pixl_core::Symmetry, Vec<Slice>, (u32, u32));

/// What the rulers were drawn for: the aids' and the view's revisions and
/// the two rulers' sizes.
type RulersDrawn = (u64, u64, (f32, f32), (f32, f32));

/// The view aids' settings and state.
#[derive(Resource)]
pub struct Aids {
    pub rulers: bool,
    pub guides_shown: bool,
    /// Vertical guides at these x edges.
    pub guides_x: Vec<i32>,
    /// Horizontal guides at these y edges.
    pub guides_y: Vec<i32>,
    pub snap: bool,
    /// The custom grid shows.
    pub grid: bool,
    pub grid_cell: (u32, u32),
    /// The grid dialog set the cell; until it does, the cell follows the
    /// canvas settings' custom grid.
    pub grid_cell_chosen: bool,
    pub grid_offset: (i32, i32),
    /// The grid's lines run at 2:1 diagonals instead of across and down.
    pub isometric: bool,
    /// Slices show whatever the tool.
    pub slices: bool,
    /// Bumped whenever something the overlay draws changes.
    pub revision: u64,
    drag: Option<Drag>,
    pub colors: AidColors,
    /// What the overlay last showed of the editor: tool, symmetry, slices.
    seen: Option<Seen>,
    /// The slice the dialog edits.
    editing: Option<usize>,
    /// The box of the free transform or of the active reference layer,
    /// its corners in document coordinates.
    pub transform_box: Option<[(f64, f64); 4]>,
    rulers_drawn: Option<RulersDrawn>,
}

impl Default for Aids {
    fn default() -> Self {
        Self {
            rulers: false,
            guides_shown: true,
            guides_x: Vec::new(),
            guides_y: Vec::new(),
            snap: false,
            grid: false,
            grid_cell: (16, 16),
            grid_cell_chosen: false,
            grid_offset: (0, 0),
            isometric: false,
            slices: false,
            revision: 1,
            drag: None,
            colors: AidColors::default(),
            seen: None,
            editing: None,
            transform_box: None,
            rulers_drawn: None,
        }
    }
}

impl Aids {
    /// What positions snap to, with snapping on.
    pub fn snap_targets(&self, editor: &Editor) -> Option<Snap> {
        if !self.snap {
            return None;
        }
        let mut xs = if self.guides_shown {
            self.guides_x.clone()
        } else {
            Vec::new()
        };
        let mut ys = if self.guides_shown {
            self.guides_y.clone()
        } else {
            Vec::new()
        };
        for s in editor.doc.slices() {
            xs.extend([s.bounds.x, s.bounds.right()]);
            ys.extend([s.bounds.y, s.bounds.bottom()]);
        }
        let grid = (self.grid && !self.isometric).then_some((
            self.grid_cell.0,
            self.grid_cell.1,
            self.grid_offset.0,
            self.grid_offset.1,
        ));
        Some(Snap { grid, xs, ys })
    }
}

fn probe(id: &str) -> Option<Rgba> {
    dom::get_by_id(id)?
        .computed_style_of("bg")
        .as_deref()
        .and_then(crate::view::parse_hex)
}

fn text_number(signals: &Signals, name: &str, default: i64) -> i64 {
    signals
        .get::<String>(name)
        .and_then(|t| t.trim().parse::<f64>().ok())
        .map_or(default, |v| v.round() as i64)
}

/// Reads the colors the aids draw with from the theme's probes: for a few
/// ticks after the look changes, since the new classes take a tick to land
/// and another to cascade, and not again until it changes, since each read
/// runs the cascade for its probe.
pub fn read_aid_colors(
    look: Res<crate::settings::Look>,
    mut aids: ResMut<Aids>,
    mut read_for: Local<Option<(crate::settings::Look, u8)>>,
) {
    let settle = match *read_for {
        Some((l, n)) if l == *look => n,
        _ => 0,
    };
    if settle <= 4 || aids.colors.guide.is_none() {
        *read_for = Some((*look, settle.saturating_add(1)));
        let colors = AidColors {
            guide: probe("probe-guide"),
            slice: probe("probe-slice"),
            axis: probe("probe-axis"),
            grid: probe("probe-custom-grid"),
            ruler: probe("probe-ruler"),
            ruler_ink: probe("probe-ruler-ink"),
        };
        if colors != aids.colors {
            aids.colors = colors;
            aids.revision += 1;
        }
    }
}

/// Starts the custom grid at the cell the canvas settings give, until the
/// grid dialog sets one.
pub fn follow_grid_setting(settings: Res<crate::settings::Settings>, mut aids: ResMut<Aids>) {
    let [w, h] = settings.canvas.custom_grid;
    if !aids.grid_cell_chosen && aids.grid_cell != (w, h) {
        aids.grid_cell = (w, h);
        aids.revision += 1;
    }
}

/// Runs the View menu's commands, the grid and slice dialogs, and keeps the
/// view's snapping and the overlay in step with the aids.
#[allow(clippy::too_many_arguments)]
pub fn aids_commands(
    chosen: Res<Chosen>,
    mut editor: ResMut<Editor>,
    mut settings: ResMut<crate::settings::Settings>,
    mut toolbox: ResMut<Toolbox>,
    mut aids: ResMut<Aids>,
    mut view: ResMut<View>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let editor = &mut *editor;
    let aids = &mut *aids;
    let mut changed = false;
    for id in &chosen.0 {
        changed = true;
        match id.as_str() {
            "view-rulers" => aids.rulers = !aids.rulers,
            "view-guides" => aids.guides_shown = !aids.guides_shown,
            "view-clear-guides" => {
                aids.guides_x.clear();
                aids.guides_y.clear();
            }
            "view-snap" => {
                aids.snap = !aids.snap;
                editor.message = Some(if aids.snap {
                    "Snapping to the grid, guides and slices".into()
                } else {
                    "Snapping off".into()
                });
            }
            "view-grid" => aids.grid = !aids.grid,
            "view-pixel-grid" => {
                // A setting, so it stays as it was left; the view redraws
                // when the canvas settings change.
                let c = &mut settings.canvas;
                c.pixel_grid = !c.pixel_grid;
                if c.pixel_grid && view.zoom * 100.0 < c.grid_from_zoom - 1e-6 {
                    editor.message = Some(format!(
                        "The pixel grid shows from {}% zoom",
                        c.grid_from_zoom.round()
                    ));
                }
            }
            "view-slices" => aids.slices = !aids.slices,
            "view-tile-x" => {
                view.tiled.0 = !view.tiled.0;
                view.revision += 1;
            }
            "view-tile-y" => {
                view.tiled.1 = !view.tiled.1;
                view.revision += 1;
            }
            "view-rotate-cw" | "view-rotate-ccw" | "view-rotate-reset" => {
                view.rotation = match id.as_str() {
                    "view-rotate-cw" => (view.rotation + TURN_STEP).rem_euclid(360.0),
                    "view-rotate-ccw" => (view.rotation - TURN_STEP).rem_euclid(360.0),
                    _ => 0.0,
                };
                view.revision += 1;
            }
            "view-flip" => {
                view.flipped = !view.flipped;
                view.revision += 1;
            }
            "view-grid-settings" => {
                signals.set("grid_title", "Grid");
                signals.set("grid_w", aids.grid_cell.0.to_string());
                signals.set("grid_h", aids.grid_cell.1.to_string());
                signals.set("grid_x", aids.grid_offset.0.to_string());
                signals.set("grid_y", aids.grid_offset.1.to_string());
                signals.set("grid_iso", aids.isometric);
                signals.set("grid_open", true);
            }
            "grid-ok" => {
                signals.set("grid_open", false);
                aids.grid_cell = (
                    text_number(&signals, "grid_w", 16).clamp(1, 4096) as u32,
                    text_number(&signals, "grid_h", 16).clamp(1, 4096) as u32,
                );
                aids.grid_offset = (
                    text_number(&signals, "grid_x", 0) as i32,
                    text_number(&signals, "grid_y", 0) as i32,
                );
                aids.isometric = signals.get::<bool>("grid_iso").unwrap_or(false);
                aids.grid = true;
                aids.grid_cell_chosen = true;
            }
            "grid-cancel" => signals.set("grid_open", false),
            "slice-ok" => {
                signals.set("slice_open", false);
                if let Some(i) = aids.editing.take() {
                    apply_slice(editor, &signals, i);
                }
            }
            "slice-cancel" => {
                signals.set("slice_open", false);
                aids.editing = None;
            }
            "slice-delete" => {
                signals.set("slice_open", false);
                if let Some(i) = aids.editing.take()
                    && i < editor.doc.slices().len()
                {
                    editor
                        .history
                        .transact(&mut editor.doc, "Delete slice", Scope::All, |d| {
                            d.slices_mut().remove(i)
                        });
                    editor.touch();
                }
            }
            _ => changed = false,
        }
    }
    if let Some(i) = toolbox.edit_slice.take()
        && let Some(s) = editor.doc.slices().get(i)
    {
        open_slice(&mut signals, s);
        aids.editing = Some(i);
    }
    let seen = (
        editor.tool,
        editor.symmetry,
        editor.doc.slices().to_vec(),
        (editor.doc.width(), editor.doc.height()),
    );
    if aids.seen.as_ref() != Some(&seen) {
        aids.seen = Some(seen);
        changed = true;
    }
    if changed {
        aids.revision += 1;
        crate::repaint_soon(&wake);
    }
    let snap = aids.snap_targets(editor);
    if view.snap != snap {
        view.snap = snap;
    }
    if let Some(stage) = dom::get_by_id("stage")
        && stage.has_class("rulers") != aids.rulers
    {
        stage.toggle_class("rulers");
        crate::repaint_soon(&wake);
    }
}

fn open_slice(signals: &mut Signals, s: &Slice) {
    signals.set("slice_title", format!("Slice {}", s.name));
    signals.set("slice_name", s.name.clone());
    for (k, v) in [
        ("slice_x", s.bounds.x),
        ("slice_y", s.bounds.y),
        ("slice_w", s.bounds.w),
        ("slice_h", s.bounds.h),
    ] {
        signals.set(k, v.to_string());
    }
    let c = s.center.unwrap_or(Rect::new(
        1,
        1,
        (s.bounds.w - 2).max(1),
        (s.bounds.h - 2).max(1),
    ));
    signals.set("slice_nine", s.center.is_some());
    for (k, v) in [
        ("slice_cx", c.x),
        ("slice_cy", c.y),
        ("slice_cw", c.w),
        ("slice_ch", c.h),
    ] {
        signals.set(k, v.to_string());
    }
    let p = s.pivot.unwrap_or_default();
    signals.set("slice_has_pivot", s.pivot.is_some());
    signals.set("slice_px", p.x.to_string());
    signals.set("slice_py", p.y.to_string());
    signals.set("slice_open", true);
}

fn apply_slice(editor: &mut Editor, signals: &Signals, i: usize) {
    let Some(old) = editor.doc.slices().get(i).cloned() else {
        return;
    };
    let n = |k: &str, d: i32| text_number(signals, k, i64::from(d)) as i32;
    let bounds = Rect::new(
        n("slice_x", old.bounds.x),
        n("slice_y", old.bounds.y),
        n("slice_w", old.bounds.w).max(1),
        n("slice_h", old.bounds.h).max(1),
    );
    let center = signals.get::<bool>("slice_nine").unwrap_or(false).then(|| {
        Rect::new(
            n("slice_cx", 0),
            n("slice_cy", 0),
            n("slice_cw", 1).max(1),
            n("slice_ch", 1).max(1),
        )
        .intersect(&Rect::new(0, 0, bounds.w, bounds.h))
    });
    let pivot = signals
        .get::<bool>("slice_has_pivot")
        .unwrap_or(false)
        .then(|| pixl_core::Point::new(n("slice_px", 0), n("slice_py", 0)));
    let name = signals
        .get::<String>("slice_name")
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(old.name.clone());
    let next = Slice {
        name,
        bounds,
        center,
        pivot,
        color: old.color,
    };
    if next != old {
        editor
            .history
            .transact(&mut editor.doc, "Edit slice", Scope::All, |d| {
                d.slices_mut()[i] = next;
            });
        editor.touch();
    }
}

/// The rulers' places in the window: the top ruler and the left one, each
/// top-left and size.
fn ruler_rects(canvases: &Query<(&Canvas, &Transform)>) -> [Option<(Vec2, Vec2)>; 2] {
    let find = |id: &str| {
        canvases
            .iter()
            .find(|(c, _)| c.id == id)
            .map(|(_, t)| (t.absolute, t.size))
    };
    [find("ruler-top"), find("ruler-left")]
}

fn inside(r: Option<(Vec2, Vec2)>, at: Vec2) -> bool {
    r.is_some_and(|(p, s)| at.x >= p.x && at.y >= p.y && at.x < p.x + s.x && at.y < p.y + s.y)
}

/// Drags guides out of the rulers, moves them and the symmetry axes by
/// their marks, and drops a guide dragged back into its ruler.
pub fn aids_input(
    mut aids: ResMut<Aids>,
    mut editor: ResMut<Editor>,
    view: Res<View>,
    mut pointer: PointerInput,
    canvases: Query<(&Canvas, &Transform)>,
) {
    let aids = &mut *aids;
    let [top, left] = ruler_rects(&canvases);
    if !aids.rulers {
        pointer.events();
        aids.drag = None;
        return;
    }
    let doc_at = |at: Vec2| view.to_doc(view.stage_point(at));
    for event in pointer.events() {
        match event {
            Pointer::Down {
                at,
                button: PointerButton::Primary,
                ..
            } => {
                let (dx, dy) = doc_at(at);
                let reach = MARK_REACH / view.zoom;
                if inside(top, at) {
                    // The top ruler holds the marks of vertical guides and
                    // of the vertical symmetry axis; a press elsewhere in it
                    // pulls out a horizontal guide.
                    let sym = editor.symmetry;
                    let axis_x = f64::from(sym.axis_x2) / 2.0;
                    aids.drag = if matches!(
                        sym.mode,
                        SymmetryMode::Horizontal | SymmetryMode::Both | SymmetryMode::Radial(_)
                    ) && (dx - axis_x).abs() <= reach
                    {
                        Some(Drag::Axis { vertical: true })
                    } else if let Some(i) = aids
                        .guides_x
                        .iter()
                        .position(|g| (f64::from(*g) - dx).abs() <= reach)
                    {
                        Some(Drag::Guide {
                            vertical: true,
                            index: Some(i),
                            at: aids.guides_x[i],
                        })
                    } else {
                        Some(Drag::Guide {
                            vertical: false,
                            index: None,
                            at: dy.round() as i32,
                        })
                    };
                } else if inside(left, at) {
                    let sym = editor.symmetry;
                    let axis_y = f64::from(sym.axis_y2) / 2.0;
                    aids.drag = if matches!(
                        sym.mode,
                        SymmetryMode::Vertical | SymmetryMode::Both | SymmetryMode::Radial(_)
                    ) && (dy - axis_y).abs() <= reach
                    {
                        Some(Drag::Axis { vertical: false })
                    } else if let Some(i) = aids
                        .guides_y
                        .iter()
                        .position(|g| (f64::from(*g) - dy).abs() <= reach)
                    {
                        Some(Drag::Guide {
                            vertical: false,
                            index: Some(i),
                            at: aids.guides_y[i],
                        })
                    } else {
                        Some(Drag::Guide {
                            vertical: true,
                            index: None,
                            at: dx.round() as i32,
                        })
                    };
                }
                if aids.drag.is_some() {
                    aids.revision += 1;
                }
            }
            Pointer::Move { at, .. } => {
                let (dx, dy) = doc_at(at);
                match aids.drag.as_mut() {
                    Some(Drag::Guide { vertical, at, .. }) => {
                        *at = if *vertical { dx } else { dy }.round() as i32;
                        aids.revision += 1;
                    }
                    Some(Drag::Axis { vertical }) => {
                        // Axes sit on pixel edges or pixel middles.
                        if *vertical {
                            editor.symmetry.axis_x2 = (dx * 2.0).round() as i32;
                        } else {
                            editor.symmetry.axis_y2 = (dy * 2.0).round() as i32;
                        }
                        aids.revision += 1;
                    }
                    None => {}
                }
            }
            Pointer::Up {
                at,
                button: PointerButton::Primary,
                ..
            } => {
                let Some(drag) = aids.drag.take() else {
                    continue;
                };
                aids.revision += 1;
                let Drag::Guide {
                    vertical,
                    index,
                    at: pos,
                } = drag
                else {
                    continue;
                };
                // Back in a ruler, a guide goes; on the stage it stays.
                let dropped = inside(top, at) || inside(left, at) || !view.contains(at);
                let list = if vertical {
                    &mut aids.guides_x
                } else {
                    &mut aids.guides_y
                };
                match (index, dropped) {
                    (Some(i), true) => {
                        list.remove(i);
                    }
                    (Some(i), false) => list[i] = pos,
                    (None, false) => list.push(pos),
                    (None, true) => {}
                }
            }
            _ => {}
        }
    }
}

/// The step between labelled ruler ticks, in document pixels, so they sit
/// at least `min_px` screen pixels apart.
pub fn ruler_step(zoom: f64, min_px: f64) -> i64 {
    let mut step = 1i64;
    loop {
        for m in [1, 2, 5] {
            if (step * m) as f64 * zoom >= min_px {
                return step * m;
            }
        }
        step *= 10;
        if step > 1_000_000_000 {
            return step;
        }
    }
}

/// Draws the two rulers: ticks and numbers in document pixels, the guides'
/// and the symmetry axes' marks.
pub fn draw_rulers(
    mut aids: ResMut<Aids>,
    editor: Res<Editor>,
    view: Res<View>,
    canvases: Query<(&Canvas, &Transform)>,
) {
    if !aids.rulers {
        aids.rulers_drawn = None;
        return;
    }
    let [Some(top), Some(left)] = ruler_rects(&canvases) else {
        return;
    };
    let key = (
        aids.revision,
        view.revision,
        (top.1.x, top.1.y),
        (left.1.x, left.1.y),
    );
    if aids.rulers_drawn == Some(key) {
        return;
    }
    aids.rulers_drawn = Some(key);
    let c = &aids.colors;
    let (Some(bg), Some(ink)) = (c.ruler, c.ruler_ink) else {
        return;
    };
    let color = |c: Rgba| {
        lumenui_canvas::Rgba::new(
            f64::from(c.r) / 255.0,
            f64::from(c.g) / 255.0,
            f64::from(c.b) / 255.0,
            f64::from(c.a) / 255.0,
        )
    };
    for (id, rect, horizontal) in [("ruler-top", top, true), ("ruler-left", left, false)] {
        let ctx = Context2d::new(id);
        ctx.resize(rect.1.x, rect.1.y);
        let (len, thick) = if horizontal {
            (f64::from(rect.1.x), f64::from(rect.1.y))
        } else {
            (f64::from(rect.1.y), f64::from(rect.1.x))
        };
        ctx.set_fill_color(color(bg));
        ctx.fill_rect(0.0, 0.0, f64::from(rect.1.x), f64::from(rect.1.y));
        // Ruler positions in stage coordinates: the stage's top-left is the
        // rulers' too.
        let offset = if horizontal {
            f64::from(rect.0.x - view.stage.0.x)
        } else {
            f64::from(rect.0.y - view.stage.0.y)
        };
        let origin = if horizontal {
            view.origin.0
        } else {
            view.origin.1
        };
        let to_screen = |d: f64| origin + d * view.zoom - offset;
        let step = ruler_step(view.zoom, 48.0);
        let first = ((offset - origin) / view.zoom / step as f64).floor() as i64 - 1;
        let last = ((offset + len - origin) / view.zoom / step as f64).ceil() as i64 + 1;
        ctx.set_fill_color(color(ink));
        let _ = ctx.set_font(&format!("{}px sans-serif", (thick * 0.55).round().max(6.0)));
        let minor = if step >= 5 { step / 5 } else { step };
        for k in first * (step / minor)..=last * (step / minor) {
            let d = (k * minor) as f64;
            let s = to_screen(d);
            if s < -1.0 || s > len + 1.0 {
                continue;
            }
            let major = (k * minor) % step == 0;
            let h = if major { thick } else { thick * 0.3 };
            if horizontal {
                ctx.fill_rect(s.floor(), thick - h, 1.0, h);
                if major {
                    ctx.fill_text(&(k * minor).to_string(), s + 3.0, thick * 0.6);
                }
            } else {
                ctx.fill_rect(thick - h, s.floor(), h, 1.0);
                if major {
                    ctx.fill_text(&(k * minor).to_string(), 2.0, s - 3.0);
                }
            }
        }
        let mark = |ctx: &Context2d, d: f64, c: Rgba| {
            let s = to_screen(d).floor();
            ctx.set_fill_color(color(c));
            if horizontal {
                ctx.fill_rect(s - 2.0, 0.0, 5.0, thick);
            } else {
                ctx.fill_rect(0.0, s - 2.0, thick, 5.0);
            }
        };
        if let Some(g) = c.guide {
            let guides = if horizontal {
                &aids.guides_x
            } else {
                &aids.guides_y
            };
            for v in guides {
                mark(&ctx, f64::from(*v), g);
            }
        }
        let sym = editor.symmetry;
        let shows = match sym.mode {
            SymmetryMode::None => false,
            SymmetryMode::Horizontal => horizontal,
            SymmetryMode::Vertical => !horizontal,
            SymmetryMode::Both | SymmetryMode::Radial(_) => true,
        };
        if shows && let Some(a) = c.axis {
            let v = if horizontal { sym.axis_x2 } else { sym.axis_y2 };
            mark(&ctx, f64::from(v) / 2.0, a);
        }
    }
}

/// Draws the aids over the document on the overlay canvas: the custom grid,
/// the guides, the slices and the symmetry axes. The overlay already has
/// the view's turn on.
pub fn draw(ctx: &Context2d, view: &View, aids: &Aids, editor: &Editor) {
    let z = view.zoom;
    let (ox, oy) = view.origin;
    let (w, h) = (editor.doc.width(), editor.doc.height());
    let color = |c: Rgba| {
        lumenui_canvas::Rgba::new(
            f64::from(c.r) / 255.0,
            f64::from(c.g) / 255.0,
            f64::from(c.b) / 255.0,
            f64::from(c.a) / 255.0,
        )
    };
    let (dw, dh) = (f64::from(w) * z, f64::from(h) * z);
    if aids.grid
        && let Some(g) = aids.colors.grid
    {
        let (cw, ch) = (aids.grid_cell.0.max(1), aids.grid_cell.1.max(1));
        // Skip a grid whose lines would be closer than four screen pixels.
        if f64::from(cw.min(ch)) * z >= 4.0 {
            ctx.set_fill_color(color(g));
            let vis = view.visible_pixels(w, h);
            if aids.isometric {
                // Lines of slope 1:2 both ways, one cell height apart.
                ctx.set_stroke_color(color(g));
                ctx.set_line_width(1.0);
                ctx.save();
                ctx.begin_path();
                ctx.rect(ox, oy, dw, dh);
                ctx.clip();
                ctx.begin_path();
                let span = f64::from(w.max(h)) * 2.0;
                let step = f64::from(ch);
                let n = (span / step).ceil() as i64 + 1;
                for k in -n..=n {
                    let y0 = f64::from(aids.grid_offset.1) + k as f64 * step;
                    // Down-right and up-right lines through (offset x, y0).
                    let x0 = f64::from(aids.grid_offset.0);
                    for dir in [1.0, -1.0] {
                        let (ax, ay) = (x0 - span, y0 - dir * span / 2.0);
                        let (bx, by) = (x0 + span, y0 + dir * span / 2.0);
                        ctx.move_to(ox + ax * z, oy + ay * z);
                        ctx.line_to(ox + bx * z, oy + by * z);
                    }
                }
                ctx.stroke();
                ctx.restore();
            } else {
                let (offx, offy) = aids.grid_offset;
                let first_x = vis.x - (vis.x - offx).rem_euclid(cw as i32);
                let mut x = first_x;
                while x <= vis.right() {
                    ctx.fill_rect(ox + f64::from(x) * z, oy, 1.0, dh);
                    x += cw as i32;
                }
                let first_y = vis.y - (vis.y - offy).rem_euclid(ch as i32);
                let mut y = first_y;
                while y <= vis.bottom() {
                    ctx.fill_rect(ox, oy + f64::from(y) * z, dw, 1.0);
                    y += ch as i32;
                }
            }
        }
    }
    if let Some(g) = aids.colors.guide {
        ctx.set_fill_color(color(g));
        let (fx0, fy0, fx1, fy1) = view.shown_frame();
        if aids.guides_shown {
            for x in &aids.guides_x {
                ctx.fill_rect(ox + f64::from(*x) * z, fy0, 1.0, fy1 - fy0);
            }
            for y in &aids.guides_y {
                ctx.fill_rect(fx0, oy + f64::from(*y) * z, fx1 - fx0, 1.0);
            }
        }
        // A guide being dragged out shows where it would land.
        if let Some(Drag::Guide { vertical, at, .. }) = aids.drag {
            if vertical {
                ctx.fill_rect(ox + f64::from(at) * z, fy0, 1.0, fy1 - fy0);
            } else {
                ctx.fill_rect(fx0, oy + f64::from(at) * z, fx1 - fx0, 1.0);
            }
        }
    }
    if (aids.slices || editor.tool == Tool::Slice)
        && let Some(sc) = aids.colors.slice
    {
        ctx.set_stroke_color(color(sc));
        ctx.set_fill_color(color(sc));
        ctx.set_line_width(1.0);
        for s in editor.doc.slices() {
            let b = s.bounds;
            let (x, y) = (ox + f64::from(b.x) * z, oy + f64::from(b.y) * z);
            let (bw, bh) = (f64::from(b.w) * z, f64::from(b.h) * z);
            ctx.stroke_rect(x + 0.5, y + 0.5, bw - 1.0, bh - 1.0);
            if let Some(c) = s.center {
                let cx = x + f64::from(c.x) * z;
                let cy = y + f64::from(c.y) * z;
                let (cw, chh) = (f64::from(c.w) * z, f64::from(c.h) * z);
                for xx in [cx, cx + cw] {
                    ctx.fill_rect(xx, y, 1.0, bh);
                }
                for yy in [cy, cy + chh] {
                    ctx.fill_rect(x, yy, bw, 1.0);
                }
            }
            if let Some(p) = s.pivot {
                let px = x + (f64::from(p.x) + 0.5) * z;
                let py = y + (f64::from(p.y) + 0.5) * z;
                ctx.fill_rect(px - 4.0, py, 9.0, 1.0);
                ctx.fill_rect(px, py - 4.0, 1.0, 9.0);
            }
        }
    }
    // The free transform's or the reference's box, with a handle at each
    // corner and edge.
    if let (Some(corners), Some(g)) = (aids.transform_box, aids.colors.guide) {
        let at = |(x, y): (f64, f64)| (ox + x * z, oy + y * z);
        ctx.set_stroke_color(color(g));
        ctx.set_fill_color(color(g));
        ctx.set_line_width(1.0);
        ctx.begin_path();
        let (x0, y0) = at(corners[0]);
        ctx.move_to(x0, y0);
        for c in &corners[1..] {
            let (x, y) = at(*c);
            ctx.line_to(x, y);
        }
        ctx.close_path();
        ctx.stroke();
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            for (x, y) in [a, ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)] {
                let (sx, sy) = at((x, y));
                ctx.fill_rect(sx - 3.0, sy - 3.0, 7.0, 7.0);
            }
        }
    }
    let sym = editor.symmetry;
    let painting = !matches!(
        editor.tool,
        Tool::Select
            | Tool::EllipseSelect
            | Tool::Lasso
            | Tool::PolygonLasso
            | Tool::Wand
            | Tool::Move
            | Tool::Transform
            | Tool::Crop
            | Tool::Measure
            | Tool::Slice
            | Tool::Eyedropper
    );
    if painting
        && sym.mode != SymmetryMode::None
        && let Some(a) = aids.colors.axis
    {
        ctx.set_fill_color(color(a));
        let ax = ox + f64::from(sym.axis_x2) / 2.0 * z;
        let ay = oy + f64::from(sym.axis_y2) / 2.0 * z;
        let vertical = matches!(
            sym.mode,
            SymmetryMode::Horizontal | SymmetryMode::Both | SymmetryMode::Radial(_)
        );
        let horizontal = matches!(
            sym.mode,
            SymmetryMode::Vertical | SymmetryMode::Both | SymmetryMode::Radial(_)
        );
        if vertical {
            ctx.fill_rect(ax.floor(), oy, 1.0, dh);
        }
        if horizontal {
            ctx.fill_rect(ox, ay.floor(), dw, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_steps_stay_readable() {
        assert_eq!(ruler_step(16.0, 48.0), 5);
        assert_eq!(ruler_step(1.0, 48.0), 50);
        assert_eq!(ruler_step(0.1, 48.0), 500);
        assert_eq!(ruler_step(64.0, 48.0), 1);
    }

    #[test]
    fn snapping_reaches_the_nearest_line() {
        let s = Snap {
            grid: Some((8, 8, 0, 0)),
            xs: vec![13],
            ys: vec![],
        };
        assert_eq!(s.apply((12.6, 3.0), 1.0), (13.0, 3.0));
        assert_eq!(s.apply((15.5, 8.8), 1.0), (16.0, 8.0));
        assert_eq!(s.apply((15.5, 9.2), 1.0), (16.0, 9.2));
        assert_eq!(s.apply((20.0, 20.0), 1.0), (20.0, 20.0));
    }
}
