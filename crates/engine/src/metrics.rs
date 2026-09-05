use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

use sdrmm_wire::QueueHealth;

pub(crate) struct QueueMetrics {
    epoch: Instant,
    queued: AtomicU64,
    capacity: AtomicU64,
    oldest: AtomicU64,
    dropped: AtomicU64,
}

impl Default for QueueMetrics {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            queued: AtomicU64::new(0),
            capacity: AtomicU64::new(0),
            oldest: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }
}

impl QueueMetrics {
    pub(crate) fn capacity(&self, value: usize) {
        self.capacity.store(value as u64, Ordering::Relaxed);
    }
    pub(crate) fn now(&self) -> u64 {
        self.epoch.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
    pub(crate) fn push(&self, count: usize) {
        if self.queued.fetch_add(count as u64, Ordering::Relaxed) == 0 {
            self.oldest.store(self.now(), Ordering::Relaxed);
        }
    }
    pub(crate) fn pop(&self, count: usize) {
        self.queued.fetch_sub(count as u64, Ordering::Relaxed);
    }
    pub(crate) fn oldest(&self, timestamp: u64) {
        self.oldest.store(timestamp, Ordering::Relaxed);
    }
    pub(crate) fn dropped(&self, count: usize) {
        self.dropped.fetch_add(count as u64, Ordering::Relaxed);
    }
    pub(crate) fn snapshot(&self) -> QueueHealth {
        let queued = self.queued.load(Ordering::Relaxed);
        QueueHealth {
            queued,
            capacity: self.capacity.load(Ordering::Relaxed),
            oldest_ms: if queued == 0 {
                0.0
            } else {
                self.now()
                    .saturating_sub(self.oldest.load(Ordering::Relaxed)) as f64
                    / 1000.0
            },
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}
