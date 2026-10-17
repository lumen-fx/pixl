//! Documents on disk: new, open, save, save as and close for `.pixl` files,
//! import from PNG, GIF and Aseprite, export to PNG and GIF, and the
//! autosave recovery copy pixl-core keeps beside a saved document.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lumenui::ecs::change_detection::Mut;
use lumenui::ecs::hierarchy::ChildOf;
use lumenui::ecs::system::SystemParam;
use lumenui::ecs::world::World;
use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::gif::{GifOptions, export_gif, import_gif};
use pixl_core::io::{export_frame_png, import_png};
use pixl_core::native::{self, SaveOptions};
use pixl_core::sheet::SheetGrid;
use pixl_core::{Background, ColorMode, Document, aseprite};

use crate::editor::Editor;
use crate::home::Home;
use crate::keymap::KeyCommands;
use crate::recents::Recents;
use crate::settings::Settings;
use crate::view::parse_hex;

/// Something that replaces the document, waiting for the user to agree to
/// lose unsaved changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Replace {
    /// The New project dialog.
    New,
    /// The Open dialog.
    Open,
    /// File > Close: back to the start screen with no document.
    Close,
    /// A document from the start screen's list.
    Recent(PathBuf),
    /// A listed document's recovery copy.
    Recover(PathBuf),
    /// The frames a sprite sheet image splits into, each lasting the given
    /// milliseconds.
    Sheet(PathBuf, SheetGrid, u32),
}

/// File work in progress.
#[derive(Resource)]
pub struct Files {
    /// Whether opening a document offers its newer recovery copy; the
    /// files settings turn recovery copies off.
    pub offer_recovery: bool,
    /// A replacement another dialog asked for, carried out (or put to the
    /// discard question) on the next tick.
    pub queued: Option<Replace>,
    /// A replacement waiting on the discard question.
    pub confirming: Option<Replace>,
    /// The document whose newer recovery copy is on offer.
    pub recovery: Option<PathBuf>,
    /// When the recovery copy was last written, and for which revision.
    last_autosave: Option<(Instant, u64)>,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            offer_recovery: true,
            queued: None,
            confirming: None,
            recovery: None,
            last_autosave: None,
        }
    }
}

/// The file kinds the dialogs offer.
const OPENABLE: (&str, &[&str]) = (
    "pixl, Aseprite, PNG and GIF files",
    &["pixl", "aseprite", "ase", "png", "gif"],
);
const PIXL: (&str, &[&str]) = ("pixl documents", &["pixl"]);
const PNG: (&str, &[&str]) = ("PNG images", &["png"]);
const ASEPRITE: (&str, &[&str]) = ("Aseprite files", &["aseprite", "ase"]);
const GIF: (&str, &[&str]) = ("GIF animations", &["gif"]);

/// A document read from a file, with what the reader could not use.
pub struct Opened {
    pub doc: Document,
    pub warnings: Vec<String>,
}

/// Reads a document: `.pixl` natively, `.aseprite` / `.ase`, `.png` and
/// `.gif` through their importers.
pub fn read_document(path: &Path) -> std::result::Result<Opened, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let fail = |e: pixl_core::Error| format!("could not open {}: {e}", path.display());
    match ext.as_str() {
        "png" => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            Ok(Opened {
                doc: import_png(&bytes).map_err(fail)?,
                warnings: Vec::new(),
            })
        }
        "gif" => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            Ok(Opened {
                doc: import_gif(&bytes).map_err(fail)?,
                warnings: Vec::new(),
            })
        }
        "aseprite" | "ase" => {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            let import = aseprite::import_aseprite(&bytes).map_err(fail)?;
            Ok(Opened {
                doc: import.document,
                warnings: import.warnings.iter().map(ToString::to_string).collect(),
            })
        }
        _ => {
            let loaded = native::load_file(path).map_err(fail)?;
            Ok(Opened {
                doc: loaded.document,
                warnings: loaded.warnings.iter().map(ToString::to_string).collect(),
            })
        }
    }
}

