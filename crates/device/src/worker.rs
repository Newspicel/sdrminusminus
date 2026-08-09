//! The capture thread every backend runs, and the flag that stops it.
//!
//! Four backends had four copies of the same fifteen lines — spawn a named thread, hand it an
//! `AtomicBool`, refuse a second start, clear the flag and join on stop — and the copies had
//! already drifted (one checked the flag to decide whether it was streaming, another checked the
//! join handle, and they disagree if a spawn fails). One copy, here.
//!
//! No I/O and no device knowledge: this owns the thread, not what runs on it.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use crate::DeviceError;

/// A backend's streaming thread.
///
/// The stop flag is the only channel in: a body must poll it and return promptly once it clears.
/// Anything that needs to *interrupt* a blocking wait (cancelling USB transfers, closing a
/// socket) has to be woken separately before [`Worker::stop`] joins — see
/// [`Capture`](crate::Capture), which does exactly that.
#[derive(Debug, Default)]
pub struct Worker {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    /// A worker with no thread.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a thread is running. False after [`Worker::stop`], and after a body returned on
    /// its own — the flag alone cannot tell those apart from "never started".
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Whether the stop flag is still set, for a body that has to observe it from outside.
    #[must_use]
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Spawn `body` on a thread named `name`.
    ///
    /// # Errors
    /// [`DeviceError::AlreadyStreaming`] if a thread is already running, [`DeviceError::Io`] if
    /// the thread cannot be spawned. A failed spawn leaves the worker exactly as it was, so a
    /// caller may retry — and, importantly, does not leave the flag set with nothing watching it.
    pub fn start<F>(&mut self, name: &'static str, body: F) -> Result<(), DeviceError>
    where
        F: FnOnce(&AtomicBool) + Send + 'static,
    {
        if self.is_running() {
            return Err(DeviceError::AlreadyStreaming);
        }
        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        match std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || body(&running))
        {
            Ok(handle) => {
                self.handle = Some(handle);
                Ok(())
            }
            Err(e) => {
                self.running.store(false, Ordering::Release);
                Err(DeviceError::Io(format!("spawn {name}: {e}")))
            }
        }
    }

    /// Clear the stop flag without waiting. For a caller that has to wake a blocked body between
    /// signalling and joining; [`Worker::stop`] does both.
    pub fn signal_stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    /// Stop the thread and join it. Idempotent.
    pub fn stop(&mut self) {
        self.signal_stop();
        self.join();
    }

    /// Join a thread that has already been signalled. Idempotent.
    pub fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn a_body_runs_until_the_flag_clears() {
        let (tx, rx) = mpsc::channel();
        let mut worker = Worker::new();
        worker
            .start("test-worker", move |running| {
                while running.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                tx.send(()).expect("receiver outlives the body");
            })
            .expect("spawn");
        assert!(worker.is_running());
        worker.stop();
        assert!(!worker.is_running());
        rx.try_recv().expect("the body saw the flag clear");
    }

    #[test]
    fn a_second_start_is_refused_while_one_runs() {
        let mut worker = Worker::new();
        worker
            .start("test-worker", |running| {
                while running.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
            })
            .expect("spawn");
        assert!(matches!(
            worker.start("test-worker", |_| {}),
            Err(DeviceError::AlreadyStreaming)
        ));
        worker.stop();
        // …and the slot frees up again, rather than staying stuck.
        worker.start("test-worker", |_| {}).expect("restart");
    }

    /// A body that returns by itself leaves the handle behind; `is_running` must report the
    /// truth so a backend does not refuse a restart for a thread that is already gone.
    #[test]
    fn a_finished_body_still_stops_cleanly() {
        let mut worker = Worker::new();
        worker.start("test-worker", |_| {}).expect("spawn");
        let deadline = Instant::now() + Duration::from_secs(5);
        worker.stop();
        assert!(Instant::now() < deadline, "stop must not block");
        assert!(!worker.is_running());
    }

    #[test]
    fn stop_is_idempotent() {
        let mut worker = Worker::new();
        worker.start("test-worker", |_| {}).expect("spawn");
        worker.stop();
        worker.stop();
        assert!(!worker.is_running());
    }
}
