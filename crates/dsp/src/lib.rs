pub mod agc;
pub mod bits;
pub mod caf;
pub mod cfar;
pub mod combine;
pub mod compander;
pub mod covariance;
pub mod ddc;
pub mod decim;
pub mod eca;
pub mod fec;
pub mod fft;
pub mod fir;
pub mod firc;
pub mod fm;
pub mod iir;
pub mod level;
pub mod linalg;
pub mod music;
pub mod nco;
pub mod noise;
pub mod pll;
pub mod resamp;
pub mod spectrum;
pub mod squelch;
pub mod steering;
pub mod sync;
pub mod tone;
pub mod window;
pub mod xcorr;

#[cfg(test)]
mod testutil;

pub use agc::Agc;
pub use bits::{
    Descrambler, DifferentialDecoder, HdlcDeframer, NrziDecoder, Scrambler, SyncDetector, bits_be,
    hamming_distance, manchester_decode, pack_lsb, pack_msb, reverse_byte,
};
pub use compander::Compander;
pub use ddc::{Ddc, DdcError, flat_bandwidth_hz, resamplable_bandwidth_hz};
pub use decim::{Decimator, RealDecimator};
pub use fec::{
    RdsOffset,
    block::{CyclicCode, ParityCode},
    bptc::{Bptc128, Bptc196},
    conv::{CONFIDENT, ERASURE, Soft, Viterbi5, soft},
    conv7::{ConvCode, Depuncturer, StreamViterbiK7, ViterbiK7, depuncture, puncture},
    crc4_msb, crc8_msb, crc16_ccitt, crc16_msb, crc16_msb_bits, crc16_x25, crc32_mpeg,
    ermes_bch_decode, ermes_bch_encode, golay23_correct, golay23_encode, golay23_ok, hdlc_fcs_ok,
    lfsr_digest8, lfsr_digest8_reflect, mode_s_append_overlaid_parity, mode_s_append_parity,
    mode_s_fix_single_bit, mode_s_overlay, mode_s_syndrome, pocsag_bch_decode, pocsag_bch_encode,
    prbs::{DAB_DISPERSAL, DVB_DISPERSAL, Prbs, PrbsSpec},
    rds_check_block, rds_correct_block, rds_encode_block, rds_syndrome, rs64_decode, rs64_encode,
    rs129_parity,
    rs256::{DVB_PRIMITIVE, ReedSolomon},
};
pub use fir::{
    design_bandpass, design_gaussian, design_lowpass, design_rds_biphase, design_rds_shaping,
    design_rrc,
};
pub use firc::FirC;
pub use fm::FmDemod;
pub use iir::{
    Biquad, ComplexOnePole, DcBlocker, Deemphasis, Highpass, IqDcBlocker, one_pole_coeff,
};
pub use level::{LEVEL_FLOOR_DB, LevelMeter};
pub use nco::Nco;
pub use noise::{AutoNotch, ClickRemover, NoiseBlanker, SpectralDenoiser};
pub use pll::{Costas, LoopFilter, Pll};
pub use resamp::FracResampler;
pub use spectrum::{SpectrumAnalyzer, adaptive_db_window, decimate_max, quantize_db};
pub use squelch::Squelch;
pub use sync::{BitSync, SymbolSync, farrow};
pub use tone::{Envelope, Goertzel, KeyingSlicer, KeyingTiming, ToneCorrelator};
pub use window::{coherent_gain, hann};
