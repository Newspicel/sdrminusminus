//! The transfer pump: a queue of bulk-IN transfers on its own thread, handing whole blocks of
//! bytes to one consumer over a bounded channel.
//!
//! Keeping several transfers in flight is what stops the radio's FIFO from overflowing — the
//! endpoint is never empty between completions — and the bounded blocking handoff is what stops
//! a busy consumer from losing samples silently instead of slowing the producer down. Both come
//! from the measurement in `PLAN-NATIVE-DRIVERS.md` §1.

use std::{
    ops::Deref,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender},
    },
    thread::JoinHandle,
    time::Duration,
};

use crate::{
    bulk::BulkIn,
    error::{Result, StreamError},
    policy::{Action, TransferPolicy},
};

/// How the pump is built. `queue_depth` doubles as the error threshold (see
/// [`TransferPolicy::new`]).
#[derive(Clone, Copy, Debug)]
pub struct StreamConfig {
    /// Bytes requested per USB transfer. Must be a multiple of the endpoint's max packet size.
    pub transfer_size: usize,
    /// Transfers kept in flight.
    pub queue_depth: usize,
    /// Blocks the consumer may fall behind by before the pump blocks.
    pub channel_depth: usize,
    /// How often a pump with nothing to do rechecks the stop flag.
    pub poll_interval: Duration,
    /// Thread name for the pump, so a stuck capture is identifiable in a backtrace.
    pub thread_name: &'static str,
}

impl StreamConfig {
    /// Defaults both vendored drivers agreed on: 16 in flight, 32 blocks of slack.
    #[must_use]
    pub const fn new(transfer_size: usize, thread_name: &'static str) -> Self {
        Self {
            transfer_size,
            queue_depth: 16,
            channel_depth: 32,
            poll_interval: Duration::from_millis(100),
            thread_name,
        }
    }
}

/// Counters for one streaming session. A device transfer carries no sequence number, so these
/// see USB-level loss only — never samples the radio dropped before the host saw them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamingStats {
    /// Completions consumed by the pump.
    pub received: u64,
    /// Completions handed to the consumer.
    pub processed: u64,
    /// Completions discarded because they errored. A zero-length success is neither.
    pub dropped: u64,
}

#[derive(Debug, Default)]
struct Shared {
    stopping: AtomicBool,
    received: AtomicU64,
    processed: AtomicU64,
    dropped: AtomicU64,
    error: OnceLock<StreamError>,
}

