//! Documents on disk: new, open, save and save as for `.pixl` files, import
//! from PNG and Aseprite, export to PNG and GIF, and the autosave recovery
//! copy pixl-core keeps beside a saved document.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lumenui::ecs::change_detection::Mut;
use lumenui::ecs::system::SystemParam;
use lumenui::ecs::world::World;
use lumenui::lumen_core::input::{MenuClicked, ModifiersState};
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_core::gif::{GifOptions, export_gif};
use pixl_core::io::{export_frame_png, import_png};
use pixl_core::native::{self, SaveOptions};
use pixl_core::{ColorMode, Document, aseprite};

use crate::editor::Editor;
use crate::input::{KeyInput, key_char};

/// How often a modified document with a file writes its recovery copy.
pub const AUTOSAVE_EVERY: Duration = Duration::from_secs(30);

/// Something that replaces the document, waiting for the user to agree to
/// lose unsaved changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replace {
    New,
    Open,
    ImportPng,
    ImportAseprite,
}

/// File work in progress.
#[derive(Resource, Default)]
pub struct Files {
    /// A replacement waiting on the discard question.
    pub confirming: Option<Replace>,
    /// The document whose newer recovery copy is on offer.
    pub recovery: Option<PathBuf>,
    /// When the recovery copy was last written, and for which revision.
    last_autosave: Option<(Instant, u64)>,
}

/// The file kinds the dialogs offer.
const PIXL: (&str, &[&str]) = ("pixl documents", &["pixl"]);
const PNG: (&str, &[&str]) = ("PNG images", &["png"]);
const ASEPRITE: (&str, &[&str]) = ("Aseprite files", &["aseprite", "ase"]);
const GIF: (&str, &[&str]) = ("GIF animations", &["gif"]);

/// A document read from a file, with what the reader could not use.
pub struct Opened {
    pub doc: Document,
    pub warnings: Vec<String>,
}

/// Reads a document: `.pixl` natively, `.aseprite` / `.ase` and `.png`
/// through their importers.
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

