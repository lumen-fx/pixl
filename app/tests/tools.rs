//! Every tool, driven by pointer and key messages over a rendered editor and
//! checked in the document it leaves.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl::Tool;
use pixl_core::{ColorMode, Document, Point, Rgba, Selection};

const BLACK: Rgba = Rgba::BLACK;
const L: PointerButton = PointerButton::Primary;

fn doc() -> Document {
    Document::new(32, 32, ColorMode::Rgba).unwrap()
}

fn px(doc: &Document, x: i32, y: i32) -> Rgba {
    let layer = doc.layers()[0].id;
    doc.cel_raster(layer, 0)
        .unwrap()
        .map_or(Rgba::TRANSPARENT, |r| Rgba::from_u32(r.get_raw(x, y)))
}

fn painted(doc: &Document) -> usize {
    (0..32)
        .flat_map(|y| (0..32).map(move |x| (x, y)))
        .filter(|&(x, y)| px(doc, x, y).a > 0)
        .count()
}

/// The pointer arriving over `a`: the window hit-tests a move on the tick
/// after it, so the element under the pointer is known by the press.
fn hover(a: (i32, i32)) -> Vec<Step> {
    vec![Step::Move(At::Doc(a.0, a.1)), Step::Wait]
}

/// A drag from `a` to `b` through the points between.
fn drag(a: (i32, i32), b: (i32, i32), button: PointerButton) -> Vec<Step> {
    let mut steps = hover(a);
    steps.push(Step::Down(At::Doc(a.0, a.1), button));
    for i in 1..=4 {
        let x = a.0 + (b.0 - a.0) * i / 4;
        let y = a.1 + (b.1 - a.1) * i / 4;
        steps.push(Step::Move(At::Doc(x, y)));
    }
    steps.push(Step::Up(At::Doc(b.0, b.1), button));
    steps
}

fn with(mut first: Vec<Step>, rest: Vec<Step>) -> Vec<Step> {
    first.extend(rest);
    first
}

#[test]
fn the_pencil_draws_a_pixel_perfect_stroke_as_one_undo_step() {
    let out = drive(editor_over(doc()), drag((2, 2), (12, 7), L));
    assert_eq!(px(&out.doc, 2, 2), BLACK);
    assert_eq!(px(&out.doc, 12, 7), BLACK);
    // A one-pixel line from (2,2) to (12,7) covers 11 columns once each.
    assert_eq!(painted(&out.doc), 11);
    assert_eq!(out.undo, vec!["Pencil".to_string()]);
    common::save(&out.frame, "tool-pencil");
}

#[test]
fn the_secondary_button_paints_the_background_color() {
    let out = drive(
        editor_over(doc()),
        with(
            vec![Step::Key("b", false)],
            drag((5, 5), (5, 5), PointerButton::Secondary),
        ),
    );
    assert_eq!(px(&out.doc, 5, 5), Rgba::WHITE);
}

#[test]
fn the_eraser_clears_what_the_pencil_drew() {
    let steps = with(
        drag((0, 10), (20, 10), L),
        with(vec![Step::Key("e", false)], drag((5, 10), (8, 10), L)),
    );
    let out = drive(editor_over(doc()), steps);
    assert_eq!(out.tool, Tool::Eraser);
    assert_eq!(px(&out.doc, 4, 10), BLACK);
    assert_eq!(px(&out.doc, 6, 10).a, 0);
    assert_eq!(px(&out.doc, 9, 10), BLACK);
}

#[test]
fn the_line_tool_draws_on_release() {
    let out = drive(
        editor_over(doc()),
        with(vec![Step::Key("l", false)], drag((1, 1), (20, 1), L)),
    );
    assert_eq!(painted(&out.doc), 20);
    assert_eq!(out.undo, vec!["Line".to_string()]);
}

