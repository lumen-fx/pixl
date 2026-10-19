//! What the customization systems (the workspace, themes, the command
//! palette and export presets) cost per frame on a 4096x4096 document of 32
//! layers, rendered with no window: a second copy of each runs every tick
//! and is timed, with the Rig layout and a user theme in force, and the
//! whole tick is timed around them.
//!
//! `cargo bench -p pixl --bench customize -- cpu` or `-- gpu` picks the
//! renderer; `side=N`, `layers=N` and `ticks=N` change the run.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lumenui::ecs::system::{IntoSystem, System};
use lumenui::ecs::world::World;
use lumenui::prelude::*;
use pixl::Editor;
use pixl_document::{ColorMode, Document, NewLayer, Placement, Rect, Rgba};

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
        let t = doc.target(id, 0).unwrap();
        for ty in 0..(side / 64) as i32 {
            for tx in 0..(side / 64) as i32 {
                let c = Rgba::new((tx * 4) as u8, (ty * 4) as u8, (i * 8) as u8, 255);
                t.raster
                    .fill_rect_raw(&Rect::new(tx * 64, ty * 64, 64, 64), c.to_u32());
            }
        }
    }
    doc
}

fn app_dir(backend: &str) -> PathBuf {
    let app = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bench-customize-{backend}"));
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

type Times = Arc<Mutex<Vec<(&'static str, Vec<Duration>)>>>;

/// A system that runs a second copy of `system` and records how long it
/// took, after the editor's own copy has run.
fn timed<M, S: IntoSystem<(), (), M> + Send + 'static>(
    name: &'static str,
    system: S,
    times: Times,
) -> impl FnMut(&mut World) {
    let mut sys = IntoSystem::into_system(system);
    let mut ready = false;
    move |world: &mut World| {
        if !ready {
            sys.initialize(world);
            ready = true;
        }
        let t = Instant::now();
        let _ = sys.run((), world);
        let took = t.elapsed();
        let mut all = times.lock().unwrap();
        match all.iter_mut().find(|(n, _)| *n == name) {
            Some((_, v)) => v.push(took),
            None => all.push((name, vec![took])),
        }
    }
}

fn stats(v: &[Duration]) -> String {
    let mut v = v.to_vec();
    v.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let mean = v.iter().map(|d| us(*d)).sum::<f64>() / v.len().max(1) as f64;
    format!(
        "mean {:.1} us / p95 {:.1} us / max {:.1} us over {}",
        mean,
        us(v[((v.len() - 1) as f64 * 0.95) as usize]),
        us(*v.last().unwrap()),
        v.len()
    )
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
    let (side, layers, ticks) = (
        arg("side", 4096) as u32,
        arg("layers", 32),
        arg("ticks", 300) as u64,
    );
    let doc = document(side, layers);
    let times: Times = Arc::default();
    let frames: Arc<Mutex<Vec<Duration>>> = Arc::default();
    let frame_out = Arc::clone(&frames);
    let mut theme = pixl::settings::themes::UserTheme::parse(
        "/* pixl theme: Bench */ .pixl { --pixl-base: dark; --accent: #ff0080; }",
        "Bench",
    )
    .unwrap();
    theme.name = "Bench".into();
    let mut themes = pixl::Themes::in_memory();
    themes.put(theme).unwrap();
    let mut workspace = pixl::Workspace::in_memory();
    workspace.live = pixl::workspace::Layout::built_in("Rig").unwrap();

    pixl::editor(Source::Disk {
        dir: app_dir(&backend),
    })
    .insert_resource(Editor::new(doc))
    .insert_resource(themes)
    .insert_resource(workspace)
    .add_systems(TickStage::Systems, |mut s: ResMut<pixl::Settings>| {
        if s.interface.user_theme.is_none() {
            s.interface.user_theme = Some("Bench".into());
        }
    })
    .add_systems(
        TickStage::Systems,
        (
            timed(
                "workspace_input",
                pixl::workspace::workspace_input,
                times.clone(),
            ),
            timed(
                "show_workspace",
                pixl::workspace::show_workspace,
                times.clone(),
            ),
            timed(
                "theme_input",
                pixl::settings::themes::theme_input,
                times.clone(),
            ),
            timed(
                "show_themes",
                pixl::settings::themes::show_themes,
                times.clone(),
            ),
            timed(
                "palette_input",
                pixl::menus::palette::palette_input,
                times.clone(),
            ),
            timed(
                "show_palette",
                pixl::menus::palette::show_palette,
                times.clone(),
            ),
            timed(
                "export_input",
                pixl::files::exports::export_input,
                times.clone(),
            ),
            timed(
                "show_exports",
                pixl::files::exports::show_exports,
                times.clone(),
            ),
        )
            .chain()
            .after(pixl::files::exports::show_exports),
    )
    .add_systems(
        TickStage::Systems,
        move |mut last: Local<Option<Instant>>| {
            let now = Instant::now();
            if let Some(t) = last.replace(now) {
                frame_out.lock().unwrap().push(now - t);
            }
        },
    )
    .run_headless_rendered(HeadlessOptions {
        dpr: 1.0,
        ticks: Some(ticks),
    })
    .expect("the editor runs");

    println!("renderer: {backend}, {side}x{side}, {layers} layers, {ticks} ticks");
    let all = times.lock().unwrap();
    let mut total = Duration::ZERO;
    for (name, v) in all.iter() {
        // The first ticks find the panels and lay them out.
        let steady = &v[v.len().min(20)..];
        total += steady.iter().sum::<Duration>() / steady.len().max(1) as u32;
        println!("{name}: {}", stats(steady));
    }
    println!(
        "all customization systems: {:.1} us per tick on average",
        total.as_secs_f64() * 1e6
    );
    let f = frames.lock().unwrap();
    println!("tick to tick: {}", stats(&f[f.len().min(20)..]));
}
