//! The Edit menu's clipboard commands, as in Aseprite. Copy puts the
//! selection of the active cel (the cel's content with no selection) on the
//! clipboard as an image; Cut also clears it, as one undo step; Copy merged
//! copies what every visible layer shows there. Paste floats the clipboard's
//! image in the free transform box, at the place it was copied from when
//! that is in view and in the middle of the view otherwise; Paste as new
//! layer puts it on a layer of its own at that place, and Paste as new
//! document opens it in a tab of its own.

use std::result::Result;

use lumenui::ecs::system::SystemParam;
use lumenui::prelude::*;
use pixl_document::{
    ColorMode, CompositeOptions, Document, Error, NewLayer, Pixels, Point, Rect, Rgba, Scope,
    composite,
};

use crate::clipboard::{ClipboardAccess, Copied, Pasted};
use crate::documents::{Documents, TabAction};
use crate::editor::Editor;
use crate::files::Opening;
use crate::menus::Chosen;
use crate::tools::transform::{Floating, FreeTransform};
use crate::view::View;

/// What the clipboard commands wait for: work under way on the document.
#[derive(SystemParam)]
pub struct Busy<'w> {
    gesture: Res<'w, crate::tools::Gesture>,
    toolbox: Res<'w, crate::tools::toolbox::Toolbox>,
    adjust: Res<'w, crate::menus::adjust::Adjust>,
    image: Res<'w, crate::menus::image::ImageMenu>,
}

impl Busy<'_> {
    fn why(&self) -> Option<&'static str> {
        if self.gesture.active() {
            Some("Finish the stroke first")
        } else if self.toolbox.active() {
            Some("Finish the shape first, or drop it with Escape")
        } else if self.adjust.active() {
            Some("Apply or cancel the filter first")
        } else if self.image.busy() {
            Some("Wait for the image edit to finish")
        } else {
            None
        }
    }
}

/// The commands this module runs.
const COMMANDS: [&str; 6] = [
    "edit-cut",
    "edit-copy",
    "edit-copy-merged",
    "edit-paste",
    "edit-paste-layer",
    "edit-paste-document",
];

/// Runs this tick's clipboard commands.
pub fn edit_commands(
    mut editor: ResMut<Editor>,
    chosen: Res<Chosen>,
    mut clipboard: ClipboardAccess,
    mut ft: ResMut<FreeTransform>,
    view: Res<View>,
    mut docs: ResMut<Documents>,
    busy: Busy,
) {
    let editor = &mut *editor;
    for id in chosen.0.iter().filter(|c| COMMANDS.contains(&c.as_str())) {
        if let Some(why) = busy.why() {
            editor.message = Some(why.to_string());
            continue;
        }
        let copying = matches!(id.as_str(), "edit-cut" | "edit-copy" | "edit-copy-merged");
        if copying && ft.active() {
            editor.message =
                Some("Apply the free transform first, or cancel it with Escape".to_string());
            continue;
        }
        let done = match id.as_str() {
            "edit-copy" => copy_cel(editor).map(|c| clipboard.copy(c)),
            "edit-cut" => cut(editor).map(|c| clipboard.copy(c)),
            "edit-copy-merged" => copy_merged(editor).map(|c| clipboard.copy(c)),
            other => match clipboard.paste() {
                None => Err("Nothing to paste".to_string()),
                Some(pasted) => match other {
                    "edit-paste" => paste(editor, &mut ft, &view, &pasted),
                    "edit-paste-layer" => paste_layer(editor, &view, &pasted),
                    _ => paste_document(editor, &pasted)
                        .map(|o| docs.act(TabAction::Open(Box::new(o)))),
                },
            },
        };
        if let Err(e) = done {
            editor.message = Some(e);
        }
    }
}

/// The raw value of an empty pixel.
fn clear_of(doc: &Document) -> u32 {
    match doc.mode() {
        ColorMode::Indexed => u32::from(doc.transparent_index()),
        ColorMode::Rgba => 0,
    }
}

fn refusal(e: Error, verb: &str) -> String {
    match e {
        Error::Locked(_) => "The layer is locked".to_string(),
        Error::NotRaster(_) => format!("Pick a raster layer to {verb}"),
        other => other.to_string(),
    }
}

/// The selection of the active cel, or its content with no selection.
fn copy_cel(editor: &Editor) -> Result<Copied, String> {
    let doc = &editor.doc;
    let raster = doc
        .cel_raster(editor.layer, editor.frame)
        .map_err(|e| refusal(e, "copy from"))?
        .ok_or("Nothing to copy")?;
    let selection = editor.selection.as_ref();
    let rect = selection
        .map_or_else(|| raster.content_bounds(), |s| s.bounds())
        .intersect(&doc.bounds());
    if rect.is_empty() {
        return Err("Nothing to copy".to_string());
    }
    let pixels = Pixels::from_raster_masked(raster, rect, selection, clear_of(doc));
    Ok(Copied::new(
        pixels,
        doc.mode(),
        doc.palette(),
        doc.transparent_index(),
        Point::new(rect.x, rect.y),
    ))
}

