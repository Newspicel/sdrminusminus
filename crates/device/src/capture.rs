//! The capture thread and its tier-1 supervisor, written once for every backend that streams
//! blocks of bytes from a radio (PLAN §6, §18).
//!
//! Both native backends had their own copy of this loop, identical but for the restart
//! primitive, the block size and the word in the log line — and the copies had already diverged
//! on a real bug. What a backend still owns is what genuinely differs: how to point its radio at
//! a stream ([`CaptureRadio`]), and how its ADC codes become samples
//! ([`SampleConverter`](crate::SampleConverter)). Everything between those two is here.
//!
//! Recovery is two tiers. Tier 1 is this supervisor: the pipe faulted but the device never left
//! the bus, so the stream is re-armed in place under [`RestartPolicy`] — milliseconds. Tier 2 is
//! the engine's fault path, reached through [`RxSink::fail`] only once tier 1 is out of
//! attempts; it is one-shot and destructive by design (it drops the device so a replug can
//! re-open it), which is why tier 1 has to live *below* it, on the capture thread.
//!
//! Transport-agnostic: the only thing this asks of a stream is blocks of bytes, a stop handle
//! and an account of how it ended. The USB implementation lives behind the `usb` feature, so a
//! Soapy-only or virtual-only build never compiles a USB stack it cannot use.

use std::{
    ops::Deref,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    DeviceError, Recovery, RestartPolicy, RxSink, SILENT_STREAM_TIMEOUT, SampleConverter, Worker,
    lock,
};

#[cfg(feature = "usb")]
mod usb;

/// Why a stream stopped delivering when nobody asked it to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamFailure {
    /// Rendered into the log line, and into the fault the engine reports.
    pub reason: String,
    /// The device left the bus. Re-arming in place cannot help — the endpoint, the interface
    /// claim and the device handle went with it — so only tier 2 can recover.
    pub fatal: bool,
}

/// One step of a capture stream.
#[derive(Debug)]
pub enum Next<B> {
    /// Bytes arrived.
    Block(B),
    /// Nothing arrived within the timeout. Not an error, and not proof of anything: it is how
    /// the supervisor stays responsive to its stop flag, and how it notices a radio that has
    /// gone quiet without failing a single transfer.
    Idle,
    /// The stream is over; [`CaptureStream::failure`] says why.
    Ended,
}

/// Stops a running stream from a thread that is not the one draining it.
pub trait StopHandle: Clone + Send + Sync + 'static {
    /// Ask the stream to finish. Returns immediately; the drain side sees the stream end.
    fn stop(&self);
}

/// A running byte stream, as the supervisor sees it.
pub trait CaptureStream: Send + 'static {
    /// A block of received bytes.
    type Block: Deref<Target = [u8]>;
    /// Handle that ends this stream from another thread.
    type Stop: StopHandle;

    /// A handle that stops this stream.
    fn stop_handle(&self) -> Self::Stop;
    /// Wait up to `timeout` for the next block.
    fn next_block(&self, timeout: Duration) -> Next<Self::Block>;
    /// Transport-level blocks lost so far this stream. Counted separately from the engine's ring
    /// overruns: a dropped transfer is a gap in the samples that nothing downstream can see.
    fn dropped(&self) -> u64;
    /// Why the stream ended. Only meaningful after [`Next::Ended`].
    fn failure(&self) -> StreamFailure;
}

/// A radio that can point itself at a capture stream.
///
/// Implementations are shared between the control thread and the capture thread, so they carry
/// their own interior mutability — in practice a `Mutex` around the driver, taken only for
/// control transfers and never across a blocking read.
pub trait CaptureRadio: Send + Sync + 'static {
    /// The stream this radio produces.
    type Stream: CaptureStream;

    /// Point the radio at a fresh stream. Called once to start, and again for every in-place
    /// restart, so it must be safe to call after [`CaptureRadio::disarm`].
    ///
    /// # Errors
    /// Whatever the radio refuses with. An error here ends the capture: a restart that cannot
    /// re-arm has nothing left to try.
    fn arm(&self) -> Result<Self::Stream, DeviceError>;

    /// Stop the radio producing. Called after every capture ends, including a failed start, and
    /// again from the control thread once the capture thread is joined — so it must be
    /// idempotent, and must not disturb a stream running in the *other* direction.
    ///
    /// The default is for radios with nothing to switch off: dropping the stream is enough.
    fn disarm(&self) {}
}

