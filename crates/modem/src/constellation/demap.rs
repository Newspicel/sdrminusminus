//! The one demapper (MODEM-PLAN §3.1): every linear entry — PSK, QAM, APSK, PAM, and every
//! exotic table phase 4 adds — turns received points into per-bit LLRs through exactly this
//! code, and the orthogonal M-FSK entry does the same through the energy path. A second
//! demapper anywhere in the crate is a defect by §3.3.
//!
//! The statistical model, stated once: circularly-symmetric complex Gaussian noise of *total*
//! variance `noise_var` = E[|n|²] = N0 (per-component variance `noise_var`/2), equiprobable
//! symbols, so `p(y|x) ∝ exp(−|y−x|² / noise_var)`. The exact per-bit LLR is then
//!
//! ```text
//! llr_k = ln Σ_{x: bit k = 1} exp(−|y−x|²/N0)  −  ln Σ_{x: bit k = 0} exp(−|y−x|²/N0)
//! ```
//!
//! positive for 1, per the crate-root sign convention. The max-log tier keeps only each sum's
//! largest term: `llr_k ≈ (min_{bit=0} |y−x|² − min_{bit=1} |y−x|²) / N0`. For any 2-point
//! table each sum has one term, so max-log *is* exact there — the BPSK test below asserts it —
//! and its error elsewhere is bounded by ln(M/2) nats per side, shrinking exponentially as the
//! distance gap over N0 grows.
//!
//! `noise_var` is not optional and not a fudge factor: it is what turns geometry into
//! likelihood, and it comes from measurement — [`noise_var_from_known`] estimates it from the
//! known symbols (sync words, pilots, training) the §3.4 hook exposes. An LLR built from a
//! guessed variance is a [`crate::soft::SoftBit`] wearing the wrong type.

use num_complex::Complex;

use super::Constellation;
use crate::soft::Llr;

/// Labels are `u32`, so no table can carry more bit positions than this; the demappers size
/// their stack scratch with it instead of allocating.
const MAX_BITS: usize = 32;

/// Max-log LLRs for the bits of one received symbol. Formula and model in the module docs;
/// distances accumulate in f64 (f32 signals, f64 accounting) and only the finished LLR drops
/// to f32. Nearest-point search over the table is O(M · k) with zero allocation — the shape
/// the hot loop of every linear demodulator inherits.
///
/// The constructor's permutation invariant is what makes this total: every bit position has
/// points on both sides, so neither minimum is ever left at infinity.
///
/// # Panics
/// If `out.len() != c.bits_per_symbol()`, or `noise_var` is not a positive finite number.
pub fn max_log_llrs(y: Complex<f32>, c: &Constellation, noise_var: f64, out: &mut [Llr]) {
    let bits = c.bits_per_symbol();
    assert_eq!(bits, out.len(), "one LLR slot per label bit");
    assert!(
        noise_var.is_finite() && noise_var > 0.0,
        "noise_var is a measured variance; {noise_var} is not one"
    );
    let mut min0 = [f64::INFINITY; MAX_BITS];
    let mut min1 = [f64::INFINITY; MAX_BITS];
    for (p, &label) in c.points().iter().zip(c.labels()) {
        let d2 = dist2(y, *p);
        for k in 0..bits {
            if (label >> k) & 1 == 1 {
                min1[k] = min1[k].min(d2);
            } else {
                min0[k] = min0[k].min(d2);
            }
        }
    }
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = Llr(((min0[k] - min1[k]) / noise_var) as f32);
    }
}

