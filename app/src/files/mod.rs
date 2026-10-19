//! Documents on disk: new, open, save and save as for `.pixl` files,
//! import from PNG, GIF and Aseprite, export to PNG and GIF, the documents
//! pixl ships, and the autosave recovery copy pixl-document keeps beside a saved
//! document.

pub mod exports;
pub mod recents;
pub mod sheets;
pub mod shipped;

use std::path::{Path, PathBuf};
use std::time::Instant;

use lumenui::ecs::change_detection::Mut;
use lumenui::ecs::system::SystemParam;
use lumenui::ecs::world::World;
use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_document::native::{self, SaveOptions};
use pixl_document::{Background, ColorMode, Document};
use pixl_io::gif::{GifOptions, export_gif};
use pixl_io::open::is_imported;
pub use pixl_io::open::{Opened, read_document};
use pixl_io::png::export_frame_png;
use pixl_io::sheet::SheetGrid;

use crate::documents::{Documents, TabAction, edit_document};
use crate::editor::Editor;
use crate::files::recents::Recents;
use crate::files::shipped::Shipped;
use crate::home::Home;
use crate::ids::ElementIds;
use crate::settings::Settings;
use crate::settings::keymap::KeyCommands;
use crate::view::parse_hex;

/// A document to open in a tab of its own, which another dialog asked for:
/// carried out on the next tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The New project dialog.
    New,
    /// The Open dialog.
    Open,
    /// A document from the start screen's list.
    Recent(PathBuf),
    /// A listed document's recovery copy.
    Recover(PathBuf),
    /// The frames a sprite sheet image splits into, each lasting the given
    /// milliseconds.
    Sheet(PathBuf, SheetGrid, u32),
    /// An untitled copy of a document pixl ships.
    Shipped(Shipped),
}

/// File work in progress.
#[derive(Resource)]
pub struct Files {
    /// Whether opening a document offers its newer recovery copy; the
    /// files settings turn recovery copies off.
    pub offer_recovery: bool,
    /// A request another dialog made, carried out on the next tick.
    pub queued: Option<Request>,
    /// The document pixl ships just opened, and its generation, for the
    /// tutorial to start the lesson over it.
    pub shipped: Option<(Shipped, u64)>,
    /// What belongs to the document in front.
    pub front: FileDocument,
}

/// The file state of one document: its recovery copy on offer and when it
/// was last written. A document keeps it while another one is in front.
#[derive(Clone, Debug, Default)]
pub struct FileDocument {
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
            shipped: None,
            front: FileDocument::default(),
        }
    }
}

impl FileDocument {
    /// Writes the recovery copy of `editor`'s document when it has edits
    /// the copy lacks, as autosave would have on its next turn. A document
    /// that goes behind another one's tab does this, so its copy holds
    /// every edit while it waits.
    pub fn write_now(&mut self, editor: &Editor, settings: &Settings) {
        if settings.autosave_every().is_none() || !editor.is_modified() {
            return;
        }
        let Some(path) = editor.path.as_ref() else {
            return;
        };
        if self
            .last_autosave
            .is_some_and(|(_, revision)| revision == editor.revision)
        {
            return;
        }
        if native::write_recovery(&editor.doc, path).is_ok() {
            self.last_autosave = Some((Instant::now(), editor.revision));
        }
    }
}

/// A document read and ready to come to the front in a tab of its own.
pub struct Opening {
    pub editor: Editor,
    /// Its file state: the newer recovery copy on offer, if any.
    pub file: FileDocument,
}

