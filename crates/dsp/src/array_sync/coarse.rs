use num_complex::Complex;

use super::cfo::{SpectralShift, derotate, parabolic};
use crate::fft::FftPair;

pub const COARSE_FRAME: usize = 32_768;
pub const COARSE_LAGS: usize = 16_384;
pub const COARSE_PEAK_DB: f32 = 15.0;
pub const COARSE_SECOND_DB: f32 = 6.0;
pub const MAX_CFO_HZ: f64 = 5_000.0;

const REACH_S: f64 = 0.25;
const MAX_DECIMATION: usize = 1_024;
const SPECTRUM_BINS: usize = 1_024;
const SEGMENT: usize = 1_024;
const PEAK_GUARD: usize = 2;

#[must_use]
pub fn coarse_decimation(sample_rate: f64, lags: usize) -> usize {
    if !(sample_rate.is_finite() && sample_rate > 0.0) || lags == 0 {
        return 1;
    }
    let wanted = (REACH_S * sample_rate / lags as f64).ceil();
    if wanted >= MAX_DECIMATION as f64 {
        return MAX_DECIMATION;
    }
    (wanted.max(1.0) as usize)
        .next_power_of_two()
        .min(MAX_DECIMATION)
}

pub struct Boxcar {
    factor: usize,
    acc: Vec<Complex<f32>>,
    filled: usize,
}

impl Boxcar {
    #[must_use]
    pub fn new(lanes: usize, factor: usize) -> Self {
        Self {
            factor: factor.max(1),
            acc: vec![Complex::default(); lanes],
            filled: 0,
        }
    }

    #[must_use]
    pub const fn factor(&self) -> usize {
        self.factor
    }

    pub fn reset(&mut self) {
        self.acc.fill(Complex::default());
        self.filled = 0;
    }

