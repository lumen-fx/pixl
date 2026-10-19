//! Export presets: named export settings (format, scale, which frames and
//! layers, the sheet layout, and an output path pattern with tokens such as
//! `{name}`, `{tag}`, `{frame}` and `{layer}`), as `exports.json` keeps
//! them. The editor's presets, its Export all, its sprite sheet dialog and
//! the `pixl-cli` command line all write their files through this module.

use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use pixl_document::document::{Document, LayerId, LayerKind};
use pixl_document::native::write_atomic;
use serde::{Deserialize, Serialize};

use crate::gif::{GifOptions, export_gif};
use crate::png::export_frame_png;
use crate::sheet::{JsonFrames, SheetLayout, SheetOptions, SpriteSheet, only_layer, sprite_sheet};

/// The file the presets are kept in, in pixl's data folder.
pub const FILE: &str = "exports.json";

/// What a preset writes.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// One PNG per frame.
    #[default]
    Png,
    /// One animated GIF.
    Gif,
    /// One sprite sheet PNG with its Aseprite-shape JSON beside it.
    Sheet,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Png, Format::Gif, Format::Sheet];

    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG frames",
            Format::Gif => "GIF",
            Format::Sheet => "Sprite sheet",
        }
    }

    pub fn from_label(label: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.label() == label)
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Png | Format::Sheet => "png",
            Format::Gif => "gif",
        }
    }
}

/// Which frames a preset writes.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Frames {
    #[default]
    All,
    /// The frame the editor shows; the first frame on the command line.
    Current,
    /// Each tag on its own, `{tag}` naming it.
    EachTag,
    /// The frames of one tag.
    Tag(String),
    /// Frames `from` to `to`, both included, counted from 0.
    Range { from: usize, to: usize },
}

impl Frames {
    pub fn label(&self) -> &'static str {
        match self {
            Frames::All => "All frames",
            Frames::Current => "Current frame",
            Frames::EachTag => "Each tag",
            Frames::Tag(_) => "One tag",
            Frames::Range { .. } => "Frame range",
        }
    }
}

/// Which layers a preset writes.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Layers {
    /// The visible layers, merged as the canvas shows them.
    #[default]
    Visible,
    /// Each visible raster layer on its own, `{layer}` naming it.
    Each,
    /// Each visible raster layer on its own, all on one sprite sheet; other
    /// formats write a file per layer as `Each` does.
    Split,
}

impl Layers {
    pub fn label(self) -> &'static str {
        match self {
            Layers::Visible => "Visible, merged",
            Layers::Each => "Each layer",
            Layers::Split => "Each layer, one sheet",
        }
    }
}

/// How a sprite sheet lays out its frames.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Sheet {
    #[default]
    Horizontal,
    Vertical,
    /// Rows of `columns` frames.
    Grid,
    /// Columns of `rows` frames, filled column by column.
    Columns,
    /// As small an image as the frames fit in.
    Packed,
}

impl Sheet {
    /// The layouts the Export presets dialog offers.
    pub const ALL: [Sheet; 4] = [
        Sheet::Horizontal,
        Sheet::Vertical,
        Sheet::Grid,
        Sheet::Packed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sheet::Horizontal => "One row",
            Sheet::Vertical => "One column",
            Sheet::Grid => "Grid",
            Sheet::Columns => "Grid by columns",
            Sheet::Packed => "Packed",
        }
    }
}

/// Where a sprite sheet's JSON goes.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Data {
    /// Beside the image, with its name and a `.json` extension.
    #[default]
    Beside,
    /// Nowhere: the sheet image only.
    None,
    /// At a path pattern of its own, with the same tokens as the image's.
    At(String),
}

