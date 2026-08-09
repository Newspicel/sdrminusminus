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

use std::sync::{Mutex, MutexGuard, PoisonError};

use num_complex::Complex;
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

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
}

/// A live transmit burst: samples in, nothing out.
///
/// The mirror of [`RxSink`], and deliberately a pull rather than a push — a transmitter runs at
/// the caller's pace, and the queue's backpressure is the only thing that keeps a burst on time.
///
/// Nothing in `engine`, `server` or the web UI holds one of these: PLAN §12a gates every
/// application-level transmit feature behind an authorized-use switch that has not been built,
/// and until it is, `Capabilities::tx_capable` stays false everywhere.
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
    /// Begin streaming, pushing captured IQ blocks into `sink` from the device's own thread.
    fn rx_start(&mut self, sink: RxSink) -> Result<(), DeviceError>;
    /// Stop streaming and join the capture thread.
    fn rx_stop(&mut self);

    /// Which directions this radio has, and whether it can run them at once. Receive-only
    /// unless a backend says otherwise, so a device cannot advertise a transmitter by omission.
    fn duplex(&self) -> Duplex {
        Duplex::RxOnly
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
}

pub mod capture;
pub mod convert;
pub mod duplex;
pub mod registry;
pub mod restart;
pub mod worker;
pub use capture::{
    Capture, CaptureConfig, CaptureRadio, CaptureStream, Next, StopHandle, StreamFailure,
};
pub use convert::{LutConverter, SampleConverter};
pub use duplex::{Direction, Duplex, DuplexState};
pub use registry::DeviceRegistry;
pub use restart::{Recovery, RestartPolicy, SILENT_STREAM_TIMEOUT};
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
}
