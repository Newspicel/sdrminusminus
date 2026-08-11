//! `sdrmm-device` — the device abstraction (PLAN §6): `DeviceDriver`/`SdrDevice` traits, the
//! capability model (re-exported from `wire`), and the `RxSink` that carries `cf32` from a
//! device thread into the engine's ring. Backends (soapy, rtlsdr, hackrf, virtual) implement
//! these; nothing here does I/O itself.
//!
//! It also owns everything a backend would otherwise write for itself and get subtly wrong: the
//! [`Duplex`] arbitration that decides whether a radio may run a direction right now, the
//! [`Worker`] every capture thread is, the [`LutConverter`] every 8-bit radio needs, and the
//! [`Capture`] supervisor that restarts a stalled stream in place before the engine's
//! destructive fault path ever hears about it. A backend supplies what genuinely differs — how
//! to point *its* radio at a stream, and what *its* ADC codes mean — and nothing else.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use num_complex::Complex;
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, StreamScope};

/// Sample delivered by every backend: interleaved IQ as `Complex<f32>` (PLAN §7: one format
/// end-to-end, conversion happens at the device edge only).
pub type Sample = Complex<f32>;

/// Errors a driver or device can raise.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("device not found: {0}")]
    NotFound(String),
    #[error("unsupported setting: {0}")]
    Unsupported(String),
    #[error("device I/O error: {0}")]
    Io(String),
    #[error("device is already streaming")]
    AlreadyStreaming,
    /// The radio has both directions but cannot run them together (PLAN §6: half duplex).
    #[error("device is {active} and cannot start {requested} until that stops")]
    DuplexConflict {
        /// The direction holding the radio.
        active: Direction,
        /// The direction that was refused.
        requested: Direction,
    },
}

/// Take a lock whose poisoning carries no meaning.
///
/// A backend's device mutex is only ever held across control transfers, so a poisoned one holds
/// no half-written state worth refusing — and refusing would mean losing the radio for the rest
/// of the session over a panic somewhere else.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The one sink a single-stream backend takes from [`SdrDevice::rx_start`]'s per-stream list.
///
/// # Errors
/// [`DeviceError::Unsupported`] unless the count is exactly one: a silently dropped extra sink
/// would strand its stream's consumers waiting on samples that never come, and the engine sizes
/// the list from the same `rx_streams` the backend advertised, so a mismatch is a bug worth
/// naming.
pub fn single_rx_sink(sinks: Vec<RxSink>) -> Result<RxSink, DeviceError> {
    let count = sinks.len();
    match sinks.into_iter().next() {
        Some(sink) if count == 1 => Ok(sink),
        _ => Err(DeviceError::Unsupported(format!(
            "this device has 1 rx stream, got {count} sinks"
        ))),
    }
}

/// Pre-flight every backend's `apply` runs on the delta's `streams` entries, against the
/// [`StreamScope`] its capabilities declare.
///
/// Shared here rather than left to each backend because the failure it prevents is silent: a
/// backend that ignored an entry it cannot honour would merge the override into its reported
/// settings and change nothing — one lane of a coherent array told it retuned while the tuner
/// never moved. On every single-stream backend the scope is all-false, so any entry is refused.
///
/// # Errors
/// [`DeviceError::Unsupported`] naming the refused entry: a stream the radio does not have, any
/// entry on a radio that declares nothing per-stream, or a field the scope keeps radio-wide.
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

/// The engine hands a device an `RxSink`; the device's capture thread pushes blocks of IQ into
/// it. The closure typically writes into an SPSC ring and counts overruns — devices never see
/// the ring directly, so this crate stays free of a transport dependency. The push is per
/// *block*, not per sample, so the indirect call is off the sample-rate hot path.
/// The per-block delivery closure behind an [`RxSink`].
type PushFn = Box<dyn FnMut(&[Sample]) + Send>;
/// The one-shot unrecoverable-error report behind [`RxSink::fail`].
type FatalFn = Box<dyn FnOnce(DeviceError) + Send>;

pub struct RxSink {
    push_fn: PushFn,
    fatal_fn: Option<FatalFn>,
}

impl RxSink {
    /// Sink without a fatal handler — for tests and simple captures. [`RxSink::fail`] then
    /// discards the error, so real backends must get the engine-wired sink instead.
    #[must_use]
    pub fn new(push_fn: impl FnMut(&[Sample]) + Send + 'static) -> Self {
        Self {
            push_fn: Box::new(push_fn),
            fatal_fn: None,
        }
    }

    /// Sink whose [`RxSink::fail`] reports to `fatal_fn` (the engine's fault channel), so a
    /// dead capture surfaces as device-set state instead of vanishing with its thread.
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

    /// Deliver one block of captured samples downstream.
    pub fn push(&mut self, samples: &[Sample]) {
        (self.push_fn)(samples);
    }

    /// Report an unrecoverable stream error, right before the capture thread exits. Cold path;
    /// the handler is one-shot, so a second call is a no-op rather than a double report.
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

/// A backend that can enumerate and open devices (PLAN §6).
pub trait DeviceDriver: Send + Sync {
    /// Stable driver id: `"virtual"`, `"soapy"`, `"rtlsdr"`, `"hackrf"`.
    fn id(&self) -> &'static str;
    /// Enumerate currently-attached devices.
    fn probe(&self) -> Vec<DeviceInfo>;
    /// Open one device for exclusive use.
    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError>;

