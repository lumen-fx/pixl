//! Placing a reference image: a box with handles over it on the canvas
//! while its layer is active, and its place in the layer properties.
//!
//! A drag inside the box moves the image, a corner handle scales it keeping
//! its proportions (Shift frees them), an edge handle stretches one side, and
//! a drag outside the box turns it (Shift steps 15 degrees). The side across
//! from the handle stays put; Alt scales from the middle instead. Each drag
//! is one undo step. Any tool but the eyedropper places the reference while
//! its layer is active: no tool paints on it.

use lumenui::lumen_core::input::PointerButton;
use lumenui::prelude::*;
use pixl_document::history::Transaction;
use pixl_document::{ReferencePlace, Scope};

use crate::editor::{Editor, Tool};
use crate::tools::transform::FreeTransform;
use crate::tools::{Place, refuse};
use crate::view::View;
use crate::view::aids::Aids;
use crate::view::input::{Pointer, PointerInput};

/// How near, in screen pixels, a press takes a handle.
const HANDLE_REACH: f64 = 6.0;

/// Whether the active layer is a reference the pointer places: the tools
/// leave the stage's pointer to it, but for the eyedropper.
pub fn takes_pointer(editor: &Editor) -> bool {
    editor.tool != Tool::Eyedropper && editor.doc.reference(editor.layer).is_ok()
}

/// What a drag in progress changes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move {
        from: (f64, f64),
    },
    /// A handle at local unit position (-1, 0 or 1 on each axis).
    Handle {
        at: (i8, i8),
    },
    Turn {
        from: f64,
    },
}

impl Grab {
    fn label(self) -> &'static str {
        match self {
            Grab::Move { .. } => "Move reference",
            Grab::Handle { .. } => "Scale reference",
            Grab::Turn { .. } => "Turn reference",
        }
    }
}

/// A drag on the reference's box.
struct Drag {
    grab: Grab,
    /// The place as it was when the drag began.
    was: ReferencePlace,
    tx: Transaction,
}

/// The drag in progress, and whether the box is on the canvas.
#[derive(Resource, Default)]
pub struct ReferenceBox {
    drag: Option<Drag>,
    shown: Option<[(f64, f64); 4]>,
}

/// A document point in the frame of an image of `size` at `place`, in
/// units of half its size (the corners at -1 and 1).
fn local(place: &ReferencePlace, size: (u32, u32), p: (f64, f64)) -> Option<(f64, f64)> {
    let (x, y) = place.matrix(size).inverse()?.apply(p.0, p.1);
    let (hw, hh) = (f64::from(size.0) / 2.0, f64::from(size.1) / 2.0);
    Some(((x - hw) / hw, (y - hh) / hh))
}

/// The handle under a document point, if any: its unit position.
fn handle_at(
    place: &ReferencePlace,
    size: (u32, u32),
    view: &View,
    p: (f64, f64),
) -> Option<(i8, i8)> {
    let (lx, ly) = local(place, size, p)?;
    let (w, h) = place.extent(size);
    let (hw, hh) = (w / 2.0 * view.zoom, h / 2.0 * view.zoom);
    let mut best: Option<((i8, i8), f64)> = None;
    for hx in [-1i8, 0, 1] {
        for hy in [-1i8, 0, 1] {
            if hx == 0 && hy == 0 {
                continue;
            }
            let d = ((lx - f64::from(hx)) * hw).hypot((ly - f64::from(hy)) * hh);
            if d <= HANDLE_REACH && best.is_none_or(|(_, b)| d < b) {
                best = Some(((hx, hy), d));
            }
        }
    }
    best.map(|(h, _)| h)
}

fn angle_to(place: &ReferencePlace, p: (f64, f64)) -> f64 {
    (p.1 - place.center.1)
        .atan2(p.0 - place.center.0)
        .to_degrees()
}

/// The place a handle drag to `p` gives: scaled along the axes the handle
/// pulls, keeping the proportions on a corner unless `free`, from the side
/// across unless `middle`.
pub fn scaled(
    was: ReferencePlace,
    size: (u32, u32),
    handle: (i8, i8),
    p: (f64, f64),
    free: bool,
    middle: bool,
) -> Option<ReferencePlace> {
    let (lx, ly) = local(&was, size, p)?;
    let factor = |l: f64, hv: i8| match hv {
        0 => 1.0,
        _ if middle => l * f64::from(hv),
        _ => (l * f64::from(hv) + 1.0) / 2.0,
    };
    let (mut fx, mut fy) = (factor(lx, handle.0), factor(ly, handle.1));
    if !free && handle.0 != 0 && handle.1 != 0 {
        let k = fx.max(fy);
        (fx, fy) = (k, k);
    }
    // A drag past the far side does not mirror the image, and it never
    // collapses.
    let (fx, fy) = (fx.max(0.01), fy.max(0.01));
    let mut next = ReferencePlace {
        scale: (was.scale.0 * fx, was.scale.1 * fy),
        ..was
    };
    if !middle {
        // The point across from the handle stays where it was.
        let (hw, hh) = (f64::from(size.0) / 2.0, f64::from(size.1) / 2.0);
        let anchor = (hw - f64::from(handle.0) * hw, hh - f64::from(handle.1) * hh);
        let old = was.matrix(size).apply(anchor.0, anchor.1);
        let new = next.matrix(size).apply(anchor.0, anchor.1);
        next.center = (next.center.0 + old.0 - new.0, next.center.1 + old.1 - new.1);
    }
    next.is_valid().then_some(next)
}

