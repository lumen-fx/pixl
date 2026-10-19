//! Aseprite files saved by Aseprite itself, checked against PNGs of the same
//! frames (and, for tilesets, PNGs of their tiles). The files come from the
//! test data of the asefile crate
//! (<https://github.com/alpine-alpaca/asefile>), MIT licensed; the license is
//! in `tests/data/aseprite/LICENSE`.

use pixl_document::{
    ColorMode, CompositeOptions, Document, LayerId, LayerKind, Point, Rgba, TilesetId,
    composite_frame,
};
use pixl_io::aseprite::{ImportWarning, export_aseprite, import_aseprite};
use pixl_io::png::decode_png;

fn file(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/data/aseprite/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Compares a frame's composite with a reference PNG. Fully transparent
/// pixels match whatever their color.
fn assert_frame(doc: &Document, frame: usize, png: &str) {
    assert_frame_within(doc, frame, png, 0);
}

/// [`assert_frame`] allowing each channel to differ by `tolerance`.
fn assert_frame_within(doc: &Document, frame: usize, png: &str, tolerance: u8) {
    let expected = decode_png(&file(png)).unwrap();
    assert_eq!(expected.size(), (doc.width(), doc.height()), "{png}");
    let expected = expected.to_rgba();
    let opts = CompositeOptions {
        background: false,
        ..CompositeOptions::EXPORT
    };
    let actual = composite_frame(doc, frame, &opts);
    for (i, (a, e)) in actual.iter().zip(&expected).enumerate() {
        if a.a == 0 && e.a == 0 {
            continue;
        }
        let close = [(a.r, e.r), (a.g, e.g), (a.b, e.b), (a.a, e.a)]
            .iter()
            .all(|(x, y)| x.abs_diff(*y) <= tolerance);
        assert!(
            close,
            "{png}: pixel ({}, {}) is {a:?}, expected {e:?}",
            i as u32 % doc.width(),
            i as u32 / doc.width()
        );
    }
}

fn import(name: &str) -> (Document, Vec<ImportWarning>) {
    let imp = import_aseprite(&file(name)).unwrap();
    // Every sample survives an export and re-import with the same frames.
    let again = import_aseprite(&export_aseprite(&imp.document).unwrap().bytes)
        .unwrap()
        .document;
    for f in 0..imp.document.frame_count() {
        assert_eq!(
            composite_frame(&again, f, &CompositeOptions::EXPORT),
            composite_frame(&imp.document, f, &CompositeOptions::EXPORT),
            "{name} frame {f} after re-export"
        );
    }
    (imp.document, imp.warnings)
}

#[test]
fn basic() {
    let (d, w) = import("basic-16x16.aseprite");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!((d.width(), d.height(), d.frame_count()), (16, 16, 1));
    assert_eq!(d.walk().len(), 1);
    assert_frame(&d, 0, "basic-16x16.png");
}

#[test]
fn layers_and_tags() {
    let (d, w) = import("layers_and_tags.aseprite");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(d.frame_count(), 4);
    assert_eq!(d.walk().len(), 6);
    assert_eq!(d.tags().len(), 3);
    assert!(d.walk().iter().any(|(depth, _)| *depth > 0), "has groups");
    for f in 0..4 {
        assert_frame(&d, f, &format!("layers_and_tags_0{}.png", f + 1));
    }
}

#[test]
fn linked_cels() {
    let (d, w) = import("linked_cels.aseprite");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(d.frame_count(), 3);
    assert_eq!(d.walk().len(), 3);
    let linked = d.walk().iter().any(|(_, l)| {
        matches!(l.kind, LayerKind::Raster { .. })
            && (0..3).any(|f| d.linked_frames(l.id, f).unwrap().len() > 1)
    });
    assert!(linked, "linked cels share one image");
    for f in 0..3 {
        assert_frame(&d, f, &format!("linked_cels_0{}.png", f + 1));
    }
}

#[test]
fn transparency() {
    let (d, _) = import("transparency.aseprite");
    assert_eq!(d.frame_count(), 2);
    // Semi-transparent layers: Aseprite blends in 8-bit integer steps and
    // pixl in floating point, so channels can round one step apart.
    assert_frame_within(&d, 0, "transparency_01.png", 1);
    assert_frame_within(&d, 1, "transparency_02.png", 1);
}

#[test]
fn background_layer() {
    let (d, _) = import("background.aseprite");
    assert_frame(&d, 0, "background.png");
}

#[test]
fn indexed() {
    let (d, w) = import("indexed.aseprite");
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(d.mode(), ColorMode::Indexed);
    assert_frame(&d, 0, "indexed_01.png");
}

#[test]
fn old_palette_chunk() {
    let (d, _) = import("256_color_old_palette_chunk.aseprite");
    assert_eq!(d.mode(), ColorMode::Indexed);
    assert_eq!(d.palette().len(), 256);
    assert_eq!(d.transparent_index(), 0);
    assert_frame(&d, 0, "256_color_old_palette_chunk.png");
}

#[test]
fn grayscale_opens_as_rgba() {
    let (d, w) = import("grayscale.aseprite");
    assert_eq!(w, vec![ImportWarning::GrayscaleConverted]);
    assert_eq!(d.mode(), ColorMode::Rgba);
    assert_frame(&d, 0, "grayscale.png");
}

#[test]
fn palette_with_alpha() {
    let (d, _) = import("palette.aseprite");
    assert_eq!(d.palette().len(), 85);
    assert_eq!(
        d.palette().get(0),
        pixl_document::Rgba::new(46, 34, 47, 255)
    );
    assert_eq!(d.palette().get(71), pixl_document::Rgba::new(0, 0, 0, 83));
}

#[test]
fn slices_keep_their_first_key() {
    let (d, w) = import("slice_advanced.aseprite");
    assert_eq!(d.slices().len(), 2);
    let s1 = &d.slices()[0];
    assert_eq!(s1.name, "Slice 1");
    assert_eq!(s1.pivot.unwrap().x, 4);
    let center = d.slices()[1].center.unwrap();
    assert_eq!((center.x, center.y, center.w, center.h), (3, 3, 2, 2));
    assert!(w.contains(&ImportWarning::SliceKeysFlattened {
        slice: "Slice 1".into()
    }));
}

#[test]
fn cels_larger_than_the_canvas_are_cropped() {
    let (d, _) = import("cel_overflow.aseprite");
    let layer = d
        .walk()
        .iter()
        .find(|(_, l)| l.is_raster())
        .map(|(_, l)| l.id)
        .unwrap();
    let r = d.cel_raster(layer, 0).unwrap().unwrap();
    assert_eq!((r.width(), r.height()), (d.width(), d.height()));
}

/// The tilemap layers of a document, bottom first.
fn tilemaps(d: &Document) -> Vec<LayerId> {
    d.walk()
        .iter()
        .filter(|(_, l)| l.is_tilemap())
        .map(|(_, l)| l.id)
        .collect()
}

/// A tile's pixels as RGBA, row-major.
fn tile_rgba(d: &Document, ts: TilesetId, index: u32) -> Vec<Rgba> {
    let set = d.tileset(ts).unwrap();
    set.tile(index)
        .unwrap()
        .iter()
        .map(|&v| match d.mode() {
            ColorMode::Rgba => Rgba::from_u32(v),
            ColorMode::Indexed => d.palette().get(v as u8),
        })
        .collect()
}

/// Compares RGBA pixels with a PNG; fully transparent pixels match any
/// color.
fn assert_pixels(actual: &[Rgba], png: &str) {
    let expected = decode_png(&file(png)).unwrap().to_rgba();
    assert_eq!(actual.len(), expected.len(), "{png}");
    for (i, (a, e)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            a == e || (a.a == 0 && e.a == 0),
            "{png}: pixel {i} is {a:?}, expected {e:?}"
        );
    }
}

