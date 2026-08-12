//! Closed-form error-rate oracles (MODEM-PLAN §3.1 `ber/theory`, §4.1) — the acceptance
//! references every measured curve is held against. Each oracle takes Eb/N0 in dB per
//! *information bit* (the crate-root accounting) and returns a probability, in f64 because
//! this is the accounting side of the harness, not the signal path.
//!
//! Trust comes from published values, not from the harness: the tests pin every family to
//! textbook waterfall points (BPSK 1e-5 at 9.588 dB, DBPSK 1e-3 at 7.93 dB, 16-QAM 1e-3 near
//! 10.5 dB, …) and pin `erfc` itself against independently computed values. Exactness status
//! is stated per function — a 0.2 dB acceptance gate read against an approximate reference
//! has to know the reference's own error term, so "exact" and "high-SNR form" are never left
//! implicit.
//!
//! `erfc` is implemented here rather than pulled from a crate for the same reason [`super::rng`]
//! owns its generator: the oracle must be bit-identical on every platform and immune to a
//! dependency changing its polynomial between versions. A committed curve compared against a
//! reference that drifted is indistinguishable from a regression.

use std::f64::consts::{FRAC_1_SQRT_2, LN_2, PI};

// --- The Gaussian tail -----------------------------------------------------------------------
//
// W. J. Cody's rational Chebyshev approximations for erf/erfc ("Rational Chebyshev
// approximation for the error function", Math. Comp. 23 (1969), 631–637), with the coefficient
// sets from his SPECFUN `CALERF` (TOMS 715). The three segment approximations carry a stated
// maximal relative error below 6e-19, so the f64 evaluation is limited by arithmetic — a few
// ulps — far inside the 1e-10 gate the tests encode for [0, 6].

/// erf on [0, 0.46875]: erf(x) ≈ x·P(x²)/Q(x²).
const ERF_NUM: [f64; 5] = [
    3.1611237438705655,
    113.86415415105016,
    377.485237685302,
    3209.3775891384694,
    0.18577770618460315,
];
const ERF_DEN: [f64; 4] = [
    23.601290952344122,
    244.02463793444417,
    1282.6165260773723,
    2844.236833439171,
];

/// erfc on [0.46875, 4]: erfc(x) ≈ e^{−x²}·P(x)/Q(x).
const ERFC_MID_NUM: [f64; 9] = [
    0.5641884969886701,
    8.883149794388377,
    66.11919063714163,
    298.6351381974001,
    881.952221241769,
    1712.0476126340707,
    2051.0783778260716,
    1230.3393547979972,
    2.1531153547440383e-8,
];
const ERFC_MID_DEN: [f64; 8] = [
    15.744926110709835,
    117.6939508913125,
    537.1811018620099,
    1621.3895745666903,
    3290.7992357334597,
    4362.619090143247,
    3439.3676741437216,
    1230.3393548037495,
];

/// erfc beyond 4: erfc(x) ≈ e^{−x²}/x·(1/√π − x⁻²·P(x⁻²)/Q(x⁻²)).
const ERFC_FAR_NUM: [f64; 6] = [
    0.30532663496123236,
    0.36034489994980445,
    0.12578172611122926,
    0.016083785148742275,
    0.0006587491615298378,
    0.016315387137302097,
];
const ERFC_FAR_DEN: [f64; 5] = [
    2.568520192289822,
    1.8729528499234604,
    0.5279051029514285,
    0.06051834131244132,
    0.0023352049762686918,
];

const FRAC_1_SQRT_PI: f64 = 0.5641895835477563;

/// The complementary error function, double precision, accurate to a few ulps everywhere it
/// does not underflow (erfc(x) < the smallest normal beyond x ≈ 26.54, where 0 is returned).
#[must_use]
pub fn erfc(x: f64) -> f64 {
    let y = x.abs();
    if y <= 0.46875 {
        // erf is what the rational form gives here; erfc = 1 − erf costs nothing because
        // erf ≤ 0.5, so the subtraction cannot cancel. Below ~ε the x² term would be pure
        // noise; the approximation at ysq = 0 is erf's leading 2x/√π.
        let ysq = if y > 1.11e-16 { y * y } else { 0.0 };
        let mut num = ERF_NUM[4] * ysq;
        let mut den = ysq;
        for (&n, &d) in ERF_NUM[..3].iter().zip(&ERF_DEN[..3]) {
            num = (num + n) * ysq;
            den = (den + d) * ysq;
        }
        return 1.0 - x * (num + ERF_NUM[3]) / (den + ERF_DEN[3]);
    }
    let tail = if y <= 4.0 {
        let mut num = ERFC_MID_NUM[8] * y;
        let mut den = y;
        for (&n, &d) in ERFC_MID_NUM[..7].iter().zip(&ERFC_MID_DEN[..7]) {
            num = (num + n) * y;
            den = (den + d) * y;
        }
        exp_neg_squared(y) * (num + ERFC_MID_NUM[7]) / (den + ERFC_MID_DEN[7])
    } else if y < 26.543 {
        let inv = 1.0 / (y * y);
        let mut num = ERFC_FAR_NUM[5] * inv;
        let mut den = inv;
        for (&n, &d) in ERFC_FAR_NUM[..4].iter().zip(&ERFC_FAR_DEN[..4]) {
            num = (num + n) * inv;
            den = (den + d) * inv;
        }
        let r = inv * (num + ERFC_FAR_NUM[4]) / (den + ERFC_FAR_DEN[4]);
        exp_neg_squared(y) * (FRAC_1_SQRT_PI - r) / y
    } else {
        0.0
    };
    if x < 0.0 { 2.0 - tail } else { tail }
}

