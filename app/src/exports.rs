//! Export presets: named export settings (format, scale, which frames and
//! layers, the sheet layout, and an output path pattern with tokens such as
//! `{name}`, `{tag}`, `{frame}` and `{layer}`), kept in `exports.json` in
//! pixl's data folder. File > Export all runs every preset marked for it on
//! the open document, on a worker thread, writing beside the document.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::gif::{GifOptions, export_gif};
use pixl_core::io::{SheetLayout, SheetOptions, export_frame_png, sprite_sheet};
use pixl_core::native::write_atomic;
use pixl_core::{Document, LayerId, LayerKind};
use serde::{Deserialize, Serialize};

use crate::editor::Editor;
use crate::home::Home;
use crate::keymap::KeyCommands;
use crate::panels::escape;
use crate::store::Store;

/// The file the presets are kept in.
pub const FILE: &str = "exports.json";

/// What the dialog says under the path field.
pub const PATH_HINT: &str =
    "Beside the document. {name} {tag} {frame} {layer} {scale} fill in; folders are made.";

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

    fn extension(self) -> &'static str {
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
    /// The frame the editor shows.
    Current,
    /// Each tag on its own, `{tag}` naming it.
    EachTag,
    /// The frames of one tag.
    Tag(String),
}

impl Frames {
    pub fn label(&self) -> &'static str {
        match self {
            Frames::All => "All frames",
            Frames::Current => "Current frame",
            Frames::EachTag => "Each tag",
            Frames::Tag(_) => "One tag",
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
}

impl Layers {
    pub fn label(self) -> &'static str {
        match self {
            Layers::Visible => "Visible, merged",
            Layers::Each => "Each layer",
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
    Grid,
}

impl Sheet {
    pub const ALL: [Sheet; 3] = [Sheet::Horizontal, Sheet::Vertical, Sheet::Grid];

    pub fn label(self) -> &'static str {
        match self {
            Sheet::Horizontal => "One row",
            Sheet::Vertical => "One column",
            Sheet::Grid => "Grid",
        }
    }
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
    pub sheet: Sheet,
    /// Columns of a grid sheet.
    pub columns: u32,
    /// Transparent pixels between sheet frames.
    pub padding: u32,
    /// Where the files go, relative to the document's folder. Tokens:
    /// `{name}` the document's name, `{tag}`, `{frame}` (numbered from 0,
    /// padded to the last frame's digits), `{layer}` and `{scale}`.
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
            sheet: Sheet::Horizontal,
            columns: 8,
            padding: 0,
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

/// One file an export writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub path: PathBuf,
    pub frames: std::ops::Range<usize>,
    /// The one layer it shows, or every visible one.
    pub layer: Option<LayerId>,
}

/// The value of each token in a path pattern.
#[derive(Clone, Debug, Default)]
struct Tokens<'a> {
    name: &'a str,
    tag: &'a str,
    frame: Option<(usize, usize)>,
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

fn fill(pattern: &str, t: &Tokens) -> String {
    let frame = t.frame.map_or_else(String::new, |(f, last)| {
        let width = last.to_string().len();
        format!("{f:0width$}")
    });
    pattern
        .replace("{name}", &component(t.name))
        .replace("{tag}", &component(t.tag))
        .replace("{frame}", &frame)
        .replace("{layer}", &component(t.layer))
        .replace("{scale}", &t.scale.to_string())
}

/// The files `preset` writes for `doc` (named `name`, showing `current`),
/// under `dir`. A path pattern with no extension gets the format's.
pub fn plan(doc: &Document, name: &str, current: usize, preset: &Preset, dir: &Path) -> Vec<Job> {
    let last = doc.frame_count().saturating_sub(1);
    let ranges: Vec<(String, std::ops::Range<usize>)> = match &preset.frames {
        Frames::All => vec![(String::new(), 0..doc.frame_count())],
        Frames::Current => vec![(String::new(), current..current + 1)],
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
    };
    let layers: Vec<(Option<LayerId>, String)> = match preset.layers {
        Layers::Visible => vec![(None, String::new())],
        Layers::Each => doc
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
    for (tag, range) in &ranges {
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
                            frame: Some((f, last)),
                            ..tokens.clone()
                        };
                        jobs.push(Job {
                            path: dir.join(fill(&pattern, &t)),
                            frames: f..f + 1,
                            layer: *layer,
                        });
                    }
                }
                Format::Gif | Format::Sheet => jobs.push(Job {
                    path: dir.join(fill(&pattern, &tokens)),
                    frames: range.clone(),
                    layer: *layer,
                }),
            }
        }
    }
    // Two jobs on one path would overwrite each other; the first stands.
    let mut seen = std::collections::HashSet::new();
    jobs.retain(|j| seen.insert(j.path.clone()));
    jobs
}

