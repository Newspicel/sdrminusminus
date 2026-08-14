use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::window::{coherent_gain, hann};

/// Reusable analyzer for a fixed FFT size.
pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    size: usize,
    window: Vec<f32>,
    inv_gain: f32,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl SpectrumAnalyzer {
    /// Build an analyzer for `size`-sample blocks (`size` should be a power of two for speed).
    #[must_use]
    pub fn new(size: usize) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(size.max(1));
        let window = hann(size);
        let inv_gain = 1.0 / coherent_gain(&window).max(f32::MIN_POSITIVE);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        Self {
            fft,
            size,
            window,
            inv_gain,
            buf: vec![Complex::new(0.0, 0.0); size],
            scratch,
        }
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Compute DC-centered power spectrum in dBFS. `input` and `out` must both be `size` long;
    /// a full-scale complex tone at a bin center reads ~0 dBFS. Allocation-free.
    pub fn power_db(&mut self, input: &[Complex<f32>], out: &mut [f32]) {
        assert_eq!(input.len(), self.size, "input length must equal FFT size");
        assert_eq!(out.len(), self.size, "output length must equal FFT size");

        for ((dst, &s), &w) in self.buf.iter_mut().zip(input).zip(&self.window) {
            *dst = s * w;
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);

        let half = self.size / 2;
        for (raw_idx, x) in self.buf.iter().enumerate() {
            let shifted = (raw_idx + half) % self.size;
            let mag = x.norm() * self.inv_gain;
            out[shifted] = 20.0 * (mag + 1e-12).log10();
        }
    }
}

pub fn decimate_max(db: &[f32], out: &mut [f32]) {
    let bins = out.len();
    assert!(bins > 0, "need at least one output bin");
    if db.is_empty() {
        out.fill(f32::NEG_INFINITY);
        return;
    }
    let len = db.len();
    if bins >= len {
        // Upsampling (or 1:1): nearest-neighbor keeps the frequency axis aligned. Never
        // reachable today (FFT_SIZE == MAX_BINS), but correct if that ever changes.
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = db[(i * len / bins).min(len - 1)];
        }
        return;
    }
    for (i, slot) in out.iter_mut().enumerate() {
        let start = i * len / bins;
        let end = ((i + 1) * len / bins).max(start + 1).min(len);
        let mut peak = f32::NEG_INFINITY;
        for &v in &db[start..end] {
            peak = peak.max(v);
        }
        *slot = peak;
    }
}

/// Quantize dB values to `u8` over `[db_min, db_max]` (: the window travels in the
/// frame header so the client can map bytes back to dB). Values clamp to the range.
pub fn quantize_db(db: &[f32], db_min: f32, db_max: f32, out: &mut [u8]) {
    assert_eq!(db.len(), out.len(), "quantize length mismatch");
    let span = (db_max - db_min).max(f32::MIN_POSITIVE);
    for (&v, slot) in db.iter().zip(out.iter_mut()) {
        let t = ((v - db_min) / span).clamp(0.0, 1.0);
        *slot = (t * 255.0 + 0.5) as u8;
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    /// A full-scale complex tone at a known offset must peak at the expected fft-shifted bin,
    /// near 0 dBFS, with a low floor elsewhere.
    #[test]
    fn tone_lands_in_expected_bin_near_0dbfs() {
        let size = 1024;
        let mut an = SpectrumAnalyzer::new(size);

        let bin = size / 8;
        let input: Vec<Complex<f32>> = (0..size)
            .map(|n| Complex::from_polar(1.0, 2.0 * PI * bin as f32 * n as f32 / size as f32))
            .collect();

        let mut db = vec![0.0f32; size];
        an.power_db(&input, &mut db);

        let expected = size / 2 + bin;
        let (peak_idx, &peak_val) = db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(peak_idx, expected, "peak bin");
        assert!(peak_val > -1.0, "peak near 0 dBFS, got {peak_val}");

        assert!(
            db[expected / 2] < -60.0,
            "floor too high: {}",
            db[expected / 2]
        );
    }

    #[test]
    fn decimate_max_preserves_peaks() {
        let mut db = vec![-100.0f32; 100];
        db[37] = -3.0; // a narrow spike
        let mut out = vec![0.0f32; 10];
        decimate_max(&db, &mut out);
        // The spike falls in the 4th output bin (37/10) and must survive.
        assert_eq!(out[3], -3.0);
    }

    #[test]
    fn quantize_maps_range_to_bytes() {
        let db = [-120.0, -70.0, -20.0];
        let mut out = [0u8; 3];
        quantize_db(&db, -120.0, -20.0, &mut out);
        assert_eq!(out[0], 0);
        assert_eq!(out[2], 255);
        assert!((out[1] as i32 - 128).abs() <= 1);
    }
}
