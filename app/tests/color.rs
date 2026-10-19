//! The color panel, driven with pointer drags and keys over a rendered
//! editor: the square, the hue strip, the wheel, the alpha strip, the
//! background swatch, the modes and the arrow keys.

pub mod common;

use common::drive::{At, Step, drive, editor_over};
use lumenui::lumen_core::input::{NamedKey, PointerButton};
use pixl::Settings;
use pixl::settings::{Channels, ColorSettings, Picker};
use pixl_document::{ColorMode, Document, Rgba};

fn doc() -> Document {
    Document::new(16, 16, ColorMode::Rgba).unwrap()
}

fn dark_red(e: &mut pixl::Editor) {
    e.fg = Rgba::opaque(200, 40, 40);
}

/// A drag on `id` from one point to another, shares of its size, with the
/// pointer brought over it first so it is hovered when the button goes
/// down.
fn drag(id: &'static str, from: (f32, f32), to: (f32, f32), release: bool) -> Vec<Step> {
    let mut steps = vec![
        Step::Edit(dark_red),
        Step::Move(At::Within(id, from.0, from.1)),
        Step::Down(At::Within(id, from.0, from.1), PointerButton::Primary),
        Step::Move(At::Within(id, (from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0)),
        Step::Move(At::Within(id, to.0, to.1)),
    ];
    if release {
        steps.push(Step::Up(At::Within(id, to.0, to.1), PointerButton::Primary));
    }
    steps.push(Step::Wait);
    steps
}

#[test]
fn a_drag_on_the_square_sets_saturation_and_value_when_released() {
    let out = drive(
        editor_over(doc()),
        drag("color-field", (0.5, 0.5), (1.0, 0.0), true),
    );
    // The top right corner of the square is the hue at full saturation
    // and value.
    assert_eq!(out.fg, Rgba::opaque(255, 0, 0));
    assert_eq!(out.bg, Rgba::WHITE);
    common::save(&out.frame, "color-square");
}

#[test]
fn a_drag_only_previews_until_the_pointer_comes_up() {
    let out = drive(
        editor_over(doc()),
        drag("color-field", (0.5, 0.5), (1.0, 0.0), false),
    );
    assert_eq!(out.fg, Rgba::opaque(200, 40, 40), "not set mid-drag");
}

#[test]
fn the_hue_strip_turns_the_hue_and_keeps_saturation_and_value() {
    let out = drive(
        editor_over(doc()),
        drag("color-hue", (0.5, 0.0), (0.5, 1.0 / 3.0), true),
    );
    // A third of the way down is green, at dark red's saturation and value.
    let hsv = out.fg.to_hsv();
    assert!((hsv.h - 120.0).abs() < 2.0, "{hsv:?}");
    assert_eq!(out.fg.g, 200);
    assert_eq!(out.fg.r, 40);
}

#[test]
fn the_wheel_ring_turns_the_hue() {
    let app = editor_over(doc()).insert_resource(Settings {
        color: ColorSettings {
            picker: Picker::Wheel,
            channels: Channels::Rgb,
        },
        ..Settings::default()
    });
    // The top of the ring, in a field wider than it is tall: a quarter
    // turn from red.
    let out = drive(app, drag("color-field", (0.5, 0.06), (0.5, 0.05), true));
    let hsv = out.fg.to_hsv();
    assert!((hsv.h - 90.0).abs() < 1.0, "{hsv:?}");
    common::save(&out.frame, "color-wheel");
}

#[test]
fn the_alpha_strip_sets_alpha() {
    let out = drive(
        editor_over(doc()),
        drag("color-alpha", (0.95, 0.5), (0.5, 0.5), true),
    );
    assert_eq!(out.fg, Rgba::new(200, 40, 40, 128));
}

#[test]
fn the_background_swatch_turns_the_panel_to_the_background() {
    let mut steps = vec![Step::Click("color-bg"), Step::Wait];
    steps.extend(drag("color-field", (0.5, 0.5), (0.0, 1.0), true));
    let out = drive(editor_over(doc()), steps);
    assert_eq!(out.bg, Rgba::BLACK, "the bottom of the square is black");
    assert_eq!(out.fg, Rgba::opaque(200, 40, 40), "the foreground stays");
}

#[test]
fn the_modes_are_kept_in_the_settings_and_hsv_sliders_set_the_color() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Edit(dark_red),
            Step::Wait,
            Step::Text("color_channels", "HSV"),
            Step::Text("color_picker", "Wheel"),
            Step::Wait,
            // Half the hue slider is cyan.
            Step::Number("color_c0", 0.5),
            Step::Wait,
        ],
    );
    assert_eq!(out.settings.color.channels, Channels::Hsv);
    assert_eq!(out.settings.color.picker, Picker::Wheel);
    assert_eq!(out.fg, Rgba::opaque(40, 200, 200));
}

#[test]
fn arrow_keys_step_what_is_under_the_pointer() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Edit(dark_red),
            Step::Move(At::Within("color-alpha", 0.5, 0.5)),
            Step::Wait,
            Step::KeyDown(NamedKey::ArrowLeft),
            Step::KeyUp(NamedKey::ArrowLeft),
            Step::KeyDown(NamedKey::ArrowLeft),
            Step::KeyUp(NamedKey::ArrowLeft),
            Step::Move(At::Within("color-field", 0.5, 0.5)),
            Step::Wait,
            Step::Shift(true),
            Step::KeyDown(NamedKey::ArrowUp),
            Step::KeyUp(NamedKey::ArrowUp),
            Step::Shift(false),
            Step::Wait,
        ],
    );
    assert_eq!(out.fg.a, 253, "two steps of alpha");
    // Shift steps the value by ten percent: from 200 to about 225.
    let v = out.fg.to_hsv().v;
    assert!((v - (200.0 / 255.0 + 0.1)).abs() < 0.01, "{v}");
}
