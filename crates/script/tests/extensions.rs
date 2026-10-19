//! The scripts folder, extension manifests and installing a zip.

use std::io::Write;
use std::path::{Path, PathBuf};

use pixl_script::extensions::{folder_name, install, parse_manifest, read_package, uninstall};
use pixl_script::{list_scripts, load_extensions};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pixl-ext-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const MANIFEST: &str = r#"{
    "name": "Outline Tools",
    "version": "1.0.0",
    "description": "Outlines",
    "commands": [
        { "id": "black", "label": "Outline in black", "script": "outline.cdl",
          "params": { "color": "000000" } }
    ],
    "palettes": [ { "name": "Ink", "file": "ink.gpl" } ]
}"#;

#[test]
fn the_scripts_folder_lists_scripts_one_folder_deep() {
    let dir = scratch("list");
    write(&dir.join("b.cdl"), "");
    write(&dir.join("A.cdl"), "");
    write(&dir.join("notes.txt"), "");
    write(&dir.join("export/each-layer.cdl"), "");
    write(&dir.join("export/deeper/no.cdl"), "");
    write(&dir.join("ext/extension.json"), MANIFEST);
    write(&dir.join("ext/outline.cdl"), "");
    let names: Vec<String> = list_scripts(&dir).into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["A", "b", "export/each-layer"]);
    assert!(list_scripts(&dir.join("missing")).is_empty());
}

#[test]
fn a_manifest_is_checked() {
    let m = parse_manifest(MANIFEST).unwrap();
    assert_eq!(m.commands[0].params["color"], "000000");
    assert!(parse_manifest(r#"{"name": ""}"#).is_err());
    let escape =
        r#"{"name": "x", "commands": [{"id": "a", "label": "A", "script": "../evil.cdl"}]}"#;
    assert!(parse_manifest(escape).unwrap_err().contains("inside"));
    let twice = r#"{"name": "x", "commands": [{"id": "a", "label": "A", "script": "a.cdl"},
        {"id": "a", "label": "B", "script": "b.cdl"}]}"#;
    assert!(parse_manifest(twice).unwrap_err().contains("two commands"));
}

#[test]
fn extensions_load_from_their_folders() {
    let dir = scratch("load");
    write(&dir.join("outline-tools/extension.json"), MANIFEST);
    write(&dir.join("broken/extension.json"), "{");
    write(&dir.join("not-one/readme.txt"), "");
    let (found, errors) = load_extensions(&dir);
    assert_eq!(found.len(), 1);
    assert_eq!(errors.len(), 1);
    let ext = &found[0];
    assert_eq!(
        ext.script_path(&ext.manifest.commands[0]),
        dir.join("outline-tools/outline.cdl")
    );
}

fn zip(path: &Path, files: &[(&str, &str)]) {
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in files {
        w.start_file(*name, opts).unwrap();
        w.write_all(text.as_bytes()).unwrap();
    }
    w.finish().unwrap();
}

#[test]
fn a_zip_installs_into_its_own_folder_and_replaces_an_older_copy() {
    let dir = scratch("install");
    let pkg = dir.join("outline.pixl-extension");
    zip(
        &pkg,
        &[
            ("outline/extension.json", MANIFEST),
            ("outline/outline.cdl", "fn main() {}"),
            ("outline/ink.gpl", "GIMP Palette\n0 0 0 black\n"),
        ],
    );
    let (m, root) = read_package(&pkg).unwrap();
    assert_eq!(
        (m.name.as_str(), root.as_str()),
        ("Outline Tools", "outline/")
    );
    let exts = dir.join("extensions");
    std::fs::create_dir_all(&exts).unwrap();
    write(&exts.join("outline-tools/stale.cdl"), "");
    let ext = install(&pkg, &exts).unwrap();
    assert_eq!(ext.dir, exts.join(folder_name("Outline Tools")));
    assert!(ext.dir.join("outline.cdl").is_file());
    assert!(ext.dir.join("ink.gpl").is_file());
    assert!(!ext.dir.join("stale.cdl").exists());
    uninstall(&ext).unwrap();
    assert!(!ext.dir.exists());
}

#[test]
fn a_zip_without_a_manifest_or_escaping_its_folder_is_refused() {
    let dir = scratch("refuse");
    let none = dir.join("none.zip");
    zip(&none, &[("a.cdl", "")]);
    assert!(install(&none, &dir).unwrap_err().contains("extension.json"));
    let evil = dir.join("evil.zip");
    zip(
        &evil,
        &[("extension.json", MANIFEST), ("../escape.cdl", "")],
    );
    assert!(install(&evil, &dir.join("into")).is_err());
    assert!(!dir.join("escape.cdl").exists());
}

#[test]
fn folder_names_are_plain() {
    assert_eq!(folder_name("Outline Tools!"), "outline-tools");
    assert_eq!(folder_name("..."), "extension");
}