/// How one backend's capture behaves.
#[derive(Clone, Copy, Debug)]
pub struct CaptureConfig {
    /// Thread name, so a stuck capture is identifiable in a backtrace.
    pub thread_name: &'static str,
    /// Radio id on every log line this supervisor writes: `"rtlsdr"`, `"hackrf"`.
    pub radio: &'static str,
    /// Samples per push into the sink. One USB transfer can be a coarse unit for a ring the DSP
    /// thread drains continuously, so a converted block is split before it goes downstream —
    /// small enough to keep latency low, large enough that the per-block indirect call stays off
    /// the sample-rate hot path. A block shorter than this is pushed whole.
    pub block_samples: usize,
    /// How often a quiet stream wakes the supervisor to re-check its stop flag.
    pub poll: Duration,
    /// How long a stream may deliver nothing before it counts as failed.
    pub silence_timeout: Duration,
    /// Attempt counting and backoff for in-place restarts.
    pub restart: RestartPolicy,
}

impl CaptureConfig {
    /// Defaults both native backends agreed on.
    #[must_use]
    pub fn new(thread_name: &'static str, radio: &'static str) -> Self {
        Self {
            thread_name,
            radio,
            block_samples: 32_768,
            poll: Duration::from_millis(100),
            silence_timeout: SILENT_STREAM_TIMEOUT,
            restart: RestartPolicy::default(),
        }
    }
}

/// A backend's running capture: the thread, the handle that stops its stream, and the radio it
/// is pointed at.
///
/// A backend embeds one and forwards `rx_start`/`rx_stop` to it. Holding the radio here is what
/// makes teardown safe: [`Capture::stop`] disarms *after* joining, so a supervisor that was
/// mid-restart cannot leave the radio armed behind a capture that no longer exists — and a
/// `Capture` that was never started holds no radio, so stopping it cannot disturb a stream
/// running the other way.
pub struct Capture<R: CaptureRadio> {
    worker: Worker,
    /// Republished by the supervisor on every restart, so a stop always reaches the stream that
    /// is actually running rather than the one it replaced.
    stop: Arc<Mutex<Option<<R::Stream as CaptureStream>::Stop>>>,
    /// Present exactly while a capture is running.
    radio: Option<Arc<R>>,
}

impl<R: CaptureRadio> std::fmt::Debug for Capture<R> {
    /// Hand-written because a stop handle is not required to be `Debug`, and demanding it of
    /// every transport to print one line would be the tail wagging the dog.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("running", &self.is_running())
            .finish_non_exhaustive()
    }
}

impl<R: CaptureRadio> Default for Capture<R> {
    fn default() -> Self {
        Self {
            worker: Worker::new(),
            stop: Arc::new(Mutex::new(None)),
            radio: None,
        }
    }
}