#[test]
fn tilemaps_open_as_tilemap_layers() {
    for (name, png) in [
        ("tilemap.aseprite", "tilemap.png"),
        ("tilemap_indexed.aseprite", "tilemap_indexed.png"),
        ("tilemap_grayscale.aseprite", "tilemap_grayscale.png"),
    ] {
        let (d, w) = import(name);
        assert!(
            w.iter()
                .all(|w| matches!(w, ImportWarning::GrayscaleConverted)),
            "{name}: {w:?}"
        );
        assert!(!d.tilesets().is_empty(), "{name}");
        assert_eq!(d.tilesets()[0].name, "test_tileset");
        assert!(!tilemaps(&d).is_empty(), "{name}");
        assert_frame(&d, 0, png);
    }
}

#[test]
fn tilesets_keep_their_tiles() {
    let (d, _) = import("tileset.aseprite");
    let ts = d.tilesets()[0].id;
    let set = d.tileset(ts).unwrap();
    assert_eq!(set.tile_size(), (16, 16));
    // Aseprite's tileset image: every tile, the empty one first, in a column.
    let strip: Vec<Rgba> = (0..set.len() as u32)
        .flat_map(|i| tile_rgba(&d, ts, i))
        .collect();
    assert_pixels(&strip, "tileset.png");
    assert_pixels(&tile_rgba(&d, ts, 1), "tileset_1.png");
}

