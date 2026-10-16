//! The documents the start screen lists: the files most recently opened or
//! saved, newest first, each with its size and a thumbnail of its first
//! frame. The list and the thumbnails live in pixl's folder of the user
//! data directory, never beside the documents.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use lumenui::lumen_core::app_paths;
use lumenui::prelude::*;
use pixl_core::io::encode_png_rgba;
use pixl_core::native::write_atomic;
use pixl_core::{CompositeOptions, Document, Rgba, composite_frame};
use serde_json::{Value, json};

/// The id pixl keeps its user data under, whatever directory it runs from.
pub const APP_ID: &str = "pixl";

/// How many documents the list keeps.
pub const MAX_RECENTS: usize = 30;

/// The longest side of a thumbnail. A larger document is sampled down by a
/// whole step, so its pixels stay square.
pub const THUMB_SIDE: u32 = 128;

const LIST_FILE: &str = "recents.json";
const THUMB_DIR: &str = "thumbs";

/// One listed document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recent {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// The thumbnail's file name in the thumbnail folder.
    pub thumb: Option<String>,
}

/// When a listed document's thumbnail is redrawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thumb {
    /// Always: the document was just saved.
    Redraw,
    /// Only when there is none, or the file changed since it was drawn: the
    /// document was just opened.
    IfStale,
}

/// The recent documents, and where they are kept.
#[derive(Resource, Default)]
pub struct Recents {
    /// The folder the list and thumbnails are kept in; `None` keeps them in
    /// memory only.
    dir: Option<PathBuf>,
    pub entries: Vec<Recent>,
    /// Bumped on every change, including a thumbnail that finished drawing.
    pub revision: u64,
    /// Thumbnails being drawn.
    work: Vec<JoinHandle<()>>,
}

impl Recents {
    /// The list kept in the user's data directory for pixl.
    pub fn user() -> Self {
        Self::load(app_paths::data_dir_for(APP_ID))
    }

    /// The list kept in `dir`, read from it when it holds one.
    pub fn load(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let entries = std::fs::read(dir.join(LIST_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .map(|v| parse(&v))
            .unwrap_or_default();
        Self {
            dir: Some(dir),
            entries,
            revision: 1,
            work: Vec::new(),
        }
    }

    /// A list that is never written anywhere.
    pub fn in_memory() -> Self {
        Self {
            revision: 1,
            ..Self::default()
        }
    }

    /// The thumbnail file of `entry`, when it has one on disk.
    pub fn thumb_path(&self, entry: &Recent) -> Option<PathBuf> {
        let path = self
            .dir
            .as_ref()?
            .join(THUMB_DIR)
            .join(entry.thumb.as_ref()?);
        path.is_file().then_some(path)
    }

    /// The entry with this key (see [`key`]).
    pub fn find(&self, key: &str) -> Option<&Recent> {
        self.entries.iter().find(|e| self::key(&e.path) == key)
    }

    /// Puts `path` at the top of the list with `doc`'s size, and draws its
    /// thumbnail from `doc` on a worker thread when `thumb` asks for it.
    pub fn note(&mut self, path: &Path, doc: &Document, thumb: Thumb) {
        let old = self
            .entries
            .iter()
            .position(|e| e.path == path)
            .map(|i| self.entries.remove(i));
        let mut entry = Recent {
            path: path.to_path_buf(),
            width: doc.width(),
            height: doc.height(),
            thumb: old.as_ref().and_then(|e| e.thumb.clone()),
        };
        if let Some(dir) = self.dir.clone() {
            let current = old.as_ref().and_then(|e| self.thumb_path(e));
            let stale = match (&current, thumb) {
                (None, _) | (_, Thumb::Redraw) => true,
                (Some(t), Thumb::IfStale) => newer(path, t),
            };
            if stale {
                let name = thumb_name(path);
                let out = dir.join(THUMB_DIR).join(&name);
                let doc = doc.clone();
                self.work.push(std::thread::spawn(move || {
                    if write_thumbnail(&doc, &out).is_ok()
                        && let Some(old) = current
                    {
                        let _ = std::fs::remove_file(old);
                    }
                }));
                entry.thumb = Some(name);
            }
        }
        self.entries.insert(0, entry);
        for dropped in self.entries.split_off(self.entries.len().min(MAX_RECENTS)) {
            if let Some(t) = self.thumb_path(&dropped) {
                let _ = std::fs::remove_file(t);
            }
        }
        self.changed();
    }

    /// Takes `path` off the list, with its thumbnail.
    pub fn remove(&mut self, path: &Path) {
        let Some(i) = self.entries.iter().position(|e| e.path == path) else {
            return;
        };
        let entry = self.entries.remove(i);
        if let Some(t) = self.thumb_path(&entry) {
            let _ = std::fs::remove_file(t);
        }
        self.changed();
    }

    /// Collects the thumbnails that finished drawing, and says whether any
    /// did.
    pub fn collect_thumbnails(&mut self) -> bool {
        let (done, running): (Vec<_>, Vec<_>) =
            self.work.drain(..).partition(JoinHandle::is_finished);
        self.work = running;
        let any = !done.is_empty();
        for handle in done {
            let _ = handle.join();
        }
        if any {
            self.revision += 1;
        }
        any
    }

    /// Whether thumbnails are still being drawn.
    pub fn drawing(&self) -> bool {
        !self.work.is_empty()
    }

    /// Waits for every thumbnail being drawn.
    pub fn finish_thumbnails(&mut self) {
        if self.work.is_empty() {
            return;
        }
        for handle in self.work.drain(..) {
            let _ = handle.join();
        }
        self.revision += 1;
    }

    fn changed(&mut self) {
        self.revision += 1;
        let Some(dir) = &self.dir else {
            return;
        };
        let list: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                json!({
                    "path": e.path.to_string_lossy(),
                    "width": e.width,
                    "height": e.height,
                    "thumb": e.thumb,
                })
            })
            .collect();
        let doc = json!({ "version": 1, "recent": list });
        if std::fs::create_dir_all(dir).is_ok() {
            let bytes = serde_json::to_vec_pretty(&doc).unwrap_or_default();
            let _ = write_atomic(&dir.join(LIST_FILE), &bytes);
        }
    }
}

