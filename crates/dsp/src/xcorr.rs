use num_complex::Complex;

use crate::fft::FftPair;

/// What one lane looks like measured against another: how far behind it runs, by how much its
/// amplitude and phase differ, and how much of the two is genuinely the same signal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayEstimate {
    /// Positive when `b` lags `a`.
    pub delay_samples: f32,
    pub phase_rad: f32,
    pub gain: f32,
    /// Magnitude-squared coherence at the peak, in `0..=1`.
    pub coherence: f32,
}

impl DelayEstimate {
    #[must_use]
    pub const fn nothing() -> Self {
        Self {
            delay_samples: 0.0,
            phase_rad: 0.0,
            gain: 0.0,
            coherence: 0.0,
        }
    }
}

/// FFT cross-correlation sized once for a frame length, then reused.
///
/// The transform runs at twice the frame length so the correlation is linear rather than
/// circular: a lag near the frame length must not wrap round and be read as a lag near zero.
pub struct XCorr {
    fft: FftPair,
    frame: usize,
    a: Vec<Complex<f32>>,
    b: Vec<Complex<f32>>,
}

impl XCorr {
    #[must_use]
    pub fn new(frame: usize) -> Self {
        let size = (frame * 2).next_power_of_two().max(2);
        Self {
            fft: FftPair::new(size),
            frame,
            a: vec![Complex::default(); size],
            b: vec![Complex::default(); size],
        }
    }

    #[must_use]
    pub const fn frame(&self) -> usize {
        self.frame
    }

    /// Fills `out` with `r[τ] = Σ b[n + τ] · conj(a[n])` for lags `-frame..frame`, in that order,
    /// so index `frame` is lag zero.
    pub fn correlate(&mut self, a: &[Complex<f32>], b: &[Complex<f32>], out: &mut Vec<f32>) {
        let frame = self.frame as isize;
        let cross = self.cross(a, b);
        out.clear();
        out.reserve(2 * frame as usize);
        let size = cross.len() as isize;
        for lag in -frame..frame {
            out.push(cross[lag.rem_euclid(size) as usize].norm());
        }
    }

    /// The whole measurement in one pass: peak lag to a fraction of a sample, and the complex
    /// ratio and coherence at that lag.
    pub fn estimate(&mut self, a: &[Complex<f32>], b: &[Complex<f32>]) -> DelayEstimate {
        let energy_a: f32 = a.iter().take(self.frame).map(Complex::norm_sqr).sum();
        let energy_b: f32 = b.iter().take(self.frame).map(Complex::norm_sqr).sum();
        if energy_a <= f32::MIN_POSITIVE || energy_b <= f32::MIN_POSITIVE {
            return DelayEstimate::nothing();
        }
        let frame = self.frame as isize;
        let cross = self.cross(a, b);
        let size = cross.len() as isize;
        let at = |lag: isize| cross[lag.rem_euclid(size) as usize];
        let mut peak = 0isize;
        let mut best = 0.0f32;
        for lag in -frame..frame {
            let power = at(lag).norm_sqr();
            if power > best {
                best = power;
                peak = lag;
            }
        }
        let value = at(peak);
        let (left, right) = (at(peak - 1).norm(), at(peak + 1).norm());
        let centre = value.norm();
        let denominator = left - 2.0 * centre + right;
        let fraction = if denominator.abs() > f32::MIN_POSITIVE {
            (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        DelayEstimate {
            delay_samples: peak as f32 + fraction,
            phase_rad: value.arg(),
            gain: centre / energy_a,
            coherence: (best / (energy_a * energy_b)).clamp(0.0, 1.0),
        }
    }

    fn cross(&mut self, a: &[Complex<f32>], b: &[Complex<f32>]) -> &[Complex<f32>] {
        let size = self.fft.len();
        let frame = self.frame;
        self.a.clear();
        self.a.extend(a.iter().take(frame).copied());
        self.a.resize(size, Complex::default());
        self.b.clear();
        self.b.extend(b.iter().take(frame).copied());
        self.b.resize(size, Complex::default());
        self.fft.forward(&mut self.a);
        self.fft.forward(&mut self.b);
        for (bin, reference) in self.b.iter_mut().zip(&self.a) {
            *bin *= reference.conj();
        }
        self.fft.inverse_scaled(&mut self.b);
        &self.b
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    fn chirp(len: usize, offset: f32) -> Vec<Complex<f32>> {
        (0..len)
            .map(|k| {
                let t = k as f32 + offset;
                Complex::from_polar(1.0, TAU * (0.02 * t + 0.000_1 * t * t))
            })
            .collect()
    }

    fn noisy(samples: &[Complex<f32>], amplitude: f32, seed: u64) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u32 << 23) as f32 - 1.0
        };
        samples
            .iter()
            .map(|s| s + Complex::new(next() * amplitude, next() * amplitude))
            .collect()
    }

