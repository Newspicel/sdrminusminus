//! Single-pole IIR helpers (): FM deemphasis, DC removal, and the complex baseband
//! smoother a mixed-down carrier is extracted with.

use std::f64::consts::TAU;

use num_complex::Complex;

/// Smoothing coefficient for `y += c·(x − y)` matching the analog RC time constant `tau_s`
/// at `rate` (matched-z pole `e^(−1/(rate·tau))`). Public because decoders that track a
/// slicing level need the same smoother at a tau of their own choosing, and a second copy of
/// two lines is a second thing to get wrong.
#[must_use]
pub fn one_pole_coeff(rate: f64, tau_s: f64) -> f32 {
    (1.0 - (-1.0 / (rate * tau_s)).exp()) as f32
}

/// FM deemphasis: single-pole lowpass with the region-standard tau (50 µs EU, 75 µs US).
#[derive(Clone, Debug)]
pub struct Deemphasis {
    state: f32,
    coeff: f32,
}

impl Deemphasis {
    #[must_use]
    pub fn new(rate: f64, tau_us: f32) -> Self {
        assert!(rate > 0.0 && tau_us > 0.0, "rate and tau must be positive");
        Self {
            state: 0.0,
            coeff: one_pole_coeff(rate, f64::from(tau_us) * 1e-6),
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        // One non-finite sample latches the recursion forever; healing per block bounds the
        // damage from a driver glitch to a block, without a per-sample branch.
        if !self.state.is_finite() {
            self.state = 0.0;
        }
        for s in samples {
            self.state += self.coeff * (*s - self.state);
            *s = self.state;
        }
    }
}

/// Cascaded single-pole complex lowpass, one `y += c·(x − y)` section per stage. The
/// coefficients are real, so the response is symmetric about DC: a carrier mixed exactly to
/// baseband passes with no phase shift at all — which is what a phase detector reading the
/// output depends on — while its neighbours roll off `stages`·6 dB per octave.
///
/// This is the cheap alternative to an FIR for a *very* narrow band at a high rate: the FM
/// pilot needs a ~400 Hz corner at 240 kHz, which no realisable FIR reaches without decimating
/// first, and decimating would cost the sample-accurate phase the reference is rebuilt from.
#[derive(Clone, Debug)]
pub struct ComplexOnePole {
    stages: Vec<Complex<f32>>,
    coeff: f32,
}

impl ComplexOnePole {
    /// `cutoff_hz` is the −3 dB corner of a single stage; the cascade's own corner is lower.
    #[must_use]
    pub fn new(rate: f64, cutoff_hz: f64, stages: usize) -> Self {
        assert!(
            rate > 0.0 && cutoff_hz > 0.0 && stages > 0,
            "rate, cutoff and stage count must be positive"
        );
        Self {
            stages: vec![Complex::new(0.0, 0.0); stages],
            coeff: one_pole_coeff(rate, 1.0 / (TAU * cutoff_hz)),
        }
    }

    /// Advance one sample. A non-finite input is dropped rather than latched into the
    /// recursion — the per-sample counterpart of the per-block healing above.
    #[must_use]
    pub fn process(&mut self, sample: Complex<f32>) -> Complex<f32> {
        let mut v = if sample.re.is_finite() && sample.im.is_finite() {
            sample
        } else {
            Complex::new(0.0, 0.0)
        };
        for stage in &mut self.stages {
            *stage += (v - *stage) * self.coeff;
            v = *stage;
        }
        v
    }

    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            *stage = Complex::new(0.0, 0.0);
        }
    }
}

/// DC blocker `y[n] = x[n] − x[n−1] + a·y[n−1]`. The 0.995 pole puts the corner near
/// fs/1500 (~32 Hz at 48 kHz) — inaudible, but it kills demodulator DC offsets.
#[derive(Clone, Debug, Default)]
pub struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    const POLE: f32 = 0.995;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        // Same per-block healing as `Deemphasis`: `y1` would latch a non-finite value.
        if !(self.x1.is_finite() && self.y1.is_finite()) {
            self.x1 = 0.0;
            self.y1 = 0.0;
        }
        for s in samples {
            let y = *s - self.x1 + Self::POLE * self.y1;
            self.x1 = *s;
            self.y1 = y;
            *s = y;
        }
    }
}

/// Cascaded single-pole sections in [`Highpass`]. Three is what makes a 300 Hz corner reach
/// −33 dB at 88.5 Hz — enough to take a CTCSS tone out of the audio — while still passing the
/// bottom of the voice band.
const HIGHPASS_SECTIONS: usize = 3;

