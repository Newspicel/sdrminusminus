use core::ops::Range;

use num_complex::Complex;

use crate::fft::FftPair;
use crate::linalg::MAX_ORDER;
use crate::window::hann;

pub const MIN_CORRELATOR_FFT: usize = 64;
pub const MAX_CORRELATOR_FFT: usize = 8192;
pub const DELAY_PAD: usize = 4;

type C64 = Complex<f64>;

const ZERO: Complex<f32> = Complex::new(0.0, 0.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CorrelatorError {
    #[error("fft size {0} is not a power of two in 64..=8192")]
    FftSize(usize),
    #[error("a correlator needs 2 to 16 lanes, not {0}")]
    Lanes(usize),
    #[error("lanes differ in length")]
    LaneLength,
    #[error("baseline {0} does not exist")]
    Baseline(usize),
    #[error("bins {0}..{1} are outside the spectrum")]
    Bins(usize, usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BandVisibility {
    pub amplitude: f64,
    pub phase_rad: f64,
    pub coherence: f64,
    pub snr_db: f32,
}

pub struct FxCorrelator {
    lanes: usize,
    fft_size: usize,
    hop: usize,
    window: Vec<f32>,
    fft: FftPair,
    frames: Vec<Vec<Complex<f32>>>,
    spectra: Vec<Vec<Complex<f32>>>,
    fill: usize,
    pairs: Vec<(usize, usize)>,
    cross: Vec<C64>,
    auto: Vec<f64>,
    count: u64,
    lag: Vec<Complex<f32>>,
    lag_fft: FftPair,
}

impl FxCorrelator {
    pub fn new(lanes: usize, fft_size: usize, overlap: bool) -> Result<Self, CorrelatorError> {
        if !(2..=MAX_ORDER).contains(&lanes) {
            return Err(CorrelatorError::Lanes(lanes));
        }
        if !(fft_size.is_power_of_two()
            && (MIN_CORRELATOR_FFT..=MAX_CORRELATOR_FFT).contains(&fft_size))
        {
            return Err(CorrelatorError::FftSize(fft_size));
        }
        let pairs: Vec<(usize, usize)> = (0..lanes)
            .flat_map(|a| (a + 1..lanes).map(move |b| (a, b)))
            .collect();
        let padded = DELAY_PAD * fft_size;
        Ok(Self {
            lanes,
            fft_size,
            hop: if overlap { fft_size / 2 } else { fft_size },
            window: hann(fft_size),
            fft: FftPair::new(fft_size),
            frames: vec![vec![ZERO; fft_size]; lanes],
            spectra: vec![vec![ZERO; fft_size]; lanes],
            fill: 0,
            cross: vec![C64::new(0.0, 0.0); pairs.len() * fft_size],
            auto: vec![0.0; lanes * fft_size],
            pairs,
            count: 0,
            lag: vec![ZERO; padded],
            lag_fft: FftPair::new(padded),
        })
    }

    #[must_use]
    pub const fn lanes(&self) -> usize {
        self.lanes
    }

    #[must_use]
    pub const fn fft_size(&self) -> usize {
        self.fft_size
    }

    #[must_use]
    pub const fn hop(&self) -> usize {
        self.hop
    }

    #[must_use]
    pub const fn frames(&self) -> u64 {
        self.count
    }

    #[must_use]
    pub fn baselines(&self) -> usize {
        self.pairs.len()
    }

    #[must_use]
    pub fn pair(&self, baseline: usize) -> Option<(usize, usize)> {
        self.pairs.get(baseline).copied()
    }

    pub fn push(&mut self, lanes: &[&[Complex<f32>]]) -> Result<u32, CorrelatorError> {
        if lanes.len() != self.lanes {
            return Err(CorrelatorError::Lanes(lanes.len()));
        }
        let len = lanes[0].len();
        if lanes.iter().any(|lane| lane.len() != len) {
            return Err(CorrelatorError::LaneLength);
        }
        let mut done = 0;
        let mut at = 0;
        while at < len {
            let take = (self.fft_size - self.fill).min(len - at);
            for (frame, lane) in self.frames.iter_mut().zip(lanes) {
                frame[self.fill..self.fill + take].copy_from_slice(&lane[at..at + take]);
            }
            self.fill += take;
            at += take;
            if self.fill == self.fft_size {
                self.transform();
                self.accumulate();
                self.slide();
                done += 1;
            }
        }
        Ok(done)
    }

    #[must_use]
    pub fn visibility(&self, baseline: usize, bin: usize) -> Option<C64> {
        if baseline >= self.pairs.len() || bin >= self.fft_size || self.count == 0 {
            return None;
        }
        Some(self.cross[baseline * self.fft_size + bin] / self.count as f64)
    }

    #[must_use]
    pub fn auto(&self, lane: usize, bin: usize) -> Option<f64> {
        if lane >= self.lanes || bin >= self.fft_size || self.count == 0 {
            return None;
        }
        Some(self.auto[lane * self.fft_size + bin] / self.count as f64)
    }

    pub fn band(
        &self,
        baseline: usize,
        bins: Range<usize>,
    ) -> Result<BandVisibility, CorrelatorError> {
        let (a, b) = self.checked(baseline, &bins)?;
        if self.count == 0 {
            return Ok(BandVisibility::default());
        }
        let sum = self.cross_sum(baseline, &bins);
        let norm = self.power_norm(a, b, &bins);
        let coherence = if norm > 0.0 { sum.norm() / norm } else { 0.0 };
        let snr = coherence * (self.count as f64 * bins.len() as f64).sqrt();
        Ok(BandVisibility {
            amplitude: sum.norm() / self.count as f64,
            phase_rad: sum.arg(),
            coherence,
            snr_db: if snr > 0.0 {
                (20.0 * snr.log10()) as f32
            } else {
                f32::NEG_INFINITY
            },
        })
    }

    pub fn delay_samples(
        &mut self,
        baseline: usize,
        bins: Range<usize>,
    ) -> Result<f64, CorrelatorError> {
        let (a, b) = self.checked(baseline, &bins)?;
        let norm = self.power_norm(a, b, &bins);
        if !(norm.is_finite() && norm > 0.0) {
            return Ok(0.0);
        }
        let f = self.fft_size;
        let padded = self.lag.len() as i64;
        self.lag.fill(ZERO);
        for bin in bins {
            let slot = (bin as i64 - (f / 2) as i64).rem_euclid(padded) as usize;
            let value = self.cross[baseline * f + bin].conj() / norm;
            self.lag[slot] = Complex::new(value.re as f32, value.im as f32);
        }
        self.lag_fft.inverse(&mut self.lag);
        Ok(peak_lag(&self.lag) / DELAY_PAD as f64)
    }

    pub fn clear_integration(&mut self) {
        self.cross.fill(C64::new(0.0, 0.0));
        self.auto.fill(0.0);
        self.count = 0;
    }

    pub fn reset(&mut self) {
        self.clear_integration();
        self.fill = 0;
    }

    fn checked(
        &self,
        baseline: usize,
        bins: &Range<usize>,
    ) -> Result<(usize, usize), CorrelatorError> {
        let pair = self
            .pair(baseline)
            .ok_or(CorrelatorError::Baseline(baseline))?;
        if bins.start >= bins.end || bins.end > self.fft_size {
            return Err(CorrelatorError::Bins(bins.start, bins.end));
        }
        Ok(pair)
    }

    fn cross_sum(&self, baseline: usize, bins: &Range<usize>) -> C64 {
        let base = baseline * self.fft_size;
        self.cross[base + bins.start..base + bins.end].iter().sum()
    }

    fn power_norm(&self, a: usize, b: usize, bins: &Range<usize>) -> f64 {
        let f = self.fft_size;
        let power = |lane: usize| -> f64 {
            self.auto[lane * f + bins.start..lane * f + bins.end]
                .iter()
                .sum()
        };
        (power(a) * power(b)).sqrt()
    }

    fn transform(&mut self) {
        let half = self.fft_size / 2;
        for (spectrum, frame) in self.spectra.iter_mut().zip(&self.frames) {
            for ((slot, sample), weight) in spectrum.iter_mut().zip(frame).zip(&self.window) {
                *slot = sample * weight;
            }
            self.fft.forward(spectrum);
            spectrum.rotate_left(half);
        }
    }

    fn accumulate(&mut self) {
        let f = self.fft_size;
        for (sums, &(a, b)) in self.cross.chunks_exact_mut(f).zip(&self.pairs) {
            for ((sum, x), y) in sums.iter_mut().zip(&self.spectra[a]).zip(&self.spectra[b]) {
                *sum += widen(*x) * widen(*y).conj();
            }
        }
        for (sums, spectrum) in self.auto.chunks_exact_mut(f).zip(&self.spectra) {
            for (sum, x) in sums.iter_mut().zip(spectrum) {
                *sum += widen(*x).norm_sqr();
            }
        }
        self.count += 1;
    }

    fn slide(&mut self) {
        for frame in &mut self.frames {
            frame.copy_within(self.hop.., 0);
        }
        self.fill = self.fft_size - self.hop;
    }
}

fn widen(value: Complex<f32>) -> C64 {
    C64::new(f64::from(value.re), f64::from(value.im))
}

fn peak_lag(lag: &[Complex<f32>]) -> f64 {
    let len = lag.len();
    let magnitude = |index: usize| f64::from(lag[index % len].norm());
    let peak = (0..len)
        .max_by(|&x, &y| magnitude(x).total_cmp(&magnitude(y)))
        .unwrap_or(0);
    let left = magnitude(peak + len - 1);
    let centre = magnitude(peak);
    let right = magnitude(peak + 1);
    let curvature = left - 2.0 * centre + right;
    let shift = if curvature < 0.0 {
        (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let position = peak as f64 + shift;
    if position > len as f64 / 2.0 {
        position - len as f64
    } else {
        position
    }
}

#[cfg(test)]
mod tests;
