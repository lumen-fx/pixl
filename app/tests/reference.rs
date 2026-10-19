//! Reference layers placed through the editor: the box's move, scale and
//! turn handles driven by the pointer, the place fields of the layer
//! properties, and the eyedropper reading the art beneath a reference.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl::Editor;
use pixl_document::{ColorMode, Document, NewLayer, Placement, ReferencePlace, Rgba, TiledImage};

const L: PointerButton = PointerButton::Primary;
const RED: Rgba = Rgba::opaque(255, 0, 0);
const BLUE: Rgba = Rgba::opaque(0, 0, 255);

/// The reference's image size.
const SIZE: (u32, u32) = (400, 200);

/// A 32x32 document: a red art pixel at (10, 15), and over it a blue
/// 400x200 reference shown 20x10, its box from (6.5, 11.5) to
/// (26.5, 21.5), so its corners and middle fall on pixel middles.
fn doc() -> Document {
    let mut d = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let art = d.layers()[0].id;
    d.target(art, 0)
        .unwrap()
        .raster
        .set_raw(10, 15, RED.to_u32());
    d.add_layer(
        NewLayer::Reference {
            image: TiledImage::new(SIZE.0, SIZE.1, BLUE),
            place: ReferencePlace {
                center: (16.5, 16.5),
                scale: (0.05, 0.05),
                rotation: 0.0,
            },
        },
        "photo",
        Placement::default(),
    )
    .unwrap();
    d
}

fn pick_reference(e: &mut Editor) {
    e.layer = e.doc.layers().last().unwrap().id;
}

fn lock_position(e: &mut Editor) {
    let id = e.doc.layers().last().unwrap().id;
    e.doc.update_layer(id, |p| p.locks.position = true).unwrap();
}

fn place(doc: &Document) -> ReferencePlace {
    doc.reference(doc.layers().last().unwrap().id).unwrap().1
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn a_drag_inside_the_box_moves_it_as_one_step() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Edit(pick_reference),
            Step::Wait,
            Step::Down(At::Doc(16, 16), L),
            Step::Move(At::Doc(18, 17)),
            Step::Move(At::Doc(20, 18)),
            Step::Up(At::Doc(20, 18), L),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "reference-moved");
    let p = place(&out.doc);
    assert!(near(p.center.0, 20.5) && near(p.center.1, 18.5), "{p:?}");
    assert_eq!(p.scale, (0.05, 0.05));
    assert_eq!(out.undo, vec!["Move reference"]);
    // The pencil, the tool in hand, painted nothing.
    assert_eq!(
        out.doc
            .cel_raster(out.doc.layers()[0].id, 0)
            .unwrap()
            .unwrap()
            .content_bounds(),
        pixl_document::Rect::new(10, 15, 1, 1)
    );
}

#[test]
fn undo_puts_a_moved_reference_back() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Edit(pick_reference),
            Step::Wait,
            Step::Down(At::Doc(16, 16), L),
            Step::Move(At::Doc(20, 18)),
            Step::Up(At::Doc(20, 18), L),
            Step::Wait,
            Step::Ctrl("z", false),
            Step::Wait,
        ],
    );
    assert_eq!(place(&out.doc).center, (16.5, 16.5));
}

#[test]
fn a_corner_scales_it_keeping_its_proportions() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Edit(pick_reference),
            Step::Wait,
            // The bottom-right corner, pulled ten pixels right.
            Step::Down(At::Doc(26, 21), L),
            Step::Move(At::Doc(31, 21)),
            Step::Move(At::Doc(36, 21)),
            Step::Up(At::Doc(36, 21), L),
            Step::Wait,
        ],
    );
    let p = place(&out.doc);
    assert!(near(p.scale.0, 0.075) && near(p.scale.1, 0.075), "{p:?}");
    let c = p.corners(SIZE);
    assert!(
        near(c[0].0, 6.5) && near(c[0].1, 11.5),
        "the top-left stays"
    );
    assert_eq!(out.undo, vec!["Scale reference"]);
}

