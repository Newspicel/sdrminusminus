use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use rtrb::{Producer, RingBuffer};

pub(super) struct Reclaimer<T> {
    queue: Producer<T>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Reclaimer<T> {
    pub(super) fn new(mut release: impl FnMut(T) + Send + 'static) -> std::io::Result<Self> {
        let (queue, mut receiver) = RingBuffer::new(64);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let worker = std::thread::Builder::new()
            .name("sdrmm-retire".into())
            .spawn(move || {
                loop {
                    while let Ok(value) = receiver.pop() {
                        release(value);
                    }
                    if stop.load(Ordering::Acquire) {
                        while let Ok(value) = receiver.pop() {
                            release(value);
                        }
                        break;
                    }
                    std::thread::park_timeout(Duration::from_millis(20));
                }
            })?;
        Ok(Self {
            queue,
            stopped,
            worker: Some(worker),
        })
    }

    pub(super) fn available(&mut self) -> bool {
        self.queue.slots() > 0
            && self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
    }

    pub(super) fn retire(&mut self, value: T) {
        let result = self.queue.push(value);
        debug_assert!(result.is_ok());
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
}

impl<T> Drop for Reclaimer<T> {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if worker.join().is_err() {
                tracing::error!("resource retirement worker panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    struct BlockingDrop(mpsc::Sender<()>, mpsc::Receiver<()>);
    impl Drop for BlockingDrop {
        fn drop(&mut self) {
            self.0.send(()).expect("entered");
            self.1
                .recv_timeout(Duration::from_secs(5))
                .expect("release");
        }
    }

    #[test]
    fn blocking_cleanup_does_not_block_the_producer() {
        let (entered, waiting) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let mut reclaimer = Reclaimer::new(drop).expect("worker");
        assert!(reclaimer.available());
        reclaimer.retire(BlockingDrop(entered, resume));
        waiting
            .recv_timeout(Duration::from_secs(5))
            .expect("cleanup started");
        assert!(reclaimer.available());
        release.send(()).expect("release cleanup");
        drop(reclaimer);
    }
}
