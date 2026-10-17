//! The toolbox's tools, driven by pointer and key messages over a rendered
//! editor and checked in the document they leave.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::{NamedKey, PointerButton};
use pixl::Tool;
use pixl_core::{ColorMode, Document, Rgba};

const L: PointerButton = PointerButton::Primary;
const R: PointerButton = PointerButton::Secondary;

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

fn hover(a: (i32, i32)) -> Vec<Step> {
    vec![Step::Move(At::Doc(a.0, a.1)), Step::Wait]
}

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

fn click(a: (i32, i32), button: PointerButton) -> Vec<Step> {
    let mut steps = hover(a);
    steps.push(Step::Down(At::Doc(a.0, a.1), button));
    steps.push(Step::Up(At::Doc(a.0, a.1), button));
    steps
}

fn then(parts: Vec<Vec<Step>>) -> Vec<Step> {
    parts.into_iter().flatten().collect()
}

/// A document with a red block at 4..8 x 4..8 and black at (20, 20).
fn art() -> Document {
    let mut d = doc();
    let l = d.layers()[0].id;
    let t = d.target(l, 0).unwrap();
    for y in 4..8 {
        for x in 4..8 {
            t.raster.set_raw(x, y, Rgba::opaque(255, 0, 0).to_u32());
        }
    }
    t.raster.set_raw(20, 20, Rgba::BLACK.to_u32());
    d
}

#[test]
fn every_tool_key_picks_its_tool() {
    let keys: [(&str, bool, Tool); 8] = [
        ("b", true, Tool::Brush),
        ("j", false, Tool::Spray),
        ("o", false, Tool::Shade),
        ("k", false, Tool::CloneStamp),
        ("r", true, Tool::Blur),
        ("g", true, Tool::Gradient),
        ("q", false, Tool::Lasso),
        ("n", false, Tool::Wand),
    ];
    for (key, shift, tool) in keys {
        let out = drive(editor_over(doc()), vec![Step::Key(key, shift)]);
        assert_eq!(out.tool, tool, "{key} shift {shift}");
    }
    let out = drive(editor_over(doc()), vec![Step::Click("tool-slice")]);
    assert_eq!(out.tool, Tool::Slice);
    common::save(&out.frame, "toolbox-toolbar");
}

#[test]
fn the_soft_brush_fades_and_the_dithered_brush_does_not_blend() {
    let soft = then(vec![
        vec![
            Step::Key("b", true),
            Step::Number("opt_size", 7.0),
            Step::Number("opt_hardness", 0.0),
        ],
        drag((10, 10), (20, 10), L),
    ]);
    let out = drive(editor_over(doc()), soft);
    assert_eq!(out.undo, vec!["Brush".to_string()]);
    let mid = px(&out.doc, 15, 10);
    let edge = px(&out.doc, 15, 13);
    assert!(mid.a > edge.a && edge.a > 0, "{mid:?} {edge:?}");
    let dithered = then(vec![
        vec![
            Step::Key("b", true),
            Step::Number("opt_size", 9.0),
            Step::Number("opt_opacity", 50.0),
            Step::Number("opt_hardness", 100.0),
            Step::Flag("opt_dithered", true),
            Step::Edit(|e| e.fg = Rgba::BLACK),
        ],
        drag((16, 16), (16, 16), L),
    ]);
    let out = drive(editor_over(doc()), dithered);
    for y in 0..32 {
        for x in 0..32 {
            let c = px(&out.doc, x, y);
            assert!(c.a == 0 || c == Rgba::BLACK, "({x}, {y}) {c:?}");
        }
    }
    let n = painted(&out.doc);
    assert!(n > 10 && n < 60, "{n}");
    common::save(&out.frame, "toolbox-brush");
}

#[test]
fn the_spray_scatters_inside_its_radius() {
    let steps = then(vec![
        vec![
            Step::Key("j", false),
            Step::Number("opt_spray_radius", 4.0),
            Step::Number("opt_spray_density", 20.0),
        ],
        drag((16, 16), (16, 16), L),
    ]);
    let out = drive(editor_over(doc()), steps);
    let n = painted(&out.doc);
    assert!(n > 5, "{n}");
    for y in 0..32 {
        for x in 0..32 {
            if px(&out.doc, x, y).a > 0 {
                assert!((x - 16).abs() <= 4 && (y - 16).abs() <= 4, "({x}, {y})");
            }
        }
    }
}

