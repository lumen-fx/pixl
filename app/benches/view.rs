//! How fast the canvas view answers on a 4096x4096 document of 32 layers,
//! rendered with no window: the progressive redraw after opening it and
//! after hiding a layer, stroke-to-screen latency on the top layer and on
//! the bottom layer (a buffer of its own, or inside the CPU-flattened buffer
//! when the layers do not fit the buffer budget), the frame after a pan and
//! after a zoom step, the same with a 4000x3000 photo as a reference layer
//! in the middle of the stack, and exporting the frame as a PNG and as a
//! packed, trimmed sprite sheet.
//!
//! `cargo bench -p pixl --bench view -- cpu` or `-- gpu` picks the renderer;
//! `side=N` and `layers=N` change the document from 4096 and 32, and
//! `budget=MiB` the layer buffer budget from the performance setting's
//! default. `log=FILE` writes every frame to a CSV file, as `pixl
//! --perf-log` does, and `overlay` shows the performance overlay while it
//! runs.
//!
//! `systems` prints what each of the editor's systems cost in each phase,
//! and `gate` checks the medians of the run against the limits for the
//! renderer in `benches/gate.json`, exiting with an error when one is
//! passed and naming the systems that cost the most in that phase.
//! `gate report` prints the same numbers and the same systems but does not
//! fail on a number past its limit, for machines the limits were not set
//! on, such as CI runners.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lumenui::glam::Vec2;
use lumenui::lumen_core::components::LumenTag;
use lumenui::lumen_core::input::{PointerButton, PointerData, PointerMoved, PointerPressed};
use lumenui::lumen_core::render_world::SurfaceCapture;
use lumenui::prelude::*;
use pixl::Editor;
use pixl::perf::SystemTimes;
use pixl::view::{Layers, View};
use pixl_document::{
    BlendMode, ColorMode, Document, NewLayer, Placement, Rect, ReferencePlace, Rgba, TiledImage,
};

const STROKES: usize = 40;

/// Ticks with nothing to do, timed after the document has opened.
const IDLE_TICKS: usize = 120;

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

/// A 4000x3000 stand-in for a photo: smooth gradients under fine noise, so
/// no two tiles are alike.
fn photo() -> TiledImage<Rgba> {
    let (w, h) = (4000u32, 3000u32);
    let mut seed = 0x9e37_79b9_u32;
    let mut px = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let n = (seed & 31) as u8;
            px.push(Rgba::opaque(
                ((x * 200 / w) as u8).wrapping_add(n),
                ((y * 200 / h) as u8).wrapping_add(n),
                (((x + y) * 100 / (w + h)) as u8).wrapping_add(n),
            ));
        }
    }
    TiledImage::from_pixels(w, h, Rgba::TRANSPARENT, &px)
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
    Idle {
        n: usize,
    },
    Stroke {
        layer_top: bool,
        n: usize,
    },
    Hiding,
    Pan {
        n: usize,
    },
    Zoom {
        n: usize,
    },
    /// A reference layer was added; waiting for its levels.
    Reference,
    RefPan {
        n: usize,
    },
    RefZoom {
        n: usize,
    },
    Export,
    Done,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Opening => "open",
            Phase::Idle { .. } => "idle",
            Phase::Stroke {
                layer_top: true, ..
            } => "stroke, top layer",
            Phase::Stroke { .. } => "stroke, bottom layer",
            Phase::Hiding => "hide a layer",
            Phase::Pan { .. } => "pan",
            Phase::Zoom { .. } => "zoom step",
            Phase::Reference => "add a reference",
            Phase::RefPan { .. } => "pan, with a reference",
            Phase::RefZoom { .. } => "zoom step, with a reference",
            Phase::Export | Phase::Done => "export",
        }
    }
}

/// Per phase, per system: total time and ticks.
type PhaseSystems = BTreeMap<&'static str, BTreeMap<&'static str, (Duration, u64)>>;

