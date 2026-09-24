use std::f64::consts::TAU;

use num_complex::Complex;

pub(super) fn modulate(
    bits: &[u8],
    bit_rate: f64,
    sample_rate: f64,
    freq_offset_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    let samples_per_bit = sample_rate / bit_rate;
    let deviation = bit_rate / 4.0;
    let mut out = Vec::with_capacity((bits.len() as f64 * samples_per_bit) as usize + 1);
    let mut phase = 0.0f64;
    for (index, &bit) in bits.iter().enumerate() {
        let level = if bit == 1 { 1.0 } else { -1.0 };
        let frequency = freq_offset_hz + level * deviation;
        let end = (((index + 1) as f64) * samples_per_bit).round() as usize;
        while out.len() < end {
            phase += TAU * frequency / sample_rate;
            out.push(Complex::new(phase.cos() as f32, phase.sin() as f32) * amplitude);
        }
    }
    out
}
