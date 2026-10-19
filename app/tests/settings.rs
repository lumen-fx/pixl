//! The settings dialog, rendered with no window: it opens from its key,
//! every page renders, a control changes its setting while the dialog is
//! open, and a rebound key runs its command.

pub mod common;

use lumenui::prelude::*;

use common::drive::{At, Step, drive, editor_over};
use lumenui::glam::Vec2;
use pixl::Settings;
use pixl::settings::WheelMode;
use pixl::settings::dialog::PAGES;
use pixl_document::{ColorMode, Document};

fn doc() -> Document {
    Document::new(64, 64, ColorMode::Rgba).unwrap()
}

#[test]
fn ctrl_comma_opens_the_dialog_and_done_closes_it() {
    let out = drive(editor_over(doc()), vec![Step::Ctrl(",", false), Step::Wait]);
    assert!(out.settings_open);
    common::save(&out.frame, "settings-open");
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Ctrl(",", false),
            Step::Wait,
            Step::Click("settings-done"),
        ],
    );
    assert!(!out.settings_open);
}

#[test]
fn every_page_renders() {
    for page in PAGES {
        let button: &'static str = Box::leak(format!("settings-page-{page}").into_boxed_str());
        let out = drive(
            editor_over(doc()),
            vec![
                Step::Ctrl(",", false),
                Step::Wait,
                Step::Click(button),
                Step::Wait,
                Step::Wait,
            ],
        );
        assert!(out.settings_open, "{page}");
        common::save(&out.frame, &format!("settings-{page}"));
    }
}

#[test]
fn a_control_sets_its_setting_while_the_dialog_is_open() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Ctrl(",", false),
            Step::Wait,
            Step::Click("settings-page-canvas"),
            Step::Wait,
            Step::Text("set_checker_size", "16"),
            Step::Wait,
            Step::Text("set_grid_color", "color(srgb 1 0 0 / 0.5)"),
            Step::Wait,
            Step::Text("set_pan_speed", "0"),
            Step::Wait,
        ],
    );
    let c = &out.settings.canvas;
    assert_eq!(c.checker_size, 16);
    assert_eq!(c.grid_color.as_deref(), Some("#ff000080"));
    // Out of range: the field keeps it, the setting does not.
    assert_eq!(out.settings.navigation.pan_speed, 1.0);
}

#[test]
fn the_theme_switch_turns_a_color_back_to_the_theme() {
    let mut settings = Settings::default();
    settings.canvas.background = Some("#204060".into());
    let out = drive(
        editor_over(doc()).insert_resource(settings),
        vec![
            Step::Wait,
            Step::Ctrl(",", false),
            Step::Wait,
            Step::Flag("set_background_theme", true),
            Step::Wait,
        ],
    );
    assert_eq!(out.settings.canvas.background, None);
}

#[test]
fn a_rebound_key_runs_its_command_and_a_taken_one_asks_first() {
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Ctrl(",", false),
            Step::Wait,
            Step::Click("settings-page-keyboard"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            // Z is free: it binds at once.
            Step::Click("key-bind-tool-pencil"),
            Step::Key("z", false),
            Step::Wait,
            // E is the eraser's: it waits for an answer, then moves.
            Step::Click("key-bind-tool-line"),
            Step::Key("e", false),
            Step::Wait,
            Step::Click("keys-take"),
            Step::Wait,
            Step::Click("settings-done"),
            Step::Wait,
            Step::Key("e", false),
            Step::Wait,
        ],
    );
    common::save(&out.frame, "settings-rebound");
    let o = &out.settings.shortcuts.overrides;
    assert_eq!(o.get("tool-pencil"), Some(&vec!["Z".to_string()]));
    assert_eq!(o.get("tool-line"), Some(&vec!["E".to_string()]));
    assert_eq!(o.get("tool-eraser"), Some(&Vec::new()));
    assert_eq!(out.tool, pixl::Tool::Line);
}

#[test]
fn the_wheel_follows_the_navigation_settings() {
    let mid = At::Window(Vec2::new(600.0, 450.0));
    let zoom = |s: Settings| {
        let out = drive(
            editor_over(doc()).insert_resource(s),
            vec![Step::Wait, Step::Wheel(mid, -32.0), Step::Wait],
        );
        let (a, b) = (&out.views[0], out.views.last().unwrap());
        (b.zoom / a.zoom, b.origin.1 - a.origin.1)
    };
    let (z, _) = zoom(Settings::default());
    assert!(z > 1.0, "a notch zooms in: {z}");
    let mut inverted = Settings::default();
    inverted.navigation.invert_wheel = true;
    let (z, _) = zoom(inverted);
    assert!(z < 1.0, "inverted, it zooms out: {z}");
    let mut panning = Settings::default();
    panning.navigation.wheel = WheelMode::Pan;
    panning.navigation.pan_speed = 2.0;
    let (z, dy) = zoom(panning);
    assert_eq!(z, 1.0);
    assert_eq!(dy, 64.0, "the wheel pans twice its distance");
}

