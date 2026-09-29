use std::sync::{
    Arc, Mutex, MutexGuard, PoisonError,
    atomic::{AtomicIsize, Ordering},
};

use num_complex::Complex;
use sdrmm_wire::{AgcGain, Capabilities, DeviceInfo, DeviceSettings, StreamScope};

pub type Sample = Complex<f32>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum DeviceError {
    #[error("device not found: {0}")]
    NotFound(String),
    #[error("unsupported setting: {0}")]
    Unsupported(String),
    #[error("device I/O error: {0}")]
    Io(String),
    #[error("the radio is no longer attached ({0})")]
    Disconnected(String),
    #[error("this radio is in use by another program ({0})")]
    InUse(String),
    #[error("this radio may not be opened by this user ({0})")]
    PermissionDenied(String),
    #[error("device is already streaming")]
    AlreadyStreaming,
    #[error("device is {active} and cannot start {requested} until that stops")]
    DuplexConflict {
        active: Direction,
        requested: Direction,
    },
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn single_rx_sink(sinks: Vec<RxSink>) -> Result<RxSink, DeviceError> {
    let count = sinks.len();
    match sinks.into_iter().next() {
        Some(sink) if count == 1 => Ok(sink),
        _ => Err(DeviceError::Unsupported(format!(
            "this device has 1 rx stream, got {count} sinks"
        ))),
    }
}

pub fn check_stream_settings(
    settings: &DeviceSettings,
    capabilities: &Capabilities,
) -> Result<(), DeviceError> {
    let scope = capabilities.per_stream;
    for entry in &settings.streams {
        let stream = entry.stream;
        if stream >= capabilities.rx_streams {
            return Err(DeviceError::Unsupported(format!(
                "streams[{stream}]: this device has {} rx streams",
                capabilities.rx_streams
            )));
        }
        if scope == StreamScope::default() {
            return Err(DeviceError::Unsupported(format!(
                "streams[{stream}]: this device declares no per-stream settings"
            )));
        }
        if entry.center_hz.is_some() && !scope.tuning {
            return Err(DeviceError::Unsupported(format!(
                "streams[{stream}].center_hz: this device's streams share one tuning"
            )));
        }
        if !entry.gains.is_empty() && !scope.gain {
            return Err(DeviceError::Unsupported(format!(
                "streams[{stream}].gains: this device's streams share one gain"
            )));
        }
        if entry.agc.is_some() && !scope.agc {
            return Err(DeviceError::Unsupported(format!(
                "streams[{stream}].agc: this device's streams share one AGC"
            )));
        }
        if entry.antenna.is_some() && !scope.antenna {
            return Err(DeviceError::Unsupported(format!(
                "streams[{stream}].antenna: this device's streams share one antenna"
            )));
        }
    }
    Ok(())
}

type PushFn = Box<dyn FnMut(&[Sample], u64) + Send>;
type ItemFn = Box<dyn FnMut(SinkItem<'_>) + Send>;
type FatalFn = Box<dyn FnOnce(DeviceError) + Send>;

#[derive(Debug)]
pub struct SinkRoom {
    free: AtomicIsize,
}

impl SinkRoom {
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        assert!(capacity <= isize::MAX as usize);
        Self {
            free: AtomicIsize::new(capacity as isize),
        }
    }

    #[must_use]
    pub fn free(&self) -> usize {
        self.free.load(Ordering::Acquire).max(0) as usize
    }

    pub fn took(&self, samples: usize) {
        self.free.fetch_sub(samples as isize, Ordering::AcqRel);
    }

    pub fn freed(&self, samples: usize) {
        self.free.fetch_add(samples as isize, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SinkItem<'a> {
    Samples { samples: &'a [Sample], index: u64 },
    Event(LaneEvent),
}

enum Handler {
    Samples(PushFn),
    Items(ItemFn),
}

impl Handler {
    fn samples(&mut self, samples: &[Sample], index: u64) {
        match self {
            Self::Samples(push_fn) => push_fn(samples, index),
            Self::Items(handler) => handler(SinkItem::Samples { samples, index }),
        }
    }

    fn event(&mut self, event: LaneEvent) {
        if let Self::Items(handler) = self {
            handler(SinkItem::Event(event));
        }
    }
}

pub struct RxSink {
    handler: Handler,
    fatal_fn: Option<FatalFn>,
    room: Option<Arc<SinkRoom>>,
    index: u64,
    marks: rtrb::Consumer<LaneMark>,
    poster: MarkPoster,
}

impl RxSink {
    fn build(handler: Handler, fatal_fn: Option<FatalFn>) -> Self {
        let (poster, marks) = MarkPoster::channel();
        Self {
            handler,
            fatal_fn,
            room: None,
            index: 0,
            marks,
            poster,
        }
    }

    #[must_use]
    pub fn new(push_fn: impl FnMut(&[Sample], u64) + Send + 'static) -> Self {
        Self::build(Handler::Samples(Box::new(push_fn)), None)
    }

    #[must_use]
    pub fn with_fatal_handler(
        push_fn: impl FnMut(&[Sample], u64) + Send + 'static,
        fatal_fn: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Self {
        Self::build(
            Handler::Samples(Box::new(push_fn)),
            Some(Box::new(fatal_fn)),
        )
    }

    #[must_use]
    pub fn with_items(
        handler: impl FnMut(SinkItem<'_>) + Send + 'static,
        fatal_fn: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Self {
        Self::build(Handler::Items(Box::new(handler)), Some(Box::new(fatal_fn)))
    }

    #[must_use]
    pub fn with_room(mut self, room: Arc<SinkRoom>) -> Self {
        self.room = Some(room);
        self
    }

    #[must_use]
    pub fn room(&self) -> Option<&Arc<SinkRoom>> {
        self.room.as_ref()
    }

    pub fn push(&mut self, samples: &[Sample]) {
        while let Ok(mark) = self.marks.pop() {
            self.handler.event(LaneEvent::Mark {
                at: self.index,
                mark,
            });
        }
        self.handler.samples(samples, self.index);
        self.index += samples.len() as u64;
    }

    pub fn dropped(&mut self, samples: u64) {
        self.index = self.index.saturating_add(samples);
    }

    pub fn dropped_estimate(&mut self, samples: u64, error: u64, scope: GapScope) {
        self.index = self.index.saturating_add(samples);
        self.realigned(Uncertainty::EstimatedGap, error, scope);
    }

    pub fn realigned(&mut self, cause: Uncertainty, error: u64, scope: GapScope) {
        self.handler.event(LaneEvent::Uncertain {
            at: self.index,
            error,
            scope,
            cause,
        });
    }

    pub fn mark(&mut self, mark: LaneMark) {
        self.handler.event(LaneEvent::Mark {
            at: self.index,
            mark,
        });
    }

    pub fn stamp_hardware(&mut self, ns: i64) {
        self.handler
            .event(LaneEvent::HardwareTime { at: self.index, ns });
    }

    #[must_use]
    pub fn mark_poster(&self) -> MarkPoster {
        self.poster.clone()
    }

    #[must_use]
    pub const fn index(&self) -> u64 {
        self.index
    }

    pub fn fail(&mut self, err: DeviceError) {
        if let Some(fatal_fn) = self.fatal_fn.take() {
            fatal_fn(err);
        }
    }

    #[must_use]
    pub fn share_failure(&mut self) -> FatalHandle {
        FatalHandle(Arc::new(Mutex::new(self.fatal_fn.take())))
    }
}

#[derive(Clone)]
pub struct FatalHandle(Arc<Mutex<Option<FatalFn>>>);

impl FatalHandle {
    pub fn fail(&self, err: DeviceError) {
        if let Some(fatal_fn) = lock(&self.0).take() {
            fatal_fn(err);
        }
    }
}

impl std::fmt::Debug for FatalHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FatalHandle")
    }
}

impl std::fmt::Debug for RxSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RxSink")
    }
}

pub trait DeviceDriver: Send + Sync {
    fn id(&self) -> &'static str;

    fn probe(&self) -> Vec<DeviceInfo>;

    fn probe_deep(&self) -> Vec<DeviceInfo> {
        self.probe()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError>;

    fn resolve(&self, _key: &str) -> Option<DeviceInfo> {
        None
    }
}

pub trait TxStream: Send {
    fn write(
        &mut self,
        samples: &[Sample],
        timeout: std::time::Duration,
        end_burst: bool,
    ) -> Result<usize, DeviceError> {
        self.write_channels(&[samples], timeout, end_burst)
    }

    fn write_channels(
        &mut self,
        channels: &[&[Sample]],
        timeout: std::time::Duration,
        end_burst: bool,
    ) -> Result<usize, DeviceError>;

    fn stop(&mut self) -> Result<(), DeviceError>;
}

pub trait SdrDevice: Send {
    fn capabilities(&self) -> &Capabilities;
    fn settings(&self) -> &DeviceSettings;
    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError>;
    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError>;
    fn rx_stop(&mut self);

    fn duplex(&self) -> Duplex {
        self.capabilities().duplex
    }

    fn in_flight_samples(&self) -> u64 {
        self.settings()
            .sample_rate
            .map_or(0, |rate| (rate * 0.1) as u64)
    }

    fn tx_start(&mut self) -> Result<Box<dyn TxStream>, DeviceError> {
        self.tx_start_channels(&[0])
    }

    fn tx_start_channels(&mut self, _channels: &[u32]) -> Result<Box<dyn TxStream>, DeviceError> {
        Err(DeviceError::Unsupported(
            "this device does not transmit".to_string(),
        ))
    }

    fn playback(&self) -> Option<Arc<PlaybackShared>> {
        None
    }

    fn agc_gains(&self) -> Result<Vec<AgcGain>, DeviceError> {
        Ok(Vec::new())
    }

    fn set_noise_source(&mut self, _on: bool) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported(
            "this radio carries no calibration noise source".to_string(),
        ))
    }

    fn sweep_start(&mut self, _plan: &SweepPlan, _sink: SweepSink) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported(
            "this radio has no firmware sweep; the scanner has to retune for every step"
                .to_string(),
        ))
    }

    fn sweep_stop(&mut self) {}
}

