//! File > Scripts: candela scripts and extensions, run on the open
//! documents through `pixl-script`.
//!
//! The Scripts dialog lists the scripts in pixl's `scripts` folder, the
//! examples pixl ships, and the commands of the extensions installed in its
//! `extensions` folder, each also in the command palette. A run works on
//! copies of the open documents on a worker thread; when it ends, each
//! document it changed takes its edits back as one undo step, and the
//! documents it made or opened open in tabs of their own. A run that raises
//! an error changes no document.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lumenui::ecs::world::{Mut, World};
use lumenui::lumen_core::app_paths;
use lumenui::lumen_core::input::MenuClicked;
use lumenui::lumen_core::tick::WakeDeadline;
use lumenui::prelude::*;
use pixl_script::{
    Cancel, EXAMPLES, Extension, Library, Outcome, ScriptDoc, Session, extensions, run,
};

use crate::documents::{Documents, TabAction};
use crate::editor::Editor;
use crate::files::Opening;
use crate::files::recents::APP_ID;
use crate::home::Home;
use crate::ids::ElementIds;
use crate::menus::palette::{Entry as PaletteEntry, Run};
use crate::panels::escape;
use crate::settings::keymap::KeyCommands;

/// The command that opens the dialog.
pub const OPEN: &str = "file-scripts";

/// Where scripts and extensions live, and where the script library is
/// written.
#[derive(Clone, Debug)]
struct Folders {
    scripts: PathBuf,
    extensions: PathBuf,
    lib: PathBuf,
}

/// What an entry of the list runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    /// A script file.
    File(PathBuf),
    /// One of [`EXAMPLES`].
    Example(usize),
    /// An extension's command.
    Command { ext: usize, command: usize },
    /// An extension's palette, which goes on the document in front.
    Palette { ext: usize, palette: usize },
}

/// One row of the Scripts dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    /// "Script", "Example", or the extension's name.
    pub group: String,
    pub what: What,
}

/// A script running on a worker thread.
struct Running {
    run: Ran,
    cancel: Cancel,
    handle: JoinHandle<Outcome>,
}

/// What a run started with.
struct Ran {
    label: String,
    started: Instant,
    /// The revision of each document handed in, by its editor's generation:
    /// a document edited while the script ran does not take its edits.
    revisions: Vec<(u64, u64)>,
}

/// The scripts, the extensions and a run in progress.
#[derive(Resource)]
pub struct Scripts {
    folders: Option<Folders>,
    pub entries: Vec<Entry>,
    pub extensions: Vec<Extension>,
    /// Extensions that could not be read.
    pub problems: Vec<String>,
    pub selected: usize,
    scanned: bool,
    shown: Option<String>,
    running: Option<Running>,
    /// What the last run printed, raised or asked to show.
    pub report: Option<String>,
    /// An extension package waiting for the user to agree to install it.
    installing: Option<PathBuf>,
}

impl Scripts {
    fn with(folders: Option<Folders>) -> Self {
        Self {
            folders,
            entries: Vec::new(),
            extensions: Vec::new(),
            problems: Vec::new(),
            selected: 0,
            scanned: false,
            shown: None,
            running: None,
            report: None,
            installing: None,
        }
    }

