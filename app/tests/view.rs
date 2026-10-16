//! The canvas view, rendered with no window: the document's pixels on
//! screen, every blend mode against pixl-core's own compositing, the grid,
//! and zoom.

pub mod common;

use std::sync::{Arc, Mutex};

use lumenui::prelude::*;
use pixl::view::{ItemSource, View, canvas_blend, plan};
use pixl::{Editor, editor};
use pixl_core::{
    BlendMode, ColorMode, CompositeOptions, Document, LayerId, NewLayer, Placement, Rect, Rgba,
    composite,
};

/// Runs the editor on `backend` over `doc`, with `setup` run on the first
/// tick, and returns the frame and the view it was drawn with.
fn render(
    backend: &str,
    doc: Document,
    ticks: u64,
    setup: impl Fn(&mut View) + Send + Sync + 'static,
) -> (Frame, View) {
    let _turn = common::turn();
    let seen: Arc<Mutex<Option<View>>> = Arc::default();
    let out = Arc::clone(&seen);
    let frame = editor(Source::Disk {
        dir: common::app_dir(backend),
    })
    .insert_resource(Editor::new(doc))
    .add_systems(
        TickStage::Systems,
        move |mut view: ResMut<View>, mut n: Local<u32>| {
            *n += 1;
            if *n == 3 {
                setup(&mut view);
            }
            *out.lock().unwrap() = Some(view.clone());
        },
    )
    .screenshot(HeadlessOptions {
        dpr: 1.0,
        ticks: Some(ticks),
    })
    .expect("the editor renders");
    let view = seen.lock().unwrap().clone().expect("the view ran");
    (frame, view)
}

/// The screen color of document pixel `(x, y)`, sampled at its middle.
fn on_screen(frame: &Frame, view: &View, x: i32, y: i32) -> [u8; 4] {
    let sx = view.stage.0.x as f64 + view.origin.0 + (f64::from(x) + 0.5) * view.zoom;
    let sy = view.stage.0.y as f64 + view.origin.1 + (f64::from(y) + 0.5) * view.zoom;
    frame.pixel(sx as u32, sy as u32).expect("inside the frame")
}

fn paint(doc: &mut Document, layer: LayerId, x: i32, y: i32, c: Rgba) {
    doc.target(layer, 0)
        .unwrap()
        .raster
        .set_raw(x, y, c.to_u32());
}

/// An opaque backdrop with a band per blend mode above it: band `i` holds
/// semi-transparent and opaque colors on a layer of mode `i`, so each band
/// shows one mode over the same backdrop.
fn bands(modes: &[BlendMode]) -> Document {
    let w = 16;
    let h = 4 * modes.len() as u32;
    let mut doc = Document::new(w, h, ColorMode::Rgba).unwrap();
    let base = doc.layers()[0].id;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let c = Rgba::opaque(
                (x * 16) as u8,
                (y * 255 / h as i32) as u8,
                200 - (x * 9) as u8,
            );
            paint(&mut doc, base, x, y, c);
        }
    }
    for (i, mode) in modes.iter().enumerate() {
        let id = doc
            .add_layer(NewLayer::Raster, format!("{mode:?}"), Placement::default())
            .unwrap();
        doc.update_layer(id, |p| {
            p.blend = *mode;
            p.opacity = 230;
        })
        .unwrap();
        for y in 0..4 {
            for x in 0..w as i32 {
                let a = [255u8, 192, 128, 64][(x % 4) as usize];
                let c = Rgba::new(255 - (x * 15) as u8, (x * 13) as u8, (y * 60) as u8, a);
                paint(&mut doc, id, x, i as i32 * 4 + y, c);
            }
        }
    }
    doc
}

/// The largest channel difference per mode between the screen and
/// pixl-core's composite of the same document.
fn blend_errors(backend: &str, modes: &[BlendMode]) -> Vec<(BlendMode, u8)> {
    let doc = bands(modes);
    let expected = composite(&doc, 0, doc.bounds(), &CompositeOptions::default());
    let w = doc.width() as i32;
    let (frame, view) = render(backend, doc, 10, |_| {});
    common::save(
        &frame,
        &format!("view-blend-bands-{backend}-{}", modes.len()),
    );
    modes
        .iter()
        .enumerate()
        .map(|(i, mode)| {
            let mut worst = 0u8;
            for y in i as i32 * 4..i as i32 * 4 + 4 {
                for x in 0..w {
                    let want = expected[(y * w + x) as usize];
                    let got = on_screen(&frame, &view, x, y);
                    for (a, b) in [want.r, want.g, want.b].into_iter().zip(got) {
                        worst = worst.max(a.abs_diff(b));
                    }
                }
            }
            (*mode, worst)
        })
        .collect()
}