    #[test]
    fn an_integer_delay_lands_on_its_own_lag() {
        let mut xcorr = XCorr::new(1024);
        let a = chirp(1024, 0.0);
        for delay in [0isize, 1, 7, 64, -13] {
            let b = chirp(1024, -delay as f32);
            let estimate = xcorr.estimate(&a, &b);
            assert!(
                (estimate.delay_samples - delay as f32).abs() < 0.05,
                "delay {delay}: measured {}",
                estimate.delay_samples
            );
            assert!(estimate.coherence > 0.8, "{:?}", estimate);
        }
    }

    #[test]
    fn a_fractional_delay_is_recovered_to_a_tenth_of_a_sample() {
        let mut xcorr = XCorr::new(2048);
        let a = chirp(2048, 0.0);
        for delay in [0.25f32, 0.5, -0.75, 2.4] {
            let b = chirp(2048, -delay);
            let estimate = xcorr.estimate(&a, &b);
            assert!(
                (estimate.delay_samples - delay).abs() < 0.1,
                "delay {delay}: measured {}",
                estimate.delay_samples
            );
        }
    }

    #[test]
    fn realistic_noise_still_leaves_the_delay_within_a_tenth_of_a_sample() {
        let mut xcorr = XCorr::new(4096);
        let clean = chirp(4096, 0.0);
        let a = noisy(&clean, 0.3, 0x1234);
        let b = noisy(&chirp(4096, -3.0), 0.3, 0x9876);
        let estimate = xcorr.estimate(&a, &b);
        assert!(
            (estimate.delay_samples - 3.0).abs() < 0.1,
            "measured {}",
            estimate.delay_samples
        );
        assert!(estimate.coherence > 0.5, "{estimate:?}");
    }

    #[test]
    fn a_phase_and_gain_offset_come_back_as_measured() {
        let mut xcorr = XCorr::new(1024);
        let a = chirp(1024, 0.0);
        let rotation = Complex::from_polar(0.5f32, 1.1);
        let b: Vec<Complex<f32>> = a.iter().map(|s| s * rotation).collect();
        let estimate = xcorr.estimate(&a, &b);
        assert!(estimate.delay_samples.abs() < 0.05);
        assert!((estimate.phase_rad - 1.1).abs() < 0.02, "{estimate:?}");
        assert!((estimate.gain - 0.5).abs() < 0.02, "{estimate:?}");
        assert!(estimate.coherence > 0.99, "{estimate:?}");
    }

    #[test]
    fn two_unrelated_signals_report_no_coherence() {
        let mut xcorr = XCorr::new(2048);
        let a = noisy(&vec![Complex::default(); 2048], 1.0, 0x1111);
        let b = noisy(&vec![Complex::default(); 2048], 1.0, 0x2222);
        let estimate = xcorr.estimate(&a, &b);
        assert!(estimate.coherence < 0.05, "{estimate:?}");
    }

    #[test]
    fn silence_is_reported_rather_than_guessed_at() {
        let mut xcorr = XCorr::new(256);
        let quiet = vec![Complex::default(); 256];
        let estimate = xcorr.estimate(&quiet, &chirp(256, 0.0));
        assert_eq!(estimate, DelayEstimate::nothing());
    }

    #[test]
    fn the_correlation_curve_peaks_where_the_estimate_says() {
        let mut xcorr = XCorr::new(512);
        let a = chirp(512, 0.0);
        let b = chirp(512, -9.0);
        let mut curve = Vec::new();
        xcorr.correlate(&a, &b, &mut curve);
        assert_eq!(curve.len(), 1024);
        let peak = curve
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .map(|(index, _)| index as isize - 512);
        assert_eq!(peak, Some(9));
    }
}
