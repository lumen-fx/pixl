//! Slices: named rectangles of the canvas, each with an optional 9-slice
//! center and pivot, made with the slice tool and edited in their dialog.

use pixl_document::{Rect, Rgba, Scope, Slice};

use crate::editor::Editor;

/// Adds a slice over `rect` as one undo step.
pub fn add(editor: &mut Editor, rect: Rect) {
    if rect.is_empty() {
        return;
    }
    let name = format!("Slice {}", editor.doc.slices().len() + 1);
    editor
        .history
        .transact(&mut editor.doc, "New slice", Scope::All, |d| {
            d.slices_mut().push(Slice {
                name,
                bounds: rect,
                center: None,
                pivot: None,
                color: Rgba::opaque(0x31, 0xa2, 0xf2),
            })
        });
    editor.touch();
}