impl Opening {
    fn new(editor: Editor) -> Self {
        Self {
            editor,
            file: FileDocument::default(),
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

/// Reads the document at `path` for a tab of its own, with the colors and
/// tool of `window` (the editor in front). Its newer recovery copy is on
/// offer when `offer_recovery`.
pub fn open_path(
    window: &Editor,
    offer_recovery: bool,
    path: &Path,
) -> std::result::Result<Opening, String> {
    let opened = read_document(path)?;
    let native = !is_imported(path);
    // An imported file is a new, unsaved document: saving writes a .pixl,
    // never over the file it came from.
    let mut editor = Editor::opened(opened.doc, native.then(|| path.to_path_buf()), window);
    if !native {
        editor.imported_from = Some(path.to_path_buf());
    }
    editor.message = (!opened.warnings.is_empty()).then(|| opened.warnings.join("; "));
    let recovery = (native && offer_recovery)
        .then(|| native::find_recovery(path))
        .flatten()
        .map(|_| path.to_path_buf());
    Ok(Opening {
        editor,
        file: FileDocument {
            recovery,
            last_autosave: None,
        },
    })
}

/// Opens the recovery copy of the document at `path` in place of the
/// document in `editor`: the document keeps the file's path and reads as
/// modified until saved.
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

/// Reads the recovery copy of the document at `path` for a tab of its own.
pub fn recover_opening(window: &Editor, path: &Path) -> std::result::Result<Opening, String> {
    let loaded = native::load_file(&native::recovery_path(path))
        .map_err(|e| format!("could not recover: {e}"))?;
    let mut editor = Editor::opened(loaded.document, Some(path.to_path_buf()), window);
    editor.touch();
    Ok(Opening::new(editor))
}

/// An untitled copy of a document pixl ships, for a tab of its own: it has
/// no file, so saving asks where to and autosave never writes it.
pub fn open_shipped(window: &Editor, which: Shipped) -> std::result::Result<Opening, String> {
    let doc = which.load()?;
    let mut editor = Editor::opened(doc, None, window);
    editor.name = Some(which.name().to_string());
    Ok(Opening::new(editor))
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

    /// The new document, with the colors and tool of `window` (the editor
    /// in front).
    pub fn create(&self, window: &Editor) -> std::result::Result<Editor, String> {
        let mut doc =
            Document::new(self.width, self.height, self.mode).map_err(|e| e.to_string())?;
        doc.set_background(self.background);
        let mut editor = Editor::opened(doc, None, window);
        editor.name = Some(self.name.clone());
        Ok(editor)
    }
}

/// Carries out a request for a document.
fn start(
    request: Request,
    editor: &mut Editor,
    files: &mut Files,
    documents: &mut Documents,
    commands: &mut Commands,
    signals: &mut Signals,
) {
    match request {
        Request::New => {
            signals.set("new_name", "untitled".to_string());
            signals.set("new_open", true);
        }
        Request::Open => commands.queue(
            FileDialog::open()
                .filter(OPENABLE.0, OPENABLE.1)
                .filter(PIXL.0, PIXL.1)
                .filter(ASEPRITE.0, ASEPRITE.1)
                .filter(PNG.0, PNG.1)
                .filter(GIF.0, GIF.1)
                .then(|world: &mut World, paths| opened(world, paths)),
        ),
        Request::Recent(path) => {
            if let Some(id) = documents.showing(editor, &path) {
                documents.act(TabAction::Switch(id));
                return;
            }
            match open_path(editor, files.offer_recovery, &path) {
                Ok(opening) => {
                    if opening.file.recovery.is_some() {
                        signals.set("recover_open", true);
                    }
                    documents.act(TabAction::Open(Box::new(opening)));
                }
                Err(e) => editor.message = Some(e),
            }
        }
        Request::Recover(path) => {
            if let Some(id) = documents.showing(editor, &path) {
                // The tab that shows the file takes its recovery copy.
                documents.act(TabAction::Switch(id));
                documents.act(TabAction::Recover(path));
                return;
            }
            match recover_opening(editor, &path) {
                Ok(opening) => documents.act(TabAction::Open(Box::new(opening))),
                Err(e) => editor.message = Some(e),
            }
        }
        Request::Sheet(path, grid, ms) => {
            match crate::files::sheets::import(editor, &path, &grid, ms) {
                Ok(new) => documents.act(TabAction::Open(Box::new(Opening::new(new)))),
                Err(e) => editor.message = Some(e),
            }
        }
        Request::Shipped(which) => match open_shipped(editor, which) {
            Ok(opening) => {
                files.shipped = Some((which, opening.editor.generation));
                documents.act(TabAction::Open(Box::new(opening)));
            }
            Err(e) => editor.message = Some(e),
        },
    }
}

/// The Open dialog's answer: the document in a tab of its own, or the tab
/// that already shows it brought to the front.
fn opened(world: &mut World, paths: Vec<PathBuf>) {
    let Some(path) = paths.into_iter().next() else {
        return;
    };
    world.resource_scope(|world, mut documents: Mut<Documents>| {
        let offer = world.resource::<Files>().offer_recovery;
        let mut editor = world.resource_mut::<Editor>();
        if let Some(id) = documents.showing(&editor, &path) {
            documents.act(TabAction::Switch(id));
            return;
        }
        match open_path(&editor, offer, &path) {
            Ok(opening) => {
                let recover = opening.file.recovery.is_some();
                documents.act(TabAction::Open(Box::new(opening)));
                if recover {
                    world
                        .resource_mut::<PropertyStore>()
                        .set(PropertyKey::global("recover_open"), true.into());
                }
            }
            Err(e) => editor.message = Some(e),
        }
    });
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
    // The answer saves the document the dialog was opened for, in front
    // or not.
    let generation = editor.generation;
    commands.queue(
        FileDialog::save()
            .filter(PIXL.0, PIXL.1)
            .file_name(name)
            .then(move |world: &mut World, paths| {
                if let Some(path) = paths.into_iter().next() {
                    edit_document(world, generation, |editor| {
                        if let Err(e) = save_to(editor, &path) {
                            editor.message = Some(e);
                        }
                    });
                }
            }),
    );
}

fn export(commands: &mut Commands, editor: &Editor, gif: bool) {
    let (label, exts) = if gif { GIF } else { PNG };
    let generation = editor.generation;
    commands.queue(
        FileDialog::save()
            .filter(label, exts)
            .file_name(if gif { "untitled.gif" } else { "untitled.png" })
            .then(move |world: &mut World, paths| {
                if let Some(path) = paths.into_iter().next() {
                    edit_document(world, generation, |editor| {
                        if let Err(e) = export_to(editor, &path, gif) {
                            editor.message = Some(e);
                        }
                    });
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
    ids: ElementIds<'w, 's>,
    keys: Res<'w, KeyCommands>,
}

impl FileInput<'_, '_> {
    /// The ids of the commands chosen this tick. A shortcut runs the
    /// command of the menu item with its id.
    fn chosen(&mut self) -> Vec<String> {
        let mut chosen: Vec<String> = self.menu.read().map(|m| m.id.clone()).collect();
        let clicked: Vec<Entity> = self.clicks.read().map(|c| c.entity).collect();
        chosen.extend(clicked.into_iter().filter_map(|e| self.ids.id_of(e)));
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
    "help-rig-demo",
];

/// The commands the start screen takes; the editor's others wait until a
/// document is open.
fn on_start_screen(id: &str) -> bool {
    matches!(id, "file-new" | "file-open")
        || ["home-", "recent-", "new-", "recover-"]
            .iter()
            .any(|p| id.starts_with(p))
}

/// The File menu, its shortcuts, the start screen's buttons and list, and
/// the New and recovery dialogs.
#[allow(clippy::too_many_arguments)]
pub fn file_commands(
    mut editor: ResMut<Editor>,
    mut files: ResMut<Files>,
    mut home: ResMut<Home>,
    mut recents: ResMut<Recents>,
    mut documents: ResMut<Documents>,
    mut commands: Commands,
    mut input: FileInput,
    mut signals: Signals,
) {
    let editor = &mut *editor;
    let documents = &mut *documents;
    if let Some(path) = home.pending.take() {
        let before = documents.pending();
        start(
            Request::Recent(path),
            editor,
            &mut files,
            documents,
            &mut commands,
            &mut signals,
        );
        if documents.pending() == before {
            // The file did not open: the start screen says why.
            home.show(documents.count() > 0);
        }
    }
    if let Some(r) = files.queued.take() {
        start(
            r,
            editor,
            &mut files,
            documents,
            &mut commands,
            &mut signals,
        );
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
        let request = match id.as_str() {
            "menu-file" => {
                signals.toggle("__menu_open:file-menu");
                None
            }
            "file-new" | "home-new" => Some(Request::New),
            "file-open" | "home-open" => Some(Request::Open),
            "file-close" => {
                if let Some(front) = documents.front() {
                    documents.act(TabAction::Close(vec![front]));
                }
                None
            }
            "help-rig-demo" | "home-rig-demo" => Some(Request::Shipped(Shipped::RigDemo)),
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
                export(&mut commands, editor, false);
                None
            }
            "file-export-gif" => {
                export(&mut commands, editor, true);
                None
            }
            "new-create" => {
                signals.set("new_open", false);
                match NewProject::from_fields(&signals).and_then(|p| p.create(editor)) {
                    Ok(new) => documents.act(TabAction::Open(Box::new(Opening::new(new)))),
                    Err(e) => editor.message = Some(e),
                }
                None
            }
            "new-cancel" => {
                signals.set("new_open", false);
                None
            }
            "recover-yes" => {
                signals.set("recover_open", false);
                if let Some(path) = files.front.recovery.take() {
                    recover_path(editor, &path);
                }
                None
            }
            "recover-no" => {
                signals.set("recover_open", false);
                if let Some(path) = files.front.recovery.take() {
                    let _ = native::discard_recovery(&path);
                }
                None
            }
            _ => {
                if let Some(path) = listed("recent-remove-") {
                    recents.remove(&path);
                    None
                } else if let Some(path) = listed("recent-recover-") {
                    Some(Request::Recover(path))
                } else if let Some(path) = listed("recent-") {
                    if path.exists() {
                        Some(Request::Recent(path))
                    } else {
                        editor.message = Some(format!("{} is missing", path.display()));
                        None
                    }
                } else {
                    None
                }
            }
        };
        if let Some(r) = request {
            start(
                r,
                editor,
                &mut files,
                documents,
                &mut commands,
                &mut signals,
            );
        }
    }
}

/// Keeps the New project dialog showing what its fields say: the preset
/// that matches the size, and the background color field when the
/// background is a color. A preset's button fills in its size.
pub fn new_form(
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    mut signals: Signals,
    mut shown: Local<Option<(String, String, String)>>,
) {
    for click in clicks.read() {
        let Ok(id) = ids.of(click.entity) else {
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

/// Writes the recovery copy of the document in front when it is modified
/// and has a file, as often as the files settings say, unless they turn
/// recovery copies off. An untitled document has nowhere beside it to keep
/// one. A document behind another one's tab wrote its copy as it went
/// there, and does not change until it comes back.
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
    match files.front.last_autosave {
        None => files.front.last_autosave = Some((now, editor.saved_revision)),
        Some((at, revision)) if revision != editor.revision && now - at >= every => {
            if native::write_recovery(&editor.doc, path).is_ok() {
                files.front.last_autosave = Some((now, editor.revision));
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
