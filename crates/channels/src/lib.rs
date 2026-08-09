//! `sdrmm-channels` — the `ChannelRx` plugin surface (PLAN §8). Depends only on `dsp` + `wire`.
//! No demodulators exist yet: they arrive at M2 (PLAN §16). This crate defines the trait, the
//! output collector, and the static registry so the engine, server, and codegen already share
//! one shape — adding a decoder later touches exactly one module here plus a `wire` settings
//! struct.

use num_complex::Complex;
use sdrmm_wire::{ChannelDescriptor, ChannelSettings};

/// Errors raised while constructing or configuring a channel.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("unknown channel type: {0}")]
    UnknownType(String),
    #[error("invalid settings: {0}")]
    InvalidSettings(String),
}

/// Construction context passed to a channel: the IQ rate it receives after the DDC, and its
/// offset from the device center. Grows as the plugin API matures (PLAN §8).
#[derive(Clone, Copy, Debug)]
pub struct ChannelCtx {
    /// Sample rate of the decimated IQ stream the channel will `process`, in Hz.
    pub input_rate: f64,
}

/// Sink a channel writes into each `process` call: demodulated audio, typed events, and
/// low-rate IQ taps for the analyzer (PLAN §8). Buffers are reused across calls by the host.
#[derive(Default)]
pub struct ChannelOutputs {
    /// Interleaved/mono PCM plus its sample rate, when the channel produced audio this block.
    pub audio_pcm: Vec<f32>,
    pub audio_rate: u32,
    /// Serialized decoder events (JSON `ServerEvent`s), emitted to the WS hub.
    pub events: Vec<String>,
    /// Decimated IQ for scope/constellation panels.
    pub iq_tap: Vec<Complex<f32>>,
}

impl ChannelOutputs {
    /// Clear all buffers without freeing capacity, ready for the next block.
    pub fn reset(&mut self) {
        self.audio_pcm.clear();
        self.audio_rate = 0;
        self.events.clear();
        self.iq_tap.clear();
    }
}

/// A receive channel: consumes decimated IQ, produces audio/events/taps (PLAN §8).
pub trait ChannelRx: Send {
    /// Static description that drives the "add channel" UI. Object-safe callers use the
    /// registry; this associated fn is for the concrete type.
    fn descriptor() -> &'static ChannelDescriptor
    where
        Self: Sized;

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError>
    where
        Self: Sized;

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError>;

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs);
}

/// Descriptors for every compiled-in channel type (PLAN §8: static, feature-gated registry).
/// Empty at M0; each demod adds its entry here as it lands.
#[must_use]
pub fn descriptors() -> Vec<ChannelDescriptor> {
    Vec::new()
}
