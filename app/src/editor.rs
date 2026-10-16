//! The open document and everything the editor knows about editing it: the
//! history, the active layer and frame, the colors, the tool and its options,
//! and the selection.

use std::path::PathBuf;

use lumenui::prelude::*;
use pixl_core::{
    ColorMode, Document, History, LayerId, LayerKind, NewLayer, Placement, Rect, Rgba, Scope,
    Selection,
};

/// The tools of the tool bar, with the key that picks each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tool {
    #[default]
    Pencil,
    Eraser,
    Line,
    Rectangle,
    Ellipse,
    Bucket,
    Eyedropper,
    Select,
    Move,
}

impl Tool {
    pub const ALL: [Tool; 9] = [
        Tool::Pencil,
        Tool::Eraser,
        Tool::Line,
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Bucket,
        Tool::Eyedropper,
        Tool::Select,
        Tool::Move,
    ];

    /// The id of the tool's button in the tool bar.
    pub fn button_id(self) -> &'static str {
        match self {
            Tool::Pencil => "tool-pencil",
            Tool::Eraser => "tool-eraser",
            Tool::Line => "tool-line",
            Tool::Rectangle => "tool-rect",
            Tool::Ellipse => "tool-ellipse",
            Tool::Bucket => "tool-bucket",
            Tool::Eyedropper => "tool-eyedropper",
            Tool::Select => "tool-select",
            Tool::Move => "tool-move",
        }
    }

    pub fn from_button_id(id: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.button_id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Pencil => "Pencil",
            Tool::Eraser => "Eraser",
            Tool::Line => "Line",
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Bucket => "Bucket",
            Tool::Eyedropper => "Eyedropper",
            Tool::Select => "Select",
            Tool::Move => "Move",
        }
    }
}

/// The settings the tool options bar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolOptions {
    /// Brush edge in pixels, for the pencil, the eraser and shape outlines.
    pub size: u32,
    /// Drop the L-shaped corners of one-pixel strokes.
    pub pixel_perfect: bool,
    /// Fill rectangles and ellipses instead of outlining them.
    pub filled: bool,
    /// Bucket: the largest per-channel difference that still fills.
    pub tolerance: u8,
    /// Bucket: grow from the clicked pixel, or take every match.
    pub contiguous: bool,
    /// Pen pressure scales the brush size.
    pub pressure_size: bool,
}

impl Default for ToolOptions {
    fn default() -> Self {
        Self {
            size: 1,
            pixel_perfect: true,
            filled: false,
            tolerance: 0,
            contiguous: true,
            pressure_size: true,
        }
    }
}

/// The document of this window and the state of editing it.
#[derive(Resource)]
pub struct Editor {
    pub doc: Document,
    pub history: History,
    /// Where the document was opened from or last saved to.
    pub path: Option<PathBuf>,
    /// The PNG, GIF or Aseprite file an imported document was read from. It
    /// is not the document's file: saving never writes to it.
    pub imported_from: Option<PathBuf>,
    /// The name a new document was given, until it is saved to a file.
    pub name: Option<String>,
    pub frame: usize,
    /// The layer tools paint on.
    pub layer: LayerId,
    pub fg: Rgba,
    pub bg: Rgba,
    pub tool: Tool,
    pub options: ToolOptions,
    pub selection: Option<Selection>,
    /// Bumped on every change to the document; views compare it.
    pub revision: u64,
    /// The revision that matches the file on disk.
    pub saved_revision: u64,
    /// Bumped when a new document replaces this one.
    pub generation: u64,
    /// A line for the status bar, such as a failed save.
    pub message: Option<String>,
    /// What changed since a view last looked.
    touched: Touched,
}

/// What changed in a document since a view last looked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Touched {
    /// Anything may have changed.
    #[default]
    All,
    /// Only the pixels of these layers.
    Layers(Vec<LayerId>),
}

impl Editor {
    /// An editor over `doc`, with its top raster layer active.
    pub fn new(doc: Document) -> Self {
        let layer = top_raster(&doc).unwrap_or(LayerId(0));
        Self {
            doc,
            history: History::default(),
            path: None,
            imported_from: None,
            name: None,
            frame: 0,
            layer,
            fg: Rgba::BLACK,
            bg: Rgba::WHITE,
            tool: Tool::default(),
            options: ToolOptions::default(),
            selection: None,
            revision: 1,
            saved_revision: 1,
            generation: 0,
            message: None,
            touched: Touched::All,
        }
    }

    /// A fresh 64x64 RGBA document.
    pub fn blank() -> Self {
        Self::new(Document::new(64, 64, ColorMode::Rgba).expect("64x64 is a valid size"))
    }

    /// Replaces the document, keeping the colors, tool and options.
    pub fn replace(&mut self, doc: Document, path: Option<PathBuf>) {
        let mut next = Editor::new(doc);
        next.fg = self.fg;
        next.bg = self.bg;
        next.tool = self.tool;
        next.options = self.options;
        next.path = path;
        next.generation = self.generation + 1;
        next.revision = self.revision + 1;
        next.saved_revision = next.revision;
        *self = next;
    }

    /// Records that the document changed, in ways a view must look for
    /// everywhere: structure, properties, any layer's pixels.
    pub fn touch(&mut self) {
        self.revision += 1;
        self.touched = Touched::All;
    }

    /// Records that only the active layer's pixels changed, so a view needs
    /// to look at that layer alone.
    pub fn touch_pixels(&mut self) {
        self.revision += 1;
        let layer = self.layer;
        if let Touched::Layers(ids) = &mut self.touched
            && !ids.contains(&layer)
        {
            ids.push(layer);
        }
    }