/// e^{−y²} by Cody's 1/16 splitting. Computed directly, the half-ulp rounding of y² becomes a
/// relative error of y²·ε in the exponential — 8e-14 by y = 26, an order above the rest of the
/// evaluation. Truncating y to 4 fractional bits makes ysq and ysq² exactly representable, so
/// the large exponent is exact and only the small remainder e^{−del} rounds.
fn exp_neg_squared(y: f64) -> f64 {
    let ysq = (y * 16.0).trunc() / 16.0;
    let del = (y - ysq) * (y + ysq);
    (-ysq * ysq).exp() * (-del).exp()
}

/// The Gaussian tail probability Q(x) = P(N(0,1) > x) = ½·erfc(x/√2) — the form every
/// coherent-detection curve below is built from.
#[must_use]
pub fn q(x: f64) -> f64 {
    0.5 * erfc(x * FRAC_1_SQRT_2)
}

// --- Coherent linear families ----------------------------------------------------------------

/// Eb/N0 per information bit, dB → linear. All oracles parameterise on the dB value exactly as
/// given; γ_b is its one f64 image.
fn ebn0_lin(ebn0_db: f64) -> f64 {
    10f64.powf(ebn0_db / 10.0)
}

/// log2(m) for a modulation order that must be a power of two ≥ 2 — a non-power-of-two order
/// handed to any oracle here is a harness bug, not a curve to produce.
fn bits_per_symbol(m: u32) -> f64 {
    debug_assert!(m >= 2 && m.is_power_of_two(), "modulation order {m}");
    f64::from(m.ilog2())
}

/// Coherent BPSK bit error rate — exact: ½·erfc(√γ_b).
#[must_use]
pub fn bpsk_ber(ebn0_db: f64) -> f64 {
    0.5 * erfc(ebn0_lin(ebn0_db).sqrt())
}

/// Coherent Gray-mapped QPSK bit error rate — exact, and identical to BPSK: the two rails are
/// independent BPSK channels each carrying one bit at the same Eb/N0, so per bit nothing
/// changes. Stated as its own function so a QPSK curve is compared against a reference that
/// says QPSK.
#[must_use]
pub fn qpsk_ber(ebn0_db: f64) -> f64 {
    bpsk_ber(ebn0_db)
}

/// M-PAM symbol error rate — exact for the uniform constellation at average symbol energy
/// k·Eb: 2(M−1)/M · Q(√(6k/(M²−1)·γ_b)). At m = 2 this is BPSK.
#[must_use]
pub fn mpam_ser(m: u32, ebn0_db: f64) -> f64 {
    let k = bits_per_symbol(m);
    let m_f = f64::from(m);
    let g = ebn0_lin(ebn0_db);
    2.0 * (m_f - 1.0) / m_f * q((6.0 * k / (m_f * m_f - 1.0) * g).sqrt())
}

/// Gray-mapped M-PAM bit error rate — the standard nearest-neighbour approximation SER/log2(M):
/// a Gray-adjacent symbol error costs exactly one bit, and errors that jump further carry
/// weight only at low SNR. Exact at m = 2; tight (a few percent) once SER ≲ 1e-1.
#[must_use]
pub fn mpam_ber(m: u32, ebn0_db: f64) -> f64 {
    mpam_ser(m, ebn0_db) / bits_per_symbol(m)
}

/// True for the orders square-QAM formulas hold for: 4, 16, 64, 256, 1024, … — two identical
/// √M-PAM rails require an even number of bits.
fn is_square_qam(m: u32) -> bool {
    m >= 4 && m.is_power_of_two() && m.ilog2().is_multiple_of(2)
}

/// Square M-QAM symbol error rate — exact: the constellation is two independent √M-PAM rails,
/// each in error with probability p = 2(1−1/√M)·Q(√(3k/(M−1)·γ_b)), so SER = 1 − (1−p)².
#[must_use]
pub fn mqam_ser(m: u32, ebn0_db: f64) -> f64 {
    debug_assert!(is_square_qam(m), "square QAM order {m}");
    let k = bits_per_symbol(m);
    let m_f = f64::from(m);
    let g = ebn0_lin(ebn0_db);
    let p = 2.0 * (1.0 - 1.0 / m_f.sqrt()) * q((3.0 * k / (m_f - 1.0) * g).sqrt());
    p * (2.0 - p)
}

/// Gray-mapped square M-QAM bit error rate — the standard nearest-neighbour form
/// (4/k)(1−1/√M)·Q(√(3k/(M−1)·γ_b)), i.e. one bit per rail-adjacent symbol error. At m = 4 it
/// reduces algebraically to [`qpsk_ber`]; for larger orders it is tight once SER ≲ 1e-1 and
/// asymptotically exact in SNR.
#[must_use]
pub fn mqam_ber(m: u32, ebn0_db: f64) -> f64 {
    debug_assert!(is_square_qam(m), "square QAM order {m}");
    let k = bits_per_symbol(m);
    let m_f = f64::from(m);
    let g = ebn0_lin(ebn0_db);
    4.0 / k * (1.0 - 1.0 / m_f.sqrt()) * q((3.0 * k / (m_f - 1.0) * g).sqrt())
}

