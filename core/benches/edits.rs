//! Timings for the brushes, filters, gradients, palette work and transforms
//! on a 4096x4096 document of 32 layers, the editor's large-document case.
//! Run with `cargo bench -p pixl-core --bench edits`; it prints wall-clock
//! times. Every edit works on one cel, so the layers above and below only
//! matter to the view, which the app's own bench measures.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pixl_core::filters::{Filter, FilterInput, write_values};
use pixl_core::palettes::{Dither, index_image, palette_from_pixels};
use pixl_core::tools::brushes::{Rng, SmudgeStroke, SoftBrush, SoftInk, SoftStroke, spray};
use pixl_core::tools::fill::Connectivity;
use pixl_core::tools::gradient::{DitherPattern, Gradient, GradientShape, gradient};
use pixl_core::tools::paint::PencilStroke;
use pixl_core::{
    Affine, Bayer, Brush, ColorMode, Document, History, Ink, LayerId, NewLayer, Pixels, Placement,
    Point, Raster, Rect, Resample, Rgba, Selection, ToolContext, edit_cel,
};

const SIZE: u32 = 4096;
const TILE: i32 = 64;

fn time<R>(label: &str, f: impl FnOnce() -> R) -> (R, Duration) {
    let start = Instant::now();
    let out = f();
    let d = start.elapsed();
    println!("{label:<56} {:>10.2} ms", d.as_secs_f64() * 1e3);
    (out, d)
}

/// 32 layers, the top one full of varied color: the case the spec's
/// performance targets name.
fn document() -> (Document, LayerId) {
    let mut d = Document::new(SIZE, SIZE, ColorMode::Rgba).unwrap();
    let mut top = d.layers()[0].id;
    for _ in 1..32 {
        top = d
            .add_layer(NewLayer::Raster, "l", Placement::default())
            .unwrap();
    }
    let t = d.target(top, 0).unwrap();
    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            let c = Rgba::opaque((x / 16 * 7) as u8, (y / 16 * 5) as u8, ((x ^ y) / 32) as u8);
            t.raster.set_raw(x, y, c.to_u32());
        }
    }
    (d, top)
}

/// A stroke of `points` across the middle of the canvas.
fn path(points: usize) -> Vec<Point> {
    (0..points)
        .map(|i| {
            Point::new(
                1500 + i as i32 * 4,
                2000 + ((i as f64 / 8.0).sin() * 60.0) as i32,
            )
        })
        .collect()
}