impl<R: CaptureRadio> Capture<R> {
    /// A capture with no thread.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a capture thread is running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.worker.is_running()
    }

    /// Arm `radio` and start draining it into `sink`.
    ///
    /// The stream is armed on the calling thread, so a radio that refuses reports through this
    /// `Result` instead of through the engine's fault path a moment later.
    ///
    /// # Errors
    /// [`DeviceError::AlreadyStreaming`] if a capture is already running, whatever
    /// [`CaptureRadio::arm`] refuses with, or [`DeviceError::Io`] if the thread cannot spawn.
    pub fn start<C: SampleConverter>(
        &mut self,
        radio: Arc<R>,
        converter: C,
        sink: RxSink,
        config: CaptureConfig,
    ) -> Result<(), DeviceError> {
        if self.is_running() {
            return Err(DeviceError::AlreadyStreaming);
        }
        let stream = radio.arm()?;
        *lock(&self.stop) = Some(stream.stop_handle());
        let stop = self.stop.clone();
        let armed = radio.clone();
        let started = self.worker.start(config.thread_name, move |running| {
            supervise(stream, &*armed, &stop, running, sink, converter, &config);
        });
        match started {
            Ok(()) => {
                self.radio = Some(radio);
                Ok(())
            }
            Err(e) => {
                // The un-spawned closure dropped the stream, which releases the transport's
                // endpoint — but the radio is still producing into it and has to be told to stop.
                *lock(&self.stop) = None;
                radio.disarm();
                Err(e)
            }
        }
    }

    /// Stop the capture and join its thread. Idempotent, and safe to call on a `Capture` that
    /// was never started.
    pub fn stop(&mut self) {
        self.worker.signal_stop();
        // The radio goes quiet before its transfers are cancelled, so the front end is not still
        // filling buffers that are about to be thrown away.
        if let Some(radio) = &self.radio {
            radio.disarm();
        }
        let stop = lock(&self.stop).clone();
        if let Some(stop) = stop {
            stop.stop();
        }
        self.worker.join();
        // Again, and this time it sticks: only after the join can nothing re-arm the radio. A
        // supervisor that was inside a restart when the stop landed may have armed it between
        // the two calls, and the supervisor's own disarm may have raced this one.
        if let Some(radio) = self.radio.take() {
            radio.disarm();
        }
        *lock(&self.stop) = None;
    }
}

