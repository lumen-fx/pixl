//! The clipboard: copy, cut and copy merged put an image on the clipboard;
//! paste floats it in the free transform box at its old place or in the
//! middle of the view; paste as new layer and as new document. The editor
//! the tests drive keeps its clipboard in memory, never the desktop's.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::NamedKey;
use lumenui::prelude::*;
use pixl::clipboard::Pasteboard;
use pixl_document::{
    ColorMode, Document, Palette, Rect, Rgba, Selection, TileFlags, TileMode, TileRef,
};

const RED: Rgba = Rgba::opaque(255, 0, 0);
const BLUE: Rgba = Rgba::opaque(0, 0, 255);

/// A 32x32 document with a red 4x4 square at (4, 4) and a blue pixel at
/// (20, 20) on its one layer.
fn square() -> Document {
    let mut d = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let l = d.layers()[0].id;
    let t = d.target(l, 0).unwrap();
    for y in 4..8 {
        for x in 4..8 {
            t.raster.set_raw(x, y, RED.to_u32());
        }
    }
    t.raster.set_raw(20, 20, BLUE.to_u32());
    d
}

fn px(doc: &Document, layer: usize, x: i32, y: i32) -> Rgba {
    let id = doc.layers()[layer].id;
    doc.cel_raster(id, 0)
        .unwrap()
        .map_or(Rgba::TRANSPARENT, |r| Rgba::from_u32(r.get_raw(x, y)))
}

fn select_square(editor: &mut pixl::Editor) {
    editor.selection = Some(Selection::rect(32, 32, Rect::new(4, 4, 4, 4)));
}

fn enter() -> [Step; 2] {
    [Step::KeyDown(NamedKey::Enter), Step::KeyUp(NamedKey::Enter)]
}

fn escape() -> [Step; 2] {
    [
        Step::KeyDown(NamedKey::Escape),
        Step::KeyUp(NamedKey::Escape),
    ]
}

fn steps(parts: &[&[Step]]) -> Vec<Step> {
    parts.iter().flat_map(|p| p.iter().cloned()).collect()
}

#[test]
fn copy_puts_the_selection_on_the_clipboard_as_an_image() {
    let out = drive(
        editor_over(square()),
        vec![
            Step::Edit(select_square),
            Step::Ctrl("c", false),
            Step::Wait,
        ],
    );
    let image = out.clipboard.expect("the clipboard holds an image");
    assert_eq!((image.width(), image.height()), (4, 4));
    assert!(
        image
            .pixels()
            .chunks_exact(4)
            .all(|c| c == [255, 0, 0, 255])
    );
    assert_eq!(out.doc, square(), "copying leaves the document alone");
    assert!(out.undo.is_empty());
}

#[test]
fn with_no_selection_copy_takes_the_cel_content() {
    let out = drive(
        editor_over(square()),
        vec![Step::Ctrl("c", false), Step::Wait],
    );
    let image = out.clipboard.expect("the clipboard holds an image");
    // From the red square's corner to the blue pixel.
    assert_eq!((image.width(), image.height()), (17, 17));
}

#[test]
fn cut_clears_the_selection_as_one_step_and_undo_puts_it_back() {
    let out = drive(
        editor_over(square()),
        vec![
            Step::Edit(select_square),
            Step::Ctrl("x", false),
            Step::Wait,
        ],
    );
    assert_eq!(out.undo, vec!["Cut"]);
    assert_eq!(px(&out.doc, 0, 5, 5), Rgba::TRANSPARENT);
    assert_eq!(px(&out.doc, 0, 20, 20), BLUE, "outside the selection stays");
    assert!(out.clipboard.is_some());
    let undone = drive(
        editor_over(square()),
        vec![
            Step::Edit(select_square),
            Step::Ctrl("x", false),
            Step::Wait,
            Step::Ctrl("z", false),
            Step::Wait,
        ],
    );
    assert_eq!(undone.doc, square());
}

#[test]
fn paste_floats_at_the_old_place_and_enter_keeps_it_as_one_step() {
    let out = drive(
        editor_over(square()),
        steps(&[
            &[
                Step::Edit(select_square),
                Step::Ctrl("c", false),
                Step::Wait,
                Step::Ctrl("v", false),
                Step::Wait,
                Step::Wait,
            ],
            &enter(),
            &[Step::Wait],
        ]),
    );
    assert!(
        out.undo.is_empty(),
        "pasted over itself, the cel is the same and no step is kept"
    );
    assert_eq!(out.doc, square());
    assert_eq!(out.tool, pixl::Tool::Move, "Enter closed the box");
    let moved = drive(
        editor_over(square()),
        steps(&[
            &[
                Step::Edit(select_square),
                Step::Ctrl("c", false),
                Step::Wait,
                Step::Ctrl("v", false),
                Step::Wait,
                Step::Move(At::Doc(5, 5)),
                Step::Down(
                    At::Doc(5, 5),
                    lumenui::lumen_core::input::PointerButton::Primary,
                ),
                Step::Move(At::Doc(13, 5)),
                Step::Up(
                    At::Doc(13, 5),
                    lumenui::lumen_core::input::PointerButton::Primary,
                ),
                Step::Wait,
            ],
            &enter(),
            &[Step::Wait],
        ]),
    );
    assert_eq!(moved.undo, vec!["Paste"]);
    assert_eq!(px(&moved.doc, 0, 13, 5), RED, "the paste moved right");
    assert_eq!(px(&moved.doc, 0, 5, 5), RED, "the original stays under it");
}

