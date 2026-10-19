//! What a script run sees and leaves behind: the open documents, the colors
//! and the parameters it was given. The host hands a session in, the run
//! edits it, and the host reads back what changed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pixl_document::{Document, LayerId, Rgba, Selection};

/// One document a run can reach.
#[derive(Clone, Debug)]
pub struct ScriptDoc {
    /// The host's name for the document, handed back untouched: the editor
    /// keeps its editor's generation here. Documents the run makes have
    /// none.
    pub key: Option<u64>,
    pub doc: Document,
    /// The document as it was handed in or last saved by the run: what a
    /// save settles tilemap edits against.
    pub origin: Document,
    pub selection: Option<Selection>,
    /// The file the document is saved to.
    pub path: Option<PathBuf>,
    /// The name of a document that has no file yet.
    pub name: Option<String>,
    /// The frame and layer the editor shows and paints on.
    pub frame: usize,
    pub layer: LayerId,
    /// The run changed the document, its selection, or the active frame
    /// or layer.
    pub changed: bool,
    /// The run made or opened the document.
    pub created: bool,
    /// The run saved the document to `path` in pixl's format, and nothing
    /// changed it since.
    pub saved: bool,
}

impl ScriptDoc {
    /// A document as the host has it open.
    pub fn new(key: Option<u64>, doc: Document) -> Self {
        let layer = doc
            .walk()
            .iter()
            .rev()
            .find(|(_, l)| l.is_raster())
            .map_or(LayerId(0), |(_, l)| l.id);
        Self {
            key,
            origin: doc.clone(),
            doc,
            selection: None,
            path: None,
            name: None,
            frame: 0,
            layer,
            changed: false,
            created: false,
            saved: false,
        }
    }

    /// The document's name: its file's name, else the name it was made
    /// with, else "untitled".
    pub fn title(&self) -> String {
        if let Some(n) = self.path.as_ref().and_then(|p| p.file_name()) {
            return n.to_string_lossy().into_owned();
        }
        self.name.clone().unwrap_or_else(|| "untitled".to_string())
    }

    /// Notes an edit: the document differs from what the host gave, and
    /// from its file.
    pub fn touch(&mut self) {
        self.changed = true;
        self.saved = false;
    }
}

/// Everything a run works on.
#[derive(Clone, Debug)]
pub struct Session {
    pub docs: Vec<ScriptDoc>,
    /// The document scripts get from `app::document()`.
    pub active: Option<usize>,
    pub fg: Rgba,
    pub bg: Rgba,
    /// `app::params()`: the CLI's `--script-param`s or an extension
    /// command's `params`.
    pub params: BTreeMap<String, String>,
    /// Whether an editor window runs the script (`app::has_ui()`).
    pub ui: bool,
    /// The folder relative paths in the API are read against: the current
    /// folder for pixl-cli, the script's own folder in the editor.
    pub base: PathBuf,
    /// What `app::alert` asked to show, in order.
    pub alerts: Vec<String>,
    /// Set from another thread to stop the run at its next call into pixl.
    pub cancel: Cancel,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            docs: Vec::new(),
            active: None,
            fg: Rgba::BLACK,
            bg: Rgba::WHITE,
            params: BTreeMap::new(),
            ui: false,
            base: PathBuf::from("."),
            alerts: Vec::new(),
            cancel: Cancel::default(),
        }
    }
}

impl Session {
    /// Adds a document and returns its index; the first one added becomes
    /// the active one.
    pub fn add(&mut self, doc: ScriptDoc) -> usize {
        self.docs.push(doc);
        let i = self.docs.len() - 1;
        if self.active.is_none() {
            self.active = Some(i);
        }
        i
    }

    /// Whether the run changed anything a host has to take back.
    pub fn changed(&self) -> bool {
        self.docs.iter().any(|d| d.changed || d.created)
    }
}

/// A flag that stops a run at its next call into pixl.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
