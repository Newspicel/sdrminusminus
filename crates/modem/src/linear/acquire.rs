use std::f64::consts::TAU;

use num_complex::Complex;
use rustfft::FftPlanner;

pub const MIN_SYMBOLS: usize = 32;

const ZERO_PAD: usize = 4;

const MIN_PEAK_OVER_NOISE: f64 = 3.0;

const SEGMENTS: usize = 8;

const MIN_SEGMENT: usize = 128;

const OUTLIER_BINS: f64 = 3.0;

const MIN_DRIFT_SIGMAS: f64 = 4.0;

const MIN_DRIFT_EXCURSION_CYCLES: f64 = 0.05;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Chirp {
    pub centre_cycles_per_symbol: f64,
    pub drift_cycles_per_symbol2: f64,
}

impl Chirp {
    pub fn remove(&self, symbols: &mut [Complex<f32>]) {
        let centre = (symbols.len() as f64 - 1.0) / 2.0;
        for (k, s) in symbols.iter_mut().enumerate() {
            let t = k as f64 - centre;
            let cycles =
                self.centre_cycles_per_symbol * t + 0.5 * self.drift_cycles_per_symbol2 * t * t;
            let theta = -TAU * cycles.rem_euclid(1.0);
            *s *= Complex::new(theta.cos() as f32, theta.sin() as f32);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Tone {
    cycles_per_symbol: f64,
    strength: f64,
}

#[derive(Clone, Copy, Debug)]
struct SegmentTone {
    at_symbol: f64,
    cycles_per_symbol: f64,
    weight: f64,
}

pub struct FrequencyAcquisition {
    planner: FftPlanner<f64>,
    stripped: Vec<Complex<f64>>,
    spectrum: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    segments: Vec<SegmentTone>,
}

impl Default for FrequencyAcquisition {
    fn default() -> Self {
        Self::new()
    }
}

impl FrequencyAcquisition {
    #[must_use]
    pub fn new() -> Self {
        Self {
            planner: FftPlanner::new(),
            stripped: Vec::new(),
            spectrum: Vec::new(),
            scratch: Vec::new(),
            segments: Vec::new(),
        }
    }

    pub fn estimate(&mut self, symbols: &[Complex<f32>], order: u32) -> Option<Chirp> {
        if symbols.len() < MIN_SYMBOLS || order == 0 {
            return None;
        }
        self.strip(symbols, order);
        let n = self.stripped.len();
        let longest = n / SEGMENTS;
        let coarse = std::iter::successors(Some(longest), |&len| Some(len / 2))
            .take_while(|&len| len >= MIN_SEGMENT)
            .find_map(|len| self.segment_line(len));
        let mut drift = coarse.map_or(0.0, |(_, slope)| slope);
        dechirp(&mut self.stripped, drift);
        let fine = self.segment_line(longest);
        if let Some((_, slope)) = fine {
            dechirp(&mut self.stripped, slope);
            drift += slope;
        }
        let line = fine.or(coarse);
        let centre = match (self.tone(0, n), line) {
            (Some(whole), _) => whole.cycles_per_symbol,
            (None, Some((intercept, _))) => intercept,
            (None, None) => return None,
        };
        let m = f64::from(order);
        let half = n as f64 / 2.0;
        let excursion = 0.5 * (drift / m).abs() * half * half;
        Some(Chirp {
            centre_cycles_per_symbol: wrap_half(centre) / m,
            drift_cycles_per_symbol2: if excursion >= MIN_DRIFT_EXCURSION_CYCLES {
                drift / m
            } else {
                0.0
            },
        })
    }

    fn strip(&mut self, symbols: &[Complex<f32>], order: u32) {
        self.stripped.clear();
        self.stripped.extend(symbols.iter().map(|&y| {
            let y = Complex::new(f64::from(y.re), f64::from(y.im));
            let r2 = y.norm_sqr();
            if r2 <= 0.0 {
                return Complex::new(0.0, 0.0);
            }
            let unit = y / r2.sqrt();
            unit.powu(order) * r2
        }));
    }

    fn segment_line(&mut self, len: usize) -> Option<(f64, f64)> {
        let n = self.stripped.len();
        if len < MIN_SEGMENT {
            return None;
        }
        let count = n / len;
        if count < 2 {
            return None;
        }
        let centre = (n as f64 - 1.0) / 2.0;
        self.segments.clear();
        for s in 0..count {
            let from = s * len;
            if let Some(tone) = self.tone(from, from + len) {
                self.segments.push(SegmentTone {
                    at_symbol: from as f64 + (len as f64 - 1.0) / 2.0 - centre,
                    cycles_per_symbol: tone.cycles_per_symbol,
                    weight: tone.strength,
                });
            }
        }
        if self.segments.len() * 2 < count {
            return None;
        }
        let reference = self
            .segments
            .iter()
            .max_by(|a, b| a.weight.total_cmp(&b.weight))?
            .cycles_per_symbol;
        for seg in &mut self.segments {
            seg.cycles_per_symbol = reference + wrap_half(seg.cycles_per_symbol - reference);
        }
        let first = fit_line(&self.segments)?;
        let bin = 1.0 / len as f64;
        self.segments.retain(|seg| {
            (seg.cycles_per_symbol - first.0 - first.1 * seg.at_symbol).abs() <= OUTLIER_BINS * bin
        });
        fit_line(&self.segments)
    }

    pub fn peak_power(&mut self, symbols: &[Complex<f32>], order: u32) -> f64 {
        if symbols.is_empty() || order == 0 {
            return 0.0;
        }
        self.strip(symbols, order);
        self.spectrum_peak(0, symbols.len())
            .map_or(0.0, |(_, power, _)| power)
    }

    fn spectrum_peak(&mut self, from: usize, to: usize) -> Option<(usize, f64, usize)> {
        let size = ((to - from) * ZERO_PAD).next_power_of_two();
        self.spectrum.clear();
        self.spectrum.extend_from_slice(&self.stripped[from..to]);
        self.spectrum.resize(size, Complex::new(0.0, 0.0));
        let fft = self.planner.plan_fft_forward(size);
        self.scratch
            .resize(fft.get_inplace_scratch_len(), Complex::new(0.0, 0.0));
        fft.process_with_scratch(&mut self.spectrum, &mut self.scratch);
        let (peak, power) = self
            .spectrum
            .iter()
            .map(Complex::norm_sqr)
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        Some((peak, power, size))
    }

    fn tone(&mut self, from: usize, to: usize) -> Option<Tone> {
        let (peak, power, size) = self.spectrum_peak(from, to)?;
        let energy: f64 = self.stripped[from..to].iter().map(Complex::norm_sqr).sum();
        let strength = power / (energy * (size as f64).ln());
        if !(energy > 0.0 && strength > MIN_PEAK_OVER_NOISE) {
            return None;
        }
        let at = |i: isize| self.spectrum[i.rem_euclid(size as isize) as usize].norm();
        let (left, mid, right) = (
            at(peak as isize - 1),
            at(peak as isize),
            at(peak as isize + 1),
        );
        let curvature = left - 2.0 * mid + right;
        let shift = if curvature < 0.0 {
            (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        Some(Tone {
            cycles_per_symbol: wrap_half((peak as f64 + shift) / size as f64),
            strength,
        })
    }
}

fn fit_line(segments: &[SegmentTone]) -> Option<(f64, f64)> {
    if segments.len() < 2 {
        return None;
    }
    let w: f64 = segments.iter().map(|s| s.weight).sum();
    let t = segments.iter().map(|s| s.weight * s.at_symbol).sum::<f64>() / w;
    let f = segments
        .iter()
        .map(|s| s.weight * s.cycles_per_symbol)
        .sum::<f64>()
        / w;
    let stt: f64 = segments
        .iter()
        .map(|s| s.weight * (s.at_symbol - t).powi(2))
        .sum();
    if stt <= 0.0 {
        return None;
    }
    let stf: f64 = segments
        .iter()
        .map(|s| s.weight * (s.at_symbol - t) * (s.cycles_per_symbol - f))
        .sum();
    let slope = stf / stt;
    let intercept = f - slope * t;
    if segments.len() > 2 {
        let scatter: f64 = segments
            .iter()
            .map(|s| s.weight * (s.cycles_per_symbol - intercept - slope * s.at_symbol).powi(2))
            .sum::<f64>()
            / w
            * segments.len() as f64
            / (segments.len() - 2) as f64;
        let standard_error = (scatter * w / segments.len() as f64 / stt).sqrt();
        if slope.abs() < MIN_DRIFT_SIGMAS * standard_error {
            return Some((f, 0.0));
        }
    }
    Some((intercept, slope))
}

fn dechirp(stripped: &mut [Complex<f64>], drift: f64) {
    if drift == 0.0 {
        return;
    }
    let centre = (stripped.len() as f64 - 1.0) / 2.0;
    for (k, z) in stripped.iter_mut().enumerate() {
        let t = k as f64 - centre;
        *z *= Complex::from_polar(1.0, -TAU * (0.5 * drift * t * t).rem_euclid(1.0));
    }
}

fn wrap_half(x: f64) -> f64 {
    x - x.round()
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::{
        impair::{Awgn, Impairment},
        rng::Rng,
    };

    use std::f64::consts::PI;

    use super::*;
    use crate::constellation::{Constellation, tables};

    fn stream(table: &Constellation, n: usize, seed: u64) -> Vec<Complex<f32>> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| table.points()[(rng.next_u64() % table.len() as u64) as usize])
            .collect()
    }

    fn chirped(table: &Constellation, n: usize, chirp: Chirp, sigma: f64) -> Vec<Complex<f32>> {
        let mut w = stream(table, n, 0xacc1);
        Chirp {
            centre_cycles_per_symbol: -chirp.centre_cycles_per_symbol,
            drift_cycles_per_symbol2: -chirp.drift_cycles_per_symbol2,
        }
        .remove(&mut w);
        if sigma > 0.0 {
            Awgn::with_sigma(sigma).apply(&mut w, &mut Rng::new(0x5eed));
        }
        w
    }

    #[test]
    fn a_static_offset_reads_back_across_the_unambiguous_range() {
        let table = tables::qam_square(4).unwrap();
        let mut acq = FrequencyAcquisition::new();
        for offset in [-0.12, -0.03, 0.0, 0.002, 0.05, 0.11] {
            let truth = Chirp {
                centre_cycles_per_symbol: offset,
                drift_cycles_per_symbol2: 0.0,
            };
            let w = chirped(&table, 4_000, truth, 0.3);
            let got = acq.estimate(&w, 4).unwrap();
            assert!(
                (got.centre_cycles_per_symbol - offset).abs() < 2e-5,
                "offset {offset}: read {got:?}"
            );
            assert!(got.drift_cycles_per_symbol2.abs() < 1e-7, "{got:?}");
        }
    }

    #[test]
    fn a_frequency_ramp_reads_back_as_drift() {
        for (name, table, order) in [
            ("qpsk", tables::qam_square(4).unwrap(), 4u32),
            ("bpsk", tables::pam(2).unwrap(), 2),
            ("16-qam", tables::qam_square(16).unwrap(), 4),
            ("8-psk", tables::psk(8).unwrap(), 8),
        ] {
            let truth = Chirp {
                centre_cycles_per_symbol: 0.01,
                drift_cycles_per_symbol2: 2e-6,
            };
            let w = chirped(&table, 8_000, truth, 0.1);
            let got = FrequencyAcquisition::new().estimate(&w, order).unwrap();
            assert!(
                (got.centre_cycles_per_symbol - 0.01).abs() < 1e-4,
                "{name}: {got:?}"
            );
            assert!(
                (got.drift_cycles_per_symbol2 - 2e-6).abs() < 2e-7,
                "{name}: {got:?}"
            );
        }
    }

    #[test]
    fn removing_the_estimate_leaves_a_steady_constellation() {
        let table = tables::qam_square(4).unwrap();
        let truth = Chirp {
            centre_cycles_per_symbol: -0.04,
            drift_cycles_per_symbol2: 5e-6,
        };
        let mut w = chirped(&table, 6_000, truth, 0.0);
        let got = FrequencyAcquisition::new().estimate(&w, 4).unwrap();
        got.remove(&mut w);
        let phases: Vec<f64> = w.iter().map(|y| f64::from(y.powu(4).arg())).collect();
        let spread = phases.iter().fold(0.0f64, |acc, &p| {
            acc.max(((p - phases[0] + PI).rem_euclid(TAU) - PI).abs())
        });
        assert!(spread < 0.3, "4th-power phase wanders {spread} rad");
    }

    #[test]
    fn noise_alone_yields_no_estimate() {
        let mut w = vec![Complex::new(0.0f32, 0.0); 4_000];
        Awgn::with_sigma(1.0).apply(&mut w, &mut Rng::new(0x404));
        assert_eq!(FrequencyAcquisition::new().estimate(&w, 4), None);
    }

    #[test]
    fn a_short_burst_is_left_to_the_loop() {
        let table = tables::qam_square(4).unwrap();
        let w = stream(&table, MIN_SYMBOLS - 1, 1);
        assert_eq!(FrequencyAcquisition::new().estimate(&w, 4), None);
    }
}
