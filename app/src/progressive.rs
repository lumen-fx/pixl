//! Tile work off the UI thread, shown as it completes.
//!
//! A large recomposite (opening a document, showing a group, a palette change
//! in an indexed document) is cut into tiles, ordered from the middle of the
//! view outward, and handed to worker threads. Each finished tile comes back
//! as RGBA8 rows, and the view writes it into its pixel buffer on the next
//! tick, so the picture fills in tile by tile while the editor stays
//! responsive.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Instant;

use pixl_core::composite::composite_with_threads;
use pixl_core::{
    CompositeOptions, Document, OnionSkin, Palette, Raster, Rect, Rgba, composite_ghosts,
};

/// Where a tile's pixels come from.
pub enum Source {
    /// One raster, converted to RGBA8 (indexed through the palette).
    Raster {
        raster: Raster,
        palette: Palette,
        transparent_index: u8,
    },
    /// A composite of a prepared document.
    Composite {
        doc: Document,
        frame: usize,
        opts: CompositeOptions,
    },
    /// The onion skin's ghost frames around `frame`.
    Ghosts {
        doc: Document,
        frame: usize,
        onion: OnionSkin,
    },
}

impl Source {
    /// The RGBA8 rows of `rect`.
    pub fn render(&self, rect: Rect) -> Vec<u8> {
        match self {
            Source::Raster {
                raster,
                palette,
                transparent_index,
            } => raster_rgba8(raster, palette, *transparent_index, rect),
            Source::Composite { doc, frame, opts } => {
                let px = composite_with_threads(doc, *frame, rect, opts, 1);
                rgba8(&px)
            }
            Source::Ghosts { doc, frame, onion } => {
                rgba8(&composite_ghosts(doc, *frame, rect, onion, false, 1))
            }
        }
    }
}

/// Straight RGBA8 bytes of colors.
pub fn rgba8(px: &[Rgba]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 4);
    for c in px {
        out.extend_from_slice(&[c.r, c.g, c.b, c.a]);
    }
    out
}

/// The RGBA8 rows of `rect` of a raster; outside it reads transparent.
pub fn raster_rgba8(
    raster: &Raster,
    palette: &Palette,
    transparent_index: u8,
    rect: Rect,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(rect.area() as usize * 4);
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let c = match raster {
                Raster::Rgba(img) => {
                    if img.bounds().contains(x, y) {
                        img.get(x, y)
                    } else {
                        Rgba::TRANSPARENT
                    }
                }
                Raster::Indexed(img) => {
                    let i = if img.bounds().contains(x, y) {
                        img.get(x, y)
                    } else {
                        transparent_index
                    };
                    if i == transparent_index {
                        Rgba::TRANSPARENT
                    } else {
                        palette.get(i)
                    }
                }
                Raster::Mask(img) => {
                    let v = img.get(x, y);
                    Rgba::new(v, v, v, 255)
                }
            };
            out.extend_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    }
    out
}

/// One tile to render.
struct Task {
    item: u64,
    stamp: u64,
    rect: Rect,
    source: Arc<Source>,
}

/// A rendered tile.
pub struct Done {
    /// The view item the tile belongs to.
    pub item: u64,
    /// When the source was taken; a newer write to the same tile wins.
    pub stamp: u64,
    pub rect: Rect,
    pub rgba: Vec<u8>,
}

struct Queue {
    tasks: Mutex<VecDeque<Task>>,
    ready: Condvar,
    stop: AtomicBool,
}

/// A pool of worker threads rendering tiles.
pub struct Pool {
    queue: Arc<Queue>,
    results: Mutex<Receiver<Done>>,
    workers: Vec<JoinHandle<()>>,
    stamps: AtomicU64,
}

