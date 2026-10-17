//! Export presets: the dialog edits a preset, and Export all writes every
//! marked preset's files beside the document.

pub mod common;

use std::path::PathBuf;

use common::drive::{Step, drive, editor_over};
use pixl::Editor;
use pixl_core::{ColorMode, Document};

fn folder() -> PathBuf {
    std::env::temp_dir().join(format!("pixl-export-ui-{}", std::process::id()))
}

fn saved_as_hero(editor: &mut Editor) {
    editor.path = Some(folder().join("hero.pixl"));
}

#[test]
fn export_all_writes_the_marked_presets_beside_the_document() {
    let _ = std::fs::remove_dir_all(folder());
    let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
    doc.insert_frame(0, 100).unwrap();
    // Export all writes on a worker thread and says what it wrote.
    let steps = vec![
        Step::Edit(saved_as_hero),
        Step::Menu("file-export-all"),
        Step::Until(|e| e.message.as_deref().is_some_and(|m| !m.ends_with("..."))),
    ];
    let out = drive(editor_over(doc), steps);
    // The starters: PNG frames and the sprite sheet run; the GIF does not.
    let made = |name: &str| folder().join(name).is_file();
    assert!(
        made("hero-0.png") && made("hero-1.png"),
        "{:?}",
        out.message
    );
    assert!(made("hero-sheet.png") && made("hero-sheet.json"));
    assert_eq!(out.message.as_deref(), Some("Exported 4 files"));
    let _ = std::fs::remove_dir_all(folder());
}

#[test]
fn an_unsaved_document_is_told_to_save_first() {
    let out = drive(
        editor_over(Document::new(8, 8, ColorMode::Rgba).unwrap()),
        vec![Step::Menu("file-export-all"), Step::Wait],
    );
    assert_eq!(
        out.message.as_deref(),
        Some("Save the document first: presets write beside it")
    );
}

#[test]
fn the_dialog_edits_the_selected_preset() {
    let out = drive(
        editor_over(Document::new(8, 8, ColorMode::Rgba).unwrap()),
        vec![
            Step::Menu("file-export-presets"),
            Step::Wait,
            Step::Wait,
            Step::ClickInside("ex-list"),
            Step::Wait,
            Step::Text("ex_format", "Sprite sheet"),
            Step::Text("ex_path", "out/{name}-{scale}x"),
            Step::Text("ex_scale", "3"),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "exports-dialog");
}
