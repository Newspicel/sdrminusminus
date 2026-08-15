use std::f64::consts::{FRAC_PI_2, PI, TAU};

use num_complex::Complex;

#[derive(Clone, Debug)]
pub struct LoopFilter {
    alpha: f64,
    beta: f64,
    freq: f64,
    limit: f64,
}

impl LoopFilter {
    #[must_use]
    pub fn new(loop_bw: f64, damping: f64, freq_limit_norm: f64) -> Self {
        assert!(
            loop_bw > 0.0 && damping > 0.0 && freq_limit_norm >= 0.0,
            "loop filter parameters must be positive"
        );
        let wn = loop_bw;
        let d = 1.0 + 2.0 * damping * wn + wn * wn;
        Self {
            alpha: 4.0 * damping * wn / d,
            beta: 4.0 * wn * wn / d,
            freq: 0.0,
            limit: TAU * freq_limit_norm,
        }
    }

    #[must_use]
    pub fn advance(&mut self, error: f64) -> f64 {
        let error = if error.is_finite() { error } else { 0.0 };
        self.freq = (self.freq + self.beta * error).clamp(-self.limit, self.limit);
        self.freq + self.alpha * error
    }

    #[must_use]
    pub fn freq_norm(&self) -> f64 {
        self.freq / TAU
    }

    pub fn reset(&mut self, freq_norm: f64) {
        self.freq = (TAU * freq_norm).clamp(-self.limit, self.limit);
    }
}

#[derive(Clone, Debug)]
pub struct Pll {
    filter: LoopFilter,
    lock: LockDetector,
    center: f64,
    phase: f64,
    inc: f64,
}

impl Pll {
    #[must_use]
    pub fn new(loop_bw: f64, damping: f64, center_norm: f64, range_norm: f64) -> Self {
        Self {
            filter: LoopFilter::new(loop_bw, damping, range_norm),
            lock: LockDetector::new(loop_bw, PI),
            center: TAU * center_norm,
            phase: 0.0,
            inc: 0.0,
        }
    }

    #[must_use]
    pub fn process(&mut self, sample: Complex<f32>) -> Complex<f32> {
        self.phase = step_phase(self.phase, self.inc);
        let reference = Complex::from_polar(1.0, self.phase);
        let error = phase_error(to_c64(sample) * reference.conj());
        self.lock.update(error);
        self.inc = wrap_pi(self.center + self.filter.advance(error.unwrap_or(0.0)));
        to_c32(reference)
    }

    #[must_use]
    pub fn harmonic(&self, n: f64) -> Complex<f32> {
        to_c32(Complex::from_polar(1.0, wrap_pi(n * self.phase)))
    }

    #[must_use]
    pub fn freq_norm(&self) -> f64 {
        self.center / TAU + self.filter.freq_norm()
    }

    #[must_use]
    pub fn increment_norm(&self) -> f64 {
        self.inc / TAU
    }

    #[must_use]
    pub fn lock(&self) -> f32 {
        self.lock.value()
    }
}

#[derive(Clone, Debug)]
pub struct Costas {
    filter: LoopFilter,
    lock: LockDetector,
    center: f64,
    phase: f64,
    inc: f64,
}

impl Costas {
    #[must_use]
    pub fn new(loop_bw: f64, damping: f64, center_norm: f64, range_norm: f64) -> Self {
        Self {
            filter: LoopFilter::new(loop_bw, damping, range_norm),
            lock: LockDetector::new(loop_bw, FRAC_PI_2),
            center: TAU * center_norm,
            phase: 0.0,
            inc: 0.0,
        }
    }

    #[must_use]
    pub fn process(&mut self, sample: Complex<f32>) -> Complex<f32> {
        self.phase = step_phase(self.phase, self.inc);
        let reference = Complex::from_polar(1.0, self.phase);
        let derotated = to_c64(sample) * reference.conj();
        let error = bpsk_error(derotated);
        self.lock.update(error);
        self.inc = wrap_pi(self.center + self.filter.advance(error.unwrap_or(0.0)));
        to_c32(derotated)
    }

    #[must_use]
    pub fn freq_norm(&self) -> f64 {
        self.center / TAU + self.filter.freq_norm()
    }

    #[must_use]
    pub fn lock(&self) -> f32 {
        self.lock.value()
    }
}

#[derive(Clone, Debug)]
struct LockDetector {
    value: f64,
    coeff: f64,
    range: f64,
}