pub mod capture;
mod clock;
pub mod convert;
pub mod duplex;
mod marks;
#[cfg(feature = "net")]
pub mod net;
pub mod playback;
pub mod pool;
pub mod registry;
pub mod restart;
pub mod schedule;
pub mod sweep;
pub mod usb;
pub mod worker;
pub use capture::{
    BlockGap, Capture, CaptureConfig, CaptureRadio, CaptureStream, Next, StopHandle, StreamFailure,
    drain_stream,
};
pub use clock::{init_clock, now_ns};
pub use convert::{LutConverter, SampleConverter};
pub use duplex::DuplexState;
pub use marks::{
    GapScope, LaneEvent, LaneMark, MARK_SLOTS, MarkPoster, UNKNOWN_ERROR, Uncertainty,
};
pub use playback::PlaybackShared;
pub use pool::{Block, BlockPool};
pub use registry::DeviceRegistry;
pub use restart::{Recovery, RestartPolicy, SILENT_STREAM_TIMEOUT};
pub use schedule::Latency;
pub use sdrmm_wire::{Direction, Duplex};
pub use sweep::{SweepBand, SweepPlan, SweepSink};
pub use worker::Worker;

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    #[test]
    fn room_keeps_debits_when_a_live_producer_beats_a_release_notification() {
        let room = SinkRoom::new(8);
        room.took(8);
        room.took(3);
        assert_eq!(room.free(), 0);
        room.freed(8);
        assert_eq!(room.free(), 5);
        room.freed(3);
        assert_eq!(room.free(), 8);
    }

    struct NoopTx;

    impl TxStream for NoopTx {
        fn write_channels(
            &mut self,
            channels: &[&[Sample]],
            _timeout: std::time::Duration,
            _end_burst: bool,
        ) -> Result<usize, DeviceError> {
            Ok(channels.first().map_or(0, |samples| samples.len()))
        }

        fn stop(&mut self) -> Result<(), DeviceError> {
            Ok(())
        }
    }

    struct MultiTxDevice {
        capabilities: Capabilities,
        settings: DeviceSettings,
        opened: Vec<u32>,
    }

    impl SdrDevice for MultiTxDevice {
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &DeviceSettings {
            &self.settings
        }

        fn apply(&mut self, _settings: &DeviceSettings) -> Result<(), DeviceError> {
            Ok(())
        }

        fn rx_start(&mut self, _sinks: Vec<RxSink>) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("receive".to_string()))
        }

        fn rx_stop(&mut self) {}

        fn tx_start_channels(
            &mut self,
            channels: &[u32],
        ) -> Result<Box<dyn TxStream>, DeviceError> {
            self.opened = channels.to_vec();
            Ok(Box::new(NoopTx))
        }
    }

    #[test]
    fn default_tx_start_opens_only_channel_zero() {
        let mut capabilities = caps(0, StreamScope::default());
        capabilities.duplex = Duplex::Full;
        capabilities.tx_streams = 3;
        let mut device = MultiTxDevice {
            capabilities,
            settings: DeviceSettings::default(),
            opened: Vec::new(),
        };
        let mut stream = device.tx_start().expect("start channel zero");
        assert_eq!(device.opened, vec![0]);
        let samples = [Sample::new(0.0, 0.0); 4];
        assert_eq!(
            stream
                .write(&samples, std::time::Duration::ZERO, false)
                .expect("write one channel"),
            samples.len()
        );
    }

    #[test]
    fn fail_invokes_fatal_handler_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let mut sink = RxSink::with_fatal_handler(
            |_, _| {},
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
        sink.fail(DeviceError::Io("first".into()));
        sink.fail(DeviceError::Io("second".into()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn every_block_carries_the_index_of_its_first_sample() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let mut sink = RxSink::new(move |samples: &[Sample], index| {
            lock(&log).push((index, samples.len()));
        });
        let block = [Complex::new(0.0, 0.0); 8];
        sink.push(&block);
        sink.push(&block[..3]);
        sink.push(&block);
        assert_eq!(*lock(&seen), vec![(0, 8), (8, 3), (11, 8)]);
        assert_eq!(sink.index(), 19);
    }

    #[test]
    fn samples_the_radio_lost_step_the_index_without_a_push() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let mut sink = RxSink::new(move |samples: &[Sample], index| {
            lock(&log).push((index, samples.len()));
        });
        let block = [Complex::new(0.0, 0.0); 4];
        sink.push(&block);
        sink.dropped(1_000);
        sink.push(&block);
        assert_eq!(*lock(&seen), vec![(0, 4), (1_004, 4)]);
        assert_eq!(sink.index(), 1_008);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Seen {
        Samples { index: u64, len: usize },
        Event(LaneEvent),
    }

    fn item_sink() -> (RxSink, Arc<Mutex<Vec<Seen>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let sink = RxSink::with_items(
            move |item: SinkItem<'_>| {
                lock(&log).push(match item {
                    SinkItem::Samples { samples, index } => Seen::Samples {
                        index,
                        len: samples.len(),
                    },
                    SinkItem::Event(event) => Seen::Event(event),
                });
            },
            |_| {},
        );
        (sink, seen)
    }

    #[test]
    fn a_mark_posted_between_blocks_lands_on_the_next_block_index() {
        let (mut sink, seen) = item_sink();
        let poster = sink.mark_poster();
        let block = [Sample::new(0.0, 0.0); 8];
        sink.push(&block);
        let mark = LaneMark::NoiseSource {
            on: true,
            in_flight: 393_216,
        };
        poster.post(mark).expect("a free slot");
        sink.push(&block[..4]);
        sink.push(&block);
        assert_eq!(
            *lock(&seen),
            vec![
                Seen::Samples { index: 0, len: 8 },
                Seen::Event(LaneEvent::Mark { at: 8, mark }),
                Seen::Samples { index: 8, len: 4 },
                Seen::Samples { index: 12, len: 8 },
            ]
        );
    }

    #[test]
    fn an_estimated_gap_moves_the_index_and_reports_its_error() {
        let (mut sink, seen) = item_sink();
        let block = [Sample::new(0.0, 0.0); 4];
        sink.push(&block);
        sink.dropped_estimate(1_000, 250, GapScope::Lane);
        sink.push(&block);
        sink.realigned(Uncertainty::Rearmed, UNKNOWN_ERROR, GapScope::Device);
        assert_eq!(sink.index(), 1_008);
        assert_eq!(
            *lock(&seen),
            vec![
                Seen::Samples { index: 0, len: 4 },
                Seen::Event(LaneEvent::Uncertain {
                    at: 1_004,
                    error: 250,
                    scope: GapScope::Lane,
                    cause: Uncertainty::EstimatedGap,
                }),
                Seen::Samples {
                    index: 1_004,
                    len: 4
                },
                Seen::Event(LaneEvent::Uncertain {
                    at: 1_008,
                    error: UNKNOWN_ERROR,
                    scope: GapScope::Device,
                    cause: Uncertainty::Rearmed,
                }),
            ]
        );
    }

    #[test]
    fn a_driver_mark_and_a_hardware_stamp_land_on_the_current_index() {
        let (mut sink, seen) = item_sink();
        let block = [Sample::new(0.0, 0.0); 4];
        sink.push(&block);
        sink.dropped(6);
        sink.mark(LaneMark::Retuned { in_flight: 0 });
        sink.stamp_hardware(-42);
        sink.push(&block);
        assert_eq!(
            *lock(&seen),
            vec![
                Seen::Samples { index: 0, len: 4 },
                Seen::Event(LaneEvent::Mark {
                    at: 10,
                    mark: LaneMark::Retuned { in_flight: 0 },
                }),
                Seen::Event(LaneEvent::HardwareTime { at: 10, ns: -42 }),
                Seen::Samples { index: 10, len: 4 },
            ]
        );
    }

    #[test]
    fn a_sample_only_sink_ignores_lane_events() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let mut sink = RxSink::new(move |samples: &[Sample], index| {
            lock(&log).push((index, samples.len()));
        });
        let poster = sink.mark_poster();
        for _ in 0..MARK_SLOTS {
            poster
                .post(LaneMark::GainChanged { in_flight: 1 })
                .expect("a free slot");
        }
        let block = [Sample::new(0.0, 0.0); 4];
        sink.mark(LaneMark::Retuned { in_flight: 0 });
        sink.stamp_hardware(7);
        sink.realigned(Uncertainty::Reset, 3, GapScope::Device);
        sink.dropped_estimate(10, 10, GapScope::Lane);
        sink.push(&block);
        poster
            .post(LaneMark::GainChanged { in_flight: 1 })
            .expect("the push emptied the queue");
        assert_eq!(*lock(&seen), vec![(10, 4)]);
    }

    #[test]
    fn a_full_mark_queue_is_an_error_not_a_loss() {
        let (mut sink, seen) = item_sink();
        let poster = sink.mark_poster();
        for in_flight in 0..MARK_SLOTS as u64 {
            poster
                .post(LaneMark::GainChanged { in_flight })
                .expect("a free slot");
        }
        match poster.post(LaneMark::GainChanged { in_flight: 99 }) {
            Err(DeviceError::Io(message)) => assert_eq!(message, "lane marks full"),
            other => panic!("a full queue must be an Io error, got {other:?}"),
        }
        sink.push(&[Sample::new(0.0, 0.0)]);
        let marks: Vec<u64> = lock(&seen)
            .iter()
            .filter_map(|seen| match seen {
                Seen::Event(LaneEvent::Mark {
                    at: 0,
                    mark: LaneMark::GainChanged { in_flight },
                }) => Some(*in_flight),
                _ => None,
            })
            .collect();
        assert_eq!(marks, (0..MARK_SLOTS as u64).collect::<Vec<_>>());
        poster
            .post(LaneMark::GainChanged { in_flight: 99 })
            .expect("room again after the push");
    }

    #[test]
    fn in_flight_defaults_to_a_tenth_of_a_second_of_samples() {
        let mut device = MultiTxDevice {
            capabilities: caps(1, StreamScope::default()),
            settings: DeviceSettings::default(),
            opened: Vec::new(),
        };
        assert_eq!(device.in_flight_samples(), 0);
        device.settings.sample_rate = Some(2_400_000.0);
        assert_eq!(device.in_flight_samples(), 240_000);
    }

    #[test]
    fn fail_without_handler_is_a_noop() {
        let mut sink = RxSink::new(|_, _| {});
        sink.fail(DeviceError::Io("dropped".into()));
    }

    #[test]
    fn single_rx_sink_takes_exactly_one() {
        let mut sink = single_rx_sink(vec![RxSink::new(|_, _| {})]).expect("one sink");
        sink.push(&[Complex::new(0.0, 0.0)]);
    }

    #[test]
    fn single_rx_sink_refuses_any_other_count() {
        for count in [0, 2, 5] {
            let sinks: Vec<RxSink> = (0..count).map(|_| RxSink::new(|_, _| {})).collect();
            match single_rx_sink(sinks) {
                Err(DeviceError::Unsupported(message)) => {
                    assert!(message.contains(&count.to_string()), "{message}");
                }
                other => panic!("{count} sinks must be Unsupported, got {other:?}"),
            }
        }
    }

    fn caps(rx_streams: u32, per_stream: StreamScope) -> Capabilities {
        Capabilities {
            freq_ranges: Vec::new(),
            sample_rates: Vec::new(),
            sample_rate_ranges: Vec::new(),
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            bandwidth_ranges: Vec::new(),
            bandwidth_auto: false,
            bias_tee: false,
            agc: sdrmm_wire::Agc::None,
            extra: Vec::new(),
            ppm: false,
            duplex: Duplex::RxOnly,
            rx_streams,
            tx_streams: 0,
            per_stream,
            directional: None,
            dc_artifact: sdrmm_wire::DcArtifact::Operator,
            hardware_sweep: false,
            coherence: sdrmm_wire::Coherence::None,
            noise_source: sdrmm_wire::NoiseSource::None,
            retune_keeps_phase: false,
            rx_stream_choices: Vec::new(),
        }
    }

    fn with_streams(entries: Vec<sdrmm_wire::StreamSettings>) -> DeviceSettings {
        DeviceSettings {
            streams: entries,
            ..DeviceSettings::default()
        }
    }

    fn entry(stream: u32) -> sdrmm_wire::StreamSettings {
        sdrmm_wire::StreamSettings {
            stream,
            ..sdrmm_wire::StreamSettings::default()
        }
    }

    fn refused_naming(settings: &DeviceSettings, capabilities: &Capabilities, needle: &str) {
        match check_stream_settings(settings, capabilities) {
            Err(DeviceError::Unsupported(message)) => {
                assert!(message.contains(needle), "{message} lacks {needle}");
            }
            Ok(()) => panic!("{settings:?} must be refused"),
            Err(other) => panic!("must be Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn check_stream_settings_passes_an_empty_table_on_any_radio() {
        let settings = DeviceSettings::default();
        check_stream_settings(&settings, &caps(1, StreamScope::default())).expect("single-stream");
        let scoped = StreamScope {
            tuning: true,
            gain: true,
            antenna: true,
            agc: false,
        };
        check_stream_settings(&settings, &caps(4, scoped)).expect("multi-stream");
    }

    #[test]
    fn an_unscoped_radio_refuses_any_entry() {
        refused_naming(
            &with_streams(vec![entry(0)]),
            &caps(1, StreamScope::default()),
            "streams[0]",
        );
    }

    #[test]
    fn a_stream_the_radio_lacks_is_refused_whatever_the_scope() {
        let scoped = StreamScope {
            tuning: true,
            gain: true,
            antenna: true,
            agc: false,
        };
        refused_naming(
            &with_streams(vec![entry(2)]),
            &caps(2, scoped),
            "streams[2]",
        );
    }

    #[test]
    fn each_field_is_refused_exactly_where_its_scope_flag_is_off() {
        let gain_only = caps(
            4,
            StreamScope {
                tuning: false,
                gain: true,
                antenna: false,
                agc: false,
            },
        );
        let mut retune = entry(1);
        retune.center_hz = Some(433_920_000.0);
        refused_naming(&with_streams(vec![retune]), &gain_only, "center_hz");

        let mut antenna = entry(1);
        antenna.antenna = Some("RX2".to_string());
        refused_naming(&with_streams(vec![antenna]), &gain_only, "antenna");

        let mut gain = entry(1);
        gain.gains = vec![sdrmm_wire::GainValue::new(sdrmm_wire::GainKind::Lna, 12.0)];
        check_stream_settings(&with_streams(vec![gain.clone()]), &gain_only)
            .expect("gain is scoped per-stream");

        let tuning_only = caps(
            4,
            StreamScope {
                tuning: true,
                gain: false,
                antenna: false,
                agc: false,
            },
        );
        refused_naming(&with_streams(vec![gain]), &tuning_only, "gains");

        let mut agc = entry(1);
        agc.agc = Some(sdrmm_wire::AgcSetting::switched(true));
        refused_naming(&with_streams(vec![agc.clone()]), &gain_only, "agc");
        let agc_too = caps(
            4,
            StreamScope {
                agc: true,
                ..StreamScope::default()
            },
        );
        check_stream_settings(&with_streams(vec![agc]), &agc_too)
            .expect("agc is scoped per-stream");
    }
}
