use std::{f64::consts::TAU, ops::RangeInclusive};

use num_complex::Complex;
use sdrmm_wire::DabTransmissionMode;

use super::mode::{Mode, TRANSMISSION_MODES};

pub const SEARCH: usize = 96;
pub const MIN_COHERENCE: f32 = 0.25;
const SEARCH_STRIDE: usize = 4;
const ALIGN_SYMBOLS: usize = 8;
const DETECT_MARGIN: f32 = 2.0;
const MAX_DRIFT: f64 = 300e-6;
const DRIFT_GAIN: f64 = 0.25;
const FREQUENCY_GAIN: f64 = 0.5;
const MAX_MISSES: u32 = 4;

#[must_use]
pub fn symbol_start(mode: Mode, at: f64, index: usize, drift: f64) -> f64 {
    at + (index * mode.symbol()) as f64 * (1.0 + drift)
}

fn prefix_terms(mode: Mode, symbol: &[Complex<f32>], skip: usize) -> (Complex<f32>, f32) {
    let mut correlation = Complex::new(0.0f32, 0.0);
    let mut energy = 0.0f32;
    for index in skip..mode.guard {
        let prefix = symbol[index];
        let tail = symbol[mode.useful + index];
        correlation += prefix * tail.conj();
        energy += prefix.norm_sqr() + tail.norm_sqr();
    }
    (correlation, energy)
}

fn prefix_sum(
    mode: Mode,
    samples: &[Complex<f32>],
    at: usize,
    symbols: usize,
    drift: f64,
    skip: usize,
) -> (Complex<f32>, f32) {
    let mut correlation = Complex::new(0.0f32, 0.0);
    let mut energy = 0.0f32;
    for index in 0..symbols {
        let start = symbol_start(mode, at as f64, index, drift).round() as usize;
        let Some(symbol) = samples.get(start..start + mode.symbol()) else {
            break;
        };
        let (part, power) = prefix_terms(mode, symbol, skip);
        correlation += part;
        energy += power;
    }
    (correlation, energy)
}

#[must_use]
pub fn coherence(mode: Mode, samples: &[Complex<f32>], at: usize, drift: f64) -> f32 {
    let (correlation, energy) = prefix_sum(mode, samples, at, ALIGN_SYMBOLS, drift, 0);
    2.0 * correlation.norm() / energy.max(1e-20)
}

#[must_use]
pub fn align(
    mode: Mode,
    samples: &[Complex<f32>],
    from: usize,
    span: usize,
    drift: f64,
) -> (f32, usize) {
    let mut best = (0.0f32, from);
    for at in (from..=from + span).step_by(SEARCH_STRIDE) {
        let value = coherence(mode, samples, at, drift);
        if value > best.0 {
            best = (value, at);
        }
    }
    for at in best.1.saturating_sub(SEARCH_STRIDE)..=best.1 + SEARCH_STRIDE {
        let value = coherence(mode, samples, at, drift);
        if value > best.0 {
            best = (value, at);
        }
    }
    best
}

#[must_use]
pub fn prefix_phase(mode: Mode, samples: &[Complex<f32>], at: usize, drift: f64) -> Complex<f32> {
    prefix_sum(mode, samples, at, mode.symbols, drift, mode.guard / 2).0
}

#[must_use]
pub fn detect_span() -> usize {
    (ALIGN_SYMBOLS + 1) * Mode::new(DabTransmissionMode::I).symbol()
}

#[must_use]
pub fn detect(samples: &[Complex<f32>], from: usize, span: usize) -> Option<DabTransmissionMode> {
    let mut ranked = TRANSMISSION_MODES.map(|candidate| {
        (
            align(Mode::new(candidate), samples, from, span, 0.0).0,
            candidate,
        )
    });
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let [best, second, ..] = ranked;
    (best.0 >= MIN_COHERENCE && best.0 >= DETECT_MARGIN * second.0).then_some(best.1)
}

