//! Carrier recovery loops (PLAN §7): a shared second-order loop filter, a tracking PLL for a
//! residual carrier or pilot tone, and a Costas loop for BPSK. Every frequency here is
//! normalised to the sample rate (cycles/sample), so the loops are rate-agnostic — the caller
//! divides by `fs` once. No allocation in `process`.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use num_complex::Complex;

/// Second-order (proportional + integral) loop filter shared by the loops below.
/// `loop_bw` and the frequency limits are normalised to the sample rate (cycles/sample).
#[derive(Clone, Debug)]
pub struct LoopFilter {
    alpha: f64,
    beta: f64,
    /// Integrator state, in rad/sample.
    freq: f64,
    /// Symmetric integrator clamp, in rad/sample.
    limit: f64,
}

impl LoopFilter {
    /// Gains come from the standard second-order form with natural frequency `wn = loop_bw` and
    /// damping `zeta`: `alpha = 4·zeta·wn/d`, `beta = 4·wn²/d`, `d = 1 + 2·zeta·wn + wn²`
    /// (Rice, *Digital Communications: A Discrete-Time Approach*, App. C; the same expression
    /// GNU Radio's `control_loop::update_gains` uses).
    ///
    /// A loop with a nominal frequency keeps that offset outside the filter and lets the filter
    /// track the signed deviation, which is what `freq_limit_norm` bounds.
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

    /// Advance with a phase error in radians; returns the new phase increment.
    #[must_use]
    pub fn advance(&mut self, error: f64) -> f64 {
        // A non-finite error latches the integrator forever — same healing argument as the
        // recursions in `iir.rs`, except here dropping the update costs a single sample.
        let error = if error.is_finite() { error } else { 0.0 };
        // The clamp is the reason callers pass a frequency limit at all: without it a noise
        // burst integrates the loop off the signal and it never pulls back in.
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

/// Tracking PLL for a residual carrier or pilot tone (RDS 19 kHz pilot, AM carrier).
#[derive(Clone, Debug)]
pub struct Pll {
    filter: LoopFilter,
    lock: LockDetector,
    /// Nominal frequency, in rad/sample.
    center: f64,
    /// Phase of the reference returned by the most recent `process`, wrapped to `[-π, π)`.
    phase: f64,
    /// Increment computed from the previous sample, applied at the top of the next `process`
    /// so that `phase` — and therefore `harmonic` — always describes the reference just
    /// returned rather than the next one.
    inc: f64,
}

impl Pll {
    /// `center_norm` is the nominal frequency, `range_norm` the ± pull-in limit around it.
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

    /// Advance one sample; returns the loop's current reference phasor (unit magnitude).
    #[must_use]
    pub fn process(&mut self, sample: Complex<f32>) -> Complex<f32> {
        self.phase = step_phase(self.phase, self.inc);
        let reference = Complex::from_polar(1.0, self.phase);
        let error = phase_error(to_c64(sample) * reference.conj());
        self.lock.update(error);
        self.inc = wrap_pi(self.center + self.filter.advance(error.unwrap_or(0.0)));
        to_c32(reference)
    }

    /// Phasor at `n` times the loop frequency — the pilot-to-subcarrier multiplier RDS/stereo
    /// need. Only integer `n` is well defined; a fractional harmonic depends on which 2π branch
    /// the loop phase happens to sit in.
    #[must_use]
    pub fn harmonic(&self, n: f64) -> Complex<f32> {
        to_c32(Complex::from_polar(1.0, wrap_pi(n * self.phase)))
    }

    #[must_use]
    pub fn freq_norm(&self) -> f64 {
        self.center / TAU + self.filter.freq_norm()
    }

    /// Smoothed |error| based lock estimate in 0..=1; > 0.5 means locked.
    #[must_use]
    pub fn lock(&self) -> f32 {
        self.lock.value()
    }
}

/// Costas loop for BPSK — the RDS 57 kHz subcarrier demodulator.
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
            // The decision-directed detector folds the error into ±π/2, so a random phase
            // averages to zero quality only against that reduced span.
            lock: LockDetector::new(loop_bw, FRAC_PI_2),
            center: TAU * center_norm,
            phase: 0.0,
            inc: 0.0,
        }
    }

    /// Advance one sample; returns the de-rotated sample (data on the real axis).
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

    /// Smoothed |error| based lock estimate in 0..=1; > 0.5 means locked.
    #[must_use]
    pub fn lock(&self) -> f32 {
        self.lock.value()
    }
}

/// Smoothed lock quality: 1 when the detector's phase error sits at zero, 0 when the error is
/// spread uniformly over `range` — i.e. the loop is riding noise, not a carrier.
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
            // Averaging over roughly one loop time constant makes the estimate follow the loop
            // instead of lagging behind it; the bounds keep pathological loop_bw usable.
            coeff: loop_bw.clamp(1e-4, 0.5),
            range,
        }
    }

    /// `None` means the sample carried no phase at all (dead or non-finite input), which counts
    /// as unlocked — not as a perfect zero-error alignment.
    fn update(&mut self, error: Option<f64>) {
        let quality = error.map_or(0.0, |e| 1.0 - 2.0 * e.abs() / self.range);
        self.value += self.coeff * (quality - self.value);
    }

    fn value(&self) -> f32 {
        self.value.clamp(0.0, 1.0) as f32
    }
}

/// Phase of a de-rotated sample, or `None` when it carries none.
fn phase_error(derotated: Complex<f64>) -> Option<f64> {
    (derotated.is_finite() && derotated.norm_sqr() > 0.0).then(|| derotated.arg())
}

/// Decision-directed BPSK detector: strip the ±1 symbol before measuring the phase, so the
/// modulation cannot steer the loop and the error stays inside ±π/2.
fn bpsk_error(derotated: Complex<f64>) -> Option<f64> {
    let symbol = if derotated.re < 0.0 { -1.0 } else { 1.0 };
    phase_error(derotated * symbol)
}

/// `inc` is pre-wrapped into `[-π, π)`, so a single correction always bounds the accumulator
/// and f64 precision cannot erode over a long run.
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

/// Reduce a per-sample phase increment into `[-π, π)`. Exact for a sampled phasor: `e^(jθn)` is
/// periodic in 2π per integer sample, so the wrapped increment produces the same sequence
/// (correctly aliased) for any requested frequency.
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
    /// A 19 kHz stereo pilot sampled at 240 kHz.
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
        // Pinned at the near edge of the range, nowhere near the tone.
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

        // Locked, the third harmonic must hold a constant phase against a directly generated
        // 3·tone phasor: every sample's relative phasor then adds up to a unit-norm mean.
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

        // The loop is blind to a 180° flip, so fix the polarity from the settled run and
        // require every symbol to agree with it.
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
