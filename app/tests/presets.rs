//! Brushes, tool presets and the quick bar: a library brush sets the brush,
//! the current brush and options save under a name and come back, and the
//! quick bar remembers what was painted with.

pub mod common;

use std::sync::{Arc, Mutex};

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use lumenui::prelude::*;
use pixl::tools::presets::{Presets, brush_from_png};
use pixl::tools::toolbox::Toolbox;
use pixl::{Editor, Tool};
use pixl_document::{ColorMode, Document, Rgba};
use pixl_io::png::encode_png_rgba;

#[derive(Clone, Debug, Default)]
struct Seen {
    size: u32,
    opacity: u8,
    tool: Option<Tool>,
    size_control: Option<f64>,
    brushes: Vec<String>,
    tool_presets: Vec<String>,
    colors: Vec<Rgba>,
    fg: Option<Rgba>,
}

type Watch = Arc<Mutex<Seen>>;

fn watch(app: App) -> (App, Watch) {
    let seen: Watch = Arc::default();
    let out = Arc::clone(&seen);
    let app = app.add_systems(
        TickStage::Systems,
        move |editor: Res<Editor>,
              toolbox: Res<Toolbox>,
              presets: Res<Presets>,
              signals: Signals| {
            *out.lock().unwrap() = Seen {
                size: editor.options.size,
                opacity: toolbox.options.opacity,
                tool: Some(editor.tool),
                size_control: signals.get::<f64>("opt_size"),
                brushes: presets.brushes.iter().map(|b| b.name.clone()).collect(),
                tool_presets: presets.tools.iter().map(|p| p.name.clone()).collect(),
                colors: presets.recent_colors.clone(),
                fg: Some(editor.fg),
            };
        },
    );
    (app, seen)
}

fn editor() -> App {
    editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap())
}

#[test]
fn a_library_brush_sets_the_brush_and_its_controls() {
    let (app, seen) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Wait,
            Step::Key("b", true),
            Step::Click("brush-pick-3"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "presets-brushes");
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.size, 8, "Soft 8 is eight pixels");
    assert_eq!(s.opacity, 60);
    assert_eq!(s.size_control, Some(8.0), "the size slider follows");
}

#[test]
fn the_current_brush_saves_under_a_name() {
    let (app, seen) = watch(editor());
    drive(
        app,
        vec![
            Step::Number("opt_size", 5.0),
            Step::Wait,
            Step::Text("brush_name", "Five"),
            Step::Click("brush-save"),
            Step::Wait,
        ],
    );
    assert_eq!(
        seen.lock().unwrap().brushes.last().map(String::as_str),
        Some("Five")
    );
}

#[test]
fn a_tool_preset_brings_back_its_tool_and_options() {
    let (app, seen) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Key("e", false),
            Step::Wait,
            Step::Number("opt_size", 9.0),
            Step::Wait,
            Step::Click("tp-open"),
            Step::Text("tp_name", "Fat eraser"),
            Step::Click("tp-save"),
            Step::Wait,
            Step::Wait,
            Step::Click("tp-close"),
            Step::Key("b", false),
            Step::Number("opt_size", 1.0),
            Step::Wait,
            Step::Click("tp-open"),
            Step::Wait,
            Step::Wait,
            Step::Click("tp-use-0"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
        ],
    );
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.tool_presets, ["Fat eraser"]);
    assert_eq!(s.tool, Some(Tool::Eraser));
    assert_eq!(s.size, 9);
    assert_eq!(out.tool, Tool::Eraser);
}

fn red(editor: &mut Editor) {
    editor.fg = Rgba::opaque(255, 0, 0);
}

fn blue(editor: &mut Editor) {
    editor.fg = Rgba::opaque(0, 0, 255);
}

#[test]
fn the_quick_bar_keeps_the_colors_painted_with() {
    let (app, seen) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Edit(red),
            Step::Down(At::Doc(3, 3), PointerButton::Primary),
            Step::Up(At::Doc(3, 3), PointerButton::Primary),
            Step::Wait,
            Step::Edit(blue),
            Step::Down(At::Doc(5, 5), PointerButton::Primary),
            Step::Up(At::Doc(5, 5), PointerButton::Primary),
            Step::Wait,
            Step::Wait,
            Step::Click("qb-color-1"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "presets-quickbar");
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.colors, [Rgba::opaque(0, 0, 255), Rgba::opaque(255, 0, 0)]);
    assert_eq!(
        s.fg,
        Some(Rgba::opaque(255, 0, 0)),
        "a click picks it again"
    );
}

#[test]
fn a_png_makes_a_brush_of_its_opaque_pixels() {
    let dir = std::env::temp_dir().join(format!("pixl-brush-png-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("star.png");
    let (o, c) = (Rgba::BLACK, Rgba::TRANSPARENT);
    let px = [c, o, c, o, o, o, c, o, c];
    std::fs::write(&path, encode_png_rgba(3, 3, &px).unwrap()).unwrap();
    let brush = brush_from_png(&path).unwrap();
    assert_eq!(brush.name, "star");
    assert_eq!(brush.size, 3);
    assert!(brush.summary().starts_with("custom 3 x 3"));
    let _ = std::fs::remove_dir_all(dir);
}
