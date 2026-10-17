//! The Select, Image, Filter and View menus, driven through their items,
//! keys and dialogs over a rendered editor.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl_core::{ColorMode, Document, Rect, Rgba};

const L: PointerButton = PointerButton::Primary;

fn px(doc: &Document, x: i32, y: i32) -> Rgba {
    let layer = doc.layers()[0].id;
    let r = doc.cel_raster(layer, 0).unwrap();
    let Some(r) = r else {
        return Rgba::TRANSPARENT;
    };
    let raw = r.get_raw(x, y);
    match doc.mode() {
        ColorMode::Rgba => Rgba::from_u32(raw),
        ColorMode::Indexed => {
            if raw as u8 == doc.transparent_index() {
                Rgba::TRANSPARENT
            } else {
                doc.palette().get(raw as u8)
            }
        }
    }
}

const RED: Rgba = Rgba::opaque(255, 0, 0);

/// 32x32 with a red block at 4..12 x 4..12 and a white pixel at (20, 20).
fn art() -> Document {
    let mut d = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let l = d.layers()[0].id;
    let t = d.target(l, 0).unwrap();
    for y in 4..12 {
        for x in 4..12 {
            t.raster.set_raw(x, y, RED.to_u32());
        }
    }
    t.raster.set_raw(20, 20, Rgba::WHITE.to_u32());
    d
}

fn waits(n: usize) -> Vec<Step> {
    vec![Step::Wait; n]
}

fn then(parts: Vec<Vec<Step>>) -> Vec<Step> {
    parts.into_iter().flatten().collect()
}

#[test]
fn selecting_all_none_inverse_and_by_amount() {
    let out = drive(editor_over(art()), vec![Step::Ctrl("a", false), Step::Wait]);
    assert_eq!(out.selection.unwrap().count(), 32 * 32);
    let out = drive(
        editor_over(art()),
        vec![
            Step::Menu("select-layer"),
            Step::Wait,
            Step::Menu("select-grow"),
            Step::Wait,
            Step::Text("amount_value", "2"),
            Step::Click("amount-ok"),
            Step::Wait,
        ],
    );
    let sel = out.selection.unwrap();
    assert!(sel.contains(2, 2) && sel.contains(22, 22) && !sel.contains(1, 1));
    let out = drive(
        editor_over(art()),
        vec![
            Step::Edit(|e| e.fg = Rgba::opaque(255, 0, 0)),
            Step::Menu("select-color"),
            Step::Wait,
            Step::Menu("select-invert"),
            Step::Wait,
        ],
    );
    let sel = out.selection.unwrap();
    assert_eq!(sel.count(), 32 * 32 - 64);
    assert!(!sel.contains(5, 5));
    common::save(&out.frame, "menus-selection-outline");
}

#[test]
fn a_saved_selection_loads_again() {
    let out = drive(
        editor_over(art()),
        vec![
            Step::Menu("select-layer"),
            Step::Wait,
            Step::Menu("select-save"),
            Step::Wait,
            Step::Ctrl("d", false),
            Step::Wait,
            Step::Menu("select-load"),
            Step::Wait,
            Step::Wait,
            Step::Click("saved-sel-0"),
            Step::Click("selections-close"),
            Step::Wait,
        ],
    );
    assert_eq!(out.selection.unwrap().count(), 65);
}

#[test]
fn canvas_size_image_size_and_turns_are_undoable_steps() {
    let out = drive(
        editor_over(art()),
        vec![
            Step::Menu("image-canvas-size"),
            Step::Wait,
            Step::Text("canvas_w", "40"),
            Step::Text("canvas_h", "36"),
            Step::Text("canvas_anchor", "Top left"),
            Step::Click("canvas-ok"),
            Step::Wait,
            Step::Menu("image-resize"),
            Step::Wait,
            Step::Text("resize_w", "80"),
            Step::Text("resize_h", "72"),
            Step::Click("resize-ok"),
            Step::Wait,
            Step::Menu("image-rotate-cw"),
            Step::Wait,
        ],
    );
    assert_eq!((out.doc.width(), out.doc.height()), (72, 80));
    assert_eq!(out.undo, vec!["Canvas size", "Image size", "Rotate canvas"]);
    // The red block, doubled, turned a quarter clockwise.
    assert_eq!(px(&out.doc, 72 - 1 - 9, 9), RED);
}

