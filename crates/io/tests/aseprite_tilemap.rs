//! A tilemap file written byte by byte from the Aseprite file specification,
//! with every flip and a linked tilemap cel, checked pixel by pixel against
//! what the specification says each flip shows.

use pixl_document::native::{SaveOptions, load, save};
use pixl_document::{
    CompositeOptions, Document, LayerId, Point, Rgba, TileFlags, TileRef, composite_frame,
};
use pixl_io::aseprite::{export_aseprite, import_aseprite};

const R: Rgba = Rgba::opaque(255, 0, 0);
const G: Rgba = Rgba::opaque(0, 255, 0);
const B: Rgba = Rgba::opaque(0, 0, 255);
const GRAY: Rgba = Rgba::opaque(9, 9, 9);

fn fixture() -> Document {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/tilemap_flips.aseprite"
    );
    let imp = import_aseprite(&std::fs::read(path).unwrap()).unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    imp.document
}

fn map(d: &Document) -> LayerId {
    d.walk()
        .iter()
        .find(|(_, l)| l.is_tilemap())
        .map(|(_, l)| l.id)
        .unwrap()
}

fn at(px: &[Rgba], x: u32, y: u32) -> Rgba {
    px[(y * 32 + x) as usize]
}

/// Every pixel the fixture's cells set, by the specification's flips: X
/// mirrors columns, Y mirrors rows, D swaps x and y after both.
fn assert_shows(d: &Document, frame: usize) {
    let px = composite_frame(d, frame, &CompositeOptions::EXPORT);
    let expect = [
        // Tile 1 as it is: red, green and blue corners.
        ((0, 0), R),
        ((7, 0), G),
        ((0, 7), B),
        // Flipped on X.
        ((15, 0), R),
        ((8, 0), G),
        ((15, 7), B),
        // Flipped on Y.
        ((16, 7), R),
        ((23, 7), G),
        ((16, 0), B),
        // Flipped on the diagonal.
        ((24, 0), R),
        ((24, 7), G),
        ((31, 0), B),
        // Tile 2's green top row, all three flips: the right column.
        ((7, 8), G),
        ((7, 15), G),
        // Tile 2 as it is.
        ((16, 8), G),
        ((23, 8), G),
    ];
    for ((x, y), c) in expect {
        assert_eq!(at(&px, x, y), c, "frame {frame} pixel ({x}, {y})");
    }
    // The raster layer below, on the first frame only, shows where the map
    // is clear.
    let under = if frame == 0 { GRAY } else { Rgba::TRANSPARENT };
    assert_eq!(at(&px, 1, 0), under, "frame {frame} pixel (1, 0)");
    for (x, y) in [(0, 8), (6, 8), (8, 8), (17, 9), (3, 3)] {
        assert_eq!(
            at(&px, x, y).a,
            0,
            "frame {frame} pixel ({x}, {y}) is clear"
        );
    }
}

#[test]
fn every_flip_shows_as_the_spec_says() {
    let d = fixture();
    assert_eq!(d.frame_count(), 2);
    let set = &d.tilesets()[0];
    assert_eq!(
        (set.name.as_str(), set.tile_size(), set.len()),
        ("flips", (8, 8), 3)
    );
    let layer = map(&d);
    assert_eq!(d.linked_frames(layer, 0).unwrap(), vec![0, 1]);
    let cells: Vec<TileRef> = d.tile_grid(layer, 0).unwrap().unwrap().to_cells();
    let f = |x, y, d| TileFlags { x, y, d };
    assert_eq!(
        cells,
        vec![
            TileRef::new(1, TileFlags::NONE),
            TileRef::new(1, f(true, false, false)),
            TileRef::new(1, f(false, true, false)),
            TileRef::new(1, f(false, false, true)),
            TileRef::new(2, f(true, true, true)),
            TileRef::EMPTY,
            TileRef::new(2, TileFlags::NONE),
            TileRef::EMPTY,
        ]
    );
    assert_shows(&d, 0);
    assert_shows(&d, 1);
}

#[test]
fn the_fixture_round_trips_through_aseprite_and_pixl() {
    let d = fixture();
    let ase = import_aseprite(&export_aseprite(&d).unwrap().bytes)
        .unwrap()
        .document;
    let native = load(&save(&d, SaveOptions::default())).unwrap().document;
    for doc in [&ase, &native] {
        assert_shows(doc, 0);
        assert_shows(doc, 1);
        let layer = map(doc);
        assert_eq!(
            doc.tile_grid(layer, 0).unwrap().unwrap().to_cells(),
            d.tile_grid(map(&d), 0).unwrap().unwrap().to_cells()
        );
        assert_eq!(doc.linked_frames(layer, 0).unwrap(), vec![0, 1]);
    }
    assert_eq!(native, d);
}

/// The position and size in cells of each tilemap cel chunk in a file.
fn tilemap_cels(bytes: &[u8]) -> Vec<(i16, i16, u16, u16)> {
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut frame = 128;
    while frame < bytes.len() {
        let len = u32_at(frame) as usize;
        let chunks = u32_at(frame + 12);
        let mut c = frame + 16;
        for _ in 0..chunks {
            let size = u32_at(c) as usize;
            // Cel chunk: layer, x, y, opacity, then the cel type.
            if u16_at(c + 4) == 0x2005 && u16_at(c + 6 + 7) == 3 {
                let x = u16_at(c + 6 + 2) as i16;
                let y = u16_at(c + 6 + 4) as i16;
                out.push((x, y, u16_at(c + 6 + 16), u16_at(c + 6 + 18)));
            }
            c += size;
        }
        frame += len;
    }
    out
}

#[test]
fn exported_tilemap_cels_hold_only_their_tiles() {
    // A cel is stored from its first to its last used cell, at that cell's
    // place on the canvas, as Aseprite stores them.
    let mut d = fixture();
    let layer = map(&d);
    let cells: Vec<Point> = (0..4).map(|x| Point::new(x, 0)).collect();
    d.place_tiles(layer, 0, &cells, TileRef::EMPTY).unwrap();
    let bytes = export_aseprite(&d).unwrap().bytes;
    assert_eq!(tilemap_cels(&bytes), vec![(0, 8, 3, 1)]);
    let again = import_aseprite(&bytes).unwrap().document;
    let l = map(&again);
    assert_eq!(
        again.tile_at(l, 0, Point::new(16, 8)).unwrap().unwrap().1,
        TileRef::new(2, TileFlags::NONE)
    );
    assert_eq!(
        composite_frame(&again, 0, &CompositeOptions::EXPORT),
        composite_frame(&d, 0, &CompositeOptions::EXPORT)
    );
}
