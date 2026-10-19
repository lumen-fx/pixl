//! How fast the editor moves between frames and plays an animation on a
//! 4096x4096 document of 32 layers, rendered with no window.
//!
//! Two documents: `sprite`, where each frame redraws a 512x512 figure on
//! four layers and links the other layers' cels to the first frame (how an
//! animation is usually built), and `full`, where every cel of every layer
//! is its own fully painted image (the worst case). For each it times a step
//! to the next frame until the renderer first shows it and until all of it
//! is drawn, with the onion skin off and on, and the frames per second
//! playback reaches at 12 frames per second and as fast as it can.
//!
//! `cargo bench -p pixl --bench playback -- cpu` or `-- gpu` picks the
//! renderer, `sprite` (the default) or `full` the document, and `side=N`,
//! `layers=N` and `frames=N` change it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lumenui::lumen_core::render_world::SurfaceCapture;
use lumenui::prelude::*;
use pixl::Editor;
use pixl::panels::timeline::Timeline;
use pixl::view::Layers;
use pixl_document::{BlendMode, ColorMode, Document, LayerId, NewLayer, Placement, Rect, Rgba};

/// Frame steps timed per document.
const STEPS: usize = 24;

/// How long each playback run lasts.
const PLAY_FOR: Duration = Duration::from_secs(3);

fn layer_ids(doc: &mut Document, layers: usize) -> Vec<LayerId> {
    (0..layers)
        .map(|i| {
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
            id
        })
        .collect()
}

/// Paints `rect` of a cel in 64x64 blocks of colors that vary with the
/// layer and the frame.
fn paint(doc: &mut Document, layer: LayerId, frame: usize, i: usize, rect: Rect) {
    let t = doc.target(layer, frame).unwrap();
    for ty in (rect.y..rect.bottom()).step_by(64) {
        for tx in (rect.x..rect.right()).step_by(64) {
            let c = Rgba::new(
                (tx / 16 + i as i32 * 7 + frame as i32 * 13) as u8,
                (ty / 16) as u8,
                (i * 8 + frame * 20) as u8,
                if (tx / 64 + ty / 64 + i as i32 + frame as i32) % 5 == 0 {
                    0
                } else {
                    255
                },
            );
            t.raster
                .fill_rect_raw(&Rect::new(tx, ty, 64, 64), c.to_u32());
        }
    }
}

/// Every layer covered in the first frame; later frames link those cels,
/// except four layers that redraw a 512x512 figure that moves.
fn sprite_document(side: u32, layers: usize, frames: usize) -> Document {
    let mut doc = Document::new(side, side, ColorMode::Rgba).unwrap();
    let ids = layer_ids(&mut doc, layers);
    for f in 1..frames {
        doc.insert_frame(f, 83).unwrap();
    }
    let whole = Rect::new(0, 0, side as i32, side as i32);
    let moving = layers.saturating_sub(4)..layers;
    for (i, id) in ids.iter().enumerate() {
        if moving.contains(&i) {
            for f in 0..frames {
                let x = (side as i32 - 512) * f as i32 / frames.max(2) as i32;
                paint(&mut doc, *id, f, i, Rect::new(x, side as i32 / 3, 512, 512));
            }
        } else {
            paint(&mut doc, *id, 0, i, whole);
            for f in 1..frames {
                doc.link_cel(*id, 0, f).unwrap();
            }
        }
    }
    doc
}

/// Every cel of every layer its own fully painted image.
fn full_document(side: u32, layers: usize, frames: usize) -> Document {
    let mut doc = Document::new(side, side, ColorMode::Rgba).unwrap();
    let ids = layer_ids(&mut doc, layers);
    for f in 1..frames {
        doc.insert_frame(f, 83).unwrap();
    }
    let whole = Rect::new(0, 0, side as i32, side as i32);
    for (i, id) in ids.iter().enumerate() {
        for f in 0..frames {
            paint(&mut doc, *id, f, i, whole);
        }
    }
    doc
}

fn app_dir(backend: &str) -> PathBuf {
    let app = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bench-play-{backend}"));
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
    /// Stepping to the next frame, with the onion skin or without it.
    Step {
        n: usize,
        onion: bool,
    },
    Play {
        run: usize,
    },
    Done,
}

/// The playback runs: frame durations and onion skin.
const RUNS: [(u32, bool, &str); 4] = [
    (83, false, "12 fps"),
    (1, false, "as fast as it goes"),
    (83, true, "12 fps, onion skin"),
    (1, true, "as fast as it goes, onion skin"),
];

