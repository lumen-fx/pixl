//! The scripting API, exercised by scripts.

use std::path::{Path, PathBuf};

use pixl_document::{ColorMode, Document, Rgba};
use pixl_script::{Library, Outcome, ScriptDoc, Session, run};

fn lib() -> Library {
    let root = std::env::temp_dir().join(format!("pixl-script-api-{}", std::process::id()));
    Library::at(&root).expect("the library writes")
}

fn session(w: u32, h: u32) -> Session {
    let mut s = Session::default();
    s.add(ScriptDoc::new(
        Some(7),
        Document::new(w, h, ColorMode::Rgba).unwrap(),
    ));
    s
}

fn go(source: &str, s: Session) -> Outcome {
    run(source, Path::new("test.cdl"), s, &lib())
}

fn ok(source: &str, s: Session) -> Outcome {
    let out = go(source, s);
    assert!(
        out.error.is_none(),
        "the script failed: {:?}\n{}",
        out.error,
        out.output
    );
    out
}

#[test]
fn a_script_reads_the_document_and_prints() {
    let out = ok(
        r#"import "pixl" as app;
fn main() {
    let d = app::document();
    print(d.width(), d.height(), d.mode(), d.layers().len(), d.frame_count());
    print(d.layers()[0].name());
}"#,
        session(32, 16),
    );
    assert_eq!(out.output, "32\n16\nrgba\n1\n1\nLayer 1\n");
    assert!(!out.session.changed());
}

#[test]
fn pixels_written_by_a_script_land_in_the_document() {
    let out = ok(
        r#"import "pixl" as app;
fn main() {
    let cel = app::document().layers()[0].cel(0);
    cel.set(1, 2, app::rgb(255, 0, 0));
    cel.line(0, 0, 3, 0, app::rgb(0, 0, 255));
    print(cel.get(1, 2).hex(), cel.get(2, 0).hex(), cel.get(5, 5).a);
}"#,
        session(8, 8),
    );
    assert_eq!(out.output, "#ff0000ff\n#0000ffff\n0\n");
    let d = &out.session.docs[0];
    assert!(d.changed);
    let layer = d.doc.layers()[0].id;
    let raster = d.doc.cel_raster(layer, 0).unwrap().unwrap();
    assert_eq!(
        Rgba::from_u32(raster.get_raw(1, 2)),
        Rgba::opaque(255, 0, 0)
    );
    assert_eq!(
        Rgba::from_u32(raster.get_raw(3, 0)),
        Rgba::opaque(0, 0, 255)
    );
}

#[test]
fn layers_frames_and_tags_change_through_the_api() {
    let out = ok(
        r#"import "pixl" as app;
fn main() {
    let d = app::document();
    let top = d.add_layer("Ink");
    top.set_opacity(128);
    top.set_blend("multiply");
    d.add_frame(1);
    d.add_frame(2);
    d.set_duration(2, 250);
    d.add_tag("walk", 0, 2);
    let g = d.add_group("Group");
    top.move_into(g, 0);
    print(d.all_layers().map(fn(l) { return l.name(); }).join(","));
    print(top.parent().name(), top.opacity(), top.blend(), d.duration(2));
    print(d.tag_frames("walk").len());
}"#,
        session(8, 8),
    );
    assert_eq!(
        out.output,
        "Layer 1,Group,Ink\nGroup\n128\nmultiply\n250\n3\n"
    );
    let d = &out.session.docs[0].doc;
    assert_eq!(d.frame_count(), 3);
    assert_eq!(d.tags()[0].name, "walk");
}

#[test]
fn filters_keep_to_the_selection() {
    let out = ok(
        r#"import "pixl" as app;
fn main() {
    let d = app::document();
    let cel = d.layers()[0].cel(0);
    cel.fill(app::rgb(10, 20, 30));
    d.select(app::Rect { x: 0, y: 0, w: 2, h: 2 });
    cel.invert();
    print(cel.get(0, 0).hex(), cel.get(3, 3).hex(), d.selection().w);
}"#,
        session(4, 4),
    );
    assert_eq!(out.output, "#f5ebe1ff\n#0a141eff\n2\n");
}

#[test]
fn an_outline_goes_around_the_art() {
    let out = ok(
        r#"import "pixl" as app;
fn main() {
    let cel = app::document().layers()[0].cel(0);
    cel.set(2, 2, app::rgb(255, 255, 255));
    cel.outline(app::rgb(0, 0, 0), true);
    print(cel.get(1, 2).hex(), cel.get(2, 2).hex(), cel.get(1, 1).a);
}"#,
        session(5, 5),
    );
    assert_eq!(out.output, "#000000ff\n#ffffffff\n0\n");
}

#[test]
fn the_palette_reads_and_writes() {
    let out = ok(
        r#"import "pixl" as app;
fn main() {
    let d = app::document();
    let cel = d.layers()[0].cel(0);
    cel.fill(app::rgb(200, 0, 0));
    cel.set(0, 0, app::rgb(0, 200, 0));
    let found = d.colors_in_image(8);
    d.set_palette(found);
    print(d.palette().len());
    print(d.palette().map(fn(c) { return c.hex(); }).sort_by(fn(a, b) { return a < b; }).join(" "));
}"#,
        session(4, 4),
    );
    assert_eq!(out.output, "2\n#00c800ff #c80000ff\n");
}