/// One named set of export settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub format: Format,
    /// Integer enlargement, nearest neighbor.
    pub scale: u32,
    pub frames: Frames,
    pub layers: Layers,
    /// Show only these layers, by name or by group path such as
    /// `body/arm`; empty shows every layer that is visible.
    pub only_layers: Vec<String>,
    /// Hide these layers, by name or group path.
    pub ignore_layers: Vec<String>,
    /// Show hidden layers too.
    pub all_layers: bool,
    pub sheet: Sheet,
    /// Columns of a grid sheet.
    pub columns: u32,
    /// Rows of a sheet filled column by column.
    pub rows: u32,
    /// Transparent pixels between sheet frames.
    pub padding: u32,
    /// Transparent pixels around the whole sheet.
    pub border: u32,
    /// Crop each sheet frame to its pixels.
    pub trim: bool,
    /// Where a sheet's JSON goes.
    pub data: Data,
    /// The shape of a sheet JSON's `frames`.
    pub json_frames: JsonFrames,
    /// Where the files go, relative to the document's folder. Tokens:
    /// `{name}` (or `{title}`) the document's name, `{tag}`, `{layer}`,
    /// `{scale}`, `{frame}` (numbered from 0, padded to the last frame's
    /// digits), `{frame1}` or `{frame001}` (numbered from the digits'
    /// value, padded to their count), and `{tagframe}` the same counted
    /// from the first frame written.
    pub path: String,
    /// Export all runs this preset.
    pub in_all: bool,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            name: "PNG frames".to_string(),
            format: Format::Png,
            scale: 1,
            frames: Frames::All,
            layers: Layers::Visible,
            only_layers: Vec::new(),
            ignore_layers: Vec::new(),
            all_layers: false,
            sheet: Sheet::Horizontal,
            columns: 8,
            rows: 8,
            padding: 0,
            border: 0,
            trim: false,
            data: Data::Beside,
            json_frames: JsonFrames::Hash,
            path: "{name}-{frame}.png".to_string(),
            in_all: true,
        }
    }
}

impl Preset {
    /// The presets a new user starts with.
    pub fn starters() -> Vec<Preset> {
        vec![
            Preset::default(),
            Preset {
                name: "Sprite sheet".to_string(),
                format: Format::Sheet,
                path: "{name}-sheet.png".to_string(),
                ..Preset::default()
            },
            Preset {
                name: "GIF per tag at 4x".to_string(),
                format: Format::Gif,
                scale: 4,
                frames: Frames::EachTag,
                path: "{name}-{tag}@{scale}x.gif".to_string(),
                in_all: false,
                ..Preset::default()
            },
        ]
    }
}

/// What `exports.json` holds.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PresetFile {
    pub presets: Vec<Preset>,
}

impl PresetFile {
    /// The presets in `exports.json` at `path`: the starters when there is
    /// no file yet or it lists none, as the editor shows a new user.
    pub fn read(path: &Path) -> Result<Vec<Preset>, String> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preset::starters()),
            Err(e) => return Err(format!("could not read {}: {e}", path.display())),
        };
        let file: PresetFile = serde_json::from_slice(&bytes)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        Ok(if file.presets.is_empty() {
            Preset::starters()
        } else {
            file.presets
        })
    }
}

/// pixl's folder of the user data directory, where the editor keeps
/// `exports.json`: `$XDG_DATA_HOME` (else `~/.local/share`) on Linux,
/// `~/Library/Application Support` on macOS, `%APPDATA%` on Windows, then
/// `lumen/pixl`. It is the folder the Lumen runtime gives the editor; the
/// editor's tests check the two agree.
pub fn user_dir() -> Option<PathBuf> {
    let env = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    let root: Option<PathBuf> = if cfg!(target_os = "windows") {
        env("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| Path::new(&h).join("Library").join("Application Support"))
    } else {
        env("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env("HOME").map(|h| Path::new(&h).join(".local").join("share")))
    };
    root.map(|r| r.join("lumen").join("pixl"))
}

/// One file an export writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub path: PathBuf,
    /// A sheet's JSON, when it writes one.
    pub data: Option<PathBuf>,
    pub frames: Range<usize>,
    /// The one layer it shows, or every visible one.
    pub layer: Option<LayerId>,
}

/// The value of each token in a path pattern.
#[derive(Clone, Debug, Default)]
struct Tokens<'a> {
    name: &'a str,
    tag: &'a str,
    /// The frame, the frame the job starts at, and the last frame of the
    /// document.
    frame: Option<(usize, usize, usize)>,
    layer: &'a str,
    scale: u32,
}