#[derive(Default)]
struct Results {
    /// Idle ticks: the editor's systems, and the whole tick.
    idle_systems: Vec<Duration>,
    idle_ticks: Vec<Duration>,
    systems: PhaseSystems,
    /// Ticks per phase.
    ticks: BTreeMap<&'static str, u64>,
    /// Elements in the document's tree.
    elements: usize,
    open: Option<(Duration, Duration, Duration, usize)>,
    hide: Option<(Duration, Duration, Duration, usize)>,
    top: Vec<Duration>,
    bottom: Vec<Duration>,
    pan: Vec<Duration>,
    zoom: Vec<Duration>,
    /// Adding the reference: until its buffer is in, and the longest tick
    /// meanwhile.
    reference: Option<(Duration, Duration)>,
    reference_started: Option<Instant>,
    ref_pan: Vec<Duration>,
    ref_zoom: Vec<Duration>,
    export: Option<(Duration, usize)>,
    sheet: Option<(Duration, usize)>,
    hide_started: Option<Instant>,
    hide_frame: Option<Duration>,
    /// The buffers the renderer held, and whether the bottom layers were
    /// flattened into one.
    buffers: Option<(usize, bool)>,
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
    let mut settings = pixl::Settings::default();
    settings.performance.buffer_budget_mib =
        arg("budget", pixl::view::DEFAULT_BUFFER_BUDGET_MIB as usize) as u64;
    let budget = settings.performance.buffer_budget_mib;
    settings.performance.show_fps = args.iter().any(|a| a == "overlay");
    let log = args.iter().find_map(|a| a.strip_prefix("log=")).map(|p| {
        pixl::perf::Perf::logging(
            pixl::perf::PerfLog::create(Path::new(p)).expect("the log file opens"),
        )
    });
    let t = Instant::now();
    let doc = document(side, layers);
    eprintln!("built the document in {:?}", t.elapsed());
    let bottom = doc.layers()[0].id;
    let top = doc.layers()[layers - 1].id;
    let middle = doc.layers()[layers / 2].id;
    let t = Instant::now();
    let mut reference = Some(photo());
    let reference_index = layers / 2;
    eprintln!("built the reference in {:?}", t.elapsed());

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
    .insert_resource(settings)
    .insert_resource(log.unwrap_or_default())
    .add_systems(
        TickStage::Systems,
        (move |mut phase: Local<Option<Phase>>,
               mut last: Local<Option<(Phase, Instant)>>,
               mut editor: ResMut<Editor>,
               mut view: ResMut<View>,
               layers: Res<Layers>,
               times: Res<SystemTimes>,
               elements: Query<(), With<LumenTag>>,
               mut down: MessageWriter<PointerPressed>,
               mut moved: MessageWriter<PointerMoved>| {
            let p = phase.get_or_insert(Phase::Opening);
            let mut results = out.lock().unwrap();
            // The last tick's systems count toward the phase it ran in.
            let now = Instant::now();
            let last_tick = last.map_or(now, |(_, at)| at);
            if let Some((was, at)) = *last {
                let bucket = results.systems.entry(was.name()).or_default();
                for (name, t) in &times.last {
                    let e = bucket.entry(name).or_default();
                    e.0 += *t;
                    e.1 += 1;
                }
                *results.ticks.entry(was.name()).or_default() += 1;
                if matches!(was, Phase::Idle { n } if n >= 10) {
                    results.idle_systems.push(times.last_total());
                    results.idle_ticks.push(now - at);
                }
            }
            *last = Some((*p, now));
            let stage_mid = view.stage.0 + view.stage.1 / 2.0;
            match *p {
                Phase::Opening => {
                    if view.stage.1.x > 0.0 && !layers.rewrites.is_empty() && layers.pending() == 0
                    {
                        results.open = timeline(&layers);
                        let flat = layers
                            .current_plan()
                            .iter()
                            .any(|i| matches!(i.source, pixl::view::ItemSource::Flat(_)));
                        results.buffers = Some((layers.buffer_stats().0, flat));
                        results.elements = elements.iter().count();
                        *p = Phase::Idle { n: 0 };
                    }
                }
                Phase::Idle { n } => {
                    // The first few ticks still settle what opening left.
                    *p = if n == IDLE_TICKS {
                        Phase::Stroke {
                            layer_top: true,
                            n: 0,
                        }
                    } else {
                        Phase::Idle { n: n + 1 }
                    };
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
                            results.hide_started = Some(Instant::now());
                            *watch.started.lock().unwrap() = Some(Instant::now());
                            cap.request();
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
                    if watch.started.lock().unwrap().is_some() || layers.pending() > 0 {
                        return;
                    }
                    results.hide_frame = watch.lap.lock().unwrap().take();
                    // A layer of its own hides by drawing one buffer fewer;
                    // one inside the flattened buffer redraws it.
                    let since = results.hide_started.expect("set when hiding");
                    results.hide = layers
                        .rewrites
                        .iter()
                        .any(|r| r.started >= since)
                        .then(|| timeline(&layers))
                        .flatten();
                    *p = Phase::Pan { n: 0 };
                }
                Phase::Pan { n } => {
                    if watch.started.lock().unwrap().is_some() {
                        return;
                    }
                    if let Some(lap) = watch.lap.lock().unwrap().take() {
                        results.pan.push(lap);
                    }
                    if n == STROKES {
                        *p = Phase::Zoom { n: 0 };
                        return;
                    }
                    view.pan(7.0, 3.0);
                    *watch.started.lock().unwrap() = Some(Instant::now());
                    cap.request();
                    *p = Phase::Pan { n: n + 1 };
                }
                Phase::Zoom { n } => {
                    if watch.started.lock().unwrap().is_some() {
                        return;
                    }
                    if let Some(lap) = watch.lap.lock().unwrap().take() {
                        results.zoom.push(lap);
                    }
                    if n == STROKES {
                        // A photo over the middle of the stack, turned a
                        // little and half transparent, covering the canvas.
                        let image = reference.take().expect("added once");
                        let k = f64::from(side) / 4000.0;
                        let _ = editor.doc.add_layer(
                            NewLayer::Reference {
                                image,
                                place: ReferencePlace {
                                    center: (f64::from(side) / 2.0, f64::from(side) / 2.0),
                                    scale: (k, k),
                                    rotation: 7.0,
                                },
                            },
                            "photo",
                            Placement {
                                parent: None,
                                index: Some(reference_index),
                            },
                        );
                        editor.touch();
                        results.reference_started = Some(Instant::now());
                        *p = Phase::Reference;
                        return;
                    }
                    // In two steps and out two, around the middle.
                    let inward = n % 4 < 2;
                    let z = view.step(inward);
                    let mid = view.middle();
                    view.zoom_at(z, mid);
                    *watch.started.lock().unwrap() = Some(Instant::now());
                    cap.request();
                    *p = Phase::Zoom { n: n + 1 };
                }
                Phase::Reference => {
                    let since = results.reference_started.expect("set when added");
                    if let Some((_, longest)) = &mut results.reference {
                        *longest = (*longest).max(now - last_tick);
                    } else {
                        results.reference = Some((Duration::ZERO, Duration::ZERO));
                    }
                    let (shown, _) = layers.reference_images.buffer_stats();
                    if shown > 0 && layers.pending() == 0 {
                        if let Some((until, _)) = &mut results.reference {
                            *until = since.elapsed();
                        }
                        *p = Phase::RefPan { n: 0 };
                    }
                }
                Phase::RefPan { n } => {
                    if watch.started.lock().unwrap().is_some() {
                        return;
                    }
                    if let Some(lap) = watch.lap.lock().unwrap().take() {
                        results.ref_pan.push(lap);
                    }
                    if n == STROKES {
                        *p = Phase::RefZoom { n: 0 };
                        return;
                    }
                    view.pan(-7.0, 3.0);
                    *watch.started.lock().unwrap() = Some(Instant::now());
                    cap.request();
                    *p = Phase::RefPan { n: n + 1 };
                }
                Phase::RefZoom { n } => {
                    if watch.started.lock().unwrap().is_some() {
                        return;
                    }
                    if let Some(lap) = watch.lap.lock().unwrap().take() {
                        results.ref_zoom.push(lap);
                    }
                    if n == STROKES {
                        *p = Phase::Export;
                        return;
                    }
                    let inward = n % 4 < 2;
                    let z = view.step(inward);
                    let mid = view.middle();
                    view.zoom_at(z, mid);
                    *watch.started.lock().unwrap() = Some(Instant::now());
                    cap.request();
                    *p = Phase::RefZoom { n: n + 1 };
                }
                Phase::Export => {
                    let t = Instant::now();
                    let png = pixl_io::png::export_frame_png(&editor.doc, 0, 1)
                        .expect("the frame exports");
                    results.export = Some((t.elapsed(), png.len()));
                    let t = Instant::now();
                    let sheet = pixl_io::sheet::sprite_sheet(
                        &editor.doc,
                        &pixl_io::sheet::SheetOptions {
                            layout: pixl_io::sheet::SheetLayout::Packed,
                            trim: true,
                            ..Default::default()
                        },
                    )
                    .expect("the sheet lays out");
                    let png = sheet.to_png().expect("the sheet encodes");
                    results.sheet = Some((t.elapsed(), png.len()));
                    *p = Phase::Done;
                }
                Phase::Done => {}
            }
        })
        .before(pixl::settings::keymap::read_keys)
        .before(pixl::view::input::navigate),
    )
    .add_plugin(BenchCapture(world_capture))
    .run_headless_rendered(HeadlessOptions {
        dpr: 1.0,
        ticks: Some(5000),
    })
    .expect("the editor runs");
    stop.store(true, Ordering::Relaxed);