/// Copies the selection of the active cel and clears it, as one undo step.
fn cut(editor: &mut Editor) -> Result<Copied, String> {
    let copied = copy_cel(editor)?;
    let (layer, frame) = (editor.layer, editor.frame);
    let image = editor
        .doc
        .target_image(layer, frame)
        .map_err(|e| refusal(e, "cut from"))?;
    let clear = clear_of(&editor.doc);
    let rect = Rect::new(
        copied.at.x,
        copied.at.y,
        copied.pixels.width as i32,
        copied.pixels.height as i32,
    );
    let result = editor.history.transact(
        &mut editor.doc,
        "Cut",
        Scope::Images(vec![image]),
        |doc| -> Result<(), Error> {
            let t = doc.target(layer, frame)?;
            for (p, v) in rect.points().zip(&copied.pixels.data) {
                if *v != clear {
                    t.raster.set_raw(p.x, p.y, clear);
                }
            }
            Ok(())
        },
    );
    result.map_err(|e| refusal(e, "cut from"))?;
    editor.touch_pixels();
    Ok(copied)
}

/// What every visible layer shows in the selection, or the whole visible
/// image, trimmed to what is not clear.
fn copy_merged(editor: &Editor) -> Result<Copied, String> {
    let doc = &editor.doc;
    let selection = editor.selection.as_ref();
    let rect = selection
        .map_or_else(|| doc.bounds(), |s| s.bounds())
        .intersect(&doc.bounds());
    if rect.is_empty() {
        return Err("Nothing to copy".to_string());
    }
    let opts = CompositeOptions {
        background: false,
        onion: None,
    };
    let mut px = composite(doc, editor.frame, rect, &opts);
    if let Some(s) = selection {
        for (p, c) in rect.points().zip(px.iter_mut()) {
            if !s.contains(p.x, p.y) {
                *c = Rgba::TRANSPARENT;
            }
        }
    }
    let (rect, px) = match selection {
        Some(_) => (rect, px),
        None => trimmed(rect, &px).ok_or("Nothing to copy")?,
    };
    let pixels = Pixels {
        width: rect.w as u32,
        height: rect.h as u32,
        data: px
            .iter()
            .map(|c| if c.a == 0 { 0 } else { c.to_u32() })
            .collect(),
    };
    Ok(Copied::new(
        pixels,
        ColorMode::Rgba,
        doc.palette(),
        doc.transparent_index(),
        Point::new(rect.x, rect.y),
    ))
}

/// The part of `px` (the colors of `rect`, row by row) that is not clear,
/// or `None` when all of it is.
fn trimmed(rect: Rect, px: &[Rgba]) -> Option<(Rect, Vec<Rgba>)> {
    let mut seen = Rect::EMPTY;
    for (p, c) in rect.points().zip(px) {
        if c.a != 0 {
            seen.include(p.x, p.y);
        }
    }
    if seen.is_empty() {
        return None;
    }
    let w = rect.w as usize;
    let out = seen
        .points()
        .map(|p| px[(p.y - rect.y) as usize * w + (p.x - rect.x) as usize])
        .collect();
    Some((seen, out))
}

/// The part of the canvas the view shows, in document pixels: left, top,
/// right and bottom. `None` before the stage has a size.
fn shown(view: &View) -> Option<(f64, f64, f64, f64)> {
    if view.stage.1.x <= 0.0 || view.stage.1.y <= 0.0 || view.zoom <= 0.0 {
        return None;
    }
    let (x0, y0, x1, y1) = view.shown_frame();
    let doc = |x: f64, y: f64| {
        (
            (x - view.origin.0) / view.zoom,
            (y - view.origin.1) / view.zoom,
        )
    };
    let (l, t) = doc(x0, y0);
    let (r, b) = doc(x1, y1);
    Some((l, t, r, b))
}

/// Where pasted pixels of `size` go: where they were copied from when any
/// of that is in view, otherwise in the middle of the view (of the canvas,
/// before the view has a size).
pub fn place(view: &View, doc: &Document, size: (u32, u32), origin: Option<Point>) -> Point {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let area = shown(view);
    if let Some(o) = origin {
        let in_view = area.is_none_or(|(l, t, r, b)| {
            let (x, y) = (f64::from(o.x), f64::from(o.y));
            x < r && x + w > l && y < b && y + h > t
        });
        if in_view {
            return o;
        }
    }
    let (cx, cy) = match area {
        Some((l, t, r, b)) => ((l + r) / 2.0, (t + b) / 2.0),
        None => (f64::from(doc.width()) / 2.0, f64::from(doc.height()) / 2.0),
    };
    Point::new((cx - w / 2.0).round() as i32, (cy - h / 2.0).round() as i32)
}