    /// The `scripts` and `extensions` folders of `dir`.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self::with(Some(Folders {
            scripts: dir.join("scripts"),
            extensions: dir.join("extensions"),
            lib: dir,
        }))
    }

    /// pixl's folder of the user data directory.
    pub fn user() -> Self {
        Self::at(app_paths::data_dir_for(APP_ID))
    }

    /// No folders: the examples alone, as the tests and the bench run the
    /// editor.
    pub fn in_memory() -> Self {
        Self::with(None)
    }

    /// The scripts folder, when there is one.
    pub fn scripts_dir(&self) -> Option<&Path> {
        self.folders.as_ref().map(|f| f.scripts.as_path())
    }

    /// Asks the user whether to install the extension package at `path`.
    pub fn offer(&mut self, path: PathBuf) {
        self.installing = Some(path);
    }

    /// Whether a script is running.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Reads the scripts folder and the extensions again.
    pub fn rescan(&mut self) {
        let mut entries = Vec::new();
        self.extensions.clear();
        self.problems.clear();
        if let Some(f) = &self.folders {
            for s in pixl_script::list_scripts(&f.scripts) {
                entries.push(Entry {
                    label: s.name,
                    group: "Script".into(),
                    what: What::File(s.path),
                });
            }
            let (found, problems) = pixl_script::load_extensions(&f.extensions);
            self.extensions = found;
            self.problems = problems;
        }
        for (ext, e) in self.extensions.iter().enumerate() {
            for (command, c) in e.manifest.commands.iter().enumerate() {
                entries.push(Entry {
                    label: c.label.clone(),
                    group: e.manifest.name.clone(),
                    what: What::Command { ext, command },
                });
            }
            for (palette, p) in e.manifest.palettes.iter().enumerate() {
                entries.push(Entry {
                    label: format!("{} palette", p.name),
                    group: e.manifest.name.clone(),
                    what: What::Palette { ext, palette },
                });
            }
        }
        for (i, x) in EXAMPLES.iter().enumerate() {
            entries.push(Entry {
                label: x.label.into(),
                group: "Example".into(),
                what: What::Example(i),
            });
        }
        self.entries = entries;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.scanned = true;
        self.shown = None;
    }

    fn ensure_scanned(&mut self) {
        if !self.scanned {
            self.rescan();
        }
    }

    /// The script library, written into pixl's data folder or, without
    /// one, the temporary folder.
    fn library(&self) -> std::result::Result<Library, String> {
        let root = self
            .folders
            .as_ref()
            .map_or_else(std::env::temp_dir, |f| f.lib.clone());
        Library::at(&root).map_err(|e| format!("Could not set up scripting: {e}"))
    }

    /// Starts `entry` over the open documents.
    fn start(&mut self, entry: &Entry, docs: &Documents, front: &Editor, home: bool) -> String {
        if self.running.is_some() {
            return "A script is still running".into();
        }
        let (source, path, params) = match &entry.what {
            What::File(path) => match std::fs::read_to_string(path) {
                Ok(s) => (s, path.clone(), BTreeMap::new()),
                Err(e) => return format!("Could not read {}: {e}", path.display()),
            },
            What::Example(i) => {
                let x = &EXAMPLES[*i];
                let dir = self
                    .scripts_dir()
                    .map_or_else(std::env::temp_dir, Path::to_path_buf);
                (x.source.to_string(), dir.join(x.file), BTreeMap::new())
            }
            What::Command { ext, command } => {
                let e = &self.extensions[*ext];
                let c = &e.manifest.commands[*command];
                let path = e.script_path(c);
                match std::fs::read_to_string(&path) {
                    Ok(s) => (s, path, c.params.clone()),
                    Err(err) => return format!("Could not read {}: {err}", path.display()),
                }
            }
            What::Palette { .. } => return "A palette is not a script".into(),
        };
        let lib = match self.library() {
            Ok(lib) => lib,
            Err(e) => return e,
        };
        let (mut session, revisions) = snapshot(docs, front, home);
        session.params = params;
        session.base = session
            .active
            .and_then(|i| session.docs[i].path.as_ref())
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .or_else(|| self.scripts_dir().map(Path::to_path_buf))
            .unwrap_or_else(std::env::temp_dir);
        let cancel = session.cancel.clone();
        let handle = std::thread::spawn(move || run(&source, &path, session, &lib));
        self.running = Some(Running {
            run: Ran {
                label: entry.label.clone(),
                started: Instant::now(),
                revisions,
            },
            cancel,
            handle,
        });
        format!("Running {}...", entry.label)
    }

    /// The finished run, if the script ended.
    fn finished(&mut self) -> Option<(Ran, Outcome)> {
        if !self
            .running
            .as_ref()
            .is_some_and(|r| r.handle.is_finished())
        {
            return None;
        }
        let Running { run, handle, .. } = self.running.take()?;
        let outcome = handle.join().unwrap_or_else(|_| Outcome {
            session: Session::default(),
            output: String::new(),
            error: Some(pixl_script::ScriptError {
                message: "the script engine stopped".into(),
                code: "panic".into(),
                file: String::new(),
                line: None,
            }),
        });
        Some((run, outcome))
    }

    /// The command palette's entries for the scripts and extension
    /// commands.
    pub fn palette_entries(&mut self) -> Vec<PaletteEntry> {
        self.ensure_scanned();
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| PaletteEntry {
                label: e.label.clone(),
                group: match e.what {
                    What::File(_) | What::Example(_) => "Script".into(),
                    What::Command { .. } | What::Palette { .. } => "Extension".into(),
                },
                keys: e.group.clone(),
                run: Run::Item(format!("script-run-{i}")),
            })
            .collect()
    }
}