/// The entries of a saved list. Entries it cannot read are skipped.
fn parse(v: &Value) -> Vec<Recent> {
    let Some(list) = v.get("recent").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|e| {
            let size = |k: &str| e.get(k)?.as_u64().and_then(|n| u32::try_from(n).ok());
            Some(Recent {
                path: PathBuf::from(e.get("path")?.as_str()?),
                width: size("width")?,
                height: size("height")?,
                thumb: e.get("thumb").and_then(Value::as_str).map(str::to_string),
            })
        })
        .take(MAX_RECENTS)
        .collect()
}

/// A short stable key for a listed path, for element ids.
pub fn key(path: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// A fresh thumbnail file name for `path`: a new name each time, so a view
/// that cached the last picture loads the new one.
fn thumb_name(path: &Path) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{stamp:x}.png", key(path))
}

/// Whether the file at `path` changed after `than` was written.
fn newer(path: &Path, than: &Path) -> bool {
    let time = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (time(path), time(than)) {
        (Some(file), Some(thumb)) => file > thumb,
        _ => true,
    }
}

/// The first frame of `doc`, sampled down to fit [`THUMB_SIDE`], as RGBA
/// pixels with their width and height.
pub fn thumbnail(doc: &Document) -> (u32, u32, Vec<Rgba>) {
    let (w, h) = (doc.width(), doc.height());
    let full = composite_frame(doc, 0, &CompositeOptions::EXPORT);
    let step = w.max(h).div_ceil(THUMB_SIDE).max(1);
    if step == 1 {
        return (w, h, full);
    }
    let (tw, th) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = Vec::with_capacity((tw * th) as usize);
    for y in 0..th {
        for x in 0..tw {
            out.push(full[((y * step) * w + x * step) as usize]);
        }
    }
    (tw, th, out)
}

/// Draws `doc`'s thumbnail into the PNG file `out`.
pub fn write_thumbnail(doc: &Document, out: &Path) -> std::result::Result<(), String> {
    let (w, h, px) = thumbnail(doc);
    let png = encode_png_rgba(w, h, &px).map_err(|e| e.to_string())?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_atomic(out, &png).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_core::ColorMode;

    #[test]
    fn a_large_document_samples_down_by_a_whole_step() {
        let doc = Document::new(1000, 300, ColorMode::Rgba).unwrap();
        let (w, h, px) = thumbnail(&doc);
        // 1000 / 128 rounds up to a step of 8.
        assert_eq!((w, h), (125, 38));
        assert_eq!(px.len(), (w * h) as usize);
        let small = Document::new(20, 10, ColorMode::Rgba).unwrap();
        assert_eq!(thumbnail(&small).0, 20);
    }

    #[test]
    fn a_saved_list_reads_back_and_skips_what_it_cannot_read() {
        let v = json!({ "version": 1, "recent": [
            { "path": "/a/b.pixl", "width": 16, "height": 8, "thumb": "x.png" },
            { "path": "/c.pixl", "width": "wide" },
            { "path": "/d.png", "width": 4, "height": 4, "thumb": null },
        ]});
        let list = parse(&v);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].path, PathBuf::from("/a/b.pixl"));
        assert_eq!(list[0].thumb.as_deref(), Some("x.png"));
        assert_eq!(list[1].thumb, None);
    }
}