/// `text` safe as one path component.
fn component(text: &str) -> String {
    let out: String = text
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    out.trim().to_string()
}

/// A frame number for a token: `{frame}` pads to the last frame's digits;
/// `{frame1}`, `{frame001}` count from the digits' value, padded to their
/// count.
fn number(digits: &str, n: usize, last: usize) -> Option<String> {
    if digits.is_empty() {
        let width = last.to_string().len();
        return Some(format!("{n:0width$}"));
    }
    let start: usize = digits.parse().ok()?;
    let width = digits.len();
    Some(format!("{:0width$}", n + start))
}

/// Whether `pattern` names a frame, so each frame gets its own file.
pub fn names_frame(pattern: &str) -> bool {
    tokens(pattern).any(|t| t.starts_with("frame") || t.starts_with("tagframe"))
}

/// The tokens in `pattern`, without their braces.
pub fn tokens(pattern: &str) -> impl Iterator<Item = &str> {
    pattern
        .split('{')
        .skip(1)
        .filter_map(|s| s.split_once('}'))
        .map(|(t, _)| t)
}

fn fill(pattern: &str, t: &Tokens) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let token = &after[..close];
        let value = match token {
            "name" | "title" => Some(component(t.name)),
            "tag" => Some(component(t.tag)),
            "layer" => Some(component(t.layer)),
            "scale" => Some(t.scale.to_string()),
            _ => {
                let frame = |digits: &str, of_tag: bool| {
                    t.frame.map_or(Some(String::new()), |(f, start, last)| {
                        if of_tag {
                            number(digits, f - start, last - start)
                        } else {
                            number(digits, f, last)
                        }
                    })
                };
                if let Some(d) = token.strip_prefix("tagframe") {
                    frame(d, true)
                } else if let Some(d) = token.strip_prefix("frame") {
                    frame(d, false)
                } else {
                    None
                }
            }
        };
        match value {
            Some(v) => out.push_str(&v),
            None => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A layer's name and its group path, such as `body/arm`, for every layer.
fn layer_paths(doc: &Document) -> Vec<(LayerId, String, String)> {
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (depth, l) in doc.walk() {
        stack.truncate(depth);
        stack.push(l.props.name.clone());
        out.push((l.id, l.props.name.clone(), stack.join("/")));
    }
    out
}

/// The ids of the layers `wanted` names, by group path or by name.
fn find_layers(doc: &Document, wanted: &[String]) -> Result<HashSet<LayerId>, String> {
    let paths = layer_paths(doc);
    let mut out = HashSet::new();
    for w in wanted {
        let by_path: Vec<LayerId> = paths
            .iter()
            .filter(|(_, _, p)| p == w)
            .map(|(id, _, _)| *id)
            .collect();
        let found = if by_path.is_empty() {
            paths
                .iter()
                .filter(|(_, n, _)| n == w)
                .map(|(id, _, _)| *id)
                .collect()
        } else {
            by_path
        };
        if found.is_empty() {
            return Err(format!("there is no layer named \"{w}\""));
        }
        out.extend(found);
    }
    Ok(out)
}

/// `doc` with the layers `preset` shows: every layer when it shows hidden
/// ones, then only the layers it names (with what their groups hold), less
/// the layers it ignores.
pub fn shown_layers<'a>(doc: &'a Document, preset: &Preset) -> Result<Cow<'a, Document>, String> {
    if preset.only_layers.is_empty() && preset.ignore_layers.is_empty() && !preset.all_layers {
        return Ok(Cow::Borrowed(doc));
    }
    let only = find_layers(doc, &preset.only_layers)?;
    let ignore = find_layers(doc, &preset.ignore_layers)?;
    let mut keep: HashSet<LayerId> = HashSet::new();
    let mut open: HashSet<LayerId> = HashSet::new();
    for &id in &only {
        open.extend(doc.ancestors(id));
    }
    for (id, ..) in layer_paths(doc) {
        if only.contains(&id) || doc.ancestors(id).iter().any(|a| only.contains(a)) {
            keep.insert(id);
        }
    }
    let mut out = doc.clone();
    for (id, ..) in layer_paths(doc) {
        let visible = if ignore.contains(&id) {
            Some(false)
        } else if only.is_empty() {
            preset.all_layers.then_some(true)
        } else if only.contains(&id) || open.contains(&id) {
            Some(true)
        } else if keep.contains(&id) {
            preset.all_layers.then_some(true)
        } else {
            Some(false)
        };
        if let Some(v) = visible {
            let _ = out.update_layer(id, |p| p.visible = v);
        }
    }
    Ok(Cow::Owned(out))
}