impl LockDetector {
    fn new(loop_bw: f64, range: f64) -> Self {
        Self {
            value: 0.0,
            coeff: loop_bw.clamp(1e-4, 0.5),
            range,
        }
    }

    fn update(&mut self, error: Option<f64>) {
        let quality = error.map_or(0.0, |e| 1.0 - 2.0 * e.abs() / self.range);
        self.value += self.coeff * (quality - self.value);
    }

    fn value(&self) -> f32 {
        self.value.clamp(0.0, 1.0) as f32
    }
}

fn phase_error(derotated: Complex<f64>) -> Option<f64> {
    (derotated.is_finite() && derotated.norm_sqr() > 0.0).then(|| derotated.arg())
}

fn bpsk_error(derotated: Complex<f64>) -> Option<f64> {
    let symbol = if derotated.re < 0.0 { -1.0 } else { 1.0 };
    phase_error(derotated * symbol)
}

fn step_phase(phase: f64, inc: f64) -> f64 {
    let next = phase + inc;
    if next >= PI {
        next - TAU
    } else if next < -PI {
        next + TAU
    } else {
        next
    }
}

fn wrap_pi(x: f64) -> f64 {
    (x + PI).rem_euclid(TAU) - PI
}

fn to_c64(x: Complex<f32>) -> Complex<f64> {
    Complex::new(f64::from(x.re), f64::from(x.im))
}

