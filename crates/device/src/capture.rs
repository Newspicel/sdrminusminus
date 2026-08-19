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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamFailure {
    pub reason: String,
    /// The radio itself is gone, so restarting the stream has nothing to restart onto.
    pub gone: bool,
}

#[derive(Debug)]
pub enum Next<B> {
    Block(B),
    Idle,
    Ended,
}

pub trait StopHandle: Clone + Send + Sync + 'static {
    fn stop(&self);
}

pub trait CaptureStream: Send + 'static {
    type Block: Deref<Target = [u8]>;
    type Stop: StopHandle;

    fn stop_handle(&self) -> Self::Stop;
    fn next_block(&self, timeout: Duration) -> Next<Self::Block>;
    fn dropped(&self) -> u64;
    fn failure(&self) -> StreamFailure;
}

pub trait CaptureRadio: Send + Sync + 'static {
    type Stream: CaptureStream;

    fn arm(&self) -> Result<Self::Stream, DeviceError>;

    fn disarm(&self) {}
}

#[derive(Clone, Copy, Debug)]
pub struct CaptureConfig {
    pub thread_name: &'static str,
    pub radio: &'static str,
    pub block_samples: usize,
    pub poll: Duration,
    pub silence_timeout: Duration,
    pub restart: RestartPolicy,
}

