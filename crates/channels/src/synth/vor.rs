use std::f64::consts::TAU;

use num_complex::Complex;

pub const RATE: f64 = 48_000.0;

#[must_use]
pub fn transmission(radial_deg: f64, seconds: usize) -> Vec<Complex<f32>> {
    (0..RATE as usize * seconds)
        .map(|index| {
            let time = index as f64 / RATE;
            let reference = TAU * 30.0 * time;
            let variable = reference - radial_deg.to_radians();
            let subcarrier = TAU * 9_960.0 * time + 16.0 * reference.sin();
            let envelope = 1.0 + 0.3 * variable.cos() + 0.3 * subcarrier.cos();
            Complex::new(envelope as f32, 0.0)
        })
        .collect()
}