/// Opens a `.pixl` document into the editor, offering its recovery copy
/// when one is newer than the file.
pub fn open_path(editor: &mut Editor, files: &mut Files, path: &Path) {
    match read_document(path) {
        Ok(opened) => {
            let native = !matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("png" | "aseprite" | "ase")
            );
            // An imported file is a new, unsaved document: saving writes a
            // .pixl, never over the file it came from.
            let path_kept = native.then(|| path.to_path_buf());
            editor.replace(opened.doc, path_kept);
            editor.message = (!opened.warnings.is_empty()).then(|| opened.warnings.join("; "));
            files.recovery = native
                .then(|| native::find_recovery(path))
                .flatten()
                .map(|_| path.to_path_buf());
            files.last_autosave = None;
        }
        Err(e) => editor.message = Some(e),
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
fn with_extension(path: &Path, ext: &str) -> PathBuf {
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

/// Makes a new document from the size fields of the New dialog.
fn create_from_fields(editor: &mut Editor, signals: &Signals) {
    let side = |name: &str| {
        signals
            .get::<String>(name)
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|v| (1..=crate::MAX_VIEW_SIDE).contains(v))
    };
    match (side("new_width"), side("new_height")) {
        (Some(w), Some(h)) => match Document::new(w, h, ColorMode::Rgba) {
            Ok(doc) => editor.replace(doc, None),
            Err(e) => editor.message = Some(e.to_string()),
        },
        _ => {
            editor.message = Some(format!(
                "A new document is 1 to {} pixels on each side",
                crate::MAX_VIEW_SIDE
            ))
        }
    }
}

fn start(replace: Replace, commands: &mut Commands, signals: &mut Signals) {
    match replace {
        Replace::New => signals.set("new_open", true),
        Replace::Open => commands.queue(
            FileDialog::open()
                .filter(PIXL.0, PIXL.1)
                .then(|world: &mut World, paths| opened(world, paths)),
        ),
        Replace::ImportPng => commands.queue(
            FileDialog::open()
                .filter(PNG.0, PNG.1)
                .then(|world: &mut World, paths| opened(world, paths)),
        ),
        Replace::ImportAseprite => commands.queue(
            FileDialog::open()
                .filter(ASEPRITE.0, ASEPRITE.1)
                .then(|world: &mut World, paths| opened(world, paths)),
        ),
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
/// button, or a shortcut.
#[derive(SystemParam)]
pub struct FileInput<'w, 's> {
    menu: MessageReader<'w, 's, MenuClicked>,
    clicks: MessageReader<'w, 's, ClickEvent>,
    ids: Query<'w, 's, &'static LumenId>,
    keys: KeyInput<'w, 's>,
    modifiers: Res<'w, ModifiersState>,
}

impl FileInput<'_, '_> {
    /// The ids of the commands chosen this tick.
    fn chosen(&mut self) -> Vec<String> {
        let mut chosen: Vec<String> = self.menu.read().map(|m| m.id.clone()).collect();
        for click in self.clicks.read() {
            if let Ok(id) = self.ids.get(click.entity) {
                chosen.push(id.0.clone());
            }
        }
        let typing = self.keys.typing();
        for k in self.keys.pressed.read() {
            let ctrl = k.modifiers.ctrl || k.modifiers.super_ || self.modifiers.0.ctrl;
            if typing || !ctrl || k.repeat {
                continue;
            }
            let shift = k.modifiers.shift;
            // With Ctrl held a platform may report the control character
            // instead of the letter.
            let id = match key_char(&k.key).as_deref() {
                Some("n" | "\x0e") => "file-new",
                Some("o" | "\x0f") => "file-open",
                Some("s" | "\x13") if shift => "file-save-as",
                Some("s" | "\x13") => "file-save",
                Some("e" | "\x05") => "file-export-png",
                _ => continue,
            };
            chosen.push(id.to_string());
        }
        chosen
    }
}

/// The File menu, its shortcuts, and the New, discard and recovery dialogs.
pub fn file_commands(
    mut editor: ResMut<Editor>,
    mut files: ResMut<Files>,
    mut commands: Commands,
    mut input: FileInput,
    mut signals: Signals,
) {
    let chosen = input.chosen();
    let editor = &mut *editor;
    for id in chosen {
        let replace = match id.as_str() {
            "menu-file" => {
                signals.toggle("__menu_open:file-menu");
                None
            }
            "file-new" => Some(Replace::New),
            "file-open" => Some(Replace::Open),
            "file-import-png" => Some(Replace::ImportPng),
            "file-import-aseprite" => Some(Replace::ImportAseprite),
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
                    start(r, &mut commands, &mut signals);
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
                    match native::load_file(&native::recovery_path(&path)) {
                        Ok(loaded) => {
                            editor.replace(loaded.document, Some(path));
                            // The recovered edits are not in the file yet.
                            editor.touch();
                        }
                        Err(e) => editor.message = Some(format!("could not recover: {e}")),
                    }
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
            _ => None,
        };
        if let Some(r) = replace {
            if editor.is_modified() {
                files.confirming = Some(r);
                signals.set("discard_open", true);
            } else {
                start(r, &mut commands, &mut signals);
            }
        }
    }
}

/// Writes the recovery copy of a modified document that has a file, every
/// [`AUTOSAVE_EVERY`]. An untitled document has nowhere beside it to keep
/// one.
pub fn autosave(editor: Res<Editor>, mut files: ResMut<Files>, wake: Option<Res<WakeDeadline>>) {
    let Some(path) = editor.path.as_ref() else {
        return;
    };
    if !editor.is_modified() {
        return;
    }
    let now = Instant::now();
    match files.last_autosave {
        None => files.last_autosave = Some((now, editor.saved_revision)),
        Some((at, revision)) if revision != editor.revision && now - at >= AUTOSAVE_EVERY => {
            if native::write_recovery(&editor.doc, path).is_ok() {
                files.last_autosave = Some((now, editor.revision));
            }
        }
        Some((at, _)) => {
            if let Some(wake) = wake {
                wake.request(at + AUTOSAVE_EVERY);
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