#[test]
fn shading_steps_along_the_palette_both_ways() {
    let mut d = Document::new(16, 16, ColorMode::Indexed).unwrap();
    let l = d.layers()[0].id;
    for x in 0..8 {
        d.target(l, 0).unwrap().raster.set_raw(x, 2, 3);
    }
    let steps = then(vec![
        vec![Step::Key("o", false)],
        drag((0, 2), (3, 2), L),
        drag((5, 2), (7, 2), R),
    ]);
    let out = drive(editor_over(d), steps);
    let r = out.doc.cel_raster(l, 0).unwrap().unwrap();
    assert_eq!(r.get_raw(1, 2), 4, "a step up the palette");
    assert_eq!(r.get_raw(6, 2), 2, "a step back");
    assert_eq!(r.get_raw(4, 2), 3, "untouched");
}

#[test]
fn the_clone_stamp_copies_from_where_alt_clicked() {
    let steps = then(vec![
        vec![Step::Key("k", false), Step::Number("opt_size", 1.0)],
        vec![Step::Alt(true)],
        click((5, 5), L),
        vec![Step::Alt(false)],
        drag((15, 15), (18, 15), L),
    ]);
    let out = drive(editor_over(art()), steps);
    let red = Rgba::opaque(255, 0, 0);
    assert_eq!(px(&out.doc, 15, 15), red);
    assert_eq!(px(&out.doc, 17, 15), red);
    assert_eq!(px(&out.doc, 18, 15).a, 0, "past the block's edge");
}

#[test]
fn the_pattern_stamp_paints_a_selected_pattern() {
    let steps = then(vec![
        vec![Step::Key("m", false)],
        drag((4, 4), (5, 4), L),
        // The options of a tool are there from the tick after it is picked.
        vec![
            Step::Key("k", true),
            Step::Wait,
            Step::Wait,
            Step::Click("opt-pattern-from-selection"),
        ],
        vec![Step::Edit(|e| e.selection = None)],
        drag((0, 12), (9, 12), L),
    ]);
    let out = drive(editor_over(art()), steps);
    let red = Rgba::opaque(255, 0, 0);
    let row: Vec<Rgba> = (0..10).map(|x| px(&out.doc, x, 12)).collect();
    assert!(row.iter().all(|c| *c == red), "{row:?} {:?}", out.message);
}

#[test]
fn smudge_blur_and_replace_change_what_is_there() {
    let red = Rgba::opaque(255, 0, 0);
    let smudge = then(vec![
        vec![
            Step::Key("r", false),
            Step::Number("opt_size", 1.0),
            Step::Number("opt_strength", 100.0),
        ],
        drag((6, 6), (14, 6), L),
    ]);
    let out = drive(editor_over(art()), smudge);
    assert_eq!(px(&out.doc, 12, 6), red, "dragged out of the block");
    let blur = then(vec![
        vec![
            Step::Key("r", true),
            Step::Number("opt_size", 3.0),
            Step::Number("opt_strength", 100.0),
        ],
        drag((8, 5), (8, 6), L),
    ]);
    let out = drive(editor_over(art()), blur);
    let c = px(&out.doc, 8, 5);
    assert!(c.a > 0 && c.a < 255, "the edge softens: {c:?}");
    let replace = then(vec![
        vec![
            Step::Key("y", false),
            Step::Number("opt_size", 8.0),
            Step::Edit(|e| {
                e.fg = Rgba::WHITE;
                e.bg = Rgba::opaque(255, 0, 0);
            }),
        ],
        drag((5, 5), (6, 6), L),
    ]);
    let out = drive(editor_over(art()), replace);
    assert_eq!(px(&out.doc, 5, 5), Rgba::WHITE);
    assert_eq!(px(&out.doc, 20, 20), Rgba::BLACK, "other colors stay");
}

