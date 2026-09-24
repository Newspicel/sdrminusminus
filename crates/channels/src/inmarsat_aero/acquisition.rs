use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

const BLOCKS: u32 = 2;

pub(super) struct CoarseAcquisition {
    size: usize,
    resolution: f64,
    tone_bins: i64,
    range_bins: i64,
    min_shift_hz: Option<f64>,
    buffer: Vec<Complex<f32>>,
    squared: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    fft: Arc<dyn Fft<f32>>,
    spectrum: Vec<f32>,
    blocks: u32,
}

impl CoarseAcquisition {
    pub(super) fn new(
        size: usize,
        fs: f64,
        line_offset_hz: f64,
        range_hz: f64,
        min_shift_hz: Option<f64>,
    ) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(size);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        let resolution = fs / size as f64;
        Self {
            size,
            resolution,
            tone_bins: (line_offset_hz / resolution).round() as i64,
            range_bins: (range_hz / resolution) as i64,
            min_shift_hz,
            buffer: Vec::with_capacity(size),
            squared: vec![Complex::new(0.0, 0.0); size],
            scratch,
            fft,
            spectrum: vec![0.0; size],
            blocks: 0,
        }
    }

    pub(super) fn push(&mut self, sample: Complex<f32>, locked: bool) -> Option<f64> {
        self.buffer.push(sample);
        if self.buffer.len() < self.size {
            return None;
        }
        let estimate = if locked {
            self.blocks = 0;
            None
        } else {
            self.estimate()
        };
        self.buffer.clear();
        estimate
    }

    fn estimate(&mut self) -> Option<f64> {
        for (squared, value) in self.squared.iter_mut().zip(&self.buffer) {
            *squared = value * value;
        }
        self.fft
            .process_with_scratch(&mut self.squared, &mut self.scratch);
        for (bin, value) in self.spectrum.iter_mut().zip(&self.squared) {
            *bin = 0.5 * *bin + 0.5 * value.norm_sqr();
        }
        self.blocks += 1;
        if self.blocks < BLOCKS {
            return None;
        }
        let size = self.size as i64;
        let bin = |k: i64| self.spectrum[k.rem_euclid(size) as usize];
        let mut best = (f32::MIN, 0i64);
        for k in -self.range_bins..=self.range_bins {
            let score = (-1..=1).fold(0.0f32, |score, j| {
                score + bin(k - self.tone_bins + j) + bin(k + self.tone_bins + j)
            });
            if score > best.0 {
                best = (score, k);
            }
        }
        let shift = best.1 as f64 * self.resolution / 2.0;
        if self
            .min_shift_hz
            .is_some_and(|minimum| shift.abs() <= minimum)
        {
            return None;
        }
        self.spectrum.fill(0.0);
        self.blocks = 0;
        Some(shift)
    }
}
