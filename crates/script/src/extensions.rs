//! The scripts folder and extensions.
//!
//! The scripts folder holds `.cdl` files the user runs by hand. An extension
//! is a folder with an `extension.json` manifest naming the commands it adds
//! (each a script with its parameters) and the palettes it brings. A `.zip`
//! (or `.pixl-extension`) holding that folder installs by unpacking into
//! the extensions folder.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The manifest's file name.
pub const MANIFEST: &str = "extension.json";

/// A script in the scripts folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptFile {
    /// Its path under the folder, without `.cdl`: `outline` or
    /// `export/each-layer`.
    pub name: String,
    pub path: PathBuf,
}

/// The `.cdl` files at the top of `dir` and one folder down, by name.
/// A folder that holds an extension manifest is an extension's, not
/// scripts, and is left out.
pub fn list_scripts(dir: &Path) -> Vec<ScriptFile> {
    fn scan(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<ScriptFile>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if depth == 0 && !path.join(MANIFEST).is_file() {
                    scan(&path, &format!("{prefix}{name}/"), depth + 1, out);
                }
            } else if let Some(stem) = name.strip_suffix(".cdl") {
                out.push(ScriptFile {
                    name: format!("{prefix}{stem}"),
                    path,
                });
            }
        }
    }
    let mut out = Vec::new();
    scan(dir, "", 0, &mut out);
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// What an extension's `extension.json` says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default)]
    pub palettes: Vec<PaletteFile>,
}

/// A command an extension adds: a script to run, with parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// Unique within the extension.
    pub id: String,
    /// What the command palette and the Scripts dialog show.
    pub label: String,
    /// The script, relative to the extension's folder.
    pub script: String,
    /// What the script reads from `app::params()`.
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

/// A palette an extension brings: a GPL, HEX, PAL, ACT or ASE palette file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteFile {
    pub name: String,
    /// The file, relative to the extension's folder.
    pub file: String,
}

/// An installed extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extension {
    /// Its folder in the extensions folder.
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Extension {
    /// Reads the extension in `dir`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(MANIFEST);
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let manifest = parse_manifest(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            manifest,
        })
    }

    /// The script file of `command`.
    pub fn script_path(&self, command: &Command) -> PathBuf {
        self.dir.join(&command.script)
    }

    /// The file of `palette`.
    pub fn palette_path(&self, palette: &PaletteFile) -> PathBuf {
        self.dir.join(&palette.file)
    }
}

/// Reads and checks a manifest: it names itself, its commands have ids,
/// labels and scripts, and every file it names stays inside its folder.
pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let m: Manifest = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if m.name.trim().is_empty() {
        return Err("the manifest has no name".into());
    }
    let mut ids = Vec::new();
    for c in &m.commands {
        if c.id.trim().is_empty() || c.label.trim().is_empty() {
            return Err("every command needs an id and a label".into());
        }
        if ids.contains(&&c.id) {
            return Err(format!("two commands have the id \"{}\"", c.id));
        }
        ids.push(&c.id);
        inside(&c.script).map_err(|e| format!("command \"{}\": {e}", c.id))?;
    }
    for p in &m.palettes {
        inside(&p.file).map_err(|e| format!("palette \"{}\": {e}", p.name))?;
    }
    Ok(m)
}

/// Checks that a path from a manifest names a file inside its folder.
fn inside(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if path.is_empty()
        || p.components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "\"{path}\" is not a path inside the extension's folder"
        ));
    }
    Ok(())
}

/// The extensions in `dir`, by name, and what could not be read.
pub fn load_extensions(dir: &Path) -> (Vec<Extension>, Vec<String>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join(MANIFEST).is_file() {
                match Extension::load(&path) {
                    Ok(e) => found.push(e),
                    Err(e) => errors.push(e),
                }
            }
        }
    }
    found.sort_by_key(|e| e.manifest.name.to_lowercase());
    (found, errors)
}

/// What a zip holds as an extension: its manifest and the folder inside the
/// zip the extension's files sit in ("" for the zip's root).
pub fn read_package(zip: &Path) -> Result<(Manifest, String), String> {
    let file = fs::File::open(zip).map_err(|e| format!("could not open {}: {e}", zip.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("{} is not a zip file: {e}", zip.display()))?;
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    let root = names
        .iter()
        .filter_map(|n| n.strip_suffix(MANIFEST))
        .filter(|prefix| prefix.is_empty() || prefix.matches('/').count() == 1)
        .min_by_key(|prefix| prefix.len())
        .map(str::to_string)
        .ok_or_else(|| format!("{} holds no {MANIFEST}", zip.display()))?;
    let mut text = String::new();
    archive
        .by_name(&format!("{root}{MANIFEST}"))
        .map_err(|e| e.to_string())?
        .read_to_string(&mut text)
        .map_err(|e| format!("could not read {MANIFEST}: {e}"))?;
    Ok((parse_manifest(&text)?, root))
}

/// A folder name for an extension called `name`: lowercase letters, digits
/// and dashes.
pub fn folder_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "extension".into()
    } else {
        out
    }
}

/// Unpacks the extension in `zip` into its own folder of `dir`, replacing
/// an older copy of it. Entries that would land outside that folder are
/// refused.
pub fn install(zip: &Path, dir: &Path) -> Result<Extension, String> {
    let (manifest, root) = read_package(zip)?;
    let target = dir.join(folder_name(&manifest.name));
    let staging = dir.join(format!(".{}.installing", folder_name(&manifest.name)));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)
        .map_err(|e| format!("could not write {}: {e}", staging.display()))?;
    let file = fs::File::open(zip).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let unpack = (|| -> Result<(), String> {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let Some(rel) = entry.enclosed_name() else {
                return Err(format!("{} names a file outside its folder", zip.display()));
            };
            let Ok(rel) = rel.strip_prefix(&root) else {
                continue;
            };
            let out = staging.join(rel);
            if entry.is_dir() {
                fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            fs::write(&out, bytes)
                .map_err(|e| format!("could not write {}: {e}", out.display()))?;
        }
        Ok(())
    })();
    if let Err(e) = unpack {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    let _ = fs::remove_dir_all(&target);
    fs::rename(&staging, &target)
        .map_err(|e| format!("could not write {}: {e}", target.display()))?;
    Extension::load(&target)
}

/// Removes an installed extension's folder.
pub fn uninstall(ext: &Extension) -> Result<(), String> {
    fs::remove_dir_all(&ext.dir).map_err(|e| format!("could not remove {}: {e}", ext.dir.display()))
}