impl Shared {
    fn stats(&self) -> StreamingStats {
        StreamingStats {
            received: self.received.load(Ordering::Relaxed),
            processed: self.processed.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

/// One block of received bytes. Returns its buffer to the pump when dropped, so a steady stream
/// allocates nothing after the first few blocks.
#[derive(Debug)]
pub struct Block {
    bytes: Vec<u8>,
    recycle: Sender<Vec<u8>>,
}

impl Deref for Block {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        let _ = self.recycle.send(std::mem::take(&mut self.bytes));
    }
}

/// Stops a running stream from a thread that is not the consumer — the control thread holds one
/// while the capture thread owns the [`RxStream`] itself.
#[derive(Clone, Debug)]
pub struct Stopper {
    shared: Arc<Shared>,
}

impl Stopper {
    /// Ask the pump to finish. Returns immediately; the consumer sees the stream end.
    pub fn stop(&self) {
        self.shared.stopping.store(true, Ordering::Release);
    }
}

/// The consumer's end of a running stream.
#[derive(Debug)]
pub struct RxStream {
    /// Dropped by [`RxStream::stop`] to release a pump blocked on backpressure — the stop flag
    /// alone only reaches a pump that is waiting on the endpoint.
    rx: Option<Receiver<Block>>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl RxStream {
    /// Block until the next block arrives, or the stream ends. `None` means ended: consult
    /// [`RxStream::error`] to find out whether that was a fault or a requested stop.
    #[must_use]
    pub fn recv(&self) -> Option<Block> {
        self.rx.as_ref()?.recv().ok()
    }

    /// A handle that can stop this stream from another thread.
    #[must_use]
    pub fn stopper(&self) -> Stopper {
        Stopper {
            shared: self.shared.clone(),
        }
    }

    /// Live counters — readable while streaming, unlike `hackrf-nusb`'s stop-only stats.
    #[must_use]
    pub fn stats(&self) -> StreamingStats {
        self.shared.stats()
    }

    /// Why the stream ended, if it ended on its own.
    #[must_use]
    pub fn error(&self) -> Option<&StreamError> {
        self.shared.error.get()
    }

    /// Stop the pump and join it. Idempotent.
    pub fn stop(&mut self) -> StreamingStats {
        self.shared.stopping.store(true, Ordering::Release);
        self.rx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.shared.stats()
    }
}

impl Drop for RxStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start streaming from `bulk_in`.
///
/// The endpoint is un-stalled and filled with `queue_depth` transfers before the pump thread
/// starts, so the first completion is already on its way when this returns.
pub fn start<B: BulkIn>(mut bulk_in: B, config: StreamConfig) -> Result<RxStream> {
    bulk_in.clear_halt()?;
    for _ in 0..config.queue_depth {
        let buffer = bulk_in.allocate(config.transfer_size);
        bulk_in.submit(buffer);
    }

    let shared = Arc::new(Shared::default());
    let (tx, rx) = mpsc::sync_channel(config.channel_depth);
    let (recycle_tx, recycle_rx) = mpsc::channel();
    let pump = Pump {
        bulk_in,
        policy: TransferPolicy::new(u32::try_from(config.queue_depth).unwrap_or(u32::MAX)),
        shared: shared.clone(),
        recycle_tx,
        recycle_rx,
        queue_depth: config.queue_depth,
        poll_interval: config.poll_interval,
    };
    // A failed spawn drops the pump, and with it the endpoint, which cancels the transfers
    // queued above — so there is nothing to unwind by hand here.
    let worker = std::thread::Builder::new()
        .name(config.thread_name.to_string())
        .spawn(move || pump.run(tx))
        .map_err(StreamError::Spawn)?;

    Ok(RxStream {
        rx: Some(rx),
        shared,
        worker: Some(worker),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    /// Nothing completed within the poll interval.
    Idle,
    /// A completion was handled.
    Progress,
    /// The stream is over.
    Ended,
}

struct Pump<B: BulkIn> {
    bulk_in: B,
    policy: TransferPolicy,
    shared: Arc<Shared>,
    recycle_tx: Sender<Vec<u8>>,
    recycle_rx: Receiver<Vec<u8>>,
    queue_depth: usize,
    poll_interval: Duration,
}

impl<B: BulkIn> Pump<B> {
    fn run(mut self, tx: SyncSender<Block>) {
        while self.step(&tx) != Step::Ended {}
        // Close the channel before draining, so the consumer learns the stream is over now
        // rather than a queue's worth of cancellations later — the in-place restart is only
        // worth ~3 ms, and waiting for the drain would dominate it.
        drop(tx);
        self.drain();
    }

    fn step(&mut self, tx: &SyncSender<Block>) -> Step {
        // `wait_next_complete` panics on an empty queue, and an empty queue can never refill
        // itself, so this is the end of the stream either way.
        if self.bulk_in.pending() == 0 {
            return Step::Ended;
        }
        let stopping = self.shared.stopping.load(Ordering::Acquire);
        let Some(completion) = self.bulk_in.wait_next_complete(self.poll_interval) else {
            return if stopping { Step::Ended } else { Step::Idle };
        };
        self.shared.received.fetch_add(1, Ordering::Relaxed);

        match self.policy.on_completion(completion.status, stopping) {
            Action::Exit => Step::Ended,
            Action::GiveUp { attempts, error } => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                let _ = self.shared.error.set(StreamError::Transfers {
                    attempts,
                    source: error,
                });
                Step::Ended
            }
            Action::Resubmit => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    consecutive_errors = self.policy.consecutive_errors(),
                    "usb transfer failed; resubmitting"
                );
                self.bulk_in.submit(completion.buffer);
                Step::Progress
            }
            Action::Deliver => {
                let len = completion.actual_len.min(completion.buffer.len());
                if len == 0 {
                    self.bulk_in.submit(completion.buffer);
                    return Step::Progress;
                }
                let mut bytes = self.recycle_rx.try_recv().unwrap_or_default();
                bytes.clear();
                bytes.extend_from_slice(&completion.buffer[..len]);
                // Back in flight before the handoff, which may block: the device keeps filling
                // its FIFO whether or not the consumer is ready.
                self.bulk_in.submit(completion.buffer);
                self.shared.processed.fetch_add(1, Ordering::Relaxed);
                let block = Block {
                    bytes,
                    recycle: self.recycle_tx.clone(),
                };
                if tx.send(block).is_err() {
                    return Step::Ended;
                }
                Step::Progress
            }
        }
    }

    /// Cancel what is still in flight and collect the completions, so every buffer is back
    /// before the endpoint drops.
    fn drain(&mut self) {
        self.bulk_in.cancel_all();
        for _ in 0..self.queue_depth {
            if self.bulk_in.pending() == 0 {
                break;
            }
            if self
                .bulk_in
                .wait_next_complete(self.poll_interval)
                .is_none()
            {
                tracing::debug!("usb transfers still pending after cancel");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Mutex, MutexGuard},
    };

    use nusb::transfer::TransferError;

    use super::*;
    use crate::bulk::Completion;

    const TRANSFER_SIZE: usize = 64;
    const DEPTH: usize = 16;

    #[derive(Debug, Default)]
    struct FakeState {
        allocated: usize,
        clear_halt_calls: usize,
        cancel_calls: usize,
        queued: VecDeque<Vec<u8>>,
        scripted: VecDeque<std::result::Result<Vec<u8>, TransferError>>,
    }

    /// Scripted [`BulkIn`]: `push_data`/`push_error` queue what the next completions report, and
    /// an empty script behaves like an endpoint that timed out.
    #[derive(Clone, Debug, Default)]
    struct FakeBulkIn {
        state: Arc<Mutex<FakeState>>,
    }

    impl FakeBulkIn {
        fn state(&self) -> MutexGuard<'_, FakeState> {
            self.state.lock().unwrap_or_else(|e| e.into_inner())
        }

        fn push_data(&self, bytes: impl Into<Vec<u8>>) {
            self.state().scripted.push_back(Ok(bytes.into()));
        }

        fn push_error(&self, error: TransferError) {
            self.state().scripted.push_back(Err(error));
        }
    }

    impl BulkIn for FakeBulkIn {
        type Buffer = Vec<u8>;

        fn clear_halt(&mut self) -> Result<()> {
            self.state().clear_halt_calls += 1;
            Ok(())
        }

        fn allocate(&self, len: usize) -> Self::Buffer {
            self.state().allocated += 1;
            vec![0; len]
        }

        fn submit(&mut self, buffer: Self::Buffer) {
            self.state().queued.push_back(buffer);
        }

        fn pending(&self) -> usize {
            self.state().queued.len()
        }

        fn wait_next_complete(&mut self, _timeout: Duration) -> Option<Completion<Self::Buffer>> {
            let mut state = self.state();
            let scripted = state.scripted.pop_front()?;
            let mut buffer = state.queued.pop_front()?;
            Some(match scripted {
                Ok(data) => {
                    buffer.clear();
                    buffer.extend_from_slice(&data);
                    Completion {
                        actual_len: data.len(),
                        buffer,
                        status: Ok(()),
                    }
                }
                Err(error) => Completion {
                    buffer,
                    actual_len: 0,
                    status: Err(error),
                },
            })
        }

        fn cancel_all(&mut self) {
            let mut state = self.state();
            state.cancel_calls += 1;
            // A real endpoint completes every cancelled transfer; the drain loop waits for them.
            let cancelled = state.queued.len();
            state.scripted.clear();
            for _ in 0..cancelled {
                state.scripted.push_back(Err(TransferError::Cancelled));
            }
        }
    }

    fn config() -> StreamConfig {
        StreamConfig {
            transfer_size: TRANSFER_SIZE,
            queue_depth: DEPTH,
            channel_depth: 4,
            poll_interval: Duration::from_millis(1),
            thread_name: "test-pump",
        }
    }

    /// Build a pump directly rather than through [`start`], so `step` can be driven one
    /// completion at a time instead of racing a thread.
    fn pump(bulk_in: FakeBulkIn) -> (Pump<FakeBulkIn>, SyncSender<Block>, Receiver<Block>) {
        let mut bulk_in = bulk_in;
        for _ in 0..DEPTH {
            let buffer = bulk_in.allocate(TRANSFER_SIZE);
            bulk_in.submit(buffer);
        }
        let (recycle_tx, recycle_rx) = mpsc::channel();
        let (tx, rx) = mpsc::sync_channel(DEPTH);
        let pump = Pump {
            bulk_in,
            policy: TransferPolicy::new(DEPTH as u32),
            shared: Arc::new(Shared::default()),
            recycle_tx,
            recycle_rx,
            queue_depth: DEPTH,
            poll_interval: Duration::from_millis(1),
        };
        (pump, tx, rx)
    }

    #[test]
    fn starting_clears_the_halt_and_fills_the_queue() {
        let fake = FakeBulkIn::default();
        let state = fake.state.clone();
        let mut stream = start(fake, config()).expect("fake endpoint cannot fail");
        {
            let state = state.lock().expect("uncontended");
            assert_eq!(state.clear_halt_calls, 1);
            assert_eq!(state.allocated, DEPTH);
            assert_eq!(state.queued.len(), DEPTH);
        }
        stream.stop();
        assert_eq!(state.lock().expect("uncontended").cancel_calls, 1);
    }

    #[test]
    fn a_completion_reaches_the_consumer_and_the_transfer_goes_back_in_flight() {
        let fake = FakeBulkIn::default();
        fake.push_data([1, 2, 3, 4]);
        let (mut pump, tx, rx) = pump(fake.clone());

        assert_eq!(pump.step(&tx), Step::Progress);
        let block = rx.try_recv().expect("one block delivered");
        assert_eq!(&*block, &[1, 2, 3, 4]);
        assert_eq!(fake.state().queued.len(), DEPTH);
        assert_eq!(
            pump.shared.stats(),
            StreamingStats {
                received: 1,
                processed: 1,
                dropped: 0
            }
        );
    }

    #[test]
    fn dropped_blocks_are_recycled_instead_of_reallocated() {
        let fake = FakeBulkIn::default();
        fake.push_data([7, 7]);
        fake.push_data([8, 8]);
        let (mut pump, tx, rx) = pump(fake);

        assert_eq!(pump.step(&tx), Step::Progress);
        let first = rx.try_recv().expect("first block");
        let address = first.as_ptr();
        drop(first);

        assert_eq!(pump.step(&tx), Step::Progress);
        let second = rx.try_recv().expect("second block");
        assert_eq!(second.as_ptr(), address, "pump must not allocate per block");
    }

    /// The `PLAN-NATIVE-DRIVERS.md` §1 regression, at the pump level: a stall aborts everything
    /// queued behind it, and the stream has to survive the whole burst.
    #[test]
    fn a_stall_and_its_cancellation_fallout_keeps_the_stream_alive() {
        let fake = FakeBulkIn::default();
        fake.push_error(TransferError::Fault);
        for _ in 0..DEPTH - 1 {
            fake.push_error(TransferError::Cancelled);
        }
        fake.push_data([1, 2]);
        let (mut pump, tx, rx) = pump(fake.clone());

        for _ in 0..DEPTH {
            assert_eq!(pump.step(&tx), Step::Progress);
        }
        assert!(pump.shared.error.get().is_none(), "stream must still be up");
        assert_eq!(pump.step(&tx), Step::Progress);
        assert_eq!(&*rx.try_recv().expect("stream recovered"), &[1, 2]);
        assert_eq!(fake.state().queued.len(), DEPTH, "queue stayed full");
        assert_eq!(pump.shared.stats().dropped, DEPTH as u64);
    }

    #[test]
    fn a_full_queue_of_genuine_errors_ends_the_stream() {
        let fake = FakeBulkIn::default();
        for _ in 0..DEPTH {
            fake.push_error(TransferError::Fault);
        }
        let (mut pump, tx, _rx) = pump(fake);

        for _ in 0..DEPTH - 1 {
            assert_eq!(pump.step(&tx), Step::Progress);
        }
        assert_eq!(pump.step(&tx), Step::Ended);
        let error = pump.shared.error.get().expect("terminal error recorded");
        assert!(
            matches!(error, StreamError::Transfers { attempts, source } if *attempts == DEPTH as u32
                && *source == TransferError::Fault)
        );
        assert!(!error.is_disconnected());
    }

    #[test]
    fn an_unplug_is_reported_as_a_disconnect() {
        let fake = FakeBulkIn::default();
        for _ in 0..DEPTH {
            fake.push_error(TransferError::Disconnected);
        }
        let (mut pump, tx, _rx) = pump(fake);
        while pump.step(&tx) != Step::Ended {}
        assert!(
            pump.shared
                .error
                .get()
                .expect("terminal error recorded")
                .is_disconnected()
        );
    }

    #[test]
    fn cancellations_after_a_stop_end_the_stream_without_an_error() {
        let fake = FakeBulkIn::default();
        for _ in 0..DEPTH {
            fake.push_error(TransferError::Cancelled);
        }
        let (mut pump, tx, _rx) = pump(fake);
        pump.shared.stopping.store(true, Ordering::Release);

        assert_eq!(pump.step(&tx), Step::Ended);
        assert!(pump.shared.error.get().is_none());
        assert_eq!(pump.shared.stats().dropped, 0);
    }

    #[test]
    fn an_idle_endpoint_only_ends_the_stream_once_stopped() {
        let fake = FakeBulkIn::default();
        let (mut pump, tx, _rx) = pump(fake);
        assert_eq!(pump.step(&tx), Step::Idle);
        pump.shared.stopping.store(true, Ordering::Release);
        assert_eq!(pump.step(&tx), Step::Ended);
    }

    #[test]
    fn a_zero_length_completion_is_neither_delivered_nor_dropped() {
        let fake = FakeBulkIn::default();
        fake.push_data([]);
        let (mut pump, tx, rx) = pump(fake);
        assert_eq!(pump.step(&tx), Step::Progress);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            pump.shared.stats(),
            StreamingStats {
                received: 1,
                processed: 0,
                dropped: 0
            }
        );
    }

    #[test]
    fn a_gone_consumer_ends_the_pump() {
        let fake = FakeBulkIn::default();
        fake.push_data([1, 2]);
        let (mut pump, tx, rx) = pump(fake);
        drop(rx);
        assert_eq!(pump.step(&tx), Step::Ended);
    }

    #[test]
    fn draining_returns_every_transfer_before_the_endpoint_drops() {
        let fake = FakeBulkIn::default();
        let (mut pump, _tx, _rx) = pump(fake.clone());
        pump.drain();
        assert_eq!(fake.state().cancel_calls, 1);
        assert_eq!(fake.state().queued.len(), 0);
    }
}
