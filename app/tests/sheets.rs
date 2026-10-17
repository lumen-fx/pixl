//! Sprite sheets, rendered with no window: both dialogs open from the File
//! menu and show their previews, an export writes the PNG and its JSON, and
//! an import splits a sheet into the frames of a new document.

pub mod common;

use std::path::PathBuf;

use common::drive::{Step, drive, editor_over};
use pixl::sheets::{ExportChoice, Sheets, read_sheet_image, write_sheet};
use pixl_core::sheet::{SheetLayout, SheetOptions};
use pixl_core::{ColorMode, Document, Rect, Rgba};

/// Four frames of a 16x16 sprite, each with a bar one pixel longer.
fn animation() -> Document {
    let mut d = Document::new(16, 16, ColorMode::Rgba).unwrap();
    for f in 1..4 {
        d.insert_frame(f, 100).unwrap();
    }
    let l = d.layers()[0].id;
    for f in 0..4 {
        d.target(l, f).unwrap().raster.fill_rect_raw(
            &Rect::new(2, 6, 4 + f as i32 * 3, 4),
            Rgba::opaque(240, 120, 40).to_u32(),
        );
    }
    d
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pixl-sheets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn the_export_dialog_previews_the_sheet() {
    let out = drive(
        editor_over(animation()),
        vec![
            Step::Menu("file-export-sheet"),
            Step::Wait,
            Step::Text("sx_layout", "Packed"),
            Step::Flag("sx_trim", true),
            Step::Text("sx_padding", "1"),
            Step::Wait,
            Step::Sleep(150),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "sheet-export");
}

#[test]
fn an_export_writes_the_png_and_its_json() {
    let path = temp("walk.png");
    let doc = animation();
    let choice = ExportChoice {
        options: SheetOptions {
            layout: SheetLayout::Grid { columns: 2 },
            padding: 2,
            border: 1,
            title: "walk".into(),
            ..SheetOptions::default()
        },
        json: true,
    };
    let message = write_sheet(&doc, &choice, &path).unwrap();
    assert!(message.contains("4 images"), "{message}");
    let image = read_sheet_image(&path).unwrap();
    assert_eq!(
        (image.width, image.height),
        (2 * 16 + 2 + 2, 2 * 16 + 2 + 2)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.with_extension("json")).unwrap()).unwrap();
    assert_eq!(json["meta"]["image"], "walk.png");
    assert_eq!(
        json["frames"]["walk 3"]["frame"],
        serde_json::json!({"x": 19, "y": 19, "w": 16, "h": 16})
    );
}

#[test]
fn an_import_splits_a_sheet_into_a_new_document() {
    // A sheet of the animation's four frames in a row.
    let path = temp("strip.png");
    let choice = ExportChoice {
        options: SheetOptions::default(),
        json: false,
    };
    write_sheet(&animation(), &choice, &path).unwrap();
    let image = read_sheet_image(&path).unwrap();
    let preview = drive(
        editor_over(Document::new(8, 8, ColorMode::Rgba).unwrap())
            .insert_resource(chosen(image.clone())),
        vec![
            Step::Menu("file-import-sheet"),
            Step::Wait,
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&preview.frame, "sheet-import");
    let out = drive(
        editor_over(Document::new(8, 8, ColorMode::Rgba).unwrap()).insert_resource(chosen(image)),
        vec![
            Step::Menu("file-import-sheet"),
            Step::Wait,
            Step::Wait,
            Step::Text("si_ms", "80"),
            Step::Wait,
            Step::Click("si-import"),
            Step::Wait,
            Step::Wait,
        ],
    );
    // A strip guesses square cells as tall as it is.
    assert_eq!((out.doc.width(), out.doc.height()), (16, 16));
    assert_eq!(out.doc.frame_count(), 4);
    assert!(out.doc.frames().iter().all(|f| f.duration_ms == 80));
    let flat =
        |d: &Document, f| pixl_core::composite_frame(d, f, &pixl_core::CompositeOptions::EXPORT);
    let original = animation();
    for f in 0..4 {
        assert_eq!(flat(&out.doc, f), flat(&original, f), "frame {f}");
    }
    assert!(
        out.message
            .unwrap_or_default()
            .contains("Imported 4 frames")
    );
}

/// The import dialog with `image` already chosen, as the file dialog
/// leaves it.
fn chosen(image: pixl::sheets::SheetImage) -> Sheets {
    let mut sheets = Sheets::default();
    sheets.set_image(image);
    sheets
}