/// Whether the active layer takes pasted pixels.
fn can_paste(editor: &Editor) -> Result<(), String> {
    let layer = editor
        .doc
        .layer(editor.layer)
        .ok_or_else(|| "Pick a raster layer to paste on".to_string())?;
    if !layer.is_raster() {
        return Err("Pick a raster layer to paste on".to_string());
    }
    let locks = editor
        .doc
        .effective_locks(editor.layer)
        .map_err(|e| refusal(e, "paste on"))?;
    if locks.pixels {
        return Err("The layer is locked".to_string());
    }
    Ok(())
}

fn values(doc: &Document, pasted: &Pasted) -> Pixels {
    pasted.values_for(doc.mode(), doc.palette(), doc.transparent_index())
}

/// Floats the pasted pixels in the free transform box on the active cel.
fn paste(
    editor: &mut Editor,
    ft: &mut FreeTransform,
    view: &View,
    pasted: &Pasted,
) -> Result<(), String> {
    can_paste(editor)?;
    let at = place(view, &editor.doc, pasted.size(), pasted.origin());
    ft.float(Floating {
        pixels: values(&editor.doc, pasted),
        at,
        back: editor.tool,
    });
    Ok(())
}

/// Puts the pasted pixels on a new layer above the active one, as one undo
/// step.
fn paste_layer(editor: &mut Editor, view: &View, pasted: &Pasted) -> Result<(), String> {
    let at = place(view, &editor.doc, pasted.size(), pasted.origin());
    let pixels = values(&editor.doc, pasted);
    let clear = clear_of(&editor.doc);
    let (placement, name) = editor.next_layer();
    let frame = editor.frame;
    let id = editor.history.transact(
        &mut editor.doc,
        "Paste as new layer",
        Scope::Images(Vec::new()),
        |doc| {
            let id = doc.add_layer(NewLayer::Raster, name, placement)?;
            let t = doc.target(id, frame)?;
            pixels.write_to(t.raster, at, Some(clear));
            Ok::<_, Error>(id)
        },
    );
    editor.layer = id.map_err(|e| refusal(e, "paste on"))?;
    editor.selection = None;
    editor.touch();
    Ok(())
}

/// A new document the size of the pasted pixels, holding them: indexed on
/// their palette when pixl copied them from an indexed document, RGBA
/// otherwise.
fn paste_document(window: &Editor, pasted: &Pasted) -> Result<Opening, String> {
    let (w, h) = pasted.size();
    let mode = match pasted.palette() {
        Some(_) => ColorMode::Indexed,
        None => ColorMode::Rgba,
    };
    let mut doc = Document::new(w, h, mode).map_err(|e| e.to_string())?;
    if let Some((palette, ti)) = pasted.palette() {
        doc.set_palette(palette.clone());
        doc.set_transparent_index(*ti);
    }
    let pixels = values(&doc, pasted);
    let layer = doc.layers()[0].id;
    let t = doc.target(layer, 0).map_err(|e| e.to_string())?;
    pixels.write_to(t.raster, Point::new(0, 0), None);
    Ok(Opening::new(Editor::opened(doc, None, window)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumenui::glam::Vec2;

    fn view() -> View {
        let mut v = View::default();
        v.zoom = 4.0;
        v.origin = (0.0, 0.0);
        v.stage = (Vec2::ZERO, Vec2::new(64.0, 64.0));
        v
    }

    #[test]
    fn a_paste_keeps_its_place_in_view_and_centers_out_of_it() {
        let doc = Document::new(64, 64, ColorMode::Rgba).unwrap();
        // The view shows document pixels 0..16 on each axis.
        let v = view();
        assert_eq!(
            place(&v, &doc, (4, 4), Some(Point::new(10, 2))),
            Point::new(10, 2)
        );
        assert_eq!(
            place(&v, &doc, (4, 4), Some(Point::new(40, 40))),
            Point::new(6, 6)
        );
        assert_eq!(place(&v, &doc, (4, 4), None), Point::new(6, 6));
    }

    #[test]
    fn trimming_keeps_what_is_not_clear() {
        let rect = Rect::new(0, 0, 3, 2);
        let red = Rgba::opaque(255, 0, 0);
        let px = vec![
            Rgba::TRANSPARENT,
            Rgba::TRANSPARENT,
            Rgba::TRANSPARENT,
            Rgba::TRANSPARENT,
            red,
            red,
        ];
        let (r, out) = trimmed(rect, &px).unwrap();
        assert_eq!(r, Rect::new(1, 1, 2, 1));
        assert_eq!(out, vec![red, red]);
        assert!(trimmed(rect, &[Rgba::TRANSPARENT; 6]).is_none());
    }
}
