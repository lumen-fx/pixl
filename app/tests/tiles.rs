//! Tilemap layers driven by pointer, key and menu messages over a rendered
//! editor: placing tiles in Tiles mode, Auto mode making a tile from pixels,
//! Manual mode editing a tile everywhere it shows, the tile bucket and
//! rectangle, the eyedropper, the Tileset panel and the Layer menu's
//! tilemap commands.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::{NamedKey, PointerButton};
use pixl::Editor;
use pixl_document::{
    ColorMode, Document, LayerId, NewLayer, Placement, Point, Rgba, TileFlags, TileMode, TileRef,
    TilesetId,
};

const L: PointerButton = PointerButton::Primary;
const RED: Rgba = Rgba::opaque(255, 0, 0);

/// A 64x64 document whose only layer is a tilemap layer of 16x16 tiles:
/// tile 1 black, tile 2 white, with tile 1 in the top row's four cells.
fn map_doc() -> Document {
    let mut d = Document::new(64, 64, ColorMode::Rgba).unwrap();
    let plain = d.layers()[0].id;
    let ts = d.add_tileset("ground", 16, 16).unwrap();
    d.add_tile(ts, vec![Rgba::BLACK.to_u32(); 256]).unwrap();
    d.add_tile(ts, vec![Rgba::WHITE.to_u32(); 256]).unwrap();
    let map = d
        .add_layer(
            NewLayer::Tilemap { tileset: ts },
            "map",
            Placement::default(),
        )
        .unwrap();
    d.remove_layer(plain).unwrap();
    let cells: Vec<Point> = (0..4).map(|x| Point::new(x, 0)).collect();
    d.place_tiles(map, 0, &cells, TileRef::new(1, TileFlags::NONE))
        .unwrap();
    d
}

fn map_of(d: &Document) -> (LayerId, TilesetId) {
    let l = d
        .walk()
        .iter()
        .find(|(_, l)| l.is_tilemap())
        .map(|(_, l)| l.id)
        .expect("a tilemap layer");
    (l, d.layer_tileset(l).unwrap())
}

fn cell(d: &Document, x: i32, y: i32) -> TileRef {
    let (l, _) = map_of(d);
    d.tile_grid(l, 0)
        .unwrap()
        .map_or(TileRef::EMPTY, |g| g.get(x, y))
}

fn px(d: &Document, x: i32, y: i32) -> Rgba {
    let (l, _) = map_of(d);
    d.cel_raster(l, 0)
        .unwrap()
        .map_or(Rgba::TRANSPARENT, |r| Rgba::from_u32(r.get_raw(x, y)))
}

fn hover(x: i32, y: i32) -> Vec<Step> {
    vec![Step::Move(At::Doc(x, y)), Step::Wait]
}

fn drag(a: (i32, i32), b: (i32, i32)) -> Vec<Step> {
    let mut steps = hover(a.0, a.1);
    steps.push(Step::Down(At::Doc(a.0, a.1), L));
    for i in 1..=4 {
        steps.push(Step::Move(At::Doc(
            a.0 + (b.0 - a.0) * i / 4,
            a.1 + (b.1 - a.1) * i / 4,
        )));
    }
    steps.push(Step::Up(At::Doc(b.0, b.1), L));
    steps
}

fn with(mut first: Vec<Step>, rest: Vec<Step>) -> Vec<Step> {
    first.extend(rest);
    first
}

fn tiles_mode(e: &mut Editor) {
    e.tiles_mode = true;
}

fn white_tile(e: &mut Editor) {
    e.tile = TileRef::new(2, TileFlags::NONE);
}

fn manual(e: &mut Editor) {
    e.history.set_tile_mode(TileMode::Manual);
}

fn red(e: &mut Editor) {
    e.fg = RED;
}

#[test]
fn tab_switches_to_tiles_and_the_pencil_places_the_picked_tile() {
    let steps = with(
        vec![
            Step::Edit(white_tile),
            Step::KeyDown(NamedKey::Tab),
            Step::KeyUp(NamedKey::Tab),
            Step::Wait,
        ],
        drag((4, 20), (60, 52)),
    );
    let out = drive(editor_over(map_doc()), steps);
    let white = TileRef::new(2, TileFlags::NONE);
    // A diagonal across the grid places a cell at a time, with no gaps.
    for (x, y) in [(0, 1), (1, 1), (2, 2), (3, 3)] {
        assert_eq!(cell(&out.doc, x, y), white, "cell ({x}, {y})");
    }
    assert_eq!(px(&out.doc, 40, 40), Rgba::WHITE);
    assert_eq!(cell(&out.doc, 0, 0).index(), 1, "the top row is untouched");
    assert_eq!(out.doc.tileset(map_of(&out.doc).1).unwrap().len(), 3);
    assert_eq!(out.undo, vec!["Place tiles".to_string()]);
    common::save(&out.frame, "tiles-place");
}

