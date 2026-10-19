//! The canvas view, rendered with no window: the document's pixels on
//! screen, every blend mode against pixl-document's own compositing, the grid,
//! zoom, and reference images drawn at screen resolution.

pub mod common;

use std::sync::{Arc, Mutex};

use lumenui::prelude::*;
use pixl::view::{ItemSource, View, canvas_blend, plan};
use pixl::{Editor, Settings, editor};
use pixl_document::{
    BlendMode, ColorMode, CompositeOptions, Document, LayerId, NewLayer, Placement, Rect,
    ReferencePlace, Rgba, TiledImage, composite,
};

/// Runs the editor on `backend` over `doc`, with `setup` run on the first
/// tick, and returns the frame and the view it was drawn with.
fn render(
    backend: &str,
    doc: Document,
    ticks: u64,
    setup: impl Fn(&mut View) + Send + Sync + 'static,
) -> (Frame, View) {
    render_with(backend, doc, Settings::default(), ticks, setup)
}

/// [`render`] with `settings` in force.
fn render_with(
    backend: &str,
    doc: Document,
    settings: Settings,
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
    .insert_resource(settings)
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
/// pixl-document's composite of the same document.
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

/// Lumen composites a blend mode in 8 bits per channel, pixl-document in
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
            "{mode:?} on {backend}: off by {worst} from pixl-document"
        );
    }
}

#[test]
fn every_blend_mode_matches_pixl_document_on_the_cpu_renderer() {
    check_blends("cpu");
}

#[test]
fn every_blend_mode_matches_pixl_document_on_the_gpu_renderer() {
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
    assert!(view.zoom >= 12.0, "a 32x32 document fits at a high zoom");
    assert_eq!(on_screen(&frame, &view, 0, 0), [255, 0, 0, 255]);
    assert_eq!(on_screen(&frame, &view, 31, 31), [0, 255, 0, 255]);
    // A transparent pixel shows the checkerboard, not the stage.
    let checker = on_screen(&frame, &view, 5, 5);
    assert_ne!(checker, [0x12, 0x12, 0x12, 0xff]);
}

