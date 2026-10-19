//! Scripts run from File > Scripts and from the command palette: the run
//! changes the document as one undo step, prints into the output dialog,
//! and extensions add their commands.

pub mod common;

use std::path::{Path, PathBuf};

use common::drive::{Step, drive, editor_over};
use lumenui::lumen_core::input::NamedKey;
use pixl::Editor;
use pixl::scripts::Scripts;
use pixl_document::{ColorMode, Document, Rgba};

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pixl-scripts-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    dir
}

const FILL: &str = r##"import "pixl" as app;
fn main() {
    let doc = app::document();
    let color = app::hex_color(app::param("color", "#ff0000"));
    doc.active_layer().cel(doc.active_frame()).fill(color);
    print("filled " + doc.name());
}
"##;

fn pixel(doc: &Document, x: i32, y: i32) -> Rgba {
    let layer = doc.layers()[0].id;
    doc.cel_raster(layer, 0)
        .unwrap()
        .map_or(Rgba::TRANSPARENT, |r| Rgba::from_u32(r.get_raw(x, y)))
}

fn ran(e: &Editor) -> bool {
    e.message
        .as_deref()
        .is_some_and(|m| m.starts_with("Ran ") || m.contains("failed"))
}

fn blank() -> Document {
    Document::new(8, 8, ColorMode::Rgba).unwrap()
}