#[test]
fn a_floating_paste_shows_in_its_box() {
    let out = drive(
        editor_over(square()),
        vec![
            Step::Edit(select_square),
            Step::Ctrl("c", false),
            Step::Wait,
            Step::Ctrl("v", false),
            Step::Wait,
            Step::Text("opt_angle", "45"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "paste-floating");
    assert_eq!(out.tool, pixl::Tool::Transform);
}

#[test]
fn escape_drops_the_paste_and_the_cel_it_made() {
    // A second frame whose cel is empty.
    let mut doc = square();
    doc.insert_frame(1, 100).unwrap();
    let out = drive(
        editor_over(doc.clone()),
        steps(&[
            &[
                Step::Edit(select_square),
                Step::Ctrl("c", false),
                Step::Wait,
                Step::Edit(|e| e.frame = 1),
                Step::Ctrl("v", false),
                Step::Wait,
                Step::Wait,
            ],
            &escape(),
            &[Step::Wait],
        ]),
    );
    assert!(out.undo.is_empty());
    assert_eq!(out.doc, doc, "no cel is left on the empty frame");
}

#[test]
fn an_image_from_another_program_pastes_in_the_middle_of_the_view() {
    let image = RgbaImage::new(2, 2, [0, 0, 255, 255].repeat(4)).unwrap();
    let out = drive(
        editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap())
            .insert_resource(Pasteboard::holding(image)),
        steps(&[
            &[Step::Ctrl("v", false), Step::Wait, Step::Wait],
            &enter(),
            &[Step::Wait],
        ]),
    );
    assert_eq!(out.undo, vec!["Paste"]);
    let layer = out.doc.layers()[0].id;
    let b = out
        .doc
        .cel_raster(layer, 0)
        .unwrap()
        .unwrap()
        .content_bounds();
    assert_eq!(b, Rect::new(15, 15, 2, 2), "centered on the fitted view");
}

#[test]
fn a_paste_whose_place_is_out_of_view_lands_in_the_middle_of_the_view() {
    let mut zoom_in = vec![
        Step::Edit(|e| {
            e.selection = Some(Selection::rect(32, 32, Rect::new(0, 0, 2, 2)));
            let l = e.doc.layers()[0].id;
            let t = e.doc.target(l, 0).unwrap();
            t.raster.set_raw(0, 0, RED.to_u32());
        }),
        Step::Ctrl("c", false),
        Step::Wait,
    ];
    // Zoom far in on the bottom right corner, so the top left is out of view.
    for _ in 0..6 {
        zoom_in.push(Step::Wheel(At::Doc(28, 28), -40.0));
        zoom_in.push(Step::Wait);
    }
    let out = drive(
        editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap()),
        steps(&[
            &zoom_in,
            &[Step::Ctrl("v", false), Step::Wait, Step::Wait],
            &enter(),
            &[Step::Wait],
        ]),
    );
    let v = out.views.last().unwrap();
    // The copied place, document pixels 0..2, is left of and above the view.
    assert!(
        v.origin.0 + 2.0 * v.zoom < 0.0 && v.origin.1 + 2.0 * v.zoom < 0.0,
        "the view zoomed in past the copied place: {} at {:?}",
        v.zoom,
        v.origin
    );
    assert_eq!(out.undo, vec!["Paste"]);
    let layer = out.doc.layers()[0].id;
    let raster = out.doc.cel_raster(layer, 0).unwrap().unwrap();
    // The red pixel copied from (0, 0) now sits where the view shows it.
    let pasted: Vec<_> = Rect::new(1, 1, 31, 31)
        .points()
        .filter(|p| Rgba::from_u32(raster.get_raw(p.x, p.y)) == RED)
        .collect();
    assert_eq!(pasted.len(), 1, "one red pixel landed in view: {pasted:?}");
    let p = pasted[0];
    let (sx, sy) = (
        v.origin.0 + (f64::from(p.x) + 0.5) * v.zoom,
        v.origin.1 + (f64::from(p.y) + 0.5) * v.zoom,
    );
    assert!(
        sx > 0.0 && sy > 0.0 && sx < f64::from(v.stage.1.x) && sy < f64::from(v.stage.1.y),
        "the paste is in view at {p:?}"
    );
}

