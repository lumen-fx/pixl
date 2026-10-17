//! Edits too large for one tick, cut into tiles and rendered on worker
//! threads (the owner's Cinebench ruling): the tiles nearest the middle of
//! the view go first, each finished tile is written into the cel on a later
//! tick within a small time budget, so the canvas fills in as the work goes
//! and the editor keeps answering, and stopping a job (a new setting,
//! Cancel, Escape) ends it at the next tile. An edit whose result cannot be
//! shown in parts (a document going to indexed color, a palette made from
//! the image) runs as a [`Task`] on a thread of its own instead, reporting
//! how far it is.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pixl_core::filters::write_values;
use pixl_core::history::Transaction;
use pixl_core::{Error, LayerId, Progress, Raster, Rect, TILE_SIZE};

use crate::editor::Editor;

/// Time a tick spends writing finished tiles into a cel.
pub const WRITE_BUDGET: Duration = Duration::from_millis(6);

/// Worker threads a job runs on, 0 for one per core but one; set from the
/// performance settings.
static WORKERS: AtomicUsize = AtomicUsize::new(0);

/// The most tiles an edit paints within its own tick; set from the
/// performance settings' progressive redraw threshold.
static SYNC_TILES: AtomicUsize = AtomicUsize::new(24);

/// Takes the worker threads and the progressive threshold from the
/// performance settings.
pub fn configure(perf: &crate::settings::PerformanceSettings) {
    WORKERS.store(perf.worker_threads, Ordering::Relaxed);
    SYNC_TILES.store(perf.progressive_tiles.max(1), Ordering::Relaxed);
}

/// The threads a job runs on.
pub fn worker_threads() -> usize {
    match WORKERS.load(Ordering::Relaxed) {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1)),
        n => n,
    }
    .max(1)
}

/// Whether an edit of `tiles` tiles is small enough to finish within its
/// own tick rather than fill in over the following ones.
pub fn finishes_at_once(tiles: usize) -> bool {
    tiles <= SYNC_TILES.load(Ordering::Relaxed)
}

/// One run of a tile renderer over a set of tiles.
pub struct TileJob {
    cancel: Arc<AtomicBool>,
    /// Behind a lock only so the job can sit in a resource.
    results: Mutex<Receiver<(Rect, Vec<u32>)>>,
    pub total: usize,
    pub done: usize,
    started: Instant,
    pub finished: Option<Duration>,
}

