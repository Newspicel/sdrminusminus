//! `sdrmm-dsp` — pure DSP primitives (PLAN §7). No I/O, no async, no internal dependencies:
//! everything downstream trusts this crate, so every primitive carries golden/analytic tests.

pub mod agc;
pub mod bits;
pub mod ddc;
pub mod decim;
pub mod fec;
pub mod fir;
pub mod firc;
pub mod fm;
pub mod iir;
pub mod nco;
pub mod pll;
pub mod resamp;
pub mod spectrum;
pub mod squelch;
pub mod sync;
pub mod tone;
pub mod window;

#[cfg(test)]
mod testutil;

pub use agc::Agc;
pub use bits::{
    Descrambler, DifferentialDecoder, HdlcDeframer, NrziDecoder, Scrambler, SyncDetector, bits_be,
    hamming_distance, manchester_decode, pack_lsb, pack_msb, reverse_byte,
};
pub use ddc::{Ddc, DdcError, flat_bandwidth_hz, resamplable_bandwidth_hz};
pub use decim::{Decimator, RealDecimator};
pub use fec::{
    RdsOffset, crc16_ccitt, crc16_x25, golay23_encode, golay23_ok, hdlc_fcs_ok,
    mode_s_append_overlaid_parity, mode_s_append_parity, mode_s_fix_single_bit, mode_s_overlay,
    mode_s_syndrome, pocsag_bch_decode, pocsag_bch_encode, rds_check_block, rds_encode_block,
    rds_syndrome,
};
pub use fir::{design_bandpass, design_gaussian, design_lowpass};
pub use firc::FirC;
pub use fm::FmDemod;
pub use iir::{ComplexOnePole, DcBlocker, Deemphasis, Highpass, one_pole_coeff};
pub use nco::Nco;
pub use pll::{Costas, LoopFilter, Pll};
pub use resamp::FracResampler;
pub use spectrum::{SpectrumAnalyzer, decimate_max, quantize_db};
pub use squelch::Squelch;
pub use sync::{BitSync, SymbolSync};
pub use tone::{Envelope, Goertzel, KeyingSlicer, KeyingTiming, ToneCorrelator};
pub use window::{coherent_gain, hann};
