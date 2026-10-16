//! Undo and redo under a memory budget.
//!
//! A transaction remembers the document's structure (layers, frames, tags,
//! palette; everything but pixels) and the tile pointers of the images in its
//! scope. On commit it keeps only what changed: tiles whose pointer differs,
//! images that appeared or disappeared, and the structure if it changed.
//! Because tiles are copy-on-write, a snapshot of an untouched tile costs a
//! pointer, and an entry holds just the tiles its edit replaced.

use std::collections::{BTreeMap, VecDeque};

use crate::document::{Document, ImageId, Structure};
use crate::geom::Rect;
use crate::image::{Raster, TileSnap};

/// Which images a transaction watches for pixel edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Only these images (plus any created during the transaction).
    Images(Vec<ImageId>),
    /// Every image in the document.
    All,
}

#[derive(Clone, Debug)]
enum ImageChange {
    Tiles {
        id: ImageId,
        /// (tile index, before, after)
        tiles: Vec<(usize, TileSnap, TileSnap)>,
    },
    /// The image changed kind or size; both versions are kept whole.
    Replaced {
        id: ImageId,
        before: Raster,
        after: Raster,
    },
    Added(ImageId, Raster),
    Removed(ImageId, Raster),
}

impl ImageChange {
    fn bytes(&self) -> usize {
        match self {
            ImageChange::Tiles { tiles, .. } => {
                tiles.iter().map(|(_, a, b)| a.bytes() + b.bytes()).sum()
            }
            ImageChange::Replaced { before, after, .. } => {
                before.memory_bytes() + after.memory_bytes()
            }
            ImageChange::Added(_, r) | ImageChange::Removed(_, r) => r.memory_bytes(),
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    label: String,
    structure: Option<Box<(Structure, Structure)>>,
    images: Vec<ImageChange>,
    bytes: usize,
}

/// An edit in progress. Created by [`History::begin`], closed by
/// [`History::commit`].
pub struct Transaction {
    label: String,
    structure: Structure,
    image_ids: Vec<ImageId>,
    watched: BTreeMap<ImageId, Raster>,
}

impl Transaction {
    /// Adds an image to the scope. An image created since [`History::begin`]
    /// needs no watching: the commit records it whole.
    pub fn watch(&mut self, doc: &Document, image: ImageId) {
        if self.image_ids.binary_search(&image).is_ok()
            && !self.watched.contains_key(&image)
            && let Some(r) = doc.images.get(&image)
        {
            self.watched.insert(image, r.clone());
        }
    }
}

/// Undo and redo stacks for one document.
#[derive(Clone, Debug)]
pub struct History {
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    budget: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(256 * 1024 * 1024)
    }
}

impl History {
    /// Empty history keeping at most `budget_bytes` of snapshots. The newest
    /// entry is always kept, even when it alone exceeds the budget.
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            budget: budget_bytes,
        }
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn set_budget(&mut self, budget_bytes: usize) {
        self.budget = budget_bytes;
        self.enforce_budget();
    }

    /// Bytes held by all entries. A tile still shared with the document
    /// counts in full.
    pub fn memory_bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(self.redo.iter())
            .map(|e| e.bytes)
            .sum()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Labels of undoable edits, oldest first.
    pub fn undo_labels(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().map(|e| e.label.as_str())
    }

    /// Labels of redoable edits, next first.
    pub fn redo_labels(&self) -> impl Iterator<Item = &str> {
        self.redo.iter().rev().map(|e| e.label.as_str())
    }

    /// Starts recording an edit.
    pub fn begin(&self, doc: &Document, label: impl Into<String>, scope: Scope) -> Transaction {
        let watched = match scope {
            Scope::All => doc.images.clone(),
            Scope::Images(ids) => ids
                .into_iter()
                .filter_map(|id| doc.images.get(&id).map(|r| (id, r.clone())))
                .collect(),
        };
        Transaction {
            label: label.into(),
            structure: doc.s.clone(),
            image_ids: doc.images.keys().copied().collect(),
            watched,
        }
    }