/// Opens a document into the editor, offering its recovery copy when one
/// is newer than the file.
pub fn open_path(editor: &mut Editor, files: &mut Files, path: &Path) {
    match read_document(path) {
        Ok(opened) => {
            let native = !path.extension().is_some_and(|e| {
                ["png", "gif", "aseprite", "ase"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            });
            // An imported file is a new, unsaved document: saving writes a
            // .pixl, never over the file it came from.
            let path_kept = native.then(|| path.to_path_buf());
            editor.replace(opened.doc, path_kept);
            if !native {
                editor.imported_from = Some(path.to_path_buf());
            }
            editor.message = (!opened.warnings.is_empty()).then(|| opened.warnings.join("; "));
            files.recovery = (native && files.offer_recovery)
                .then(|| native::find_recovery(path))
                .flatten()
                .map(|_| path.to_path_buf());
            files.last_autosave = None;
        }
        Err(e) => editor.message = Some(e),
    }
}

/// Opens the recovery copy of the document at `path` in place of the file:
/// the document keeps the file's path and reads as modified until saved.
pub fn recover_path(editor: &mut Editor, path: &Path) {
    match native::load_file(&native::recovery_path(path)) {
        Ok(loaded) => {
            editor.replace(loaded.document, Some(path.to_path_buf()));
            // The recovered edits are not in the file yet.
            editor.touch();
        }
        Err(e) => editor.message = Some(format!("could not recover: {e}")),
    }
}

/// Saves the document as `.pixl` to `path` and drops its recovery copy.
pub fn save_to(editor: &mut Editor, path: &Path) -> std::result::Result<(), String> {
    let path = with_extension(path, "pixl");
    native::save_file(&editor.doc, &path, SaveOptions::default())
        .map_err(|e| format!("could not save {}: {e}", path.display()))?;
    let _ = native::discard_recovery(&path);
    editor.path = Some(path);
    editor.saved_revision = editor.revision;
    editor.message = None;
    Ok(())
}

/// Writes the current frame as a PNG, or the animation as a GIF.
pub fn export_to(editor: &mut Editor, path: &Path, gif: bool) -> std::result::Result<(), String> {
    let (bytes, warnings) = if gif {
        let out = export_gif(&editor.doc, &GifOptions::default()).map_err(|e| e.to_string())?;
        (
            out.bytes,
            out.warnings
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
    } else {
        (
            export_frame_png(&editor.doc, editor.frame, 1).map_err(|e| e.to_string())?,
            Vec::new(),
        )
    };
    let path = with_extension(path, if gif { "gif" } else { "png" });
    native::write_atomic(&path, &bytes)
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    editor.message = Some(if warnings.is_empty() {
        format!("Exported {}", path.display())
    } else {
        format!("Exported {}: {}", path.display(), warnings.join("; "))
    });
    Ok(())
}

/// `path` with `ext` when it has no extension of that kind.
pub(crate) fn with_extension(path: &Path, ext: &str) -> PathBuf {
    let has = path
        .extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext));
    if has {
        path.to_path_buf()
    } else {
        let mut p = path.as_os_str().to_owned();
        p.push(".");
        p.push(ext);
        PathBuf::from(p)
    }
}

/// The sizes the New project dialog offers with one click: squares, then
/// sprite and screen sizes. The dialog's markup has a button for each.
pub const PRESETS: &[(u32, u32)] = &[
    (16, 16),
    (32, 32),
    (64, 64),
    (128, 128),
    (256, 256),
    (512, 512),
    (16, 24),
    (24, 32),
    (32, 48),
    (48, 48),
    (160, 144),
    (256, 224),
    (256, 240),
    (320, 180),
    (320, 240),
    (640, 360),
];

/// The id of a preset's button in the New project dialog.
pub fn preset_id(w: u32, h: u32) -> String {
    format!("preset-{w}x{h}")
}

/// What the New project dialog asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct NewProject {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub mode: ColorMode,
    pub background: Background,
}

impl NewProject {
    /// The dialog's fields, checked.
    pub fn from_fields(signals: &Signals) -> std::result::Result<Self, String> {
        let text = |name: &str| signals.get::<String>(name).unwrap_or_default();
        let side = |name: &str| {
            text(name)
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|v| (1..=crate::MAX_VIEW_SIDE).contains(v))
        };
        let (Some(width), Some(height)) = (side("new_width"), side("new_height")) else {
            return Err(format!(
                "A new document is 1 to {} pixels on each side",
                crate::MAX_VIEW_SIDE
            ));
        };
        let mode = match text("new_mode").as_str() {
            "Indexed" => ColorMode::Indexed,
            _ => ColorMode::Rgba,
        };
        let background = match text("new_background").as_str() {
            "Color" => Background::Color(
                parse_hex(&text("new_bg_hex"))
                    .ok_or("The background color is a hex color such as #ffffff")?,
            ),
            _ => Background::Transparent,
        };
        let name = text("new_name").trim().to_string();
        Ok(Self {
            name: if name.is_empty() {
                "untitled".to_string()
            } else {
                name
            },
            width,
            height,
            mode,
            background,
        })
    }

    /// Replaces the editor's document with this new one.
    pub fn create(&self, editor: &mut Editor) -> std::result::Result<(), String> {
        let mut doc =
            Document::new(self.width, self.height, self.mode).map_err(|e| e.to_string())?;
        doc.set_background(self.background);
        editor.replace(doc, None);
        editor.name = Some(self.name.clone());
        Ok(())
    }
}