/// The exact tier: full log-sum-exp over both hypothesis sets, per the module-doc formula.
/// Costs an `exp` per point per bit where max-log costs a compare, and buys back at most
/// ln(M/2) nats of approximation error — worth it only near sensitivity, where the
/// constellation's neighbours are genuinely confusable; the convergence test below pins both
/// regimes. Zero allocation, same panics as [`max_log_llrs`].
///
/// # Panics
/// If `out.len() != c.bits_per_symbol()`, or `noise_var` is not a positive finite number.
pub fn exact_llrs(y: Complex<f32>, c: &Constellation, noise_var: f64, out: &mut [Llr]) {
    let bits = c.bits_per_symbol();
    assert_eq!(bits, out.len(), "one LLR slot per label bit");
    assert!(
        noise_var.is_finite() && noise_var > 0.0,
        "noise_var is a measured variance; {noise_var} is not one"
    );
    let mut sum0 = [LogSum::EMPTY; MAX_BITS];
    let mut sum1 = [LogSum::EMPTY; MAX_BITS];
    for (p, &label) in c.points().iter().zip(c.labels()) {
        let exponent = -dist2(y, *p) / noise_var;
        for k in 0..bits {
            if (label >> k) & 1 == 1 {
                sum1[k].add(exponent);
            } else {
                sum0[k].add(exponent);
            }
        }
    }
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = Llr((sum1[k].value() - sum0[k].value()) as f32);
    }
}

/// Max-log LLRs from tone energies — the noncoherent M-FSK demapper (MODEM-PLAN §7 phase 1).
/// `energies[m]` is the filterbank output power of tone `m`, and the tone index *is* the bit
/// label (natural binary, bit k = `(m >> k) & 1`); a standard whose tone numbering is Gray or
/// otherwise permuted reorders its energies before this call — the permutation is that
/// entry's data, not this function's concern.
///
/// The form: `llr_k = (max_{bit=1} E_m − max_{bit=0} E_m) / noise_var`. This is the max-log
/// reduction of the square-law statistic: under noise alone a tone bin's energy is
/// exponential with mean N0, under signal-plus-noise its mean rises by Es, so each bin's
/// log-likelihood-ratio contribution is affine in E_m with slope `Es/(N0(N0+Es))` → 1/N0 as
/// Es/N0 grows, and max-log keeps the dominant bin per side. The exact nonfaded noncoherent
/// form would replace each energy with `ln I₀(2√(Es·E_m)/N0)` — a Bessel ratio per bin — and
/// sum, not maximise. Max-log suffices at the operating SNRs because a decodable M-FSK signal
/// has one bin far above the rest (that is what "orthogonal signalling works" means), the
/// Bessel weight is monotone in energy so the dominant bin and the sign are identical, and
/// the residual is a slowly varying positive scale that a Viterbi metric comparison is
/// invariant to. What max-log costs is only calibration accuracy near sensitivity — the
/// genie-bound harness measures that gap rather than trusting this paragraph.
///
/// # Panics
/// If `energies.len()` is not a power of two ≥ 2, if `out.len()` is not its log₂, or if
/// `noise_var` is not a positive finite number.
pub fn energy_llrs(energies: &[f32], noise_var: f64, out: &mut [Llr]) {
    let m = energies.len();
    assert!(
        m.is_power_of_two() && m >= 2 && m.trailing_zeros() as usize <= MAX_BITS,
        "{m} tones is not a 2^k filterbank"
    );
    let bits = m.trailing_zeros() as usize;
    assert_eq!(bits, out.len(), "one LLR slot per tone-index bit");
    assert!(
        noise_var.is_finite() && noise_var > 0.0,
        "noise_var is a measured variance; {noise_var} is not one"
    );
    let mut max0 = [f64::NEG_INFINITY; MAX_BITS];
    let mut max1 = [f64::NEG_INFINITY; MAX_BITS];
    for (tone, &e) in energies.iter().enumerate() {
        let e = f64::from(e);
        for k in 0..bits {
            if (tone >> k) & 1 == 1 {
                max1[k] = max1[k].max(e);
            } else {
                max0[k] = max0[k].max(e);
            }
        }
    }
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = Llr(((max1[k] - max0[k]) / noise_var) as f32);
    }
}

/// Data-aided noise-variance estimate: the mean of |received − expected|² over positions whose
/// transmitted symbols are known — the §3.4 known-symbol hook feeds sync words, pilots and
/// training sequences straight into this. Returns the *total* complex variance E[|n|²] = N0,
/// exactly the `noise_var` the demappers take (per-component σ² is half of it).
///
/// The estimate charges everything the model didn't remove — residual CFO rotation,
/// mis-equalised ISI — to noise, which *under*states every LLR magnitude rather than
/// overstating it: the honest direction for a soft value to err. Relative accuracy is
/// ~1/√(2n) (chi-squared with 2n degrees of freedom), so ~1% at 5000 known symbols and the
/// calibration test's 2% gate at 1e5 is comfortably estimator-limited.
///
/// # Panics
/// If the slices differ in length or are empty — an average over nothing is not 0, it is a
/// caller bug.
#[must_use]
pub fn noise_var_from_known(received: &[Complex<f32>], expected: &[Complex<f32>]) -> f64 {
    assert_eq!(
        received.len(),
        expected.len(),
        "received and expected must pair one-to-one"
    );
    assert!(!received.is_empty(), "no known symbols, no estimate");
    received
        .iter()
        .zip(expected)
        .map(|(&r, &e)| dist2(r, e))
        .sum::<f64>()
        / received.len() as f64
}

