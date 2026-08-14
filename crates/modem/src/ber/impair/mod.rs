mod burst;
mod carrier;
mod channel;
mod frontend;
mod interference;
mod multipath;
mod noise;
mod sinc;
#[cfg(test)]
mod testutil;
mod timing;

pub use burst::BurstModel;
pub use carrier::{Cfo, Drift, PhaseNoise};
pub use channel::{Channel, ChannelSpec};
pub use frontend::{Clipping, DcOffset, IqImbalance, Quantiser};
pub use interference::Interferer;
pub use multipath::{Multipath, MultipathProfile};
pub use noise::{Awgn, sigma_for_channel_snr, sigma_for_ebn0};
use num_complex::Complex;
pub use timing::{ClockError, JitterKind, TimingJitter, TimingOffset};

use super::rng::Rng;

/// One impairment applied to a complex-baseband waveform. The trait exists so the
/// [`Channel`] composition and the sweep/limits runners can hold any axis behind one call;
/// concrete parameters live on the concrete types, never here.
pub trait Impairment {
    /// Applies the impairment in place. Deterministic impairments ignore `rng`; it is in the
    /// signature anyway so a composition does not need to know which of its stages draw.
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng);
}

/// Mean of `|x[n]|²` in `f64` — the power every relative level in this module is stated
/// against. Zero for an empty waveform rather than NaN, so a degenerate input degrades to
/// "nothing to calibrate against" instead of poisoning everything downstream.
pub(crate) fn mean_power(x: &[Complex<f32>]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    signal_energy(x) / x.len() as f64
}

/// `Σ |x[n]|²` in `f64` — the energy [`sigma_for_ebn0`] divides by the information-bit count.
/// Summing squared `f32` magnitudes in `f64` keeps the accumulation exact far past any
/// waveform length a sweep uses.
pub(crate) fn signal_energy(x: &[Complex<f32>]) -> f64 {
    x.iter()
        .map(|s| {
            let re = f64::from(s.re);
            let im = f64::from(s.im);
            re * re + im * im
        })
        .sum()
}

/// RMS magnitude, `√(mean |x|²)` — the reference for clipping thresholds, DC levels,
/// quantiser full scale and the burst noise floor.
pub(crate) fn rms(x: &[Complex<f32>]) -> f64 {
    mean_power(x).sqrt()
}