/// Coherent M-PSK symbol error rate. Exact for m = 2 (Q(√(2γ_b))) and m = 4
/// (2Q − Q² over the two rails). For m > 4 no closed form exists; this is the standard
/// nearest-boundary form 2Q(√(2k·γ_b)·sin(π/M)) — it counts the two adjacent decision
/// boundaries and nothing beyond, so it reads slightly low at low SNR and is asymptotically
/// exact in SNR (well inside measurement tolerance once SER ≲ 1e-2).
#[must_use]
pub fn mpsk_ser(m: u32, ebn0_db: f64) -> f64 {
    let k = bits_per_symbol(m);
    let g = ebn0_lin(ebn0_db);
    match m {
        2 => q((2.0 * g).sqrt()),
        4 => {
            let p = q((2.0 * g).sqrt());
            p * (2.0 - p)
        }
        _ => 2.0 * q((2.0 * k * g).sqrt() * (PI / f64::from(m)).sin()),
    }
}

// --- Differential detection ------------------------------------------------------------------

/// Differentially detected binary DPSK bit error rate — exact: ½·e^{−γ_b}. No carrier
/// recovery, no Gaussian tail: the noncoherent detection statistic gives a pure exponential.
#[must_use]
pub fn dbpsk_ber(ebn0_db: f64) -> f64 {
    0.5 * (-ebn0_lin(ebn0_db)).exp()
}

/// Differentially detected Gray-coded DQPSK (equivalently π/4-DQPSK) bit error rate — exact,
/// the Marcum-Q form (Proakis & Salehi 5e, §4.5-5):
/// P_b = Q₁(a,b) − ½·I₀(ab)·e^{−(a²+b²)/2}, a,b = √(2γ_b(1 ∓ 1/√2)).
///
/// Evaluated through the series rearrangement P_b = e^{−(b−a)²/2}·(½·Î₀ + Σ_{k≥1} (a/b)^k·Î_k)
/// with Î_k = I_k(ab)·e^{−ab} — every term positive, so the high-SNR regime where Q₁ and the
/// I₀ term nearly cancel costs no precision.
#[must_use]
pub fn dqpsk_ber(ebn0_db: f64) -> f64 {
    let g = ebn0_lin(ebn0_db);
    let a = (2.0 * g * (1.0 - FRAC_1_SQRT_2)).sqrt();
    let b = (2.0 * g * (1.0 + FRAC_1_SQRT_2)).sqrt();
    let r = a / b;
    let x = a * b;
    let scaled = bessel_i_scaled(x, series_len(x, r));
    let mut sum = 0.5 * scaled[0];
    let mut rk = r;
    for &ik in &scaled[1..] {
        sum += rk * ik;
        rk *= r;
    }
    let d = b - a;
    (-0.5 * d * d).exp() * sum
}

/// The first-order Marcum Q function Q₁(a, b) for a, b ≥ 0, via the canonical series
/// Q₁ = e^{−(a²+b²)/2}·Σ_{k≥0} (a/b)^k·I_k(ab), computed with exponentially scaled Bessel
/// terms so nothing overflows however large ab gets. For a > b the series conditioning flips,
/// so the symmetry Q₁(a,b) + Q₁(b,a) = 1 + e^{−(a²+b²)/2}·I₀(ab) maps it back.
#[must_use]
pub fn marcum_q1(a: f64, b: f64) -> f64 {
    debug_assert!(a >= 0.0 && b >= 0.0, "Marcum Q of ({a}, {b})");
    if b <= 0.0 {
        return 1.0;
    }
    if a <= 0.0 {
        return (-0.5 * b * b).exp();
    }
    if a > b {
        let (swapped, i0_scaled) = q1_core(b, a);
        let d = a - b;
        return 1.0 + (-0.5 * d * d).exp() * i0_scaled - swapped;
    }
    q1_core(a, b).0
}

/// (Q₁(a,b), I₀(ab)·e^{−ab}) for 0 < a ≤ b. With Î_k = I_k(ab)·e^{−ab} the series becomes
/// e^{−(b−a)²/2}·Σ (a/b)^k·Î_k — all factors in [0, 1], no overflow at any SNR.
fn q1_core(a: f64, b: f64) -> (f64, f64) {
    let x = a * b;
    let r = a / b;
    let scaled = bessel_i_scaled(x, series_len(x, r));
    let mut sum = 0.0;
    let mut rk = 1.0;
    for &ik in &scaled {
        sum += rk * ik;
        rk *= r;
    }
    let d = b - a;
    ((-0.5 * d * d).exp() * sum, scaled[0])
}

/// One-step estimate of I_j(x)/I_{j−1}(x) — the Padé form x/(j + √(j² + x²)), always in
/// (0, 1]. Only used to size series truncation and recurrence start; a few percent of error
/// here just costs a couple of spare terms.
fn bessel_ratio_estimate(j: f64, x: f64) -> f64 {
    x / (j + (j * j + x * x).sqrt())
}

/// Highest series index worth keeping for Σ (a/b)^k·Î_k(x): walk the per-term bound
/// r·(ratio estimate) down to 1e-22 of the leading term. The cap only matters for
/// astronomically large x, where the tail it truncates is still far below f64 resolution
/// of the sum.
fn series_len(x: f64, r: f64) -> usize {
    let mut k = 1usize;
    let mut bound = 1.0f64;
    while bound > 1e-22 && k < 2000 {
        bound *= r * bessel_ratio_estimate(k as f64, x);
        k += 1;
    }
    k
}