impl TileJob {
    /// Starts rendering `tiles`, in the order given, on worker threads.
    /// Each worker makes its own renderer with `make`, so one can keep a
    /// scratch copy of what it paints on; a renderer returns the raw values
    /// of a tile, row by row.
    pub fn start<M, R>(tiles: Vec<Rect>, make: M) -> TileJob
    where
        M: Fn() -> R + Send + Sync + 'static,
        R: FnMut(Rect) -> Vec<u32>,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let total = tiles.len();
        let tiles = Arc::new(tiles);
        let next = Arc::new(AtomicUsize::new(0));
        let make = Arc::new(make);
        let workers = worker_threads().min(total.max(1));
        for _ in 0..workers {
            let (tiles, next, cancel, make, tx) = (
                Arc::clone(&tiles),
                Arc::clone(&next),
                Arc::clone(&cancel),
                Arc::clone(&make),
                tx.clone(),
            );
            std::thread::spawn(move || {
                let mut render = make();
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(rect) = tiles.get(i) else {
                        return;
                    };
                    let values = render(*rect);
                    if tx.send((*rect, values)).is_err() {
                        return;
                    }
                }
            });
        }
        TileJob {
            cancel,
            results: Mutex::new(rx),
            total,
            done: 0,
            started: Instant::now(),
            finished: None,
        }
    }

    /// Ends the work at the next tile; tiles already rendered are dropped
    /// with the job.
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Hands finished tiles to `write` until `until` passes (every one that
    /// is ready with `None`). Returns whether any came.
    pub fn drain(&mut self, until: Option<Instant>, mut write: impl FnMut(Rect, &[u32])) -> bool {
        let mut any = false;
        let results = self
            .results
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        while until.is_none_or(|t| Instant::now() < t) {
            let Ok((rect, values)) = results.try_recv() else {
                break;
            };
            write(rect, &values);
            self.done += 1;
            any = true;
        }
        if self.complete() && self.finished.is_none() {
            self.finished = Some(self.started.elapsed());
        }
        any
    }

    /// Like [`TileJob::drain`], but waits for every tile still to come.
    pub fn finish(&mut self, mut write: impl FnMut(Rect, &[u32])) {
        while !self.complete() {
            let next = self
                .results
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .recv_timeout(Duration::from_millis(50));
            match next {
                Ok((rect, values)) => {
                    write(rect, &values);
                    self.done += 1;
                }
                // Every worker has stopped: nothing more comes.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        if self.finished.is_none() {
            self.finished = Some(self.started.elapsed());
        }
    }

    pub fn complete(&self) -> bool {
        self.done >= self.total
    }

    /// How far the job is, for a status line.
    pub fn progress(&self) -> String {
        match self.finished {
            Some(t) => format!("{} tiles in {} ms", self.total, t.as_millis()),
            None => format!("{} of {} tiles", self.done, self.total),
        }
    }
}

impl Drop for TileJob {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Work on a thread of its own whose result comes back whole, with the
/// [`Progress`] it reports and stops by.
pub struct Task<T> {
    pub progress: Arc<Progress>,
    result: Mutex<Receiver<T>>,
}

impl<T: Send + 'static> Task<T> {
    pub fn spawn(work: impl FnOnce(&Progress) -> T + Send + 'static) -> Task<T> {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel();
        let p = Arc::clone(&progress);
        std::thread::spawn(move || {
            let _ = tx.send(work(&p));
        });
        Task {
            progress,
            result: Mutex::new(rx),
        }
    }

    /// The result, once the work is done.
    pub fn poll(&mut self) -> Option<T> {
        self.result
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .try_recv()
            .ok()
    }

    /// Asks the work to stop; what it returns is dropped with the task.
    pub fn stop(&self) {
        self.progress.stop();
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.progress.stop();
    }
}

/// An edit of one cel painted by a [`TileJob`]: its undo step begins
/// before the work starts and is kept once every tile is written, and a
/// cancel rolls the document back to how the step found it.
pub struct PaintJob {
    layer: LayerId,
    frame: usize,
    /// The job paints the layer's mask, not its pixels.
    mask: bool,
    tx: Option<Transaction>,
    pub job: TileJob,
}

impl PaintJob {
    /// Starts the undo step of `label` on what tools paint on, and the
    /// workers `make` gives over `tiles` of it. `make` is handed the cel as
    /// it was, to paint a scratch copy of.
    pub fn start<M, R>(
        editor: &mut Editor,
        label: &str,
        tiles: Vec<Rect>,
        make: impl FnOnce(Raster) -> M,
    ) -> Result<PaintJob, Error>
    where
        M: Fn() -> R + Send + Sync + 'static,
        R: FnMut(Rect) -> Vec<u32>,
    {
        let (image, tx) = editor.begin_paint(label)?;
        let original = editor
            .doc
            .image(image)
            .cloned()
            .ok_or(Error::LayerNotFound(editor.layer))?;
        let job = TileJob::start(tiles, make(original));
        Ok(PaintJob {
            layer: editor.layer,
            frame: editor.frame,
            mask: editor.mask_edit,
            tx: Some(tx),
            job,
        })
    }

    /// Writes the tiles finished by `until` (all of them with `None`, which
    /// waits for the rest) into the cel, and keeps the undo step once the
    /// last is in. Returns whether the job is done.
    pub fn pump(&mut self, editor: &mut Editor, until: Option<Instant>) -> bool {
        let (layer, frame, mask) = (self.layer, self.frame, self.mask);
        let write = |rect: Rect, values: &[u32]| {
            let target = if mask {
                editor.doc.mask_target(layer)
            } else {
                editor.doc.target(layer, frame)
            };
            if let Ok(t) = target {
                write_values(t.raster, rect, values);
            }
        };
        let wrote = match until {
            Some(_) => self.job.drain(until, write),
            None => {
                self.job.finish(write);
                true
            }
        };
        if wrote {
            editor.touch_layers(&[layer]);
        }
        if !self.job.complete() {
            return false;
        }
        if let Some(tx) = self.tx.take() {
            editor.history.commit(&editor.doc, tx);
        }
        true
    }

    /// Stops the work and puts the document back as the edit found it.
    pub fn cancel(mut self, editor: &mut Editor) {
        self.job.stop();
        if let Some(tx) = self.tx.take() {
            editor.history.rollback(&mut editor.doc, tx);
        }
        editor.touch_layers(&[self.layer]);
    }
}

/// The tiles of `area`, on the document's tile grid, nearest `focus` first.
pub fn tiles_of(area: Rect, focus: (f64, f64)) -> Vec<Rect> {
    let mut out = Vec::new();
    if area.is_empty() {
        return out;
    }
    let t = TILE_SIZE;
    let (x0, y0) = (area.x.div_euclid(t) * t, area.y.div_euclid(t) * t);
    let mut y = y0;
    while y < area.bottom() {
        let mut x = x0;
        while x < area.right() {
            let r = Rect::new(x, y, t, t).intersect(&area);
            if !r.is_empty() {
                out.push(r);
            }
            x += t;
        }
        y += t;
    }
    let d = |r: &Rect| {
        let cx = f64::from(r.x) + f64::from(r.w) / 2.0 - focus.0;
        let cy = f64::from(r.y) + f64::from(r.h) / 2.0 - focus.1;
        cx * cx + cy * cy
    };
    out.sort_by(|a, b| d(a).total_cmp(&d(b)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_area_nearest_first() {
        let t = tiles_of(Rect::new(10, 10, 200, 100), (180.0, 90.0));
        let covered: i64 = t.iter().map(|r| r.area()).sum();
        assert_eq!(covered, 200 * 100);
        assert!(t[0].contains(180, 90));
    }

    #[test]
    fn every_tile_comes_back_once() {
        let tiles = tiles_of(Rect::new(0, 0, 300, 200), (0.0, 0.0));
        let n = tiles.len();
        let mut job = TileJob::start(tiles, || |r: Rect| vec![r.x as u32; r.area() as usize]);
        let mut seen = Vec::new();
        job.finish(|r, v| {
            assert_eq!(v.len(), r.area() as usize);
            seen.push(r);
        });
        assert_eq!(seen.len(), n);
        assert!(job.complete());
    }

    #[test]
    fn a_stopped_job_ends() {
        let tiles = tiles_of(Rect::new(0, 0, 4096, 4096), (0.0, 0.0));
        let mut job = TileJob::start(tiles, || {
            |r: Rect| {
                std::thread::sleep(Duration::from_millis(1));
                vec![0; r.area() as usize]
            }
        });
        job.stop();
        job.finish(|_, _| {});
        assert!(job.done < job.total);
    }

    #[test]
    fn a_paint_job_writes_every_tile_as_one_step_and_a_cancel_leaves_no_trace() {
        use pixl_core::{ColorMode, Document};
        let doc = Document::new(300, 200, ColorMode::Rgba).unwrap();
        let bounds = doc.bounds();
        let tiles = || tiles_of(bounds, (0.0, 0.0));
        let fill = |_: Raster| || |r: Rect| vec![0xff00_00ff; r.area() as usize];
        let mut editor = Editor::new(doc.clone());
        let layer = editor.layer;
        let mut job = PaintJob::start(&mut editor, "Fill", tiles(), fill).unwrap();
        assert!(job.pump(&mut editor, None));
        let raster = editor.doc.cel_raster(layer, 0).unwrap().unwrap();
        assert_eq!(raster.get_raw(299, 199), 0xff00_00ff);
        assert_eq!(editor.history.undo_labels().collect::<Vec<_>>(), ["Fill"]);

        let mut editor = Editor::new(doc);
        let slow = |_: Raster| {
            || {
                |r: Rect| {
                    std::thread::sleep(Duration::from_millis(5));
                    vec![0xff00_00ff; r.area() as usize]
                }
            }
        };
        let mut job = PaintJob::start(&mut editor, "Fill", tiles(), slow).unwrap();
        job.pump(&mut editor, Some(Instant::now()));
        job.cancel(&mut editor);
        assert_eq!(editor.doc.cel(layer, 0).unwrap(), None);
        assert!(!editor.history.can_undo());
    }
}