/// A filter over the whole cel, tile by tile on every core, as the editor
/// runs it.
fn filter_all(raster: &Raster, filter: &Filter) -> Raster {
    let input = FilterInput {
        raster: raster.clone(),
        palette: Default::default(),
        transparent_index: 0,
        selection: None,
    };
    let prepared = Arc::new(filter.prepare());
    let tiles: Vec<Rect> = (0..SIZE as i32 / TILE)
        .flat_map(|ty| {
            (0..SIZE as i32 / TILE).map(move |tx| Rect::new(tx * TILE, ty * TILE, TILE, TILE))
        })
        .collect();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = tiles.len().div_ceil(threads);
    let results: Vec<(Rect, Vec<u32>)> = std::thread::scope(|s| {
        let jobs: Vec<_> = tiles
            .chunks(chunk)
            .map(|c| {
                let (prepared, input) = (&prepared, &input);
                s.spawn(move || {
                    c.iter()
                        .map(|r| (*r, prepared.render(input, *r)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
    });
    let mut out = raster.clone();
    for (r, v) in results {
        write_values(&mut out, r, &v);
    }
    out
}

fn main() {
    let (mut doc, layer) = time("build 4096x4096, 32 layers", document).0;
    let mut history = History::default();
    let ctx = ToolContext::default();
    let pts = path(200);
    let red = Rgba::opaque(255, 0, 0);

    println!("-- strokes of 200 points, one undo step each --");
    time("soft brush 32 px, hardness 50%", || {
        edit_cel(&mut doc, &mut history, "b", layer, 0, &ctx, |c| {
            let mut s = SoftStroke::new(SoftBrush {
                size: 32,
                opacity: 200,
                hardness: 128,
                dithered: false,
                ink: SoftInk::Paint(red),
            });
            for p in &pts {
                s.add(c, *p);
            }
        })
        .unwrap()
    });
    time("dithered brush 32 px", || {
        edit_cel(&mut doc, &mut history, "b", layer, 0, &ctx, |c| {
            let mut s = SoftStroke::new(SoftBrush {
                size: 32,
                opacity: 128,
                hardness: 255,
                dithered: true,
                ink: SoftInk::Paint(red),
            });
            for p in &pts {
                s.add(c, *p);
            }
        })
        .unwrap()
    });
    time("spray radius 16, 30 dots a dab", || {
        edit_cel(&mut doc, &mut history, "s", layer, 0, &ctx, |c| {
            let mut rng = Rng::new(7);
            for p in &pts {
                spray(c, *p, 16, 30, &Ink::Paint(red.into()), &mut rng);
            }
        })
        .unwrap()
    });
    time("smudge 8 px", || {
        edit_cel(&mut doc, &mut history, "s", layer, 0, &ctx, |c| {
            let mut s = SmudgeStroke::new(c, Brush::circle(8), 180, false);
            for p in &pts {
                s.add(c, *p);
            }
        })
        .unwrap()
    });
    let source = Arc::new(doc.cel_raster(layer, 0).unwrap().unwrap().clone());
    time("blur 8 px", || {
        edit_cel(&mut doc, &mut history, "b", layer, 0, &ctx, |c| {
            let ink = Ink::Blur {
                source: source.clone(),
                sharpen: false,
                amount: 255,
                keep_colors: false,
            };
            let mut s = PencilStroke::new(c, Brush::circle(8), ink, false);
            for p in &pts {
                s.add(c, *p);
            }
            s.finish(c);
        })
        .unwrap()
    });
    time("clone stamp 8 px", || {
        edit_cel(&mut doc, &mut history, "c", layer, 0, &ctx, |c| {
            let ink = Ink::Clone {
                source: source.clone(),
                offset: Point::new(-500, -500),
            };
            let mut s = PencilStroke::new(c, Brush::circle(8), ink, false);
            for p in &pts {
                s.add(c, *p);
            }
            s.finish(c);
        })
        .unwrap()
    });

    println!("-- whole-cel operations --");
    time("gradient, linear, blue noise, 4096x4096", || {
        let g = Gradient {
            shape: GradientShape::Linear,
            from: Point::new(0, 0),
            to: Point::new(SIZE as i32 - 1, 0),
            colors: vec![Rgba::BLACK.into(), Rgba::WHITE.into()],
            dither: DitherPattern::BlueNoise,
            repeat: false,
        };
        edit_cel(&mut doc, &mut history, "g", layer, 0, &ctx, |c| {
            gradient(c, &g)
        })
        .unwrap()
    });
    let raster = doc.cel_raster(layer, 0).unwrap().unwrap().clone();
    for (name, f) in [
        (
            "hue/saturation",
            Filter::HueSaturation {
                hue: 40.0,
                saturation: 10.0,
                lightness: 0.0,
            },
        ),
        (
            "curves",
            Filter::Curves {
                points: vec![(64, 40), (192, 220)],
            },
        ),
        (
            "outline outside, 8-connected",
            Filter::Outline {
                color: Rgba::BLACK,
                outside: true,
                connectivity: Connectivity::Eight,
            },
        ),
        (
            "noise",
            Filter::Noise {
                amount: 20,
                monochrome: false,
                seed: 3,
            },
        ),
        ("pixelate 8", Filter::Pixelate { size: 8 }),
    ] {
        time(&format!("filter {name}, tiles on every core"), || {
            filter_all(&raster, &f)
        });
    }
    time("filter hue/saturation, one tile (64x64)", || {
        let input = FilterInput {
            raster: raster.clone(),
            palette: Default::default(),
            transparent_index: 0,
            selection: None,
        };
        Filter::HueSaturation {
            hue: 40.0,
            saturation: 10.0,
            lightness: 0.0,
        }
        .prepare()
        .render(&input, Rect::new(0, 0, 64, 64))
    });
    let Raster::Rgba(img) = &raster else {
        unreachable!("an RGBA document");
    };
    let (palette, _) = time("palette of 32 from the image (median cut)", || {
        palette_from_pixels(img.to_pixels(), 32, true)
    });
    time("index to 32 colors, no dither", || {
        index_image(img, &palette, 0, Dither::None)
    });
    time("index to 32 colors, Bayer 4x4", || {
        index_image(img, &palette, 0, Dither::Ordered(Bayer::B4))
    });
    time("index to 32 colors, Floyd-Steinberg", || {
        index_image(img, &palette, 0, Dither::FloydSteinberg)
    });

    println!("-- free transform of a 512x512 selection --");
    let lifted = Pixels::from_raster(&raster, Rect::new(1000, 1000, 512, 512));
    let m = Affine::about((256.0, 256.0), (1.0, 1.0), (0.0, 0.0), 30.0, (0.0, 0.0));
    let out = m.bounds_of(Rect::new(0, 0, 512, 512));
    for r in [Resample::Nearest, Resample::CleanEdge, Resample::RotSprite] {
        time(&format!("turn 30 degrees, {r:?}"), || {
            lifted.affine(&m, out, 0, r)
        });
    }
    let whole = Pixels::from_raster(&raster, raster.bounds());
    let m = Affine::about((2048.0, 2048.0), (1.0, 1.0), (0.0, 0.0), 30.0, (0.0, 0.0));
    time("turn the whole 4096x4096 cel 30 degrees, RotSprite", || {
        whole.affine(
            &m,
            Rect::new(0, 0, SIZE as i32, SIZE as i32),
            0,
            Resample::RotSprite,
        )
    });

    println!("-- selections --");
    let mut sel = Selection::ellipse(SIZE, SIZE, Rect::new(500, 500, 3000, 3000));
    time("grow a 3000 px ellipse selection by 4", || sel.grow(4));
    time("border it by 2", || sel.border(2));
}