#[test]
fn the_dither_brush_mixes_two_colors() {
    let steps = then(vec![
        vec![
            Step::Key("o", true),
            Step::Number("opt_size", 8.0),
            Step::Edit(|e| {
                e.fg = Rgba::BLACK;
                e.bg = Rgba::WHITE;
            }),
        ],
        drag((16, 16), (16, 16), L),
    ]);
    let out = drive(editor_over(doc()), steps);
    let (mut black, mut white) = (0u32, 0u32);
    for y in 0..32 {
        for x in 0..32 {
            match px(&out.doc, x, y) {
                Rgba::BLACK => black += 1,
                Rgba::WHITE => white += 1,
                _ => {}
            }
        }
    }
    assert!(black > 0 && white > 0 && black.abs_diff(white) <= 4);
}

#[test]
fn curves_and_polygons_take_several_clicks() {
    let curve = then(vec![
        vec![Step::Key("l", true)],
        drag((2, 20), (28, 20), L),
        click((8, 2), L),
        click((22, 2), L),
    ]);
    let out = drive(editor_over(doc()), curve);
    assert_eq!(out.undo, vec!["Curve".to_string()]);
    assert_eq!(px(&out.doc, 2, 20), Rgba::BLACK);
    assert_eq!(px(&out.doc, 28, 20), Rgba::BLACK);
    assert_eq!(px(&out.doc, 15, 20).a, 0, "the middle bends up");
    let top = (0..20).find(|&y| (0..32).any(|x| px(&out.doc, x, y).a > 0));
    assert!(top.is_some_and(|y| y < 12), "{top:?}");
    common::save(&out.frame, "toolbox-curve");

    let polygon = then(vec![
        vec![Step::Key("h", false), Step::Flag("opt_filled", true)],
        click((2, 2), L),
        click((20, 2), L),
        click((2, 20), L),
        vec![Step::KeyDown(NamedKey::Enter), Step::KeyUp(NamedKey::Enter)],
    ]);
    let out = drive(editor_over(doc()), polygon);
    assert_eq!(out.undo, vec!["Polygon".to_string()]);
    assert_eq!(px(&out.doc, 5, 5), Rgba::BLACK);
    assert_eq!(px(&out.doc, 18, 18).a, 0);
}

#[test]
fn a_contour_fills_and_a_lasso_selects() {
    let mut contour = vec![Step::Key("p", true)];
    contour.extend(hover((4, 4)));
    contour.push(Step::Down(At::Doc(4, 4), L));
    for p in [(20, 4), (20, 20), (4, 20)] {
        contour.push(Step::Move(At::Doc(p.0, p.1)));
    }
    contour.push(Step::Up(At::Doc(4, 20), L));
    let out = drive(editor_over(doc()), contour);
    assert_eq!(px(&out.doc, 12, 12), Rgba::BLACK);
    assert_eq!(out.undo, vec!["Contour".to_string()]);

    let mut lasso = vec![Step::Key("q", false)];
    lasso.extend(hover((4, 4)));
    lasso.push(Step::Down(At::Doc(4, 4), L));
    for p in [(20, 4), (20, 20), (4, 20)] {
        lasso.push(Step::Move(At::Doc(p.0, p.1)));
    }
    lasso.push(Step::Up(At::Doc(4, 20), L));
    let out = drive(editor_over(doc()), lasso);
    let sel = out.selection.expect("a selection");
    assert!(sel.contains(12, 12) && !sel.contains(25, 25));
    assert!(out.undo.is_empty(), "selecting is not an edit");
}

#[test]
fn the_wand_selects_a_color_and_adds_with_shift() {
    let steps = then(vec![
        vec![Step::Key("n", false)],
        click((5, 5), L),
        vec![Step::Shift(true)],
        click((20, 20), L),
        vec![Step::Shift(false)],
    ]);
    let out = drive(editor_over(art()), steps);
    let sel = out.selection.expect("a selection");
    assert_eq!(sel.count(), 16 + 1);
    assert!(sel.contains(20, 20) && !sel.contains(10, 10));
    common::save(&out.frame, "toolbox-wand");
}

