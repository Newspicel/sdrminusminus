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

pub trait Impairment {
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng);
}

pub(crate) fn mean_power(x: &[Complex<f32>]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    signal_energy(x) / x.len() as f64
}

pub(crate) fn signal_energy(x: &[Complex<f32>]) -> f64 {
    x.iter()
        .map(|s| {
            let re = f64::from(s.re);
            let im = f64::from(s.im);
            re * re + im * im
        })
        .sum()
}

pub(crate) fn rms(x: &[Complex<f32>]) -> f64 {
    mean_power(x).sqrt()
}