/// `doc` showing only the raster layer `layer` (its groups stay as they
/// are).
fn only_layer(doc: &Document, layer: LayerId) -> Document {
    let mut out = doc.clone();
    let others: Vec<LayerId> = out
        .walk()
        .into_iter()
        .filter(|(_, l)| matches!(l.kind, LayerKind::Raster { .. }) && l.id != layer)
        .map(|(_, l)| l.id)
        .collect();
    for id in others {
        let _ = out.update_layer(id, |p| p.visible = false);
    }
    out
}

/// Writes one job's file (and a sheet's JSON beside it), returning how many
/// files it wrote.
pub fn run_job(doc: &Document, preset: &Preset, job: &Job) -> std::result::Result<usize, String> {
    let shown;
    let doc = match job.layer {
        Some(id) => {
            shown = only_layer(doc, id);
            &shown
        }
        None => doc,
    };
    let scale = preset.scale.max(1);
    let mut files = 1;
    let bytes = match preset.format {
        Format::Png => export_frame_png(doc, job.frames.start, scale).map_err(|e| e.to_string())?,
        Format::Gif => {
            export_gif(
                doc,
                &GifOptions {
                    frames: Some(job.frames.clone()),
                    scale,
                    repeat: None,
                },
            )
            .map_err(|e| e.to_string())?
            .bytes
        }
        Format::Sheet => {
            let image_name = job
                .path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let title = job
                .path
                .file_stem()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let sheet = sprite_sheet(
                doc,
                &SheetOptions {
                    layout: match preset.sheet {
                        Sheet::Horizontal => SheetLayout::Horizontal,
                        Sheet::Vertical => SheetLayout::Vertical,
                        Sheet::Grid => SheetLayout::Grid {
                            columns: preset.columns.max(1),
                        },
                    },
                    frames: Some(job.frames.clone()),
                    padding: preset.padding,
                    scale,
                    title,
                    image_name,
                    ..SheetOptions::default()
                },
            )
            .map_err(|e| e.to_string())?;
            write(&job.path.with_extension("json"), sheet.json.as_bytes())?;
            files += 1;
            sheet.to_png().map_err(|e| e.to_string())?
        }
    };
    write(&job.path, &bytes)?;
    Ok(files)
}

fn write(path: &Path, bytes: &[u8]) -> std::result::Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    write_atomic(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// What `exports.json` holds.
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    presets: Vec<Preset>,
}

/// The presets, the one the dialog edits, and an export in progress.
#[derive(Resource)]
pub struct Exports {
    store: Store,
    pub presets: Vec<Preset>,
    /// The preset the dialog shows.
    pub selected: usize,
    /// The dialog's fields were filled for this preset (index, contents).
    filled: Option<(usize, Preset)>,
    shown_list: Option<String>,
    /// Exports running on a worker thread: each result names its file.
    running: Option<JoinHandle<Vec<std::result::Result<usize, String>>>>,
}

impl Exports {
    pub fn load(store: Store) -> Self {
        let presets = store
            .load::<Saved>(FILE)
            .map(|s| s.presets)
            .filter(|p| !p.is_empty())
            .unwrap_or_else(Preset::starters);
        Self {
            store,
            presets,
            selected: 0,
            filled: None,
            shown_list: None,
            running: None,
        }
    }

    pub fn user() -> Self {
        Self::load(Store::user())
    }

    pub fn in_memory() -> Self {
        Self::load(Store::in_memory())
    }

    fn persist(&self) {
        let _ = self.store.save(
            FILE,
            &Saved {
                presets: self.presets.clone(),
            },
        );
    }