/// Highpass at `corner_hz`, 6 dB/octave per section, built as `x − lowpass(x)` three times.
///
/// This is an IIR because the job cannot be done any other way at audio rates: taking a
/// subaudible tone out from under speech means a stopband at 250 Hz and a passband at 300,
/// which is a transition of 0.001 of the sample rate at 48 kHz and thousands of FIR taps. A
/// radio does not do that either — it cascades gentle sections and accepts that the highest
/// CTCSS tones are only damped, not removed.
#[derive(Clone, Debug)]
pub struct Highpass {
    lows: [f32; HIGHPASS_SECTIONS],
    coeff: f32,
}

impl Highpass {
    /// # Panics
    /// If `rate` or `corner_hz` is not positive.
    #[must_use]
    pub fn new(rate: f64, corner_hz: f64) -> Self {
        assert!(
            rate > 0.0 && corner_hz > 0.0,
            "rate and corner must be positive"
        );
        Self {
            lows: [0.0; HIGHPASS_SECTIONS],
            coeff: one_pole_coeff(rate, 1.0 / (std::f64::consts::TAU * corner_hz)),
        }
    }

    pub fn reset(&mut self) {
        self.lows = [0.0; HIGHPASS_SECTIONS];
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        // Same per-block healing as the filters above: one non-finite sample would latch.
        if !self.lows.iter().all(|v| v.is_finite()) {
            self.reset();
        }
        for s in samples {
            for low in &mut self.lows {
                *low += self.coeff * (*s - *low);
                *s -= *low;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_1_SQRT_2;

    use super::*;
    use crate::testutil::{complex_tone, real_tone, rms_c, rms_r};

    fn deemphasis_gain(rate: f64, tau_us: f32, freq_hz: f64) -> f32 {
        let mut filter = Deemphasis::new(rate, tau_us);
        let n = (rate * 0.2) as usize;
        let mut x = real_tone(freq_hz / rate, n);
        filter.process(&mut x);
        rms_r(&x[n / 2..]) / FRAC_1_SQRT_2
    }

    #[test]
    fn deemphasis_minus_3_db_point_matches_tau() {
        let (rate, tau_us) = (48_000.0, 50.0f32);
        let expected = 1.0 / (std::f64::consts::TAU * 50e-6);
        let (mut lo, mut hi) = (1_000.0f64, 10_000.0f64);
        for _ in 0..25 {
            let mid = (lo + hi) / 2.0;
            if deemphasis_gain(rate, tau_us, mid) > FRAC_1_SQRT_2 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let found = (lo + hi) / 2.0;
        let rel_err = (found - expected).abs() / expected;
        assert!(
            rel_err < 0.05,
            "-3 dB at {found} Hz, expected {expected} Hz"
        );
    }

    #[test]
    fn deemphasis_recovers_after_non_finite_sample() {
        let mut filter = Deemphasis::new(48_000.0, 50.0);
        let mut poisoned = real_tone(1_000.0 / 48_000.0, 480);
        poisoned[100] = f32::NAN;
        filter.process(&mut poisoned);

        let mut tone = real_tone(1_000.0 / 48_000.0, 9_600);
        filter.process(&mut tone);
        assert!(tone.iter().all(|v| v.is_finite()), "state still poisoned");
        // 50 µs de-emphasis passes 1 kHz at |H| ≈ 0.954.
        let gain = rms_r(&tone[4_800..]) / FRAC_1_SQRT_2;
        assert!((0.9..1.0).contains(&gain), "post-recovery gain {gain}");
    }

    #[test]
    fn dc_blocker_recovers_after_non_finite_sample() {
        let mut blocker = DcBlocker::new();
        let mut poisoned = vec![0.5f32; 480];
        poisoned[7] = f32::INFINITY;
        blocker.process(&mut poisoned);

        let mut tone = real_tone(1_000.0 / 48_000.0, 48_000);
        blocker.process(&mut tone);
        assert!(tone.iter().all(|v| v.is_finite()), "state still poisoned");
        let gain = rms_r(&tone[4_800..]) / FRAC_1_SQRT_2;
        assert!((0.891..1.122).contains(&gain), "post-recovery gain {gain}");
    }

    /// Settled response of a `stages`-deep cascade to a complex tone at `freq_norm`.
    fn cascade_gain(freq_norm: f64, stages: usize) -> f32 {
        let mut filter = ComplexOnePole::new(1.0, CUTOFF_NORM, stages);
        let input = complex_tone(freq_norm, 200_000);
        let out: Vec<Complex<f32>> = input.iter().map(|&s| filter.process(s)).collect();
        rms_c(&out[out.len() / 2..])
    }

    /// −3 dB corner of one stage, normalized to the sample rate.
    const CUTOFF_NORM: f64 = 0.001;

    #[test]
    fn complex_one_pole_matches_the_analytic_single_pole_response() {
        // |H(f)| = (1 + (f/fc)²)^(−stages/2), the cascade of identical RC sections.
        for (freq_norm, stages) in [(CUTOFF_NORM, 1), (10.0 * CUTOFF_NORM, 1), (0.01, 3)] {
            let ratio = freq_norm / CUTOFF_NORM;
            let expected = (1.0 + ratio * ratio).powf(-(stages as f64) / 2.0) as f32;
            let gain = cascade_gain(freq_norm, stages);
            assert!(
                (gain - expected).abs() < 0.1 * expected.max(1e-3),
                "{stages} stages at {ratio}·fc: gain {gain}, expected {expected}"
            );
        }
    }

    /// Real coefficients ⇒ a mirror-image tone is treated identically, and DC passes untouched
    /// in both magnitude *and* phase — the stereo pilot reference is rebuilt from that phase.
    #[test]
    fn complex_one_pole_is_symmetric_about_dc_and_passes_it_unrotated() {
        let above = cascade_gain(0.005, 3);
        let below = cascade_gain(-0.005, 3);
        assert!((above - below).abs() < 1e-4, "{above} vs {below}");

        let mut filter = ComplexOnePole::new(1.0, CUTOFF_NORM, 3);
        let dc = Complex::new(0.6f32, -0.8);
        let mut out = Complex::new(0.0, 0.0);
        for _ in 0..20_000 {
            out = filter.process(dc);
        }
        assert!(
            (out - dc).norm() < 1e-3,
            "dc settled to {out}, expected {dc}"
        );
    }

    #[test]
    fn complex_one_pole_recovers_after_non_finite_sample() {
        let mut filter = ComplexOnePole::new(1.0, CUTOFF_NORM, 2);
        let _ = filter.process(Complex::new(f32::NAN, 0.0));
        let _ = filter.process(Complex::new(0.0, f32::INFINITY));
        let mut out = Complex::new(0.0, 0.0);
        for _ in 0..20_000 {
            out = filter.process(Complex::new(1.0, 0.0));
        }
        assert!(out.re.is_finite() && out.im.is_finite(), "state poisoned");
        assert!((out.re - 1.0).abs() < 1e-3, "post-recovery gain {}", out.re);
    }

    fn highpass_gain(corner_hz: f64, freq_hz: f64) -> f32 {
        let rate = 48_000.0;
        let mut filter = Highpass::new(rate, corner_hz);
        let n = (rate * 2.0) as usize;
        let mut x = real_tone(freq_hz / rate, n);
        filter.process(&mut x);
        rms_r(&x[n / 2..]) / FRAC_1_SQRT_2
    }

    /// The numbers the audio path depends on: a CTCSS tone damped out of audibility, the
    /// voice band left where it was.
    #[test]
    fn highpass_takes_the_subaudible_band_out_and_keeps_the_voice_band() {
        // Three cascaded 6 dB/octave sections: (f / √(f² + fc²))³.
        let analytic = |f: f64| (f / f.hypot(300.0)).powi(3) as f32;
        for freq_hz in [67.0f64, 88.5, 254.1, 300.0, 1_000.0] {
            let gain = highpass_gain(300.0, freq_hz);
            let want = analytic(freq_hz);
            assert!(
                (gain / want - 1.0).abs() < 0.1,
                "{freq_hz} Hz: gain {gain}, expected {want}"
            );
        }
        // The two ends of the trade: a CTCSS tone is gone from the audio, the voice is not.
        // The discrete sections shed a little more than the analog form at the top of the
        // voice band — ~0.7 dB at 3 kHz — which is the price of doing this at 48 kHz at all.
        assert!(highpass_gain(300.0, 88.5) < 0.03);
        assert!(highpass_gain(300.0, 3_000.0) > 0.9);
    }

    #[test]
    fn highpass_removes_a_dc_offset_and_recovers_from_a_non_finite_sample() {
        let mut filter = Highpass::new(48_000.0, 300.0);
        let mut dc = vec![1.0f32; 48_000];
        filter.process(&mut dc);
        assert!(dc[dc.len() - 1].abs() < 0.01, "dc residue {}", dc[47_999]);

        let mut poisoned = vec![0.5f32; 480];
        poisoned[7] = f32::NAN;
        filter.process(&mut poisoned);
        let mut tone = real_tone(1_000.0 / 48_000.0, 48_000);
        filter.process(&mut tone);
        assert!(tone.iter().all(|v| v.is_finite()), "state still poisoned");
    }

    #[test]
    fn dc_blocker_kills_dc_and_passes_1_khz() {
        let mut blocker = DcBlocker::new();
        let mut dc = vec![1.0f32; 48_000];
        blocker.process(&mut dc);
        let tail = dc[dc.len() - 1].abs();
        assert!(tail < 0.01, "dc residue {tail}");

        let mut blocker = DcBlocker::new();
        let mut tone = real_tone(1_000.0 / 48_000.0, 48_000);
        blocker.process(&mut tone);
        let gain = rms_r(&tone[4_800..]) / FRAC_1_SQRT_2;
        assert!((0.891..1.122).contains(&gain), "1 kHz gain {gain}");
    }
}
