//! The one fractional-time interpolator behind every timing impairment (static offset,
//! sample-clock ppm, jitter). Windowed sinc rather than the Farrow structure `sdrmm-dsp` uses
//! in its symbol sync: this is a test instrument, so interpolation error must sit far below
//! every tolerance being measured (0.01 sample on delay recovery, 5% on jitter RMS), and a
//! 32-tap Blackman-windowed sinc buys that accuracy for a per-sample cost no one here counts.
//! A shared kernel also means the three timing axes cannot disagree about what "a fraction of
//! a sample" is.

use std::f64::consts::PI;

use num_complex::Complex;

/// Taps on each side of the interpolation instant. At 16, the Blackman-windowed kernel's
/// stopband is below −70 dB and its passband is flat to ~1e-4 over the band any shaped
/// waveform in the harness occupies — interpolation artefacts land two orders of magnitude
/// under the tightest calibration tolerance in this module.
const HALF: i64 = 16;

/// Band-limited value of `x` at continuous time `t`, in sample units. Beyond either edge the
/// waveform is taken as zero, which is what a finite capture is; callers measuring near an
/// edge skip [`edge_guard`] samples instead of trusting them.
pub(crate) fn interp(x: &[Complex<f32>], t: f64) -> Complex<f32> {
    let base = t.floor() as i64;
    let mut re = 0.0f64;
    let mut im = 0.0f64;
    for m in (base - HALF + 1)..=(base + HALF) {
        let Some(s) = usize::try_from(m).ok().and_then(|i| x.get(i)) else {
            continue;
        };
        let u = m as f64 - t;
        let w = sinc(u) * blackman(u);
        re += f64::from(s.re) * w;
        im += f64::from(s.im) * w;
    }
    Complex::new(re as f32, im as f32)
}

/// Samples at each end of an interpolated waveform still inside the kernel's reach of the
/// zero-padded edge. Measurements exclude them; the impairments keep them so length
/// bookkeeping stays trivial.
#[cfg(test)]
pub(crate) fn edge_guard() -> usize {
    HALF as usize
}

fn sinc(u: f64) -> f64 {
    if u.abs() < 1e-12 {
        return 1.0;
    }
    let p = PI * u;
    p.sin() / p
}

/// Centered Blackman window over `[-HALF, HALF]`; exactly zero at the ends, so the kernel has
/// no truncation discontinuity to leak wideband error.
fn blackman(u: f64) -> f64 {
    let r = u / HALF as f64;
    0.42 + 0.5 * (PI * r).cos() + 0.08 * (2.0 * PI * r).cos()
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::{edge_guard, interp};
    use crate::ber::impair::testutil::tone;

    /// The accuracy claim the timing calibrations lean on: on an in-band tone, the
    /// interpolator's value at an arbitrary fractional instant matches the analytic value to
    /// ~1e-4 — far below the 0.01-sample and 5% tolerances measured through it.
    #[test]
    fn interpolates_a_tone_to_its_analytic_value() {
        let f = 0.21;
        let x = tone(f, 512);
        let guard = edge_guard() as f64;
        for k in 0..200 {
            let t = guard + k as f64 * (512.0 - 2.0 * guard) / 200.0;
            let got = interp(&x, t);
            let phase = 2.0 * std::f64::consts::PI * f * t;
            let want = Complex::new(phase.cos(), phase.sin());
            let err = ((f64::from(got.re) - want.re).powi(2)
                + (f64::from(got.im) - want.im).powi(2))
            .sqrt();
            assert!(err < 3e-4, "t={t}: err {err}");
        }
    }

    /// At integer instants the kernel must collapse to the identity — anything else would put
    /// a systematic bias under every timing measurement.
    #[test]
    fn integer_instants_reproduce_the_samples() {
        let x = tone(0.13, 256);
        for n in edge_guard()..256 - edge_guard() {
            let got = interp(&x, n as f64);
            assert!((got - x[n]).norm() < 2e-6, "n={n}");
        }
    }
}
