//! The Image menu: crop, canvas size and image size, flips and turns of the
//! whole canvas, the color mode, and replacing a color in every cel.

use pixl_core::{Rect, Scope, crop};

use crate::editor::Editor;

/// Crops the canvas to `rect` as one undo step.
pub fn crop_to(editor: &mut Editor, rect: Rect) {
    let rect = rect.intersect(&editor.doc.bounds());
    if rect.is_empty() {
        return;
    }
    let done = editor
        .history
        .transact(&mut editor.doc, "Crop", Scope::All, |d| crop(d, rect));
    match done {
        Ok(_) => after_resize(editor),
        Err(e) => editor.message = Some(e.to_string()),
    }
}

/// What every change of the canvas size leaves behind: no selection, the
/// symmetry centered on the new canvas.
pub fn after_resize(editor: &mut Editor) {
    editor.selection = None;
    let (w, h) = (editor.doc.width(), editor.doc.height());
    editor.symmetry = pixl_core::Symmetry::centered(editor.symmetry.mode, w, h);
    editor.touch();
}