    /// Adopt a device this driver can address but no probe can find, from its key alone.
    ///
    /// A network receiver is named, not discovered: neither rtl_tcp nor SpyServer has a
    /// discovery protocol, so the only thing that can produce `10.0.0.5:1234` is an operator
    /// typing it. Everything above this crate still works in probe results — a device set is
    /// faulted when its device leaves the probe list, and a stored workspace binds by matching
    /// one — so a driver that adopts a key must also report it from [`DeviceDriver::probe`]
    /// afterwards, for as long as it is willing to open it.
    ///
    /// The key returned in [`DeviceInfo::key`] is the canonical one and may differ from the key
    /// asked for (a defaulted port, a lowercased host). Callers must use it rather than the one
    /// they passed, or the id they hold will not be the id the probe reports.
    ///
    /// The default refuses everything, which is right for every backend that enumerates real
    /// hardware: there, a key no probe found names a device that is not attached.
    fn resolve(&self, _key: &str) -> Option<DeviceInfo> {
        None
    }
}

/// A live transmit burst: samples in, nothing out.
///
/// The mirror of [`RxSink`], and deliberately a pull rather than a push — a transmitter runs at
/// the caller's pace, and the queue's backpressure is the only thing that keeps a burst on time.
///
/// Nothing in `engine`, `server` or the web UI holds one of these: PLAN §12a gates every
/// application-level transmit feature behind an authorized-use switch that has not been built,
/// and until it is, nothing above this crate holds a [`TxStream`] — whatever
/// `Capabilities::duplex` says the hardware has.
pub trait TxStream: Send {
    /// Queue `samples` for transmission, returning how many were accepted.
    ///
    /// A short return means `timeout` expired with the queue full; the caller keeps the rest and
    /// calls again. `end_burst` marks the samples as the end of a burst, so a radio that
    /// distinguishes "the host finished" from "the host fell behind" can be told which happened.
    ///
    /// # Errors
    /// [`DeviceError::Io`] if the transmit path gave up, or the stream is already stopped.
    fn write(
        &mut self,
        samples: &[Sample],
        timeout: std::time::Duration,
        end_burst: bool,
    ) -> Result<usize, DeviceError>;

    /// Send everything queued, then stop transmitting. Idempotent.
    ///
    /// # Errors
    /// [`DeviceError::Io`] if the queue could not be drained. The radio stops radiating either
    /// way — leaving it on the air is never the right outcome.
    fn stop(&mut self) -> Result<(), DeviceError>;
}

/// An opened radio.
///
/// The RX half is what the engine drives. The TX half is declared here from day one, as PLAN §6
/// always specified, and is implemented by the backends whose hardware has it — but no code path
/// above this crate calls it, and [`Duplex`] is what decides whether a given radio may run a
/// direction at all (PLAN §12a).
pub trait SdrDevice: Send {
    /// Serialized to the client as-is to drive backend-driven UI (PLAN §6).
    fn capabilities(&self) -> &Capabilities;
    /// Currently-applied settings.
    fn settings(&self) -> &DeviceSettings;
    /// Apply a settings delta (retune, gain, rate…). Absent fields are unchanged.
    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError>;
    /// Begin streaming, pushing captured IQ blocks into `sinks` — one per rx stream, in stream
    /// order — from the device's own thread(s).
    ///
    /// # Errors
    /// [`DeviceError::Unsupported`] when the sink count is not [`Capabilities::rx_streams`]
    /// (single-stream backends via [`single_rx_sink`]): dropping an extra sink would strand its
    /// stream's consumers silently.
    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError>;
    /// Stop streaming and join the capture thread.
    fn rx_stop(&mut self);

    /// Which directions this radio has, and whether it can run them at once.
    ///
    /// Read off [`Capabilities::duplex`], which the client already renders from, so the
    /// arbitration and the picture of the radio cannot disagree — a backend that overrode this
    /// and forgot its capabilities would refuse a direction the UI had just drawn a port for.
    /// The capability defaults to receive-only, so a backend that says nothing still cannot
    /// advertise a transmitter by omission.
    fn duplex(&self) -> Duplex {
        self.capabilities().duplex
    }

    /// Claim the radio for transmit.
    ///
    /// # Errors
    /// [`DeviceError::Unsupported`] on a receive-only radio — the default, so only a backend
    /// with a transmitter has to think about this — or [`DeviceError::DuplexConflict`] while a
    /// half-duplex radio is receiving.
    fn tx_start(&mut self) -> Result<Box<dyn TxStream>, DeviceError> {
        Err(DeviceError::Unsupported(
            "this device does not transmit".to_string(),
        ))
    }

    /// The replay transport, on a device that is a recording rather than a radio. `None` is the
    /// default, so only a playback backend has to think about it.
    ///
    /// The handle is shared, not copied: the control plane keeps it after the device has moved
    /// into its capture runtime, which is what lets a pause or a seek land — and a position be
    /// read for the snapshot — without taking the lock the capture thread is holding.
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
            sample_rate_range: None,
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            extra: Vec::new(),
            ppm: false,
            duplex: Duplex::RxOnly,
            rx_streams,
            tx_streams: 0,
            per_stream,
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

    /// A radio that declares nothing per-stream refuses even an empty entry: merging it would
    /// make `settings()` report a `streams` table the radio cannot mean anything by.
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
