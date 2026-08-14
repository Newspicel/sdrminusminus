//! Window functions for spectral analysis ( dsp inventory).

use std::f32::consts::PI;

/// Periodic Hann window of length `n` (denominator `n`, correct for FFT analysis where the
/// window tiles seamlessly — as opposed to the symmetric `n-1` form used for filter design).
#[must_use]
pub fn hann(n: usize) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n as f32).cos()))
        .collect()
}

/// Coherent gain of a window: `sum(w)`. A full-scale tone at a bin center produces a
/// magnitude of `sum(w)` before normalization, so dividing by it yields ~0 dBFS.
#[must_use]
pub fn coherent_gain(window: &[f32]) -> f32 {
    window.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_endpoints_are_zero_and_center_is_one() {
        let w = hann(8);
        assert!(w[0].abs() < 1e-6);
        // Periodic Hann peaks at the midpoint n/2.
        assert!((w[4] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn hann_coherent_gain_is_half_n() {
        let w = hann(1024);
        // sum(periodic Hann) == n/2 exactly.
        assert!((coherent_gain(&w) - 512.0).abs() < 1e-2);
    }
}
