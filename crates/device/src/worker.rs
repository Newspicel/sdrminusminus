use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use crate::{
    DeviceError,
    schedule::{self, Latency},
};

#[derive(Debug, Default)]
pub struct Worker {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    #[must_use]
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

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
            .spawn(move || {
                schedule::claim(Latency::Critical);
                body(&running);
            }) {
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

    pub fn signal_stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    pub fn stop(&mut self) {
        self.signal_stop();
        self.join();
    }

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
        worker.start("test-worker", |_| {}).expect("restart");
    }

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
