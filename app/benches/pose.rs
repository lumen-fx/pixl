//! How fast posing answers: a 20-bone character, each bone a layer of its
//! own, posed on a frame other than its rest frame, so every pointer move
//! renders the pose into the frame. Times each move to the frame that shows
//! it, rendered with no window.
//!
//! `cargo bench -p pixl --bench pose -- cpu` or `-- gpu` picks the renderer;
//! `side=N` puts the 256x256 character in a larger document and
//! `layers=N` adds that many full layers under it (the view bench's
//! 4096x4096 document of 32 layers is `side=4096 layers=32`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lumenui::glam::Vec2 as Window;
use lumenui::lumen_core::input::{PointerButton, PointerData, PointerMoved, PointerPressed};
use lumenui::lumen_core::render_world::SurfaceCapture;
use lumenui::prelude::*;
use pixl::Editor;
use pixl::rig::{RigMode, RigUi};
use pixl::view::{Layers, View};
use pixl_core::rig::Vec2;
use pixl_core::{Binding, ColorMode, Document, LayerId, NewLayer, Placement, Point, Rect, Rgba};

const MOVES: usize = 60;
/// Frames timed before the drag, with nothing changing: the renderer's own
/// cost of a frame.
const IDLE: usize = 20;

/// Head, tail and parent of each bone: a spine, a head, two arms, two legs
/// and a tail, on a 256x256 area.
fn skeleton() -> Vec<(Vec2, Vec2, Option<usize>)> {
    let v = Vec2::new;
    let mut bones = vec![
        (v(128.0, 150.0), v(128.0, 130.0), None),
        (v(128.0, 130.0), v(128.0, 110.0), Some(0)),
        (v(128.0, 110.0), v(128.0, 92.0), Some(1)),
        (v(128.0, 92.0), v(128.0, 80.0), Some(2)),
        (v(128.0, 80.0), v(128.0, 52.0), Some(3)),
    ];
    for side in [-1.0, 1.0] {
        let (mut parent, mut at) = (2, v(128.0, 100.0));
        for _ in 0..3 {
            let next = at + v(side * 22.0, 10.0);
            bones.push((at, next, Some(parent)));
            parent = bones.len() - 1;
            at = next;
        }
        let (mut parent, mut at) = (0, v(128.0 + side * 10.0, 150.0));
        for _ in 0..3 {
            let next = at + v(side * 4.0, 26.0);
            bones.push((at, next, Some(parent)));
            parent = bones.len() - 1;
            at = next;
        }
    }
    let (mut parent, mut at) = (0, v(128.0, 150.0));
    for _ in 0..3 {
        let next = at + v(-14.0, 14.0);
        bones.push((at, next, Some(parent)));
        parent = bones.len() - 1;
        at = next;
    }
    bones
}

fn paint_limb(doc: &mut Document, layer: LayerId, head: Vec2, tail: Vec2) {
    let t = doc.target(layer, 0).unwrap();
    let d = tail - head;
    let len = d.length();
    let n = Vec2::new(-d.y / len, d.x / len);
    for i in 0..=(len as i32 * 2) {
        let c = head + d * (i as f64 / (len * 2.0));
        for k in -4i32..=4 {
            let p = c + n * k as f64;
            let v = if k.abs() == 4 {
                Rgba::opaque(30, 20, 20)
            } else {
                Rgba::opaque(200, 120, 80)
            };
            t.raster.set_raw(p.x as i32, p.y as i32, v.to_u32());
        }
    }
}

/// The document: `layers` full layers, then the character's 20 part
/// layers and its skeleton, two frames. Returns the skeleton and the bone
/// the bench turns (the right forearm) with its tail.
fn document(side: u32, layers: usize) -> (Document, LayerId, Vec2) {
    let mut doc = Document::new(side, side, ColorMode::Rgba).unwrap();
    let first = doc.layers()[0].id;
    for i in 0..layers {
        let id = if i == 0 {
            first
        } else {
            doc.add_layer(NewLayer::Raster, format!("Fill {i}"), Placement::default())
                .unwrap()
        };
        let t = doc.target(id, 0).unwrap();
        for ty in 0..(side / 64) as i32 {
            for tx in 0..(side / 64) as i32 {
                let a = if (tx + ty + i as i32) % 5 == 0 {
                    0
                } else {
                    255
                };
                let c = Rgba::new((tx * 4) as u8, (ty * 4) as u8, (i * 8) as u8, a);
                t.raster
                    .fill_rect_raw(&Rect::new(tx * 64, ty * 64, 64, 64), c.to_u32());
            }
        }
    }
    if layers == 0 {
        doc.remove_layer(first).ok();
    }
    doc.insert_frame(1, 100).unwrap();
    let skel = doc
        .add_layer(NewLayer::Skeleton, "Skeleton", Placement::default())
        .unwrap();
    let mut ids = Vec::new();
    let bones = skeleton();
    for (i, (head, tail, parent)) in bones.iter().enumerate() {
        let layer = doc
            .add_layer(NewLayer::Raster, format!("Part {i}"), Placement::default())
            .unwrap();
        paint_limb(&mut doc, layer, *head, *tail);
        let rig = doc.rig_mut(skel).unwrap();
        let id = rig
            .add_bone(format!("bone {i}"), parent.map(|p| ids[p]), *head, *tail)
            .unwrap();
        rig.bind(Binding {
            layer,
            region: None,
            bone: id,
            pivot: *head,
        })
        .unwrap();
        ids.push(id);
    }
    // The bone after the left arm's three: the right upper arm's... pick the
    // middle bone of the right arm.
    let (head, tail, _) = bones[12];
    (doc, skel, (head + tail) * 0.5)
}