#[test]
fn tilemaps_share_tilesets_and_keep_their_cells() {
    let (d, _) = import("tilemap_multi.aseprite");
    assert_frame(&d, 0, "tilemap_multi.png");
    let maps = tilemaps(&d);
    assert_eq!(maps.len(), 2);
    for (layer, png) in maps
        .iter()
        .zip(["tilemap_multi_map1.png", "tilemap_multi_map2.png"])
    {
        // One layer alone, as Aseprite exports it.
        let mut alone = d.clone();
        for (_, l) in d.walk() {
            alone
                .update_layer(l.id, |p| p.visible = l.id == *layer)
                .unwrap();
        }
        assert_frame(&alone, 0, png);
    }
    let map1 = d
        .walk()
        .iter()
        .find(|(_, l)| l.props.name == "Tilemap 1")
        .map(|(_, l)| l.id)
        .unwrap();
    let ts = d.layer_tileset(map1).unwrap();
    assert_eq!(d.tileset(ts).unwrap().tile_size(), (20, 16));
    assert_pixels(&tile_rgba(&d, ts, 3), "tilemap_single_tile_1.png");
    // Cells as asefile reads them, relative to the cel's own grid.
    let grid = d.tile_grid(map1, 0).unwrap().unwrap();
    let cell = |x: i32, y: i32| {
        let p = Point::new(grid.origin.x, grid.origin.y);
        let at = d
            .tile_at(map1, 0, Point::new(p.x + x * 20, p.y + y * 16))
            .unwrap();
        at.map_or(0, |(_, t)| t.index())
    };
    // The grid starts at the canvas's first column of cells; asefile counts
    // from the cel's first stored cell, which is the same here.
    assert_eq!(
        (cell(0, 0), cell(0, 2), cell(0, 3), cell(11, 5), cell(4, 7)),
        (0, 4, 2, 3, 3)
    );
}

#[test]
fn tiles_at_the_edges_keep_their_empty_pixels() {
    let (d, _) = import("tilemap_empty_edges.aseprite");
    let layer = tilemaps(&d)[0];
    let ts = d.layer_tileset(layer).unwrap();
    let grid = d.tile_grid(layer, 0).unwrap().unwrap();
    let o = grid.origin;
    let t00 = d.tile_at(layer, 0, o).unwrap().unwrap().1;
    assert_eq!(t00.index(), 1);
    assert_pixels(
        &tile_rgba(&d, ts, t00.index()),
        "tilemap_empty_edges_0_0.png",
    );
    let t01 = d
        .tile_at(layer, 0, Point::new(o.x, o.y + 16))
        .unwrap()
        .unwrap()
        .1;
    assert_pixels(
        &tile_rgba(&d, ts, t01.index()),
        "tilemap_empty_edges_0_1.png",
    );
}

#[test]
fn tilemaps_survive_a_round_trip() {
    for name in [
        "tilemap.aseprite",
        "tilemap_indexed.aseprite",
        "tilemap_multi.aseprite",
        "tileset.aseprite",
    ] {
        let d = import_aseprite(&file(name)).unwrap().document;
        let again = import_aseprite(&export_aseprite(&d).unwrap().bytes)
            .unwrap()
            .document;
        assert_eq!(again.tilesets().len(), d.tilesets().len(), "{name}");
        for (a, b) in d.tilesets().iter().zip(again.tilesets()) {
            assert_eq!(a.tiles().collect::<Vec<_>>(), b.tiles().collect::<Vec<_>>());
            assert_eq!((&a.name, a.base_index), (&b.name, b.base_index));
        }
        let (ma, mb) = (tilemaps(&d), tilemaps(&again));
        assert_eq!(ma.len(), mb.len(), "{name}");
        for (a, b) in ma.iter().zip(&mb) {
            for f in 0..d.frame_count() {
                let cells = |doc: &Document, l: LayerId| {
                    doc.tile_grid(l, f)
                        .unwrap()
                        .map(|g| (g.origin, g.to_cells()))
                };
                assert_eq!(cells(&d, *a), cells(&again, *b), "{name} frame {f}");
            }
        }
    }
}

#[test]
fn truncated_samples_error_cleanly() {
    for name in [
        "layers_and_tags.aseprite",
        "linked_cels.aseprite",
        "slice_advanced.aseprite",
        "tilemap.aseprite",
        "tilemap_multi.aseprite",
    ] {
        let bytes = file(name);
        for len in 0..bytes.len() {
            assert!(
                import_aseprite(&bytes[..len]).is_err(),
                "{name} prefix {len}"
            );
        }
    }
}