impl CaptureConfig {
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

pub struct Capture<R: CaptureRadio> {
    worker: Worker,
    stop: Arc<Mutex<Option<<R::Stream as CaptureStream>::Stop>>>,
    radio: Option<Arc<R>>,
}

impl<R: CaptureRadio> std::fmt::Debug for Capture<R> {
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
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.worker.is_running()
    }

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
                *lock(&self.stop) = None;
                radio.disarm();
                Err(e)
            }
        }
    }

    pub fn stop(&mut self) {
        self.worker.signal_stop();
        if let Some(radio) = &self.radio {
            radio.disarm();
        }
        let stop = lock(&self.stop).clone();
        if let Some(stop) = stop {
            stop.stop();
        }
        self.worker.join();
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
        if failure.gone {
            sink.fail(DeviceError::Disconnected(failure.reason));
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
        if !running.load(Ordering::Acquire) {
            break;
        }
        match radio.arm() {
            Ok(fresh) => {
                *lock(published) = Some(fresh.stop_handle());
                if !running.load(Ordering::Acquire) {
                    break;
                }
                tracing::info!(radio = config.radio, attempt, "stream restarted");
                stream = fresh;
            }
            Err(DeviceError::Disconnected(reason)) => {
                sink.fail(DeviceError::Disconnected(reason));
                break;
            }
            Err(e) => {
                sink.fail(DeviceError::Io(format!("stream restart failed: {e}")));
                break;
            }
        }
    }
    radio.disarm();
}

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
                let samples = converter.convert(&block);
                let per_transfer = samples.len() as u64;
                let total = stream.dropped();
                if total > *dropped {
                    let lost = (total - *dropped) * per_transfer;
                    tracing::warn!(
                        radio = config.radio,
                        dropped = total,
                        lost,
                        "transport dropped transfers"
                    );
                    sink.dropped(lost);
                    *dropped = total;
                }
                for chunk in samples.chunks(chunk_size) {
                    sink.push(chunk);
                }
            }
            Next::Idle => {
                if last_block.elapsed() >= config.silence_timeout {
                    return Some(StreamFailure {
                        reason: format!("no samples for {:?}", config.silence_timeout),
                        gone: false,
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

    #[derive(Clone, Debug)]
    enum Step {
        Block(Vec<u8>),
        Quiet,
        Fail { reason: &'static str, gone: bool },
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
                Some(Step::Quiet) | None => Next::Idle,
                Some(Step::Fail { reason, gone }) => {
                    *lock(&self.failure) = Some(StreamFailure {
                        reason: reason.to_string(),
                        gone,
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
                gone: false,
            })
        }
    }

    #[derive(Debug)]
    struct FakeRadio {
        streams: Mutex<VecDeque<Vec<Step>>>,
        arms: AtomicUsize,
        disarms: AtomicUsize,
        armed: AtomicBool,
        refuse_arm_from: AtomicUsize,
        unplugged: AtomicBool,
        stop_on_failure: AtomicBool,
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
                unplugged: AtomicBool::new(false),
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

        fn stop_when_it_faults(&self) {
            self.stop_on_failure.store(true, Ordering::SeqCst);
        }

        fn refuse_arm_from(&self, n: usize) {
            self.refuse_arm_from.store(n, Ordering::SeqCst);
        }

        fn unplugged_from(&self, n: usize) {
            self.unplugged.store(true, Ordering::SeqCst);
            self.refuse_arm_from(n);
        }
    }

    impl CaptureRadio for FakeRadio {
        type Stream = FakeStream;

        fn arm(&self) -> Result<FakeStream, DeviceError> {
            let nth = self.arms.fetch_add(1, Ordering::SeqCst);
            if nth >= self.refuse_arm_from.load(Ordering::SeqCst) {
                return Err(if self.unplugged.load(Ordering::SeqCst) {
                    DeviceError::Disconnected("control transfer failed".to_string())
                } else {
                    DeviceError::Io("radio refused to arm".to_string())
                });
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

    #[derive(Debug)]
    struct Run {
        blocks: Vec<Vec<f32>>,
        fault: Option<String>,
    }

    fn supervised(radio: &Arc<FakeRadio>, config: CaptureConfig) -> Run {
        let running = Arc::new(AtomicBool::new(true));
        *lock(&radio.running) = Some(running.clone());
        let (block_tx, block_rx) = mpsc::channel();
        let (fault_tx, fault_rx) = mpsc::channel();
        let sink = RxSink::with_fatal_handler(
            move |samples: &[Sample], _index: u64| {
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
            gone: false,
        }]
    }

    #[test]
    fn blocks_reach_the_sink_and_a_device_that_is_gone_ends_the_capture() {
        let radio = FakeRadio::with([vec![
            Step::Block(vec![1, 2]),
            Step::Fail {
                reason: "unplugged",
                gone: true,
            },
        ]]);
        let run = supervised(&radio, config());
        assert_eq!(run.blocks, vec![vec![1.0, 2.0]]);
        assert_eq!(
            run.fault.as_deref(),
            Some("the radio is no longer attached (unplugged)")
        );
        assert_eq!(radio.arms(), 1);
        assert!(!radio.is_armed(), "the supervisor disarms on its way out");
    }

    #[test]
    fn a_transient_failure_restarts_in_place_and_keeps_delivering() {
        let radio = FakeRadio::with([
            vec![
                Step::Block(vec![1]),
                Step::Fail {
                    reason: "stalled",
                    gone: false,
                },
            ],
            vec![
                Step::Block(vec![2]),
                Step::Fail {
                    reason: "unplugged",
                    gone: true,
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

    #[test]
    fn a_radio_unplugged_mid_stream_reports_that_and_not_the_restart_it_tripped_over() {
        let radio = FakeRadio::with([stalled()]);
        radio.unplugged_from(1);
        let run = supervised(&radio, config());
        let fault = run.fault.expect("a fault");
        assert!(fault.contains("no longer attached"), "{fault}");
        assert!(
            !fault.contains("restart"),
            "an unplugged radio is not a restart failure: {fault}"
        );
    }

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
                gone: true,
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
        capture.stop();
        assert!(!capture.is_running());
    }

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
                    RxSink::new(|_, _| {}),
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
                RxSink::new(|_, _| {}),
                config(),
            )
            .expect("start");
        assert!(matches!(
            capture.start(
                radio.clone(),
                OneSamplePerByte::default(),
                RxSink::new(|_, _| {}),
                config()
            ),
            Err(DeviceError::AlreadyStreaming)
        ));
        capture.stop();
        let after = radio.disarms();
        capture.stop();
        assert_eq!(radio.disarms(), after, "a stopped capture holds no radio");
        capture
            .start(
                radio.clone(),
                OneSamplePerByte::default(),
                RxSink::new(|_, _| {}),
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
                RxSink::new(|_, _| {}),
                config(),
            )
            .expect_err("arm refused");
        assert!(matches!(error, DeviceError::Io(_)));
        assert!(!capture.is_running());
        assert!(!radio.is_armed());
    }
}
