use std::{f64::consts::TAU, ops::RangeInclusive};

use num_complex::Complex;
use sdrmm_dsp::fft::Transform;
use sdrmm_wire::DabTransmissionMode;

use super::{mode::Mode, ofdm::reference_symbol_for_mode};

const MIN_CONFIDENCE: f32 = 0.2;
const MIN_PEAK_RATIO: f32 = 1.5;
const FIRST_PATH: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Probe {
    pub shift: i32,
    pub first_path: i32,
    pub mean_delay: f64,
}

pub struct PrsProbe {
    mode: Mode,
    fft: Transform,
    inverse: Transform,
    reference: Vec<Complex<f32>>,
    pairs: Vec<(i16, Complex<f32>)>,
    expected: Complex<f32>,
    spectrum: Vec<Complex<f32>>,
    response: Vec<Complex<f32>>,
}

impl PrsProbe {
    #[must_use]
    pub fn for_mode(transmission_mode: DabTransmissionMode) -> Self {
        let mode = Mode::new(transmission_mode);
        let reference = reference_symbol_for_mode(transmission_mode);
        let half = mode.carriers() as i16 / 2;
        let pairs = (-half..half)
            .filter(|&carrier| carrier != 0 && carrier != -1)
            .map(|carrier| {
                let step = reference[mode.carrier_bin(carrier + 1)]
                    * reference[mode.carrier_bin(carrier)].conj();
                (carrier, step.conj())
            })
            .collect();
        Self {
            mode,
            fft: Transform::forward(mode.useful),
            inverse: Transform::inverse(mode.useful),
            reference,
            pairs,
            expected: Complex::from_polar(
                1.0,
                std::f32::consts::TAU * mode.backoff() as f32 / mode.useful as f32,
            ),
            spectrum: vec![Complex::new(0.0, 0.0); mode.useful],
            response: vec![Complex::new(0.0, 0.0); mode.useful],
        }
    }

    pub fn probe(&mut self, window: &[Complex<f32>], shifts: RangeInclusive<i32>) -> Option<Probe> {
        if window.len() != self.mode.useful {
            return None;
        }
        self.spectrum.copy_from_slice(window);
        self.fft.process(&mut self.spectrum);
        let (shift, slope) = self.best_shift(shifts)?;
        let first_path = self.first_path(shift);
        (first_path.unsigned_abs() as usize <= self.mode.guard).then_some(Probe {
            shift,
            first_path,
            mean_delay: f64::from(slope.arg()) * self.mode.useful as f64 / TAU,
        })
    }

    fn bin(&self, carrier: i16, shift: i32) -> usize {
        (i32::from(carrier) + shift).rem_euclid(self.mode.useful as i32) as usize
    }

    fn differential(&self, shift: i32) -> (Complex<f32>, f32) {
        let mut sum = Complex::new(0.0f32, 0.0);
        let mut norm = 0.0f32;
        for &(carrier, reference) in &self.pairs {
            let low = self.spectrum[self.bin(carrier, shift)];
            let high = self.spectrum[self.bin(carrier + 1, shift)];
            let product = high * low.conj();
            sum += product * reference;
            norm += product.norm();
        }
        (sum, norm)
    }

    fn best_shift(&self, shifts: RangeInclusive<i32>) -> Option<(i32, Complex<f32>)> {
        let mut best = (0, Complex::new(0.0f32, 0.0), 0.0f32, 0.0f32);
        let mut second = 0.0f32;
        for shift in shifts {
            let (sum, norm) = self.differential(shift);
            let score = (sum * self.expected).re;
            if score > best.2 {
                second = best.2;
                best = (shift, sum, score, norm);
            } else if score > second {
                second = score;
            }
        }
        let confidence = best.2 / best.3.max(1e-20);
        (confidence >= MIN_CONFIDENCE && best.2 >= MIN_PEAK_RATIO * second)
            .then_some((best.0, best.1))
    }

