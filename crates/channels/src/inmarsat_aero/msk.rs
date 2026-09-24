use std::f32::consts::{FRAC_1_PI, FRAC_PI_2, PI, TAU};

use num_complex::Complex;

use super::{
    acquisition::CoarseAcquisition,
    taps::{Fir, lowpass_taps},
};

const MAG_ALPHA: f32 = 0.01;
const LOCK_ALPHA: f32 = 0.01;
const LOCK_THRESHOLD: f32 = 0.5;
const LOWPASS_TAPS: usize = 101;
const LOWPASS_BIT_RATE_FRACTION: f64 = 0.6;
const PHASE_GAIN: f32 = 0.08;
const FREQ_GAIN: f32 = 0.001;
const TIMING_GAIN: f32 = 0.1;
const SOFT_LIMIT: f32 = 1.0;
const ACQUISITION_FFT: usize = 4_096;
const ACQUISITION_RANGE_HZ: f64 = 800.0;

pub(super) struct CoherentMsk {
    samples_per_bit: f64,
    fs: f64,
    lowpass: Fir,
    acquisition: CoarseAcquisition,
    weights: Vec<f32>,
    early_late: usize,
    history: Vec<Complex<f32>>,
    head: usize,
    countdowns: [Option<usize>; 2],
    timing: f64,
    nco_phase: f32,
    nco_freq: f32,
    quarter_turn: Complex<f32>,
    points: [Complex<f32>; 3],
    magnitude: f32,
    lock: f32,
}