/// The frames `preset` writes, each run with the tag that names it.
fn ranges(doc: &Document, current: usize, preset: &Preset) -> Vec<(String, Range<usize>)> {
    match &preset.frames {
        Frames::All => vec![(String::new(), 0..doc.frame_count())],
        Frames::Current => vec![(String::new(), current..current + 1)],
        Frames::Range { from, to } => vec![(String::new(), *from..to.saturating_add(1))],
        Frames::EachTag => doc
            .tags()
            .iter()
            .map(|t| (t.name.clone(), t.from..t.to + 1))
            .collect(),
        Frames::Tag(tag) => doc
            .tags()
            .iter()
            .filter(|t| t.name == *tag)
            .map(|t| (t.name.clone(), t.from..t.to + 1))
            .collect(),
    }
}

/// The files `preset` writes for `doc` (named `name`, showing `current`),
/// under `dir`. A path pattern with no extension gets the format's. A
/// layer the preset names that the document lacks is an error.
pub fn plan(
    doc: &Document,
    name: &str,
    current: usize,
    preset: &Preset,
    dir: &Path,
) -> Result<Vec<Job>, String> {
    let shown = shown_layers(doc, preset)?;
    let doc = shown.as_ref();
    let last = doc.frame_count().saturating_sub(1);
    let one_sheet = preset.format == Format::Sheet && preset.layers == Layers::Split;
    let layers: Vec<(Option<LayerId>, String)> = match preset.layers {
        Layers::Visible => vec![(None, String::new())],
        Layers::Split if one_sheet => vec![(None, String::new())],
        Layers::Each | Layers::Split => doc
            .walk()
            .into_iter()
            .filter(|(_, l)| matches!(l.kind, LayerKind::Raster { .. }) && l.props.visible)
            .map(|(_, l)| (Some(l.id), l.props.name.clone()))
            .collect(),
    };
    let pattern = if Path::new(&preset.path).extension().is_some() {
        preset.path.clone()
    } else {
        format!("{}.{}", preset.path, preset.format.extension())
    };
    let mut jobs = Vec::new();
    for (tag, range) in &ranges(doc, current, preset) {
        let range = range.start.min(doc.frame_count())..range.end.min(doc.frame_count());
        if range.is_empty() {
            continue;
        }
        for (layer, layer_name) in &layers {
            let tokens = Tokens {
                name,
                tag,
                frame: None,
                layer: layer_name,
                scale: preset.scale.max(1),
            };
            match preset.format {
                Format::Png => {
                    for f in range.clone() {
                        let t = Tokens {
                            frame: Some((f, range.start, last)),
                            ..tokens.clone()
                        };
                        jobs.push(Job {
                            path: dir.join(fill(&pattern, &t)),
                            data: None,
                            frames: f..f + 1,
                            layer: *layer,
                        });
                    }
                }
                Format::Gif => jobs.push(Job {
                    path: dir.join(fill(&pattern, &tokens)),
                    data: None,
                    frames: range.clone(),
                    layer: *layer,
                }),
                Format::Sheet => {
                    let path = dir.join(fill(&pattern, &tokens));
                    let data = match &preset.data {
                        Data::Beside => Some(path.with_extension("json")),
                        Data::None => None,
                        Data::At(p) => Some(dir.join(fill(p, &tokens))),
                    };
                    jobs.push(Job {
                        path,
                        data,
                        frames: range.clone(),
                        layer: *layer,
                    });
                }
            }
        }
    }
    // Two jobs on one path would overwrite each other; the first stands.
    let mut seen = HashSet::new();
    jobs.retain(|j| seen.insert(j.path.clone()));
    Ok(jobs)
}