/// Exponentially scaled modified Bessel functions Î_k = I_k(x)·e^{−x} for k = 0..=k_max, by
/// Miller's algorithm: the upward recurrence I_{k−1} = I_{k+1} + (2k/x)·I_k is unstable in the
/// direction of growing k, so it is run downward from a start order high enough that the seed's
/// arbitrariness has decayed below 1e-24 by k_max, then the whole set is normalised at once
/// through the identity I₀(x) + 2·Σ_{k≥1} I_k(x) = e^x — which lands directly on the scaled
/// values without ever forming e^x.
fn bessel_i_scaled(x: f64, k_max: usize) -> Vec<f64> {
    let mut out = vec![0.0; k_max + 1];
    if x <= 0.0 {
        out[0] = 1.0;
        return out;
    }
    let mut start = k_max + 1;
    let mut headroom = bessel_ratio_estimate(start as f64, x);
    while headroom > 1e-24 && start < k_max + 4000 {
        start += 1;
        headroom *= bessel_ratio_estimate(start as f64, x);
    }
    // Unnormalised values grow toward k = 0 by the same factor the headroom shrank, so the
    // tiny seed keeps everything in normal f64 range; the 1e250 rescale is for callers far
    // outside any Eb/N0 a curve will ever see.
    let mut above = 0.0f64;
    let mut cur = 1e-280f64;
    let mut norm = 2.0 * cur;
    let mut j = start;
    while j > 0 {
        let below = above + (2.0 * j as f64 / x) * cur;
        above = cur;
        cur = below;
        j -= 1;
        norm += if j == 0 { cur } else { 2.0 * cur };
        if j <= k_max {
            out[j] = cur;
        }
        if cur > 1e250 {
            above *= 1e-250;
            cur *= 1e-250;
            norm *= 1e-250;
            for v in &mut out {
                *v *= 1e-250;
            }
        }
    }
    for v in &mut out {
        *v /= norm;
    }
    out
}

// --- Noncoherent orthogonal M-FSK ------------------------------------------------------------

/// Noncoherent orthogonal M-FSK symbol error rate — exact, the alternating binomial sum
/// (Proakis & Salehi 5e, §4.5-4) at symbol SNR γ_s = k·γ_b:
/// P_s = Σ_{n=1}^{M−1} (−1)^{n+1}·C(M−1,n)/(n+1)·e^{−γ_s·n/(n+1)}.
///
/// The sum is violently ill-conditioned in plain f64: at M = 64 and 0 dB the alternating terms
/// reach 8.6e13 while the result is 0.296, and plain double precision misses it by 19% (measured).
/// So the binomials are held exactly as integers — which is why m stops at 64, the largest
/// order whose every C(m−1, n) still converts to double-double without rounding — and every
/// term and the accumulation run in double-double arithmetic: absolute error ≤ max-term·1e-32
/// ≈ 1e-17 across the whole SNR axis, so the oracle stays exact even where a low-SNR sweep
/// point reads the worst of the cancellation.
#[must_use]
pub fn mfsk_noncoherent_ser(m: u32, ebn0_db: f64) -> f64 {
    debug_assert!(m <= 64, "binomials are exact only to m = 64, got {m}");
    let k = bits_per_symbol(m);
    let gs = Dd::product(k, ebn0_lin(ebn0_db));
    let mut sum = Dd::ZERO;
    let mut binom: u128 = 1;
    for n in 1..m {
        binom = binom * u128::from(m - n) / u128::from(n);
        let np1 = f64::from(n + 1);
        let term = Dd::from_u64(binom as u64)
            .mul(gs.mul_f64(-f64::from(n)).div_f64(np1).exp())
            .div_f64(np1);
        sum = sum.add(if n % 2 == 1 { term } else { term.neg() });
    }
    sum.to_f64()
}

/// Noncoherent orthogonal M-FSK bit error rate — exact given the SER: orthogonal signalling
/// makes every wrong symbol equally likely, so a symbol error flips each bit with probability
/// 2^{k−1}/(2^k−1), i.e. BER = SER·M/(2(M−1)). At m = 2 the factor is 1.
#[must_use]
pub fn mfsk_noncoherent_ber(m: u32, ebn0_db: f64) -> f64 {
    let m_f = f64::from(m);
    mfsk_noncoherent_ser(m, ebn0_db) * m_f / (2.0 * (m_f - 1.0))
}

// --- Table-driven nearest-neighbour bound -----------------------------------------------------

/// The high-SNR error rate of an *arbitrary* constellation, computed from the table itself
/// (MODEM-PLAN §3.3: constellations are data — so is their reference curve).
///
/// The closed forms above exist because PAM, PSK and square QAM have regular geometries whose
/// error probability integrates in closed form. Cross-QAM, star-QAM, non-uniform QAM and APSK
/// do not — but every one of them obeys the same nearest-neighbour asymptote, and that is the
/// acceptance reference §4.1 asks for wherever a curve has no exact oracle:
///
/// ```text
/// SER ≈ N̄ · Q(d_min / (2σ)),   σ² = N0/2,  Es = 1  ⇒  SER ≈ N̄ · Q(d_min·√(k·γ_b/2))
/// ```
///
/// with `d_min` the table's minimum distance and `N̄` the mean number of points at that
/// distance. This is the union bound truncated to the closest shell: it *over*counts (every
/// pair beyond the shell is dropped, but the shell's own overlaps are double-counted) by a
/// factor that vanishes as SNR grows, so it is an upper bound in the tail and an approximation
/// at the shoulder. Tolerances against it are therefore stated at high SNR, and every entry
/// using it also commits its measured curve.
///
/// The BER form divides by the labelling's own measured cost rather than assuming Gray: a
/// nearest-neighbour symbol error costs [`Self::bits_per_error`] bits on average, which for the
/// closed-form families is exactly 1 and for the descent-labelled ones is what the descent
/// reached.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NearestNeighbour {
    /// Minimum distance of the unit-mean-energy table.
    pub d_min: f64,
    /// Mean number of points at `d_min` from a point.
    pub neighbours: f64,
    /// Mean Hamming distance across the minimum-distance pairs.
    pub bits_per_error: f64,
    /// log2 of the table size.
    pub bits_per_symbol: f64,
}

