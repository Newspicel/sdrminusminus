use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

/// A planned transform and its inverse, with the scratch both need already sized.
///
/// Planning is the expensive part and reuse is the whole point: a processor builds one of these
/// when its size is settled and transforms in place from then on without touching the allocator.
pub struct FftPair {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    len: usize,
}

impl FftPair {
    #[must_use]
    pub fn new(len: usize) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(len);
        let inverse = planner.plan_fft_inverse(len);
        let scratch = vec![
            Complex::default();
            forward
                .get_inplace_scratch_len()
                .max(inverse.get_inplace_scratch_len())
        ];
        Self {
            forward,
            inverse,
            scratch,
            len,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn forward(&mut self, buf: &mut [Complex<f32>]) {
        self.forward.process_with_scratch(buf, &mut self.scratch);
    }

    pub fn inverse(&mut self, buf: &mut [Complex<f32>]) {
        self.inverse.process_with_scratch(buf, &mut self.scratch);
    }

    /// The inverse scaled so that a forward followed by an inverse is the identity.
    pub fn inverse_scaled(&mut self, buf: &mut [Complex<f32>]) {
        self.inverse(buf);
        let scale = 1.0 / self.len as f32;
        for value in buf.iter_mut() {
            *value *= scale;
        }
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