    /// Closes an edit. Returns false (and records nothing) if nothing changed.
    /// A recorded edit clears the redo stack.
    pub fn commit(&mut self, doc: &Document, tx: Transaction) -> bool {
        let mut images = Vec::new();
        for (id, before) in &tx.watched {
            let Some(after) = doc.images.get(id) else {
                continue;
            };
            if !before.same_shape(after) {
                images.push(ImageChange::Replaced {
                    id: *id,
                    before: before.clone(),
                    after: after.clone(),
                });
                continue;
            }
            let tiles: Vec<_> = (0..after.tile_count())
                .filter(|&t| !before.same_tile(after, t))
                .map(|t| (t, before.tile_snap(t), after.tile_snap(t)))
                .collect();
            if !tiles.is_empty() {
                images.push(ImageChange::Tiles { id: *id, tiles });
            }
        }
        let structure_changed = doc.s != tx.structure;
        if structure_changed || doc.images.len() != tx.image_ids.len() {
            for (id, r) in &doc.images {
                if tx.image_ids.binary_search(id).is_err() {
                    images.push(ImageChange::Added(*id, r.clone()));
                }
            }
            for id in &tx.image_ids {
                if !doc.images.contains_key(id) {
                    // Removed images were either watched or untouched since
                    // begin; only watched ones can be restored exactly.
                    if let Some(r) = tx.watched.get(id) {
                        images.push(ImageChange::Removed(*id, r.clone()));
                    }
                }
            }
        }
        if !structure_changed && images.is_empty() {
            return false;
        }
        let structure = structure_changed.then(|| Box::new((tx.structure, doc.s.clone())));
        let bytes = images.iter().map(ImageChange::bytes).sum::<usize>()
            + structure
                .as_ref()
                .map_or(0, |_| 2 * std::mem::size_of::<Structure>());
        self.redo.clear();
        self.undo.push_back(Entry {
            label: tx.label,
            structure,
            images,
            bytes,
        });
        self.enforce_budget();
        true
    }

    /// Runs `edit` as one undoable step. Structural edits that may delete
    /// images should use [`Scope::All`] so the deleted pixels can come back.
    pub fn transact<R>(
        &mut self,
        doc: &mut Document,
        label: impl Into<String>,
        scope: Scope,
        edit: impl FnOnce(&mut Document) -> R,
    ) -> R {
        let tx = self.begin(doc, label, scope);
        let out = edit(doc);
        self.commit(doc, tx);
        out
    }

    fn enforce_budget(&mut self) {
        while self.memory_bytes() > self.budget {
            if !self.redo.is_empty() {
                self.redo.remove(0);
            } else if self.undo.len() > 1 {
                self.undo.pop_front();
            } else {
                break;
            }
        }
    }

    /// Reverts the newest edit; returns the canvas area that changed.
    pub fn undo(&mut self, doc: &mut Document) -> Option<Rect> {
        let entry = self.undo.pop_back()?;
        let dirty = apply(doc, &entry, false);
        self.redo.push(entry);
        Some(dirty)
    }

    /// Reapplies the newest undone edit; returns the canvas area that changed.
    pub fn redo(&mut self, doc: &mut Document) -> Option<Rect> {
        let entry = self.redo.pop()?;
        let dirty = apply(doc, &entry, true);
        self.undo.push_back(entry);
        Some(dirty)
    }
}