#[test]
fn the_eraser_bucket_rectangle_and_eyedropper_work_on_cells() {
    let steps = [
        vec![Step::Edit(tiles_mode), Step::Edit(white_tile)],
        // The bucket fills the empty cells joined to the clicked one.
        vec![Step::Key("g", false)],
        hover(20, 40),
        vec![Step::Down(At::Doc(20, 40), L), Step::Up(At::Doc(20, 40), L)],
        // The eyedropper takes tile 1 from the top row.
        vec![Step::Key("i", false)],
        hover(50, 5),
        vec![Step::Down(At::Doc(50, 5), L), Step::Up(At::Doc(50, 5), L)],
        // The rectangle puts it over a block of cells.
        vec![Step::Key("u", false)],
        drag((20, 20), (40, 40)),
        // The eraser empties a cell.
        vec![Step::Key("e", false)],
        drag((2, 60), (2, 60)),
    ]
    .concat();
    let out = drive(editor_over(map_doc()), steps);
    let black = TileRef::new(1, TileFlags::NONE);
    let white = TileRef::new(2, TileFlags::NONE);
    assert_eq!(cell(&out.doc, 3, 3), white, "filled by the bucket");
    assert_eq!(cell(&out.doc, 0, 3), TileRef::EMPTY, "erased");
    for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
        assert_eq!(
            cell(&out.doc, x, y),
            black,
            "the rectangle's cell ({x}, {y})"
        );
    }
    assert_eq!(
        out.undo,
        vec!["Fill tiles", "Tile rectangle", "Erase tiles"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
}

#[test]
fn auto_mode_makes_a_tile_from_pixels() {
    // Pixels mode on a tilemap layer, the default Auto draw mode.
    let steps = with(vec![Step::Edit(red)], drag((20, 20), (20, 20)));
    let out = drive(editor_over(map_doc()), steps);
    let (_, ts) = map_of(&out.doc);
    let set = out.doc.tileset(ts).unwrap();
    assert_eq!(set.len(), 4, "the empty cell got a tile of its own");
    assert_eq!(cell(&out.doc, 1, 1).index(), 3);
    assert_eq!(px(&out.doc, 20, 20), RED);
    // A tile shown in four places is not changed by an edit to one.
    let steps = with(vec![Step::Edit(red)], drag((5, 5), (5, 5)));
    let out = drive(editor_over(map_doc()), steps);
    assert_eq!(px(&out.doc, 5, 5), RED);
    assert_eq!(
        px(&out.doc, 21, 5),
        Rgba::BLACK,
        "the other cells keep tile 1"
    );
    assert_ne!(cell(&out.doc, 0, 0).index(), 1);
    assert_eq!(cell(&out.doc, 1, 0).index(), 1);
}

#[test]
fn manual_mode_edits_the_tile_everywhere_it_shows() {
    let steps = with(
        vec![Step::Edit(manual), Step::Edit(red)],
        drag((5, 5), (8, 5)),
    );
    let out = drive(editor_over(map_doc()), steps);
    for cell_x in 0..4 {
        for x in 5..=8 {
            assert_eq!(
                px(&out.doc, cell_x * 16 + x, 5),
                RED,
                "cell {cell_x} pixel {x}"
            );
        }
    }
    let (_, ts) = map_of(&out.doc);
    assert_eq!(out.doc.tileset(ts).unwrap().len(), 3, "no tile was made");
    assert_eq!(out.undo, vec!["Pencil".to_string()]);
    common::save(&out.frame, "tiles-manual");
}

#[test]
fn the_tileset_panel_picks_a_tile_for_the_tools() {
    let steps = [
        vec![
            Step::Menu("window-layout-tiles"),
            Step::Wait,
            Step::Wait,
            // Tile 2 is the third cell of the panel's first row: tiles of
            // 16 pixels show at 32, one pixel apart.
            Step::Move(At::Offset("tileset-canvas", 70.0, 10.0)),
            Step::Wait,
            Step::Down(At::Offset("tileset-canvas", 70.0, 10.0), L),
            Step::Up(At::Offset("tileset-canvas", 70.0, 10.0), L),
            Step::Wait,
        ],
        drag((20, 40), (20, 40)),
    ]
    .concat();
    let out = drive(editor_over(map_doc()), steps);
    common::save(&out.frame, "tiles-panel");
    assert_eq!(cell(&out.doc, 1, 2), TileRef::new(2, TileFlags::NONE));
}

#[test]
fn the_layer_menu_makes_and_converts_tilemap_layers() {
    let mut doc = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let base = doc.layers()[0].id;
    {
        let t = doc.target(base, 0).unwrap();
        for x in 0..32 {
            t.raster.set_raw(x, 3, RED.to_u32());
        }
    }
    let steps = vec![
        Step::Menu("lm-tilemap-convert"),
        Step::Wait,
        Step::Text("tm_width", "8"),
        Step::Text("tm_height", "8"),
        Step::Wait,
        Step::Click("tm-ok"),
        Step::Wait,
        Step::Menu("lm-tilemap-new"),
        Step::Wait,
        Step::Click("tm-set-1"),
        Step::Wait,
        Step::Click("tm-ok"),
        Step::Wait,
    ];
    let out = drive(editor_over(doc), steps);
    let maps: Vec<LayerId> = out
        .doc
        .walk()
        .iter()
        .filter(|(_, l)| l.is_tilemap())
        .map(|(_, l)| l.id)
        .collect();
    assert_eq!(maps.len(), 2, "the converted layer and the new one");
    assert_eq!(out.doc.tilesets().len(), 1, "both use the one tileset");
    let set = &out.doc.tilesets()[0];
    assert_eq!(set.tile_size(), (8, 8));
    assert_eq!(set.len(), 2, "four red cells share one tile");
    assert_eq!(
        out.undo,
        vec!["Convert to tilemap", "New tilemap layer"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
    // The dialog, for a look.
    let out = drive(
        editor_over(map_doc()),
        vec![Step::Menu("lm-tilemap-new"), Step::Wait, Step::Wait],
    );
    common::save(&out.frame, "tiles-dialog");
    // And back to a normal layer keeping its pixels.
    let steps = vec![Step::Menu("lm-tilemap-raster"), Step::Wait];
    let out = drive(editor_over(map_doc()), steps);
    assert!(out.doc.walk().iter().all(|(_, l)| !l.is_tilemap()));
    let l = out.doc.layers()[0].id;
    let r = out.doc.cel_raster(l, 0).unwrap().unwrap();
    assert_eq!(Rgba::from_u32(r.get_raw(40, 3)), Rgba::BLACK);
}
