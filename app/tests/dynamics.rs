//! Brush dynamics, driven through the Dynamics popup and the pointer: size,
//! angle and color following pen pressure and the pointer's speed.

pub mod common;

use common::drive::{At, Outcome, Step, drive, editor_over};
use lumenui::lumen_core::input::PointerButton;
use pixl::Tool;
use pixl::tools::dynamics::Follow;
use pixl_document::{ColorMode, Document, Rgba};

const L: PointerButton = PointerButton::Primary;

fn doc(mode: ColorMode) -> Document {
    Document::new(32, 32, mode).unwrap()
}

fn raw(out: &Outcome, x: i32, y: i32) -> Option<u32> {
    let layer = out.doc.layers()[0].id;
    let r = out.doc.cel_raster(layer, 0).unwrap()?;
    Some(r.get_raw(x, y))
}

fn rgba(out: &Outcome, x: i32, y: i32) -> Rgba {
    raw(out, x, y).map_or(Rgba::TRANSPARENT, Rgba::from_u32)
}

/// How many pixels of column `x` the stroke covered.
fn column(out: &Outcome, x: i32, clear: u32) -> usize {
    (0..32)
        .filter(|&y| raw(out, x, y).is_some_and(|v| v != clear))
        .count()
}

/// Opens the popup and sets its controls: the signal each writes, and the
/// value.
fn popup(controls: &[(&'static str, &'static str)]) -> Vec<Step> {
    let mut steps = vec![Step::Click("opt-dynamics"), Step::Wait];
    for (signal, value) in controls {
        steps.push(Step::Text(signal, value));
        steps.push(Step::Wait);
    }
    steps.push(Step::Click("dyn-close"));
    steps.push(Step::Wait);
    steps
}

/// A pen stroke along row `y` from `x0` to `x1` at one pressure.
fn pen_stroke(y: i32, x0: i32, x1: i32, pressure: f32) -> Vec<Step> {
    let mut steps = vec![Step::Move(At::Doc(x0, y)), Step::Wait];
    steps.push(Step::PenDown(At::Doc(x0, y), pressure));
    let mut x = x0;
    while x < x1 {
        x = (x + 2).min(x1);
        steps.push(Step::PenMove(At::Doc(x, y), pressure));
    }
    steps.push(Step::PenUp(At::Doc(x1, y)));
    steps
}

#[test]
fn the_popup_sets_the_tools_own_dynamics() {
    let mut steps = vec![Step::Key("b", false), Step::Wait];
    steps.extend(popup(&[("dyn_size", "Velocity"), ("dyn_size_min", "50")]));
    // The brush keeps its own.
    steps.extend([Step::Key("e", false), Step::Wait]);
    steps.extend(popup(&[("dyn_size", "Off")]));
    let out = drive(editor_over(doc(ColorMode::Rgba)), steps);
    let pencil = out.settings.dynamics(Tool::Pencil);
    assert_eq!(pencil.size, Follow::Velocity);
    assert_eq!(pencil.min_size, 50);
    assert_eq!(out.settings.dynamics(Tool::Eraser).size, Follow::Off);
    assert_eq!(
        out.settings.dynamics(Tool::Brush),
        pixl::tools::dynamics::ToolDynamics::for_tool(Tool::Brush)
    );
}

#[test]
fn a_mouse_stroke_grows_with_speed() {
    // The view fits the 32 pixel document at 1600 percent: a document
    // pixel is 16 screen pixels.
    let mut steps = vec![Step::Edit(|e| e.options.size = 8), Step::Wait];
    steps.extend(popup(&[("dyn_size", "Velocity")]));
    // Full velocity at 30 percent of the scale: 600 screen pixels a second.
    steps.push(Step::Number("dyn_v_max", 30.0));
    steps.push(Step::Wait);
    steps.extend([Step::Move(At::Doc(2, 16)), Step::Wait]);
    steps.push(Step::Down(At::Doc(2, 16), L));
    // Slowly, a pixel (16 screen pixels) every quarter of a second.
    for x in 3..=10 {
        steps.push(Step::Sleep(250));
        steps.push(Step::Move(At::Doc(x, 16)));
    }
    // Then fast, four pixels (64 screen pixels) a tick.
    for x in [14, 18, 22, 26, 30] {
        steps.push(Step::Move(At::Doc(x, 16)));
    }
    steps.push(Step::Up(At::Doc(30, 16), L));
    let out = drive(editor_over(doc(ColorMode::Rgba)), steps);
    common::save(&out.frame, "dynamics-velocity-stroke");
    // Clear of the wide brush the fast part ends on.
    assert_eq!(column(&out, 4, 0), 1, "slow paints one pixel");
    assert!(
        column(&out, 26, 0) >= 5,
        "fast paints wide: {}",
        column(&out, 26, 0)
    );
}