    fn first_path(&mut self, shift: i32) -> i32 {
        self.response.fill(Complex::new(0.0, 0.0));
        let half = self.mode.carriers() as i16 / 2;
        for carrier in (-half..=half).filter(|&carrier| carrier != 0) {
            let bin = self.mode.carrier_bin(carrier);
            self.response[bin] =
                self.spectrum[self.bin(carrier, shift)] * self.reference[bin].conj();
        }
        self.inverse.process(&mut self.response);
        let (peak, strongest) =
            self.response
                .iter()
                .enumerate()
                .fold((0, 0.0f32), |best, (index, value)| {
                    if value.norm_sqr() > best.1 {
                        (index, value.norm_sqr())
                    } else {
                        best
                    }
                });
        let useful = self.mode.useful;
        let earliest = (0..=self.mode.guard)
            .rev()
            .map(|back| (peak + useful - back) % useful)
            .find(|&index| self.response[index].norm_sqr() >= FIRST_PATH * strongest)
            .unwrap_or(peak);
        if earliest >= useful / 2 {
            earliest as i32 - useful as i32
        } else {
            earliest as i32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dab::mode::TRANSMISSION_MODES, synth};

    fn window(
        iq: &[Complex<f32>],
        start: usize,
        useful: usize,
        cycles_per_sample: f64,
    ) -> Vec<Complex<f32>> {
        iq[start..start + useful]
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                let phase = TAU * cycles_per_sample * index as f64;
                value * Complex::new(phase.cos() as f32, phase.sin() as f32)
            })
            .collect()
    }

    #[test]
    fn the_reference_symbol_reveals_integer_offset_and_timing() {
        for transmission in TRANSMISSION_MODES {
            let mode = Mode::new(transmission);
            let iq = synth::dab::ensemble_for_mode(transmission, 1);
            let mut probe = PrsProbe::for_mode(transmission);
            for (bins, offset) in [(0, 0i32), (3, -5), (-4, -7)] {
                let start = (mode.null + mode.guard) as i32 + offset;
                let cycles = f64::from(bins) / mode.useful as f64;
                let found = probe
                    .probe(&window(&iq, start as usize, mode.useful, cycles), -6..=6)
                    .expect("a reference symbol");
                assert_eq!(found.shift, bins, "{transmission:?}");
                assert_eq!(found.first_path, -offset, "{transmission:?}");
                assert!(
                    (found.mean_delay - f64::from(offset)).abs() < 0.01,
                    "{transmission:?}: {}",
                    found.mean_delay
                );
            }
        }
    }

    #[test]
    fn a_large_offset_is_not_taken_for_its_mirrored_phase_table() {
        let transmission = DabTransmissionMode::I;
        let mode = Mode::new(transmission);
        let iq = synth::dab::ensemble_for_mode(transmission, 1);
        let mut probe = PrsProbe::for_mode(transmission);
        let start = mode.null + mode.guard - mode.backoff();
        for bins in [-31, 33, -40] {
            let cycles = f64::from(bins) / mode.useful as f64;
            let found = probe
                .probe(&window(&iq, start, mode.useful, cycles), -40..=40)
                .expect("a reference symbol");
            assert_eq!(found.shift, bins);
            assert_eq!(found.first_path, mode.backoff() as i32);
        }
    }

    #[test]
    fn the_first_path_leads_a_stronger_echo() {
        let transmission = DabTransmissionMode::Ii;
        let mode = Mode::new(transmission);
        let clean = synth::dab::ensemble_for_mode(transmission, 1);
        let mut echoed = clean.clone();
        for index in 40..echoed.len() {
            echoed[index] += clean[index - 40] * 1.5;
        }
        let mut probe = PrsProbe::for_mode(transmission);
        let start = mode.null + mode.guard;
        let found = probe
            .probe(&window(&echoed, start, mode.useful, 0.0), -2..=2)
            .expect("a reference symbol");
        assert_eq!(found.first_path, 0);
    }

    #[test]
    fn noise_is_not_a_reference_symbol() {
        let mut probe = PrsProbe::for_mode(DabTransmissionMode::Iii);
        let noise = crate::testutil::complex_noise(5, 1.0, 256);
        assert_eq!(probe.probe(&noise, -5..=5), None);
    }
}