    /// Whether an export is still running.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Starts `presets` on `editor`'s document on a worker thread. Without
    /// a file there is no folder to write beside, so it says so instead.
    fn start(&mut self, editor: &mut Editor, presets: Vec<Preset>) {
        if self.running.is_some() {
            editor.message = Some("An export is still running".to_string());
            return;
        }
        let Some(dir) = editor
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
        else {
            editor.message = Some("Save the document first: presets write beside it".to_string());
            return;
        };
        if presets.is_empty() {
            editor.message = Some("No preset is marked for Export all".to_string());
            return;
        }
        let name = editor
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| editor.name(), |s| s.to_string_lossy().into_owned());
        // The document's tiles are shared, so the copy is cheap.
        let doc = editor.doc.clone();
        let current = editor.frame;
        editor.message = Some(format!("Exporting {} presets...", presets.len()));
        self.running = Some(std::thread::spawn(move || {
            let mut out = Vec::new();
            for preset in &presets {
                for job in plan(&doc, &name, current, preset, &dir) {
                    out.push(run_job(&doc, preset, &job));
                }
            }
            out
        }));
    }

    /// Collects a finished export into a message for the status bar.
    fn collect(&mut self) -> Option<String> {
        if !self.running.as_ref().is_some_and(JoinHandle::is_finished) {
            return None;
        }
        let results = self.running.take()?.join().unwrap_or_default();
        let written: usize = results.iter().filter_map(|r| r.as_ref().ok()).sum();
        let failed: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        Some(match failed.first() {
            None => format!("Exported {written} files"),
            Some(e) => format!("Exported {written} files; {} failed: {e}", failed.len()),
        })
    }
}

/// The dialog's fields as signals.
fn fill_fields(signals: &mut Signals, p: &Preset) {
    signals.set("ex_name", p.name.clone());
    signals.set("ex_format", p.format.label().to_string());
    signals.set("ex_scale", p.scale.to_string());
    signals.set("ex_frames", p.frames.label().to_string());
    let tag = match &p.frames {
        Frames::Tag(t) => t.clone(),
        _ => String::new(),
    };
    signals.set("ex_tag", tag);
    signals.set("ex_layers", p.layers.label().to_string());
    signals.set("ex_sheet", p.sheet.label().to_string());
    signals.set("ex_columns", p.columns.to_string());
    signals.set("ex_padding", p.padding.to_string());
    signals.set("ex_path", p.path.clone());
    signals.set("ex_in_all", p.in_all);
    signals.set("ex_is_sheet", p.format == Format::Sheet);
    signals.set("ex_is_tag", matches!(p.frames, Frames::Tag(_)));
}

/// The preset the dialog's fields describe, over `base`.
fn read_fields(signals: &Signals, base: &Preset) -> Preset {
    let text = |name: &str| signals.get::<String>(name).unwrap_or_default();
    let number = |name: &str, fallback: u32, max: u32| {
        text(name)
            .trim()
            .parse::<u32>()
            .map_or(fallback, |v| v.clamp(0, max))
    };
    let frames = match text("ex_frames").as_str() {
        "Current frame" => Frames::Current,
        "Each tag" => Frames::EachTag,
        "One tag" => Frames::Tag(text("ex_tag").trim().to_string()),
        _ => Frames::All,
    };
    Preset {
        name: {
            let n = text("ex_name").trim().to_string();
            if n.is_empty() { base.name.clone() } else { n }
        },
        format: Format::from_label(&text("ex_format")).unwrap_or(base.format),
        scale: number("ex_scale", base.scale, 64).max(1),
        frames,
        layers: if text("ex_layers") == Layers::Each.label() {
            Layers::Each
        } else {
            Layers::Visible
        },
        sheet: Sheet::ALL
            .into_iter()
            .find(|s| s.label() == text("ex_sheet"))
            .unwrap_or(base.sheet),
        columns: number("ex_columns", base.columns, 4096).max(1),
        padding: number("ex_padding", base.padding, 256),
        path: {
            let p = text("ex_path").trim().to_string();
            if p.is_empty() { base.path.clone() } else { p }
        },
        in_all: signals.get::<bool>("ex_in_all").unwrap_or(base.in_all),
    }
}