#[test]
fn shift_snaps_a_rectangle_to_a_square() {
    let steps = with(
        vec![Step::Key("u", false), Step::Shift(true)],
        drag((2, 2), (10, 6), L),
    );
    let out = drive(editor_over(doc()), steps);
    assert_eq!(out.tool, Tool::Rectangle);
    assert_eq!(px(&out.doc, 10, 10), BLACK, "the corner of a square");
    assert_eq!(px(&out.doc, 6, 6).a, 0, "an outline, not filled");
}

#[test]
fn shift_u_picks_the_ellipse() {
    let out = drive(
        editor_over(doc()),
        with(vec![Step::Key("u", true)], drag((4, 4), (20, 14), L)),
    );
    assert_eq!(out.tool, Tool::Ellipse);
    assert_eq!(px(&out.doc, 12, 4), BLACK, "top of the ellipse");
    assert_eq!(px(&out.doc, 4, 4).a, 0, "not the corner of the box");
}

#[test]
fn the_bucket_fills_inside_an_outline() {
    let steps = with(
        with(vec![Step::Key("u", false)], drag((2, 2), (12, 12), L)),
        vec![
            Step::Key("g", false),
            Step::Move(At::Doc(6, 6)),
            Step::Wait,
            Step::Down(At::Doc(6, 6), L),
            Step::Up(At::Doc(6, 6), L),
        ],
    );
    let out = drive(editor_over(doc()), steps);
    assert_eq!(px(&out.doc, 6, 6), BLACK);
    assert_eq!(px(&out.doc, 20, 20).a, 0, "outside stays empty");
    assert_eq!(painted(&out.doc), 11 * 11);
}

#[test]
fn the_eyedropper_picks_the_foreground_from_the_document() {
    let mut d = doc();
    let layer = d.layers()[0].id;
    d.target(layer, 0)
        .unwrap()
        .raster
        .set_raw(7, 7, Rgba::opaque(10, 200, 30).to_u32());
    let out = drive(
        editor_over(d),
        vec![
            Step::Key("i", false),
            Step::Move(At::Doc(7, 7)),
            Step::Wait,
            Step::Down(At::Doc(7, 7), L),
            Step::Up(At::Doc(7, 7), L),
        ],
    );
    assert_eq!(out.fg, Rgba::opaque(10, 200, 30));
}

#[test]
fn a_selection_masks_the_pencil() {
    let steps = with(
        with(vec![Step::Key("m", false)], drag((0, 0), (9, 31), L)),
        with(vec![Step::Key("b", false)], drag((0, 15), (31, 15), L)),
    );
    let out = drive(editor_over(doc()), steps);
    let sel = out.selection.expect("a selection");
    assert_eq!(sel.bounds().w, 10);
    assert_eq!(px(&out.doc, 5, 15), BLACK);
    assert_eq!(px(&out.doc, 20, 15).a, 0, "outside the selection");
}

#[test]
fn the_move_tool_carries_the_selected_pixels() {
    let steps = with(
        drag((2, 2), (4, 2), L),
        with(
            with(vec![Step::Key("m", false)], drag((1, 1), (5, 3), L)),
            with(vec![Step::Key("v", false)], drag((3, 2), (13, 12), L)),
        ),
    );
    let out = drive(editor_over(doc()), steps);
    assert_eq!(px(&out.doc, 2, 2).a, 0, "lifted from where it was");
    assert_eq!(
        px(&out.doc, 12, 12),
        BLACK,
        "dropped ten pixels down and right"
    );
    assert_eq!(px(&out.doc, 14, 12), BLACK);
    assert_eq!(out.selection.unwrap().bounds().x, 11);
    assert_eq!(out.undo.last().map(String::as_str), Some("Move"));
}

