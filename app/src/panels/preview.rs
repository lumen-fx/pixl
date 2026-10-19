//! The preview beside the timeline: the document at actual size, or fitted
//! to the panel, playing along with the canvas.
//!
//! It draws the canvas view's own layer buffers, scaled, so it composites
//! nothing itself and costs one more draw of each buffer.

use lumenui::glam::Vec2;
use lumenui::lumen_core::components::Transform;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use lumenui_canvas::{Canvas, Context2d};

use crate::editor::Editor;
use crate::ids::ElementIds;
use crate::tools::set_if_changed;
use crate::view::{Layers, View, ViewColors};

/// How the preview shows the document.
#[derive(Resource, Default)]
pub struct PreviewPane {
    /// Fit the document to the panel instead of one document pixel per
    /// screen pixel.
    pub fit: bool,
    drawn: Option<Drawn>,
}

impl PreviewPane {
    /// Hands over how the preview shows the document in front.
    pub fn take_document(&mut self) -> PreviewPane {
        std::mem::take(self)
    }

    /// Puts back how the preview showed the document coming to the front.
    pub fn restore_document(&mut self, mut saved: PreviewPane) {
        saved.drawn = None;
        *self = saved;
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Drawn {
    size: (f32, f32),
    art: u64,
    doc: (u32, u32),
    fit: bool,
    colors: u64,
}

/// Where the document sits in a preview of `size`: centered, at actual size
/// or fitted. A document larger than the panel always fits.
pub fn preview_view(doc: (u32, u32), size: Vec2, fit: bool) -> View {
    let (w, h) = (f64::from(doc.0), f64::from(doc.1));
    let (sw, sh) = (f64::from(size.x), f64::from(size.y));
    let most = (sw / w).min(sh / h);
    let zoom = if most < 1.0 {
        most
    } else if fit {
        // Whole steps keep every document pixel the same size.
        most.floor()
    } else {
        1.0
    };
    let mut view = View::default();
    view.zoom = zoom;
    view.origin = (
        ((sw - w * zoom) / 2.0).round(),
        ((sh - h * zoom) / 2.0).round(),
    );
    view.stage = (Vec2::ZERO, size);
    view
}

/// The zoom button.
pub fn preview_controls(
    mut pane: ResMut<PreviewPane>,
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
) {
    for c in clicks.read() {
        if ids.of(c.entity).is_ok_and(|id| id.0 == "pv-zoom") {
            pane.fit = !pane.fit;
        }
    }
}

/// Redraws the preview when the canvas's buffers or the panel changed.
pub fn draw_preview(
    editor: Res<Editor>,
    layers: Res<Layers>,
    colors: Res<ViewColors>,
    mut pane: ResMut<PreviewPane>,
    canvases: Query<(&Canvas, &Transform)>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    set_if_changed(
        &mut signals,
        "pv_zoom_label",
        if pane.fit { "Fit" } else { "1:1" }.to_string(),
    );
    let Some((_, t)) = canvases.iter().find(|(c, _)| c.id == "preview") else {
        return;
    };
    let size = (t.size.x, t.size.y);
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return;
    }
    let doc = &editor.doc;
    let now = Drawn {
        size,
        art: layers.art_revision,
        doc: (doc.width(), doc.height()),
        fit: pane.fit,
        colors: colors.revision,
    };
    if pane.drawn.as_ref() == Some(&now) {
        return;
    }
    let ctx = Context2d::new("preview");
    if pane.drawn.as_ref().map(|d| d.size) != Some(size) {
        ctx.resize(size.0, size.1);
    }
    ctx.clear();
    let view = preview_view(now.doc, t.size, pane.fit);
    if let Some(c) = colors.checker_a {
        let (x, y, w, h) = view.doc_rect(now.doc.0, now.doc.1);
        ctx.set_fill_color(lumenui_canvas::Rgba::new(
            f64::from(c.r) / 255.0,
            f64::from(c.g) / 255.0,
            f64::from(c.b) / 255.0,
            1.0,
        ));
        ctx.fill_rect(x, y, w, h);
    }
    layers.draw_document(&ctx, doc, &view, false, true);
    pane.drawn = Some(now);
    crate::repaint_soon(&wake);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_documents_show_at_actual_size_or_whole_steps() {
        let v = preview_view((32, 16), Vec2::new(200.0, 100.0), false);
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.origin, (84.0, 42.0));
        assert_eq!(
            preview_view((32, 16), Vec2::new(200.0, 100.0), true).zoom,
            6.0
        );
        let big = preview_view((4096, 4096), Vec2::new(200.0, 100.0), false);
        assert!((big.zoom - 100.0 / 4096.0).abs() < 1e-9);
    }
}
