use num_complex::Complex;

use super::taps::{Fir, lowpass_taps};

const FREQ_ALPHA: f32 = 0.0004;
const TIMING_GAIN: f64 = 0.1;
const MAG_ALPHA: f32 = 0.01;
const LOWPASS_TAPS: usize = 101;
const LOWPASS_BIT_RATE_FRACTION: f64 = 0.6;

pub(super) struct MskDemod {
    samples_per_bit: f64,
    lowpass: Fir,
    filtered: Vec<Complex<f32>>,
    previous_sample: Complex<f32>,
    previous_discriminator: f32,
    freq_offset: f32,
    timing: f64,
    accumulator: f32,
    accumulated: u32,
    magnitude: f32,
}

impl MskDemod {
    pub(super) fn new(channel_rate: f64, bit_rate: f64) -> Self {
        Self {
            samples_per_bit: channel_rate / bit_rate,
            lowpass: Fir::new(
                lowpass_taps(
                    LOWPASS_BIT_RATE_FRACTION * bit_rate / channel_rate,
                    LOWPASS_TAPS,
                ),
                1,
            ),
            filtered: Vec::new(),
            previous_sample: Complex::new(0.0, 0.0),
            previous_discriminator: 0.0,
            freq_offset: 0.0,
            timing: 0.0,
            accumulator: 0.0,
            accumulated: 0,
            magnitude: 1e-3,
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<(f32, u8)>) {
        self.filtered.clear();
        self.lowpass.process(input, &mut self.filtered);
        for index in 0..self.filtered.len() {
            let sample = self.filtered[index];
            if let Some(bit) = self.step(sample) {
                out.push(bit);
            }
        }
    }

    fn step(&mut self, sample: Complex<f32>) -> Option<(f32, u8)> {
        let raw = (sample * self.previous_sample.conj()).arg();
        self.previous_sample = sample;
        self.freq_offset += FREQ_ALPHA * (raw - self.freq_offset);
        let discriminator = raw - self.freq_offset;
        if discriminator != 0.0
            && self.previous_discriminator != 0.0
            && (discriminator < 0.0) != (self.previous_discriminator < 0.0)
        {
            let error =
                self.timing - (self.timing / self.samples_per_bit).round() * self.samples_per_bit;
            self.timing -= TIMING_GAIN * error;
        }
        self.previous_discriminator = discriminator;
        self.accumulator += discriminator;
        self.accumulated += 1;
        self.timing += 1.0;
        if self.timing < self.samples_per_bit {
            return None;
        }
        self.timing -= self.samples_per_bit;
        let level = self.accumulator / self.accumulated.max(1) as f32;
        self.accumulator = 0.0;
        self.accumulated = 0;
        self.magnitude += MAG_ALPHA * (level.abs() - self.magnitude);
        let soft = (level / self.magnitude.max(1e-9)).clamp(-1.0, 1.0);
        Some((soft, u8::from(soft > 0.0)))
    }
}
