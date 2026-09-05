use std::{io, sync::Arc, thread::JoinHandle, time::Duration};

use rtrb::{Consumer, Producer, RingBuffer};

pub(crate) mod channel;
pub(crate) mod coherent;
pub(crate) mod recording;
pub(crate) mod spectrum;

pub(crate) struct Publisher<T> {
    ready: Producer<Box<T>>,
    free: Consumer<Box<T>>,
    spare: Option<Box<T>>,
    worker: Option<JoinHandle<()>>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

impl<T: Send + 'static> Publisher<T> {
    pub(crate) fn new(
        name: &str,
        capacity: usize,
        mut make: impl FnMut() -> T,
        mut publish: impl FnMut(&mut T) + Send + 'static,
        mut poll: impl FnMut() + Send + 'static,
    ) -> io::Result<Self> {
        let (ready, mut pending) = RingBuffer::<Box<T>>::new(capacity);
        let (mut recycled, free) = RingBuffer::new(capacity);
        for _ in 0..capacity {
            recycled
                .push(Box::new(make()))
                .map_err(|_| io::Error::other("publication buffer pool initialization failed"))?;
        }
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = stopped.clone();
        let worker = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                loop {
                    poll();
                    while let Ok(mut packet) = pending.pop() {
                        publish(&mut packet);
                        if recycled.push(packet).is_err() {
                            return;
                        }
                    }
                    if stop.load(std::sync::atomic::Ordering::Acquire) {
                        while let Ok(mut packet) = pending.pop() {
                            publish(&mut packet);
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            })?;
        Ok(Self {
            ready,
            free,
            spare: None,
            worker: Some(worker),
            stopped,
        })
    }

    pub(crate) fn submit(&mut self, fill: impl FnOnce(&mut T)) -> bool {
        if self.worker.as_ref().is_none_or(JoinHandle::is_finished) {
            return false;
        }
        let Some(mut packet) = self.spare.take().or_else(|| self.free.pop().ok()) else {
            return false;
        };
        fill(&mut packet);
        match self.ready.push(packet) {
            Ok(()) => true,
            Err(rtrb::PushError::Full(packet)) => {
                self.spare = Some(packet);
                false
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn flush(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self.free.slots() != self.free.buffer().capacity() {
            assert!(
                std::time::Instant::now() < deadline,
                "publisher did not drain"
            );
            std::thread::yield_now();
        }
    }
}

impl<T> Drop for Publisher<T> {
    fn drop(&mut self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("publication worker panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};

    use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

    use super::*;

    #[global_allocator]
    static ALLOC: CountingAlloc = CountingAlloc::new();

    #[test]
    fn saturation_is_nonblocking_and_recycled_buffers_preserve_order_without_allocating() {
        let (entered, waiting) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let received = observed.clone();
        let mut first = true;
        let mut publisher = Publisher::new(
            "test-publish",
            4,
            || vec![0u64; 16],
            move |packet| {
                if first {
                    first = false;
                    entered.send(()).expect("signal worker entered");
                    resume
                        .recv_timeout(Duration::from_secs(5))
                        .expect("release worker");
                }
                received.lock().expect("results").push(packet[0]);
            },
            || {},
        )
        .expect("publisher");
        assert!(publisher.submit(|packet| packet[0] = 0));
        waiting
            .recv_timeout(Duration::from_secs(5))
            .expect("worker started");
        let mut accepted = [false; 4];
        assert_no_alloc("publication while consumer is blocked", || {
            for (index, result) in accepted.iter_mut().enumerate() {
                *result = publisher.submit(|packet| packet[0] = index as u64 + 1);
            }
        });
        release.send(()).expect("release");
        assert_eq!(accepted, [true, true, true, false]);
        publisher.flush();
        for sequence in 4..20 {
            assert_no_alloc("recycled publication", || {
                assert!(publisher.submit(|packet| packet[0] = sequence));
            });
            publisher.flush();
        }
        drop(publisher);
        assert_eq!(
            *observed.lock().expect("results"),
            (0..20).collect::<Vec<_>>()
        );
    }

    #[test]
    fn shutdown_drains_pending_publications() {
        let values = Arc::new(Mutex::new(Vec::new()));
        let received = values.clone();
        let mut publisher = Publisher::new(
            "test-drain",
            8,
            || 0,
            move |value| {
                received.lock().expect("values").push(*value);
            },
            || {},
        )
        .expect("publisher");
        for value in 0..8 {
            assert!(publisher.submit(|packet| *packet = value));
        }
        drop(publisher);
        assert_eq!(*values.lock().expect("values"), (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn recording_publication_failures_are_reported_without_allocating() {
        let (audio, _audio_blocks, audio_state) = crate::audio_recording::create_tap();
        let (iq, _position, _iq_blocks, iq_state) = crate::recording::create_tap();
        assert_no_alloc("recording failure reporting", || {
            audio.publication_failed();
            iq.publication_failed();
        });
        assert!(
            audio_state
                .error()
                .expect("audio fault")
                .contains("publication")
        );
        assert!(iq_state.error().expect("IQ fault").contains("publication"));
        assert!(!iq.healthy());
    }
}
