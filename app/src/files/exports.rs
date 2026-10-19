//! The Export presets dialog and File > Export all. The presets themselves
//! (what each writes, and the writing) are `pixl_io::export`'s, kept in
//! `exports.json` in pixl's data folder. Export all runs every preset marked
//! for it on the open document, on a worker thread, writing beside the
//! document.

use std::path::Path;
use std::thread::JoinHandle;

use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_io::export::run_presets;
pub use pixl_io::export::{FILE, Format, Frames, Layers, Preset, PresetFile, Sheet};

use crate::editor::Editor;
use crate::home::Home;
use crate::ids::ElementIds;
use crate::panels::escape;
use crate::settings::keymap::KeyCommands;
use crate::settings::store::Store;

/// What the dialog says under the path field.
pub const PATH_HINT: &str =
    "Beside the document. {name} {tag} {frame} {layer} {scale} fill in; folders are made.";

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
            .load::<PresetFile>(FILE)
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
            &PresetFile {
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
            run_presets(&doc, &name, current, &presets, &dir)
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
    signals.set("ex_trim", p.trim);
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
        "All frames" => Frames::All,
        "Current frame" => Frames::Current,
        "Each tag" => Frames::EachTag,
        "One tag" => Frames::Tag(text("ex_tag").trim().to_string()),
        // A choice the dialog does not offer, such as a range written in
        // exports.json, stays.
        _ => base.frames.clone(),
    };
    Preset {
        name: {
            let n = text("ex_name").trim().to_string();
            if n.is_empty() { base.name.clone() } else { n }
        },
        format: Format::from_label(&text("ex_format")).unwrap_or(base.format),
        scale: number("ex_scale", base.scale, 64).max(1),
        frames,
        layers: [Layers::Visible, Layers::Each]
            .into_iter()
            .find(|l| l.label() == text("ex_layers"))
            .unwrap_or(base.layers),
        sheet: Sheet::ALL
            .into_iter()
            .find(|s| s.label() == text("ex_sheet"))
            .unwrap_or(base.sheet),
        columns: number("ex_columns", base.columns, 4096).max(1),
        padding: number("ex_padding", base.padding, 256),
        trim: signals.get::<bool>("ex_trim").unwrap_or(base.trim),
        path: {
            let p = text("ex_path").trim().to_string();
            if p.is_empty() { base.path.clone() } else { p }
        },
        in_all: signals.get::<bool>("ex_in_all").unwrap_or(base.in_all),
        // What the dialog has no field for stays as exports.json says.
        ..base.clone()
    }
}

/// The ids this tick's menu items, clicks and keys chose.
fn chosen(
    menu: &mut MessageReader<MenuClicked>,
    clicks: &mut MessageReader<ClickEvent>,
    ids: &ElementIds,
    keys: &KeyCommands,
) -> Vec<String> {
    let mut out: Vec<String> = menu.read().map(|m| m.id.clone()).collect();
    out.extend(clicks.read().filter_map(|c| ids.id_of(c.entity)));
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
    ids: ElementIds,
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
    use crate::files::recents::APP_ID;
    use lumenui::lumen_core::app_paths;

    #[test]
    fn presets_keep_and_a_new_user_gets_the_starters() {
        let dir = std::env::temp_dir().join(format!("pixl-exports-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ex = Exports::load(Store::at(&dir));
        assert_eq!(ex.presets, Preset::starters());
        ex.presets[0].scale = 3;
        ex.persist();
        assert_eq!(Exports::load(Store::at(&dir)).presets[0].scale, 3);
        assert_eq!(PresetFile::read(&dir.join(FILE)).unwrap()[0].scale, 3);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_command_line_finds_the_presets_the_editor_keeps() {
        assert_eq!(
            pixl_io::export::user_dir(),
            Some(app_paths::data_dir_for(APP_ID))
        );
    }
}
