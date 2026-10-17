//! How fast the canvas view answers on a 4096x4096 document of 32 layers,
//! rendered with no window: the progressive redraw after opening it and
//! after hiding a layer, stroke-to-screen latency on the top layer (a buffer
//! of its own) and on the bottom layer (inside the CPU-flattened buffer), and
//! the frame after a pan.
//!
//! `cargo bench -p pixl --bench view -- cpu` or `-- gpu` picks the renderer;
//! `side=N` and `layers=N` change the document from 4096 and 32.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{PointerButton, PointerData, PointerMoved, PointerPressed};
use lumenui::lumen_core::render_world::SurfaceCapture;
use lumenui::prelude::*;
use pixl::Editor;
use pixl::view::{Layers, View};
use pixl_core::{BlendMode, ColorMode, Document, NewLayer, Placement, Rect, Rgba};

const STROKES: usize = 40;

/// Every layer covered: one flat color per 64x64 tile, four blend modes.
fn document(side: u32, layers: usize) -> Document {
    let mut doc = Document::new(side, side, ColorMode::Rgba).unwrap();
    for i in 0..layers {
        let id = if i == 0 {
            doc.layers()[0].id
        } else {
            doc.add_layer(
                NewLayer::Raster,
                format!("Layer {}", i + 1),
                Placement::default(),
            )
            .unwrap()
        };
        if i > 0 {
            let mode = [
                BlendMode::Normal,
                BlendMode::Multiply,
                BlendMode::Screen,
                BlendMode::Overlay,
            ][i % 4];
            doc.update_layer(id, |p| {
                p.blend = mode;
                p.opacity = 200;
            })
            .unwrap();
        }
        let t = doc.target(id, 0).unwrap();
        for ty in 0..(side / 64) as i32 {
            for tx in 0..(side / 64) as i32 {
                let c = Rgba::new(
                    (tx * 4 + i as i32 * 7) as u8,
                    (ty * 4) as u8,
                    (i * 8) as u8,
                    if (tx + ty + i as i32) % 5 == 0 {
                        0
                    } else {
                        255
                    },
                );
                t.raster
                    .fill_rect_raw(&Rect::new(tx * 64, ty * 64, 64, 64), c.to_u32());
            }
        }
    }
    doc
}

fn app_dir(backend: &str) -> PathBuf {
    let app = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bench-{backend}"));
    std::fs::create_dir_all(&dir).unwrap();
    for part in ["src", "assets"] {
        let link = dir.join(part);
        if !link.exists() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(app.join(part), &link).unwrap();
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(app.join(part), &link).unwrap();
        }
    }
    let toml = std::fs::read_to_string(app.join("lumen.toml")).unwrap();
    std::fs::write(
        dir.join("lumen.toml"),
        format!("{toml}\n[render]\nbackend = \"{backend}\"\n\n[mcp]\nport = 0\n"),
    )
    .unwrap();
    dir
}

/// What the script is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Opening,
    Stroke { layer_top: bool, n: usize },
    Hiding,
    Pan { n: usize },
    Done,
}

#[derive(Default)]
struct Results {
    open: Option<(Duration, Duration, Duration, usize)>,
    hide: Option<(Duration, Duration, Duration, usize)>,
    top: Vec<Duration>,
    bottom: Vec<Duration>,
    pan: Vec<Duration>,
}

/// The time from a request to the renderer's answer, timed by a thread
/// that watches the capture flag.
struct Stopwatch {
    started: Arc<Mutex<Option<Instant>>>,
    lap: Arc<Mutex<Option<Duration>>>,
}

fn watch(capture: SurfaceCapture, stop: Arc<AtomicBool>) -> Stopwatch {
    let started: Arc<Mutex<Option<Instant>>> = Arc::default();
    let lap: Arc<Mutex<Option<Duration>>> = Arc::default();
    let (s, l) = (Arc::clone(&started), Arc::clone(&lap));
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let begun = *s.lock().unwrap();
            if let Some(t0) = begun
                && !capture.is_requested()
            {
                *l.lock().unwrap() = Some(t0.elapsed());
                *s.lock().unwrap() = None;
            }
            std::thread::sleep(Duration::from_micros(50));
        }
    });
    Stopwatch { started, lap }
}

fn timeline(layers: &Layers) -> Option<(Duration, Duration, Duration, usize)> {
    let r = layers.rewrites.iter().max_by_key(|r| r.tiles)?;
    let mut a = r.arrivals.clone();
    a.sort();
    Some((*a.first()?, a[a.len() / 2], *a.last()?, r.tiles))
}

