//! The Nyquist amplitude-pulse pair for linear modulation: raised cosine and its root. Both
//! are truncated infinite-support designs on the centred odd-tap grid (module docs), with
//! `span` counted in symbols *each side* of the peak — `sdrmm_dsp::design_rrc`'s convention,
//! inherited so the wrap below stays bit-identical under [`Norm::Area`].

use sdrmm_dsp::design_rrc;

use super::{Norm, normalise, renorm_designed};

/// Raised cosine — *the* ISI-free Nyquist pulse: `h(t) = sinc(t/T)·cos(παt/T)/(1−(2αt/T)²)`
/// (Proakis, *Digital Communications*, raised-cosine spectrum), zero at every nonzero symbol
/// instant. No transmitter in the catalog sends it whole; it exists because a matched TX/RX
/// RRC pair multiplies to it in frequency, so it is the closed-form reference the RRC cascade
/// is tested against — and the analysis pulse for anything needing a known-ISI-free cascade.
///
/// `span` symbols each side of the peak; `2·span·sps` taps rounded up to odd for a true
/// centre tap.
#[must_use]
pub fn raised_cosine(sps: f64, alpha: f64, span: usize, norm: Norm) -> Vec<f32> {
    assert!(sps > 1.0, "need more than one sample per symbol");
    assert!(
        alpha > 0.0 && alpha <= 1.0,
        "roll-off must be in (0, 1]: {alpha}"
    );
    assert!(span >= 1, "span must cover at least one symbol");
    let mut taps = (2.0 * span as f64 * sps).round() as usize;
    if taps.is_multiple_of(2) {
        taps += 1;
    }
    let centre = (taps - 1) as f64 / 2.0;
    let h: Vec<f64> = (0..taps)
        .map(|k| rc_pulse((k as f64 - centre) / sps, alpha))
        .collect();
    normalise(h, norm)
}

/// Root-raised cosine — the half each end of a link applies so the *cascade* is the ISI-free
/// [`raised_cosine`]. The catalog's workhorse: DMR/dPMR/NXDN C4FM use α = 0.2
/// (ETSI TS 102 361-1, TIA-102.BAAA), TETRA π/4-DQPSK uses α = 0.35 (EN 300 392-2), M17 uses
/// α = 0.5.
///
/// Wraps `sdrmm_dsp::design_rrc` (§1 minimal duplication): under [`Norm::Area`] the taps are
/// its output bit for bit — the DC-gain normalisation live channels already rely on — and
/// [`Norm::Energy`] rescales that same shape. `span` symbols each side, `2·span·sps` taps
/// rounded up to odd.
#[must_use]
pub fn root_raised_cosine(sps: f64, alpha: f64, span: usize, norm: Norm) -> Vec<f32> {
    renorm_designed(design_rrc(sps, alpha, span), norm)
}

/// The RC impulse response at `t` symbol periods. The removable singularity at
/// `t = ±1/(2α)` — where cos and the denominator vanish together — evaluates by L'Hôpital to
/// `(π/4)·sinc(1/(2α))`.
fn rc_pulse(t: f64, alpha: f64) -> f64 {
    let t0 = 1.0 / (2.0 * alpha);
    if (t.abs() - t0).abs() < 1e-9 {
        return std::f64::consts::FRAC_PI_4 * sinc(t0);
    }
    sinc(t) * (std::f64::consts::PI * alpha * t).cos() / (1.0 - (2.0 * alpha * t).powi(2))
}

