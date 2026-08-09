//! `sdrmm-dsp` — pure DSP primitives (PLAN §7). No I/O, no async, no internal dependencies:
//! everything downstream trusts this crate, so every primitive carries golden/analytic tests.

pub mod nco;
pub mod spectrum;
pub mod window;

pub use nco::Nco;
pub use spectrum::{SpectrumAnalyzer, decimate_max, quantize_db};
pub use window::{coherent_gain, hann};
