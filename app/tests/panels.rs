//! The panels, driven by clicks and control values over a rendered editor.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl_core::{BlendMode, ColorMode, Document, LayerId, Rgba};

fn doc() -> Document {
    Document::new(16, 16, ColorMode::Rgba).unwrap()
}

/// Root layer ids, top first.
fn stack(doc: &Document) -> Vec<LayerId> {
    doc.layers().iter().rev().map(|l| l.id).collect()
}

#[test]
fn layers_add_reorder_hide_and_delete_as_undo_steps() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("layer-add"),
            Step::Click("layer-add"),
            Step::Click("layer-down"),
            Step::Click("layer-eye-1"),
            Step::Click("layer-row-2"),
            Step::Click("layer-delete"),
        ],
    );
    // Layer 1 at the bottom, then 2 and 3 above it; 3 moved below 2, then
    // 2 was deleted.
    assert_eq!(stack(&out.doc), vec![LayerId(3), LayerId(1)]);
    assert!(!out.doc.layer(LayerId(1)).unwrap().props.visible);
    assert_eq!(
        out.undo,
        [
            "New layer",
            "New layer",
            "Lower layer",
            "Visibility",
            "Delete layer"
        ]
    );
}

#[test]
fn a_slider_drag_of_opacity_is_one_undo_step() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Number("layer_opacity", 80.0),
            Step::Number("layer_opacity", 60.0),
            Step::Number("layer_opacity", 40.0),
            Step::Up(
                At::Window(lumenui::glam::Vec2::ZERO),
                PointerButton::Primary,
            ),
            Step::Text("layer_blend", "Multiply"),
        ],
    );
    let layer = out.doc.layers()[0].props.clone();
    assert_eq!(layer.opacity, 102, "40 percent");
    assert_eq!(layer.blend, BlendMode::Multiply);
    assert_eq!(out.undo, ["Opacity", "Blend mode"]);
}

#[test]
fn undo_redo_and_history_jumps() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("layer-add"),
            Step::Click("layer-add"),
            Step::Click("layer-add"),
            Step::Ctrl("z", false),
            Step::Ctrl("z", false),
            Step::Ctrl("z", true),
            Step::Click("history-1"),
            Step::Wait,
        ],
    );
    assert_eq!(out.doc.layers().len(), 2, "back to one added layer");
    assert_eq!(out.undo, ["New layer"]);
}

#[test]
fn swatches_and_channels_set_the_foreground() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("swatch-4"),
            Step::Wait,
            Step::Number("color_g", 200.0),
            Step::Wait,
            Step::Click("color-swap"),
        ],
    );
    assert_eq!(out.bg, Rgba::opaque(0xbe, 200, 0x33));
    assert_eq!(out.fg, Rgba::WHITE);
}