#[must_use]
pub fn shifts(mode: Mode, frequency: f64, limit: f64) -> RangeInclusive<i32> {
    let spacing = 1.0 / mode.useful as f64;
    let low = ((-limit - frequency) / spacing).ceil() as i32;
    let high = ((limit - frequency) / spacing).floor() as i32;
    low.min(0)..=high.max(0)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Tracker {
    pub frequency: f64,
    pub drift: f64,
    next: Option<f64>,
    last: Option<f64>,
    elapsed: u32,
    misses: u32,
    measured: bool,
}

impl Tracker {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn forget(&mut self) {
        self.next = None;
        self.last = None;
        self.elapsed = 0;
        self.misses = 0;
    }

    #[must_use]
    pub const fn next(&self) -> Option<f64> {
        self.next
    }

    #[must_use]
    pub fn refined(&self, mode: Mode, phase: Complex<f32>) -> f64 {
        let spacing = 1.0 / mode.useful as f64;
        let measured = -f64::from(phase.arg()) / TAU * spacing;
        let residual = measured - self.frequency;
        let residual = residual - (residual / spacing).round() * spacing;
        let gain = if self.next.is_some() {
            FREQUENCY_GAIN
        } else {
            1.0
        };
        self.frequency + gain * residual
    }

    pub fn locked(&mut self, mode: Mode, frequency: f64, timing: f64, at: f64) {
        self.frequency = frequency;
        let frame = mode.frame() as f64;
        if let Some(last) = self.last {
            let measured = (timing - last) / (f64::from(self.elapsed) * frame) - 1.0;
            if measured.abs() <= MAX_DRIFT {
                let gain = if self.measured { DRIFT_GAIN } else { 1.0 };
                self.drift += gain * (measured - self.drift);
                self.measured = true;
            }
        }
        self.last = Some(timing);
        self.elapsed = 1;
        self.misses = 0;
        self.next = Some(at + frame * (1.0 + self.drift));
    }

    pub fn missed(&mut self, mode: Mode) {
        self.misses += 1;
        self.elapsed += 1;
        if self.misses > MAX_MISSES {
            self.forget();
            return;
        }
        if let Some(next) = &mut self.next {
            *next += mode.frame() as f64 * (1.0 + self.drift);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    fn turned(iq: &[Complex<f32>], cycles_per_sample: f64) -> Vec<Complex<f32>> {
        iq.iter()
            .enumerate()
            .map(|(index, &value)| {
                let phase = TAU * cycles_per_sample * index as f64;
                value * Complex::new(phase.cos() as f32, phase.sin() as f32)
            })
            .collect()
    }

    #[test]
    fn the_cyclic_prefix_finds_the_reference_symbol_and_its_fractional_offset() {
        for transmission in TRANSMISSION_MODES {
            let mode = Mode::new(transmission);
            let shift = 0.3 / mode.useful as f64;
            let iq = turned(&synth::dab::ensemble_for_mode(transmission, 2), shift);
            let (value, at) = align(mode, &iq, mode.null - SEARCH, 2 * SEARCH, 0.0);
            assert!(value > 0.9, "{transmission:?}: coherence {value}");
            assert_eq!(at, mode.null, "{transmission:?}");
            let estimate = Tracker::default().refined(mode, prefix_phase(mode, &iq, at, 0.0));
            assert!(
                (estimate - shift).abs() * mode.useful as f64 <= 1e-3,
                "{transmission:?}: {estimate} for {shift}"
            );
        }
    }

    #[test]
    fn the_cyclic_prefix_length_names_the_transmission_mode() {
        for transmission in TRANSMISSION_MODES {
            let mode = Mode::new(transmission);
            let iq =
                crate::testutil::at_snr(&synth::dab::ensemble_for_mode(transmission, 4), 6.0, 3);
            assert_eq!(
                detect(&iq, mode.null - SEARCH, 2 * SEARCH),
                Some(transmission)
            );
        }
        let noise = crate::testutil::complex_noise(9, 1.0, 4 * detect_span());
        assert_eq!(detect(&noise, 0, 2 * SEARCH), None);
    }

    #[test]
    fn shifts_stay_inside_the_frequency_limit() {
        let mode = Mode::new(DabTransmissionMode::Ii);
        let limit = 40_000.0 / 2_048_000.0;
        assert_eq!(shifts(mode, 0.0, limit), -10..=10);
        assert_eq!(shifts(mode, 10.0 / 512.0, limit), -20..=0);
    }

    #[test]
    fn the_tracker_learns_sample_clock_drift_and_predicts_the_next_frame() {
        let mode = Mode::new(DabTransmissionMode::Iii);
        let frame = mode.frame() as f64;
        let drift = 80e-6;
        let mut tracker = Tracker::default();
        for index in 0..6 {
            let at = 1_000.0 + index as f64 * frame * (1.0 + drift);
            tracker.locked(mode, 0.0, at + 9.0, at);
        }
        assert!((tracker.drift - drift).abs() < 1e-7, "{}", tracker.drift);
        let predicted = tracker.next().expect("tracking");
        assert!((predicted - (1_000.0 + 6.0 * frame * (1.0 + drift))).abs() < 0.01);
        for _ in 0..=MAX_MISSES {
            tracker.missed(mode);
        }
        assert_eq!(tracker.next(), None);
    }
}