#[test]
fn the_ellipse_selection_and_the_polygon_lasso() {
    let out = drive(
        editor_over(doc()),
        then(vec![vec![Step::Key("m", true)], drag((0, 0), (20, 20), L)]),
    );
    let sel = out.selection.expect("a selection");
    assert!(sel.contains(10, 10) && !sel.contains(0, 0));
    let out = drive(
        editor_over(doc()),
        then(vec![
            vec![Step::Key("q", true)],
            click((2, 2), L),
            click((20, 2), L),
            click((20, 20), L),
            click((2, 2), L),
        ]),
    );
    let sel = out.selection.expect("a selection");
    assert!(sel.contains(15, 5) && !sel.contains(4, 18));
}

#[test]
fn a_gradient_steps_between_the_colors() {
    let steps = then(vec![
        vec![
            Step::Key("g", true),
            Step::Edit(|e| {
                e.fg = Rgba::BLACK;
                e.bg = Rgba::WHITE;
            }),
        ],
        drag((0, 0), (31, 0), L),
    ]);
    let out = drive(editor_over(doc()), steps);
    assert_eq!(px(&out.doc, 0, 10), Rgba::BLACK);
    assert_eq!(px(&out.doc, 31, 10), Rgba::WHITE);
    for y in 0..32 {
        for x in 0..32 {
            let c = px(&out.doc, x, y);
            assert!(c == Rgba::BLACK || c == Rgba::WHITE, "no blends");
        }
    }
    common::save(&out.frame, "toolbox-gradient");
}

#[test]
fn crop_text_measure_and_slices() {
    let crop = then(vec![vec![Step::Key("c", false)], drag((4, 4), (11, 9), L)]);
    let out = drive(editor_over(art()), crop);
    assert_eq!((out.doc.width(), out.doc.height()), (8, 6));
    assert_eq!(out.undo, vec!["Crop".to_string()]);
    assert_eq!(px(&out.doc, 0, 0), Rgba::opaque(255, 0, 0));

    let text = then(vec![
        vec![Step::Key("t", false), Step::Text("opt_text", "Hi")],
        click((2, 2), L),
    ]);
    let out = drive(editor_over(doc()), text);
    assert_eq!(out.undo, vec!["Text".to_string()]);
    // The H's left stem.
    assert!((2..9).all(|y| px(&out.doc, 2, y) == Rgba::BLACK));
    common::save(&out.frame, "toolbox-text");

    let measure = then(vec![vec![Step::Key("c", true)], drag((1, 1), (4, 5), L)]);
    let out = drive(editor_over(doc()), measure);
    let m = out.message.unwrap_or_default();
    assert!(m.contains("4 x 5 px") && m.contains("length 5.0"), "{m}");

    let slice = then(vec![vec![Step::Key("f", false)], drag((2, 2), (9, 5), L)]);
    let out = drive(editor_over(doc()), slice);
    assert_eq!(out.doc.slices().len(), 1);
    assert_eq!(out.doc.slices()[0].bounds, pixl_core::Rect::new(2, 2, 8, 4));
}

#[test]
fn symmetry_mirrors_the_brush_tools() {
    let steps = then(vec![
        vec![
            Step::Key("o", true),
            Step::Number("opt_size", 1.0),
            Step::Number("opt_dither_amount", 100.0),
            Step::Text("opt_symmetry", "Mirror left and right"),
        ],
        drag((2, 5), (4, 5), L),
    ]);
    let out = drive(editor_over(doc()), steps);
    assert_eq!(px(&out.doc, 3, 5), Rgba::BLACK);
    assert_eq!(
        px(&out.doc, 28, 5),
        Rgba::BLACK,
        "mirrored across the middle"
    );
}

#[test]
fn escape_drops_a_polygon_in_progress() {
    let steps = then(vec![
        vec![Step::Key("h", false)],
        click((2, 2), L),
        click((20, 2), L),
        vec![
            Step::KeyDown(NamedKey::Escape),
            Step::KeyUp(NamedKey::Escape),
        ],
        click((2, 20), L),
        vec![Step::KeyDown(NamedKey::Enter), Step::KeyUp(NamedKey::Enter)],
    ]);
    let out = drive(editor_over(doc()), steps);
    assert!(out.undo.is_empty(), "{:?}", out.undo);
}
