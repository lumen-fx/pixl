//! Runs the `pixl-cli` binary on fixture documents and compares what it
//! writes with what the editor's exports write for the same settings: the
//! Export sprite sheet dialog, PNG and GIF export, and the export presets.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pixl_document::native::{self, SaveOptions};
use pixl_document::{ColorMode, Document, LoopDirection, NewLayer, Placement, Rect, Rgba, Tag};
use pixl_io::export::{self, Data, FILE, Format, Frames, Preset, PresetFile, Sheet};
use pixl_io::gif::{GifOptions, export_gif};
use pixl_io::open::read_document;
use pixl_io::png::{encode_png_rgba, export_frame_png};
use pixl_io::sheet::{JsonFrames, SheetLayout, SheetOptions, sprite_sheet};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../io/tests/data/aseprite")
        .join(name)
}

/// A fresh folder for one test.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pixl-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cli(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pixl-cli"))
        .args(args)
        .current_dir(dir)
        // No preset file of the machine running the tests leaks in.
        .env("XDG_DATA_HOME", dir.join("no-data"))
        .env("APPDATA", dir.join("no-data"))
        .env("HOME", dir.join("no-home"))
        .output()
        .unwrap()
}

fn ok(out: &Output) {
    assert!(
        out.status.success(),
        "pixl-cli failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn read(path: impl AsRef<Path>) -> Vec<u8> {
    std::fs::read(path.as_ref()).unwrap_or_else(|e| panic!("{}: {e}", path.as_ref().display()))
}

/// A document of two layers, five frames and two tags, saved as `.pixl`.
fn hero(dir: &Path) -> (PathBuf, Document) {
    let mut doc = Document::new(8, 6, ColorMode::Rgba).unwrap();
    for _ in 0..4 {
        doc.insert_frame(0, 120).unwrap();
    }
    let top = doc
        .add_layer(NewLayer::Raster, "Top".to_string(), Placement::default())
        .unwrap();
    let base = doc.layers()[0].id;
    for f in 0..5 {
        doc.target(base, f).unwrap().raster.fill_rect_raw(
            &Rect::new(f as i32, 1, 2, 3),
            Rgba::new(200, 40, 40, 255).to_u32(),
        );
        doc.target(top, f).unwrap().raster.fill_rect_raw(
            &Rect::new(1, f as i32, 3, 1),
            Rgba::new(20, 90, 220, 255).to_u32(),
        );
    }
    for (name, from, to) in [("idle", 0, 1), ("walk", 2, 4)] {
        doc.tags_mut().push(Tag {
            name: name.to_string(),
            from,
            to,
            direction: LoopDirection::Forward,
            repeat: 0,
            color: Rgba::BLACK,
        });
    }
    let path = dir.join("hero.pixl");
    native::save_file(&doc, &path, SaveOptions::default()).unwrap();
    let doc = read_document(&path).unwrap().doc;
    (path, doc)
}

#[test]
fn a_sheet_matches_the_export_sprite_sheet_dialog() {
    let dir = folder("sheet");
    let input = fixture("layers_and_tags.aseprite");
    let out = cli(
        &dir,
        &[
            "-b",
            input.to_str().unwrap(),
            "--sheet",
            "sheet.png",
            "--data",
            "sheet.json",
            "--sheet-type",
            "rows",
            "--sheet-columns",
            "2",
            "--shape-padding",
            "1",
            "--border-padding",
            "2",
            "--trim",
            "--format",
            "json-array",
        ],
    );
    ok(&out);
    // What the dialog builds for the same fields: the document's title
    // names the frames and the image's file name goes in meta.image.
    let doc = read_document(&input).unwrap().doc;
    let options = SheetOptions {
        layout: SheetLayout::Grid { columns: 2 },
        frames: Some(0..doc.frame_count()),
        padding: 1,
        border: 2,
        trim: true,
        by_layer: false,
        scale: 1,
        title: "layers_and_tags".to_string(),
        image_name: "sheet.png".to_string(),
        json_frames: JsonFrames::Array,
    };
    let editor = sprite_sheet(&doc, &options).unwrap();
    assert_eq!(read(dir.join("sheet.png")), editor.to_png().unwrap());
    assert_eq!(read(dir.join("sheet.json")), editor.json.as_bytes());
    let json: serde_json::Value = serde_json::from_slice(&read(dir.join("sheet.json"))).unwrap();
    assert!(json["frames"].is_array());
    assert_eq!(json["meta"]["image"], "sheet.png");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn split_layers_put_every_layer_on_one_sheet() {
    let dir = folder("split-sheet");
    let (input, doc) = hero(&dir);
    let out = cli(
        &dir,
        &[
            "--split-layers",
            input.to_str().unwrap(),
            "--sheet",
            "layers.png",
            "--data",
            "layers.json",
            "--tag",
            "walk",
            "--sheet-type",
            "columns",
            "--sheet-rows",
            "2",
        ],
    );
    ok(&out);
    let editor = sprite_sheet(
        &doc,
        &SheetOptions {
            layout: SheetLayout::Columns { rows: 2 },
            frames: Some(2..5),
            by_layer: true,
            title: "hero".to_string(),
            image_name: "layers.png".to_string(),
            ..SheetOptions::default()
        },
    )
    .unwrap();
    assert_eq!(editor.cells.len(), 6);
    assert_eq!(read(dir.join("layers.png")), editor.to_png().unwrap());
    assert_eq!(read(dir.join("layers.json")), editor.json.as_bytes());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn save_as_png_writes_the_frames_png_export_writes() {
    let dir = folder("png");
    let (input, doc) = hero(&dir);
    ok(&cli(
        &dir,
        &[
            input.to_str().unwrap(),
            "--save-as",
            "out/hero.png",
            "--scale",
            "3",
        ],
    ));
    // Aseprite's numbering: the frames from 1, beside the name.
    for f in 0..5 {
        assert_eq!(
            read(dir.join(format!("out/hero{}.png", f + 1))),
            export_frame_png(&doc, f, 3).unwrap()
        );
    }
    ok(&cli(
        &dir,
        &[
            input.to_str().unwrap(),
            "--save-as",
            "tokens/{title}-{tag}-{frame001}.png",
        ],
    ));
    assert_eq!(
        read(dir.join("tokens/hero-walk-004.png")),
        export_frame_png(&doc, 3, 1).unwrap()
    );
    assert!(dir.join("tokens/hero-idle-001.png").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn save_as_gif_per_tag_matches_gif_export() {
    let dir = folder("gif");
    let (input, doc) = hero(&dir);
    ok(&cli(
        &dir,
        &[
            input.to_str().unwrap(),
            "--split-tags",
            "--save-as",
            "anim.gif",
            "--scale",
            "2",
        ],
    ));
    for (tag, frames) in [("idle", 0..2), ("walk", 2..5)] {
        let editor = export_gif(
            &doc,
            &GifOptions {
                frames: Some(frames),
                scale: 2,
                repeat: None,
            },
        )
        .unwrap();
        assert_eq!(read(dir.join(format!("anim #{tag}.gif"))), editor.bytes);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_named_layer_exports_alone() {
    let dir = folder("layer");
    let (input, doc) = hero(&dir);
    ok(&cli(
        &dir,
        &[
            input.to_str().unwrap(),
            "--layer",
            "Top",
            "--oneframe",
            "--save-as",
            "top.png",
        ],
    ));
    let top = doc.layers()[1].id;
    let alone = pixl_io::sheet::only_layer(&doc, top);
    assert_eq!(
        read(dir.join("top.png")),
        export_frame_png(&alone, 0, 1).unwrap()
    );
    ok(&cli(
        &dir,
        &[
            input.to_str().unwrap(),
            "--ignore-layer",
            "Top",
            "--frame-range",
            "1,1",
            "--save-as",
            "base.png",
        ],
    ));
    assert_eq!(read(dir.join("base.png")), {
        let base = doc.layers()[0].id;
        export_frame_png(&pixl_io::sheet::only_layer(&doc, base), 1, 1).unwrap()
    });
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn presets_write_what_the_editor_s_export_all_writes() {
    let dir = folder("presets");
    let (input, doc) = hero(&dir);
    let presets = vec![
        Preset {
            name: "Atlas".to_string(),
            format: Format::Sheet,
            sheet: Sheet::Packed,
            trim: true,
            path: "build/{name}-atlas".to_string(),
            ..Preset::default()
        },
        Preset {
            name: "Tags".to_string(),
            format: Format::Gif,
            frames: Frames::EachTag,
            scale: 2,
            path: "build/{name}-{tag}".to_string(),
            in_all: false,
            ..Preset::default()
        },
        Preset {
            name: "Strip".to_string(),
            format: Format::Sheet,
            data: Data::None,
            path: "build/{name}-strip".to_string(),
            ..Preset::default()
        },
    ];
    let file = dir.join(FILE);
    std::fs::write(
        &file,
        serde_json::to_vec(&PresetFile {
            presets: presets.clone(),
        })
        .unwrap(),
    )
    .unwrap();
    let names = cli(
        &dir,
        &["--presets", file.to_str().unwrap(), "--list-presets"],
    );
    ok(&names);
    assert_eq!(
        String::from_utf8_lossy(&names.stdout),
        "Atlas\nTags\nStrip\n"
    );

    ok(&cli(
        &dir,
        &[
            input.to_str().unwrap(),
            "--presets",
            file.to_str().unwrap(),
            "--all-presets",
            "--preset",
            "Tags",
        ],
    ));
    // The editor's Export all, plus the one preset it leaves out.
    let editor_dir = dir.join("editor");
    std::fs::create_dir_all(&editor_dir).unwrap();
    let results = export::run_presets(&doc, "hero", 0, &presets, &editor_dir);
    assert!(results.iter().all(Result::is_ok));
    let mut written = 0;
    for entry in std::fs::read_dir(editor_dir.join("build")).unwrap() {
        let name = entry.unwrap().file_name();
        assert_eq!(
            read(dir.join("build").join(&name)),
            read(editor_dir.join("build").join(&name)),
            "{name:?}"
        );
        written += 1;
    }
    assert_eq!(written, 5, "atlas png + json, two gifs, the strip");
    assert!(!dir.join("build/hero-strip.json").exists());

    let missing = cli(
        &dir,
        &[
            input.to_str().unwrap(),
            "--presets",
            file.to_str().unwrap(),
            "--preset",
            "Nope",
        ],
    );
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Atlas, Tags, Strip"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_new_user_s_starter_presets_run_without_a_file() {
    let dir = folder("starters");
    let (input, doc) = hero(&dir);
    ok(&cli(
        &dir,
        &[input.to_str().unwrap(), "--preset", "Sprite sheet"],
    ));
    let preset = &Preset::starters()[1];
    let jobs = export::plan(&doc, "hero", 0, preset, &dir).unwrap();
    let options = export::sheet_options(preset, &jobs[0], "hero");
    let editor = sprite_sheet(&doc, &options).unwrap();
    assert_eq!(read(dir.join("hero-sheet.png")), editor.to_png().unwrap());
    assert_eq!(read(dir.join("hero-sheet.json")), editor.json.as_bytes());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn png_and_gif_documents_export_too() {
    let dir = folder("inputs");
    let px: Vec<Rgba> = (0..16)
        .map(|i| Rgba::new(i * 15, 0, 255 - i * 15, 255))
        .collect();
    std::fs::write(dir.join("tile.png"), encode_png_rgba(4, 4, &px).unwrap()).unwrap();
    ok(&cli(
        &dir,
        &["tile.png", "--save-as", "big.png", "--scale", "2"],
    ));
    let doc = read_document(&dir.join("tile.png")).unwrap().doc;
    assert_eq!(
        read(dir.join("big.png")),
        export_frame_png(&doc, 0, 2).unwrap()
    );

    let (_, hero) = hero(&dir);
    std::fs::write(
        dir.join("hero.gif"),
        export_gif(&hero, &GifOptions::default()).unwrap().bytes,
    )
    .unwrap();
    ok(&cli(
        &dir,
        &[
            "hero.gif",
            "--sheet",
            "from-gif.png",
            "--sheet-type",
            "vertical",
        ],
    ));
    let gif_doc = read_document(&dir.join("hero.gif")).unwrap().doc;
    let editor = sprite_sheet(
        &gif_doc,
        &SheetOptions {
            layout: SheetLayout::Vertical,
            frames: Some(0..gif_doc.frame_count()),
            title: "hero".to_string(),
            image_name: "from-gif.png".to_string(),
            ..SheetOptions::default()
        },
    )
    .unwrap();
    assert_eq!(read(dir.join("from-gif.png")), editor.to_png().unwrap());
    assert!(!dir.join("from-gif.json").exists(), "no --data, no JSON");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lists_print_one_name_per_line() {
    let dir = folder("lists");
    let (input, _) = hero(&dir);
    let out = cli(
        &dir,
        &[input.to_str().unwrap(), "--list-tags", "--list-layers"],
    );
    ok(&out);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Layer 1\nTop\nidle\nwalk\n"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn failures_exit_with_codes_a_pipeline_can_tell_apart() {
    let dir = folder("failures");
    let (input, _) = hero(&dir);
    let input = input.to_str().unwrap();
    let code = |args: &[&str]| {
        let out = cli(&dir, args);
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(!err.contains('\x1b'), "no escape codes: {err:?}");
        (out.status.code(), err)
    };
    assert_eq!(code(&["--help"]).0, Some(0));
    let (c, err) = code(&[input, "--frob"]);
    assert_eq!(c, Some(2));
    assert!(err.contains("unknown option --frob"));
    assert_eq!(code(&[input, "--script", "x.lua"]).0, Some(2));
    assert_eq!(code(&[input]).0, Some(2));
    assert_eq!(code(&[input, "--save-as", "x.webp"]).0, Some(2));
    let (c, err) = code(&[input, "--tag", "fly", "--save-as", "x.png"]);
    assert_eq!(c, Some(1));
    assert!(err.contains("idle, walk"), "{err}");
    assert_eq!(
        code(&[input, "--layer", "Nope", "--save-as", "x.png"]).0,
        Some(1)
    );
    assert_eq!(
        code(&[input, "--frame-range", "0,9", "--save-as", "x.png"]).0,
        Some(1)
    );
    assert_eq!(code(&["missing.pixl", "--save-as", "x.png"]).0, Some(1));
    let _ = std::fs::remove_dir_all(dir);
}
