use std::f64::consts::TAU;

use num_complex::Complex;
use rustfft::FftPlanner;

pub(crate) fn complex_tone(freq_norm: f64, len: usize) -> Vec<Complex<f32>> {
    (0..len)
        .map(|n| {
            let p = TAU * freq_norm * n as f64;
            Complex::new(p.cos() as f32, p.sin() as f32)
        })
        .collect()
}

pub(crate) fn real_tone(freq_norm: f64, len: usize) -> Vec<f32> {
    (0..len)
        .map(|n| (TAU * freq_norm * n as f64).sin() as f32)
        .collect()
}

pub(crate) fn rms_c(x: &[Complex<f32>]) -> f32 {
    (x.iter().map(|v| f64::from(v.norm_sqr())).sum::<f64>() / x.len() as f64).sqrt() as f32
}

pub(crate) fn rms_r(x: &[f32]) -> f32 {
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

pub(crate) fn tone_peak_and_snr(x: &[Complex<f32>]) -> (usize, f32) {
    let n = x.len();
    let mut buf = x.to_vec();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let power: Vec<f64> = buf.iter().map(|v| f64::from(v.norm_sqr())).collect();
    let peak = power
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap();
    let mut signal = 0.0;
    for d in -3i64..=3 {
        signal += power[(peak as i64 + d).rem_euclid(n as i64) as usize];
    }
    let total: f64 = power.iter().sum();
    let noise = (total - signal).max(1e-30);
    (peak, (10.0 * (signal / noise).log10()) as f32)
}

pub(crate) struct XorShift32(pub(crate) u32);

impl XorShift32 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.next_u32() as f64 / f64::from(u32::MAX) * 2.0 - 1.0) as f32
    }
}