    let r = results.lock().unwrap();
    let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1000.0);
    let fine = |d: Duration| format!("{:.2} ms", d.as_secs_f64() * 1000.0);
    println!("renderer: {backend}, {side}x{side}, {layers} layers, buffer budget {budget} MiB");
    if let Some((buffers, flat)) = r.buffers {
        println!(
            "buffers: {buffers}{}",
            if flat {
                ", the bottom layers flattened into one"
            } else {
                ", one per layer"
            }
        );
    }
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
        ("zoom step", &r.zoom),
        ("pan, with a reference", &r.ref_pan),
        ("zoom step, with a reference", &r.ref_zoom),
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
    if let Some((until, longest)) = r.reference {
        println!(
            "add a 4000x3000 reference: shows after {}, longest tick meanwhile {}",
            ms(until),
            ms(longest)
        );
    }
    if let Some(frame) = r.hide_frame {
        println!("hide a layer: first frame {}", ms(frame));
    }
    match r.hide {
        Some((first, half, last, tiles)) => println!(
            "hide a layer, redraw: {tiles} tiles, first {} / half {} / all {}",
            ms(first),
            ms(half),
            ms(last)
        ),
        None => println!("hide a layer, redraw: none, the layer has a buffer of its own"),
    }
    if let Some((t, bytes)) = r.export {
        println!("export PNG: {} ({} KiB)", ms(t), bytes / 1024);
    }
    if !r.idle_ticks.is_empty() {
        println!(
            "idle tick: editor systems median {} / p95 {}, whole tick median {} / p95 {} over {}",
            fine(percentile(&r.idle_systems, 0.5)),
            fine(percentile(&r.idle_systems, 0.95)),
            fine(percentile(&r.idle_ticks, 0.5)),
            fine(percentile(&r.idle_ticks, 0.95)),
            r.idle_ticks.len()
        );
    }
    println!("elements in the tree: {}", r.elements);
    if args.iter().any(|a| a == "systems") {
        for (phase, systems) in &r.systems {
            let ticks = r.ticks.get(phase).copied().unwrap_or(1).max(1);
            let total: Duration = systems.values().map(|(t, _)| *t).sum();
            println!(
                "{phase}: the editor's systems {} a tick over {ticks} ticks",
                fine(total / ticks as u32)
            );
            for (name, t) in heaviest(systems, 8) {
                println!("  {name}: {} a tick", fine(t / ticks as u32));
            }
        }
    }
    if let Some((t, bytes)) = r.sheet {
        println!(
            "export sprite sheet, packed and trimmed: {} ({} KiB)",
            ms(t),
            bytes / 1024
        );
    }
    if args.iter().any(|a| a == "gate") {
        let enforce = !args.iter().any(|a| a == "report");
        let passed = gate(&r, &backend, side, layers, enforce);
        drop(r);
        if !passed {
            std::process::exit(1);
        }
    }
}

