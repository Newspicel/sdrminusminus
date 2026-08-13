//! The filters an analog receiver is built out of, and the one design routine the asymmetric
//! ones need.
//!
//! Three of them, and each earns its place by being *part of the measurement* rather than
//! decoration:
//!
//! - **The predetection band** ([`BandFilter`]). Every closed form in [`theory`] reads the
//!   noise in a stated bandwidth, and an analog detector is nonlinear: a magnitude, an argument
//!   or a product folds whatever noise reaches it down into the audio band, so noise outside
//!   the transmitted band is not merely harmless, it is *counted twice*. The IF filter a real
//!   receiver has is what makes the oracle apply, which is why the engines carry one instead of
//!   assuming their input was cleaned somewhere else.
//! - **The Hilbert transformer** ([`design_hilbert`]). The quadrature half of a single-sideband
//!   exciter, and deliberately the *other* method from the receiver's band filter, so that
//!   neither can hide the other's error (the arrangement `channels::ssb` already used and this
//!   module inherits).
//! - **The vestigial slope** ([`design_vestigial`]). A vestigial-sideband filter is not a band
//!   filter with one edge moved: what makes VSB detectable without distortion is that its two
//!   skirts *add to a constant* across the carrier, so the sideband energy a receiver loses on
//!   one side it regains on the other. That is a complementary-symmetry condition on the
//!   response, checked by test here rather than asserted in prose.
//!
//! Everything is stated in cycles per sample, the crate's rate-free convention (crate root):
//! an entry's physical numbers follow from its own sample rate, and no filter here knows one.

use std::f64::consts::PI;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, FirC, design_lowpass};

/// Largest band-filter length the engines will design. A guard on a parameter, not a tuning
/// knob: taps are `O(n)` per sample on the hot path and a four-figure filter is a mistake being
/// made somewhere upstream, not a design choice.
pub const MAX_TAPS: usize = 1_023;

/// A complex band filter ahead of a detector — the receiver's IF selectivity, stated by its
/// two edges in cycles per sample so an asymmetric band (single sideband, vestigial sideband)
/// is expressible without a second type.
///
/// `low` may be negative and `high` must exceed it; a band symmetric about the carrier is the
/// ordinary double-sideband case and is built as a real-tap filter, which is half the
/// multiplies of the general one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandFilter {
    pub low: f64,
    pub high: f64,
    pub taps: usize,
}

impl BandFilter {
    /// A band of half-width `half_width` about the carrier — the double-sideband case.
    #[must_use]
    pub fn symmetric(half_width: f64, taps: usize) -> Self {
        Self {
            low: -half_width,
            high: half_width,
            taps,
        }
    }

    /// Builds the runner. Edges are clamped into `(-½, ½)` and the tap count into
    /// `3..=MAX_TAPS` rather than rejected: a band filter is selectivity, and a receiver whose
    /// configuration asks for more than Nyquist holds should pass everything it has, not fail.
    #[must_use]
    pub fn build(&self) -> Band {
        // The lower edge is held a guard width below the upper one, so a band asked for *at*
        // Nyquist still leaves `high`'s clamp a range to work in. Without it a `low` of 0.4999
        // hands `clamp` a floor above its ceiling, which panics.
        const EDGE: f64 = 0.4999;
        const GUARD: f64 = 1e-6;
        let low = self.low.clamp(-EDGE, EDGE - GUARD);
        let high = self.high.clamp((low + GUARD).min(EDGE), EDGE);
        let taps = self.taps.clamp(3, MAX_TAPS) | 1;
        let half_width = 0.5 * (high - low);
        let centre = 0.5 * (high + low);
        let prototype = design_lowpass(taps, half_width.min(0.4999));
        if centre.abs() < 1e-12 {
            Band::Real(Decimator::new(&prototype, 1))
        } else {
            Band::Complex(FirC::from_lowpass(&prototype, centre))
        }
    }

    /// Group delay in samples — what an engine must skip before its output means anything, and
    /// what a measurement window must be offset by.
    #[must_use]
    pub fn delay(&self) -> usize {
        (self.taps.clamp(3, MAX_TAPS) | 1) / 2
    }
}

