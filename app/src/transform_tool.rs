//! Free transform: the selection (or the whole layer's content) lifted into
//! a box with handles, then moved, scaled, turned, skewed and flipped with
//! a live preview on the canvas. The corner and edge handles scale (Shift
//! keeps the proportions, Ctrl on an edge skews), a drag outside the box
//! turns it (Shift steps 15 degrees), a drag inside moves it. Enter or Apply
//! keeps the result as one undo step; Escape or Cancel puts the pixels back.
//! The pixels are resampled by nearest neighbor, RotSprite or clean edge,
//! so no new colors appear.

use lumenui::lumen_core::input::{Key, NamedKey, PointerButton};
use lumenui::prelude::*;
use pixl_core::history::Transaction;
use pixl_core::transform::draw_transformed;
use pixl_core::{Affine, ColorMode, Pixels, Point, Raster, Rect, Resample, Scope};

use crate::aids::Aids;
use crate::editor::{Editor, Tool};
use crate::input::{Pointer, PointerInput};
use crate::menus::Chosen;
use crate::tools::{Place, refuse};
use crate::view::View;

/// How near, in screen pixels, a press takes a handle.
const HANDLE_REACH: f64 = 6.0;

/// The settings of a free transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub scale: (f64, f64),
    pub skew: (f64, f64),
    pub rotation: f64,
    pub offset: (f64, f64),
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            scale: (1.0, 1.0),
            skew: (0.0, 0.0),
            rotation: 0.0,
            offset: (0.0, 0.0),
        }
    }
}

/// What a drag in progress changes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move {
        from: (f64, f64),
        was: Shape,
    },
    /// A handle at local unit position (-1, 0 or 1 on each axis).
    Handle {
        at: (i8, i8),
        was: Shape,
        skew: bool,
    },
    Turn {
        from: f64,
        was: Shape,
    },
}

/// A free transform in progress.
struct Session {
    layer: pixl_core::LayerId,
    frame: usize,
    tx: Option<Transaction>,
    /// The cel as it was.
    original: Raster,
    /// The lifted pixels and where their top-left was.
    lifted: Pixels,
    rect: Rect,
    /// The selection the pixels came from, to move with them.
    shape: Shape,
    grab: Option<Grab>,
    clear: u32,
}

impl Session {
    fn pivot(&self) -> (f64, f64) {
        (
            f64::from(self.rect.x) + f64::from(self.rect.w) / 2.0,
            f64::from(self.rect.y) + f64::from(self.rect.h) / 2.0,
        )
    }

    fn matrix(&self) -> Affine {
        let s = self.shape;
        Affine::about(self.pivot(), s.scale, s.skew, s.rotation, s.offset)
    }

    /// The box's corners in document coordinates, clockwise from the
    /// top-left.
    fn corners(&self) -> [(f64, f64); 4] {
        let m = self.matrix();
        let r = self.rect;
        let (x0, y0, x1, y1) = (
            f64::from(r.x),
            f64::from(r.y),
            f64::from(r.right()),
            f64::from(r.bottom()),
        );
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| m.apply(x, y))
    }

    /// A document point in the box's own frame, in units of half its size
    /// (the corners at -1 and 1).
    fn local(&self, p: (f64, f64)) -> Option<(f64, f64)> {
        local_of(self.rect, self.shape, p)
    }
}

/// A document point in the frame of a box of `rect` with `shape`, in units
/// of half its size.
fn local_of(rect: Rect, shape: Shape, p: (f64, f64)) -> Option<(f64, f64)> {
    let pivot = (
        f64::from(rect.x) + f64::from(rect.w) / 2.0,
        f64::from(rect.y) + f64::from(rect.h) / 2.0,
    );
    let m = Affine::about(pivot, shape.scale, shape.skew, shape.rotation, shape.offset);
    let (x, y) = m.inverse()?.apply(p.0, p.1);
    let (hw, hh) = (
        f64::from(rect.w).max(1.0) / 2.0,
        f64::from(rect.h).max(1.0) / 2.0,
    );
    Some(((x - pivot.0) / hw, (y - pivot.1) / hh))
}