#[test]
fn pressure_colors_the_stroke_from_the_background_to_the_foreground() {
    let fg = Rgba::opaque(0, 0, 0);
    let bg = Rgba::opaque(255, 255, 255);
    let mut steps = vec![Step::Edit(|e| {
        e.fg = Rgba::opaque(0, 0, 0);
        e.bg = Rgba::opaque(255, 255, 255);
        e.options.size = 3;
    })];
    steps.extend(popup(&[("dyn_size", "Off"), ("dyn_color", "Pressure")]));
    steps.extend(pen_stroke(4, 2, 28, 1.0));
    steps.extend(pen_stroke(16, 2, 28, 0.5));
    steps.extend(pen_stroke(26, 2, 28, 0.0));
    let out = drive(editor_over(doc(ColorMode::Rgba)), steps);
    assert_eq!(rgba(&out, 15, 4), fg, "full pressure paints the foreground");
    assert_eq!(rgba(&out, 15, 26), bg, "no pressure paints the background");
    let mid = rgba(&out, 15, 16);
    assert!(
        mid != fg && mid != bg && mid.r > 100 && mid.r < 155,
        "{mid:?}"
    );
}

#[test]
fn a_dithered_color_in_indexed_mode_adds_no_color() {
    let mut steps = vec![Step::Edit(|e| {
        let p = e.doc.palette().colors().to_vec();
        e.fg = p[1];
        e.bg = p[2];
        e.options.size = 5;
    })];
    steps.extend(popup(&[
        ("dyn_size", "Off"),
        ("dyn_color", "Pressure"),
        ("dyn_dither", "Bayer 4x4"),
    ]));
    steps.extend(pen_stroke(8, 2, 28, 0.5));
    steps.extend(pen_stroke(20, 2, 28, 0.3));
    let out = drive(editor_over(doc(ColorMode::Indexed)), steps);
    common::save(&out.frame, "dynamics-dithered-indexed");
    let clear = u32::from(out.doc.transparent_index());
    let mut used: Vec<u32> = (0..32)
        .flat_map(|y| (0..32).map(move |x| (x, y)))
        .filter_map(|(x, y)| raw(&out, x, y))
        .filter(|&v| v != clear)
        .collect();
    used.sort();
    used.dedup();
    assert_eq!(used, [1, 2], "only the foreground and background entries");
}

#[test]
fn pressure_turns_a_square_brush() {
    let mut steps = vec![Step::Edit(|e| {
        e.options.size = 7;
        e.brush_shape = pixl::editor::BrushShape::Square;
    })];
    steps.extend(popup(&[
        ("dyn_size", "Off"),
        ("dyn_angle", "Pressure"),
        ("dyn_angle_min", "0"),
        ("dyn_angle_max", "45"),
    ]));
    // A dab at no pressure, and one at full pressure.
    steps.extend(pen_stroke(8, 8, 8, 0.0));
    steps.extend(pen_stroke(22, 22, 22, 1.0));
    let out = drive(editor_over(doc(ColorMode::Rgba)), steps);
    common::save(&out.frame, "dynamics-turned-square");
    let set = |x: i32, y: i32| rgba(&out, x, y).a > 0;
    assert!(
        set(5, 5) && set(11, 11),
        "the square's corners at 0 degrees"
    );
    assert!(
        !set(19, 19) && !set(26, 26),
        "turned 45 degrees, its corners go"
    );
    assert!(
        set(22, 19) && set(19, 22),
        "and its points reach out instead"
    );
}

#[test]
fn the_popup_opens_under_its_button() {
    let steps = vec![
        Step::Wait,
        Step::Click("opt-dynamics"),
        Step::Wait,
        Step::Text("dyn_angle", "Velocity"),
        Step::Wait,
        Step::Wait,
    ];
    let out = drive(editor_over(doc(ColorMode::Rgba)), steps);
    common::save(&out.frame, "dynamics-popup");
    assert_eq!(out.settings.dynamics(Tool::Pencil).angle, Follow::Velocity);
}