/// Relative slack defining "at the minimum distance", matching the constellation module's own:
/// several orders above trigonometric rounding, several below the gap to the next shell in
/// every catalog table.
const SHELL_SLACK: f64 = 1.002;

impl NearestNeighbour {
    /// Reads the three geometric numbers off a table. O(M²) — a setup-time measurement, not a
    /// per-point one; hold the result and call [`Self::ser`] on the grid.
    #[must_use]
    pub fn of(c: &crate::constellation::Constellation) -> Self {
        let p = c.points();
        let n = p.len();
        let d2 = |a: usize, b: usize| f64::from((p[a] - p[b]).norm_sqr());
        let mut min = f64::INFINITY;
        for i in 0..n {
            for j in (i + 1)..n {
                min = min.min(d2(i, j));
            }
        }
        let limit = min * SHELL_SLACK;
        let (mut pairs, mut bits) = (0u64, 0u64);
        for i in 0..n {
            for j in (i + 1)..n {
                if d2(i, j) <= limit {
                    pairs += 1;
                    bits += u64::from((c.labels()[i] ^ c.labels()[j]).count_ones());
                }
            }
        }
        Self {
            d_min: min.sqrt(),
            // Each pair is one neighbour for each of its two endpoints.
            neighbours: 2.0 * pairs as f64 / n as f64,
            bits_per_error: bits as f64 / pairs as f64,
            bits_per_symbol: f64::from(c.bits_per_symbol() as u32),
        }
    }

    /// Symbol error rate at Eb/N0 in dB, per the module's per-information-bit accounting.
    #[must_use]
    pub fn ser(self, ebn0_db: f64) -> f64 {
        let gs = self.bits_per_symbol * ebn0_lin(ebn0_db);
        (self.neighbours * q(self.d_min * (0.5 * gs).sqrt())).min(1.0)
    }

    /// Bit error rate: symbol errors times the labelling's measured bits per error, spread over
    /// the symbol's bits.
    #[must_use]
    pub fn ber(self, ebn0_db: f64) -> f64 {
        (self.ser(ebn0_db) * self.bits_per_error / self.bits_per_symbol).min(1.0)
    }
}

// --- Double-double arithmetic ----------------------------------------------------------------
//
// An unevaluated sum hi + lo of two f64 (Dekker/Knuth error-free transformations, as in the
// QD library), worth ~31 significant digits. Deterministic across platforms: every operation
// is built from IEEE-754 correctly-rounded +, −, × and fused multiply-add. Only what the
// M-FSK sum needs exists — this is not a general library.

/// ln 2 to double-double precision, for the range reduction in [`Dd::exp`].
const LN2_DD: Dd = Dd {
    hi: LN_2,
    lo: 2.3190468138462996e-17,
};

#[derive(Clone, Copy, Debug)]
struct Dd {
    hi: f64,
    lo: f64,
}

/// Error-free a + b (Knuth two-sum): hi is the rounded sum, lo the exact rounding error.
fn two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let bb = s - a;
    Dd {
        hi: s,
        lo: (a - (s - bb)) + (b - bb),
    }
}

/// Error-free a + b assuming |a| ≥ |b| — one operation cheaper than [`two_sum`].
fn quick_two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    Dd {
        hi: s,
        lo: b - (s - a),
    }
}

/// Error-free a·b via fused multiply-add: the FMA rounds once, so a·b − round(a·b) is exact.
fn two_prod(a: f64, b: f64) -> Dd {
    let p = a * b;
    Dd {
        hi: p,
        lo: a.mul_add(b, -p),
    }
}

impl Dd {
    const ZERO: Dd = Dd { hi: 0.0, lo: 0.0 };
    const ONE: Dd = Dd { hi: 1.0, lo: 0.0 };

    /// The exact product of two f64 — how γ_s = k·γ_b enters without rounding.
    fn product(a: f64, b: f64) -> Dd {
        two_prod(a, b)
    }

    /// Exact conversion of an integer up to 2^63 (binomials for m ≤ 64 fit): the rounded
    /// image plus the integer remainder, which is small enough to be a second exact f64.
    fn from_u64(v: u64) -> Dd {
        let hi = v as f64;
        #[allow(clippy::cast_possible_truncation)]
        let lo = (v as i128 - hi as i128) as f64;
        Dd { hi, lo }
    }

    fn neg(self) -> Dd {
        Dd {
            hi: -self.hi,
            lo: -self.lo,
        }
    }

    fn add(self, o: Dd) -> Dd {
        let s = two_sum(self.hi, o.hi);
        let t = two_sum(self.lo, o.lo);
        let u = quick_two_sum(s.hi, s.lo + t.hi);
        quick_two_sum(u.hi, u.lo + t.lo)
    }

    fn mul(self, o: Dd) -> Dd {
        let p = two_prod(self.hi, o.hi);
        quick_two_sum(p.hi, p.lo + (self.hi * o.lo + self.lo * o.hi))
    }

