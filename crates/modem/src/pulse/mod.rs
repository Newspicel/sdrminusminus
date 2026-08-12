//! Pulse-shape design (MODEM-PLAN §3.1 `pulse/`) — the one place a pulse shape is defined.
//! Modulators, matched filters and `testgen` all draw their taps from here, which is what keeps
//! a modulator and its demodulator matched to the *same* pulse instead of two implementations
//! that drift apart (§1.2). Where `sdrmm_dsp` already designs a shape (`design_rrc`,
//! `design_gaussian`), the constructor here wraps and re-normalises it — never re-derives the
//! closed form — and a bit-identity test pins the wrap (§1 minimal duplication).
//!
//! **Design is cold path.** Constructors allocate freely and compute in `f64`; taps are
//! designed once at engine construction and only *consumed* by the hot path. The §4.2
//! zero-allocation gate applies to `process()`, not to this module.
//!
//! **Two normalisations, explicit at every call site.** A tap vector answers one of two
//! questions, and the two differ by a factor no test downstream of an Eb/N0 sweep would ever
//! localise to the pulse, so [`Norm`] forces the caller to say which one it means:
//!
//! - [`Norm::Energy`], `Σ h[n]² = 1` — the crate-root convention for linear-modulation pulses
//!   and matched filters: a transmitted symbol's energy is its constellation point's squared
//!   magnitude, and the matched-filter cascade peaks at exactly 1, so every Eb/N0 in
//!   [`crate::ber`] means the same thing across entries.
//! - [`Norm::Area`], `Σ h[n] = 1` — unit DC gain, `sdrmm_dsp`'s convention for `design_rrc` and
//!   `design_gaussian`: a filter that preserves the level of what it filters (what a
//!   discriminator-fed level estimate relies on), and the CPM frequency-pulse convention — with
//!   unit area, the phase pulse [`phase_pulse`] reaches q = ½, fixing the per-symbol phase step
//!   at π·h (see `cpm.rs`).
//!
//! **Two sampling grids**, matching what each family of shapes is for:
//!
//! - Infinite-support Nyquist pulses (RC, RRC, the Gaussian premod filter) are truncated,
//!   *centred* designs with an odd tap count and a true centre tap — `sdrmm_dsp::fir`'s
//!   convention, kept so the wrapped designs pass through untouched under [`Norm::Area`].
//! - Finite-support pulses (rect, half-sine, LREC, LRC and the composed Gaussian frequency
//!   pulse) cover exactly their `[0, L·T]` support. The closed-support family samples at
//!   interval midpoints, `t = (k + ½)/sps` — symmetric for any tap count, no wasted zero
//!   endpoint taps, and the Riemann sum a frequency pulse's integral wants.
//!
//! Design math is `f64` throughout; taps are `f32` (CLAUDE.md: f32 signals, f64 design math).

mod cpm;
mod nyquist;

pub use cpm::{gaussian, gaussian_freq, half_sine, lrc, lrec, phase_pulse, rect};
pub use nyquist::{raised_cosine, root_raised_cosine};

/// Which sum of the taps is fixed to 1. See the module docs for why the choice is always the
/// caller's: the two conventions answer different questions (symbol energy vs level/phase
/// gain), and getting one while meaning the other is a silent decibel-scale error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Norm {
    /// `Σ h[n]² = 1`. For linear-modulation shaping and matched filters (crate-root
    /// convention): symbol energy equals the constellation point's |·|², matched-filter peak
    /// is exactly 1.
    Energy,
    /// `Σ h[n] = 1` (unit DC gain). For level-preserving filters and CPM frequency pulses:
    /// the cumulative phase pulse reaches q = ½, so a full-response symbol advances the
    /// carrier phase by exactly π·h.
    Area,
}