/// The ids this tick's menu items, clicks and keys chose.
fn chosen(
    menu: &mut MessageReader<MenuClicked>,
    clicks: &mut MessageReader<ClickEvent>,
    ids: &Query<&LumenId>,
    keys: &KeyCommands,
) -> Vec<String> {
    let mut out: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    out.extend(
        clicks
            .read()
            .filter_map(|c| ids.get(c.entity).ok().map(|id| id.0.clone())),
    );
    for id in ["file-export-presets", "file-export-all"] {
        if keys.ran(id, false) {
            out.push(id.to_string());
        }
    }
    out
}

/// The Export presets dialog and Export all.
#[allow(clippy::too_many_arguments)]
pub fn export_input(
    mut exports: ResMut<Exports>,
    mut editor: ResMut<Editor>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    ids: Query<&LumenId>,
    keys: Res<KeyCommands>,
    home: Res<Home>,
    mut signals: Signals,
) {
    let chosen = chosen(&mut menu, &mut clicks, &ids, &keys);
    if home.open {
        return;
    }
    let open = signals.get::<bool>("ex_open") == Some(true);
    // Keep what the fields say in the preset as it is typed.
    if open
        && let Some((i, filled)) = exports.filled.clone()
        && let Some(current) = exports.presets.get(i).cloned()
    {
        let read = read_fields(&signals, &current);
        if read != filled {
            signals.set("ex_is_sheet", read.format == Format::Sheet);
            signals.set("ex_is_tag", matches!(read.frames, Frames::Tag(_)));
            exports.presets[i] = read.clone();
            exports.filled = Some((i, read));
            exports.shown_list = None;
            exports.persist();
        }
    }
    for id in chosen {
        match id.as_str() {
            "file-export-presets" => {
                exports.filled = None;
                exports.shown_list = None;
                signals.set("ex_open", true);
            }
            "ex-close" => signals.set("ex_open", false),
            "ex-new" | "ex-duplicate" => {
                let mut p = if id == "ex-new" {
                    Preset::default()
                } else {
                    exports
                        .presets
                        .get(exports.selected)
                        .cloned()
                        .unwrap_or_default()
                };
                p.name = format!("{} copy", p.name);
                exports.presets.push(p);
                exports.selected = exports.presets.len() - 1;
                exports.filled = None;
                exports.persist();
            }
            "ex-delete" => {
                if exports.presets.len() > 1 {
                    let i = exports.selected.min(exports.presets.len() - 1);
                    exports.presets.remove(i);
                    exports.selected = i.saturating_sub(1);
                    exports.filled = None;
                    exports.persist();
                }
            }
            "ex-run" => {
                if let Some(p) = exports.presets.get(exports.selected).cloned() {
                    exports.start(&mut editor, vec![p]);
                }
            }
            "file-export-all" | "ex-run-all" => {
                let all: Vec<Preset> = exports
                    .presets
                    .iter()
                    .filter(|p| p.in_all)
                    .cloned()
                    .collect();
                exports.start(&mut editor, all);
            }
            other => {
                if let Some(i) = other
                    .strip_prefix("ex-preset-")
                    .and_then(|i| i.parse::<usize>().ok())
                    .filter(|&i| i < exports.presets.len())
                {
                    exports.selected = i;
                    exports.filled = None;
                }
            }
        }
    }
}

