//! Single-pole audio IIR helpers (PLAN §7): FM deemphasis and DC removal.

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

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_1_SQRT_2;

    use super::*;
    use crate::testutil::{real_tone, rms_r};

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