impl Pool {
    /// A pool with one worker per core but one, left for the UI thread.
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        Self::with_workers(cores.saturating_sub(1).max(1))
    }

    pub fn with_workers(count: usize) -> Self {
        let queue = Arc::new(Queue {
            tasks: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let (tx, rx) = channel();
        let workers = (0..count)
            .map(|i| {
                let queue = Arc::clone(&queue);
                let tx: Sender<Done> = tx.clone();
                std::thread::Builder::new()
                    .name(format!("pixl-tiles-{i}"))
                    .spawn(move || work(&queue, &tx))
                    .expect("spawn a tile worker")
            })
            .collect();
        Self {
            queue,
            results: Mutex::new(rx),
            workers,
            stamps: AtomicU64::new(1),
        }
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// A stamp later than every one handed out before.
    pub fn stamp(&self) -> u64 {
        self.stamps.fetch_add(1, Ordering::Relaxed)
    }

    /// Queues `rects` of `source` for `item`, in the order given. Returns the
    /// stamp the results carry.
    pub fn submit(&self, item: u64, source: Source, rects: Vec<Rect>) -> u64 {
        let stamp = self.stamp();
        let source = Arc::new(source);
        let mut tasks = self
            .queue
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for rect in rects {
            tasks.push_back(Task {
                item,
                stamp,
                rect,
                source: Arc::clone(&source),
            });
        }
        drop(tasks);
        self.queue.ready.notify_all();
        stamp
    }

    /// Drops the queued tiles of `item`; tiles already being rendered still
    /// arrive.
    pub fn cancel(&self, item: u64) {
        self.queue
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|t| t.item != item);
    }

    /// Tiles still waiting for a worker.
    pub fn queued(&self) -> usize {
        self.queue
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// A finished tile, if one is waiting.
    pub fn next_finished(&self) -> Option<Done> {
        self.results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_recv()
            .ok()
    }

    /// Finished tiles, until `deadline` passes or none are left.
    pub fn finished(&self, deadline: Instant) -> Vec<Done> {
        let mut out = Vec::new();
        while Instant::now() < deadline {
            match self.next_finished() {
                Some(done) => out.push(done),
                None => break,
            }
        }
        out
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.queue.stop.store(true, Ordering::Release);
        self.queue.ready.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn work(queue: &Queue, tx: &Sender<Done>) {
    loop {
        let task = {
            let mut tasks = queue.tasks.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if queue.stop.load(Ordering::Acquire) {
                    return;
                }
                if let Some(t) = tasks.pop_front() {
                    break t;
                }
                tasks = queue
                    .ready
                    .wait(tasks)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let rgba = task.source.render(task.rect);
        let done = Done {
            item: task.item,
            stamp: task.stamp,
            rect: task.rect,
            rgba,
        };
        if tx.send(done).is_err() {
            return;
        }
    }
}

/// `area` cut into `size`-pixel tiles, nearest to `focus` first.
pub fn tiles_from(area: Rect, size: i32, focus: (f64, f64)) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut y = area.y;
    while y < area.bottom() {
        let mut x = area.x;
        while x < area.right() {
            out.push(Rect::new(x, y, size, size).intersect(&area));
            x += size;
        }
        y += size;
    }
    let dist = |r: &Rect| {
        let cx = r.x as f64 + r.w as f64 / 2.0 - focus.0;
        let cy = r.y as f64 + r.h as f64 / 2.0 - focus.1;
        cx * cx + cy * cy
    };
    out.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixl_core::{ColorMode, TiledImage};
    use std::time::Duration;

    #[test]
    fn tiles_cover_the_area_once_nearest_first() {
        let area = Rect::new(0, 0, 300, 200);
        let tiles = tiles_from(area, 128, (290.0, 190.0));
        assert_eq!(tiles.iter().map(|r| r.area()).sum::<i64>(), area.area());
        assert_eq!(tiles[0], Rect::new(256, 128, 44, 72));
    }

    #[test]
    fn workers_render_every_tile() {
        let mut img = TiledImage::new(64, 64, Rgba::TRANSPARENT);
        img.set(3, 4, Rgba::opaque(10, 20, 30));
        let pool = Pool::with_workers(2);
        let rects = tiles_from(Rect::new(0, 0, 64, 64), 16, (0.0, 0.0));
        let n = rects.len();
        let doc = pixl_core::Document::new(4, 4, ColorMode::Rgba).unwrap();
        pool.submit(
            7,
            Source::Raster {
                raster: Raster::Rgba(img),
                palette: doc.palette().clone(),
                transparent_index: 0,
            },
            rects,
        );
        let mut got = Vec::new();
        let until = Instant::now() + Duration::from_secs(10);
        while got.len() < n && Instant::now() < until {
            got.extend(pool.finished(Instant::now() + Duration::from_millis(5)));
        }
        assert_eq!(got.len(), n);
        let first = got
            .iter()
            .find(|d| d.rect == Rect::new(0, 0, 16, 16))
            .unwrap();
        let at = (4 * 16 + 3) * 4;
        assert_eq!(&first.rgba[at..at + 4], &[10, 20, 30, 255]);
    }
}
