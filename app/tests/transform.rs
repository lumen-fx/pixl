//! The free transform tool: lift, move, turn, flip and scale with a live
//! preview, then apply as one undo step or cancel.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{NamedKey, PointerButton};
use pixl::Tool;
use pixl_document::{ColorMode, Document, Rect, Rgba};

const L: PointerButton = PointerButton::Primary;
const RED: Rgba = Rgba::opaque(255, 0, 0);

fn px(doc: &Document, x: i32, y: i32) -> Rgba {
    let layer = doc.layers()[0].id;
    doc.cel_raster(layer, 0)
        .unwrap()
        .map_or(Rgba::TRANSPARENT, |r| Rgba::from_u32(r.get_raw(x, y)))
}

/// A red bar at 4..12 x 6..8 with a white end pixel at (11, 6).
fn bar() -> Document {
    let mut d = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let l = d.layers()[0].id;
    let t = d.target(l, 0).unwrap();
    for y in 6..8 {
        for x in 4..12 {
            t.raster.set_raw(x, y, RED.to_u32());
        }
    }
    t.raster.set_raw(11, 6, Rgba::WHITE.to_u32());
    d
}

fn painted(doc: &Document) -> Rect {
    let layer = doc.layers()[0].id;
    doc.cel_raster(layer, 0).unwrap().unwrap().content_bounds()
}

#[test]
fn flipping_and_applying_is_one_step() {
    let out = drive(
        editor_over(bar()),
        vec![
            Step::Ctrl("t", false),
            Step::Wait,
            Step::Wait,
            Step::Click("xf-flip-h"),
            Step::Wait,
            Step::KeyDown(NamedKey::Enter),
            Step::KeyUp(NamedKey::Enter),
            Step::Wait,
        ],
    );
    assert_eq!(
        px(&out.doc, 4, 6),
        Rgba::WHITE,
        "the white end moved to the left"
    );
    assert_eq!(px(&out.doc, 11, 6), RED);
    assert_eq!(out.undo, vec!["Free transform"]);
    assert_eq!(out.tool, Tool::Move, "the tool hands back to moving");
}

#[test]
fn a_typed_angle_turns_and_escape_cancels() {
    let turned = drive(
        editor_over(bar()),
        vec![
            Step::Ctrl("t", false),
            Step::Wait,
            Step::Wait,
            Step::Text("opt_angle", "90"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&turned.frame, "transform-turned");
    let b = painted(&turned.doc);
    assert_eq!(
        (b.w, b.h),
        (2, 8),
        "a quarter turn stands the bar up: {b:?}"
    );
    let out = drive(
        editor_over(bar()),
        vec![
            Step::Ctrl("t", false),
            Step::Wait,
            Step::Text("opt_angle", "33"),
            Step::Wait,
            Step::KeyDown(NamedKey::Escape),
            Step::KeyUp(NamedKey::Escape),
            Step::Wait,
        ],
    );
    assert_eq!(out.doc, bar());
    assert!(out.undo.is_empty());
}

#[test]
fn dragging_inside_moves_and_an_edge_handle_scales() {
    // Find where the view puts the document, then drag in window terms.
    let probe = drive(editor_over(bar()), vec![Step::Wait]);
    let v = probe.views.last().unwrap().clone();
    let win = |x: f64, y: f64| {
        Vec2::new(
            v.stage.0.x + (v.origin.0 + x * v.zoom) as f32,
            v.stage.0.y + (v.origin.1 + y * v.zoom) as f32,
        )
    };
    let moved = drive(
        editor_over(bar()),
        vec![
            Step::Ctrl("t", false),
            Step::Wait,
            Step::Move(At::Doc(6, 6)),
            Step::Wait,
            Step::Down(At::Doc(6, 6), L),
            Step::Move(At::Doc(8, 10)),
            Step::Up(At::Doc(8, 10), L),
            Step::Click("xf-apply"),
            Step::Wait,
        ],
    );
    assert_eq!(painted(&moved.doc), Rect::new(6, 10, 8, 2));
    // The right edge's handle sits at x 12, y 7 (the middle of the bar).
    let scaled = drive(
        editor_over(bar()),
        vec![
            Step::Ctrl("t", false),
            Step::Text("opt_resample", "Nearest"),
            Step::Wait,
            Step::Move(At::Window(win(12.0, 7.0))),
            Step::Wait,
            Step::Down(At::Window(win(12.0, 7.0)), L),
            Step::Move(At::Window(win(16.0, 7.0))),
            Step::Move(At::Window(win(20.0, 7.0))),
            Step::Up(At::Window(win(20.0, 7.0)), L),
            Step::Click("xf-apply"),
            Step::Wait,
        ],
    );
    common::save(&scaled.frame, "transform-scaled");
    // Twice as wide, the left end staying where it was.
    assert_eq!(painted(&scaled.doc), Rect::new(4, 6, 16, 2));
}

#[test]
fn a_large_box_previews_tile_by_tile_and_applies_the_same_pixels() {
    use pixl_document::transform::draw_transformed;
    use pixl_document::{Affine, Pixels, Point, Resample};
    // Large enough that the preview goes to the workers.
    let side = 640;
    let mut d = Document::new(side, side, ColorMode::Rgba).unwrap();
    let l = d.layers()[0].id;
    let t = d.target(l, 0).unwrap();
    for y in 0..side as i32 {
        for x in 0..side as i32 {
            if (x / 7 + y / 5) % 3 == 0 {
                t.raster.set_raw(x, y, RED.to_u32());
            }
        }
    }
    let mut expected = d.clone();
    let out = drive(
        editor_over(d),
        vec![
            Step::Ctrl("t", false),
            Step::Wait,
            Step::Text("opt_angle", "30"),
            Step::Wait,
            Step::KeyDown(NamedKey::Enter),
            Step::KeyUp(NamedKey::Enter),
            Step::Until(|e| e.history.undo_labels().any(|l| l == "Free transform")),
        ],
    );
    // The same transform drawn at once by pixl-document.
    let t = expected.target(l, 0).unwrap();
    let rect = t.raster.content_bounds();
    let lifted = Pixels::from_raster(t.raster, rect);
    t.raster.fill_rect_raw(&rect, 0);
    let pivot = (
        f64::from(rect.x) + f64::from(rect.w) / 2.0,
        f64::from(rect.y) + f64::from(rect.h) / 2.0,
    );
    let m = Affine::about(pivot, (1.0, 1.0), (0.0, 0.0), 30.0, (0.0, 0.0));
    draw_transformed(
        t.raster,
        &lifted,
        Point::new(rect.x, rect.y),
        &m,
        Resample::RotSprite,
        0,
    );
    let want = expected.cel_raster(l, 0).unwrap().unwrap();
    let got = out.doc.cel_raster(l, 0).unwrap().unwrap();
    let all = Rect::new(0, 0, side as i32, side as i32);
    let differ = all
        .points()
        .filter(|p| want.get_raw(p.x, p.y) != got.get_raw(p.x, p.y))
        .count();
    assert_eq!(differ, 0);
    assert_eq!(out.undo, vec!["Free transform".to_string()]);
}
