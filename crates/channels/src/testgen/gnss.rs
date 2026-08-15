use std::f32::consts::TAU;

use num_complex::Complex;

use crate::gnss::sampled_code;

pub const RATE: f64 = 2_048_000.0;

/// Ideal GPS L1 C/A baseband for acquisition fixtures. Navigation data is held at +1; the
/// acquisition event is intentionally independent of telemetry framing.
#[must_use]
pub fn acquisition(
    prn: u8,
    doppler_hz: f32,
    code_phase_samples: usize,
    milliseconds: usize,
) -> Vec<Complex<f32>> {
    let code = sampled_code(prn);
    let len = milliseconds * code.len();
    let mut out = Vec::with_capacity(len);
    for n in 0..len {
        let carrier = Complex::from_polar(1.0, TAU * doppler_hz * n as f32 / RATE as f32);
        out.push(carrier * code[(n + code_phase_samples) % code.len()]);
    }
    out
}