/// Fills the dialog for the selected preset, lists the presets, and puts a
/// finished export's outcome in the status bar.
pub fn show_exports(
    mut exports: ResMut<Exports>,
    mut editor: ResMut<Editor>,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    if let Some(note) = exports.collect() {
        editor.message = Some(note);
    }
    if exports.busy()
        && let Some(wake) = &wake
    {
        // Look again soon for the export to finish.
        wake.request(std::time::Instant::now() + std::time::Duration::from_millis(50));
    }
    if signals.get::<bool>("ex_open") != Some(true) {
        return;
    }
    let i = exports
        .selected
        .min(exports.presets.len().saturating_sub(1));
    if exports.filled.as_ref().map(|(k, _)| *k) != Some(i)
        && let Some(p) = exports.presets.get(i).cloned()
    {
        fill_fields(&mut signals, &p);
        exports.filled = Some((i, p));
        exports.shown_list = None;
        crate::repaint_soon(&wake);
    }
    let mut markup = String::new();
    for (k, p) in exports.presets.iter().enumerate() {
        let current = if k == i { " current" } else { "" };
        let all = if p.in_all { "in Export all" } else { "" };
        markup.push_str(&format!(
            r#"<row id="ex-preset-{k}" class="ex-preset{current}"><column class="ex-preset-text"><label class="ex-preset-name" text="{}"/><label class="ex-preset-meta" text="{}"/></column><spacer/><label class="ex-preset-all" text="{all}"/></row>"#,
            escape(&p.name),
            // Braces in markup read signals; the list shows a token as <name>.
            escape(&format!(
                "{}, {}x, {}",
                p.format.label(),
                p.scale,
                p.path.replace('{', "<").replace('}', ">")
            )),
        ));
    }
    if exports.shown_list.as_deref() != Some(markup.as_str())
        && let Some(list) = dom::get_by_id("ex-list")
    {
        list.set_inner_markup(&markup);
        exports.shown_list = Some(markup);
        crate::repaint_soon(&wake);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_core::{ColorMode, LoopDirection, NewLayer, Placement, Rgba, Tag};

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

    #[test]
    fn png_frames_number_every_frame_padded() {
        let jobs = plan(&doc(), "hero", 0, &Preset::default(), Path::new("out"));
        assert_eq!(jobs.len(), 12);
        assert_eq!(names(&jobs)[0], "out/hero-00.png");
        assert_eq!(names(&jobs)[11], "out/hero-11.png");
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
        let jobs = plan(&doc(), "hero", 0, &preset, Path::new(""));
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
    fn one_tag_and_the_current_frame() {
        let tag = Preset {
            frames: Frames::Tag("run".to_string()),
            path: "{name}-{tag}-{frame}".to_string(),
            ..Preset::default()
        };
        let jobs = plan(&doc(), "hero", 0, &tag, Path::new(""));
        assert_eq!(
            names(&jobs),
            [
                "hero-run-04.png",
                "hero-run-05.png",
                "hero-run-06.png",
                "hero-run-07.png"
            ]
        );
        let current = Preset {
            frames: Frames::Current,
            path: "{name}".to_string(),
            ..Preset::default()
        };
        assert_eq!(
            names(&plan(&doc(), "hero", 5, &current, Path::new(""))),
            ["hero.png"]
        );
        let missing = Preset {
            frames: Frames::Tag("fly".to_string()),
            ..Preset::default()
        };
        assert!(plan(&doc(), "hero", 0, &missing, Path::new("")).is_empty());
    }

    #[test]
    fn a_sheet_writes_its_png_and_json() {
        let dir = std::env::temp_dir().join(format!("pixl-exports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let preset = Preset {
            format: Format::Sheet,
            sheet: Sheet::Grid,
            columns: 4,
            path: "sheets/{name}".to_string(),
            ..Preset::default()
        };
        let d = doc();
        let jobs = plan(&d, "hero", 0, &preset, &dir);
        assert_eq!(jobs.len(), 1);
        run_job(&d, &preset, &jobs[0]).unwrap();
        let png = std::fs::read(dir.join("sheets/hero.png")).unwrap();
        let sheet = pixl_core::io::decode_png(&png).unwrap();
        assert_eq!(sheet.size(), (16, 12));
        let json = std::fs::read_to_string(dir.join("sheets/hero.json")).unwrap();
        assert!(
            json.contains("\"image\": \"hero.png\"") || json.contains("\"image\":\"hero.png\"")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn presets_keep_and_a_new_user_gets_the_starters() {
        let dir = std::env::temp_dir().join(format!("pixl-exports-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ex = Exports::load(Store::at(&dir));
        assert_eq!(ex.presets, Preset::starters());
        ex.presets[0].scale = 3;
        ex.persist();
        assert_eq!(Exports::load(Store::at(&dir)).presets[0].scale, 3);
        let _ = std::fs::remove_dir_all(dir);
    }
}