fn dist2(a: Complex<f32>, b: Complex<f32>) -> f64 {
    let dr = f64::from(a.re) - f64::from(b.re);
    let di = f64::from(a.im) - f64::from(b.im);
    dr * dr + di * di
}

/// Streaming log-sum-exp: `value()` = ln Σ exp(xᵢ) over everything `add`ed, kept stable by
/// carrying the running maximum outside the exponentials — a raw sum would underflow to
/// ln(0) = −∞ for any high-SNR exponent set.
#[derive(Clone, Copy)]
struct LogSum {
    max: f64,
    sum: f64,
}

impl LogSum {
    const EMPTY: Self = Self {
        max: f64::NEG_INFINITY,
        sum: 0.0,
    };

    fn add(&mut self, x: f64) {
        if x > self.max {
            // exp(−∞) is 0.0, so the first term lands as sum = 1 without a special case.
            self.sum = self.sum * (self.max - x).exp() + 1.0;
            self.max = x;
        } else {
            self.sum += (x - self.max).exp();
        }
    }

    fn value(self) -> f64 {
        self.max + self.sum.ln()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::{
        impair::{Awgn, Impairment},
        perf::assert_no_alloc,
        rng::Rng,
    };

    fn bpsk() -> Constellation {
        Constellation::from_points(
            vec![Complex::new(-1.0, 0.0), Complex::new(1.0, 0.0)],
            vec![0, 1],
        )
        .unwrap()
    }

    /// Gray 4-PAM as a real-axis table, handed in as ±1/±3 and normalised by construction to
    /// ±1/√5, ±3/√5 (mean Es 5 → scale 1/√5).
    fn gray_4pam() -> Constellation {
        Constellation::from_points(
            vec![
                Complex::new(-3.0, 0.0),
                Complex::new(-1.0, 0.0),
                Complex::new(1.0, 0.0),
                Complex::new(3.0, 0.0),
            ],
            vec![0b00, 0b01, 0b11, 0b10],
        )
        .unwrap()
    }

    /// The 2-level acceptance case: for BPSK at ±1 the closed form is llr = 2·re(y)/σ² with σ²
    /// the per-component variance — equivalently 4·re(y)/noise_var, since noise_var = 2σ².
    /// Hand numbers: σ = 0.5, y = 0.3 − 0.2j → llr = 2·0.3/0.25 = 2.4.
    #[test]
    fn bpsk_matches_the_hand_computed_closed_form() {
        let c = bpsk();
        let sigma: f64 = 0.5;
        let noise_var = 2.0 * sigma * sigma;
        let y = Complex::new(0.3f32, -0.2);
        let mut out = [Llr(0.0); 1];
        max_log_llrs(y, &c, noise_var, &mut out);
        assert!((f64::from(out[0].0) - 2.4).abs() < 1e-5, "llr {}", out[0].0);
        // The closed form with the received value's own f32 rounding, to full precision.
        let closed = 2.0 * f64::from(y.re) / (sigma * sigma);
        assert!((f64::from(out[0].0) - closed).abs() < 1e-6);
    }

    /// With one point per hypothesis each log-sum has a single term, so max-log and exact are
    /// the same number, not merely close — asserted across the plane, per the acceptance.
    #[test]
    fn two_point_table_makes_maxlog_exact() {
        let c = bpsk();
        for y in [
            Complex::new(0.3f32, -0.2),
            Complex::new(-1.7, 0.4),
            Complex::new(0.01, 0.0),
            Complex::new(2.5, -2.5),
        ] {
            let mut ml = [Llr(0.0); 1];
            let mut ex = [Llr(0.0); 1];
            max_log_llrs(y, &c, 0.5, &mut ml);
            exact_llrs(y, &c, 0.5, &mut ex);
            assert!(
                (ml[0].0 - ex[0].0).abs() < 1e-6,
                "max-log {} vs exact {} at {y}",
                ml[0].0,
                ex[0].0
            );
        }
    }

    /// The 4-level acceptance case, hand-computed outside this codebase for y = 0.6,
    /// noise_var = 0.5 on the normalised table (A = 1/√5):
    ///   d² to −3A, −A, +A, +3A = 3.76996911, 1.09665630, 0.02334369, 0.55003112
    ///   bit 0 (LSB): min₀ over {00, 10} = 0.55003112, min₁ over {01, 11} = 0.02334369
    ///     → llr₀ = (0.55003112 − 0.02334369)/0.5 = 1.0533748
    ///   bit 1: min₀ over {00, 01} = 1.09665630, min₁ over {11, 10} = 0.02334369
    ///     → llr₁ = (1.09665630 − 0.02334369)/0.5 = 2.1466253
    #[test]
    fn gray_4pam_maxlog_matches_hand_computation() {
        let c = gray_4pam();
        let mut out = [Llr(0.0); 2];
        max_log_llrs(Complex::new(0.6, 0.0), &c, 0.5, &mut out);
        assert!(
            (f64::from(out[0].0) - 1.053_374_8).abs() < 1e-5,
            "bit 0: {}",
            out[0].0
        );
        assert!(
            (f64::from(out[1].0) - 2.146_625_3).abs() < 1e-5,
            "bit 1: {}",
            out[1].0
        );
    }

    /// One exact-tier hand value at the same point: full log-sum-exp over the d² table above,
    ///   llr₀ = ln(e^−0.04668738 + e^−2.19331261) − ln(e^−1.10006224 + e^−7.53993823)
    ///        = 0.0638498 − (−1.0984668) = 1.1623167
    /// and llr₁ = 2.4410572 by the same arithmetic.
    #[test]
    fn gray_4pam_exact_tier_matches_hand_computation() {
        let c = gray_4pam();
        let mut out = [Llr(0.0); 2];
        exact_llrs(Complex::new(0.6, 0.0), &c, 0.5, &mut out);
        assert!(
            (f64::from(out[0].0) - 1.162_316_7).abs() < 1e-5,
            "bit 0: {}",
            out[0].0
        );
        assert!(
            (f64::from(out[1].0) - 2.441_057_2).abs() < 1e-5,
            "bit 1: {}",
            out[1].0
        );
    }

    /// The two tiers' relationship is predictable in both regimes: at high SNR the non-nearest
    /// points' exp terms vanish and exact → max-log; at low SNR they differ measurably, but
    /// never by more than ln(2) per side here — each hypothesis sum has two terms, so each
    /// log-sum exceeds its max term by at most ln 2.
    #[test]
    fn exact_converges_to_maxlog_at_high_snr_and_departs_boundedly_at_low() {
        let c = gray_4pam();
        let y = Complex::new(0.6, 0.0);
        let diff = |noise_var: f64| {
            let mut ml = [Llr(0.0); 2];
            let mut ex = [Llr(0.0); 2];
            max_log_llrs(y, &c, noise_var, &mut ml);
            exact_llrs(y, &c, noise_var, &mut ex);
            [
                f64::from(ex[0].0) - f64::from(ml[0].0),
                f64::from(ex[1].0) - f64::from(ml[1].0),
            ]
        };
        for d in diff(0.01) {
            assert!(d.abs() < 1e-6, "high SNR: {d}");
        }
        let low = diff(4.0);
        let bound = std::f64::consts::LN_2 + 1e-9;
        assert!(low.iter().any(|d| d.abs() > 0.01), "low SNR: {low:?}");
        for d in low {
            assert!(d.abs() <= bound, "low SNR diff {d} past ln 2");
        }
    }

    /// The M-FSK energy path, hand-computed for 4 tones at noise_var = 0.5,
    /// E = [0.1, 1.2, 0.3, 0.2] (tone index = label, natural binary):
    ///   bit 0: max over tones {1,3} = 1.2, over {0,2} = 0.3 → (1.2 − 0.3)/0.5 = +1.8
    ///   bit 1: max over tones {2,3} = 0.3, over {0,1} = 1.2 → (0.3 − 1.2)/0.5 = −1.8
    /// Tone 1 dominating votes bit 0 = 1 and bit 1 = 0, matching its label 0b01.
    #[test]
    fn fsk4_energy_llrs_match_hand_computation() {
        let energies = [0.1f32, 1.2, 0.3, 0.2];
        let mut out = [Llr(0.0); 2];
        energy_llrs(&energies, 0.5, &mut out);
        assert!(
            (f64::from(out[0].0) - 1.8).abs() < 1e-6,
            "bit 0: {}",
            out[0].0
        );
        assert!(
            (f64::from(out[1].0) + 1.8).abs() < 1e-6,
            "bit 1: {}",
            out[1].0
        );
        assert!(out[0].bit());
        assert!(!out[1].bit());
    }

    /// Calibration against impair-injected AWGN of known sigma: per-component σ = 0.3 means
    /// total variance 2σ² = 0.18. At 1e5 samples the estimator's own standard error is
    /// ~0.22%, so the 2% acceptance gate reads the estimator's correctness, not its noise.
    #[test]
    fn noise_var_estimator_reads_injected_awgn_within_two_percent() {
        let x = std::f32::consts::FRAC_1_SQRT_2;
        let qpsk = [
            Complex::new(x, x),
            Complex::new(-x, x),
            Complex::new(-x, -x),
            Complex::new(x, -x),
        ];
        let expected: Vec<Complex<f32>> = (0..100_000).map(|i| qpsk[i % 4]).collect();
        let mut received = expected.clone();
        let sigma = 0.3;
        Awgn::with_sigma(sigma).apply(&mut received, &mut Rng::new(0x0f5e));
        let estimate = noise_var_from_known(&received, &expected);
        let truth = 2.0 * sigma * sigma;
        assert!(
            (estimate / truth - 1.0).abs() < 0.02,
            "estimated {estimate}, injected {truth}"
        );
    }

    #[test]
    fn clean_symbols_estimate_zero_variance() {
        let expected = vec![Complex::new(1.0f32, 0.0); 16];
        assert_eq!(noise_var_from_known(&expected, &expected), 0.0);
    }

    /// §4.2's zero-allocation discipline applies to the demap paths from day one: they are
    /// the inner loop of every linear entry. The counting allocator is installed once per
    /// test binary, in `ber::perf::tests`.
    #[test]
    fn demap_paths_allocate_nothing() {
        let c = gray_4pam();
        let y = Complex::new(0.6f32, -0.1);
        let energies = [0.1f32, 1.2, 0.3, 0.2];
        let symbols = [Complex::new(1.0f32, 0.0); 64];
        let mut out = [Llr(0.0); 2];
        assert_no_alloc("max_log_llrs", || max_log_llrs(y, &c, 0.5, &mut out));
        assert_no_alloc("exact_llrs", || exact_llrs(y, &c, 0.5, &mut out));
        assert_no_alloc("energy_llrs", || energy_llrs(&energies, 0.5, &mut out));
        assert_no_alloc("hard_slice", || {
            std::hint::black_box(c.hard_slice(y));
        });
        assert_no_alloc("noise_var_from_known", || {
            std::hint::black_box(noise_var_from_known(&symbols, &symbols));
        });
    }

    /// max-log LLR signs agree with the nearest-point decision by construction; a sweep over
    /// the plane pins the wiring (labels, bit order, sign convention) rather than the math.
    #[test]
    fn maxlog_signs_agree_with_hard_slice() {
        let c = gray_4pam();
        let mut out = [Llr(0.0); 2];
        for i in -20..=20 {
            let y = Complex::new(i as f32 * 0.1, 0.0);
            max_log_llrs(y, &c, 0.3, &mut out);
            let label = c.hard_slice(y);
            for (k, llr) in out.iter().enumerate() {
                if !llr.is_erasure() {
                    assert_eq!(
                        llr.bit(),
                        (label >> k) & 1 == 1,
                        "bit {k} at y = {y}: llr {}",
                        llr.0
                    );
                }
            }
        }
    }
}
