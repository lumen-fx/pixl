//! The command palette: Ctrl+Shift+P opens it, typing narrows it, Enter
//! runs the best match: a tool, a layout, a recent document.

pub mod common;

use std::sync::{Arc, Mutex};

use common::drive::{Step, drive, editor_over};
use lumenui::lumen_core::input::NamedKey;
use lumenui::prelude::*;
use pixl::files::recents::Thumb;
use pixl::{Recents, Tool};
use pixl_document::native::{SaveOptions, save_file};
use pixl_document::{ColorMode, Document};

fn editor() -> App {
    editor_over(Document::new(32, 32, ColorMode::Rgba).unwrap())
}

/// Whether the palette is open, and which dock the color panel sits in.
type Seen = Arc<Mutex<(bool, Option<String>)>>;

fn watch(app: App) -> (App, Seen) {
    let seen: Seen = Arc::default();
    let out = Arc::clone(&seen);
    let app = app.add_systems(TickStage::Systems, move |signals: Signals| {
        let open = signals.get::<bool>("cp_open") == Some(true);
        let dock = dom::get_by_id("panel-color")
            .and_then(|n| n.parent())
            .and_then(|p| p.id());
        *out.lock().unwrap() = (open, dock);
    });
    (app, seen)
}

#[test]
fn the_palette_opens_narrows_and_runs_a_tool() {
    let (app, seen) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Ctrl("p", true),
            Step::Wait,
            Step::Text("cp_query", "era"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "palette-open");
    assert!(seen.lock().unwrap().0, "the palette is open");

    let (app, seen) = watch(editor());
    let out = drive(
        app,
        vec![
            Step::Ctrl("p", true),
            Step::Wait,
            Step::Text("cp_query", "era"),
            Step::Wait,
            Step::KeyDown(NamedKey::Enter),
            Step::Wait,
        ],
    );
    assert_eq!(out.tool, Tool::Eraser);
    assert!(
        !seen.lock().unwrap().0,
        "running a result closes the palette"
    );
}

#[test]
fn the_arrow_keys_pick_and_a_layout_runs() {
    let (app, seen) = watch(editor());
    drive(
        app,
        vec![
            Step::Ctrl("p", true),
            Step::Wait,
            Step::Text("cp_query", "layout"),
            Step::Wait,
            // Paint, Animate, Rig: the third is Rig.
            Step::KeyDown(NamedKey::ArrowDown),
            Step::KeyDown(NamedKey::ArrowDown),
            Step::Wait,
            Step::KeyDown(NamedKey::Enter),
            Step::Wait,
            Step::Wait,
            Step::Wait,
        ],
    );
    assert_eq!(seen.lock().unwrap().1.as_deref(), Some("dock-left"));
}

#[test]
fn a_recent_document_opens_from_the_palette() {
    let dir = std::env::temp_dir().join(format!("pixl-palette-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hero-walk.pixl");
    let doc = Document::new(24, 16, ColorMode::Rgba).unwrap();
    save_file(&doc, &path, SaveOptions::default()).unwrap();
    let mut recents = Recents::in_memory();
    recents.note(&path, &doc, Thumb::IfStale);
    let out = drive(
        editor().insert_resource(recents),
        vec![
            Step::Ctrl("p", true),
            Step::Wait,
            Step::Text("cp_query", "hero"),
            Step::Wait,
            Step::KeyDown(NamedKey::Enter),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert_eq!(out.doc.width(), 24);
    assert_eq!(out.title, "hero-walk.pixl");
    let _ = std::fs::remove_dir_all(dir);
}