impl<R: CaptureRadio> Drop for Capture<R> {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Drain `stream`, restarting it in place while the restart policy allows, until the caller asks
/// to stop or the failure is one no restart can fix.
fn supervise<R: CaptureRadio, C: SampleConverter>(
    mut stream: R::Stream,
    radio: &R,
    published: &Mutex<Option<<R::Stream as CaptureStream>::Stop>>,
    running: &AtomicBool,
    mut sink: RxSink,
    mut converter: C,
    config: &CaptureConfig,
) {
    let mut policy = config.restart;
    let mut dropped = 0u64;
    loop {
        let started = Instant::now();
        let Some(failure) = drain(
            &stream,
            running,
            &mut sink,
            &mut converter,
            &mut dropped,
            config,
        ) else {
            break;
        };
        if !running.load(Ordering::Acquire) {
            break;
        }
        if failure.fatal {
            sink.fail(DeviceError::Io(format!("device lost: {}", failure.reason)));
            break;
        }
        let Recovery::RetryAfter { attempt, delay } = policy.on_failure(started.elapsed()) else {
            sink.fail(DeviceError::Io(format!(
                "device lost after {} restart attempts: {}",
                policy.attempts() - 1,
                failure.reason
            )));
            break;
        };
        // A restart drops whatever the pipe had in flight, so it is never free and never silent.
        tracing::warn!(
            radio = config.radio,
            attempt,
            ?delay,
            reason = %failure.reason,
            "stream failed; restarting in place"
        );
        drop(stream);
        std::thread::sleep(delay);
        converter.reset();
        // Re-checked after the backoff: a stop that landed while this thread slept must not
        // re-arm the radio behind the control thread's back.
        if !running.load(Ordering::Acquire) {
            break;
        }
        match radio.arm() {
            Ok(fresh) => {
                // Published before the re-check, so a concurrent stop either ends this stream or
                // is seen below. One of the two always happens.
                *lock(published) = Some(fresh.stop_handle());
                if !running.load(Ordering::Acquire) {
                    break;
                }
                tracing::info!(radio = config.radio, attempt, "stream restarted");
                stream = fresh;
            }
            Err(e) => {
                sink.fail(DeviceError::Io(format!("stream restart failed: {e}")));
                break;
            }
        }
    }
    // Every exit path, including the ones nobody is waiting on: the radio stops producing before
    // this thread goes away. The control thread disarms again after joining, which is what makes
    // the pair safe rather than either one alone.
    radio.disarm();
}

/// Consume blocks until the stream ends or goes quiet. `None` means the caller asked to stop.
fn drain<S: CaptureStream, C: SampleConverter>(
    stream: &S,
    running: &AtomicBool,
    sink: &mut RxSink,
    converter: &mut C,
    dropped: &mut u64,
    config: &CaptureConfig,
) -> Option<StreamFailure> {
    let chunk_size = config.block_samples.max(1);
    let mut last_block = Instant::now();
    while running.load(Ordering::Acquire) {
        match stream.next_block(config.poll) {
            Next::Block(block) => {
                last_block = Instant::now();
                for chunk in converter.convert(&block).chunks(chunk_size) {
                    sink.push(chunk);
                }
                let total = stream.dropped();
                if total > *dropped {
                    tracing::warn!(
                        radio = config.radio,
                        dropped = total,
                        "transport dropped transfers"
                    );
                    *dropped = total;
                }
            }
            Next::Idle => {
                // A streaming radio free-runs and cannot go quiet while healthy, and an unplug
                // fails its queued transfers rather than going silent — so this fires only for a
                // board that has wedged with no error to report, which would otherwise park this
                // thread forever behind a waterfall the device set still advertises as running.
                if last_block.elapsed() >= config.silence_timeout {
                    return Some(StreamFailure {
                        reason: format!("no samples for {:?}", config.silence_timeout),
                        fatal: false,
                    });
                }
            }
            Next::Ended => return Some(stream.failure()),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
    };

    use super::*;
    use crate::Sample;

    /// What a scripted stream does next.
    #[derive(Clone, Debug)]
    enum Step {
        Block(Vec<u8>),
        Quiet,
        Fail { reason: &'static str, fatal: bool },
    }

    #[derive(Clone, Debug, Default)]
    struct FakeStop {
        stopped: Arc<AtomicBool>,
    }

    impl StopHandle for FakeStop {
        fn stop(&self) {
            self.stopped.store(true, Ordering::Release);
        }
    }

    #[derive(Debug)]
    struct FakeStream {
        steps: Mutex<VecDeque<Step>>,
        failure: Mutex<Option<StreamFailure>>,
        stop: FakeStop,
        /// Cleared when this stream faults, to model a stop landing at the same moment.
        stop_on_failure: Option<Arc<AtomicBool>>,
    }

    impl CaptureStream for FakeStream {
        type Block = Vec<u8>;
        type Stop = FakeStop;

        fn stop_handle(&self) -> FakeStop {
            self.stop.clone()
        }

        fn next_block(&self, _timeout: Duration) -> Next<Vec<u8>> {
            match lock(&self.steps).pop_front() {
                Some(Step::Block(bytes)) => Next::Block(bytes),
                // An exhausted script behaves like a radio that has gone quiet.
                Some(Step::Quiet) | None => Next::Idle,
                Some(Step::Fail { reason, fatal }) => {
                    *lock(&self.failure) = Some(StreamFailure {
                        reason: reason.to_string(),
                        fatal,
                    });
                    if let Some(running) = &self.stop_on_failure {
                        running.store(false, Ordering::Release);
                    }
                    Next::Ended
                }
            }
        }

        fn dropped(&self) -> u64 {
            0
        }

        fn failure(&self) -> StreamFailure {
            lock(&self.failure).take().unwrap_or(StreamFailure {
                reason: "ended".to_string(),
                fatal: false,
            })
        }
    }

    /// One scripted stream per `arm`, and a count of how often the radio was pointed each way.
    #[derive(Debug)]
    struct FakeRadio {
        streams: Mutex<VecDeque<Vec<Step>>>,
        arms: AtomicUsize,
        disarms: AtomicUsize,
        armed: AtomicBool,
        /// Arm number from which the radio starts refusing; never, by default.
        refuse_arm_from: AtomicUsize,
        /// Whether a stream should clear the supervisor's stop flag as it faults.
        stop_on_failure: AtomicBool,
        /// The supervisor's stop flag, so a scripted stream can clear it.
        running: Mutex<Option<Arc<AtomicBool>>>,
    }

    impl FakeRadio {
        fn with(streams: impl IntoIterator<Item = Vec<Step>>) -> Arc<Self> {
            Arc::new(Self {
                streams: Mutex::new(streams.into_iter().collect()),
                arms: AtomicUsize::new(0),
                disarms: AtomicUsize::new(0),
                armed: AtomicBool::new(false),
                refuse_arm_from: AtomicUsize::new(usize::MAX),
                stop_on_failure: AtomicBool::new(false),
                running: Mutex::new(None),
            })
        }

        fn arms(&self) -> usize {
            self.arms.load(Ordering::SeqCst)
        }

        fn disarms(&self) -> usize {
            self.disarms.load(Ordering::SeqCst)
        }

        fn is_armed(&self) -> bool {
            self.armed.load(Ordering::SeqCst)
        }

        /// Model a stop landing at the exact moment the stream faults.
        fn stop_when_it_faults(&self) {
            self.stop_on_failure.store(true, Ordering::SeqCst);
        }

        /// Refuse every arm from the `n`th onward, counting the first as zero.
        fn refuse_arm_from(&self, n: usize) {
            self.refuse_arm_from.store(n, Ordering::SeqCst);
        }
    }

    impl CaptureRadio for FakeRadio {
        type Stream = FakeStream;

        fn arm(&self) -> Result<FakeStream, DeviceError> {
            let nth = self.arms.fetch_add(1, Ordering::SeqCst);
            if nth >= self.refuse_arm_from.load(Ordering::SeqCst) {
                return Err(DeviceError::Io("radio refused to arm".to_string()));
            }
            self.armed.store(true, Ordering::SeqCst);
            Ok(FakeStream {
                steps: Mutex::new(lock(&self.streams).pop_front().unwrap_or_default().into()),
                failure: Mutex::new(None),
                stop: FakeStop::default(),
                stop_on_failure: self
                    .stop_on_failure
                    .load(Ordering::SeqCst)
                    .then(|| lock(&self.running).clone())
                    .flatten(),
            })
        }

        fn disarm(&self) {
            self.disarms.fetch_add(1, Ordering::SeqCst);
            self.armed.store(false, Ordering::SeqCst);
        }
    }

    /// One sample per byte, so a test can count what reached the sink.
    #[derive(Debug, Default)]
    struct OneSamplePerByte {
        out: Vec<Sample>,
        resets: usize,
    }

    impl SampleConverter for OneSamplePerByte {
        fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
            self.out.clear();
            self.out
                .extend(bytes.iter().map(|&b| Sample::new(f32::from(b), 0.0)));
            &self.out
        }

        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    fn config() -> CaptureConfig {
        CaptureConfig {
            poll: Duration::from_millis(1),
            restart: RestartPolicy {
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
                ..RestartPolicy::default()
            },
            ..CaptureConfig::new("test-capture", "fake")
        }
    }

    /// What one supervised run delivered: the blocks pushed, and the fault it reported, if any.
    #[derive(Debug)]
    struct Run {
        blocks: Vec<Vec<f32>>,
        fault: Option<String>,
    }

    /// Drive [`supervise`] on this thread. The script decides when the run ends, so no test here
    /// waits on a clock or a race.
    fn supervised(radio: &Arc<FakeRadio>, config: CaptureConfig) -> Run {
        let running = Arc::new(AtomicBool::new(true));
        // Handed to every scripted stream, so a test that asked for it can have a stop land at
        // the exact moment the stream faults.
        *lock(&radio.running) = Some(running.clone());
        let (block_tx, block_rx) = mpsc::channel();
        let (fault_tx, fault_rx) = mpsc::channel();
        let sink = RxSink::with_fatal_handler(
            move |samples: &[Sample]| {
                let _ = block_tx.send(samples.iter().map(|s| s.re).collect::<Vec<_>>());
            },
            move |err| {
                let _ = fault_tx.send(err.to_string());
            },
        );
        let stream = radio.arm().expect("first arm");
        let published = Mutex::new(Some(stream.stop_handle()));
        supervise(
            stream,
            &**radio,
            &published,
            &running,
            sink,
            OneSamplePerByte::default(),
            &config,
        );
        Run {
            blocks: block_rx.try_iter().collect(),
            fault: fault_rx.try_recv().ok(),
        }
    }

    fn stalled() -> Vec<Step> {
        vec![Step::Fail {
            reason: "stalled",
            fatal: false,
        }]
    }

    #[test]
    fn blocks_reach_the_sink_and_a_fatal_failure_ends_the_capture() {
        let radio = FakeRadio::with([vec![
            Step::Block(vec![1, 2]),
            Step::Fail {
                reason: "unplugged",
                fatal: true,
            },
        ]]);
        let run = supervised(&radio, config());
        assert_eq!(run.blocks, vec![vec![1.0, 2.0]]);
        assert_eq!(
            run.fault.as_deref(),
            Some("device I/O error: device lost: unplugged")
        );
        // A device that left the bus is never re-armed: tier 1 cannot help.
        assert_eq!(radio.arms(), 1);
        assert!(!radio.is_armed(), "the supervisor disarms on its way out");
    }

    /// The whole point of tier 1: a transient stall costs a re-arm, not the device.
    #[test]
    fn a_transient_failure_restarts_in_place_and_keeps_delivering() {
        let radio = FakeRadio::with([
            vec![
                Step::Block(vec![1]),
                Step::Fail {
                    reason: "stalled",
                    fatal: false,
                },
            ],
            vec![
                Step::Block(vec![2]),
                Step::Fail {
                    reason: "unplugged",
                    fatal: true,
                },
            ],
        ]);
        let run = supervised(&radio, config());
        assert_eq!(run.blocks, vec![vec![1.0], vec![2.0]]);
        assert_eq!(radio.arms(), 2, "the stream was re-armed once");
        assert!(
            run.fault.is_some_and(|fault| fault.contains("unplugged")),
            "only the unrecoverable failure reaches the engine"
        );
    }

    /// A stop that lands on the same failure must not re-arm the radio the control thread is
    /// already tearing down.
    #[test]
    fn a_stop_racing_a_failure_never_re_arms() {
        let radio = FakeRadio::with([stalled(), stalled()]);
        radio.stop_when_it_faults();
        let run = supervised(&radio, config());
        assert_eq!(
            radio.arms(),
            1,
            "the radio must not be re-armed after a stop"
        );
        assert!(!radio.is_armed());
        assert!(run.fault.is_none(), "a requested stop is not a fault");
    }

    #[test]
    fn restarts_run_out_and_the_engine_hears_about_it() {
        let radio = FakeRadio::with([stalled(), stalled(), stalled(), stalled()]);
        let run = supervised(&radio, config());
        // Three attempts on top of the original arm, then the fault.
        assert_eq!(radio.arms(), 4);
        let fault = run.fault.expect("a fault after the last attempt");
        assert!(fault.contains("after 3 restart attempts"), "{fault}");
        assert!(!radio.is_armed());
    }

    #[test]
    fn a_radio_that_cannot_re_arm_reports_instead_of_spinning() {
        let radio = FakeRadio::with([stalled()]);
        radio.refuse_arm_from(1);
        let run = supervised(&radio, config());
        let fault = run.fault.expect("a fault");
        assert!(fault.contains("stream restart failed"), "{fault}");
    }

    /// A wedged board reports no error at all; without this the capture thread would park
    /// forever behind a device set the engine still advertises as running.
    #[test]
    fn a_silent_stream_is_treated_as_a_failure() {
        let quiet = || vec![Step::Quiet];
        let radio = FakeRadio::with([quiet(), quiet(), quiet(), quiet()]);
        let run = supervised(
            &radio,
            CaptureConfig {
                silence_timeout: Duration::ZERO,
                ..config()
            },
        );
        assert_eq!(
            radio.arms(),
            4,
            "silence is restarted like any other failure"
        );
        let fault = run.fault.expect("silence eventually faults the device");
        assert!(fault.contains("no samples for"), "{fault}");
    }

    #[test]
    fn a_converted_block_is_split_into_sink_sized_pushes() {
        let radio = FakeRadio::with([vec![
            Step::Block(vec![1, 2, 3, 4, 5]),
            Step::Fail {
                reason: "done",
                fatal: true,
            },
        ]]);
        let run = supervised(
            &radio,
            CaptureConfig {
                block_samples: 2,
                ..config()
            },
        );
        assert_eq!(
            run.blocks,
            vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0]],
            "the tail is pushed short rather than held back"
        );
    }

    #[test]
    fn a_capture_that_never_started_holds_no_radio_to_disturb() {
        let mut capture = Capture::<FakeRadio>::new();
        assert!(!capture.is_running());
        // The property a half-duplex radio depends on: stopping a capture that is not running
        // must not reach the radio at all, or it would silence a burst going the other way.
        capture.stop();
        assert!(!capture.is_running());
    }

    /// The teardown invariant, whatever the interleaving: once `stop` returns, the radio is not
    /// armed. A supervisor that was mid-restart when the stop landed cannot leave it that way,
    /// because the control thread disarms *after* the join.
    #[test]
    fn stopping_leaves_the_radio_disarmed_however_it_raced() {
        for _ in 0..32 {
            let quiet = || vec![Step::Quiet];
            let radio = FakeRadio::with([stalled(), quiet(), quiet(), quiet()]);
            let mut capture = Capture::new();
            capture
                .start(
                    radio.clone(),
                    OneSamplePerByte::default(),
                    RxSink::new(|_| {}),
                    config(),
                )
                .expect("start");
            capture.stop();
            assert!(!capture.is_running());
            assert!(!radio.is_armed(), "the radio was left armed after a stop");
            assert!(radio.disarms() > 0);
        }
    }

    #[test]
    fn a_second_start_is_refused_and_a_second_stop_is_a_no_op() {
        let quiet = || vec![Step::Quiet];
        let radio = FakeRadio::with([quiet(), quiet()]);
        let mut capture = Capture::new();
        capture
            .start(
                radio.clone(),
                OneSamplePerByte::default(),
                RxSink::new(|_| {}),
                config(),
            )
            .expect("start");
        assert!(matches!(
            capture.start(
                radio.clone(),
                OneSamplePerByte::default(),
                RxSink::new(|_| {}),
                config()
            ),
            Err(DeviceError::AlreadyStreaming)
        ));
        capture.stop();
        let after = radio.disarms();
        capture.stop();
        assert_eq!(radio.disarms(), after, "a stopped capture holds no radio");
        // …and the device can stream again, rather than being stuck.
        capture
            .start(
                radio.clone(),
                OneSamplePerByte::default(),
                RxSink::new(|_| {}),
                config(),
            )
            .expect("restart");
    }

    #[test]
    fn a_radio_that_refuses_to_arm_never_starts_a_thread() {
        let radio = FakeRadio::with([]);
        radio.refuse_arm_from(0);
        let mut capture = Capture::new();
        let error = capture
            .start(
                radio.clone(),
                OneSamplePerByte::default(),
                RxSink::new(|_| {}),
                config(),
            )
            .expect_err("arm refused");
        assert!(matches!(error, DeviceError::Io(_)));
        assert!(!capture.is_running());
        assert!(!radio.is_armed());
    }
}