#[derive(Default)]
struct Results {
    open: Option<Duration>,
    /// Per step: the first frame that shows it, and the frame that shows
    /// all of it.
    steps: Vec<(Duration, Duration)>,
    onion_steps: Vec<(Duration, Duration)>,
    plays: Vec<(&'static str, f64, f64)>,
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

fn percentile(v: &[Duration], p: f64) -> Duration {
    let mut v = v.to_vec();
    v.sort();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn run(doc: Document, backend: &str) {
    let capture = SurfaceCapture::default();
    let stop = Arc::new(AtomicBool::new(false));
    let watch = watch(capture.clone(), Arc::clone(&stop));
    let out: Arc<Mutex<Results>> = Arc::default();
    let cap = capture.clone();
    let world_capture = capture.clone();
    let opened = Instant::now();

    pixl::editor(Source::Disk {
        dir: app_dir(backend),
    })
    .insert_resource(Editor::new(doc))
    .add_systems(
        TickStage::Systems,
        (move |mut phase: Local<Option<Phase>>,
               mut since: Local<Option<(Instant, usize, usize)>>,
               mut stepped: Local<Option<Instant>>,
               mut shown_phase: Local<Option<Phase>>,
               wake: Option<Res<lumenui::lumen_core::tick::WakeDeadline>>,
               mut editor: ResMut<Editor>,
               mut timeline: ResMut<Timeline>,
               mut signals: Signals,
               layers: Res<Layers>| {
            // A window ticks every frame; the bench keeps the headless run
            // ticking the same way.
            if let Some(wake) = &wake {
                wake.request(Instant::now() + Duration::from_millis(2));
            }
            let p = phase.get_or_insert(Phase::Opening);
            // Progress on stderr, once per part of the script.
            let part = match *p {
                Phase::Step { onion, .. } => Phase::Step { n: 0, onion },
                Phase::Play { run } => Phase::Play { run },
                other => other,
            };
            if *shown_phase != Some(part) {
                eprintln!("{part:?} at {:?}", opened.elapsed());
                *shown_phase = Some(part);
            }
            let mut results = out.lock().unwrap();
            match *p {
                Phase::Opening => {
                    if !layers.rewrites.is_empty() && layers.pending() == 0 {
                        results.open = Some(opened.elapsed());
                        *p = Phase::Step { n: 0, onion: false };
                    }
                }
                Phase::Step { n, onion } => {
                    if watch.started.lock().unwrap().is_some() || layers.pending() > 0 {
                        return;
                    }
                    if timeline.onion.on != onion {
                        signals.set("onion_on", onion);
                        return;
                    }
                    if let (Some(lap), Some(t0)) = (watch.lap.lock().unwrap().take(), *stepped) {
                        let list = if onion {
                            &mut results.onion_steps
                        } else {
                            &mut results.steps
                        };
                        list.push((lap, t0.elapsed()));
                    }
                    if n == STEPS {
                        *p = if onion {
                            signals.set("onion_on", false);
                            Phase::Play { run: 0 }
                        } else {
                            Phase::Step { n: 0, onion: true }
                        };
                        *stepped = None;
                        return;
                    }
                    let next = (editor.frame + 1) % editor.doc.frame_count();
                    editor.go_to_frame(next);
                    *stepped = Some(Instant::now());
                    *watch.started.lock().unwrap() = Some(Instant::now());
                    cap.request();
                    *p = Phase::Step { n: n + 1, onion };
                }
                Phase::Play { run } => {
                    let Some((ms, onion, label)) = RUNS.get(run).copied() else {
                        *p = Phase::Done;
                        return;
                    };
                    match *since {
                        None => {
                            if layers.pending() > 0 || timeline.onion.on != onion {
                                if timeline.onion.on != onion {
                                    signals.set("onion_on", onion);
                                }
                                return;
                            }
                            for f in 0..editor.doc.frame_count() {
                                let _ = editor.doc.set_frame_duration(f, ms);
                            }
                            // The timeline's checkbox owns the setting.
                            signals.set("onion_on", onion);
                            timeline.playback.loop_tag = false;
                            timeline.playback.start(&mut editor, Instant::now());
                            *since = Some((Instant::now(), editor.frame, 0));
                        }
                        Some((t0, last, shown)) => {
                            if t0.elapsed() >= PLAY_FOR {
                                let fps = timeline.playback.fps().unwrap_or(0.0);
                                let average = shown as f64 / t0.elapsed().as_secs_f64();
                                results.plays.push((label, fps, average));
                                timeline.playback.stop();
                                signals.set("onion_on", false);
                                *since = None;
                                *p = Phase::Play { run: run + 1 };
                            } else if editor.frame != last {
                                *since = Some((t0, editor.frame, shown + 1));
                            }
                        }
                    }
                }
                Phase::Done => {
                    report(&results);
                    // The run has no window to close; the bench is done.
                    std::process::exit(0);
                }
            }
        })
        .before(pixl::panels::timeline::playback),
    )
    .add_plugin(BenchCapture(world_capture))
    .run_headless_rendered(HeadlessOptions {
        dpr: 1.0,
        ticks: None,
    })
    .expect("the editor runs");
    stop.store(true, Ordering::Relaxed);
}

fn report(r: &Results) {
    let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1000.0);
    if let Some(open) = r.open {
        println!("  open: {}", ms(open));
    }
    for (name, steps) in [
        ("next frame", &r.steps),
        ("next frame, onion skin", &r.onion_steps),
    ] {
        if steps.is_empty() {
            continue;
        }
        let first: Vec<Duration> = steps.iter().map(|s| s.0).collect();
        let all: Vec<Duration> = steps.iter().map(|s| s.1).collect();
        println!(
            "  {name}: first shown median {} / p95 {}, all of it median {} / p95 {} / max {} over {}",
            ms(percentile(&first, 0.5)),
            ms(percentile(&first, 0.95)),
            ms(percentile(&all, 0.5)),
            ms(percentile(&all, 0.95)),
            ms(*all.iter().max().unwrap()),
            steps.len()
        );
    }
    for (label, fps, average) in &r.plays {
        println!("  playback, {label}: {fps:.1} fps over the last second, {average:.1} on average");
    }
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
    let (side, layers, frames) = (
        arg("side", 4096) as u32,
        arg("layers", 32),
        arg("frames", 8),
    );
    let full = args.iter().any(|a| a == "full");
    let t = Instant::now();
    let (name, doc) = if full {
        ("full", full_document(side, layers, frames))
    } else {
        ("sprite", sprite_document(side, layers, frames))
    };
    eprintln!("built the {name} document in {:?}", t.elapsed());
    println!("renderer: {backend}, {side}x{side}, {layers} layers, {frames} frames, {name}");
    run(doc, &backend);
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