/// Scales a designed `f64` shape to the requested normalisation and rounds to `f32` once, at
/// the very end — one rounding step, so both normalisations of a shape are exact scalings of
/// each other.
fn normalise(mut h: Vec<f64>, norm: Norm) -> Vec<f32> {
    let scale = match norm {
        Norm::Energy => h.iter().map(|v| v * v).sum::<f64>().sqrt().recip(),
        Norm::Area => h.iter().sum::<f64>().recip(),
    };
    // Every shape in this module has strictly positive energy and area by construction, so a
    // non-finite scale is an internal defect, not a caller error.
    assert!(
        scale.is_finite(),
        "pulse shape has no energy/area to normalise"
    );
    for v in &mut h {
        *v *= scale;
    }
    h.into_iter().map(|v| v as f32).collect()
}

/// Re-normalises taps that arrive from an `sdrmm_dsp::fir::design_*` function, which all
/// normalise to unit DC gain. Under [`Norm::Area`] the taps pass through *untouched* — that is
/// what makes the constructor provably a wrap and not a fork, pinned by bit-identity tests.
fn renorm_designed(taps: Vec<f32>, norm: Norm) -> Vec<f32> {
    match norm {
        Norm::Area => taps,
        Norm::Energy => {
            let energy: f64 = taps.iter().map(|&h| f64::from(h) * f64::from(h)).sum();
            let scale = energy.sqrt().recip();
            taps.iter()
                .map(|&h| (f64::from(h) * scale) as f32)
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Entry = (&'static str, Box<dyn Fn(Norm) -> Vec<f32>>);

    /// Every public pulse at representative parameters, integer and fractional sps both —
    /// the §7 phase-2 acceptance assertions run over this table, so a pulse added without
    /// joining it is a review catch, not a silent gap.
    fn catalog() -> Vec<Entry> {
        vec![
            ("rect", Box::new(|n| rect(8.0, n))),
            ("half_sine", Box::new(|n| half_sine(10.0, n))),
            ("lrec(3)", Box::new(|n| lrec(8.0, 3, n))),
            ("lrc(2)", Box::new(|n| lrc(8.0, 2, n))),
            ("lrc(2) fractional sps", Box::new(|n| lrc(6.4, 2, n))),
            (
                "raised_cosine",
                Box::new(|n| raised_cosine(8.0, 0.35, 6, n)),
            ),
            (
                "root_raised_cosine",
                Box::new(|n| root_raised_cosine(8.0, 0.2, 8, n)),
            ),
            ("gaussian", Box::new(|n| gaussian(8.0, 0.5, 3, n))),
            (
                "gaussian_freq",
                Box::new(|n| gaussian_freq(10.0, 0.3, 4, n)),
            ),
        ]
    }

    #[test]
    fn every_pulse_is_unit_energy_under_energy_norm() {
        for (name, build) in catalog() {
            let taps = build(Norm::Energy);
            let energy: f64 = taps.iter().map(|&h| f64::from(h) * f64::from(h)).sum();
            assert!((energy - 1.0).abs() < 1e-5, "{name}: Σh² = {energy}");
        }
    }

    #[test]
    fn every_pulse_is_unit_area_under_area_norm() {
        for (name, build) in catalog() {
            let taps = build(Norm::Area);
            let area: f64 = taps.iter().map(|&h| f64::from(h)).sum();
            assert!((area - 1.0).abs() < 1e-5, "{name}: Σh = {area}");
        }
    }

    #[test]
    fn the_two_normalisations_are_exact_scalings_of_one_shape() {
        for (name, build) in catalog() {
            let e = build(Norm::Energy);
            let a = build(Norm::Area);
            assert_eq!(e.len(), a.len(), "{name}");
            // The shared ratio is read off the largest tap for numeric headroom.
            let peak = e
                .iter()
                .enumerate()
                .max_by(|(_, x), (_, y)| x.abs().total_cmp(&y.abs()))
                .map(|(i, _)| i)
                .unwrap();
            let ratio = f64::from(a[peak]) / f64::from(e[peak]);
            for (&he, &ha) in e.iter().zip(&a) {
                let err = f64::from(ha) - f64::from(he) * ratio;
                assert!(err.abs() < 1e-6, "{name}: shapes diverge by {err}");
            }
        }
    }
}