impl Default for Scripts {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// The open documents as a run sees them, and the revision each was at.
fn snapshot(docs: &Documents, front: &Editor, home: bool) -> (Session, Vec<(u64, u64)>) {
    let mut session = Session {
        fg: front.fg,
        bg: front.bg,
        ui: true,
        ..Session::default()
    };
    let mut revisions = Vec::new();
    let front_id = docs.front();
    for (id, e) in docs.editors(front) {
        let mut d = ScriptDoc::new(Some(e.generation), e.doc.clone());
        d.selection = e.selection.clone();
        d.path = e.path.clone();
        d.name = Some(e.name());
        d.frame = e.frame;
        d.layer = e.layer;
        let i = session.add(d);
        if Some(id) == front_id && !home {
            session.active = Some(i);
        }
        revisions.push((e.generation, e.revision));
    }
    if home || front_id.is_none() {
        session.active = None;
    }
    (session, revisions)
}

/// Puts what a run left into the editors: one undo step per changed
/// document, a tab for each document it made, its colors, and its report.
fn apply(world: &mut World, running: Ran, outcome: Outcome) {
    let Outcome {
        session,
        output,
        error,
    } = outcome;
    let label = format!("Script: {}", running.label);
    let mut skipped = Vec::new();
    let mut opened = Vec::new();
    let active_key = session
        .active
        .and_then(|i| session.docs.get(i))
        .and_then(|d| d.key);
    // A run that raised takes nothing back: its edits go with it, as an
    // undo step that never happened.
    let docs = if error.is_some() {
        Vec::new()
    } else {
        session.docs
    };
    for d in docs {
        match d.key {
            Some(generation) if d.changed || d.saved => {
                let was = running
                    .revisions
                    .iter()
                    .find(|(g, _)| *g == generation)
                    .map(|(_, r)| *r);
                let title = d.title();
                crate::documents::edit_document(world, generation, |editor| {
                    if Some(editor.revision) != was {
                        skipped.push(title);
                        return;
                    }
                    if d.changed {
                        editor.apply_script(d.doc, d.selection, d.layer, d.frame, &label);
                    }
                    if d.saved && d.path.is_some() {
                        editor.path = d.path;
                        editor.saved_revision = editor.revision;
                    }
                });
            }
            Some(_) => {}
            None if d.created => opened.push(d),
            None => {}
        }
    }
    let window = world.resource::<Editor>();
    let fg_bg = if error.is_some() {
        (window.fg, window.bg)
    } else {
        (session.fg, session.bg)
    };
    let mut openings = Vec::new();
    for d in opened {
        let mut editor = Editor::opened(d.doc, d.path.clone(), window);
        if d.path.is_none() {
            editor.name = d.name;
        }
        editor.selection = d.selection;
        editor.layer = d.layer;
        editor.frame = d.frame;
        if !d.saved {
            editor.touch();
        }
        openings.push(editor);
    }
    let made = openings.len();
    {
        let mut front = world.resource_mut::<Editor>();
        front.fg = fg_bg.0;
        front.bg = fg_bg.1;
    }
    // A run whose last word was an existing document brings it to the
    // front; documents it made open in front of the rest.
    let switch = if made == 0 && error.is_none() {
        let docs = world.resource::<Documents>();
        let front = world.resource::<Editor>();
        active_key.and_then(|g| {
            docs.editors(front)
                .find(|(_, e)| e.generation == g)
                .map(|(id, _)| id)
                .filter(|id| Some(*id) != docs.front())
        })
    } else {
        None
    };
    {
        let mut docs = world.resource_mut::<Documents>();
        for editor in openings {
            docs.act(TabAction::Open(Box::new(Opening::new(editor))));
        }
        if let Some(id) = switch {
            docs.act(TabAction::Switch(id));
        }
    }
    let mut report = String::new();
    if !output.is_empty() {
        report.push_str(&output);
    }
    for a in &session.alerts {
        report.push_str(a);
        report.push('\n');
    }
    let message = match &error {
        Some(e) => {
            report.push_str(&format!("{e}\n"));
            if e.code == "cancelled" {
                format!("Stopped {}", running.label)
            } else {
                format!("{} failed, nothing changed: {}", running.label, e.message)
            }
        }
        None if !skipped.is_empty() => format!(
            "Ran {}; {} changed while it ran and kept its own edits",
            running.label,
            skipped.join(", ")
        ),
        None => format!(
            "Ran {} in {} ms",
            running.label,
            running.started.elapsed().as_millis()
        ),
    };
    world.resource_mut::<Editor>().message = Some(message);
    let mut scripts = world.resource_mut::<Scripts>();
    scripts.report = (!report.trim().is_empty()).then(|| report.trim_end().to_string());
}

/// Opens a folder in the system's file manager.
fn reveal(dir: &Path) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(dir)
        .spawn()
        .map(drop)
        .map_err(|e| format!("could not open {}: {e}", dir.display()))
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
    if keys.ran(OPEN, false) {
        out.push(OPEN.to_string());
    }
    out
}