/// The commands and keys the free transform reads.
#[derive(lumenui::ecs::system::SystemParam)]
pub struct XfInput<'w, 's> {
    chosen: Res<'w, Chosen>,
    keys: MessageReader<'w, 's, KeyPressed>,
}

/// The free transform in progress, if any, and its resampling.
#[derive(Resource, Default)]
pub struct FreeTransform {
    session: Option<Session>,
    pub resample: Resample,
}

impl FreeTransform {
    pub fn active(&self) -> bool {
        self.session.is_some()
    }
}

fn clear_of(editor: &Editor) -> u32 {
    match editor.doc.mode() {
        ColorMode::Indexed => u32::from(editor.doc.transparent_index()),
        ColorMode::Rgba => 0,
    }
}

/// Lifts the selection, or the layer's content, into a new session.
fn start(editor: &mut Editor) -> Option<Session> {
    let (layer, frame) = (editor.layer, editor.frame);
    match editor.doc.effective_locks(layer) {
        Ok(l) if l.pixels || l.position => {
            refuse(editor, pixl_core::Error::Locked(layer));
            return None;
        }
        Err(e) => {
            refuse(editor, e);
            return None;
        }
        _ => {}
    }
    let image = match editor.doc.target_image(layer, frame) {
        Ok(i) => i,
        Err(e) => {
            refuse(editor, e);
            return None;
        }
    };
    let original = editor.doc.image(image)?.clone();
    let clear = clear_of(editor);
    let selection = editor.selection.clone();
    let rect = match &selection {
        Some(s) => s.bounds(),
        None => original.content_bounds(),
    }
    .intersect(&editor.doc.bounds());
    if rect.is_empty() {
        editor.message = Some("Nothing to transform".into());
        return None;
    }
    let lifted = Pixels::from_raster_masked(&original, rect, selection.as_ref(), clear);
    let mut tx = editor
        .history
        .begin(&editor.doc, "Free transform", Scope::Images(vec![image]));
    tx.watch(&editor.doc, image);
    editor.selection = None;
    Some(Session {
        layer,
        frame,
        tx: Some(tx),
        original,
        lifted,
        rect,
        shape: Shape::default(),
        grab: None,
        clear,
    })
}

/// Shows the session's current shape on the cel.
fn preview(editor: &mut Editor, s: &Session, resample: Resample) {
    let Ok(t) = editor.doc.target(s.layer, s.frame) else {
        return;
    };
    *t.raster = s.original.clone();
    // Take the lifted pixels out, then draw them transformed.
    for (i, p) in s.rect.points().enumerate() {
        if s.lifted.data[i] != s.clear {
            t.raster.set_raw(p.x, p.y, s.clear);
        }
    }
    draw_transformed(
        t.raster,
        &s.lifted,
        Point::new(s.rect.x, s.rect.y),
        &s.matrix(),
        resample,
        s.clear,
    );
    editor.touch_pixels();
}

fn finish(editor: &mut Editor, mut s: Session, keep: bool) {
    if keep {
        if let Some(tx) = s.tx.take() {
            editor.history.commit(&editor.doc, tx);
        }
    } else if let Ok(t) = editor.doc.target(s.layer, s.frame) {
        *t.raster = s.original.clone();
    }
    editor.touch_pixels();
}

/// The handle under a document point, if any: its unit position.
fn handle_at(s: &Session, view: &View, p: (f64, f64)) -> Option<(i8, i8)> {
    let (lx, ly) = s.local(p)?;
    let (hw, hh) = (
        f64::from(s.rect.w).max(1.0) / 2.0 * s.shape.scale.0.abs() * view.zoom,
        f64::from(s.rect.h).max(1.0) / 2.0 * s.shape.scale.1.abs() * view.zoom,
    );
    let mut best: Option<((i8, i8), f64)> = None;
    for hx in [-1i8, 0, 1] {
        for hy in [-1i8, 0, 1] {
            if hx == 0 && hy == 0 {
                continue;
            }
            let dx = (lx - f64::from(hx)) * hw;
            let dy = (ly - f64::from(hy)) * hh;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= HANDLE_REACH && best.is_none_or(|(_, b)| d < b) {
                best = Some(((hx, hy), d));
            }
        }
    }
    best.map(|(h, _)| h)
}