// Private in `sdrmm_dsp::fir`, so restated here; five lines, not a fork of design math.
fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convolve(a: &[f32], b: &[f32]) -> Vec<f64> {
        let mut out = vec![0.0f64; a.len() + b.len() - 1];
        for (i, &x) in a.iter().enumerate() {
            for (j, &y) in b.iter().enumerate() {
                out[i + j] += f64::from(x) * f64::from(y);
            }
        }
        out
    }

    /// The wrap is a wrap: same call, same bits. A reimplementation that merely agreed to a
    /// tolerance would be exactly the fork §1 forbids.
    #[test]
    fn rrc_under_area_norm_is_bit_identical_to_design_rrc() {
        for (sps, alpha, span) in [(4.0, 0.2, 8), (8.0, 0.35, 6), (10.0, 0.5, 8)] {
            assert_eq!(
                root_raised_cosine(sps, alpha, span, Norm::Area),
                design_rrc(sps, alpha, span),
                "sps={sps} alpha={alpha} span={span}"
            );
        }
    }

    /// §7 phase-2 acceptance: TX-RRC ⊗ RX-RRC is Nyquist. With unit-energy taps the cascade
    /// peak is Σh² = 1 exactly, and every other symbol instant must sit below 1e-3 of it.
    /// What remains at the instants is span-truncation ISI, concentrated at the edge symbol
    /// m = span (measured for α = 0.2, sps = 4: 5.5e-3 at span 8, 9.7e-4 at 16, 1.7e-4 at
    /// 20, while interior symbols sit near 1e-5) — a wrong RRC formula would instead leave
    /// percent-level ISI at *every* instant, at any span. Span 20 keeps the gate about the
    /// design math with margin, not about one truncation choice.
    #[test]
    fn tx_rrc_through_rx_rrc_is_isi_free_at_symbol_instants() {
        const SPAN: usize = 20;
        for alpha in [0.2, 0.35, 0.5] {
            for sps in [4usize, 8, 10] {
                let h = root_raised_cosine(sps as f64, alpha, SPAN, Norm::Energy);
                let cascade = convolve(&h, &h);
                let centre = cascade.len() / 2;
                let peak = cascade[centre];
                assert!(
                    (peak - 1.0).abs() < 1e-4,
                    "alpha={alpha} sps={sps}: matched-filter peak {peak}"
                );
                for m in 1..=2 * SPAN {
                    let off = m * sps;
                    if off > centre {
                        break;
                    }
                    for isi in [cascade[centre - off], cascade[centre + off]] {
                        assert!(
                            (isi / peak).abs() < 1e-3,
                            "alpha={alpha} sps={sps}: ISI {isi:.2e} at symbol {m}"
                        );
                    }
                }
            }
        }
    }

    /// The cascade does not just have Nyquist zeros — it is the raised cosine, sample for
    /// sample. Two span-20 RRCs convolve to span 40, the same grid as
    /// `raised_cosine(span=40)`, so the comparison is pointwise. The tolerance is the RRC
    /// tail-truncation error at the cascade's edge (spans 8/12 measured right at 1e-3).
    #[test]
    fn rrc_cascade_is_the_raised_cosine_closed_form() {
        let (sps, alpha, span) = (8.0, 0.35, 20usize);
        let h = root_raised_cosine(sps, alpha, span, Norm::Energy);
        let cascade = convolve(&h, &h);
        let rc = raised_cosine(sps, alpha, 2 * span, Norm::Area);
        assert_eq!(cascade.len(), rc.len());
        let centre = rc.len() / 2;
        let rc_peak = f64::from(rc[centre]);
        let peak = cascade[centre];
        for (k, (&r, c)) in rc.iter().zip(&cascade).enumerate() {
            let err = f64::from(r) / rc_peak - c / peak;
            assert!(
                err.abs() < 1e-3,
                "tap {k}: cascade differs from RC by {err:.2e}"
            );
        }
    }

    /// The defining property, straight from the formula: `sinc(m) = 0` for integer m ≠ 0.
    /// α = 0.5 with sps = 4 puts the removable singularity (t = 1) exactly on a symbol
    /// instant, so the L'Hôpital branch is exercised where it must also produce zero.
    #[test]
    fn raised_cosine_is_zero_at_every_other_symbol_instant() {
        for (sps, alpha) in [(8usize, 0.35), (4, 0.5)] {
            let rc = raised_cosine(sps as f64, alpha, 6, Norm::Area);
            let centre = rc.len() / 2;
            let peak = f64::from(rc[centre]);
            for m in 1..=6 {
                let off = m * sps;
                for tap in [rc[centre - off], rc[centre + off]] {
                    assert!(
                        (f64::from(tap) / peak).abs() < 1e-9,
                        "sps={sps} alpha={alpha}: RC not zero at symbol {m}"
                    );
                }
            }
        }
    }

    /// α = 0.4 at sps = 4 lands a tap exactly on the singularity t = 1/(2α) = 1.25 *between*
    /// symbol instants, where the limit is nonzero: (π/4)·sinc(1.25) = −√2/10. Normalisation
    /// cancels in the ratio to the centre tap (h(0) = 1), so the constant is checked directly.
    #[test]
    fn the_singular_tap_takes_its_closed_form_limit() {
        let rc = raised_cosine(4.0, 0.4, 6, Norm::Area);
        let centre = rc.len() / 2;
        let ratio = f64::from(rc[centre + 5]) / f64::from(rc[centre]);
        let expected = -(2.0f64.sqrt()) / 10.0;
        assert!(
            (ratio - expected).abs() < 1e-6,
            "singular tap ratio {ratio} vs {expected}"
        );
    }
}
