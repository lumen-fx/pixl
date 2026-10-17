//! How the editor answers while an edit too large for one frame runs, on a
//! 4096x4096 document of 32 layers rendered with no window: a gradient over
//! the whole top layer, a free transform turning the whole top cel with
//! RotSprite, a palette made from the image, and the document put on its
//! palette with Floyd-Steinberg. For each, the time until it lands as an
//! undo step, and the longest tick while it runs, which is how long the
//! editor stops answering.
//!
//! `cargo bench -p pixl --bench heavy -- gpu` (or `cpu`); `side=N` and
//! `layers=N` change the document.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lumenui::glam::Vec2;
use lumenui::lumen_core::input::{
    Key, KeyPressed, MenuClicked, Modifiers, PointerButton, PointerData, PointerMoved,
    PointerPressed, PointerReleased,
};
use lumenui::prelude::*;
use pixl::view::{Layers, View};
use pixl::{Editor, Tool};
use pixl_core::{BlendMode, ColorMode, Document, NewLayer, Placement, Rect, Rgba};

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
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bench-heavy-{backend}"));
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

/// The edits, in the order they run, with the undo label each lands as.
const EDITS: [(&str, &str); 4] = [
    ("gradient over the top layer", "Gradient"),
    ("free transform, whole cel, RotSprite", "Free transform"),
    ("palette of 32 from the image", "Palette from image"),
    ("indexed color, Floyd-Steinberg", "Indexed color"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Opening,
    /// Starting edit `i`, a step at a time.
    Start {
        i: usize,
        step: usize,
    },
    /// Edit `i` runs: when it started and the longest tick so far.
    Running {
        i: usize,
        since: Instant,
        longest: Duration,
    },
    Done,
}

/// The systems that cost the most in a tick, with their time.
type Heaviest = Vec<(&'static str, Duration)>;

/// Per edit: time to land, longest tick, ticks, the main world's part of
/// the longest tick (the rest is the render), and the editor's systems that
/// cost the most in it.
type Outcome = (Duration, Duration, usize, Duration, Heaviest);

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
    let doc = document(side, layers);
    let top = doc.layers()[layers - 1].id;
    let results: Arc<Mutex<Vec<Option<Outcome>>>> = Arc::new(Mutex::new(vec![None; EDITS.len()]));
    let out = Arc::clone(&results);

    let mut editor = Editor::new(doc);
    editor.layer = top;
    pixl::editor(Source::Disk {
        dir: app_dir(&backend),
    })
    .insert_resource(editor)
    .add_systems(
        TickStage::Systems,
        (move |mut phase: Local<Option<Phase>>,
               mut last: Local<Option<Instant>>,
               mut ticks: Local<(usize, Duration, Heaviest)>,
               (times, perf): (Res<pixl::perf::SystemTimes>, Res<pixl::perf::Perf>),
               mut editor: ResMut<Editor>,
               view: Res<View>,
               layers: Res<Layers>,
               mut signals: Signals,
               mut keys: MessageWriter<KeyPressed>,
               mut menu: MessageWriter<MenuClicked>,
               (mut down, mut moved, mut up): (
            MessageWriter<PointerPressed>,
            MessageWriter<PointerMoved>,
            MessageWriter<PointerReleased>,
        )| {
            let now = Instant::now();
            let tick = last.map_or(Duration::ZERO, |l| now - l);
            *last = Some(now);
            let p = phase.get_or_insert(Phase::Opening);
            match *p {
                Phase::Opening => {
                    if view.stage.1.x > 0.0 && !layers.rewrites.is_empty() && layers.pending() == 0
                    {
                        *p = Phase::Start { i: 0, step: 0 };
                    }
                }
                Phase::Start { i, step } => {
                    let mid = view.stage.0 + view.stage.1 / 2.0;
                    let mut started = false;
                    match (i, step) {
                        (0, 0) => editor.tool = Tool::Gradient,
                        (0, 1) => {
                            down.write(PointerPressed {
                                position: mid - Vec2::new(200.0, 0.0),
                                button: PointerButton::Primary,
                                local: None,
                                pointer: PointerData::mouse(1),
                            });
                        }
                        (0, 2) => {
                            moved.write(PointerMoved {
                                position: mid + Vec2::new(200.0, 50.0),
                                local: None,
                                pointer: PointerData::mouse(1),
                            });
                        }
                        (0, 3) => {
                            up.write(PointerReleased {
                                position: mid + Vec2::new(200.0, 50.0),
                                button: PointerButton::Primary,
                                local: None,
                                pointer: PointerData::mouse(0),
                            });
                            started = true;
                        }
                        (1, 0) => {
                            keys.write(KeyPressed {
                                key: Key::Character("t".into()),
                                modifiers: Modifiers {
                                    ctrl: true,
                                    ..Modifiers::default()
                                },
                                repeat: false,
                            });
                        }
                        (1, 1) => {}
                        (1, 2) => signals.set("opt_angle", "30"),
                        (1, 3) => {
                            keys.write(KeyPressed {
                                key: Key::Named(lumenui::lumen_core::input::NamedKey::Enter),
                                modifiers: Modifiers::default(),
                                repeat: false,
                            });
                            started = true;
                        }
                        (2, 0) => {
                            signals.set("quantize_open", true);
                            signals.set("quantize_colors", "32");
                        }
                        (2, 1) => {
                            menu.write(MenuClicked {
                                id: "quantize-ok".into(),
                            });
                            started = true;
                        }
                        (3, 0) => {
                            signals.set("indexed_open", true);
                            signals.set("indexed_palette", "Current palette");
                            signals.set("indexed_dither", "Floyd-Steinberg");
                        }
                        (3, 1) => {
                            menu.write(MenuClicked {
                                id: "indexed-ok".into(),
                            });
                            started = true;
                        }
                        _ => {}
                    }
                    *p = if started {
                        *ticks = (0, Duration::ZERO, Vec::new());
                        Phase::Running {
                            i,
                            since: now,
                            longest: Duration::ZERO,
                        }
                    } else {
                        Phase::Start { i, step: step + 1 }
                    };
                }
                Phase::Running { i, since, longest } => {
                    ticks.0 += 1;
                    if tick > longest {
                        let mut heavy = times.last.clone();
                        heavy.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
                        heavy.truncate(4);
                        ticks.2 = heavy;
                        ticks.1 = perf.frames.back().map_or(Duration::ZERO, |f| f.work);
                    }
                    let longest = longest.max(tick);
                    let landed = editor.history.undo_labels().any(|l| l == EDITS[i].1);
                    if landed {
                        out.lock().unwrap()[i] =
                            Some((now - since, longest, ticks.0, ticks.1, ticks.2.clone()));
                        *p = if i + 1 < EDITS.len() {
                            Phase::Start { i: i + 1, step: 0 }
                        } else {
                            Phase::Done
                        };
                    } else {
                        *p = Phase::Running { i, since, longest };
                    }
                }
                Phase::Done => {}
            }
        })
        .before(pixl::keymap::read_keys)
        .before(pixl::input::navigate),
    )
    .run_headless_rendered(HeadlessOptions {
        dpr: 1.0,
        ticks: Some(3000),
    })
    .expect("the editor runs");

    let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1000.0);
    println!("renderer: {backend}, {side}x{side}, {layers} layers");
    for ((name, _), r) in EDITS.iter().zip(results.lock().unwrap().iter()) {
        match r {
            Some((total, longest, ticks, work, heavy)) => {
                println!(
                    "{name}: lands in {} over {ticks} ticks, longest tick {} ({} before the render)",
                    ms(*total),
                    ms(*longest),
                    ms(*work)
                );
                let parts: Vec<String> = heavy
                    .iter()
                    .map(|(n, t)| format!("{n} {}", ms(*t)))
                    .collect();
                println!("  in the longest tick: {}", parts.join(", "));
            }
            None => println!("{name}: did not land"),
        }
    }
}