#[test]
fn a_drag_outside_the_box_turns_it() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Edit(pick_reference),
            Step::Wait,
            Step::Shift(true),
            // From straight below the middle to straight right of it.
            Step::Down(At::Doc(16, 30), L),
            Step::Move(At::Doc(25, 26)),
            Step::Move(At::Doc(30, 16)),
            Step::Up(At::Doc(30, 16), L),
            Step::Shift(false),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "reference-turned");
    let p = place(&out.doc);
    assert!(near(p.rotation, 270.0), "{p:?}");
    assert_eq!(p.center, (16.5, 16.5));
    assert_eq!(out.undo, vec!["Turn reference"]);
}

#[test]
fn a_position_lock_keeps_it_in_place() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Edit(pick_reference),
            Step::Edit(lock_position),
            Step::Wait,
            Step::Down(At::Doc(16, 16), L),
            Step::Move(At::Doc(20, 18)),
            Step::Up(At::Doc(20, 18), L),
            Step::Wait,
        ],
    );
    assert_eq!(place(&out.doc).center, (16.5, 16.5));
    assert_eq!(out.message.as_deref(), Some("The layer is locked"));
}

#[test]
fn the_properties_dialog_places_it_by_numbers() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Edit(pick_reference),
            Step::Wait,
            Step::Click("tl-edit-layer"),
            Step::Wait,
            Step::Wait,
            Step::Text("lp_ref_x", "10"),
            Step::Text("lp_ref_w", "40"),
            Step::Text("lp_ref_angle", "45"),
            Step::Click("lp-ok"),
            Step::Wait,
        ],
    );
    let p = place(&out.doc);
    assert_eq!(p.center, (10.0, 16.5));
    assert!(near(p.scale.0, 0.1), "40 pixels wide: {p:?}");
    assert_eq!(p.scale.1, 0.05, "the height stays");
    assert_eq!(p.rotation, 45.0);
    assert_eq!(out.undo, vec!["Layer properties"]);
}

#[test]
fn the_properties_dialog_unchanged_moves_nothing() {
    let mut d = doc();
    let id = d.layers().last().unwrap().id;
    // A place whose numbers do not round to two decimals.
    let odd = ReferencePlace {
        center: (16.123_456, 16.987_654),
        scale: (0.033_333, 0.041_111),
        rotation: 12.345_678,
    };
    d.set_reference_place(id, odd).unwrap();
    let out = drive(
        editor_over(d),
        vec![
            Step::Edit(pick_reference),
            Step::Wait,
            Step::Click("tl-edit-layer"),
            Step::Wait,
            Step::Wait,
            Step::Click("lp-ok"),
            Step::Wait,
        ],
    );
    assert_eq!(place(&out.doc), odd);
    assert!(out.undo.is_empty(), "{:?}", out.undo);
}

#[test]
fn the_eyedropper_reads_the_art_under_a_reference() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Click("tool-eyedropper"),
            Step::Wait,
            // The red art pixel, under the opaque blue reference.
            Step::Down(At::Doc(10, 15), L),
            Step::Up(At::Doc(10, 15), L),
            Step::Wait,
            // Only the reference shows here: the art is transparent, so the
            // color stays.
            Step::Down(At::Doc(16, 16), L),
            Step::Up(At::Doc(16, 16), L),
            Step::Wait,
            // With the reference layer active it reads the art too.
            Step::Edit(pick_reference),
            Step::Wait,
            Step::Down(At::Doc(20, 20), L),
            Step::Up(At::Doc(20, 20), L),
            Step::Wait,
        ],
    );
    assert_eq!(out.fg, RED);
    assert_eq!(
        place(&out.doc).center,
        (16.5, 16.5),
        "the eyedropper moves nothing"
    );
}