fn apply(doc: &mut Document, entry: &Entry, forward: bool) -> Rect {
    let mut dirty = Rect::EMPTY;
    if let Some(s) = &entry.structure {
        doc.s = if forward { s.1.clone() } else { s.0.clone() };
        dirty = doc.bounds();
    }
    for change in &entry.images {
        match change {
            ImageChange::Tiles { id, tiles } => {
                if let Some(r) = doc.images.get_mut(id) {
                    for (t, before, after) in tiles {
                        r.restore_tile(*t, if forward { after } else { before });
                        dirty = dirty.union(&r.tile_rect(*t));
                    }
                }
            }
            ImageChange::Replaced { id, before, after } => {
                doc.images
                    .insert(*id, if forward { after } else { before }.clone());
                dirty = doc.bounds();
            }
            ImageChange::Added(id, r) => {
                if forward {
                    doc.images.insert(*id, r.clone());
                } else {
                    doc.images.remove(id);
                }
            }
            ImageChange::Removed(id, r) => {
                if forward {
                    doc.images.remove(id);
                } else {
                    doc.images.insert(*id, r.clone());
                }
            }
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{ColorMode, Rgba};
    use crate::document::{NewLayer, Placement};
    use crate::image::TILE_AREA;

    fn doc() -> Document {
        Document::new(256, 256, ColorMode::Rgba).unwrap()
    }

    fn stroke(h: &mut History, d: &mut Document, x: i32, y: i32, c: Rgba) {
        let layer = d.layers()[0].id;
        let image = d.target_image(layer, 0).unwrap();
        h.transact(d, "pencil", Scope::Images(vec![image]), |d| {
            d.target(layer, 0).unwrap().raster.set_raw(x, y, c.to_u32());
        });
    }

    fn pixel(d: &Document, x: i32, y: i32) -> u32 {
        let layer = d.layers()[0].id;
        d.cel_raster(layer, 0)
            .unwrap()
            .map_or(0, |r| r.get_raw(x, y))
    }

    #[test]
    fn undo_redo_restores_tiles() {
        let mut d = doc();
        let mut h = History::default();
        stroke(&mut h, &mut d, 3, 3, Rgba::BLACK);
        stroke(&mut h, &mut d, 3, 3, Rgba::WHITE);
        stroke(&mut h, &mut d, 200, 200, Rgba::WHITE);
        assert_eq!(h.undo(&mut d), Some(Rect::new(192, 192, 64, 64)));
        assert_eq!(pixel(&d, 200, 200), 0);
        h.undo(&mut d);
        assert_eq!(pixel(&d, 3, 3), Rgba::BLACK.to_u32());
        h.redo(&mut d);
        assert_eq!(pixel(&d, 3, 3), Rgba::WHITE.to_u32());
        assert!(h.can_redo());
        stroke(&mut h, &mut d, 9, 9, Rgba::BLACK);
        assert!(!h.can_redo(), "a new edit drops the redo stack");
    }

    #[test]
    fn entries_hold_only_changed_tiles() {
        let mut d = doc();
        let mut h = History::default();
        stroke(&mut h, &mut d, 3, 3, Rgba::BLACK);
        // The first stroke also creates the cel (structure and a new image).
        let first = h.memory_bytes();
        stroke(&mut h, &mut d, 4, 3, Rgba::BLACK);
        let second = h.memory_bytes() - first;
        assert_eq!(second, 2 * TILE_AREA * 4, "one tile before and after");
    }

    #[test]
    fn empty_edits_are_not_recorded() {
        let mut d = doc();
        let mut h = History::default();
        stroke(&mut h, &mut d, 3, 3, Rgba::BLACK);
        stroke(&mut h, &mut d, 3, 3, Rgba::BLACK);
        assert_eq!(h.undo_labels().count(), 1);
    }

    #[test]
    fn budget_drops_oldest_entries() {
        let mut d = doc();
        // Each stroke lands in a fresh tile: nothing before, one tile after.
        let mut h = History::new(3 * TILE_AREA * 4);
        for i in 0..10 {
            stroke(&mut h, &mut d, i * 64 % 256, i * 64 / 256 * 64, Rgba::BLACK);
        }
        assert!(h.memory_bytes() <= h.budget());
        assert_eq!(h.undo_labels().count(), 3);
        while h.undo(&mut d).is_some() {}
        assert_eq!(pixel(&d, 9 * 64 % 256, 128), 0);
        assert_eq!(pixel(&d, 0, 0), Rgba::BLACK.to_u32(), "evicted edits stay");
        h.set_budget(0);
        assert_eq!(h.memory_bytes(), 0, "redo entries go first");
    }

    #[test]
    fn structural_edits_round_trip() {
        let mut d = doc();
        let mut h = History::default();
        stroke(&mut h, &mut d, 1, 1, Rgba::BLACK);
        let base = d.layers()[0].id;
        let before = d.clone();
        h.transact(&mut d, "add layer", Scope::All, |d| {
            d.add_layer(NewLayer::Raster, "two", Placement::default())
                .unwrap();
            d.insert_frame(1, 40).unwrap();
            d.link_cel(base, 0, 1).unwrap();
        });
        let after = d.clone();
        h.undo(&mut d);
        assert_eq!(d, before);
        h.redo(&mut d);
        assert_eq!(d, after);
        h.transact(&mut d, "delete", Scope::All, |d| {
            d.remove_layer(base).unwrap()
        });
        assert_eq!(d.images.len(), 0);
        h.undo(&mut d);
        assert_eq!(d, after, "the deleted layer's pixels come back");
    }

    #[test]
    fn mode_change_replaces_images() {
        let mut d = doc();
        let mut h = History::default();
        let red = d.palette().get(4);
        stroke(&mut h, &mut d, 1, 1, red);
        let before = d.clone();
        h.transact(&mut d, "indexed", Scope::All, |d| {
            d.set_color_mode(ColorMode::Indexed)
        });
        assert_eq!(pixel(&d, 1, 1), 4);
        assert_eq!(h.undo(&mut d), Some(d.bounds()));
        assert_eq!(d, before);
    }
}
