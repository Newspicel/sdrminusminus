//! `sdrmm-device` — the device abstraction (PLAN §6): `DeviceDriver`/`SdrDevice` traits, the
//! capability model (re-exported from `wire`), and the `RxSink` that carries `cf32` from a
//! device thread into the engine's ring. Backends (soapy, rtlsdr, hackrf, virtual) implement
//! these; nothing here does I/O itself.

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
}

/// The engine hands a device an `RxSink`; the device's capture thread pushes blocks of IQ into
/// it. The closure typically writes into an SPSC ring and counts overruns — devices never see
/// the ring directly, so this crate stays free of a transport dependency. The push is per
/// *block*, not per sample, so the indirect call is off the sample-rate hot path.
/// The per-block delivery closure behind an [`RxSink`].
type PushFn = Box<dyn FnMut(&[Sample]) + Send>;

pub struct RxSink {
    push_fn: PushFn,
}

impl RxSink {
    #[must_use]
    pub fn new(push_fn: impl FnMut(&[Sample]) + Send + 'static) -> Self {
        Self {
            push_fn: Box::new(push_fn),
        }
    }

    /// Deliver one block of captured samples downstream.
    pub fn push(&mut self, samples: &[Sample]) {
        (self.push_fn)(samples);
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

/// An opened receiver. The RX half is live now; the TX half is declared but unimplemented
/// through the RX phases (PLAN §1, §6).
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
}

pub mod registry;
pub use registry::DeviceRegistry;