fn angle(s: &Session, p: (f64, f64)) -> f64 {
    let (cx, cy) = s.pivot();
    let (cx, cy) = (cx + s.shape.offset.0, cy + s.shape.offset.1);
    (p.1 - cy).atan2(p.0 - cx).to_degrees()
}

/// Runs the free transform: picks up the selection when the tool is chosen,
/// drags handles, and applies or cancels.
pub fn use_transform(
    mut editor: ResMut<Editor>,
    mut ft: ResMut<FreeTransform>,
    mut aids: ResMut<Aids>,
    input: XfInput,
    place: Place,
    mut pointer: PointerInput,
    mut signals: Signals,
) {
    let XfInput { chosen, mut keys } = input;
    let editor = &mut *editor;
    let ft = &mut *ft;
    let resample = match signals.get::<String>("opt_resample").as_deref() {
        Some("Clean edge") => Resample::CleanEdge,
        Some("Nearest") => Resample::Nearest,
        _ => Resample::RotSprite,
    };
    let mut dirty = false;
    if resample != ft.resample {
        ft.resample = resample;
        dirty = true;
    }
    // Leaving the tool keeps what was done.
    if editor.tool != Tool::Transform {
        pointer.events();
        keys.clear();
        if let Some(s) = ft.session.take() {
            finish(editor, s, true);
            aids.transform_box = None;
            aids.revision += 1;
        }
        return;
    }
    if ft.session.is_none() && chosen.0.is_empty() {
        ft.session = start(editor);
        if ft.session.is_some() {
            dirty = true;
            signals.set("opt_angle", "0".to_string());
        }
    }
    let Some(mut s) = ft.session.take() else {
        pointer.events();
        return;
    };
    let mut keep: Option<bool> = None;
    for id in &chosen.0 {
        match id.as_str() {
            "xf-flip-h" => {
                s.shape.scale.0 = -s.shape.scale.0;
                dirty = true;
            }
            "xf-flip-v" => {
                s.shape.scale.1 = -s.shape.scale.1;
                dirty = true;
            }
            "xf-apply" => keep = Some(true),
            "xf-cancel" => keep = Some(false),
            _ => {}
        }
    }
    for k in keys.read() {
        match k.key {
            Key::Named(NamedKey::Enter) => keep = Some(true),
            Key::Named(NamedKey::Escape) => keep = Some(false),
            _ => {}
        }
    }
    // The angle field turns the box to a typed angle.
    if let Some(a) = signals
        .get::<String>("opt_angle")
        .and_then(|t| t.trim().parse::<f64>().ok())
        && (a - s.shape.rotation).abs() > 1e-6
        && s.grab.is_none()
    {
        s.shape.rotation = a;
        dirty = true;
    }
    let view = &place.view;
    let shift = place.modifiers.0.shift;
    let ctrl = place.modifiers.0.ctrl;
    let alt = place.modifiers.0.alt;
    let over = place.over_stage();
    for event in pointer.events() {
        match event {
            Pointer::Down {
                at,
                button: PointerButton::Primary,
                ..
            } if over && view.contains(at) => {
                let p = view.to_doc(view.stage_point(at));
                let was = s.shape;
                s.grab = Some(match handle_at(&s, view, p) {
                    Some(h) => Grab::Handle {
                        at: h,
                        was,
                        skew: ctrl && (h.0 == 0 || h.1 == 0),
                    },
                    None => match s.local(p) {
                        Some((lx, ly)) if lx.abs() <= 1.0 && ly.abs() <= 1.0 => {
                            Grab::Move { from: p, was }
                        }
                        _ => Grab::Turn {
                            from: angle(&s, p),
                            was,
                        },
                    },
                });
            }
            Pointer::Move { at, .. } => {
                let Some(grab) = s.grab else {
                    continue;
                };
                let p = view.to_doc(view.stage_point(at));
                match grab {
                    Grab::Move { from, was } => {
                        // Moves land on whole pixels.
                        s.shape.offset = (
                            (was.offset.0 + p.0 - from.0).round(),
                            (was.offset.1 + p.1 - from.1).round(),
                        );
                    }
                    Grab::Turn { from, was } => {
                        let mut a = was.rotation + angle(&s, p) - from;
                        if shift {
                            a = (a / 15.0).round() * 15.0;
                        }
                        s.shape.rotation = a.rem_euclid(360.0);
                        signals.set("opt_angle", format!("{:.1}", s.shape.rotation));
                    }
                    Grab::Handle { at: h, was, skew } => {
                        // Work in the frame of the box as it was grabbed.
                        let Some((lx, ly)) = local_of(s.rect, was, p) else {
                            continue;
                        };
                        if skew {
                            let (hw, hh) = (
                                f64::from(s.rect.w).max(1.0) / 2.0,
                                f64::from(s.rect.h).max(1.0) / 2.0,
                            );
                            if h.1 != 0 {
                                // Top or bottom edge: slide along x.
                                let along = lx * hw;
                                s.shape.skew.0 =
                                    was.skew.0 + (along / hh * f64::from(h.1)).atan().to_degrees();
                            } else {
                                let along = ly * hh;
                                s.shape.skew.1 =
                                    was.skew.1 + (along / hw * f64::from(h.0)).atan().to_degrees();
                            }
                        } else {
                            // How much the box grows along each axis the
                            // handle pulls: from the opposite side, or from
                            // the middle with Alt held.
                            let factor = |l: f64, hv: i8| match hv {
                                0 => 1.0,
                                _ if alt => l * f64::from(hv),
                                _ => (l * f64::from(hv) + 1.0) / 2.0,
                            };
                            let (mut fx, mut fy) = (factor(lx, h.0), factor(ly, h.1));
                            if shift && h.0 != 0 && h.1 != 0 {
                                let k = fx.abs().max(fy.abs());
                                fx = k.copysign(fx);
                                fy = k.copysign(fy);
                            }
                            // A box never collapses to nothing.
                            let floor = |v: f64| {
                                if v.abs() < 0.01 {
                                    0.01_f64.copysign(v)
                                } else {
                                    v
                                }
                            };
                            let next = Shape {
                                scale: (floor(was.scale.0 * fx), floor(was.scale.1 * fy)),
                                ..was
                            };
                            // The opposite side stays where it was.
                            let offset = if alt {
                                was.offset
                            } else {
                                let (cx, cy) = s.pivot();
                                let (hw, hh) = (
                                    f64::from(s.rect.w).max(1.0) / 2.0,
                                    f64::from(s.rect.h).max(1.0) / 2.0,
                                );
                                let anchor = (cx - f64::from(h.0) * hw, cy - f64::from(h.1) * hh);
                                let at = |sh: Shape| {
                                    Affine::about(
                                        (cx, cy),
                                        sh.scale,
                                        sh.skew,
                                        sh.rotation,
                                        sh.offset,
                                    )
                                    .apply(anchor.0, anchor.1)
                                };
                                let (old, new) = (at(was), at(next));
                                (was.offset.0 + old.0 - new.0, was.offset.1 + old.1 - new.1)
                            };
                            s.shape = Shape { offset, ..next };
                        }
                    }
                }
                dirty = true;
            }
            Pointer::Up {
                button: PointerButton::Primary,
                ..
            } => s.grab = None,
            _ => {}
        }
    }
    if let Some(k) = keep {
        finish(editor, s, k);
        aids.transform_box = None;
        aids.revision += 1;
        // The tool's job is done: back to moving.
        editor.tool = Tool::Move;
        return;
    }
    if dirty {
        preview(editor, &s, ft.resample);
        aids.transform_box = Some(s.corners());
        aids.revision += 1;
    }
    ft.session = Some(s);
}