#[test]
fn indexed_color_with_a_dither_and_back() {
    let mut d = Document::new(64, 8, ColorMode::Rgba).unwrap();
    let l = d.layers()[0].id;
    for x in 0..64 {
        for y in 0..8 {
            let v = (x * 4) as u8;
            d.target(l, 0)
                .unwrap()
                .raster
                .set_raw(x, y, Rgba::opaque(v, v, v).to_u32());
        }
    }
    let out = drive(
        editor_over(d),
        vec![
            Step::Menu("image-indexed"),
            Step::Wait,
            Step::Text("indexed_palette", "New palette from the image"),
            Step::Text("indexed_colors", "4"),
            Step::Text("indexed_dither", "Floyd-Steinberg"),
            Step::Click("indexed-ok"),
            Step::Wait,
        ],
    );
    assert_eq!(out.doc.mode(), ColorMode::Indexed);
    assert!(out.doc.palette().len() <= 4);
    common::save(&out.frame, "menus-indexed-fs");
}

#[test]
fn replacing_a_color_and_a_palette_from_the_image() {
    let out = drive(
        editor_over(art()),
        vec![
            Step::Menu("image-replace-color"),
            Step::Wait,
            Step::Text("replace_from", "#ff0000"),
            Step::Text("replace_to", "color(srgb 0 0 1)"),
            Step::Click("replace-ok"),
            Step::Wait,
            Step::Menu("palette-from-image"),
            Step::Wait,
            Step::Text("quantize_colors", "8"),
            Step::Click("quantize-ok"),
            Step::Wait,
        ],
    );
    assert_eq!(px(&out.doc, 5, 5), Rgba::opaque(0, 0, 255));
    let colors = out.doc.palette().colors();
    assert!(colors.contains(&Rgba::opaque(0, 0, 255)) && colors.contains(&Rgba::WHITE));
    assert_eq!(colors.len(), 2);
}

#[test]
fn the_ramp_builder_adds_its_ramp() {
    let out = drive(
        editor_over(art()),
        vec![
            Step::Edit(|e| e.fg = Rgba::opaque(60, 140, 200)),
            Step::Menu("palette-ramp"),
            Step::Wait,
            Step::Text("ramp_steps", "6"),
            Step::Wait,
            Step::Wait,
            Step::Click("ramp-ok"),
            Step::Wait,
        ],
    );
    assert_eq!(out.doc.palette().len(), 16 + 6);
    assert_eq!(out.undo, vec!["Add ramp"]);
}

#[test]
fn filters_preview_and_apply_as_one_step_or_cancel() {
    let hue = then(vec![
        vec![Step::Ctrl("u", false), Step::Wait],
        vec![Step::Number("flt_hue", 120.0)],
        waits(4),
        vec![Step::Click("flt-ok")],
        waits(4),
    ]);
    let out = drive(editor_over(art()), hue);
    assert_eq!(px(&out.doc, 5, 5), Rgba::opaque(0, 255, 0));
    assert_eq!(out.undo, vec!["Hue and saturation"]);

    let cancel = then(vec![
        vec![Step::Menu("filter-posterize"), Step::Wait],
        vec![Step::Number("flt_levels", 2.0)],
        waits(4),
        vec![Step::Click("flt-cancel")],
        waits(2),
    ]);
    let out = drive(editor_over(art()), cancel);
    assert_eq!(out.doc, art(), "cancel leaves the cel as it was");
    assert!(out.undo.is_empty());

    let invert = then(vec![vec![Step::Ctrl("i", false)], waits(4)]);
    let out = drive(editor_over(art()), invert);
    assert_eq!(px(&out.doc, 5, 5), Rgba::opaque(0, 255, 255));
    assert_eq!(px(&out.doc, 0, 0), Rgba::TRANSPARENT);
}

#[test]
fn outline_and_shadow_reach_past_the_art() {
    let steps = then(vec![
        vec![
            Step::Edit(|e| e.fg = Rgba::BLACK),
            Step::Menu("filter-outline"),
            Step::Wait,
        ],
        waits(3),
        vec![Step::Click("flt-ok")],
        waits(4),
    ]);
    let out = drive(editor_over(art()), steps);
    assert_eq!(px(&out.doc, 3, 5), Rgba::BLACK);
    assert_eq!(px(&out.doc, 3, 3).a, 0, "four-connected corners stay clear");
    common::save(&out.frame, "menus-outline");
}