fn app_dir(backend: &str) -> PathBuf {
    let app = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bench-pose-{backend}"));
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
    let (side, layers) = (arg("side", 256) as u32, arg("layers", 0));
    let (doc, skel, grab) = document(side, layers);

    let capture = SurfaceCapture::default();
    let stop = Arc::new(AtomicBool::new(false));
    let started: Arc<Mutex<Option<Instant>>> = Arc::default();
    let lap: Arc<Mutex<Option<Duration>>> = Arc::default();
    {
        let (s, l, c, stop) = (
            Arc::clone(&started),
            Arc::clone(&lap),
            capture.clone(),
            Arc::clone(&stop),
        );
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let begun = *s.lock().unwrap();
                if let Some(t0) = begun
                    && !c.is_requested()
                {
                    *l.lock().unwrap() = Some(t0.elapsed());
                    *s.lock().unwrap() = None;
                }
                std::thread::sleep(Duration::from_micros(50));
            }
        });
    }
    let laps: Arc<Mutex<Vec<Duration>>> = Arc::default();
    let idle: Arc<Mutex<Vec<Duration>>> = Arc::default();
    let out = Arc::clone(&laps);
    let idle_out = Arc::clone(&idle);
    let cap = capture.clone();
    let world_capture = capture.clone();
    let mut editor = Editor::new(doc);
    editor.frame = 1;
    editor.layer = skel;
    pixl::editor(Source::Disk {
        dir: app_dir(&backend),
    })
    .insert_resource(editor)
    .add_systems(
        TickStage::Systems,
        (move |mut step: Local<Option<usize>>,
               mut ui: ResMut<RigUi>,
               view: Res<View>,
               layers: Res<Layers>,
               mut down: MessageWriter<PointerPressed>,
               mut moved: MessageWriter<PointerMoved>| {
            ui.mode = Some(RigMode::Pose);
            if view.stage.1.x <= 0.0 || layers.pending() > 0 {
                return;
            }
            if started.lock().unwrap().is_some() {
                return;
            }
            let n = step.get_or_insert(0);
            if let Some(l) = lap.lock().unwrap().take() {
                if *n <= IDLE {
                    idle_out.lock().unwrap().push(l);
                } else if *n > IDLE + 1 {
                    out.lock().unwrap().push(l);
                }
            }
            if *n > IDLE + MOVES {
                return;
            }
            if *n < IDLE {
                *started.lock().unwrap() = Some(Instant::now());
                cap.request();
                *n += 1;
                return;
            }
            let screen = |p: Vec2| {
                let s = pixl::tools::screen_of(&view, Point::new(p.x as i32, p.y as i32));
                Window::new(s.x, s.y)
            };
            if *n == IDLE {
                down.write(PointerPressed {
                    position: screen(grab),
                    button: PointerButton::Primary,
                    local: None,
                    pointer: PointerData::mouse(1),
                });
            } else {
                // Swing around the arm's shoulder.
                let a = (*n - IDLE) as f64 * 0.15;
                let p = Vec2::new(150.0 + 30.0 * a.cos(), 105.0 + 30.0 * a.sin());
                moved.write(PointerMoved {
                    position: screen(p),
                    local: None,
                    pointer: PointerData::mouse(1),
                });
            }
            *started.lock().unwrap() = Some(Instant::now());
            cap.request();
            *n += 1;
        })
        .before(pixl::input::navigate),
    )
    .add_plugin(BenchCapture(world_capture))
    .run_headless_rendered(HeadlessOptions {
        dpr: 1.0,
        ticks: Some(1500),
    })
    .expect("the editor runs");
    stop.store(true, Ordering::Relaxed);
    let v = laps.lock().unwrap();
    let ms = |d: Duration| format!("{:.2} ms", d.as_secs_f64() * 1000.0);
    println!("renderer: {backend}, {side}x{side}, {layers} fill layers + 20 parts");
    let idle = idle.lock().unwrap();
    if !idle.is_empty() {
        println!(
            "idle frame: median {} / p95 {} over {}",
            ms(percentile(&idle, 0.5)),
            ms(percentile(&idle, 0.95)),
            idle.len()
        );
    }
    if v.is_empty() {
        println!("no pose moves were timed");
        return;
    }
    println!(
        "pose move to screen: median {} / p95 {} / max {} over {}",
        ms(percentile(&v, 0.5)),
        ms(percentile(&v, 0.95)),
        ms(*v.iter().max().unwrap()),
        v.len()
    );
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