#[test]
fn pen_pressure_sizes_the_brush() {
    let steps = vec![
        Step::Edit(|e| e.options.size = 8),
        Step::Move(At::Doc(2, 16)),
        Step::Wait,
        Step::PenDown(At::Doc(2, 16), 0.1),
        Step::PenMove(At::Doc(8, 16), 0.1),
        Step::PenMove(At::Doc(16, 16), 1.0),
        Step::PenMove(At::Doc(24, 16), 1.0),
        Step::PenUp(At::Doc(24, 16)),
    ];
    let out = drive(editor_over(doc()), steps);
    let column = |x: i32| (0..32).filter(|&y| px(&out.doc, x, y).a > 0).count();
    assert_eq!(column(4), 1, "light pressure paints one pixel");
    assert!(column(20) >= 7, "full pressure paints the full brush");
}

#[test]
fn painting_shows_on_screen_on_the_same_tick() {
    let out = drive(editor_over(doc()), drag((3, 3), (3, 3), L));
    let v = out.views.last().unwrap();
    let at = pixl::tools::screen_of(v, pixl_core::Point::new(3, 3));
    assert_eq!(
        out.frame.pixel(at.x as u32, at.y as u32),
        Some([0, 0, 0, 255])
    );
}

#[test]
fn a_locked_layer_refuses_a_stroke_and_gains_no_cel() {
    let mut d = doc();
    let layer = d.layers()[0].id;
    d.update_layer(layer, |p| p.locks.pixels = true).unwrap();
    // The pencil, the eraser and the spray (a toolbox tool).
    for tool in ["b", "e", "j"] {
        let steps = with(vec![Step::Key(tool, false)], drag((2, 2), (12, 7), L));
        let out = drive(editor_over(d.clone()), steps);
        assert_eq!(out.doc.cel(layer, 0).unwrap(), None, "{tool}");
        assert!(out.undo.is_empty(), "{tool}: {:?}", out.undo);
        assert_eq!(
            out.message.as_deref(),
            Some("The layer is locked"),
            "{tool}"
        );
    }
}

#[test]
fn undoing_the_first_stroke_on_an_empty_frame_takes_its_cel_away() {
    let d = doc();
    let layer = d.layers()[0].id;
    let steps = with(
        drag((2, 2), (12, 7), L),
        vec![Step::Ctrl("z", false), Step::Wait],
    );
    let out = drive(editor_over(d), steps);
    assert_eq!(out.doc.cel(layer, 0).unwrap(), None);
}

/// A lasso's pixels move, and only they: the rest of their bounding box
/// stays, and the selection keeps its shape.
#[test]
fn the_move_tool_carries_a_lasso_selection_in_its_shape() {
    let steps = with(
        vec![Step::Edit(|e| {
            let layer = e.doc.layers()[0].id;
            let raster = e.doc.target(layer, 0).unwrap().raster;
            for y in 0..8 {
                for x in 0..8 {
                    raster.set_raw(x, y, Rgba::BLACK.to_u32());
                }
            }
            // A triangle: the lower left half of the block.
            let points = [Point::new(0, 0), Point::new(0, 7), Point::new(7, 7)];
            e.selection = Some(Selection::lasso(32, 32, &points));
        })],
        with(vec![Step::Key("v", false)], drag((1, 6), (11, 16), L)),
    );
    let out = drive(editor_over(doc()), steps);
    assert_eq!(px(&out.doc, 7, 0), BLACK, "outside the lasso, left alone");
    assert_eq!(px(&out.doc, 0, 7).a, 0, "lifted from where it was");
    assert_eq!(
        px(&out.doc, 10, 17),
        BLACK,
        "dropped ten pixels down and right"
    );
    assert_eq!(
        px(&out.doc, 17, 10).a,
        0,
        "the box corner did not come along"
    );
    let selection = out.selection.unwrap();
    assert!(selection.contains(10, 17));
    assert!(!selection.contains(17, 10), "the selection kept its shape");
    let before = Selection::lasso(
        32,
        32,
        &[Point::new(0, 0), Point::new(0, 7), Point::new(7, 7)],
    );
    assert_eq!(selection.count(), before.count());
}
