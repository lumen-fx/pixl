//! Large rewrites run on the workers and reach the buffers tile by tile,
//! nearest the middle of the view first.

use std::time::{Duration, Instant};

use pixl::Editor;
use pixl::editor::Touched;
use pixl::view::{ItemSource, Layers};
use pixl_document::{ColorMode, Document, NewLayer, Placement, Rgba, TILE_SIZE};

fn painted(side: u32) -> Document {
    let mut doc = Document::new(side, side, ColorMode::Rgba).unwrap();
    let id = doc.layers()[0].id;
    let t = doc.target(id, 0).unwrap();
    for y in 0..side as i32 {
        for x in 0..side as i32 {
            t.raster
                .set_raw(x, y, Rgba::opaque(x as u8, y as u8, 7).to_u32());
        }
    }
    doc
}

fn settle(layers: &mut Layers, width: u32) {
    let until = Instant::now() + Duration::from_secs(30);
    while layers.pending() > 0 && Instant::now() < until {
        layers.drain(width);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(layers.pending(), 0, "every tile arrived");
}

#[test]
fn an_opened_document_reaches_its_buffer_tile_by_tile() {
    let side = 1024;
    let editor = Editor::new(painted(side));
    let id = editor.doc.layers()[0].id;
    let mut layers = Layers::default();
    layers.sync(&editor, &Touched::All, (512.0, 512.0));
    let tiles = (side / TILE_SIZE as u32).pow(2) as usize;
    assert!(layers.pending() > 0, "a whole document goes to the workers");
    settle(&mut layers, side);
    let r = &layers.rewrites[0];
    assert_eq!((r.tiles, r.done, r.arrivals.len()), (tiles, tiles, tiles));
    let buffer = layers.buffer(&ItemSource::Layer(id)).unwrap();
    let mut px = [0u8; 4];
    for (x, y) in [(0, 0), (511, 512), (1023, 1023), (300, 77)] {
        buffer.read_rgba8(x, y, 1, 1, &mut px).unwrap();
        assert_eq!(px, [x as u8, y as u8, 7, 255], "pixel {x},{y}");
    }
}

#[test]
fn a_stroke_rewrites_only_its_tiles_on_the_spot() {
    let side = 1024;
    let mut editor = Editor::new(painted(side));
    let id = editor.doc.layers()[0].id;
    let mut layers = Layers::default();
    let all = editor.take_touched();
    layers.sync(&editor, &all, (0.0, 0.0));
    settle(&mut layers, side);
    let rewrites = layers.rewrites.len();
    editor
        .doc
        .target(id, 0)
        .unwrap()
        .raster
        .set_raw(700, 700, Rgba::opaque(1, 2, 3).to_u32());
    editor.touch();
    layers.sync(&editor, &Touched::All, (0.0, 0.0));
    assert_eq!(
        layers.pending(),
        0,
        "one tile is written without the workers"
    );
    assert_eq!(layers.rewrites.len(), rewrites);
    let mut px = [0u8; 4];
    layers
        .buffer(&ItemSource::Layer(id))
        .unwrap()
        .read_rgba8(700, 700, 1, 1, &mut px)
        .unwrap();
    assert_eq!(px, [1, 2, 3, 255]);
}

#[test]
fn a_group_is_composited_by_the_workers() {
    let mut doc = Document::new(512, 512, ColorMode::Rgba).unwrap();
    let g = doc
        .add_layer(NewLayer::Group, "g", Placement::default())
        .unwrap();
    let inner = doc
        .add_layer(
            NewLayer::Raster,
            "in",
            Placement {
                parent: Some(g),
                index: None,
            },
        )
        .unwrap();
    {
        let t = doc.target(inner, 0).unwrap();
        for y in 0..512 {
            for x in 0..512 {
                t.raster.set_raw(x, y, Rgba::new(200, 10, 20, 128).to_u32());
            }
        }
    }
    let editor = Editor::new(doc);
    let mut layers = Layers::default();
    layers.sync(&editor, &Touched::All, (256.0, 256.0));
    settle(&mut layers, 512);
    let mut px = [0u8; 4];
    layers
        .buffer(&ItemSource::Isolated(g))
        .unwrap()
        .read_rgba8(400, 30, 1, 1, &mut px)
        .unwrap();
    assert_eq!(px, [200, 10, 20, 128]);
}

#[test]
fn turning_the_onion_skin_off_mid_draw_waits_for_nothing() {
    let side = 1024;
    let mut doc = painted(side);
    doc.insert_frame(1, 100).unwrap();
    let id = doc.layers()[0].id;
    doc.link_cel(id, 0, 1).unwrap();
    let editor = Editor::new(doc);
    let mut layers = Layers::default();
    layers.sync(&editor, &Touched::All, (0.0, 0.0));
    settle(&mut layers, side);
    // The ghosts of a whole frame go to the workers...
    layers.onion = Some(pixl_document::OnionSkin::default());
    layers.sync(&editor, &Touched::Layers(Vec::new()), (0.0, 0.0));
    assert!(layers.buffer(&ItemSource::Ghosts).is_some());
    assert!(layers.pending() > 0);
    // ...and are dropped when the skin goes before they arrive.
    layers.onion = None;
    layers.sync(&editor, &Touched::Layers(Vec::new()), (0.0, 0.0));
    assert!(layers.buffer(&ItemSource::Ghosts).is_none());
    settle(&mut layers, side);
}
