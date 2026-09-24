use std::f32::consts::{FRAC_PI_2, TAU};

use num_complex::Complex;

use super::taps::{Fir, lowpass_taps};

const FREQ_ALPHA: f32 = 0.0004;
const TIMING_GAIN: f64 = 0.1;
const CARRIER_GAIN: f32 = 0.05;
const MAG_ALPHA: f32 = 0.01;
const LOWPASS_TAPS: usize = 101;

pub(super) struct CoherentMskDemod {
    samples_per_bit: f64,
    lowpass: Fir,
    filtered: Vec<Complex<f32>>,
    bit_samples: Vec<Complex<f32>>,
    theta: f32,
    freq_offset: f32,
    previous_sample: Complex<f32>,
    previous_discriminator: f32,
    timing: f64,
    magnitude: f32,
    have_previous: bool,
}

impl CoherentMskDemod {
    pub(super) fn new(channel_rate: f64, bit_rate: f64) -> Self {
        let samples_per_bit = channel_rate / bit_rate;
        Self {
            samples_per_bit,
            lowpass: Fir::new(lowpass_taps(0.6 * bit_rate / channel_rate, LOWPASS_TAPS), 1),
            filtered: Vec::new(),
            bit_samples: Vec::with_capacity(samples_per_bit.ceil() as usize + 2),
            theta: 0.0,
            freq_offset: 0.0,
            previous_sample: Complex::new(0.0, 0.0),
            previous_discriminator: 0.0,
            timing: 0.0,
            magnitude: 1e-3,
            have_previous: false,
        }
    }

    fn decide_bit(&mut self) -> (f32, u8) {
        let count = self.bit_samples.len().max(1) as f32;
        let mut plus = Complex::new(0.0f32, 0.0);
        let mut minus = Complex::new(0.0f32, 0.0);
        for (index, &sample) in self.bit_samples.iter().enumerate() {
            let fraction = (index as f32 + 0.5) / count;
            plus += sample * Complex::from_polar(1.0, self.theta + FRAC_PI_2 * fraction).conj();
            minus += sample * Complex::from_polar(1.0, self.theta - FRAC_PI_2 * fraction).conj();
        }
        let bit = u8::from(plus.re > minus.re);
        let (deviation, matched) = if bit == 1 {
            (FRAC_PI_2, plus)
        } else {
            (-FRAC_PI_2, minus)
        };
        self.theta += deviation + CARRIER_GAIN * matched.arg();
        if self.theta > TAU {
            self.theta -= TAU;
        } else if self.theta < -TAU {
            self.theta += TAU;
        }
        let margin = plus.re - minus.re;
        self.magnitude += MAG_ALPHA * (margin.abs() - self.magnitude);
        self.bit_samples.clear();
        ((margin / self.magnitude.max(1e-9)).clamp(-1.0, 1.0), bit)
    }

    fn track_timing(&mut self, sample: Complex<f32>) {
        if self.have_previous {
            let raw = (sample * self.previous_sample.conj()).arg();
            self.freq_offset += FREQ_ALPHA * (raw - self.freq_offset);
            let discriminator = raw - self.freq_offset;
            if discriminator != 0.0
                && self.previous_discriminator != 0.0
                && (discriminator < 0.0) != (self.previous_discriminator < 0.0)
            {
                let error = self.timing
                    - (self.timing / self.samples_per_bit).round() * self.samples_per_bit;
                self.timing -= TIMING_GAIN * error;
            }
            self.previous_discriminator = discriminator;
        }
        self.previous_sample = sample;
        self.have_previous = true;
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<(f32, u8)>) {
        self.filtered.clear();
        self.lowpass.process(input, &mut self.filtered);
        for index in 0..self.filtered.len() {
            let sample = self.filtered[index];
            self.bit_samples.push(sample);
            self.track_timing(sample);
            self.timing += 1.0;
            if self.timing >= self.samples_per_bit {
                self.timing -= self.samples_per_bit;
                out.push(self.decide_bit());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inmarsat_aero::{decoder::CHANNEL_RATE, demod::MskDemod, modulate::modulate};

    const BIT_RATE: f64 = 1200.0;

    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn gauss(&mut self) -> f32 {
            let u1 = ((self.next_u64() >> 11) as f32 / (1u64 << 53) as f32).max(1e-12);
            let u2 = (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32;
            (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
        }
    }

    fn align_ber(sent: &[u8], received: &[(f32, u8)]) -> f64 {
        let hard: Vec<u8> = received.iter().map(|&(_, bit)| bit).collect();
        let mut best = 1.0f64;
        for lag in 0..40usize.min(hard.len()) {
            let n = sent.len().min(hard.len().saturating_sub(lag));
            if n < 2000 {
                break;
            }
            for polarity in [0u8, 1] {
                let errors = (0..n)
                    .filter(|&k| (hard[lag + k] ^ polarity) != sent[k])
                    .count();
                best = best.min(errors as f64 / n as f64);
            }
        }
        best
    }

    fn measure_ber(ebn0_db: f64, coherent: bool, seed: u64) -> f64 {
        let mut rng = Rng(seed);
        let bits: Vec<u8> = (0..20_000).map(|_| (rng.next_u64() & 1) as u8).collect();
        let signal = modulate(&bits, BIT_RATE, CHANNEL_RATE, 0.0, 1.0);
        let sigma = ((CHANNEL_RATE / BIT_RATE / 10f64.powf(ebn0_db / 10.0)) / 2.0).sqrt() as f32;
        let noisy: Vec<Complex<f32>> = signal
            .iter()
            .map(|&sample| sample + Complex::new(rng.gauss() * sigma, rng.gauss() * sigma))
            .collect();
        let mut out = Vec::new();
        if coherent {
            CoherentMskDemod::new(CHANNEL_RATE, BIT_RATE).process(&noisy, &mut out);
        } else {
            MskDemod::new(CHANNEL_RATE, BIT_RATE).process(&noisy, &mut out);
        }
        align_ber(&bits, &out)
    }

    fn average_ber(ebn0_db: f64, coherent: bool) -> f64 {
        (0..4u64)
            .map(|trial| measure_ber(ebn0_db, coherent, 0x1234 + trial * 99 + 1))
            .sum::<f64>()
            / 4.0
    }

    #[test]
    fn coherent_beats_discriminator_ber_vs_snr() {
        let mut strictly_better = 0;
        for ebn0 in [4.0, 6.0, 8.0] {
            let discriminator = average_ber(ebn0, false);
            let coherent = average_ber(ebn0, true);
            assert!(coherent <= discriminator * 1.05 + 1e-4, "{ebn0} dB");
            if coherent < discriminator * 0.8 {
                strictly_better += 1;
            }
        }
        assert!(strictly_better >= 2);
        assert!(average_ber(7.0, true) < average_ber(8.0, false));
    }
}
