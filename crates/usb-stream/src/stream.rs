use std::{
    ops::Deref,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
    thread::JoinHandle,
    time::Duration,
};

use nusb::transfer::TransferError;

use crate::{
    bulk::BulkIn,
    error::{Result, StreamError},
    policy::{Action, TransferPolicy},
};

#[derive(Clone, Copy, Debug)]
pub struct StreamConfig {
    pub transfer_size: usize,
    pub queue_depth: usize,
    pub channel_depth: usize,
    pub poll_interval: Duration,
    pub thread_name: &'static str,
}

impl StreamConfig {
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamingStats {
    pub received: u64,
    pub processed: u64,
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

#[derive(Clone, Debug)]
pub struct Stopper {
    shared: Arc<Shared>,
}

impl Stopper {
    pub fn stop(&self) {
        self.shared.stopping.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
pub struct RxStream {
    rx: Option<Receiver<Block>>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl RxStream {
    pub fn recv_timeout(&self, timeout: Duration) -> std::result::Result<Block, RecvTimeoutError> {
        self.rx
            .as_ref()
            .ok_or(RecvTimeoutError::Disconnected)?
            .recv_timeout(timeout)
    }

    #[must_use]
    pub fn stopper(&self) -> Stopper {
        Stopper {
            shared: self.shared.clone(),
        }
    }

    #[must_use]
    pub fn stats(&self) -> StreamingStats {
        self.shared.stats()
    }

    #[must_use]
    pub fn error(&self) -> Option<&StreamError> {
        self.shared.error.get()
    }

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

pub fn start<B: BulkIn>(mut bulk_in: B, config: StreamConfig) -> Result<RxStream> {
    if config.queue_depth == 0 {
        return Err(StreamError::Config("queue_depth must be at least 1"));
    }
    if config.transfer_size == 0 {
        return Err(StreamError::Config("transfer_size must be at least 1"));
    }
    // A misaligned IN length makes every transfer complete with InvalidArgument, which would
    // surface as a stream that failed rather than as the configuration error it is.
    // Zero means the transport does not report a packet size, which only the test endpoints do.
    let max_packet = bulk_in.max_packet_size();
    if max_packet > 0 && !config.transfer_size.is_multiple_of(max_packet) {
        return Err(StreamError::Config(
            "transfer_size must be a multiple of the endpoint's maximum packet size",
        ));
    }
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
    Idle,
    Progress,
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
        drop(tx);
        self.drain();
    }

    fn step(&mut self, tx: &SyncSender<Block>) -> Step {
        if self.bulk_in.pending() == 0 {
            return Step::Ended;
        }
        let stopping = self.shared.stopping.load(Ordering::Acquire);
        let Some(completion) = self.bulk_in.wait_next_complete(self.poll_interval) else {
            return if stopping { Step::Ended } else { Step::Idle };
        };
        self.shared.received.fetch_add(1, Ordering::Relaxed);

        let status = completion.status;
        match self.policy.on_completion(status, stopping) {
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
                if status == Err(TransferError::Cancelled) {
                    tracing::debug!("usb transfer cancelled by a fault ahead of it; resubmitting");
                } else {
                    tracing::warn!(
                        consecutive_errors = self.policy.consecutive_errors(),
                        "usb transfer failed; resubmitting"
                    );
                }
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

    #[derive(Clone, Debug, Default)]
    struct FakeBulkIn {
        state: Arc<Mutex<FakeState>>,
        max_packet: usize,
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

        fn max_packet_size(&self) -> usize {
            self.max_packet
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
    fn an_unrunnable_config_is_refused_instead_of_ending_the_stream_silently() {
        for (config, what) in [
            (
                StreamConfig {
                    queue_depth: 0,
                    ..config()
                },
                "queue_depth",
            ),
            (
                StreamConfig {
                    transfer_size: 0,
                    ..config()
                },
                "transfer_size",
            ),
        ] {
            let error = start(FakeBulkIn::default(), config).expect_err(what);
            assert!(matches!(error, StreamError::Config(_)), "{what}: {error}");
            assert!(!error.is_disconnected());
        }
    }

    #[test]
    fn a_transfer_size_the_endpoint_cannot_carry_is_a_config_error_not_a_failed_stream() {
        let endpoint = FakeBulkIn {
            max_packet: 512,
            ..FakeBulkIn::default()
        };
        let misaligned = StreamConfig {
            transfer_size: 1000,
            ..config()
        };
        let error = start(endpoint, misaligned).expect_err("1000 is not a multiple of 512");
        assert!(matches!(error, StreamError::Config(_)), "{error}");

        let endpoint = FakeBulkIn {
            max_packet: 512,
            ..FakeBulkIn::default()
        };
        let aligned = StreamConfig {
            transfer_size: 16_384,
            ..config()
        };
        start(endpoint, aligned).expect("16384 is 32 whole packets");
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