#[test]
fn a_script_from_the_dialog_is_one_undo_step() {
    let dir = folder("dialog");
    std::fs::write(dir.join("scripts/fill.cdl"), FILL).unwrap();
    let app = editor_over(blank()).insert_resource(Scripts::at(&dir));
    let out = drive(
        app,
        vec![
            Step::Menu("file-scripts"),
            Step::Wait,
            Step::Wait,
            Step::Click("sc-item-0"),
            Step::Wait,
            Step::Click("sc-run"),
            Step::Until(ran),
            Step::Wait,
            Step::Wait,
        ],
    );
    common::save(&out.frame, "scripts-output");
    assert!(
        out.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Ran fill")),
        "{:?}",
        out.message
    );
    assert_eq!(pixel(&out.doc, 3, 3), Rgba::opaque(255, 0, 0));
    assert_eq!(out.undo.last().map(String::as_str), Some("Script: fill"));
    assert!(out.modified);

    // Undo takes the whole run back.
    let app = editor_over(blank()).insert_resource(Scripts::at(&dir));
    let out = drive(
        app,
        vec![
            Step::Menu("file-scripts"),
            Step::Wait,
            Step::Click("sc-run"),
            Step::Until(ran),
            Step::Wait,
            Step::Menu("undo"),
            Step::Wait,
        ],
    );
    assert_eq!(pixel(&out.doc, 3, 3), Rgba::TRANSPARENT);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_extension_command_runs_from_the_command_palette_with_its_params() {
    let dir = folder("palette");
    let ext = dir.join("extensions/fills");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(ext.join("fill.cdl"), FILL).unwrap();
    std::fs::write(
        ext.join("extension.json"),
        r##"{"name": "Fills", "commands": [
            {"id": "blue", "label": "Fill with blue", "script": "fill.cdl",
             "params": {"color": "#0000ff"}}
        ]}"##,
    )
    .unwrap();
    let app = editor_over(blank()).insert_resource(Scripts::at(&dir));
    let out = drive(
        app,
        vec![
            Step::Ctrl("p", true),
            Step::Wait,
            Step::Text("cp_query", "fill with blue"),
            Step::Wait,
            Step::KeyDown(NamedKey::Enter),
            Step::Until(ran),
            Step::Wait,
        ],
    );
    assert_eq!(pixel(&out.doc, 0, 0), Rgba::opaque(0, 0, 255));
    assert_eq!(
        out.undo.last().map(String::as_str),
        Some("Script: Fill with blue")
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_failing_script_leaves_the_document_and_says_why() {
    let dir = folder("fail");
    std::fs::write(
        dir.join("scripts/bad.cdl"),
        "import \"pixl\" as app;\nfn main() {\n    app::document().layers()[0].cel(0).set(0, 0, app::rgb(1, 2, 3));\n    let zero = 0;\n    print(1 / zero);\n}\n",
    )
    .unwrap();
    let app = editor_over(blank()).insert_resource(Scripts::at(&dir));
    let out = drive(
        app,
        vec![
            Step::Menu("file-scripts"),
            Step::Wait,
            Step::Click("sc-run"),
            Step::Until(ran),
            Step::Wait,
        ],
    );
    let message = out.message.unwrap_or_default();
    assert!(message.starts_with("bad failed"), "{message}");
    assert_eq!(pixel(&out.doc, 0, 0), Rgba::TRANSPARENT);
    assert!(out.undo.iter().all(|u| !u.starts_with("Script")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_script_that_makes_a_document_opens_it_in_a_tab() {
    let dir = folder("new");
    std::fs::write(
        dir.join("scripts/make.cdl"),
        "import \"pixl\" as app;\nfn main() {\n    let d = app::new_document(4, 4);\n    d.layers()[0].cel(0).set(1, 1, app::rgb(0, 255, 0));\n}\n",
    )
    .unwrap();
    let app = editor_over(blank()).insert_resource(Scripts::at(&dir));
    let out = drive(
        app,
        vec![
            Step::Menu("file-scripts"),
            Step::Wait,
            Step::Click("sc-run"),
            Step::Until(ran),
            Step::Wait,
            Step::Wait,
        ],
    );
    assert_eq!(out.tabs.len(), 2, "{:?}", out.tabs);
    assert_eq!(out.front, Some(1));
    assert_eq!((out.doc.width(), out.doc.height()), (4, 4));
    assert_eq!(pixel(&out.doc, 1, 1), Rgba::opaque(0, 255, 0));
    let _ = std::fs::remove_dir_all(Path::new(&dir));
}

#[test]
fn an_extension_installs_after_asking_and_brings_its_palette() {
    let dir = folder("install");
    let pkg = dir.join("inks.pixl-extension");
    {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::fs::File::create(&pkg).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, text) in [
            (
                "inks/extension.json",
                r#"{"name": "Inks", "version": "2.0",
                    "commands": [{"id": "fill", "label": "Ink fill", "script": "fill.cdl"}],
                    "palettes": [{"name": "Ink", "file": "ink.gpl"}]}"#,
            ),
            ("inks/fill.cdl", FILL),
            (
                "inks/ink.gpl",
                "GIMP Palette\nName: Ink\n#\n 17  34  51\tdark\n221 204 187\tlight\n",
            ),
        ] {
            w.start_file(name, opts).unwrap();
            w.write_all(text.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }
    let mut scripts = Scripts::at(&dir);
    scripts.offer(pkg);
    let app = editor_over(blank()).insert_resource(scripts);
    let out = drive(
        app,
        vec![
            Step::Wait,
            Step::Wait,
            Step::Click("sc-install-yes"),
            Step::Wait,
            Step::Ctrl("p", true),
            Step::Wait,
            Step::Text("cp_query", "ink palette"),
            Step::Wait,
            Step::KeyDown(NamedKey::Enter),
            Step::Wait,
        ],
    );
    assert!(dir.join("extensions/inks/fill.cdl").is_file());
    let colors = out.doc.palette().colors().to_vec();
    assert_eq!(
        colors,
        [Rgba::opaque(17, 34, 51), Rgba::opaque(221, 204, 187)]
    );

    // Removing it takes its folder away.
    let app = editor_over(blank()).insert_resource(Scripts::at(&dir));
    let out = drive(
        app,
        vec![
            Step::Menu("file-scripts"),
            Step::Wait,
            Step::Wait,
            Step::Click("sc-item-0"),
            Step::Wait,
            Step::Click("sc-remove"),
            Step::Wait,
        ],
    );
    assert_eq!(out.message.as_deref(), Some("Removed Inks"));
    assert!(!dir.join("extensions/inks").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stop_lets_go_of_a_script_that_never_calls_into_pixl() {
    let dir = folder("stop");
    // A loop with no call into pixl cannot see Stop; it ends by itself well
    // after the test.
    std::fs::write(
        dir.join("scripts/spin.cdl"),
        "import \"pixl\" as app;\nfn main() {\n    app::document().layers()[0].cel(0).set(0, 0, app::rgb(9, 9, 9));\n    let i = 0;\n    while i < 2000000000 { i = i + 1; }\n}\n",
    )
    .unwrap();
    let app = editor_over(blank()).insert_resource(Scripts::at(&dir));
    let out = drive(
        app,
        vec![
            Step::Menu("file-scripts"),
            Step::Wait,
            Step::Click("sc-run"),
            Step::Wait,
            Step::Wait,
            Step::Click("sc-stop"),
            Step::Until(|e| {
                e.message
                    .as_deref()
                    .is_some_and(|m| m.contains("did not stop"))
            }),
            Step::Wait,
        ],
    );
    assert_eq!(
        out.message.as_deref(),
        Some("spin did not stop; it runs on in the background and changes nothing")
    );
    assert_eq!(pixel(&out.doc, 0, 0), Rgba::TRANSPARENT);
    let _ = std::fs::remove_dir_all(dir);
}
