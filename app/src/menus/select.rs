//! The Select menu: select all, none and the inverse, select by color or by
//! a layer's content, grow, shrink and border by a number of pixels, and
//! selections saved by name to load again (added to, subtracted from or
//! intersected with the current one). Saved selections last as long as the
//! editor runs.

use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_document::{Rgba, SelectOp, Selection};

use crate::editor::Editor;
use crate::menus::Chosen;
use crate::panels::escape;

/// What the pixels dialog is asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Amount {
    Grow,
    Shrink,
    Border,
}

/// Saved selections and the dialogs' state.
#[derive(Resource, Default)]
pub struct Selections {
    pub saved: Vec<(String, Selection)>,
    /// The saved selection last loaded, which Delete removes.
    picked: Option<usize>,
    asking: Option<Amount>,
    shown: Option<String>,
}

impl Selections {
    /// Hands over the saved selections of the document in front, leaving
    /// none for the next.
    pub fn take_document(&mut self) -> Selections {
        std::mem::take(self)
    }

    /// Puts back the saved selections of the document coming to the front.
    pub fn restore_document(&mut self, mut saved: Selections) {
        saved.asking = None;
        saved.shown = None;
        *self = saved;
    }
}

/// Every pixel of the active cel that is not clear: the layer's content.
pub fn layer_content(editor: &Editor) -> Option<Selection> {
    let (w, h) = (editor.doc.width(), editor.doc.height());
    let raster = editor.doc.cel_raster(editor.layer, editor.frame).ok()??;
    let clear = match editor.doc.mode() {
        pixl_document::ColorMode::Indexed => u32::from(editor.doc.transparent_index()),
        pixl_document::ColorMode::Rgba => 0,
    };
    let b = raster.content_bounds();
    let s = Selection::from_points(
        w,
        h,
        b.points().filter(|p| {
            let v = raster.get_raw(p.x, p.y);
            match editor.doc.mode() {
                pixl_document::ColorMode::Rgba => Rgba::from_u32(v).a > 0,
                pixl_document::ColorMode::Indexed => v != clear,
            }
        }),
    );
    (!s.is_empty()).then_some(s)
}

/// Every pixel of the active cel within the tolerance of `color`.
pub fn by_color(editor: &Editor, color: Rgba) -> Option<Selection> {
    let (w, h) = (editor.doc.width(), editor.doc.height());
    let raster = editor.doc.cel_raster(editor.layer, editor.frame).ok()??;
    let palette = editor.doc.palette();
    let ti = editor.doc.transparent_index();
    let tolerance = editor.options.tolerance;
    let rgba = |v: u32| match raster {
        pixl_document::Raster::Indexed(_) => {
            if v as u8 == ti {
                Rgba::TRANSPARENT
            } else {
                palette.get(v as u8)
            }
        }
        _ => Rgba::from_u32(v),
    };
    let s = Selection::from_points(
        w,
        h,
        editor
            .doc
            .bounds()
            .points()
            .filter(|p| rgba(raster.get_raw(p.x, p.y)).distance(color) <= tolerance),
    );
    (!s.is_empty()).then_some(s)
}

fn op_named(name: &str) -> SelectOp {
    match name {
        "Add" => SelectOp::Add,
        "Subtract" => SelectOp::Subtract,
        "Intersect" => SelectOp::Intersect,
        _ => SelectOp::Replace,
    }
}

/// Runs the Select menu's commands and its two dialogs.
pub fn select_commands(
    chosen: Res<Chosen>,
    mut editor: ResMut<Editor>,
    mut sel: ResMut<Selections>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    let editor = &mut *editor;
    let (w, h) = (editor.doc.width(), editor.doc.height());
    for id in &chosen.0 {
        match id.as_str() {
            "select-all" => editor.selection = Some(Selection::all(w, h)),
            "select-none" => editor.selection = None,
            "select-invert" => {
                let mut s = editor
                    .selection
                    .clone()
                    .unwrap_or_else(|| Selection::empty(w, h));
                s.invert();
                editor.selection = (!s.is_empty()).then_some(s);
            }
            "select-color" => editor.selection = by_color(editor, editor.fg),
            "select-layer" => editor.selection = layer_content(editor),
            "select-grow" | "select-shrink" | "select-border" => {
                let (amount, title) = match id.as_str() {
                    "select-grow" => (Amount::Grow, "Grow the selection"),
                    "select-shrink" => (Amount::Shrink, "Shrink the selection"),
                    _ => (Amount::Border, "Border of the selection"),
                };
                if editor.selection.is_none() {
                    editor.message = Some("Nothing is selected".into());
                    continue;
                }
                sel.asking = Some(amount);
                signals.set("amount_title", title);
                signals.set("amount_open", true);
            }
            "amount-ok" => {
                signals.set("amount_open", false);
                let n = signals
                    .get::<String>("amount_value")
                    .and_then(|t| t.trim().parse::<u32>().ok())
                    .unwrap_or(1)
                    .clamp(1, 4096);
                if let (Some(a), Some(s)) = (sel.asking.take(), editor.selection.as_mut()) {
                    match a {
                        Amount::Grow => s.grow(n),
                        Amount::Shrink => s.shrink(n),
                        Amount::Border => s.border(n),
                    }
                    if s.is_empty() {
                        editor.selection = None;
                    }
                }
            }
            "amount-cancel" => {
                signals.set("amount_open", false);
                sel.asking = None;
            }
            "select-save" => match &editor.selection {
                Some(s) => {
                    let name = format!("Selection {}", sel.saved.len() + 1);
                    editor.message = Some(format!("Saved as {name}"));
                    sel.saved.push((name, s.clone()));
                }
                None => editor.message = Some("Nothing is selected".into()),
            },
            "select-load" => signals.set("selections_open", true),
            "selections-close" => signals.set("selections_open", false),
            "selections-delete" => {
                if let Some(i) = sel.picked.take()
                    && i < sel.saved.len()
                {
                    sel.saved.remove(i);
                }
            }
            other => {
                if let Some(i) = other
                    .strip_prefix("saved-sel-")
                    .and_then(|n| n.parse::<usize>().ok())
                    && let Some((_, s)) = sel.saved.get(i)
                {
                    // A saved selection from a canvas of another size does
                    // not fit this one.
                    if (s.width(), s.height()) != (w, h) {
                        editor.message = Some("That selection is for another canvas size".into());
                        continue;
                    }
                    let op = op_named(&signals.get::<String>("selections_op").unwrap_or_default());
                    let mut next = match (&editor.selection, op) {
                        (Some(cur), op) if op != SelectOp::Replace => cur.clone(),
                        _ => Selection::empty(w, h),
                    };
                    next.combine(s, op);
                    editor.selection = (!next.is_empty()).then_some(next);
                    sel.picked = Some(i);
                }
            }
        }
    }
    let rows: String = sel
        .saved
        .iter()
        .enumerate()
        .map(|(i, (name, s))| {
            let picked = if sel.picked == Some(i) { " current" } else { "" };
            let b = s.bounds();
            format!(
                r#"<row id="saved-sel-{i}" class="ws-layout{picked}"><label class="ws-panel-name" text="{}"/><label class="ws-layout-kind" text="{} x {}"/></row>"#,
                escape(name),
                b.w,
                b.h
            )
        })
        .collect();
    if sel.shown.as_deref() != Some(rows.as_str())
        && let Some(list) = dom::get_by_id("selections-list")
    {
        list.set_inner_markup(&rows);
        sel.shown = Some(rows);
        crate::repaint_soon(&wake);
    }
}