    fn mul_f64(self, m: f64) -> Dd {
        let p = two_prod(self.hi, m);
        quick_two_sum(p.hi, p.lo + self.lo * m)
    }

    fn div_f64(self, d: f64) -> Dd {
        let q1 = self.hi / d;
        let p = two_prod(q1, d);
        let s = two_sum(self.hi, -p.hi);
        let q2 = (s.hi + ((s.lo + self.lo) - p.lo)) / d;
        quick_two_sum(q1, q2)
    }

    /// e^self for self ≤ 0, by range reduction against [`LN2_DD`] and a Taylor series on the
    /// remainder (|r| ≤ ln2/2, so ~25 terms reach 1e-35). Arguments below −708 return zero:
    /// that is where f64 exp underflows anyway, and a sum term that small decides nothing.
    fn exp(self) -> Dd {
        debug_assert!(
            self.hi <= 0.0,
            "Dd::exp is written for the e^-x of a decay term"
        );
        if self.hi < -708.0 {
            return Dd::ZERO;
        }
        let k = (self.hi / LN2_DD.hi).round();
        let r = self.add(LN2_DD.mul_f64(-k));
        let mut sum = Dd::ONE;
        let mut term = Dd::ONE;
        let mut i = 1.0f64;
        loop {
            term = term.mul(r).div_f64(i);
            sum = sum.add(term);
            i += 1.0;
            if term.hi.abs() < 1e-35 || i > 40.0 {
                break;
            }
        }
        // 2^k is exact, so the scaling touches neither component's error term.
        #[allow(clippy::cast_possible_truncation)]
        let scale = 2f64.powi(k as i32);
        Dd {
            hi: sum.hi * scale,
            lo: sum.lo * scale,
        }
    }