/// With the settings dialog open over the canvas, the wheel scrolls the
/// dialog and leaves the view alone.
#[test]
fn the_wheel_over_a_dialog_leaves_the_canvas() {
    let mid = At::Window(Vec2::new(720.0, 450.0));
    let out = drive(
        editor_over(doc()),
        vec![
            Step::Ctrl(",", false),
            Step::Wait,
            Step::Move(mid),
            Step::Wait,
            Step::Wheel(mid, -32.0),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert!(out.settings_open);
    let (a, b) = (&out.views[0], out.views.last().unwrap());
    assert_eq!(b.zoom, a.zoom, "no zoom");
    assert_eq!(b.origin, a.origin, "no pan");
}

#[test]
fn the_menus_show_the_keys_in_force() {
    let mut settings = Settings::default();
    settings.shortcuts.preset = pixl::settings::keymap::Preset::Aseprite;
    settings
        .shortcuts
        .overrides
        .insert("undo".into(), vec!["Ctrl+U".into()]);
    let out = drive(
        editor_over(doc()).insert_resource(settings),
        vec![Step::Wait, Step::Click("menu-edit"), Step::Wait, Step::Wait],
    );
    common::save(&out.frame, "settings-edit-menu");
}

#[test]
fn the_interface_settings_reach_the_window() {
    let mut settings = Settings::default();
    settings.interface.theme = pixl::settings::Theme::Light;
    settings.interface.scale = 150;
    let out = drive(
        editor_over(doc()).insert_resource(settings.clone()),
        vec![Step::Wait, Step::Ctrl(",", false), Step::Wait, Step::Wait],
    );
    common::save(&out.frame, "settings-light-150");
    let out = drive(
        editor_over(doc()).insert_resource(settings),
        vec![Step::Wait, Step::Wait],
    );
    common::save(&out.frame, "light-150");
    // The light set's status bar, taller at 150 percent.
    let h = out.frame.height();
    assert_eq!(out.frame.pixel(4, h - 4), Some([0xe2, 0xe2, 0xe2, 0xff]));
    assert_eq!(out.frame.pixel(4, h - 36), Some([0xe2, 0xe2, 0xe2, 0xff]));
}

#[test]
fn a_new_buffer_budget_plans_the_layers_again() {
    use std::sync::{Arc, Mutex};
    // Five layers of 2048x2048 are 16 MiB each: 64 MiB holds four buffers,
    // so the bottom two layers share one; 128 MiB holds all five.
    let mut doc = Document::new(2048, 2048, ColorMode::Rgba).unwrap();
    for i in 0..4 {
        doc.add_layer(
            pixl_document::NewLayer::Raster,
            format!("l{i}"),
            pixl_document::Placement::default(),
        )
        .unwrap();
    }
    let mut settings = Settings::default();
    settings.performance.buffer_budget_mib = 64;
    let counts: Arc<Mutex<Vec<usize>>> = Arc::default();
    let seen = Arc::clone(&counts);
    let app = editor_over(doc).insert_resource(settings).add_systems(
        lumenui::prelude::TickStage::Systems,
        (move |layers: lumenui::prelude::Res<pixl::view::Layers>| {
            seen.lock().unwrap().push(layers.current_plan().len());
        })
        .after(pixl::view::draw_stage),
    );
    drive(
        app,
        vec![
            Step::Wait,
            Step::Ctrl(",", false),
            Step::Wait,
            Step::Text("set_buffer_budget", "128"),
            Step::Wait,
            Step::Wait,
        ],
    );
    let counts = counts.lock().unwrap();
    assert!(counts.contains(&4), "{counts:?}");
    assert_eq!(counts.last(), Some(&5), "{counts:?}");
}

/// Runs a saved, then edited, document for over a second with `settings`,
/// and says whether a recovery copy was written.
fn recovery_written(name: &str, settings: Settings) -> bool {
    let dir = std::env::temp_dir().join(format!("pixl-settings-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let mut editor = pixl::Editor::new(doc());
    pixl::files::save_to(&mut editor, &path).unwrap();
    let _ = pixl_document::native::discard_recovery(&path);
    drive(
        editor_over(doc())
            .insert_resource(editor)
            .insert_resource(settings),
        vec![
            Step::Wait,
            Step::Edit(|e| e.touch()),
            Step::Wait,
            Step::Sleep(1100),
            Step::Wait,
            Step::Wait,
        ],
    );
    pixl_document::native::find_recovery(&path).is_some()
}

#[test]
fn autosave_follows_the_files_settings() {
    let mut every_second = Settings::default();
    every_second.files.autosave_seconds = 1;
    assert!(recovery_written("every-second.pixl", every_second.clone()));
    let mut off = every_second;
    off.files.recovery = false;
    assert!(!recovery_written("recovery-off.pixl", off));
}

#[test]
fn without_zoom_to_cursor_the_wheel_zooms_around_the_middle() {
    let mut settings = Settings::default();
    settings.navigation.zoom_to_cursor = false;
    let corner = At::Window(Vec2::new(300.0, 200.0));
    let out = drive(
        editor_over(doc()).insert_resource(settings),
        vec![Step::Wait, Step::Wheel(corner, -32.0), Step::Wait],
    );
    let (a, b) = (&out.views[0], out.views.last().unwrap());
    assert!(b.zoom > a.zoom);
    let (ma, mb) = (a.to_doc(a.middle()), b.to_doc(b.middle()));
    assert!(
        (ma.0 - mb.0).abs() < 1.0 && (ma.1 - mb.1).abs() < 1.0,
        "{ma:?} {mb:?}"
    );
}