#[test]
fn an_indexed_document_takes_colors_and_indexes() {
    let out = ok(
        r#"import "pixl" as app;
fn main() {
    let d = app::new_indexed_document(4, 4);
    let cel = d.layers()[0].cel(0);
    cel.set(0, 0, app::rgb(255, 255, 255));
    cel.set_index(1, 0, 1);
    print(cel.index(0, 0), cel.index(1, 0), cel.get(1, 0).hex(), app::document().mode());
}"#,
        Session::default(),
    );
    assert_eq!(out.output, "2\n1\n#000000ff\nindexed\n");
    assert!(out.session.docs[0].created);
}

#[test]
fn an_api_error_names_the_call() {
    let out = go(
        "import \"pixl\" as app;\nfn main() {\n    let d = app::document();\n    d.layers()[0].cel(9).set(0, 0, app::rgb(1, 2, 3));\n}\n",
        session(4, 4),
    );
    let e = out.error.expect("frame 9 does not exist");
    assert_eq!(e.code, "host_fn_error");
    assert_eq!(e.message, "set_pixel: there is no frame 9");
    assert_eq!(e.file, "test.cdl");
}

#[test]
fn a_runtime_error_in_the_script_names_its_line() {
    let out = go(
        "fn main() {\n    let zero = 0;\n    print(1 / zero);\n}\n",
        session(4, 4),
    );
    let e = out.error.expect("division by zero");
    assert_eq!(e.code, "division_by_zero");
    assert_eq!(e.line, Some(3));
}

#[test]
fn a_compile_error_comes_back_without_running() {
    let out = go(
        "import \"pixl\" as app;\nfn main() {\n    app::document().nope();\n}\n",
        session(4, 4),
    );
    let e = out.error.expect("nope is not a method");
    assert_eq!(e.line, Some(3), "{e:?}");
    assert!(!out.session.changed());
}

#[test]
fn a_thrown_error_and_a_caught_one() {
    let out = go(
        r#"import "pixl" as app;
fn main() {
    try {
        app::document().layer("missing");
    } catch "no_layer" {
        print("caught");
    }
    throw("done here");
}"#,
        session(4, 4),
    );
    assert_eq!(out.output, "caught\n");
    assert_eq!(out.error.unwrap().code, "done here");
}

#[test]
fn a_cancelled_run_stops_at_its_next_call() {
    let s = session(4, 4);
    s.cancel.cancel();
    let out = go(
        r#"import "pixl" as app;
fn main() { print("before"); app::document().width(); print("after"); }"#,
        s,
    );
    assert_eq!(out.output, "before\n");
    assert_eq!(out.error.unwrap().code, "cancelled");
}

#[test]
fn params_reach_the_script() {
    let mut s = session(4, 4);
    s.params.insert("color".into(), "red".into());
    let out = ok(
        r#"import "pixl" as app;
fn main() { print(app::param("color", "blue"), app::param("size", "2"), app::has_ui()); }"#,
        s,
    );
    assert_eq!(out.output, "red\n2\nfalse\n");
}

#[test]
fn canvas_transforms_and_saving() {
    let dir = std::env::temp_dir().join(format!("pixl-script-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = session(4, 2);
    s.base = dir.clone();
    let out = ok(
        r#"import "pixl" as app;
fn main() {
    let d = app::document();
    d.layers()[0].cel(0).set(0, 0, app::rgb(9, 9, 9));
    d.rotate(1);
    d.scale(4, 8, "nearest");
    d.save_as("turned");
    d.export_png("turned.png", 0, 2);
    d.export_gif("turned.gif", 1);
    d.export_sheet("sheet.png", "sheet.json", 0);
    print(d.width(), d.height(), d.name());
}"#,
        s,
    );
    assert_eq!(out.output, "4\n8\nturned.pixl\n");
    let d = &out.session.docs[0];
    assert!(d.saved);
    assert_eq!(d.path.as_deref(), Some(dir.join("turned.pixl").as_path()));
    for f in [
        "turned.pixl",
        "turned.png",
        "turned.gif",
        "sheet.png",
        "sheet.json",
    ] {
        assert!(dir.join(f).is_file(), "{f} was written");
    }
    let back = pixl_io::open::read_document(&dir.join("turned.pixl")).unwrap();
    assert_eq!((back.doc.width(), back.doc.height()), (4, 8));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_host_block_matches_the_function_table() {
    let api = pixl_script::api_source();
    for d in pixl_script::host::DEFS {
        assert!(
            api.contains(&format!("{}(", d.name)),
            "{} is declared",
            d.name
        );
    }
    let path = PathBuf::from("unused.cdl");
    let out = run(
        "import \"pixl\";\nfn main() {}\n",
        &path,
        Session::default(),
        &lib(),
    );
    assert!(out.error.is_none(), "{:?}", out.error);
}
