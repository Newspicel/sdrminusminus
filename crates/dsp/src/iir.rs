use std::f64::consts::TAU;

use num_complex::Complex;

#[must_use]
pub fn one_pole_coeff(rate: f64, tau_s: f64) -> f32 {
    (1.0 - (-1.0 / (rate * tau_s)).exp()) as f32
}

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
        if !self.state.is_finite() {
            self.state = 0.0;
        }
        for s in samples {
            self.state += self.coeff * (*s - self.state);
            *s = self.state;
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComplexOnePole {
    stages: Vec<Complex<f32>>,
    coeff: f32,
}

impl ComplexOnePole {
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

/// Removes a zero-IF front end's own DC term from a complex stream.
///
/// A signal genuinely at 0 Hz is arithmetically indistinguishable from that offset and is removed
/// with it, so the corner must stay far below one analysis bin.
#[derive(Clone, Debug)]
pub struct IqDcBlocker {
    mean: Complex<f32>,
    coeff: f32,
}

impl IqDcBlocker {
    #[must_use]
    pub fn new(rate: f64, corner_hz: f64) -> Self {
        assert!(
            rate > 0.0 && corner_hz > 0.0,
            "rate and corner must be positive"
        );
        Self {
            mean: Complex::new(0.0, 0.0),
            coeff: one_pole_coeff(rate, 1.0 / (TAU * corner_hz)),
        }
    }

    pub fn reset(&mut self) {
        self.mean = Complex::new(0.0, 0.0);
    }

    pub fn process(&mut self, samples: &mut [Complex<f32>]) {
        if !(self.mean.re.is_finite() && self.mean.im.is_finite()) {
            self.reset();
        }
        for s in samples {
            if !(s.re.is_finite() && s.im.is_finite()) {
                *s = Complex::new(0.0, 0.0);
                continue;
            }
            self.mean += (*s - self.mean) * self.coeff;
            *s -= self.mean;
        }
    }
}

const HIGHPASS_SECTIONS: usize = 3;

#[derive(Clone, Debug)]
pub struct Highpass {
    lows: [f32; HIGHPASS_SECTIONS],
    coeff: f32,
}

impl Highpass {
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

#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    const MIN_Q: f64 = 0.05;

    fn from_unnormalized(b: [f64; 3], a: [f64; 3]) -> Self {
        let a0 = if a[0].abs() > f64::MIN_POSITIVE {
            a[0]
        } else {
            1.0
        };
        Self {
            b0: (b[0] / a0) as f32,
            b1: (b[1] / a0) as f32,
            b2: (b[2] / a0) as f32,
            a1: (a[1] / a0) as f32,
            a2: (a[2] / a0) as f32,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn geometry(rate: f64, freq_hz: f64, q: f64) -> (f64, f64, f64) {
        let nyquist = rate / 2.0;
        let freq = freq_hz.clamp(rate * 1e-4, nyquist * 0.995);
        let w0 = TAU * freq / rate;
        let alpha = w0.sin() / (2.0 * q.max(Self::MIN_Q));
        (w0.cos(), w0.sin(), alpha)
    }

    #[must_use]
    pub fn lowpass(rate: f64, freq_hz: f64, q: f64) -> Self {
        assert!(rate > 0.0, "rate must be positive");
        let (cos_w0, _, alpha) = Self::geometry(rate, freq_hz, q);
        Self::from_unnormalized(
            [(1.0 - cos_w0) / 2.0, 1.0 - cos_w0, (1.0 - cos_w0) / 2.0],
            [1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha],
        )
    }

    #[must_use]
    pub fn highpass(rate: f64, freq_hz: f64, q: f64) -> Self {
        assert!(rate > 0.0, "rate must be positive");
        let (cos_w0, _, alpha) = Self::geometry(rate, freq_hz, q);
        Self::from_unnormalized(
            [(1.0 + cos_w0) / 2.0, -(1.0 + cos_w0), (1.0 + cos_w0) / 2.0],
            [1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha],
        )
    }

    #[must_use]
    pub fn notch(rate: f64, freq_hz: f64, q: f64) -> Self {
        assert!(rate > 0.0, "rate must be positive");
        let (cos_w0, _, alpha) = Self::geometry(rate, freq_hz, q);
        Self::from_unnormalized(
            [1.0, -2.0 * cos_w0, 1.0],
            [1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha],
        )
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        if !(self.z1.is_finite() && self.z2.is_finite()) {
            self.reset();
        }
        for s in samples {
            let x = *s;
            let y = self.b0 * x + self.z1;
            self.z1 = self.b1 * x - self.a1 * y + self.z2;
            self.z2 = self.b2 * x - self.a2 * y;
            *s = y;
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

    fn cascade_gain(freq_norm: f64, stages: usize) -> f32 {
        let mut filter = ComplexOnePole::new(1.0, CUTOFF_NORM, stages);
        let input = complex_tone(freq_norm, 200_000);
        let out: Vec<Complex<f32>> = input.iter().map(|&s| filter.process(s)).collect();
        rms_c(&out[out.len() / 2..])
    }

    const CUTOFF_NORM: f64 = 0.001;

    #[test]
    fn complex_one_pole_matches_the_analytic_single_pole_response() {
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

    #[test]
    fn highpass_takes_the_subaudible_band_out_and_keeps_the_voice_band() {
        let analytic = |f: f64| (f / f.hypot(300.0)).powi(3) as f32;
        for freq_hz in [67.0f64, 88.5, 254.1, 300.0, 1_000.0] {
            let gain = highpass_gain(300.0, freq_hz);
            let want = analytic(freq_hz);
            assert!(
                (gain / want - 1.0).abs() < 0.1,
                "{freq_hz} Hz: gain {gain}, expected {want}"
            );
        }
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

    fn biquad_gain(mut filter: Biquad, freq_hz: f64) -> f32 {
        let rate = 48_000.0;
        let n = (rate * 0.5) as usize;
        let mut x = real_tone(freq_hz / rate, n);
        filter.process(&mut x);
        rms_r(&x[n / 2..]) / FRAC_1_SQRT_2
    }

    #[test]
    fn biquad_corners_are_3_db_down_and_roll_off_at_12_db_per_octave() {
        let rate = 48_000.0;
        let q = std::f64::consts::FRAC_1_SQRT_2;
        for corner in [300.0f64, 3_000.0] {
            let at = biquad_gain(Biquad::lowpass(rate, corner, q), corner);
            assert!(
                (at - FRAC_1_SQRT_2).abs() < 0.03,
                "lowpass {corner} Hz: {at}"
            );
            let octave = biquad_gain(Biquad::lowpass(rate, corner, q), corner * 2.0);
            assert!((0.2..0.28).contains(&octave), "lowpass octave up: {octave}");

            let at = biquad_gain(Biquad::highpass(rate, corner, q), corner);
            assert!(
                (at - FRAC_1_SQRT_2).abs() < 0.03,
                "highpass {corner} Hz: {at}"
            );
            let octave = biquad_gain(Biquad::highpass(rate, corner, q), corner / 2.0);
            assert!(
                (0.2..0.28).contains(&octave),
                "highpass octave down: {octave}"
            );
        }
    }

    #[test]
    fn biquad_notch_removes_its_tone_and_leaves_its_neighbours() {
        let rate = 48_000.0;
        let notch = || Biquad::notch(rate, 1_000.0, 1_000.0 / 60.0);
        assert!(biquad_gain(notch(), 1_000.0) < 0.02);
        assert!(biquad_gain(notch(), 700.0) > 0.9);
        assert!(biquad_gain(notch(), 1_500.0) > 0.9);
    }

    #[test]
    fn biquad_recovers_after_a_non_finite_sample() {
        let mut filter = Biquad::lowpass(48_000.0, 3_000.0, std::f64::consts::FRAC_1_SQRT_2);
        let mut poisoned = vec![0.5f32; 480];
        poisoned[3] = f32::NAN;
        filter.process(&mut poisoned);
        let mut tone = real_tone(1_000.0 / 48_000.0, 24_000);
        filter.process(&mut tone);
        assert!(tone.iter().all(|v| v.is_finite()), "state still poisoned");
        let gain = rms_r(&tone[12_000..]) / FRAC_1_SQRT_2;
        assert!((0.9..1.05).contains(&gain), "post-recovery gain {gain}");
    }

    #[test]
    fn biquad_stays_stable_at_degenerate_settings() {
        for (freq_hz, q) in [(0.0f64, 0.0f64), (48_000.0, 0.0), (-100.0, -5.0)] {
            let mut filter = Biquad::notch(48_000.0, freq_hz, q);
            let mut x = real_tone(1_000.0 / 48_000.0, 48_000);
            filter.process(&mut x);
            assert!(
                x.iter().all(|v| v.is_finite() && v.abs() < 10.0),
                "{freq_hz} Hz at q {q} diverged"
            );
        }
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

    const IQ_RATE: f64 = 2_400_000.0;
    const IQ_CORNER: f64 = 20.0;

    #[test]
    fn iq_dc_blocker_removes_a_complex_offset() {
        let mut blocker = IqDcBlocker::new(IQ_RATE, IQ_CORNER);
        let mut x = vec![Complex::new(0.021, -0.014); 1 << 19];
        blocker.process(&mut x);
        let residue = rms_c(&x[x.len() / 2..]);
        assert!(residue < 1e-4, "dc residue {residue}");
    }

    #[test]
    fn iq_dc_blocker_leaves_a_carrier_one_bin_off_dc_alone() {
        let n = 1 << 19;
        let bin_hz = IQ_RATE / 4_096.0;
        let mut x = complex_tone(bin_hz / IQ_RATE, n);
        let clean = rms_c(&x[n / 2..]);
        IqDcBlocker::new(IQ_RATE, IQ_CORNER).process(&mut x);
        let gain = rms_c(&x[n / 2..]) / clean;
        assert!((0.99..1.01).contains(&gain), "one bin off dc: gain {gain}");
    }

    #[test]
    fn iq_dc_blocker_separates_an_offset_from_a_carrier_riding_on_it() {
        let n = 1 << 19;
        let offset = Complex::new(0.05, 0.03);
        let mut x = complex_tone(50_000.0 / IQ_RATE, n);
        for s in &mut x {
            *s += offset;
        }
        IqDcBlocker::new(IQ_RATE, IQ_CORNER).process(&mut x);
        let tail = &x[n / 2..];
        let mean: Complex<f32> = tail.iter().sum::<Complex<f32>>() / tail.len() as f32;
        assert!(mean.norm() < 1e-3, "offset survived: {}", mean.norm());
        assert!(
            (rms_c(tail) - 1.0).abs() < 0.01,
            "carrier lost: {}",
            rms_c(tail)
        );
    }

    #[test]
    fn iq_dc_blocker_recovers_after_a_non_finite_sample() {
        let mut blocker = IqDcBlocker::new(IQ_RATE, IQ_CORNER);
        let mut poisoned = vec![Complex::new(0.5, 0.5); 4_096];
        poisoned[0] = Complex::new(f32::NAN, 0.0);
        blocker.process(&mut poisoned);
        assert!(
            poisoned
                .iter()
                .all(|v| v.re.is_finite() && v.im.is_finite()),
            "a non-finite sample poisoned the stream"
        );
    }

    #[test]
    fn iq_dc_blocker_resets_its_estimate() {
        let mut blocker = IqDcBlocker::new(IQ_RATE, IQ_CORNER);
        let mut settled = vec![Complex::new(1.0, 0.0); 1 << 19];
        blocker.process(&mut settled);
        blocker.reset();
        let mut fresh = vec![Complex::new(1.0, 0.0); 8];
        blocker.process(&mut fresh);
        assert!(
            fresh[0].norm() > 0.99,
            "a reset blocker kept its old estimate"
        );
    }
}