#[test]
fn zooming_in_shows_the_pixel_grid_once_it_is_on() {
    let doc = Document::new(16, 16, ColorMode::Rgba).unwrap();
    let mut settings = Settings::default();
    settings.canvas.pixel_grid = true;
    let (frame, view) = render_with("cpu", doc, settings, 8, |v| {
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

/// A 512x512 ramp: red climbs left to right, green top to bottom.
fn ramp() -> TiledImage<Rgba> {
    let mut img = TiledImage::new(512, 512, Rgba::TRANSPARENT);
    for y in 0..512 {
        for x in 0..512 {
            img.set(x, y, Rgba::opaque((x / 2) as u8, (y / 2) as u8, 60));
        }
    }
    img
}

/// A 32x32 document with one blue pixel at (20, 20) and the ramp as a
/// reference covering the canvas, turned by `rotation`, under the art or
/// over it.
fn traced(rotation: f64, over: bool) -> Document {
    let mut doc = Document::new(32, 32, ColorMode::Rgba).unwrap();
    let art = doc.layers()[0].id;
    paint(&mut doc, art, 20, 20, Rgba::opaque(0, 0, 255));
    doc.add_layer(
        NewLayer::Reference {
            image: ramp(),
            place: ReferencePlace {
                center: (16.0, 16.0),
                scale: (32.0 / 512.0, 32.0 / 512.0),
                rotation,
            },
        },
        "photo",
        Placement {
            parent: None,
            index: Some(if over { 1 } else { 0 }),
        },
    )
    .unwrap();
    doc
}

/// The screen color at document position `(x, y)`, which need not be a
/// pixel's middle.
fn at_doc(frame: &Frame, view: &View, x: f64, y: f64) -> [u8; 4] {
    let sx = f64::from(view.stage.0.x) + view.origin.0 + x * view.zoom;
    let sy = f64::from(view.stage.0.y) + view.origin.1 + y * view.zoom;
    frame.pixel(sx as u32, sy as u32).expect("inside the frame")
}

fn check_reference(backend: &str) {
    let (frame, view) = render(backend, traced(0.0, false), 30, |_| {});
    common::save(&frame, &format!("view-reference-{backend}"));
    assert!(view.zoom >= 12.0);
    // The art draws over a reference below it.
    assert_eq!(on_screen(&frame, &view, 20, 20), [0, 0, 255, 255]);
    // The ramp is where its place puts it: half way across, red is half
    // way up; a quarter down, green a quarter.
    let mid = at_doc(&frame, &view, 16.0, 8.0);
    assert!(mid[0].abs_diff(128) <= 4, "red half way: {mid:?}");
    assert!(mid[1].abs_diff(64) <= 4, "green a quarter: {mid:?}");
    assert_eq!(mid[3], 255);
    // Smooth at screen resolution: inside one document pixel the ramp
    // still climbs, where a reference on the document's grid would show one
    // color.
    let left = at_doc(&frame, &view, 8.1, 8.5);
    let right = at_doc(&frame, &view, 8.9, 8.5);
    assert!(
        right[0] >= left[0] + 4,
        "the ramp climbs inside a pixel: {left:?} to {right:?}"
    );

    // Turned a quarter clockwise and over the art: red now climbs down
    // the screen and the reference hides the blue pixel.
    let (frame, view) = render(backend, traced(90.0, true), 30, |_| {});
    common::save(&frame, &format!("view-reference-turned-{backend}"));
    let (top, bottom) = (
        at_doc(&frame, &view, 16.0, 4.0),
        at_doc(&frame, &view, 16.0, 28.0),
    );
    assert!(top[0] < 50 && bottom[0] > 200, "{top:?} {bottom:?}");
    assert_ne!(on_screen(&frame, &view, 20, 20), [0, 0, 255, 255]);
}

#[test]
fn a_reference_draws_smooth_where_it_is_placed_on_the_cpu_renderer() {
    check_reference("cpu");
}

#[test]
fn a_reference_draws_smooth_where_it_is_placed_on_the_gpu_renderer() {
    check_reference("gpu");
}

/// One-pixel black and white stripes, 2048 wide, shown at a tenth of a
/// screen pixel each: a smooth view shows even gray where sampling the
/// image at screen pixels would show bands of black and white.
fn check_shrunk(backend: &str) {
    let mut img = TiledImage::new(2048, 256, Rgba::TRANSPARENT);
    for y in 0..256 {
        for x in 0..2048 {
            let v = if x % 2 == 0 { 0 } else { 255 };
            img.set(x, y, Rgba::opaque(v, v, v));
        }
    }
    let mut doc = Document::new(32, 32, ColorMode::Rgba).unwrap();
    doc.add_layer(
        NewLayer::Reference {
            image: img,
            place: ReferencePlace {
                center: (16.0, 16.0),
                scale: (1.0 / 256.0, 1.0 / 256.0),
                rotation: 0.0,
            },
        },
        "stripes",
        Placement::default(),
    )
    .unwrap();
    let (frame, view) = render(backend, doc, 30, |_| {});
    common::save(&frame, &format!("view-reference-shrunk-{backend}"));
    assert!(view.zoom * (1.0 / 256.0) < 0.2, "shrunk past a fifth");
    // The stripes cover document x 12..20, y 15.5..16.5.
    for i in 0..40 {
        let x = 12.2 + f64::from(i) * 0.19;
        let c = at_doc(&frame, &view, x, 16.0);
        assert!(c[0].abs_diff(128) <= 24, "gray at {x}: {c:?}");
    }
}

#[test]
fn a_shrunk_reference_shows_its_average_on_the_cpu_renderer() {
    check_shrunk("cpu");
}

#[test]
fn a_shrunk_reference_shows_its_average_on_the_gpu_renderer() {
    check_shrunk("gpu");
}
