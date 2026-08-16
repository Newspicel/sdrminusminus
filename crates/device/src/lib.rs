use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use num_complex::Complex;
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, StreamScope};

pub type Sample = Complex<f32>;

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("device not found: {0}")]
    NotFound(String),
    #[error("unsupported setting: {0}")]
    Unsupported(String),
    #[error("device I/O error: {0}")]
    Io(String),
    #[error("this radio is already in use — another program has it open ({0})")]
    InUse(String),
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
        if entry.antenna.is_some() && !scope.antenna {
            return Err(DeviceError::Unsupported(format!(
                "streams[{stream}].antenna: this device's streams share one antenna"
            )));
        }
    }
    Ok(())
}

type PushFn = Box<dyn FnMut(&[Sample]) + Send>;
type FatalFn = Box<dyn FnOnce(DeviceError) + Send>;

pub struct RxSink {
    push_fn: PushFn,
    fatal_fn: Option<FatalFn>,
}

impl RxSink {
    #[must_use]
    pub fn new(push_fn: impl FnMut(&[Sample]) + Send + 'static) -> Self {
        Self {
            push_fn: Box::new(push_fn),
            fatal_fn: None,
        }
    }

    #[must_use]
    pub fn with_fatal_handler(
        push_fn: impl FnMut(&[Sample]) + Send + 'static,
        fatal_fn: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Self {
        Self {
            push_fn: Box::new(push_fn),
            fatal_fn: Some(Box::new(fatal_fn)),
        }
    }

    pub fn push(&mut self, samples: &[Sample]) {
        (self.push_fn)(samples);
    }

    pub fn fail(&mut self, err: DeviceError) {
        if let Some(fatal_fn) = self.fatal_fn.take() {
            fatal_fn(err);
        }
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
}

pub mod capture;
pub mod convert;
pub mod duplex;
pub mod playback;
pub mod registry;
pub mod restart;
pub mod usb;
pub mod worker;
pub use capture::{
    Capture, CaptureConfig, CaptureRadio, CaptureStream, Next, StopHandle, StreamFailure,
};
pub use convert::{LutConverter, SampleConverter};
pub use duplex::DuplexState;
pub use playback::PlaybackShared;
pub use registry::DeviceRegistry;
pub use restart::{Recovery, RestartPolicy, SILENT_STREAM_TIMEOUT};
pub use sdrmm_wire::{Direction, Duplex};
pub use worker::Worker;

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

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
            |_| {},
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
        sink.fail(DeviceError::Io("first".into()));
        sink.fail(DeviceError::Io("second".into()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fail_without_handler_is_a_noop() {
        let mut sink = RxSink::new(|_| {});
        sink.fail(DeviceError::Io("dropped".into()));
    }

    #[test]
    fn single_rx_sink_takes_exactly_one() {
        let mut sink = single_rx_sink(vec![RxSink::new(|_| {})]).expect("one sink");
        sink.push(&[Complex::new(0.0, 0.0)]);
    }

    #[test]
    fn single_rx_sink_refuses_any_other_count() {
        for count in [0, 2, 5] {
            let sinks: Vec<RxSink> = (0..count).map(|_| RxSink::new(|_| {})).collect();
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
            extra: Vec::new(),
            ppm: false,
            duplex: Duplex::RxOnly,
            rx_streams,
            tx_streams: 0,
            per_stream,
            directional: None,
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
            },
        );
        let mut retune = entry(1);
        retune.center_hz = Some(433_920_000.0);
        refused_naming(&with_streams(vec![retune]), &gain_only, "center_hz");

        let mut antenna = entry(1);
        antenna.antenna = Some("RX2".to_string());
        refused_naming(&with_streams(vec![antenna]), &gain_only, "antenna");

        let mut gain = entry(1);
        gain.gains = vec![sdrmm_wire::GainValue {
            stage: "LNA".to_string(),
            value_db: 12.0,
        }];
        check_stream_settings(&with_streams(vec![gain.clone()]), &gain_only)
            .expect("gain is scoped per-stream");

        let tuning_only = caps(
            4,
            StreamScope {
                tuning: true,
                gain: false,
                antenna: false,
            },
        );
        refused_naming(&with_streams(vec![gain]), &tuning_only, "gains");
    }
}