/// Shows the active reference's box and handles, and moves, scales and
/// turns it with the pointer.
pub fn place_reference(
    mut editor: ResMut<Editor>,
    mut state: ResMut<ReferenceBox>,
    mut aids: ResMut<Aids>,
    ft: Res<FreeTransform>,
    place: Place,
    mut pointer: PointerInput,
) {
    let editor = &mut *editor;
    let events = pointer.events();
    let active = takes_pointer(editor) && !ft.active() && !place.rig.captures_pointer();
    let reference = editor
        .doc
        .reference(editor.layer)
        .ok()
        .map(|(img, p)| ((img.width(), img.height()), p));
    let Some((size, now)) = reference.filter(|_| active) else {
        // A drag ends with its layer: what it did so far is kept.
        if let Some(d) = state.drag.take() {
            editor.history.commit(&mut editor.doc, d.tx);
        }
        if state.shown.take().is_some() {
            aids.transform_box = None;
            aids.revision += 1;
        }
        return;
    };
    let view = &place.view;
    let mods = place.modifiers.0;
    let over = place.over_stage();
    let layer = editor.layer;
    for event in events {
        match event {
            Pointer::Down {
                at,
                button: PointerButton::Primary,
                ..
            } if state.drag.is_none()
                && over
                && view.contains(at)
                && !place.nav.space_held
                && place.nav.panning.is_none() =>
            {
                if editor.doc.effective_locks(layer).is_ok_and(|l| l.position) {
                    refuse(editor, pixl_document::Error::Locked(layer));
                    continue;
                }
                let p = view.to_doc(view.stage_point(at));
                let grab = match handle_at(&now, size, view, p) {
                    Some(h) => Grab::Handle { at: h },
                    None => match local(&now, size, p) {
                        Some((lx, ly)) if lx.abs() <= 1.0 && ly.abs() <= 1.0 => {
                            Grab::Move { from: p }
                        }
                        _ => Grab::Turn {
                            from: angle_to(&now, p),
                        },
                    },
                };
                let tx = editor
                    .history
                    .begin(&editor.doc, grab.label(), Scope::Images(Vec::new()));
                state.drag = Some(Drag { grab, was: now, tx });
            }
            Pointer::Move { at, .. } => {
                let Some(d) = &state.drag else {
                    continue;
                };
                let p = view.to_doc(view.stage_point(at));
                let was = d.was;
                let next = match d.grab {
                    Grab::Move { from } => Some(ReferencePlace {
                        center: (was.center.0 + p.0 - from.0, was.center.1 + p.1 - from.1),
                        ..was
                    }),
                    Grab::Turn { from } => {
                        let mut a = was.rotation + angle_to(&was, p) - from;
                        if mods.shift {
                            a = (a / 15.0).round() * 15.0;
                        }
                        Some(ReferencePlace {
                            rotation: a.rem_euclid(360.0),
                            ..was
                        })
                    }
                    Grab::Handle { at: h } => scaled(was, size, h, p, mods.shift, mods.alt),
                };
                if let Some(next) = next
                    && editor.doc.set_reference_place(layer, next).is_ok()
                {
                    editor.touch();
                }
            }
            Pointer::Up {
                button: PointerButton::Primary,
                ..
            } => {
                if let Some(d) = state.drag.take() {
                    editor.history.commit(&mut editor.doc, d.tx);
                    editor.touch();
                }
            }
            _ => {}
        }
    }
    // The box follows the place, wherever it was set from.
    let corners = editor
        .doc
        .reference(layer)
        .map_or(now.corners(size), |(_, p)| p.corners(size));
    if state.shown != Some(corners) {
        state.shown = Some(corners);
        aids.transform_box = Some(corners);
        aids.revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    const SIZE: (u32, u32) = (400, 200);

    fn place() -> ReferencePlace {
        ReferencePlace {
            center: (16.0, 16.0),
            scale: (0.05, 0.05),
            rotation: 0.0,
        }
    }

    #[test]
    fn a_corner_scales_evenly_from_the_corner_across() {
        // The box spans (6, 11) to (26, 21); pull the bottom-right corner
        // ten pixels right.
        let next = scaled(place(), SIZE, (1, 1), (36.0, 21.0), false, false).unwrap();
        assert!(near(next.scale, (0.075, 0.075)));
        let c = next.corners(SIZE);
        assert!(near(c[0], (6.0, 11.0)), "the top-left stays: {c:?}");
        assert!(near(c[2], (36.0, 26.0)));
        // Shift frees the proportions.
        let free = scaled(place(), SIZE, (1, 1), (36.0, 21.0), true, false).unwrap();
        assert!(near(free.scale, (0.075, 0.05)));
        // Alt scales from the middle.
        let mid = scaled(place(), SIZE, (1, 0), (36.0, 16.0), false, true).unwrap();
        assert!(near(mid.center, (16.0, 16.0)));
        assert!(near(mid.scale, (0.1, 0.05)));
    }

    #[test]
    fn an_edge_handle_follows_a_turned_image() {
        // Turned a quarter: the image's right edge points down.
        let turned = ReferencePlace {
            rotation: 90.0,
            ..place()
        };
        let next = scaled(turned, SIZE, (1, 0), (16.0, 36.0), false, false).unwrap();
        assert!(near(next.scale, (0.075, 0.05)));
        // Its left edge, now at the top, stays.
        let c = next.corners(SIZE);
        assert!(near(c[0], (21.0, 6.0)), "{c:?}");
    }

    #[test]
    fn a_drag_past_the_far_side_does_not_mirror() {
        let next = scaled(place(), SIZE, (1, 1), (-30.0, -30.0), false, false).unwrap();
        assert!(next.scale.0 > 0.0 && next.scale.1 > 0.0);
    }
}