/// The sheet options `preset` describes for one job, for a document named
/// `name`.
pub fn sheet_options(preset: &Preset, job: &Job, name: &str) -> SheetOptions {
    SheetOptions {
        layout: match preset.sheet {
            Sheet::Horizontal => SheetLayout::Horizontal,
            Sheet::Vertical => SheetLayout::Vertical,
            Sheet::Grid => SheetLayout::Grid {
                columns: preset.columns.max(1),
            },
            Sheet::Columns => SheetLayout::Columns {
                rows: preset.rows.max(1),
            },
            Sheet::Packed => SheetLayout::Packed,
        },
        frames: Some(job.frames.clone()),
        padding: preset.padding,
        border: preset.border,
        trim: preset.trim,
        by_layer: preset.layers == Layers::Split,
        scale: preset.scale.max(1),
        title: name.to_string(),
        image_name: job
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        json_frames: preset.json_frames,
    }
}

/// Writes one job's file (and a sheet's JSON), for a document named
/// `name`, returning how many files it wrote.
pub fn run_job(doc: &Document, name: &str, preset: &Preset, job: &Job) -> Result<usize, String> {
    let shown = shown_layers(doc, preset)?;
    let alone;
    let doc = match job.layer {
        Some(id) => {
            alone = only_layer(shown.as_ref(), id);
            &alone
        }
        None => shown.as_ref(),
    };
    let scale = preset.scale.max(1);
    match preset.format {
        Format::Png => {
            let bytes =
                export_frame_png(doc, job.frames.start, scale).map_err(|e| e.to_string())?;
            write(&job.path, &bytes)?;
            Ok(1)
        }
        Format::Gif => {
            let gif = export_gif(
                doc,
                &GifOptions {
                    frames: Some(job.frames.clone()),
                    scale,
                    repeat: None,
                },
            )
            .map_err(|e| e.to_string())?;
            write(&job.path, &gif.bytes)?;
            Ok(1)
        }
        Format::Sheet => {
            let options = sheet_options(preset, job, name);
            write_sheet(doc, &options, &job.path, job.data.as_deref())?;
            Ok(1 + usize::from(job.data.is_some()))
        }
    }
}

/// Lays out `doc`'s sheet and writes its image to `image` and, given a
/// path, its JSON to `data`. Folders are made.
pub fn write_sheet(
    doc: &Document,
    options: &SheetOptions,
    image: &Path,
    data: Option<&Path>,
) -> Result<SpriteSheet, String> {
    let sheet = sprite_sheet(doc, options).map_err(|e| e.to_string())?;
    let png = sheet.to_png().map_err(|e| e.to_string())?;
    write(image, &png)?;
    if let Some(data) = data {
        write(data, sheet.json.as_bytes())?;
    }
    Ok(sheet)
}