    pub fn push(
        &mut self,
        lanes: &[&[Complex<f32>]],
        out: &mut [Vec<Complex<f32>>],
    ) -> Result<(), CoarseError> {
        let expected = self.acc.len();
        if let Some(got) = [lanes.len(), out.len()]
            .into_iter()
            .find(|&count| count != expected)
        {
            return Err(CoarseError::LaneCount { expected, got });
        }
        let samples = lanes.first().map_or(0, |lane| lane.len());
        if lanes.iter().any(|lane| lane.len() != samples) {
            return Err(CoarseError::LaneLength);
        }
        let scale = 1.0 / self.factor as f32;
        let mut at = 0;
        while at < samples {
            let take = (self.factor - self.filled).min(samples - at);
            for (acc, lane) in self.acc.iter_mut().zip(lanes) {
                *acc += lane[at..at + take].iter().sum::<Complex<f32>>();
            }
            self.filled += take;
            at += take;
            if self.filled == self.factor {
                for (acc, out) in self.acc.iter_mut().zip(out.iter_mut()) {
                    out.push(*acc * scale);
                    *acc = Complex::default();
                }
                self.filled = 0;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoarseLag {
    pub lag: i64,
    pub peak_db: f32,
    pub second_db: f32,
    pub cfo_hz: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CoarseError {
    #[error("no correlation peak")]
    NoPeak,
    #[error("ambiguous correlation")]
    Ambiguous,
    #[error("capture too short")]
    Short,
    #[error("sample rate out of range")]
    Rate,
    #[error("expected {expected} lanes, got {got}")]
    LaneCount { expected: usize, got: usize },
    #[error("lanes differ in length")]
    LaneLength,
}

pub struct CoarseSearch {
    fft: FftPair,
    frame: usize,
    lags: usize,
    a: Vec<Complex<f32>>,
    b: Vec<Complex<f32>>,
    lane_spectrum: Vec<Complex<f32>>,
    power: Vec<f64>,
    shift: SpectralShift,
}

impl CoarseSearch {
    #[must_use]
    pub fn new(frame: usize, lags: usize) -> Self {
        let frame = frame.max(1);
        let size = (frame + 2 * lags).next_power_of_two().max(2);
        Self {
            fft: FftPair::new(size),
            frame,
            lags,
            a: vec![Complex::default(); size],
            b: vec![Complex::default(); size],
            lane_spectrum: vec![Complex::default(); size],
            power: vec![0.0; 2 * lags + 1],
            shift: SpectralShift::new(SPECTRUM_BINS),
        }
    }

    #[must_use]
    pub const fn span(&self) -> usize {
        self.frame + 2 * self.lags
    }

    pub fn lag(
        &mut self,
        reference: &[Complex<f32>],
        lane: &[Complex<f32>],
    ) -> Result<CoarseLag, CoarseError> {
        self.check(reference, lane)?;
        self.coherent(reference, lane, 0.0)
    }

    pub fn lag_across_clocks(
        &mut self,
        reference: &[Complex<f32>],
        lane: &[Complex<f32>],
        rate_hz: f64,
    ) -> Result<CoarseLag, CoarseError> {
        if !(rate_hz.is_finite() && rate_hz > 0.0) {
            return Err(CoarseError::Rate);
        }
        self.check(reference, lane)?;
        let reach = (MAX_CFO_HZ / rate_hz * self.shift.bins() as f64).ceil() as usize;
        let rough = self.shift.estimate(
            &reference[self.lags..self.lags + self.frame],
            &lane[..self.span()],
            reach,
        );
        let index = self.incoherent_peak(reference, lane, rough);
        let cycles = rough + self.residual(reference, lane, index, rough);
        let found = self.coherent(reference, lane, cycles)?;
        Ok(CoarseLag {
            cfo_hz: cycles * rate_hz,
            ..found
        })
    }

    fn check(&self, reference: &[Complex<f32>], lane: &[Complex<f32>]) -> Result<(), CoarseError> {
        if reference.len() < self.span() || lane.len() < self.span() {
            return Err(CoarseError::Short);
        }
        Ok(())
    }

    fn load_reference(&mut self, reference: &[Complex<f32>], start: usize, end: usize) {
        self.a.fill(Complex::default());
        self.a[start..end].copy_from_slice(&reference[self.lags + start..self.lags + end]);
        self.fft.forward(&mut self.a);
    }

    fn coherent(
        &mut self,
        reference: &[Complex<f32>],
        lane: &[Complex<f32>],
        cycles: f64,
    ) -> Result<CoarseLag, CoarseError> {
        let span = self.span();
        self.load_reference(reference, 0, self.frame);
        self.b.fill(Complex::default());
        derotate(&lane[..span], 0, cycles, &mut self.b[..span]);
        self.fft.forward(&mut self.b);
        for (value, reference) in self.b.iter_mut().zip(&self.a) {
            *value *= reference.conj();
        }
        self.fft.inverse(&mut self.b);
        for (power, value) in self.power.iter_mut().zip(&self.b) {
            *power = f64::from(value.norm_sqr());
        }
        self.judge()
    }

    fn judge(&self) -> Result<CoarseLag, CoarseError> {
        let (peak, best) = self
            .power
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .ok_or(CoarseError::NoPeak)?;
        let mean = self.power.iter().sum::<f64>() / self.power.len() as f64;
        if !(best.is_finite() && mean.is_finite()) || best <= 0.0 || mean <= 0.0 {
            return Err(CoarseError::NoPeak);
        }
        let second = self
            .power
            .iter()
            .enumerate()
            .filter(|(index, _)| index.abs_diff(peak) > PEAK_GUARD)
            .map(|(_, power)| *power)
            .fold(0.0, f64::max);
        let peak_db = (10.0 * (best / mean).log10()) as f32;
        let second_db = if second > 0.0 {
            (10.0 * (best / second).log10()) as f32
        } else {
            f32::INFINITY
        };
        if peak_db.is_nan() || peak_db < COARSE_PEAK_DB {
            return Err(CoarseError::NoPeak);
        }
        if second_db < COARSE_SECOND_DB {
            return Err(CoarseError::Ambiguous);
        }
        Ok(CoarseLag {
            lag: peak as i64 - self.lags as i64,
            peak_db,
            second_db,
            cfo_hz: 0.0,
        })
    }

    fn incoherent_peak(
        &mut self,
        reference: &[Complex<f32>],
        lane: &[Complex<f32>],
        cycles: f64,
    ) -> usize {
        let span = self.span();
        self.lane_spectrum.fill(Complex::default());
        derotate(&lane[..span], 0, cycles, &mut self.lane_spectrum[..span]);
        self.fft.forward(&mut self.lane_spectrum);
        self.power.fill(0.0);
        let segment = SEGMENT.min(self.frame);
        for start in (0..self.frame).step_by(segment) {
            self.load_reference(reference, start, (start + segment).min(self.frame));
            for ((value, lane), reference) in
                self.b.iter_mut().zip(&self.lane_spectrum).zip(&self.a)
            {
                *value = lane * reference.conj();
            }
            self.fft.inverse(&mut self.b);
            for (power, value) in self.power.iter_mut().zip(&self.b) {
                *power += f64::from(value.norm_sqr());
            }
        }
        self.power
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(self.lags, |(index, _)| index)
    }

    fn residual(
        &mut self,
        reference: &[Complex<f32>],
        lane: &[Complex<f32>],
        index: usize,
        cycles: f64,
    ) -> f64 {
        let frame = self.frame;
        self.b.fill(Complex::default());
        derotate(
            &lane[index..index + frame],
            index,
            cycles,
            &mut self.b[..frame],
        );
        for (value, reference) in self.b[..frame]
            .iter_mut()
            .zip(&reference[self.lags..self.lags + frame])
        {
            *value *= reference.conj();
        }
        self.fft.forward(&mut self.b);
        let size = self.b.len() as isize;
        let reach = (size / SEGMENT.min(frame) as isize).max(1);
        let magnitude = |bin: isize| f64::from(self.b[bin.rem_euclid(size) as usize].norm());
        let best = (-reach..=reach)
            .max_by(|a, b| magnitude(*a).total_cmp(&magnitude(*b)))
            .unwrap_or(0);
        let fraction = parabolic(magnitude(best - 1), magnitude(best), magnitude(best + 1));
        (best as f64 + fraction) / size as f64
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;
    use crate::array_sync::signals::Gaussian;

    const RATE: f64 = 2_400_000.0;
    const SHIFT: usize = 480_000;

    struct Pair {
        reference: Vec<Complex<f32>>,
        lane: Vec<Complex<f32>>,
        factor: usize,
    }

    fn carriers(len: usize, lines: &[(f64, f32)]) -> Vec<Complex<f32>> {
        let mut phasors: Vec<(Complex<f64>, Complex<f64>)> = lines
            .iter()
            .map(|&(hz, amplitude)| {
                (
                    Complex::new(f64::from(amplitude), 0.0),
                    Complex::from_polar(1.0, TAU * hz / RATE),
                )
            })
            .collect();
        (0..len)
            .map(|_| {
                let mut sum = Complex::<f64>::default();
                for (phasor, step) in &mut phasors {
                    sum += *phasor;
                    *phasor *= *step;
                }
                Complex::new(sum.re as f32, sum.im as f32)
            })
            .collect()
    }

    fn decimated_pair(cfo_hz: f64, lines: &[(f64, f32)], seed: u64) -> Pair {
        let factor = coarse_decimation(RATE, COARSE_LAGS);
        let raw = (COARSE_FRAME + 2 * COARSE_LAGS) * factor;
        let mut noise = Gaussian::new(seed);
        let tones = carriers(raw + SHIFT, lines);
        let common: Vec<Complex<f32>> = tones.iter().map(|tone| tone + noise.sample()).collect();
        let step = Complex::from_polar(1.0, TAU * cfo_hz / RATE);
        let mut rotation = Complex::<f64>::new(1.0, 0.0);
        let mut boxcar = Boxcar::new(2, factor);
        let mut out: Vec<Vec<Complex<f32>>> =
            (0..2).map(|_| Vec::with_capacity(raw / factor)).collect();
        let mut reference = Vec::new();
        let mut lane = Vec::new();
        for start in (0..raw).step_by(65_536) {
            let end = (start + 65_536).min(raw);
            reference.clear();
            lane.clear();
            for n in start..end {
                reference.push(common[n + SHIFT] + noise.sample() * 0.5);
                let turn = Complex::new(rotation.re as f32, rotation.im as f32);
                lane.push((common[n] + noise.sample() * 0.5) * turn);
                rotation *= step;
            }
            boxcar.push(&[&reference, &lane], &mut out).unwrap();
        }
        let lane = out.pop().unwrap_or_default();
        let reference = out.pop().unwrap_or_default();
        Pair {
            reference,
            lane,
            factor,
        }
    }

    #[test]
    fn decimation_reaches_a_quarter_second() {
        assert_eq!(coarse_decimation(RATE, COARSE_LAGS), 64);
        assert!(64.0 * COARSE_LAGS as f64 / RATE >= 0.25);
        assert_eq!(coarse_decimation(48_000.0, COARSE_LAGS), 1);
        assert_eq!(coarse_decimation(1e10, COARSE_LAGS), 1024);
        assert_eq!(coarse_decimation(f64::NAN, COARSE_LAGS), 1);
    }

    #[test]
    fn boxcar_keeps_the_decimated_phase_common_to_all_lanes() {
        let source = Gaussian::new(30).block(64 * 50 + 17);
        let gains = [
            Complex::new(1.0, 0.0),
            Complex::from_polar(0.5, 2.0),
            Complex::from_polar(2.0, -1.0),
        ];
        let lanes: Vec<Vec<Complex<f32>>> = gains
            .iter()
            .map(|gain| source.iter().map(|value| value * gain).collect())
            .collect();
        let mut boxcar = Boxcar::new(3, 64);
        let mut out = vec![Vec::new(); 3];
        let mut at = 0;
        for size in [1, 63, 64, 65, 7, 1000, 2000, 50].into_iter().cycle() {
            let end = (at + size).min(source.len());
            let views: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[at..end]).collect();
            boxcar.push(&views, &mut out).unwrap();
            at = end;
            if at == source.len() {
                break;
            }
        }
        assert!(out.iter().all(|lane| lane.len() == 50));
        for (index, value) in out[0].iter().enumerate() {
            let expected = source[index * 64..(index + 1) * 64]
                .iter()
                .sum::<Complex<f32>>()
                / 64.0;
            assert!((value - expected).norm() < 1e-5);
            for (lane, gain) in out.iter().zip(gains).skip(1) {
                assert!((lane[index] - value * gain).norm() < 1e-5);
            }
        }
    }

    #[test]
    fn coarse_finds_a_200_ms_lag_on_noise() {
        let pair = decimated_pair(0.0, &[], 1);
        let mut search = CoarseSearch::new(COARSE_FRAME, COARSE_LAGS);
        let found = search.lag(&pair.reference, &pair.lane).unwrap();
        let raw = found.lag * pair.factor as i64;
        assert!(raw.abs_diff(SHIFT as i64) <= pair.factor as u64, "{raw}");
        assert!(found.peak_db > 30.0, "{found:?}");
        assert!(found.second_db > 20.0, "{found:?}");
    }

    #[test]
    fn coarse_finds_a_lag_under_a_1_khz_cfo() {
        let lines = [(-9_000.0, 0.02), (2_100.0, 0.02), (11_000.0, 0.02)];
        let pair = decimated_pair(1_000.0, &lines, 2);
        let mut search = CoarseSearch::new(COARSE_FRAME, COARSE_LAGS);
        assert!(search.lag(&pair.reference, &pair.lane).is_err());
        let rate = RATE / pair.factor as f64;
        let found = search
            .lag_across_clocks(&pair.reference, &pair.lane, rate)
            .unwrap();
        assert_eq!(found.lag * pair.factor as i64, SHIFT as i64);
        assert!((found.cfo_hz - 1_000.0).abs() < 2.0, "{found:?}");
        assert!(found.peak_db > 20.0, "{found:?}");
    }

    #[test]
    fn lanes_on_one_clock_measure_no_cfo() {
        let pair = decimated_pair(0.0, &[], 3);
        let mut search = CoarseSearch::new(COARSE_FRAME, COARSE_LAGS);
        let rate = RATE / pair.factor as f64;
        let found = search
            .lag_across_clocks(&pair.reference, &pair.lane, rate)
            .unwrap();
        assert!((found.lag * pair.factor as i64).abs_diff(SHIFT as i64) <= pair.factor as u64);
        assert!(found.cfo_hz.abs() < 0.5, "{found:?}");
    }

    #[test]
    fn coarse_refuses_a_periodic_signal_as_ambiguous() {
        let burst = Gaussian::new(31).block(4_096);
        let mut search = CoarseSearch::new(COARSE_FRAME, COARSE_LAGS);
        let reference: Vec<Complex<f32>> = (0..search.span()).map(|n| burst[n % 4_096]).collect();
        let lane: Vec<Complex<f32>> = (0..search.span())
            .map(|n| burst[(n + 4_096 - 1_000) % 4_096])
            .collect();
        assert_eq!(search.lag(&reference, &lane), Err(CoarseError::Ambiguous));
    }

    #[test]
    fn a_short_capture_or_a_bad_rate_is_refused() {
        let mut search = CoarseSearch::new(1_024, 256);
        let short = vec![Complex::new(1.0, 0.0); search.span() - 1];
        let long = vec![Complex::new(1.0, 0.0); search.span()];
        assert_eq!(search.lag(&short, &long), Err(CoarseError::Short));
        assert_eq!(search.lag(&long, &short), Err(CoarseError::Short));
        assert_eq!(
            search.lag_across_clocks(&long, &long, f64::NAN),
            Err(CoarseError::Rate)
        );
        let silence = vec![Complex::default(); search.span()];
        assert_eq!(search.lag(&silence, &silence), Err(CoarseError::NoPeak));
    }

    #[test]
    fn a_lagging_lane_has_a_positive_lag() {
        let mut search = CoarseSearch::new(4_096, 1_024);
        let common = Gaussian::new(33).block(search.span() + 600);
        let early = &common[600..600 + search.span()];
        let late = &common[..search.span()];
        assert_eq!(search.lag(early, late).map(|found| found.lag), Ok(600));
        assert_eq!(search.lag(late, early).map(|found| found.lag), Ok(-600));
    }

    #[test]
    fn a_boxcar_refuses_mismatched_lanes() {
        let mut boxcar = Boxcar::new(2, 4);
        let long = [Complex::new(1.0, 0.0); 8];
        let short = [Complex::new(1.0, 0.0); 7];
        let mut out = vec![Vec::new(); 2];
        assert_eq!(
            boxcar.push(&[&long], &mut out),
            Err(CoarseError::LaneCount {
                expected: 2,
                got: 1
            })
        );
        assert_eq!(
            boxcar.push(&[&long, &long], &mut out[..1]),
            Err(CoarseError::LaneCount {
                expected: 2,
                got: 1
            })
        );
        assert_eq!(
            boxcar.push(&[&long, &short], &mut out),
            Err(CoarseError::LaneLength)
        );
        assert!(out.iter().all(Vec::is_empty));
        assert_eq!(boxcar.push(&[&long, &long], &mut out), Ok(()));
        assert!(out.iter().all(|lane| lane.len() == 2));
    }

    #[test]
    fn unrelated_lanes_have_no_peak() {
        let mut noise = Gaussian::new(32);
        let mut search = CoarseSearch::new(4_096, 1_024);
        let reference = noise.block(search.span());
        let lane = noise.block(search.span());
        assert_eq!(search.lag(&reference, &lane), Err(CoarseError::NoPeak));
    }
}