/// The phase each gated number is measured in, for naming its systems.
const GATED: [(&str, &str); 7] = [
    ("idle_systems_ms", "idle"),
    ("idle_tick_ms", "idle"),
    ("stroke_top_ms", "stroke, top layer"),
    ("stroke_bottom_ms", "stroke, bottom layer"),
    ("pan_ms", "pan"),
    ("pan_reference_ms", "pan, with a reference"),
    ("zoom_reference_ms", "zoom step, with a reference"),
];

/// Checks the medians of the run against `benches/gate.json` for the
/// renderer, and prints each. A number past its limit names the editor's
/// systems that cost the most in its phase, and fails the gate when
/// `enforce` is set. The limits hold for the 4096x4096 document of 32
/// layers only.
fn gate(r: &Results, backend: &str, side: u32, layers: usize, enforce: bool) -> bool {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/gate.json");
    let text = std::fs::read_to_string(&path).expect("benches/gate.json reads");
    let limits: serde_json::Value = serde_json::from_str(&text).expect("gate.json is JSON");
    let Some(limits) = limits.get(backend) else {
        println!("gate: no limits for the {backend} renderer");
        return false;
    };
    if (side, layers) != (4096, 32) {
        println!("gate: the limits are for 4096x4096 and 32 layers");
        return false;
    }
    let median = |v: &[Duration]| (!v.is_empty()).then(|| percentile(v, 0.5));
    let measured = |key: &str| match key {
        "idle_systems_ms" => median(&r.idle_systems),
        "idle_tick_ms" => median(&r.idle_ticks),
        "stroke_top_ms" => median(&r.top),
        "stroke_bottom_ms" => median(&r.bottom),
        "pan_ms" => median(&r.pan),
        "pan_reference_ms" => median(&r.ref_pan),
        "zoom_reference_ms" => median(&r.ref_zoom),
        _ => None,
    };
    let mut passed = true;
    for (key, phase) in GATED {
        let Some(limit) = limits.get(key).and_then(serde_json::Value::as_f64) else {
            continue;
        };
        let Some(value) = measured(key).map(|d| d.as_secs_f64() * 1000.0) else {
            println!("gate {key}: not measured");
            passed = false;
            continue;
        };
        let ok = value <= limit;
        println!(
            "gate {key}: {value:.2} ms, limit {limit:.2} ms, {}",
            if ok { "ok" } else { "OVER" }
        );
        if !ok {
            if enforce {
                passed = false;
            }
            if let Some(systems) = r.systems.get(phase) {
                let ticks = r.ticks.get(phase).copied().unwrap_or(1).max(1);
                for (name, t) in heaviest(systems, 5) {
                    println!(
                        "  {name}: {:.2} ms a tick",
                        t.as_secs_f64() * 1000.0 / ticks as f64
                    );
                }
            }
        }
    }
    if !enforce {
        println!("gate: report only, a number past its limit does not fail the run");
    }
    passed
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

/// The `n` systems that cost the most in a phase, with their total time.
fn heaviest(
    systems: &BTreeMap<&'static str, (Duration, u64)>,
    n: usize,
) -> Vec<(&'static str, Duration)> {
    let mut v: Vec<(&'static str, Duration)> =
        systems.iter().map(|(name, (t, _))| (*name, *t)).collect();
    v.sort_by_key(|s| std::cmp::Reverse(s.1));
    v.truncate(n);
    v
}