/// Writes `bytes` to `path` atomically, making its folder first.
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    }
    write_atomic(path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Runs every preset of `presets` on `doc` (named `name`, showing
/// `current`), writing under `dir`: each file's outcome, a written count
/// or why it failed.
pub fn run_presets(
    doc: &Document,
    name: &str,
    current: usize,
    presets: &[Preset],
    dir: &Path,
) -> Vec<Result<usize, String>> {
    let mut out = Vec::new();
    for preset in presets {
        match plan(doc, name, current, preset, dir) {
            Ok(jobs) => out.extend(jobs.iter().map(|job| run_job(doc, name, preset, job))),
            Err(e) => out.push(Err(format!("{}: {e}", preset.name))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_document::{ColorMode, LoopDirection, NewLayer, Placement, Rgba, Tag};

    fn doc() -> Document {
        let mut doc = Document::new(4, 4, ColorMode::Rgba).unwrap();
        for _ in 0..11 {
            doc.insert_frame(0, 100).unwrap();
        }
        doc.add_layer(
            NewLayer::Raster,
            "Shadow/top".to_string(),
            Placement::default(),
        )
        .unwrap();
        for (name, from, to) in [("walk", 0, 3), ("run", 4, 7)] {
            doc.tags_mut().push(Tag {
                name: name.to_string(),
                from,
                to,
                direction: LoopDirection::Forward,
                repeat: 0,
                color: Rgba::BLACK,
            });
        }
        doc
    }

    fn names(jobs: &[Job]) -> Vec<String> {
        jobs.iter()
            .map(|j| j.path.to_string_lossy().into_owned())
            .collect()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pixl-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn png_frames_number_every_frame_padded() {
        let jobs = plan(&doc(), "hero", 0, &Preset::default(), Path::new("out")).unwrap();
        assert_eq!(jobs.len(), 12);
        assert_eq!(jobs[0].path, Path::new("out").join("hero-00.png"));
        assert_eq!(jobs[11].path, Path::new("out").join("hero-11.png"));
    }

    #[test]
    fn frame_tokens_count_from_their_digits() {
        let t = |f| Tokens {
            name: "hero",
            frame: Some((f, 4, 11)),
            ..Tokens::default()
        };
        assert_eq!(fill("{title}{frame1}", &t(4)), "hero5");
        assert_eq!(fill("{frame001}-{frame000}", &t(4)), "005-004");
        assert_eq!(fill("{frame0}-{tagframe}", &t(9)), "9-5");
        assert_eq!(fill("{nope}-{frame", &t(4)), "{nope}-{frame");
        assert!(names_frame("a{frame001}.png"));
        assert!(!names_frame("a{tag}.png"));
    }

    #[test]
    fn each_tag_and_each_layer_multiply_and_name_their_files() {
        let preset = Preset {
            format: Format::Gif,
            frames: Frames::EachTag,
            layers: Layers::Each,
            scale: 2,
            path: "{tag}/{layer}@{scale}x".to_string(),
            ..Preset::default()
        };
        let jobs = plan(&doc(), "hero", 0, &preset, Path::new("")).unwrap();
        assert_eq!(
            names(&jobs),
            [
                "walk/Layer 1@2x.gif",
                "walk/Shadow_top@2x.gif",
                "run/Layer 1@2x.gif",
                "run/Shadow_top@2x.gif"
            ]
        );
        assert_eq!(jobs[2].frames, 4..8);
    }

    #[test]
    fn one_tag_a_range_and_the_current_frame() {
        let tag = Preset {
            frames: Frames::Tag("run".to_string()),
            path: "{name}-{tag}-{frame}".to_string(),
            ..Preset::default()
        };
        let jobs = plan(&doc(), "hero", 0, &tag, Path::new("")).unwrap();
        assert_eq!(
            names(&jobs),
            [
                "hero-run-04.png",
                "hero-run-05.png",
                "hero-run-06.png",
                "hero-run-07.png"
            ]
        );
        let range = Preset {
            frames: Frames::Range { from: 2, to: 3 },
            ..Preset::default()
        };
        assert_eq!(
            names(&plan(&doc(), "hero", 0, &range, Path::new("")).unwrap()),
            ["hero-02.png", "hero-03.png"]
        );
        let current = Preset {
            frames: Frames::Current,
            path: "{name}".to_string(),
            ..Preset::default()
        };
        assert_eq!(
            names(&plan(&doc(), "hero", 5, &current, Path::new("")).unwrap()),
            ["hero.png"]
        );
        let missing = Preset {
            frames: Frames::Tag("fly".to_string()),
            ..Preset::default()
        };
        assert!(
            plan(&doc(), "hero", 0, &missing, Path::new(""))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn named_layers_show_alone_and_a_missing_one_is_an_error() {
        let d = doc();
        let only = Preset {
            layers: Layers::Each,
            only_layers: vec!["Shadow/top".to_string()],
            path: "{layer}".to_string(),
            frames: Frames::Current,
            ..Preset::default()
        };
        assert_eq!(
            names(&plan(&d, "hero", 0, &only, Path::new("")).unwrap()),
            ["Shadow_top.png"]
        );
        let ignore = Preset {
            ignore_layers: vec!["Layer 1".to_string()],
            ..only.clone()
        };
        assert_eq!(
            names(&plan(&d, "hero", 0, &ignore, Path::new("")).unwrap()),
            ["Shadow_top.png"]
        );
        let missing = Preset {
            only_layers: vec!["Nope".to_string()],
            ..only
        };
        assert!(plan(&d, "hero", 0, &missing, Path::new("")).is_err());
    }

    #[test]
    fn a_sheet_writes_its_png_and_json() {
        let dir = temp("sheet");
        let preset = Preset {
            format: Format::Sheet,
            sheet: Sheet::Grid,
            columns: 4,
            path: "sheets/{name}".to_string(),
            ..Preset::default()
        };
        let d = doc();
        let jobs = plan(&d, "hero", 0, &preset, &dir).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(run_job(&d, "hero", &preset, &jobs[0]), Ok(2));
        let png = std::fs::read(dir.join("sheets/hero.png")).unwrap();
        let sheet = crate::png::decode_png(&png).unwrap();
        assert_eq!(sheet.size(), (16, 12));
        let json = std::fs::read_to_string(dir.join("sheets/hero.json")).unwrap();
        assert!(json.contains("\"image\": \"hero.png\""));
        assert!(json.contains("\"hero 0\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_packed_trimmed_sheet_is_as_small_as_its_pixels() {
        let dir = temp("packed");
        let mut d = Document::new(16, 16, ColorMode::Rgba).unwrap();
        let layer = d.layers()[0].id;
        let t = d.target(layer, 0).unwrap();
        t.raster
            .fill_rect_raw(&pixl_document::Rect::new(2, 2, 3, 2), Rgba::BLACK.to_u32());
        let preset = Preset {
            format: Format::Sheet,
            sheet: Sheet::Packed,
            trim: true,
            frames: Frames::Current,
            data: Data::None,
            path: "{name}".to_string(),
            ..Preset::default()
        };
        let jobs = plan(&d, "dot", 0, &preset, &dir).unwrap();
        assert_eq!(run_job(&d, "dot", &preset, &jobs[0]), Ok(1));
        let png = std::fs::read(dir.join("dot.png")).unwrap();
        assert_eq!(crate::png::decode_png(&png).unwrap().size(), (3, 2));
        assert!(!dir.join("dot.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_preset_file_reads_as_the_starters() {
        let dir = temp("file");
        assert_eq!(
            PresetFile::read(&dir.join(FILE)).unwrap(),
            Preset::starters()
        );
        std::fs::create_dir_all(&dir).unwrap();
        // A file from before the newer fields reads with their defaults.
        std::fs::write(
            dir.join(FILE),
            r#"{"presets":[{"name":"x","format":"gif","scale":2}]}"#,
        )
        .unwrap();
        let read = PresetFile::read(&dir.join(FILE)).unwrap();
        assert_eq!(read[0].format, Format::Gif);
        assert_eq!(read[0].data, Data::Beside);
        std::fs::write(dir.join(FILE), b"{nope").unwrap();
        assert!(PresetFile::read(&dir.join(FILE)).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
