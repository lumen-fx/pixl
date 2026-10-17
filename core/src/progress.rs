//! How a long edit run off the UI thread reports how far it is, and how it
//! is told to stop.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The parts of a long edit done so far, how many there are, and a flag
/// that stops it between parts.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    stop: AtomicBool,
}

impl Progress {
    /// Parts done and parts in all.
    pub fn get(&self) -> (usize, usize) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }

    pub fn set_total(&self, total: usize) {
        self.total.store(total, Ordering::Relaxed);
    }

    /// Counts one more part done.
    pub fn step(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    /// Asks the edit to stop at its next part.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}