/// The Scripts dialog, the palette's script entries, and starting runs.
#[allow(clippy::too_many_arguments)]
pub fn script_input(
    mut scripts: ResMut<Scripts>,
    mut editor: ResMut<Editor>,
    documents: Res<Documents>,
    home: Res<Home>,
    mut menu: MessageReader<MenuClicked>,
    mut clicks: MessageReader<ClickEvent>,
    ids: ElementIds,
    keys: Res<KeyCommands>,
    mut commands: Commands,
    mut signals: Signals,
) {
    let chosen = chosen(&mut menu, &mut clicks, &ids, &keys);
    for id in chosen {
        let run_entry = |scripts: &mut Scripts, editor: &mut Editor, i: usize| {
            scripts.ensure_scanned();
            if let Some(entry) = scripts.entries.get(i).cloned() {
                match entry.what {
                    What::Palette { ext, palette } => {
                        let e = &scripts.extensions[ext];
                        let path = e.palette_path(&e.manifest.palettes[palette]);
                        match crate::menus::image::load_palette_file(&path) {
                            Ok(p) if !home.open && documents.front().is_some() => {
                                crate::menus::image::use_palette(editor, &entry.label, p);
                            }
                            Ok(_) => editor.message = Some("Open a document first".into()),
                            Err(e) => editor.message = Some(e),
                        }
                    }
                    _ => {
                        editor.message = Some(scripts.start(&entry, &documents, editor, home.open));
                    }
                }
            }
        };
        match id.as_str() {
            OPEN => {
                scripts.rescan();
                signals.set("sc_open", true);
            }
            "sc-close" => signals.set("sc_open", false),
            "sc-rescan" => scripts.rescan(),
            "sc-run" => {
                let i = scripts.selected;
                run_entry(&mut scripts, &mut editor, i);
            }
            "sc-stop" => {
                if let Some(r) = &scripts.running {
                    r.cancel.cancel();
                }
            }
            "sc-folder" => {
                let dir = scripts.scripts_dir().map(Path::to_path_buf);
                match dir {
                    Some(dir) => {
                        if let Err(e) = reveal(&dir) {
                            editor.message = Some(e);
                        }
                    }
                    None => editor.message = Some("This window keeps no scripts folder".into()),
                }
            }
            "sc-run-file" => {
                commands.queue(FileDialog::open().filter("candela scripts", &["cdl"]).then(
                    |world: &mut World, paths| {
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        let entry = Entry {
                            label: path
                                .file_stem()
                                .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
                            group: "Script".into(),
                            what: What::File(path),
                        };
                        let home = world.resource::<Home>().open;
                        let message = world.resource_scope(|world, mut scripts: Mut<Scripts>| {
                            let docs = world.resource::<Documents>();
                            let front = world.resource::<Editor>();
                            scripts.start(&entry, docs, front, home)
                        });
                        world.resource_mut::<Editor>().message = Some(message);
                    },
                ))
            }
            "sc-install" => commands.queue(
                FileDialog::open()
                    .filter("pixl extensions", &["pixl-extension", "zip"])
                    .then(|world: &mut World, paths| {
                        if let Some(path) = paths.into_iter().next() {
                            world.resource_mut::<Scripts>().offer(path);
                        }
                    }),
            ),
            "sc-install-yes" => {
                if let Some(path) = scripts.installing.take() {
                    signals.set("sc_confirm_open", false);
                    let into = scripts.folders.as_ref().map(|f| f.extensions.clone());
                    editor.message = Some(match into {
                        Some(dir) => match std::fs::create_dir_all(&dir)
                            .map_err(|e| e.to_string())
                            .and_then(|_| extensions::install(&path, &dir))
                        {
                            Ok(ext) => format!("Installed {}", ext.manifest.name),
                            Err(e) => format!("Could not install: {e}"),
                        },
                        None => "This window keeps no extensions folder".into(),
                    });
                    scripts.rescan();
                }
            }
            "sc-install-no" => {
                scripts.installing = None;
                signals.set("sc_confirm_open", false);
            }
            "sc-remove" => {
                let ext = match scripts.entries.get(scripts.selected).map(|e| &e.what) {
                    Some(What::Command { ext, .. } | What::Palette { ext, .. }) => Some(*ext),
                    _ => None,
                };
                match ext.and_then(|i| scripts.extensions.get(i)) {
                    Some(e) => {
                        let name = e.manifest.name.clone();
                        editor.message = Some(match extensions::uninstall(e) {
                            Ok(()) => format!("Removed {name}"),
                            Err(e) => e,
                        });
                        scripts.rescan();
                    }
                    None => {
                        editor.message = Some("Pick an extension's command to remove it".into())
                    }
                }
            }
            "so-close" => {
                scripts.report = None;
                signals.set("so_open", false);
                signals.set("so_text", String::new());
            }
            other => {
                if let Some(i) = other
                    .strip_prefix("sc-item-")
                    .and_then(|i| i.parse::<usize>().ok())
                    .filter(|&i| i < scripts.entries.len())
                {
                    scripts.selected = i;
                    scripts.shown = None;
                } else if let Some(i) = other
                    .strip_prefix("script-run-")
                    .and_then(|i| i.parse::<usize>().ok())
                {
                    run_entry(&mut scripts, &mut editor, i);
                }
            }
        }
    }
}