/// Makes a new document from the fields of the New project dialog.
fn create_from_fields(editor: &mut Editor, signals: &Signals) {
    let made = NewProject::from_fields(signals).and_then(|p| p.create(editor));
    if let Err(e) = made {
        editor.message = Some(e);
    }
}

/// Opens a document from the start screen's list, with the recovery offer
/// when it has a newer recovery copy.
fn open_recent(editor: &mut Editor, files: &mut Files, signals: &mut Signals, path: &Path) {
    open_path(editor, files, path);
    if files.recovery.is_some() {
        signals.set("recover_open", true);
    }
}

/// Carries out a replacement the user agreed to.
fn start(
    replace: Replace,
    editor: &mut Editor,
    files: &mut Files,
    home: &mut Home,
    commands: &mut Commands,
    signals: &mut Signals,
) {
    match replace {
        Replace::New => {
            signals.set("new_name", "untitled".to_string());
            signals.set("new_open", true);
        }
        Replace::Open => commands.queue(
            FileDialog::open()
                .filter(OPENABLE.0, OPENABLE.1)
                .filter(PIXL.0, PIXL.1)
                .filter(ASEPRITE.0, ASEPRITE.1)
                .filter(PNG.0, PNG.1)
                .filter(GIF.0, GIF.1)
                .then(|world: &mut World, paths| opened(world, paths)),
        ),
        Replace::Close => {
            editor.replace(
                Document::new(64, 64, ColorMode::Rgba).expect("64x64 is a valid size"),
                None,
            );
            files.recovery = None;
            files.last_autosave = None;
            home.show(editor, false);
        }
        Replace::Recent(path) => open_recent(editor, files, signals, &path),
        Replace::Recover(path) => recover_path(editor, &path),
        Replace::Sheet(path, grid, ms) => crate::sheets::import_into(editor, &path, &grid, ms),
    }
}

/// The Open and Import dialogs' answer.
fn opened(world: &mut World, paths: Vec<PathBuf>) {
    let Some(path) = paths.into_iter().next() else {
        return;
    };
    world.resource_scope(|world, mut files: Mut<Files>| {
        let mut editor = world.resource_mut::<Editor>();
        open_path(&mut editor, &mut files, &path);
    });
    if world.resource::<Files>().recovery.is_some() {
        world
            .resource_mut::<PropertyStore>()
            .set(PropertyKey::global("recover_open"), true.into());
    }
}

