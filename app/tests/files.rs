//! Documents on disk: save and open, import, export, the recovery copy, and
//! the New and discard dialogs.

pub mod common;

use std::path::PathBuf;

use common::drive::{Step, drive, editor_over};
use pixl::Editor;
use pixl::files::{Files, export_to, open_path, read_document, save_to};
use pixl_core::native;
use pixl_core::{ColorMode, Document, Rgba};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("files");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(native::recovery_path(&path));
    path
}

fn dotted() -> Document {
    let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
    let id = doc.layers()[0].id;
    doc.target(id, 0)
        .unwrap()
        .raster
        .set_raw(3, 4, Rgba::opaque(9, 8, 7).to_u32());
    doc
}

fn pixel(doc: &Document, x: i32, y: i32) -> Rgba {
    let id = doc.layers()[0].id;
    Rgba::from_u32(doc.cel_raster(id, 0).unwrap().unwrap().get_raw(x, y))
}

#[test]
fn a_saved_document_opens_as_it_was_and_reads_unmodified() {
    let path = scratch("saved");
    let mut editor = Editor::new(dotted());
    editor.touch();
    assert!(editor.is_modified());
    save_to(&mut editor, &path).unwrap();
    let saved = path.with_extension("pixl");
    assert_eq!(editor.path.as_deref(), Some(saved.as_path()));
    assert!(!editor.is_modified());

    let mut other = Editor::blank();
    let mut files = Files::default();
    open_path(&mut other, &mut files, &saved);
    assert_eq!(pixel(&other.doc, 3, 4), Rgba::opaque(9, 8, 7));
    assert_eq!(other.title(), "saved.pixl");
    assert!(files.recovery.is_none());
}

#[test]
fn a_newer_recovery_copy_is_offered_on_open() {
    let path = scratch("recover.pixl");
    let mut editor = Editor::new(dotted());
    save_to(&mut editor, &path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    native::write_recovery(&editor.doc, &path).unwrap();
    let mut files = Files::default();
    open_path(&mut Editor::blank(), &mut files, &path);
    assert_eq!(files.recovery.as_deref(), Some(path.as_path()));
}

#[test]
fn aseprite_and_png_files_import_as_new_documents() {
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../core/tests/data/aseprite");
    let ase = read_document(&data.join("layers_and_tags.aseprite")).unwrap();
    assert!(ase.doc.layers().len() > 1);
    let png = read_document(&data.join("basic-16x16.png")).unwrap();
    assert_eq!((png.doc.width(), png.doc.height()), (16, 16));

    let mut editor = Editor::blank();
    let mut files = Files::default();
    open_path(&mut editor, &mut files, &data.join("basic-16x16.aseprite"));
    assert_eq!(editor.path, None, "saving an import never writes over it");
}

#[test]
fn exports_write_png_and_gif() {
    let mut editor = Editor::new(dotted());
    let png = scratch("out.png");
    export_to(&mut editor, &png, false).unwrap();
    let back = read_document(&png).unwrap();
    assert_eq!(pixel(&back.doc, 3, 4), Rgba::opaque(9, 8, 7));
    let gif = scratch("out");
    export_to(&mut editor, &gif, true).unwrap();
    let bytes = std::fs::read(gif.with_extension("gif")).unwrap();
    assert_eq!(&bytes[..6], b"GIF89a");
    // Open reads a GIF back as a new document.
    let back = read_document(&gif.with_extension("gif")).unwrap();
    let shown = pixl_core::composite(
        &back.doc,
        0,
        pixl_core::Rect::pixel(3, 4),
        &pixl_core::CompositeOptions::EXPORT,
    );
    assert_eq!(shown, [Rgba::opaque(9, 8, 7)]);
}

#[test]
fn new_asks_before_discarding_edits_then_makes_the_size_asked_for() {
    let out = drive(
        editor_over(dotted()),
        vec![
            Step::Edit(|e| e.touch()),
            Step::Menu("file-new"),
            Step::Wait,
            Step::Click("discard-yes"),
            Step::Wait,
            Step::Text("new_width", "40"),
            Step::Text("new_height", "24"),
            Step::Click("new-create"),
            Step::Wait,
        ],
    );
    assert_eq!((out.doc.width(), out.doc.height()), (40, 24));
}

#[test]
fn keeping_the_edits_keeps_the_document() {
    let out = drive(
        editor_over(dotted()),
        vec![
            Step::Edit(|e| e.touch()),
            Step::Menu("file-new"),
            Step::Wait,
            Step::Click("discard-no"),
            Step::Wait,
        ],
    );
    assert_eq!(pixel(&out.doc, 3, 4), Rgba::opaque(9, 8, 7));
}
