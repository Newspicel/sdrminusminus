use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftDirection, FftNum, FftPlanner, num_traits::Zero};

#[derive(Clone)]
pub struct Transform<T: FftNum = f32> {
    plan: Arc<dyn Fft<T>>,
    scratch: Vec<Complex<T>>,
}

impl<T: FftNum> Transform<T> {
    #[must_use]
    pub fn forward(len: usize) -> Self {
        Self::planned(len, FftDirection::Forward)
    }

    #[must_use]
    pub fn inverse(len: usize) -> Self {
        Self::planned(len, FftDirection::Inverse)
    }

    fn planned(len: usize, direction: FftDirection) -> Self {
        let plan = FftPlanner::new().plan_fft(len, direction);
        let scratch = vec![Complex::zero(); plan.get_inplace_scratch_len()];
        Self { plan, scratch }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.plan.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plan.len() == 0
    }

    pub fn process(&mut self, buf: &mut [Complex<T>]) {
        self.plan.process_with_scratch(buf, &mut self.scratch);
    }
}

impl Transform<f32> {
    pub fn process_unitary(&mut self, buf: &mut [Complex<f32>]) {
        self.process(buf);
        scale(buf, (self.len() as f32).sqrt().recip());
    }
}

/// A planned transform and its inverse, with the scratch both need already sized.
///
/// Planning is the expensive part and reuse is the whole point: a processor builds one of these
/// when its size is settled and transforms in place from then on without touching the allocator.
#[derive(Clone)]
pub struct FftPair<T: FftNum = f32> {
    forward: Transform<T>,
    inverse: Transform<T>,
}

impl<T: FftNum> FftPair<T> {
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            forward: Transform::forward(len),
            inverse: Transform::inverse(len),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    pub fn forward(&mut self, buf: &mut [Complex<T>]) {
        self.forward.process(buf);
    }

    pub fn inverse(&mut self, buf: &mut [Complex<T>]) {
        self.inverse.process(buf);
    }
}

impl FftPair<f32> {
    pub fn inverse_scaled(&mut self, buf: &mut [Complex<f32>]) {
        self.inverse(buf);
        scale(buf, 1.0 / self.len() as f32);
    }

    pub fn forward_unitary(&mut self, buf: &mut [Complex<f32>]) {
        self.forward.process_unitary(buf);
    }

    pub fn inverse_unitary(&mut self, buf: &mut [Complex<f32>]) {
        self.inverse.process_unitary(buf);
    }
}

fn scale(buf: &mut [Complex<f32>], factor: f32) {
    for value in buf.iter_mut() {
        *value *= factor;
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    #[test]
    fn a_round_trip_returns_the_input() {
        let mut fft = FftPair::new(64);
        let original: Vec<Complex<f32>> = (0..64)
            .map(|k| Complex::from_polar(1.0, TAU * 3.0 * k as f32 / 64.0))
            .collect();
        let mut buf = original.clone();
        fft.forward(&mut buf);
        fft.inverse_scaled(&mut buf);
        for (index, (a, b)) in original.iter().zip(&buf).enumerate() {
            assert!((a - b).norm() < 1e-4, "sample {index}: {a} vs {b}");
        }
    }

    #[test]
    fn the_unitary_pair_keeps_energy_at_any_size() {
        for n in [8usize, 48, 80, 100] {
            let mut fft = FftPair::new(n);
            let original: Vec<Complex<f32>> = (0..n)
                .map(|k| Complex::new((k as f32).sin(), (0.7 * k as f32).cos()))
                .collect();
            let energy =
                |x: &[Complex<f32>]| x.iter().map(|v| f64::from(v.norm_sqr())).sum::<f64>();
            let mut buf = original.clone();
            fft.forward_unitary(&mut buf);
            assert!(
                (energy(&buf) / energy(&original) - 1.0).abs() < 1e-4,
                "n = {n}: energy moved"
            );
            fft.inverse_unitary(&mut buf);
            for (k, (a, b)) in buf.iter().zip(&original).enumerate() {
                assert!((a - b).norm() < 1e-4, "n = {n}, sample {k}");
            }
        }
    }

    #[test]
    fn a_tone_lands_in_its_own_bin() {
        let mut fft = FftPair::new(128);
        let mut buf: Vec<Complex<f32>> = (0..128)
            .map(|k| Complex::from_polar(1.0, TAU * 9.0 * k as f32 / 128.0))
            .collect();
        fft.forward(&mut buf);
        let peak = buf
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
            .map(|(bin, _)| bin);
        assert_eq!(peak, Some(9));
        assert!((buf[9].norm() - 128.0).abs() < 1e-2);
    }
}