    /// What changed since the last call, for a view to bring itself up to
    /// date with.
    pub fn take_touched(&mut self) -> Touched {
        std::mem::replace(&mut self.touched, Touched::Layers(Vec::new()))
    }

    pub fn is_modified(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// The document's name: its file's name, else the name it was made
    /// with, else "untitled".
    pub fn name(&self) -> String {
        if let Some(n) = self.path.as_ref().and_then(|p| p.file_name()) {
            return n.to_string_lossy().into_owned();
        }
        self.name.clone().unwrap_or_else(|| "untitled".to_string())
    }

    /// The file the start screen lists this document under: its own file,
    /// or the file it was imported from.
    pub fn recent_path(&self) -> Option<&PathBuf> {
        self.path.as_ref().or(self.imported_from.as_ref())
    }

    /// The document's name for the title bar.
    pub fn title(&self) -> String {
        let name = self.name();
        if self.is_modified() {
            format!("{name} *")
        } else {
            name
        }
    }

    pub fn undo(&mut self) -> Option<Rect> {
        let dirty = self.history.undo(&mut self.doc)?;
        self.after_structure_change();
        self.touch();
        Some(dirty)
    }

    pub fn redo(&mut self) -> Option<Rect> {
        let dirty = self.history.redo(&mut self.doc)?;
        self.after_structure_change();
        self.touch();
        Some(dirty)
    }

    /// Keeps the active layer pointing at a layer that exists.
    fn after_structure_change(&mut self) {
        if self.doc.layer(self.layer).is_none() {
            self.layer = top_raster(&self.doc).unwrap_or(LayerId(0));
        }
        self.frame = self.frame.min(self.doc.frame_count().saturating_sub(1));
    }

    /// Adds a raster layer above the active one and makes it active.
    pub fn add_layer(&mut self) {
        let at = self.placement_above(self.layer);
        let name = format!("Layer {}", self.doc.walk().len() + 1);
        let id = self
            .history
            .transact(&mut self.doc, "New layer", Scope::All, |doc| {
                doc.add_layer(NewLayer::Raster, name, at)
            });
        if let Ok(id) = id {
            self.layer = id;
            self.touch();
        }
    }

    /// Deletes the active layer, unless it is the last one.
    pub fn delete_layer(&mut self) {
        if self.doc.walk().len() <= 1 {
            return;
        }
        let id = self.layer;
        let below = self.neighbor(id, false).or_else(|| self.neighbor(id, true));
        let done = self
            .history
            .transact(&mut self.doc, "Delete layer", Scope::All, |doc| {
                doc.remove_layer(id)
            });
        if done.is_ok() {
            self.layer = below.unwrap_or(LayerId(0));
            self.after_structure_change();
            self.touch();
        }
    }

    /// Moves the active layer one step up (toward the top) or down among
    /// its siblings.
    pub fn move_layer(&mut self, up: bool) {
        let id = self.layer;
        let parent = self.doc.parent_of(id);
        let siblings = match parent {
            None => self.doc.layers(),
            Some(p) => self.doc.layer(p).map_or(&[][..], |l| l.children()),
        };
        let Some(pos) = siblings.iter().position(|l| l.id == id) else {
            return;
        };
        let target = if up {
            if pos + 1 >= siblings.len() {
                return;
            }
            pos + 1
        } else {
            if pos == 0 {
                return;
            }
            pos - 1
        };
        let label = if up { "Raise layer" } else { "Lower layer" };
        let done = self
            .history
            .transact(&mut self.doc, label, Scope::Images(Vec::new()), |doc| {
                doc.move_layer(
                    id,
                    Placement {
                        parent,
                        index: Some(target),
                    },
                )
            });
        if done.is_ok() {
            self.touch();
        }
    }

    /// Edits the active layer's properties as one undo step.
    pub fn update_layer(
        &mut self,
        label: &str,
        id: LayerId,
        edit: impl FnOnce(&mut pixl_core::LayerProps),
    ) {
        let done = self
            .history
            .transact(&mut self.doc, label, Scope::Images(Vec::new()), |doc| {
                doc.update_layer(id, edit)
            });
        if done.is_ok() {
            self.touch();
        }
    }

    /// The placement just above `id`, in its own group.
    fn placement_above(&self, id: LayerId) -> Placement {
        let parent = self.doc.parent_of(id);
        let siblings = match parent {
            None => self.doc.layers(),
            Some(p) => self.doc.layer(p).map_or(&[][..], |l| l.children()),
        };
        let index = siblings.iter().position(|l| l.id == id).map(|i| i + 1);
        Placement { parent, index }
    }

    /// The sibling above (`up`) or below `id`.
    fn neighbor(&self, id: LayerId, up: bool) -> Option<LayerId> {
        let parent = self.doc.parent_of(id);
        let siblings = match parent {
            None => self.doc.layers(),
            Some(p) => self.doc.layer(p).map_or(&[][..], |l| l.children()),
        };
        let pos = siblings.iter().position(|l| l.id == id)?;
        let other = if up {
            pos.checked_add(1)?
        } else {
            pos.checked_sub(1)?
        };
        siblings.get(other).map(|l| l.id)
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::blank()
    }
}

/// The topmost raster layer, searching groups too.
pub fn top_raster(doc: &Document) -> Option<LayerId> {
    doc.walk()
        .into_iter()
        .rev()
        .find(|(_, l)| matches!(l.kind, LayerKind::Raster { .. }))
        .map(|(_, l)| l.id)
}