/// Lists the scripts, shows a run in progress, takes a finished run back
/// into the documents, and asks before an extension installs.
pub fn show_scripts(
    mut scripts: ResMut<Scripts>,
    mut commands: Commands,
    mut signals: Signals,
    wake: Option<Res<WakeDeadline>>,
) {
    if let Some((running, outcome)) = scripts.finished() {
        commands.queue(move |world: &mut World| apply(world, running, outcome));
    }
    let busy = scripts.running.as_ref().map(|r| r.run.label.clone());
    if signals.get::<bool>("sc_busy") != Some(busy.is_some()) {
        signals.set("sc_busy", busy.is_some());
    }
    if let Some(label) = busy {
        let text = format!("Running {label}...");
        if signals.get::<String>("sc_busy_text").as_deref() != Some(text.as_str()) {
            signals.set("sc_busy_text", text);
        }
        if let Some(wake) = &wake {
            wake.request(Instant::now() + Duration::from_millis(15));
        }
    }
    if let Some(report) = scripts.report.clone()
        && signals.get::<String>("so_text").as_deref() != Some(report.as_str())
    {
        signals.set("so_text", report);
        signals.set("so_open", true);
        crate::repaint_soon(&wake);
    }
    if let Some(path) = scripts.installing.clone()
        && signals.get::<bool>("sc_confirm_open") != Some(true)
    {
        let text = match extensions::read_package(&path) {
            Ok((m, _)) => format!(
                "Install {}{}? Its commands are scripts: they can change your documents and read and write any file, as any program you run can.{}",
                m.name,
                if m.version.is_empty() {
                    String::new()
                } else {
                    format!(" {}", m.version)
                },
                if m.description.is_empty() {
                    String::new()
                } else {
                    format!("\n\n{}", m.description)
                }
            ),
            Err(e) => {
                scripts.installing = None;
                signals.set("sc_confirm_text", String::new());
                signals.set("so_text", format!("Could not install: {e}"));
                signals.set("so_open", true);
                return;
            }
        };
        signals.set("sc_confirm_text", text);
        signals.set("sc_confirm_open", true);
        crate::repaint_soon(&wake);
    }
    if signals.get::<bool>("sc_open") != Some(true) {
        return;
    }
    let mut markup = String::new();
    for (i, e) in scripts.entries.iter().enumerate() {
        let current = if i == scripts.selected {
            " current"
        } else {
            ""
        };
        markup.push_str(&format!(
            r#"<row id="sc-item-{i}" class="sc-item{current}"><label class="sc-label" text="{}"/><spacer/><label class="sc-group" text="{}"/></row>"#,
            escape(&e.label),
            escape(&e.group),
        ));
    }
    for p in &scripts.problems {
        markup.push_str(&format!(
            r#"<label class="sc-problem" text="{}"/>"#,
            escape(p)
        ));
    }
    if scripts.shown.as_deref() != Some(markup.as_str())
        && let Some(list) = dom::get_by_id("sc-list")
    {
        list.set_inner_markup(&markup);
        scripts.shown = Some(markup);
        let folder = scripts.scripts_dir().map_or_else(
            || "No scripts folder".to_string(),
            |d| format!("Scripts folder: {}", d.display()),
        );
        signals.set("sc_folder", folder);
        crate::repaint_soon(&wake);
    }
}