fn to_c32(x: Complex<f64>) -> Complex<f32> {
    Complex::new(x.re as f32, x.im as f32)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_1_SQRT_2;

    use super::*;
    use crate::testutil::{XorShift32, complex_tone};

    const DAMPING: f64 = FRAC_1_SQRT_2;
    const PILOT: f64 = 19_000.0 / 240_000.0;
    const RANGE: f64 = 300.0 / 240_000.0;
    const PILOT_BW: f64 = 0.005;

    fn pilot_pll() -> Pll {
        Pll::new(PILOT_BW, DAMPING, PILOT, RANGE)
    }

    #[test]
    fn loop_filter_integrator_clamps_and_resets() {
        let limit = 0.002;
        let mut f = LoopFilter::new(0.01, DAMPING, limit);
        for _ in 0..10_000 {
            let _ = f.advance(1.0);
        }
        assert!((f.freq_norm() - limit).abs() < 1e-12, "{}", f.freq_norm());
        for _ in 0..10_000 {
            let _ = f.advance(-1.0);
        }
        assert!((f.freq_norm() + limit).abs() < 1e-12, "{}", f.freq_norm());

        f.reset(0.0);
        assert!(f.freq_norm().abs() < f64::EPSILON);
        assert!(f.advance(0.0).abs() < f64::EPSILON, "zero error must coast");
        f.reset(10.0 * limit);
        assert!(
            (f.freq_norm() - limit).abs() < 1e-12,
            "reset ignored the limit: {}",
            f.freq_norm()
        );
    }

    #[test]
    fn loop_filter_ignores_a_non_finite_error() {
        let mut f = LoopFilter::new(0.01, DAMPING, 0.01);
        for _ in 0..100 {
            let _ = f.advance(0.5);
        }
        let before = f.freq_norm();
        assert!(f.advance(f64::NAN).is_finite(), "NaN reached the output");
        assert!((f.freq_norm() - before).abs() < f64::EPSILON);
    }

    #[test]
    fn pll_locks_to_a_tone_inside_the_pull_in_range() {
        let tone = PILOT + 120.0 / 240_000.0;
        let mut pll = pilot_pll();
        for x in complex_tone(tone, 40_000) {
            let _ = pll.process(x);
        }
        let err = (pll.freq_norm() - tone).abs();
        assert!(
            err < 0.01 * (tone - PILOT),
            "settled at {} instead of {tone}",
            pll.freq_norm()
        );
        assert!(pll.lock() > 0.5, "lock {}", pll.lock());
    }

    #[test]
    fn pll_refuses_to_follow_a_tone_outside_the_range() {
        let tone = PILOT + 3.0 * RANGE;
        let mut pll = pilot_pll();
        for (n, x) in complex_tone(tone, 40_000).into_iter().enumerate() {
            let _ = pll.process(x);
            let offset = pll.freq_norm() - PILOT;
            assert!(offset.abs() <= RANGE + 1e-12, "escaped by {offset} at {n}");
        }
        assert!(
            (pll.freq_norm() - (PILOT + RANGE)).abs() < 1e-9,
            "not clamped at the edge: {}",
            pll.freq_norm()
        );
    }

    #[test]
    fn pll_harmonic_rotates_at_a_multiple_of_the_loop_frequency() {
        let tone = PILOT + 50.0 / 240_000.0;
        let signal = complex_tone(tone, 60_000);
        let mut pll = pilot_pll();
        for &x in &signal[..40_000] {
            let _ = pll.process(x);
        }
        assert!(pll.lock() > 0.5, "lock {}", pll.lock());

        let mut sum = Complex::new(0.0f64, 0.0);
        for (k, &x) in signal[40_000..].iter().enumerate() {
            let _ = pll.process(x);
            let reference = Complex::from_polar(1.0, TAU * 3.0 * tone * (40_000 + k) as f64);
            sum += to_c64(pll.harmonic(3.0)) * reference.conj();
        }
        let mean = sum / (signal.len() - 40_000) as f64;
        assert!(
            (mean.norm() - 1.0).abs() < 1e-2,
            "harmonic drifted: mean norm {}",
            mean.norm()
        );
    }

    #[test]
    fn costas_recovers_bpsk_symbols() {
        const SPS: usize = 8;
        const SYMBOLS: usize = 2_000;
        const OFFSET: f64 = 0.001;

        let mut rng = XorShift32(0x1234_5678);
        let symbols: Vec<f64> = (0..SYMBOLS)
            .map(|_| if rng.next_f32() < 0.0 { -1.0 } else { 1.0 })
            .collect();
        let mut costas = Costas::new(0.01, DAMPING, 0.0, 0.01);
        let out: Vec<Complex<f32>> = (0..SYMBOLS * SPS)
            .map(|n| {
                let carrier = Complex::from_polar(1.0, TAU * OFFSET * n as f64 + 0.7);
                costas.process(to_c32(carrier * symbols[n / SPS]))
            })
            .collect();

        let settled = SYMBOLS * SPS / 2;
        let correlation: f64 = out[settled..]
            .iter()
            .enumerate()
            .map(|(k, y)| f64::from(y.re) * symbols[(settled + k) / SPS])
            .sum();
        let polarity = if correlation < 0.0 { -1.0 } else { 1.0 };
        for (k, y) in out[settled..].iter().enumerate() {
            let n = settled + k;
            let expected = symbols[n / SPS] * polarity;
            assert!(
                f64::from(y.re) * expected > 0.5,
                "symbol {expected} lost at {n}: {y}"
            );
            assert!(y.im.abs() < 0.25, "quadrature leakage at {n}: {y}");
        }
        assert!(costas.lock() > 0.5, "lock {}", costas.lock());
        assert!(
            (costas.freq_norm() - OFFSET).abs() < 0.01 * OFFSET,
            "freq {}",
            costas.freq_norm()
        );
    }

    #[test]
    fn pll_recovers_after_a_non_finite_sample() {
        let tone = PILOT + 60.0 / 240_000.0;
        let mut pll = pilot_pll();
        for (n, x) in complex_tone(tone, 40_000).into_iter().enumerate() {
            let x = if n == 100 {
                Complex::new(f32::NAN, 0.0)
            } else {
                x
            };
            assert!(pll.process(x).is_finite(), "reference poisoned at {n}");
        }
        assert!(
            (pll.freq_norm() - tone).abs() < 0.01 * (tone - PILOT),
            "loop stuck at {}",
            pll.freq_norm()
        );
        assert!(pll.lock() > 0.5, "lock {}", pll.lock());
    }

    #[test]
    fn costas_recovers_after_a_non_finite_sample() {
        const OFFSET: f64 = 0.001;
        let mut costas = Costas::new(0.01, DAMPING, 0.0, 0.01);
        for (n, x) in complex_tone(OFFSET, 20_000).into_iter().enumerate() {
            let poisoned = n == 100;
            let x = if poisoned {
                Complex::new(f32::INFINITY, f32::NAN)
            } else {
                x
            };
            let y = costas.process(x);
            assert!(poisoned || y.is_finite(), "output poisoned at {n}");
        }
        assert!(
            (costas.freq_norm() - OFFSET).abs() < 0.01 * OFFSET,
            "loop stuck at {}",
            costas.freq_norm()
        );
        assert!(costas.lock() > 0.5, "lock {}", costas.lock());
    }
}