#[test]
fn paste_as_new_layer_adds_a_layer_as_one_step() {
    let out = drive(
        editor_over(square()),
        vec![
            Step::Edit(select_square),
            Step::Ctrl("c", false),
            Step::Wait,
            Step::Ctrl("v", true),
            Step::Wait,
        ],
    );
    assert_eq!(out.undo, vec!["Paste as new layer"]);
    assert_eq!(out.doc.layers().len(), 2);
    assert_eq!(px(&out.doc, 1, 5, 5), RED, "at the place it came from");
    assert_eq!(px(&out.doc, 1, 20, 20), Rgba::TRANSPARENT);
    assert_eq!(out.at.1, out.doc.layers()[1].id, "the new layer is active");
    let undone = drive(
        editor_over(square()),
        vec![
            Step::Edit(select_square),
            Step::Ctrl("c", false),
            Step::Wait,
            Step::Ctrl("v", true),
            Step::Wait,
            Step::Ctrl("z", false),
            Step::Wait,
        ],
    );
    assert_eq!(undone.doc, square());
}

#[test]
fn paste_as_new_document_opens_a_tab_the_size_of_the_image() {
    let out = drive(
        editor_over(square()),
        vec![
            Step::Edit(select_square),
            Step::Ctrl("c", false),
            Step::Wait,
            Step::Menu("edit-paste-document"),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert_eq!(out.tabs.len(), 2);
    assert_eq!(out.front, Some(1));
    assert_eq!((out.doc.width(), out.doc.height()), (4, 4));
    assert_eq!(px(&out.doc, 0, 0, 0), RED);
}

#[test]
fn copy_merged_flattens_the_visible_layers() {
    let mut doc = square();
    let top = doc
        .add_layer(
            pixl_document::NewLayer::Raster,
            "top",
            pixl_document::Placement::default(),
        )
        .unwrap();
    doc.target(top, 0)
        .unwrap()
        .raster
        .set_raw(5, 5, BLUE.to_u32());
    let out = drive(
        editor_over(doc),
        vec![Step::Edit(select_square), Step::Ctrl("c", true), Step::Wait],
    );
    let image = out.clipboard.expect("the clipboard holds an image");
    assert_eq!((image.width(), image.height()), (4, 4));
    let at = |x: usize, y: usize| &image.pixels()[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
    assert_eq!(at(1, 1), [0, 0, 255, 255], "the top layer's pixel");
    assert_eq!(at(0, 0), [255, 0, 0, 255], "the layer below shows through");
}

/// An indexed document on black, red and blue, with index 0 clear.
fn indexed() -> Document {
    let mut d = Document::new(16, 16, ColorMode::Indexed).unwrap();
    d.set_palette(Palette::new(vec![Rgba::TRANSPARENT, Rgba::BLACK, RED, BLUE]).unwrap());
    d.set_transparent_index(0);
    d
}

#[test]
fn an_image_pasted_on_an_indexed_document_lands_on_its_palette() {
    // Nearly red and nearly blue, and a clear pixel.
    let mut bytes = Vec::new();
    for c in [
        [250, 10, 0, 255],
        [0, 5, 240, 255],
        [9, 9, 9, 0],
        [250, 10, 0, 255],
    ] {
        bytes.extend_from_slice(&c);
    }
    let image = RgbaImage::new(2, 2, bytes).unwrap();
    let out = drive(
        editor_over(indexed()).insert_resource(Pasteboard::holding(image)),
        steps(&[
            &[Step::Ctrl("v", false), Step::Wait, Step::Wait],
            &enter(),
            &[Step::Wait],
        ]),
    );
    assert_eq!(out.undo, vec!["Paste"]);
    let layer = out.doc.layers()[0].id;
    let r = out.doc.cel_raster(layer, 0).unwrap().unwrap();
    let b = r.content_bounds();
    let at = |dx: i32, dy: i32| r.get_raw(b.x + dx, b.y + dy);
    // Centered on the 16x16 canvas: the image's top left at (7, 7).
    assert_eq!((b.x, b.y), (7, 7));
    assert_eq!(at(0, 0), 2, "red");
    assert_eq!(at(1, 0), 3, "blue");
    assert_eq!(at(0, 1), 0, "clear stays clear");
}

#[test]
fn indexed_pixels_copy_and_paste_as_their_indices() {
    let mut doc = indexed();
    let l = doc.layers()[0].id;
    let t = doc.target(l, 0).unwrap();
    t.raster.set_raw(2, 2, 3);
    t.raster.set_raw(3, 2, 1);
    let out = drive(
        editor_over(doc),
        vec![
            Step::Ctrl("c", false),
            Step::Wait,
            Step::Ctrl("v", true),
            Step::Wait,
            Step::Menu("edit-paste-document"),
            Step::Wait,
            Step::Wait,
        ],
    );
    let image = out.clipboard.expect("the clipboard holds an image");
    assert_eq!(image.pixels(), &[0, 0, 255, 255, 0, 0, 0, 255]);
    // The new document is indexed on the same palette.
    assert_eq!(out.doc.mode(), ColorMode::Indexed);
    assert_eq!(out.doc.palette(), indexed().palette());
    let layer = out.doc.layers()[0].id;
    let r = out.doc.cel_raster(layer, 0).unwrap().unwrap();
    assert_eq!((r.get_raw(0, 0), r.get_raw(1, 0)), (3, 1));
}

/// A 64x64 document whose only layer is a tilemap layer of 16x16 tiles,
/// with tile 1 (black) in every cell.
fn tiled() -> Document {
    let mut d = Document::new(64, 64, ColorMode::Rgba).unwrap();
    let plain = d.layers()[0].id;
    let ts = d.add_tileset("ground", 16, 16).unwrap();
    d.add_tile(ts, vec![Rgba::BLACK.to_u32(); 256]).unwrap();
    let map = d
        .add_layer(
            pixl_document::NewLayer::Tilemap { tileset: ts },
            "map",
            pixl_document::Placement::default(),
        )
        .unwrap();
    d.remove_layer(plain).unwrap();
    let cells: Vec<_> = Rect::new(0, 0, 4, 4).points().collect();
    d.place_tiles(map, 0, &cells, TileRef::new(1, TileFlags::NONE))
        .unwrap();
    d
}

/// Pastes a red 2x2 image from another program on [`tiled`], in the middle
/// of the view, where four cells meet, and keeps it with Enter, after
/// `setup` picks the tile draw mode.
fn paste_on_tiles(setup: fn(&mut pixl::Editor)) -> common::drive::Outcome {
    let image = RgbaImage::new(2, 2, [255, 0, 0, 255].repeat(4)).unwrap();
    drive(
        editor_over(tiled()).insert_resource(Pasteboard::holding(image)),
        steps(&[
            &[Step::Edit(setup)],
            &[Step::Ctrl("v", false), Step::Wait, Step::Wait],
            &enter(),
            &[Step::Wait],
        ]),
    )
}

/// The pixels of the one layer that are red.
fn reds(doc: &Document) -> Vec<(i32, i32)> {
    Rect::new(0, 0, doc.width() as i32, doc.height() as i32)
        .points()
        .filter(|p| px(doc, 0, p.x, p.y) == RED)
        .map(|p| (p.x, p.y))
        .collect()
}

#[test]
fn a_paste_on_a_tilemap_layer_lands_in_its_tiles_by_the_draw_mode() {
    // Manual: the paste edits tile 1, so it shows in every cell.
    let out = paste_on_tiles(|e| e.history.set_tile_mode(TileMode::Manual));
    assert_eq!(out.undo, vec!["Paste"]);
    let l = out.doc.layers()[0].id;
    let ts = out.doc.layer_tileset(l).unwrap();
    assert_eq!(out.doc.tileset(ts).unwrap().len(), 2, "no tile was made");
    let shown = reds(&out.doc);
    assert!(
        shown.contains(&(31, 31)) && shown.contains(&(32, 32)),
        "the paste is where it was put: {shown:?}"
    );
    assert!(
        shown.contains(&(15, 15)) && shown.contains(&(0, 0)) && shown.contains(&(63, 63)),
        "the edited tile shows in the other cells too"
    );
    assert_eq!(
        shown.len(),
        64,
        "each of the 16 cells shows the four corners"
    );

    // Auto: the four cells the paste covers get tiles of their own and the
    // rest keep tile 1.
    let out = paste_on_tiles(|e| e.history.set_tile_mode(TileMode::Auto));
    assert_eq!(out.undo, vec!["Paste"]);
    let l = out.doc.layers()[0].id;
    let ts = out.doc.layer_tileset(l).unwrap();
    assert_eq!(
        out.doc.tileset(ts).unwrap().len(),
        6,
        "four tiles were made"
    );
    let mut shown = reds(&out.doc);
    shown.sort_unstable();
    assert_eq!(shown, vec![(31, 31), (31, 32), (32, 31), (32, 32)]);
    let grid = out.doc.tile_grid(l, 0).unwrap().unwrap();
    assert_eq!(grid.get(0, 0).index(), 1, "a cell the paste missed");
    assert_ne!(grid.get(1, 1).index(), 1, "a cell the paste covered");
}