fn percentile(v: &[Duration], p: f64) -> Duration {
    let mut v = v.to_vec();
    v.sort();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let backend = args
        .iter()
        .find(|a| *a == "cpu" || *a == "gpu")
        .cloned()
        .unwrap_or_else(|| "cpu".into());
    let arg = |name: &str, default: usize| {
        args.iter()
            .find_map(|a| a.strip_prefix(name)?.strip_prefix('=')?.parse().ok())
            .unwrap_or(default)
    };
    let (side, layers) = (arg("side", 4096) as u32, arg("layers", 32));
    let t = Instant::now();
    let doc = document(side, layers);
    eprintln!("built the document in {:?}", t.elapsed());
    let bottom = doc.layers()[0].id;
    let top = doc.layers()[layers - 1].id;
    let middle = doc.layers()[layers / 2].id;

    let capture = SurfaceCapture::default();
    let stop = Arc::new(AtomicBool::new(false));
    let watch = watch(capture.clone(), Arc::clone(&stop));
    let results: Arc<Mutex<Results>> = Arc::default();
    let out = Arc::clone(&results);
    let cap = capture.clone();
    let world_capture = capture.clone();

    let mut editor = Editor::new(doc);
    editor.layer = top;
    pixl::editor(Source::Disk {
        dir: app_dir(&backend),
    })
    .insert_resource(editor)
    .add_systems(
        TickStage::Systems,
        (move |mut phase: Local<Option<Phase>>,
               mut editor: ResMut<Editor>,
               mut view: ResMut<View>,
               layers: Res<Layers>,
               mut down: MessageWriter<PointerPressed>,
               mut moved: MessageWriter<PointerMoved>| {
            let p = phase.get_or_insert(Phase::Opening);
            let mut results = out.lock().unwrap();
            let stage_mid = view.stage.0 + view.stage.1 / 2.0;
            match *p {
                Phase::Opening => {
                    if view.stage.1.x > 0.0 && !layers.rewrites.is_empty() && layers.pending() == 0
                    {
                        results.open = timeline(&layers);
                        *p = Phase::Stroke {
                            layer_top: true,
                            n: 0,
                        };
                    }
                }
                Phase::Stroke { layer_top, n } => {
                    if watch.started.lock().unwrap().is_some() {
                        return;
                    }
                    if let Some(lap) = watch.lap.lock().unwrap().take() {
                        let list = if layer_top {
                            &mut results.top
                        } else {
                            &mut results.bottom
                        };
                        list.push(lap);
                    }
                    if n == STROKES {
                        *p = if layer_top {
                            editor.layer = bottom;
                            Phase::Stroke {
                                layer_top: false,
                                n: 0,
                            }
                        } else {
                            let _ = editor.doc.update_layer(middle, |l| l.visible = false);
                            editor.touch();
                            Phase::Hiding
                        };
                        return;
                    }
                    let y = if layer_top { -100.0 } else { 100.0 };
                    let at = stage_mid + Vec2::new(-200.0 + n as f32 * 10.0, y);
                    if n == 0 {
                        down.write(PointerPressed {
                            position: at,
                            button: PointerButton::Primary,
                            local: None,
                            pointer: PointerData::mouse(1),
                        });
                    } else {
                        moved.write(PointerMoved {
                            position: at,
                            local: None,
                            pointer: PointerData::mouse(1),
                        });
                    }
                    *watch.started.lock().unwrap() = Some(Instant::now());
                    cap.request();
                    *p = Phase::Stroke {
                        layer_top,
                        n: n + 1,
                    };
                }
                Phase::Hiding => {
                    if layers.pending() == 0 && layers.rewrites.iter().any(|r| r.done == r.tiles) {
                        results.hide = timeline(&layers);
                        *p = Phase::Pan { n: 0 };
                    }
                }
                Phase::Pan { n } => {
                    if watch.started.lock().unwrap().is_some() {
                        return;
                    }
                    if let Some(lap) = watch.lap.lock().unwrap().take() {
                        results.pan.push(lap);
                    }
                    if n == STROKES {
                        *p = Phase::Done;
                        return;
                    }
                    view.pan(7.0, 3.0);
                    *watch.started.lock().unwrap() = Some(Instant::now());
                    cap.request();
                    *p = Phase::Pan { n: n + 1 };
                }
                Phase::Done => {}
            }
        })
        .before(pixl::keymap::read_keys)
        .before(pixl::input::navigate),
    )
    .add_plugin(BenchCapture(world_capture))
    .run_headless_rendered(HeadlessOptions {
        dpr: 1.0,
        ticks: Some(3000),
    })
    .expect("the editor runs");
    stop.store(true, Ordering::Relaxed);

    let r = results.lock().unwrap();
    let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1000.0);
    println!("renderer: {backend}, {side}x{side}, {layers} layers");
    if let Some((first, half, last, tiles)) = r.open {
        println!(
            "open: {tiles} tiles, first {} / half {} / all {}",
            ms(first),
            ms(half),
            ms(last)
        );
    }
    for (name, v) in [
        ("stroke, top layer", &r.top),
        ("stroke, bottom layer", &r.bottom),
        ("pan", &r.pan),
    ] {
        if !v.is_empty() {
            println!(
                "{name}: median {} / p95 {} / max {} over {}",
                ms(percentile(v, 0.5)),
                ms(percentile(v, 0.95)),
                ms(*v.iter().max().unwrap()),
                v.len()
            );
        }
    }
    if let Some((first, half, last, tiles)) = r.hide {
        println!(
            "hide a layer: {tiles} tiles, first {} / half {} / all {}",
            ms(first),
            ms(half),
            ms(last)
        );
    }
}

/// Puts the bench's capture handle in both worlds, where the renderer
/// answers it.
struct BenchCapture(SurfaceCapture);

impl Plugin for BenchCapture {
    fn build(self, app: &mut EcsApp) {
        app.world.insert_resource(self.0.clone());
        app.render_world.insert_resource(self.0);
    }
}
