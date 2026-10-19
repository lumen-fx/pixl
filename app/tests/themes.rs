//! Themes: the high contrast and retro sets, the theme editor reading the
//! tokens, an edit reaching the preview, a theme saved, and one imported.

pub mod common;

use std::sync::{Arc, Mutex};

use common::drive::{Step, drive, editor_over};
use lumenui::ecs::world::World;
use lumenui::prelude::*;
use pixl::settings::store::Store;
use pixl::settings::themes::{UserTheme, import};
use pixl::view::ViewColors;
use pixl::{Settings, Themes};
use pixl_document::{ColorMode, Document};

fn editor() -> App {
    editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap())
}

/// The status bar's background, the surface-1 token.
fn status_bar(frame: &Frame) -> [u8; 4] {
    frame
        .pixel(700, frame.height() - 4)
        .expect("inside the frame")
}

/// What the editor ended with, for the theme tests.
#[derive(Clone, Debug, Default)]
struct Seen {
    accent_field: Option<String>,
    preview_accent: Option<String>,
    root_accent: Option<String>,
    user_theme: Option<String>,
}

fn watch(app: App) -> (App, Arc<Mutex<Seen>>) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let out = Arc::clone(&seen);
    let app = app.add_systems(
        TickStage::Systems,
        move |settings: Res<Settings>, signals: Signals| {
            *out.lock().unwrap() = Seen {
                accent_field: signals.get::<String>("th_accent"),
                preview_accent: dom::get_by_id("tp-accent").and_then(|n| n.computed_style_of("bg")),
                root_accent: dom::get_by_id("app")
                    .and_then(|n| n.inline_style().get("--accent").cloned()),
                user_theme: settings.interface.user_theme.clone(),
            };
        },
    );
    (app, seen)
}

#[test]
fn high_contrast_and_retro_are_built_in_sets() {
    let out = drive(editor(), vec![Step::Edit(|_| {}), Step::Wait, Step::Wait]);
    assert_eq!(status_bar(&out.frame), [0x1a, 0x1a, 0x1a, 0xff]);
    for (theme, bar, name) in [
        (
            pixl::settings::Theme::Contrast,
            [0, 0, 0, 0xff],
            "theme-contrast",
        ),
        (
            pixl::settings::Theme::Retro,
            [0x1d, 0x4a, 0x1d, 0xff],
            "theme-retro",
        ),
    ] {
        let app = editor().add_systems(TickStage::Systems, move |mut s: ResMut<Settings>| {
            s.interface.theme = theme;
        });
        let out = drive(app, vec![Step::Wait, Step::Wait]);
        common::save(&out.frame, name);
        assert_eq!(status_bar(&out.frame), bar, "{name}");
    }
}

#[test]
fn the_editor_reads_the_tokens_and_an_edit_reaches_the_preview_and_a_saved_theme() {
    let dir = std::env::temp_dir().join(format!("pixl-themes-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (app, seen) = watch(editor().insert_resource(Themes::load(Store::at(&dir))));
    let out = drive(
        app,
        vec![
            Step::Menu("window-themes"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Text("th_accent", "color(srgb 1 0 0.5)"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "themes-editor");
    let s = seen.lock().unwrap().clone();
    // The field keeps the color in its own form.
    assert_eq!(
        s.accent_field
            .as_deref()
            .and_then(pixl::settings::themes::parse_color),
        Some(pixl_document::Rgba::opaque(255, 0, 128))
    );
    assert_eq!(s.preview_accent.as_deref(), Some("#ff0080"));
    assert_eq!(s.root_accent.as_deref(), Some("#ff0080"));

    let (app, seen) = watch(editor().insert_resource(Themes::load(Store::at(&dir))));
    drive(
        app,
        vec![
            Step::Menu("window-themes"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
            Step::Text("th_accent", "#ff0080"),
            Step::Wait,
            Step::Text("th_name", "Pink"),
            Step::Click("th-save"),
            Step::Wait,
            Step::Click("th-close"),
            Step::Wait,
            Step::Wait,
        ],
    );
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.user_theme.as_deref(), Some("Pink"));
    assert_eq!(s.root_accent.as_deref(), Some("#ff0080"));
    let kept = Themes::load(Store::at(&dir));
    let pink = kept.get("Pink").expect("the theme was saved");
    assert_eq!(pink.base.class(), "dark");
    assert_eq!(
        pink.tokens["accent"],
        pixl_document::Rgba::opaque(255, 0, 128)
    );
    assert_eq!(pink.tokens.len(), pixl::settings::themes::TOKENS.len());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_imported_theme_is_kept_and_put_in_force() {
    let dir = std::env::temp_dir().join(format!("pixl-themes-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("shared.pixltheme");
    std::fs::write(
        &file,
        "/* pixl theme: Shared */\n.pixl {\n  --pixl-base: retro;\n  --accent: #ffcc00;\n}\n",
    )
    .unwrap();
    let mut world = World::new();
    world.insert_resource(Themes::load(Store::at(dir.join("data"))));
    world.insert_resource(Settings::default());
    world.insert_resource(ViewColors::default());
    let note = import(&mut world, &file);
    assert_eq!(note, "Imported Shared");
    let settings = world.resource::<Settings>();
    assert_eq!(settings.interface.user_theme.as_deref(), Some("Shared"));
    assert_eq!(settings.interface.theme.class(), "retro");
    let kept = Themes::load(Store::at(dir.join("data")));
    assert_eq!(
        kept.get("Shared").map(|t| t.tokens.len()),
        Some(1),
        "the theme is in the themes folder"
    );
    let broken = dir.join("broken.pixltheme");
    std::fs::write(&broken, "nothing here").unwrap();
    assert!(import(&mut world, &broken).starts_with("Could not import"));
    let _ = UserTheme::parse("--accent: #fff;", "x").unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