/// A built [`BandFilter`]. Two variants for one reason: a band centred on the carrier has real
/// taps, and running it as a complex filter would double the arithmetic on the hot path of
/// every double-sideband entry in the catalog.
pub enum Band {
    Real(Decimator),
    Complex(FirC),
}

impl Band {
    /// Replaces `out` with one filtered sample per input sample.
    pub fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        match self {
            Self::Real(f) => f.process(iq, out),
            Self::Complex(f) => f.process(iq, out),
        }
    }
}

/// A whole-sample delay line — the in-phase half of a Hilbert pair, whose only job is to lose
/// exactly as many samples as the quadrature filter does. Written rather than approximated with
/// a filter of its own, because "exactly" is the requirement: a delay off by one sample turns
/// the sideband rejection of a phasing exciter into a frequency-dependent leak.
pub struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    #[must_use]
    pub fn new(samples: usize) -> Self {
        Self {
            buf: vec![0.0; samples],
            pos: 0,
        }
    }

    /// Replaces `out` with `x` delayed, one sample per input sample. A zero-sample delay is a
    /// pass-through, not a one-sample delay.
    pub fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        out.clear();
        if self.buf.is_empty() {
            out.extend_from_slice(x);
            return;
        }
        for &v in x {
            out.push(self.buf[self.pos]);
            self.buf[self.pos] = v;
            self.pos = (self.pos + 1) % self.buf.len();
        }
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

/// A complex FIR from an arbitrary frequency response — the designer the asymmetric filters
/// need, since a real prototype modulated to a centre frequency can only produce a band whose
/// two skirts are mirror images.
///
/// The impulse response is the inverse transform of `response` evaluated by midpoint rule over
/// a grid `oversample` times finer than the tap count, then symmetric-Blackman windowed. A
/// quadrature rather than an FFT because this runs once at construction and the grid density
/// is then free: at 16× the response's own detail the truncation error is far below the
/// window's own sidelobes, and the arithmetic reads as the definition it is.
///
/// # Panics
/// If `taps` is below 3 or above [`MAX_TAPS`].
#[must_use]
pub fn design_from_response(
    taps: usize,
    oversample: usize,
    response: impl Fn(f64) -> Complex<f64>,
) -> Vec<Complex<f32>> {
    assert!(
        (3..=MAX_TAPS).contains(&taps),
        "band filters run 3..={MAX_TAPS} taps, got {taps}"
    );
    let taps = taps | 1;
    let centre = (taps - 1) as f64 / 2.0;
    let grid = (taps * oversample.max(1)).next_power_of_two();
    let window = blackman(taps);
    (0..taps)
        .map(|k| {
            let offset = k as f64 - centre;
            let mut acc = Complex::new(0.0, 0.0);
            for g in 0..grid {
                // Midpoint of the g-th cell of [-½, ½): no endpoint is evaluated twice, and a
                // response with a jump at ±½ contributes its two sides symmetrically.
                let f = (g as f64 + 0.5) / grid as f64 - 0.5;
                acc += response(f) * Complex::from_polar(1.0, 2.0 * PI * f * offset);
            }
            let h = acc / grid as f64 * window[k];
            Complex::new(h.re as f32, h.im as f32)
        })
        .collect()
}

/// Symmetric Blackman — the window `sdrmm_dsp`'s own filter designs use, restated here because
/// the crate exports only the *periodic* Hann meant for spectral analysis, and a filter
/// designed against a periodic window is asymmetric by one sample.
fn blackman(taps: usize) -> Vec<f64> {
    let n = (taps - 1) as f64;
    (0..taps)
        .map(|k| {
            let x = 2.0 * PI * k as f64 / n;
            0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
        })
        .collect()
}