/// Lumen composites a blend mode in 8 bits per channel, pixl-core in
/// floating point; one or two steps apart is rounding.
const ROUNDING: u8 = 3;

fn check_blends(backend: &str) {
    // The modes the view hands to Lumen's renderer: one buffer per layer.
    let canvas: Vec<BlendMode> = BlendMode::ALL
        .into_iter()
        .filter(|m| canvas_blend(*m).is_some())
        .collect();
    let doc = bands(&canvas);
    assert!(
        plan(&doc, 0)
            .iter()
            .all(|i| matches!(i.source, ItemSource::Layer(_))),
        "every band is its own buffer"
    );
    // The rest flatten on the CPU.
    let cpu = [BlendMode::Add, BlendMode::Subtract];
    let errors: Vec<_> = blend_errors(backend, &canvas)
        .into_iter()
        .chain(blend_errors(backend, &cpu))
        .collect();
    eprintln!("{backend} blend errors: {errors:?}");
    for (mode, worst) in errors {
        assert!(
            worst <= ROUNDING,
            "{mode:?} on {backend}: off by {worst} from pixl-core"
        );
    }
}

#[test]
fn every_blend_mode_matches_pixl_core_on_the_cpu_renderer() {
    check_blends("cpu");
}

#[test]
fn every_blend_mode_matches_pixl_core_on_the_gpu_renderer() {
    check_blends("gpu");
}

#[test]
fn the_document_fits_the_stage_with_sharp_pixels() {
    let mut doc = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let id = doc.layers()[0].id;
    paint(&mut doc, id, 0, 0, Rgba::opaque(255, 0, 0));
    paint(&mut doc, id, 31, 31, Rgba::opaque(0, 255, 0));
    let (frame, view) = render("cpu", doc, 8, |_| {});
    common::save(&frame, "view-fit");
    assert!(view.zoom >= 16.0, "a 32x32 document fits at a high zoom");
    assert_eq!(on_screen(&frame, &view, 0, 0), [255, 0, 0, 255]);
    assert_eq!(on_screen(&frame, &view, 31, 31), [0, 255, 0, 255]);
    // A transparent pixel shows the checkerboard, not the stage.
    let checker = on_screen(&frame, &view, 5, 5);
    assert_ne!(checker, [0x12, 0x12, 0x12, 0xff]);
}

#[test]
fn zooming_in_shows_the_pixel_grid() {
    let doc = Document::new(16, 16, ColorMode::Rgba).unwrap();
    let (frame, view) = render("cpu", doc, 8, |v| {
        let z = 32.0;
        let mid = v.middle();
        v.zoom_at(z, mid);
    });
    common::save(&frame, "view-grid");
    assert_eq!(view.zoom, 32.0);
    // A grid line lies on each pixel's left edge.
    let sx = view.stage.0.x as f64 + view.origin.0 + 4.0 * view.zoom;
    let sy = view.stage.0.y as f64 + view.origin.1 + 4.5 * view.zoom;
    let line = frame.pixel(sx as u32, sy as u32).unwrap();
    let cell = on_screen(&frame, &view, 4, 4);
    assert_ne!(line, cell);
}

#[test]
fn hiding_a_layer_redraws_without_it() {
    let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
    let base = doc.layers()[0].id;
    let top = doc
        .add_layer(NewLayer::Raster, "top", Placement::default())
        .unwrap();
    paint(&mut doc, base, 2, 2, Rgba::opaque(0, 0, 255));
    paint(&mut doc, top, 2, 2, Rgba::opaque(255, 255, 0));
    doc.update_layer(top, |p| p.visible = false).unwrap();
    let (frame, view) = render("cpu", doc, 8, |_| {});
    assert_eq!(on_screen(&frame, &view, 2, 2), [0, 0, 255, 255]);
    let _ = Rect::EMPTY;
}
