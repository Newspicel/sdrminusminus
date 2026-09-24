use std::f64::consts::TAU;

use num_complex::Complex;

use super::demod::{RRC_BETA, SampleFir, rrc_taps};

const SHAPING_TAPS: usize = 161;
const SHAPING_MAX_RATE: f64 = 96_000.0;

pub fn modulate(
    symbols: &[u8],
    symbol_rate: f64,
    sample_rate: f64,
    freq_offset_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    let samples_per_symbol = sample_rate / symbol_rate;
    let mut baseband = Vec::with_capacity((symbols.len() as f64 * samples_per_symbol) as usize + 1);
    for (index, &symbol) in symbols.iter().enumerate() {
        let level = if symbol == 1 { 1.0f32 } else { -1.0 };
        let end = (((index + 1) as f64) * samples_per_symbol).round() as usize;
        baseband.resize(end.max(baseband.len()), Complex::new(level, 0.0));
    }
    let shaped = if sample_rate <= SHAPING_MAX_RATE {
        SampleFir::new(rrc_taps(RRC_BETA, samples_per_symbol, SHAPING_TAPS)).filter(&baseband)
    } else {
        baseband
    };
    shaped
        .into_iter()
        .enumerate()
        .map(|(n, sample)| {
            let phase = TAU * freq_offset_hz * n as f64 / sample_rate;
            Complex::new(phase.cos() as f32, phase.sin() as f32) * sample * amplitude
        })
        .collect()
}