fn save_as(commands: &mut Commands, editor: &Editor) {
    let name = editor
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map_or_else(
            || "untitled.pixl".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
    commands.queue(
        FileDialog::save()
            .filter(PIXL.0, PIXL.1)
            .file_name(name)
            .then(|world: &mut World, paths| {
                if let Some(path) = paths.into_iter().next() {
                    let mut editor = world.resource_mut::<Editor>();
                    if let Err(e) = save_to(&mut editor, &path) {
                        editor.message = Some(e);
                    }
                }
            }),
    );
}

fn export(commands: &mut Commands, gif: bool) {
    let (label, exts) = if gif { GIF } else { PNG };
    commands.queue(
        FileDialog::save()
            .filter(label, exts)
            .file_name(if gif { "untitled.gif" } else { "untitled.png" })
            .then(move |world: &mut World, paths| {
                if let Some(path) = paths.into_iter().next() {
                    let mut editor = world.resource_mut::<Editor>();
                    if let Err(e) = export_to(&mut editor, &path, gif) {
                        editor.message = Some(e);
                    }
                }
            }),
    );
}

/// The ways a file command arrives: a File menu item, a click on a dialog
/// or start screen button, or a shortcut.
#[derive(SystemParam)]
pub struct FileInput<'w, 's> {
    menu: MessageReader<'w, 's, MenuClicked>,
    clicks: MessageReader<'w, 's, ClickEvent>,
    ids: Query<'w, 's, &'static LumenId>,
    parents: Query<'w, 's, &'static ChildOf>,
    keys: Res<'w, KeyCommands>,
}

impl FileInput<'_, '_> {
    /// The id of `entity` or of its nearest ancestor that has one: a click
    /// on a part of a start screen row is a click on the row.
    fn id_of(&self, mut entity: Entity) -> Option<String> {
        loop {
            if let Ok(id) = self.ids.get(entity) {
                return Some(id.0.clone());
            }
            entity = self.parents.get(entity).ok()?.parent();
        }
    }

    /// The ids of the commands chosen this tick. A shortcut runs the
    /// command of the menu item with its id.
    fn chosen(&mut self) -> Vec<String> {
        let mut chosen: Vec<String> = self.menu.read().map(|m| m.id.clone()).collect();
        let clicked: Vec<Entity> = self.clicks.read().map(|c| c.entity).collect();
        chosen.extend(clicked.into_iter().filter_map(|e| self.id_of(e)));
        chosen.extend(
            self.keys
                .pressed
                .iter()
                .filter(|(id, repeat)| !repeat && FILE_COMMANDS.contains(&id.as_str()))
                .map(|(id, _)| id.clone()),
        );
        chosen
    }
}

/// The file commands this system carries out when their key is pressed.
const FILE_COMMANDS: &[&str] = &[
    "file-new",
    "file-open",
    "file-save",
    "file-save-as",
    "file-export-png",
    "file-export-gif",
    "file-close",
];

/// The commands the start screen takes; the editor's others wait until a
/// document is open.
fn on_start_screen(id: &str) -> bool {
    matches!(id, "file-new" | "file-open")
        || ["home-", "recent-", "new-", "discard-", "recover-"]
            .iter()
            .any(|p| id.starts_with(p))
}

/// The File menu, its shortcuts, the start screen's buttons and list, and
/// the New, discard and recovery dialogs.
#[allow(clippy::too_many_arguments)]
pub fn file_commands(
    mut editor: ResMut<Editor>,
    mut files: ResMut<Files>,
    mut home: ResMut<Home>,
    mut recents: ResMut<Recents>,
    mut commands: Commands,
    mut input: FileInput,
    mut signals: Signals,
) {
    let editor = &mut *editor;
    if let Some(path) = home.pending.take() {
        let before = editor.generation;
        open_recent(editor, &mut files, &mut signals, &path);
        if editor.generation == before {
            // The file did not open: the start screen says why.
            home.show(editor, false);
        }
    }
    if let Some(r) = files.queued.take() {
        if editor.is_modified() {
            files.confirming = Some(r);
            signals.set("discard_open", true);
        } else {
            start(
                r,
                editor,
                &mut files,
                &mut home,
                &mut commands,
                &mut signals,
            );
        }
    }
    let chosen = input.chosen();
    for id in chosen {
        if home.open && !on_start_screen(&id) {
            continue;
        }
        let listed = |prefix: &str| {
            id.strip_prefix(prefix)
                .and_then(|k| recents.find(k))
                .map(|r| r.path.clone())
        };
        let replace = match id.as_str() {
            "menu-file" => {
                signals.toggle("__menu_open:file-menu");
                None
            }
            "file-new" | "home-new" => Some(Replace::New),
            "file-open" | "home-open" => Some(Replace::Open),
            "file-close" => Some(Replace::Close),
            "go-home" => {
                home.show(editor, true);
                None
            }
            "home-back" => {
                home.open = false;
                None
            }
            "file-save" => {
                match editor.path.clone() {
                    Some(path) => {
                        if let Err(e) = save_to(editor, &path) {
                            editor.message = Some(e);
                        }
                    }
                    None => save_as(&mut commands, editor),
                }
                None
            }
            "file-save-as" => {
                save_as(&mut commands, editor);
                None
            }
            "file-export-png" => {
                export(&mut commands, false);
                None
            }
            "file-export-gif" => {
                export(&mut commands, true);
                None
            }
            "new-create" => {
                signals.set("new_open", false);
                create_from_fields(editor, &signals);
                None
            }
            "new-cancel" => {
                signals.set("new_open", false);
                None
            }
            "discard-yes" => {
                signals.set("discard_open", false);
                if let Some(r) = files.confirming.take() {
                    start(
                        r,
                        editor,
                        &mut files,
                        &mut home,
                        &mut commands,
                        &mut signals,
                    );
                }
                None
            }
            "discard-no" => {
                signals.set("discard_open", false);
                files.confirming = None;
                None
            }
            "recover-yes" => {
                signals.set("recover_open", false);
                if let Some(path) = files.recovery.take() {
                    recover_path(editor, &path);
                }
                None
            }
            "recover-no" => {
                signals.set("recover_open", false);
                if let Some(path) = files.recovery.take() {
                    let _ = native::discard_recovery(&path);
                }
                None
            }
            _ => {
                if let Some(path) = listed("recent-remove-") {
                    recents.remove(&path);
                    None
                } else if let Some(path) = listed("recent-recover-") {
                    Some(Replace::Recover(path))
                } else if let Some(path) = listed("recent-") {
                    if path.exists() {
                        Some(Replace::Recent(path))
                    } else {
                        editor.message = Some(format!("{} is missing", path.display()));
                        None
                    }
                } else {
                    None
                }
            }
        };
        if let Some(r) = replace {
            if editor.is_modified() {
                files.confirming = Some(r);
                signals.set("discard_open", true);
            } else {
                start(
                    r,
                    editor,
                    &mut files,
                    &mut home,
                    &mut commands,
                    &mut signals,
                );
            }
        }
    }
}

/// Keeps the New project dialog showing what its fields say: the preset
/// that matches the size, and the background color field when the
/// background is a color. A preset's button fills in its size.
pub fn new_form(
    mut clicks: MessageReader<ClickEvent>,
    ids: Query<&LumenId>,
    mut signals: Signals,
    mut shown: Local<Option<(String, String, String)>>,
) {
    for click in clicks.read() {
        let Ok(id) = ids.get(click.entity) else {
            continue;
        };
        if let Some((w, h)) = id.0.strip_prefix("preset-").and_then(|s| s.split_once('x')) {
            signals.set("new_width", w.to_string());
            signals.set("new_height", h.to_string());
        }
    }
    let text = |name: &str| signals.get::<String>(name).unwrap_or_default();
    let now = (
        format!("{}x{}", text("new_width").trim(), text("new_height").trim()),
        text("new_background"),
        text("new_bg_hex"),
    );
    if shown.as_ref() == Some(&now) {
        return;
    }
    if dom::get_by_id("new-name").is_none() {
        // The dialog is not built yet.
        return;
    }
    for (w, h) in PRESETS {
        if let Some(button) = dom::get_by_id(&preset_id(*w, *h)) {
            crate::set_class(button, "active", now.0 == format!("{w}x{h}"));
        }
    }
    let color = now.1 == "Color";
    if signals.get::<bool>("new_bg_color") != Some(color) {
        signals.set("new_bg_color", color);
    }
    // The swatch shows the color typed, which is the user's data. It exists
    // while the background is a color.
    if let (Some(c), Some(swatch)) = (parse_hex(&now.2), dom::get_by_id("new-bg-swatch")) {
        swatch.set_style("bg", &crate::tools::hex(c));
    } else if color {
        return;
    }
    *shown = Some(now);
}

/// Writes the recovery copy of a modified document that has a file, as
/// often as the files settings say, unless they turn recovery copies off.
/// An untitled document has nowhere beside it to keep one.
pub fn autosave(
    editor: Res<Editor>,
    mut files: ResMut<Files>,
    settings: Res<Settings>,
    wake: Option<Res<WakeDeadline>>,
) {
    if files.offer_recovery != settings.files.recovery {
        files.offer_recovery = settings.files.recovery;
    }
    let Some(every) = settings.autosave_every() else {
        return;
    };
    let Some(path) = editor.path.as_ref() else {
        return;
    };
    if !editor.is_modified() {
        return;
    }
    let now = Instant::now();
    match files.last_autosave {
        None => files.last_autosave = Some((now, editor.saved_revision)),
        Some((at, revision)) if revision != editor.revision && now - at >= every => {
            if native::write_recovery(&editor.doc, path).is_ok() {
                files.last_autosave = Some((now, editor.revision));
            }
        }
        Some((at, _)) => {
            if let Some(wake) = wake {
                wake.request(at + every);
            }
        }
    }
}

/// The status bar's message line and the window title.
pub fn show_message(editor: Res<Editor>, mut signals: Signals) {
    let message = editor.message.clone().unwrap_or_default();
    crate::tools::set_if_changed(&mut signals, "status_message", message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_adds_the_extension_once() {
        assert_eq!(
            with_extension(Path::new("a/b"), "pixl"),
            PathBuf::from("a/b.pixl")
        );
        assert_eq!(
            with_extension(Path::new("a/b.PIXL"), "pixl"),
            PathBuf::from("a/b.PIXL")
        );
        assert_eq!(
            with_extension(Path::new("a/b.png"), "pixl"),
            PathBuf::from("a/b.png.pixl")
        );
    }
}
