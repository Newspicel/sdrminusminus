use std::{io, sync::Arc, thread::JoinHandle, time::Duration};

use rtrb::{Consumer, Producer, RingBuffer};

pub(crate) mod channel;
pub(crate) mod coherent;
pub(crate) mod recording;
pub(crate) mod spectrum;

#[cfg(test)]
mod tests;

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