impl CoherentMsk {
    pub(super) fn new(channel_rate: f64, bit_rate: f64) -> Self {
        let samples_per_bit = channel_rate / bit_rate;
        let half = samples_per_bit.round() as usize;
        let early_late = (samples_per_bit / 8.0).round().max(1.0) as usize;
        let weights: Vec<f32> = (0..=2 * half)
            .map(|index| {
                let offset = index as f32 - half as f32;
                (FRAC_PI_2 * offset / samples_per_bit as f32).cos()
            })
            .collect();
        Self {
            samples_per_bit,
            fs: channel_rate,
            lowpass: Fir::new(
                lowpass_taps(
                    LOWPASS_BIT_RATE_FRACTION * bit_rate / channel_rate,
                    LOWPASS_TAPS,
                ),
                1,
            ),
            acquisition: CoarseAcquisition::new(
                ACQUISITION_FFT,
                channel_rate,
                bit_rate / 2.0,
                ACQUISITION_RANGE_HZ,
                None,
            ),
            history: vec![Complex::new(0.0, 0.0); weights.len() + 2 * early_late],
            weights,
            early_late,
            head: 0,
            countdowns: [None; 2],
            timing: 0.0,
            nco_phase: 0.0,
            nco_freq: 0.0,
            quarter_turn: Complex::new(1.0, 0.0),
            points: [Complex::new(0.0, 0.0); 3],
            magnitude: 1e-3,
            lock: 0.0,
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<(f32, u8)>) {
        for &sample in input {
            if let Some(bit) = self.step(sample) {
                out.push(bit);
            }
        }
    }

    fn step(&mut self, sample: Complex<f32>) -> Option<(f32, u8)> {
        if let Some(offset_hz) = self.acquisition.push(sample, self.lock > LOCK_THRESHOLD) {
            self.nco_freq = (f64::from(TAU) * offset_hz / self.fs) as f32;
        }
        let mixed = sample * Complex::from_polar(1.0, -self.nco_phase);
        self.nco_phase = wrap(self.nco_phase + self.nco_freq);
        let filtered = self.lowpass.filter(mixed);
        self.head = (self.head + 1) % self.history.len();
        self.history[self.head] = filtered;
        let mut output = None;
        for slot in 0..self.countdowns.len() {
            match self.countdowns[slot] {
                Some(0) => {
                    self.countdowns[slot] = None;
                    output = Some(self.detect());
                }
                Some(remaining) => self.countdowns[slot] = Some(remaining - 1),
                None => {}
            }
        }
        self.timing += 1.0;
        if self.timing >= self.samples_per_bit {
            self.timing -= self.samples_per_bit;
            if let Some(slot) = self.countdowns.iter().position(Option::is_none) {
                self.countdowns[slot] = Some(self.weights.len() / 2 + self.early_late - 1);
            }
        }
        output
    }

    fn correlate(&self, shift: usize) -> Complex<f32> {
        let length = self.history.len();
        let start = self.head + 1 + shift;
        self.weights
            .iter()
            .enumerate()
            .fold(Complex::new(0.0, 0.0), |sum, (index, &weight)| {
                sum + self.history[(start + index) % length] * weight
            })
    }

    fn detect(&mut self) -> (f32, u8) {
        let rotation = self.quarter_turn;
        self.quarter_turn *= Complex::new(0.0, -1.0);
        let early = (self.correlate(0) * rotation).re;
        let point = self.correlate(self.early_late) * rotation;
        let late = (self.correlate(2 * self.early_late) * rotation).re;
        self.points = [self.points[1], self.points[2], point];
        self.magnitude += MAG_ALPHA * (point.re.abs() - self.magnitude);
        let timing_error = point.re.signum() * (late - early) / self.magnitude.max(1e-9);
        self.timing -= f64::from(TIMING_GAIN * timing_error);
        self.track_carrier();
        self.soft_bit()
    }

    fn track_carrier(&mut self) {
        let [before, current, after] = self.points.map(|point| point.re.signum());
        let expected = Complex::new(current, (after - before) * FRAC_1_PI);
        let error = (self.points[1] * expected.conj())
            .arg()
            .clamp(-FRAC_PI_2, FRAC_PI_2);
        self.lock += LOCK_ALPHA * ((2.0 * error).cos() - self.lock);
        self.nco_phase = wrap(self.nco_phase + PHASE_GAIN * error);
        self.nco_freq += FREQ_GAIN * error / self.samples_per_bit as f32;
    }

    fn soft_bit(&self) -> (f32, u8) {
        let (first, second) = (self.points[1].re, self.points[2].re);
        let reliability = first.abs().min(second.abs()) / self.magnitude.max(1e-9);
        let soft = (reliability * first.signum() * second.signum()).clamp(-SOFT_LIMIT, SOFT_LIMIT);
        (soft, u8::from(soft > 0.0))
    }
}

fn wrap(phase: f32) -> f32 {
    if phase > PI {
        phase - TAU
    } else if phase < -PI {
        phase + TAU
    } else {
        phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inmarsat_aero::{
        decoder::CHANNEL_RATE, demod::MskDemod, modulate::modulate, tests::Gauss,
    };

    const SETTLE_BITS: usize = 3_000;

    fn bit_errors(sent: &[u8], received: &[(f32, u8)]) -> f64 {
        (0..8)
            .map(|lag| {
                let compared = sent.len().min(received.len().saturating_sub(lag));
                let errors = (SETTLE_BITS..compared)
                    .filter(|&index| received[index + lag].1 != sent[index])
                    .count();
                errors as f64 / (compared - SETTLE_BITS) as f64
            })
            .fold(1.0, f64::min)
    }

    fn rates(rate: f64, cfo: f64, es_n0_db: f64) -> (f64, f64) {
        let mut noise = Gauss(77);
        let bits: Vec<u8> = (0..9_000).map(|_| u8::from(noise.sample() > 0.0)).collect();
        let mut iq = modulate(&bits, rate, CHANNEL_RATE, cfo, 1.0);
        noise.add(&mut iq, 1.0, CHANNEL_RATE / rate, es_n0_db);
        let (mut coherent, mut discriminator) = (Vec::new(), Vec::new());
        CoherentMsk::new(CHANNEL_RATE, rate).process(&iq, &mut coherent);
        MskDemod::new(CHANNEL_RATE, rate).process(&iq, &mut discriminator);
        (
            bit_errors(&bits, &coherent),
            bit_errors(&bits, &discriminator),
        )
    }

    #[test]
    fn clean_signal_has_no_errors() {
        for (rate, cfo) in [(600.0, 0.0), (600.0, -250.0), (1200.0, 300.0)] {
            assert_eq!(rates(rate, cfo, 40.0).0, 0.0, "{rate} bit/s at {cfo} Hz");
        }
    }

    #[test]
    fn beats_the_discriminator_in_noise() {
        for (rate, cfo) in [(600.0, 120.0), (1200.0, -150.0)] {
            let (coherent, discriminator) = rates(rate, cfo, 4.0);
            assert!(coherent < 0.04, "{rate} bit/s: {coherent}");
            assert!(
                coherent * 2.0 < discriminator,
                "{rate} bit/s: {coherent} vs {discriminator}"
            );
        }
    }
}