    fn to_f64(self) -> f64 {
        self.hi + self.lo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rel(actual: f64, expected: f64, tol: f64, what: &str) {
        let rel = ((actual - expected) / expected).abs();
        assert!(
            rel < tol,
            "{what}: got {actual:e}, want {expected:e}, rel err {rel:e}"
        );
    }

    /// The table-driven bound must reproduce the closed forms it generalises, in the tail where
    /// the nearest-neighbour asymptote is the whole story. 4-PAM and 16-QAM are the two shapes
    /// the catalog's exotic tables are read against, and both are exact enough by 15 dB that a
    /// 2% agreement is a real check on `d_min`, `N̄` and the Eb accounting all three.
    #[test]
    fn nearest_neighbour_bound_reproduces_the_closed_forms() {
        use crate::constellation::tables;
        let pam4 = NearestNeighbour::of(&tables::pam(4).unwrap());
        // ±1/±3 at mean Es 1: spacing 2/√5, four points, the two inner ones with two neighbours.
        assert!((pam4.d_min - 2.0 / 5f64.sqrt()).abs() < 1e-6, "{pam4:?}");
        assert!((pam4.neighbours - 1.5).abs() < 1e-12);
        assert!((pam4.bits_per_error - 1.0).abs() < 1e-12);
        for db in [12.0, 15.0, 18.0] {
            assert_rel(pam4.ser(db), mpam_ser(4, db), 0.02, "4-PAM SER");
            assert_rel(pam4.ber(db), mpam_ber(4, db), 0.02, "4-PAM BER");
        }
        let qam16 = NearestNeighbour::of(&tables::qam_square(16).unwrap());
        assert!((qam16.neighbours - 3.0).abs() < 1e-12, "{qam16:?}");
        for db in [14.0, 17.0, 20.0] {
            assert_rel(qam16.ser(db), mqam_ser(16, db), 0.02, "16-QAM SER");
            assert_rel(qam16.ber(db), mqam_ber(16, db), 0.02, "16-QAM BER");
        }
        // BPSK is the degenerate case the whole harness is calibrated on: one neighbour at
        // distance 2, so the bound is the exact curve.
        let bpsk = NearestNeighbour::of(&tables::pam(2).unwrap());
        for db in [0.0, 5.0, 10.0] {
            assert_rel(bpsk.ser(db), bpsk_ber(db), 1e-9, "BPSK");
        }
    }

    /// The exotic tables have no closed form; what can still be checked is that their geometry
    /// reads back sanely and that the bound orders them the way their densities demand.
    #[test]
    fn exotic_tables_read_back_a_usable_bound() {
        use crate::constellation::tables;
        let cross32 = NearestNeighbour::of(&tables::qam_cross(32).unwrap());
        let qam32_ish = NearestNeighbour::of(&tables::qam_square(64).unwrap());
        assert!(cross32.bits_per_error > 1.0, "{cross32:?}");
        // Five bits per symbol packed into a cross beats six into a square at equal Es.
        assert!(cross32.d_min > qam32_ish.d_min);
        assert!(cross32.ser(20.0) < qam32_ish.ser(20.0));
        let apsk16 = NearestNeighbour::of(&tables::apsk16_dvbs2(3.15).unwrap());
        let qam16 = NearestNeighbour::of(&tables::qam_square(16).unwrap());
        // DVB-S2 16-APSK gives away Euclidean distance to stay circular; on a linear channel
        // that is a measurable loss against 16-QAM, and the bound must say so.
        assert!(apsk16.d_min < qam16.d_min, "{apsk16:?} vs {qam16:?}");
        for nn in [cross32, apsk16] {
            assert!(nn.ser(25.0) < 1e-6 && nn.ser(0.0) <= 1.0);
        }
    }

    /// Every reference value in this module was computed independently with mpmath at 40
    /// decimal digits, then rounded to the nearest f64.
    #[test]
    fn erfc_matches_independent_values() {
        let table = [
            (0.1, 0.8875370839817152),
            (0.5, 0.4795001221869535),
            (1.0, 0.15729920705028513),
            (1.5, 0.033894853524689274),
            (2.0, 0.004677734981047266),
            (3.0, 2.209049699858544e-5),
            (4.0, 1.541725790028002e-8),
            (5.0, 1.537459794428035e-12),
            (6.0, 2.1519736712498913e-17),
            (-0.5, 1.5204998778130465),
            (-2.0, 1.9953222650189528),
        ];
        for (x, want) in table {
            // The requirement is 1e-10 on [0, 6]; Cody delivers a few ulps, so gate at 1e-12.
            assert_rel(erfc(x), want, 1e-12, &format!("erfc({x})"));
        }
        assert!((erfc(0.0) - 1.0).abs() < 1e-15, "erfc(0)");
        assert_eq!(erfc(30.0), 0.0, "erfc past the underflow cutoff");
    }

    #[test]
    fn q_is_the_gaussian_tail() {
        assert!((q(0.0) - 0.5).abs() < 1e-15, "q(0)");
        let table = [
            (1.0, 0.15865525393145705),
            (2.0, 0.02275013194817921),
            (3.0, 0.0013498980316300946),
            (4.0, 3.1671241833119924e-5),
        ];
        for (x, want) in table {
            assert_rel(q(x), want, 1e-12, &format!("q({x})"));
        }
    }

    #[test]
    fn bpsk_hits_published_waterfall_points() {
        assert_rel(bpsk_ber(6.789522612404168), 1e-3, 1e-10, "BPSK at 6.79 dB");
        assert_rel(bpsk_ber(9.587858346847607), 1e-5, 1e-10, "BPSK at 9.59 dB");
    }

    #[test]
    fn qpsk_ber_equals_bpsk_ber() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            assert_eq!(qpsk_ber(db), bpsk_ber(db), "at {db} dB");
        }
    }

    #[test]
    fn dbpsk_hits_published_waterfall_point() {
        assert_rel(
            dbpsk_ber(7.934137466447398),
            1e-3,
            1e-10,
            "DBPSK at 7.93 dB",
        );
    }

    #[test]
    fn pam_matches_forms_and_reduces_to_bpsk() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            assert_rel(
                mpam_ber(2, db),
                bpsk_ber(db),
                1e-12,
                &format!("2-PAM at {db} dB"),
            );
            assert_rel(
                mpam_ser(2, db),
                bpsk_ber(db),
                1e-12,
                &format!("2-PAM SER at {db} dB"),
            );
        }
        assert_rel(
            mpam_ser(4, 6.0),
            0.05574261263932141,
            1e-12,
            "4-PAM SER at 6 dB",
        );
        assert_rel(
            mpam_ser(4, 10.0),
            0.0035083012357854495,
            1e-12,
            "4-PAM SER at 10 dB",
        );
    }

    #[test]
    fn qam16_matches_table_values() {
        // The published table point: Gray 16-QAM crosses BER 1e-3 near 10.5 dB.
        assert_rel(
            mqam_ber(16, 10.5),
            0.001025725227946195,
            1e-12,
            "16-QAM BER at 10.5 dB",
        );
        assert_rel(
            mqam_ber(16, 10.522401171856055),
            1e-3,
            1e-10,
            "16-QAM at its 1e-3 point",
        );
        assert_rel(
            mqam_ber(16, 4.0),
            0.058618457419250876,
            1e-12,
            "16-QAM BER at 4 dB",
        );
        assert_rel(
            mqam_ser(16, 12.0),
            0.0005545578503225422,
            1e-12,
            "16-QAM SER at 12 dB",
        );
        assert_rel(
            mqam_ber(64, 18.0),
            6.35114807198656e-6,
            1e-12,
            "64-QAM BER at 18 dB",
        );
    }

    #[test]
    fn qam4_ber_equals_qpsk() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            assert_rel(
                mqam_ber(4, db),
                qpsk_ber(db),
                1e-12,
                &format!("4-QAM at {db} dB"),
            );
        }
    }

    #[test]
    fn psk_matches_forms() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            assert_rel(
                mpsk_ser(2, db),
                bpsk_ber(db),
                1e-12,
                &format!("2-PSK at {db} dB"),
            );
        }
        assert_rel(
            mpsk_ser(4, 6.79),
            0.001997857657700032,
            1e-12,
            "QPSK SER at 6.79 dB",
        );
        assert_rel(
            mpsk_ser(8, 10.0),
            0.0030341859621386717,
            1e-12,
            "8-PSK SER at 10 dB",
        );
        assert_rel(
            mpsk_ser(8, 14.0),
            2.6268980874931245e-6,
            1e-12,
            "8-PSK SER at 14 dB",
        );
    }

    #[test]
    fn marcum_q1_matches_independent_values() {
        let table = [
            (1.0, 2.0, 0.26901206003591),
            (0.5, 3.0, 0.01784367338648221),
            (2.0, 1.0, 0.918107696369406), // a > b: the symmetry branch
            (3.0, 3.0, 0.5674797622908615), // a = b: slowest series convergence
            (2.42, 5.84, 0.0005012345039334298),
        ];
        for (a, b, want) in table {
            assert_rel(marcum_q1(a, b), want, 1e-12, &format!("Q1({a}, {b})"));
        }
        assert_eq!(marcum_q1(1.5, 0.0), 1.0, "Q1(a, 0)");
        assert_rel(marcum_q1(0.0, 2.0), (-2.0f64).exp(), 1e-14, "Q1(0, b)");
    }

    #[test]
    fn dqpsk_matches_exact_marcum_form() {
        let table = [
            (0.0, 0.1639075303995848),
            (4.0, 0.04874886223803079),
            (6.0, 0.017235900604692805),
            (8.0, 0.0036429431289647296),
            (10.0, 0.0003431845960334517),
            (12.0, 9.052589122173602e-6),
            (14.0, 3.197767175455164e-8),
        ];
        for (db, want) in table {
            assert_rel(dqpsk_ber(db), want, 1e-12, &format!("DQPSK at {db} dB"));
        }
        assert_rel(
            dqpsk_ber(9.197822982008024),
            1e-3,
            1e-10,
            "DQPSK at its 1e-3 point",
        );
    }

    #[test]
    fn noncoherent_2fsk_is_half_exp() {
        for tenth in 0..=140 {
            let db = f64::from(tenth) * 0.1;
            let want = 0.5 * (-0.5 * 10f64.powf(db / 10.0)).exp();
            assert_rel(
                mfsk_noncoherent_ser(2, db),
                want,
                1e-13,
                &format!("2-FSK at {db} dB"),
            );
            assert_eq!(
                mfsk_noncoherent_ber(2, db),
                mfsk_noncoherent_ser(2, db),
                "binary: every symbol error is the bit error"
            );
        }
        assert_rel(
            mfsk_noncoherent_ber(2, 10.94443742308721),
            1e-3,
            1e-10,
            "2-FSK 1e-3 point",
        );
    }

    /// The reason the sum runs in double-double: at M = 64 and 0 dB plain f64 returns 0.3516
    /// against the true 0.2964 — a 19% miss. These gates are orders tighter than the f64
    /// failure, so they fail again if anyone "simplifies" the arithmetic back.
    #[test]
    fn mfsk_alternating_sum_survives_cancellation() {
        assert_rel(
            mfsk_noncoherent_ser(64, 0.0),
            0.29641064049182236,
            1e-13,
            "64-FSK at 0 dB",
        );
        assert_rel(
            mfsk_noncoherent_ser(64, 6.0),
            0.0001696329205128982,
            1e-13,
            "64-FSK at 6 dB",
        );
        assert_rel(
            mfsk_noncoherent_ser(64, 8.0),
            1.8445695709968937e-7,
            1e-13,
            "64-FSK at 8 dB",
        );
        assert_rel(
            mfsk_noncoherent_ser(16, 8.0),
            2.3508202502470605e-5,
            1e-13,
            "16-FSK at 8 dB",
        );
        assert_rel(
            mfsk_noncoherent_ser(4, 8.0),
            0.0025255900294627294,
            1e-13,
            "4-FSK at 8 dB",
        );
        assert_rel(
            mfsk_noncoherent_ber(4, 8.0),
            0.0016837266863084864,
            1e-13,
            "4-FSK BER at 8 dB",
        );
    }

    #[test]
    fn mfsk_bit_conversion_is_the_orthogonal_factor() {
        for (m, factor) in [(4u32, 2.0 / 3.0), (16, 8.0 / 15.0), (64, 32.0 / 63.0)] {
            let ratio = mfsk_noncoherent_ber(m, 5.0) / mfsk_noncoherent_ser(m, 5.0);
            assert_rel(ratio, factor, 1e-14, &format!("{m}-FSK bit factor"));
        }
    }

    fn assert_strictly_decreasing(f: &dyn Fn(f64) -> f64, what: &str) {
        let mut prev = f(0.0);
        for tenth in 1..=140 {
            let db = f64::from(tenth) * 0.1;
            let cur = f(db);
            assert!(
                cur < prev,
                "{what} not strictly decreasing at {db} dB: {cur:e} !< {prev:e}"
            );
            prev = cur;
        }
    }

    #[test]
    fn every_curve_strictly_decreases_over_the_sweep_range() {
        assert_strictly_decreasing(&bpsk_ber, "BPSK");
        assert_strictly_decreasing(&qpsk_ber, "QPSK");
        assert_strictly_decreasing(&dbpsk_ber, "DBPSK");
        assert_strictly_decreasing(&dqpsk_ber, "DQPSK");
        for m in [2u32, 4, 8] {
            assert_strictly_decreasing(&|db| mpam_ser(m, db), &format!("{m}-PAM SER"));
            assert_strictly_decreasing(&|db| mpam_ber(m, db), &format!("{m}-PAM BER"));
        }
        for m in [4u32, 16, 64, 256, 1024] {
            assert_strictly_decreasing(&|db| mqam_ser(m, db), &format!("{m}-QAM SER"));
            assert_strictly_decreasing(&|db| mqam_ber(m, db), &format!("{m}-QAM BER"));
        }
        for m in [2u32, 4, 8, 16] {
            assert_strictly_decreasing(&|db| mpsk_ser(m, db), &format!("{m}-PSK SER"));
        }
        for m in [2u32, 4, 16, 64] {
            assert_strictly_decreasing(&|db| mfsk_noncoherent_ser(m, db), &format!("{m}-FSK SER"));
            assert_strictly_decreasing(&|db| mfsk_noncoherent_ber(m, db), &format!("{m}-FSK BER"));
        }
    }
}
