//! Runs scripts through the `pixl-cli` binary: the shipped examples on
//! fixture documents, parameters, errors and exports after a script.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pixl_document::native::{self, SaveOptions};
use pixl_document::{ColorMode, Document, NewLayer, Placement, Rgba};
use pixl_io::open::read_document;
use pixl_io::png::decode_png;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pixl-cli-script-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn example(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../script/scripts")
        .join(name)
        .display()
        .to_string()
}

fn cli(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pixl-cli"))
        .args(args)
        .current_dir(dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("APPDATA", dir.join("data"))
        .env("HOME", dir.join("home"))
        .output()
        .unwrap()
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "pixl-cli failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Two layers over three frames: a red square on "Body" and a blue pixel
/// on "Eye", saved as `hero.pixl`.
fn hero(dir: &Path) -> PathBuf {
    let mut doc = Document::new(8, 8, ColorMode::Rgba).unwrap();
    doc.insert_frame(1, 100).unwrap();
    doc.insert_frame(2, 100).unwrap();
    let body = doc.layers()[0].id;
    doc.update_layer(body, |p| p.name = "Body".into()).unwrap();
    let eye = doc
        .add_layer(NewLayer::Raster, "Eye", Placement::default())
        .unwrap();
    for f in 0..3 {
        let t = doc.target(body, f).unwrap();
        for y in 2..6 {
            for x in 2..6 {
                t.raster.set_raw(x, y, Rgba::opaque(200, 0, 0).to_u32());
            }
        }
    }
    let t = doc.target(eye, 0).unwrap();
    t.raster.set_raw(3, 3, Rgba::opaque(0, 0, 255).to_u32());
    let path = dir.join("hero.pixl");
    native::save_file(&doc, &path, SaveOptions::default()).unwrap();
    path
}

#[test]
fn palette_from_image_sets_the_palette_and_saves() {
    let dir = folder("palette");
    let hero = hero(&dir);
    let script = dir.join("then-save.cdl");
    std::fs::write(
        &script,
        format!(
            "import \"pixl\" as app;\nimport \"{}\" as example;\nfn main() {{ example::main(); app::document().save_as(\"out.pixl\"); }}\n",
            example("palette-from-image.cdl").replace('\\', "/")
        ),
    )
    .unwrap();
    let out = ok(&cli(
        &dir,
        &[
            hero.to_str().unwrap(),
            "--script",
            script.to_str().unwrap(),
            "--script-param",
            "max=4",
        ],
    ));
    assert_eq!(out, "the palette holds 2 colors\n");
    let saved = read_document(&dir.join("out.pixl")).unwrap().doc;
    let mut colors: Vec<Rgba> = saved.palette().colors().to_vec();
    colors.sort_by_key(|c| (c.r, c.g, c.b));
    assert_eq!(colors, [Rgba::opaque(0, 0, 255), Rgba::opaque(200, 0, 0)]);
}

#[test]
fn outline_all_frames_outlines_every_frame_before_an_export() {
    let dir = folder("outline");
    let hero = hero(&dir);
    // The script outlines the active layer: the top one, "Eye", has a cel on
    // frame 0 only.
    let out = ok(&cli(
        &dir,
        &[
            hero.to_str().unwrap(),
            "--script",
            &example("outline-all-frames.cdl"),
            "--script-param",
            "color=#00ff00",
            "--layer",
            "Eye",
            "--save-as",
            "eye{frame}.png",
        ],
    ));
    assert_eq!(out, "outlined 1 frames of Eye\n");
    let png = decode_png(&std::fs::read(dir.join("eye0.png")).unwrap()).unwrap();
    let px = png.to_rgba();
    assert_eq!(px[3 * 8 + 2], Rgba::opaque(0, 255, 0), "left of the eye");
    assert_eq!(px[3 * 8 + 3], Rgba::opaque(0, 0, 255), "the eye");
    assert_eq!(px[0], Rgba::TRANSPARENT);
    // The file itself is unchanged: nothing is saved unless the script saves.
    let back = read_document(&hero).unwrap().doc;
    let eye = back.layers()[1].id;
    let cel = back.cel_raster(eye, 0).unwrap().unwrap();
    assert_eq!(Rgba::from_u32(cel.get_raw(2, 3)), Rgba::TRANSPARENT);
}

#[test]
fn export_each_layer_writes_a_png_per_layer() {
    let dir = folder("each");
    let hero = hero(&dir);
    let out = ok(&cli(
        &dir,
        &[
            hero.to_str().unwrap(),
            "--script",
            &example("export-each-layer.cdl"),
            "--script-param",
            "scale=2",
        ],
    ));
    assert_eq!(out, "hero-Body.png\nhero-Eye.png\n");
    let body = decode_png(&std::fs::read(dir.join("hero-Body.png")).unwrap()).unwrap();
    assert_eq!(body.size(), (16, 16));
    let px = body.to_rgba();
    assert_eq!(px[5 * 16 + 5], Rgba::opaque(200, 0, 0));
    let eye = decode_png(&std::fs::read(dir.join("hero-Eye.png")).unwrap()).unwrap();
    assert_eq!(eye.to_rgba()[6 * 16 + 6], Rgba::opaque(0, 0, 255));
    assert_eq!(
        eye.to_rgba()[5 * 16 + 5],
        Rgba::TRANSPARENT,
        "no body on the eye's PNG"
    );
}

#[test]
fn a_script_without_a_document_makes_its_own() {
    let dir = folder("new");
    let script = dir.join("make.cdl");
    std::fs::write(
        &script,
        r##"import "pixl" as app;
fn main() {
    let d = app::new_document(4, 4);
    d.layers()[0].cel(0).rect(app::Rect { x: 0, y: 0, w: 4, h: 4 }, app::hex_color("#123"), true);
    d.save_as("made");
    print(app::documents().len(), d.layers()[0].cel(0).get(3, 3).hex());
}"##,
    )
    .unwrap();
    let out = ok(&cli(&dir, &["--script", script.to_str().unwrap()]));
    assert_eq!(out, "1\n#112233ff\n");
    assert!(dir.join("made.pixl").is_file());
}

#[test]
fn script_failures_exit_with_one_and_name_the_line() {
    let dir = folder("fail");
    let hero = hero(&dir);
    let script = dir.join("bad.cdl");
    std::fs::write(
        &script,
        "import \"pixl\" as app;\nfn main() {\n    print(\"start\");\n    let zero = 0;\n    print(app::document().width() / zero);\n}\n",
    )
    .unwrap();
    let out = cli(
        &dir,
        &[hero.to_str().unwrap(), "--script", script.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "start\n");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("bad.cdl:5:"), "{err}");
    let lua = cli(&dir, &[hero.to_str().unwrap(), "--script", "x.lua"]);
    assert_eq!(lua.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&lua.stderr).contains("Lua"));
    let alert = dir.join("alert.cdl");
    std::fs::write(
        &alert,
        "import \"pixl\" as app;\nfn main() { app::alert(\"look here\"); }\n",
    )
    .unwrap();
    let out = cli(&dir, &["--script", alert.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("look here"));
}