/// A Hilbert transformer: `taps` odd, unit magnitude and −90° across the passband, so that
/// `x + j·H{x}` is the analytic signal of a real `x` delayed by `taps/2`.
///
/// The ideal response `h[n] = 2/(πn)` for odd `n` and 0 for even is windowed rather than
/// truncated, and its even taps are *exactly* zero — which is what makes the in-phase path a
/// pure delay of `taps/2` samples with no filter of its own to match.
///
/// # Panics
/// If `taps` is even, below 3, or above [`MAX_TAPS`].
#[must_use]
pub fn design_hilbert(taps: usize) -> Vec<f32> {
    assert!(
        (3..=MAX_TAPS).contains(&taps) && !taps.is_multiple_of(2),
        "a Hilbert transformer needs an odd 3..={MAX_TAPS} taps, got {taps}"
    );
    let centre = (taps - 1) as isize / 2;
    let window = blackman(taps);
    (0..taps)
        .map(|k| {
            let n = k as isize - centre;
            if n % 2 == 0 {
                0.0
            } else {
                (2.0 / (PI * n as f64) * window[k]) as f32
            }
        })
        .collect()
}

/// The vestigial-sideband response: the upper sideband in full out to `upper`, the lower one
/// carved away over a raised-sine slope of half-width `vestige` about the carrier, all in
/// cycles per sample.
///
/// The slope's shape is the whole point. `½·(1 + sin(π f / 2v))` satisfies
/// `H(−f) + H(+f) = 1` across the vestige, so the two sidebands of every message component
/// inside it add back to one — a synchronous detector then recovers the message undistorted
/// from a spectrum that carries barely more than half of it. A plain band edge would not, and
/// the difference is a low-frequency droop no equaliser downstream can distinguish from the
/// programme material.
///
/// # Panics
/// If the vestige is not positive and smaller than `upper`, or `taps` is out of range.
#[must_use]
pub fn design_vestigial(taps: usize, vestige: f64, upper: f64) -> Vec<Complex<f32>> {
    assert!(
        vestige > 0.0 && vestige < upper && upper < 0.5,
        "need 0 < vestige < upper < ½, got vestige {vestige}, upper {upper}"
    );
    design_from_response(taps, 16, |f| {
        let gain = if f < -vestige || f > upper {
            0.0
        } else if f <= vestige {
            0.5 * (1.0 + (PI * f / (2.0 * vestige)).sin())
        } else {
            1.0
        };
        Complex::new(gain, 0.0)
    })
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;

    /// Response of `taps` at normalised frequency `f`, by direct evaluation of the DTFT —
    /// small, exact, and free of any FFT convention to get wrong.
    fn response(taps: &[Complex<f32>], f: f64) -> Complex<f64> {
        taps.iter()
            .enumerate()
            .map(|(k, &t)| {
                Complex::new(f64::from(t.re), f64::from(t.im))
                    * Complex::from_polar(1.0, -2.0 * PI * f * k as f64)
            })
            .sum()
    }

    fn real_response(taps: &[f32], f: f64) -> Complex<f64> {
        let complex: Vec<Complex<f32>> = taps.iter().map(|&t| Complex::new(t, 0.0)).collect();
        response(&complex, f)
    }

    /// A symmetric band is built with real taps and an offset one with complex taps — the
    /// arithmetic saving that variant exists for.
    #[test]
    fn a_symmetric_band_is_a_real_filter_and_an_offset_one_is_not() {
        assert!(matches!(
            BandFilter::symmetric(0.1, 65).build(),
            Band::Real(_)
        ));
        assert!(matches!(
            BandFilter {
                low: 0.01,
                high: 0.1,
                taps: 65
            }
            .build(),
            Band::Complex(_)
        ));
    }

    /// A band asked for past Nyquist is clamped, not rejected and not a panic — including the
    /// degenerate case where both edges land on the same side of the clamp.
    #[test]
    fn a_band_at_the_nyquist_edge_still_builds() {
        for (low, high) in [(0.4999, 0.5), (0.6, 0.7), (-0.7, -0.6), (0.5, 0.4)] {
            let mut band = BandFilter {
                low,
                high,
                taps: 33,
            }
            .build();
            let mut out = Vec::new();
            band.process(&[Complex::new(1.0, 0.0); 64], &mut out);
            assert_eq!(out.len(), 64, "band {low}..{high} produced no output");
            assert!(
                out.iter().all(|s| s.re.is_finite() && s.im.is_finite()),
                "band {low}..{high} produced a non-finite sample"
            );
        }
    }

    /// The band passes what it says it passes and stops what it says it stops, measured on
    /// complex tones through the built runner rather than on the design.
    #[test]
    fn an_offset_band_passes_its_own_side_only() {
        let mut band = BandFilter {
            low: 0.02,
            high: 0.12,
            taps: 129,
        }
        .build();
        let level = |f: f64| {
            let x: Vec<Complex<f32>> = (0..4096)
                .map(|n| Complex::from_polar(1.0, (2.0 * PI * f * n as f64) as f32))
                .collect();
            let mut out = Vec::new();
            let mut band = BandFilter {
                low: 0.02,
                high: 0.12,
                taps: 129,
            }
            .build();
            band.process(&x, &mut out);
            let tail = &out[512..];
            (tail.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>() / tail.len() as f64).sqrt()
        };
        let mut sink = Vec::new();
        band.process(&[Complex::new(0.0, 0.0); 8], &mut sink);
        assert!((level(0.07) - 1.0).abs() < 0.02, "passband {}", level(0.07));
        assert!(level(-0.07) < 0.01, "image {}", level(-0.07));
        assert!(level(0.25) < 0.01, "stopband {}", level(0.25));
    }

    /// The transformer's defining property, read straight off its response: unit magnitude and
    /// a quarter turn, one way below the carrier and the other above.
    #[test]
    fn the_hilbert_response_is_a_quarter_turn_each_way() {
        let taps = design_hilbert(129);
        for &f in &[0.05, 0.1, 0.2, 0.4] {
            let h = real_response(&taps, f);
            assert!(
                (h.norm() - 1.0).abs() < 0.02,
                "magnitude at {f}: {}",
                h.norm()
            );
            // The linear-phase delay of taps/2 is removed before the quadrature is read.
            let delay = Complex::from_polar(1.0, 2.0 * PI * f * ((taps.len() - 1) / 2) as f64);
            let turn = (h * delay).arg();
            assert!(
                (turn + std::f64::consts::FRAC_PI_2).abs() < 0.02,
                "phase at {f}: {turn}"
            );
            let mirror = real_response(&taps, -f) * delay.conj();
            assert!(
                (mirror.arg() - std::f64::consts::FRAC_PI_2).abs() < 0.02,
                "phase at -{f}: {}",
                mirror.arg()
            );
        }
        // Every even tap is exactly zero, which is what makes the in-phase path a bare delay.
        for (k, &t) in taps.iter().enumerate() {
            if (k as isize - 64) % 2 == 0 {
                assert_eq!(t, 0.0, "tap {k}");
            }
        }
    }

    /// The vestigial slope's reason for existing: skirts that add to one across the carrier.
    /// Asserted on the *designed* filter, not on the ideal response it was drawn from, since
    /// windowing is exactly what could break it.
    #[test]
    fn the_vestigial_skirts_add_to_a_constant() {
        let taps = design_vestigial(257, 0.01, 0.1);
        let delay = |f: f64| Complex::from_polar(1.0, 2.0 * PI * f * 128.0);
        let at = |f: f64| (response(&taps, f) * delay(f)).re;
        assert!((at(0.0) - 0.5).abs() < 0.01, "carrier {}", at(0.0));
        for &f in &[0.002, 0.004, 0.006, 0.008] {
            let sum = at(f) + at(-f);
            assert!((sum - 1.0).abs() < 0.02, "skirts at ±{f} sum to {sum}");
        }
        assert!((at(0.05) - 1.0).abs() < 0.02, "upper sideband {}", at(0.05));
        assert!(at(-0.05).abs() < 0.02, "carved sideband {}", at(-0.05));
        assert!(at(0.2).abs() < 0.02, "stopband {}", at(0.2));
    }

    /// The designer reproduces a response a real prototype can also produce — the check that
    /// the quadrature and the windowing agree with `sdrmm_dsp`'s own lowpass design.
    #[test]
    fn the_designer_reproduces_a_plain_lowpass() {
        let designed = design_from_response(65, 16, |f| {
            Complex::new(f64::from(u8::from(f.abs() <= 0.1)), 0.0)
        });
        let reference = design_lowpass(65, 0.1);
        for &f in &[0.0, 0.05, 0.09, 0.2, 0.4] {
            let a = response(&designed, f).norm();
            let b = real_response(&reference, f).norm();
            assert!((a - b).abs() < 0.03, "at {f}: designed {a}, reference {b}");
        }
    }
}
