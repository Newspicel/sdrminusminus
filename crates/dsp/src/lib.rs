//! `sdrmm-dsp` — pure DSP primitives (PLAN §7). No I/O, no async, no internal dependencies:
//! everything downstream trusts this crate, so every primitive carries golden/analytic tests.

pub mod agc;
pub mod ddc;
pub mod decim;
pub mod fir;
pub mod firc;
pub mod fm;
pub mod iir;
pub mod nco;
pub mod resamp;
pub mod spectrum;
pub mod squelch;
pub mod window;

#[cfg(test)]
mod testutil;

pub use agc::Agc;
pub use ddc::{Ddc, DdcError};
pub use decim::{Decimator, RealDecimator};
pub use fir::design_lowpass;
pub use firc::FirC;
pub use fm::FmDemod;
pub use iir::{DcBlocker, Deemphasis};
pub use nco::Nco;
pub use resamp::FracResampler;
pub use spectrum::{SpectrumAnalyzer, decimate_max, quantize_db};
pub use squelch::Squelch;
pub use window::{coherent_gain, hann};