#[test]
fn a_filter_on_a_large_cel_renders_tile_by_tile() {
    let mut d = Document::new(1024, 1024, ColorMode::Rgba).unwrap();
    let l = d.layers()[0].id;
    let t = d.target(l, 0).unwrap();
    // Red with a blue pixel in every 8x8 block: pixelating takes it out.
    for y in 0..1024 {
        for x in 0..1024 {
            let c = if x % 8 == 3 && y % 8 == 3 {
                Rgba::opaque(0, 0, 255)
            } else {
                RED
            };
            t.raster.set_raw(x, y, c.to_u32());
        }
    }
    let steps = then(vec![
        vec![Step::Menu("filter-pixelate"), Step::Wait],
        vec![Step::Number("flt_size", 8.0)],
        vec![Step::Click("flt-ok")],
        waits(200),
    ]);
    let out = drive(editor_over(d), steps);
    assert_eq!(out.undo, vec!["Pixelate"], "{:?}", out.message);
    assert_eq!(px(&out.doc, 1003, 1003), RED);
    assert_eq!(px(&out.doc, 3, 3), RED);
}

#[test]
fn a_turned_and_mirrored_view_paints_where_the_pointer_is() {
    let steps = vec![
        Step::Key("]", false),
        Step::Menu("view-rotate-cw"),
        Step::Menu("view-rotate-cw"),
        Step::Menu("view-flip"),
        Step::Wait,
        Step::Move(At::Doc(6, 20)),
        Step::Wait,
        Step::Down(At::Doc(6, 20), L),
        Step::Move(At::Doc(9, 20)),
        Step::Up(At::Doc(9, 20), L),
        Step::Wait,
    ];
    let out = drive(
        editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap()),
        steps,
    );
    let v = out.views.last().unwrap();
    assert_eq!(v.rotation, 30.0);
    assert!(v.flipped);
    assert_eq!(px(&out.doc, 6, 20), Rgba::BLACK);
    assert_eq!(px(&out.doc, 9, 20), Rgba::BLACK);
    common::save(&out.frame, "menus-turned-view");
}

#[test]
fn tiled_mode_rulers_and_the_grid_show() {
    let out = drive(
        editor_over(art()),
        vec![
            Step::Menu("view-tile-x"),
            Step::Menu("view-tile-y"),
            Step::Ctrl("r", false),
            Step::Menu("view-grid-settings"),
            Step::Wait,
            Step::Text("grid_w", "8"),
            Step::Text("grid_h", "8"),
            Step::Click("grid-ok"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
        ],
    );
    let v = out.views.last().unwrap();
    assert_eq!(v.tiled, (true, true));
    common::save(&out.frame, "menus-tiled-rulers-grid");
    // The red block repeats to the right of the document.
    let s = pixl::tools::screen_of(v, pixl_core::Point::new(32 + 5, 5));
    assert_eq!(
        out.frame.pixel(s.x as u32, s.y as u32),
        Some([0xff, 0x00, 0x00, 0xff])
    );
}

#[test]
fn snapping_puts_a_selection_on_the_grid() {
    let out = drive(
        editor_over(art()),
        vec![
            Step::Menu("view-grid-settings"),
            Step::Wait,
            Step::Text("grid_w", "8"),
            Step::Text("grid_h", "8"),
            Step::Click("grid-ok"),
            Step::Menu("view-snap"),
            // Out to 400 percent, where snapping reaches a pixel and a half.
            Step::Key("-", false),
            Step::Key("-", false),
            Step::Key("-", false),
            Step::Key("-", false),
            Step::Key("m", false),
            Step::Wait,
            Step::Move(At::Doc(1, 1)),
            Step::Wait,
            Step::Down(At::Doc(1, 1), L),
            Step::Move(At::Doc(14, 14)),
            Step::Up(At::Doc(14, 14), L),
            Step::Wait,
        ],
    );
    let b = out.selection.unwrap().bounds();
    assert_eq!(b, Rect::new(0, 0, 16, 16));
}

#[test]
fn a_slice_opens_its_dialog_and_takes_a_9_slice_center() {
    let mut d = art();
    d.slices_mut().push(pixl_core::Slice {
        name: "button".into(),
        bounds: Rect::new(2, 2, 10, 10),
        center: None,
        pivot: None,
        color: Rgba::BLACK,
    });
    let out = drive(
        editor_over(d),
        vec![
            Step::Key("f", false),
            Step::Move(At::Doc(5, 5)),
            Step::Wait,
            Step::Down(At::Doc(5, 5), L),
            Step::Up(At::Doc(5, 5), L),
            Step::Wait,
            Step::Wait,
            Step::Flag("slice_nine", true),
            Step::Text("slice_cx", "3"),
            Step::Text("slice_cy", "3"),
            Step::Text("slice_cw", "4"),
            Step::Text("slice_ch", "4"),
            Step::Click("slice-ok"),
            Step::Wait,
            Step::Wait,
        ],
    );
    let s = &out.doc.slices()[0];
    assert_eq!(s.center, Some(Rect::new(3, 3, 4, 4)));
    assert_eq!(out.undo, vec!["Edit slice"]);
    common::save(&out.frame, "menus-slice");
}
