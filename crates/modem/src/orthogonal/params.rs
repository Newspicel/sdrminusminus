//! The tone plan: M equally spaced tones, and the one constraint that makes them a *set*.
//!
//! Orthogonality is not a property of the tones alone but of the tones and the symbol period
//! together: two tones `Δf` apart are orthogonal over `T` under noncoherent detection exactly
//! when `Δf·T` is a whole number (under coherent detection a half-integer suffices, but this
//! engine's detector never sees a phase, so the whole-integer condition is the one that binds).
//! [`MfskParams`] therefore carries the spacing in *cycles per symbol*, where the condition is
//! visible — `spacing = 1` is the tightest orthogonal plan and the one FT8 and most M-FSK
//! standards use — instead of in Hz, where it would depend on a sample rate the engine does not
//! have and cannot check.

use crate::{
    cpm::{CpmParams, Mapping},
    pulse::{self, Norm},
};

/// Tones a bank may carry. The bound is the exact noncoherent oracle's
/// (`ber::theory::mfsk_noncoherent_ser`, exact to M = 64): a plan this engine can measure but
/// the harness cannot reference is an entry with no acceptance, which §1.2 does not allow.
pub const MAX_TONES: usize = 64;

/// One orthogonal M-FSK tone plan.
#[derive(Clone, Debug, PartialEq)]
pub struct MfskParams {
    m: usize,
    sps: f64,
    spacing: f64,
}

impl MfskParams {
    /// `spacing` is the tone separation in cycles per symbol — see the module docs for why that
    /// unit and not Hz.
    ///
    /// # Panics
    /// If `m` is not a power of two in `2..=MAX_TONES`; if `sps` is not a whole number of at
    /// least two (the matched filter is one symbol of samples, so a fractional one would have
    /// no defined length); if `spacing` is not a whole number of at least one (the plan would
    /// not be orthogonal, which is the entry's whole premise); or if the outer tone reaches
    /// Nyquist, `spacing·(M−1) ≥ sps`.
    #[must_use]
    pub fn new(m: usize, sps: f64, spacing: f64) -> Self {
        assert!(
            m >= 2 && m.is_power_of_two() && m <= MAX_TONES,
            "M must be a power of two in 2..={MAX_TONES}, got {m}"
        );
        assert!(
            sps.is_finite() && sps >= 2.0 && (sps - sps.round()).abs() < 1e-9,
            "samples per symbol must be a whole number of at least two, got {sps}"
        );
        assert!(
            spacing.is_finite() && spacing >= 1.0 && (spacing - spacing.round()).abs() < 1e-9,
            "tone spacing must be a whole number of cycles per symbol, got {spacing}"
        );
        assert!(
            spacing * ((m - 1) as f64) < sps,
            "outer tones reach Nyquist: {spacing}·{} cycles/symbol at {sps} samples/symbol",
            m - 1
        );
        Self { m, sps, spacing }
    }

    /// The tightest orthogonal plan: one cycle per symbol between neighbours.
    ///
    /// # Panics
    /// As [`Self::new`].
    #[must_use]
    pub fn orthogonal(m: usize, sps: f64) -> Self {
        Self::new(m, sps, 1.0)
    }

    #[must_use]
    pub fn m(&self) -> usize {
        self.m
    }

    #[must_use]
    pub fn sps(&self) -> f64 {
        self.sps
    }

    /// Samples one symbol spans — the matched filter's length.
    #[must_use]
    pub fn window(&self) -> usize {
        self.sps as usize
    }

    #[must_use]
    pub fn spacing(&self) -> f64 {
        self.spacing
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> u32 {
        self.m.trailing_zeros()
    }

    /// Tone `index`'s frequency in cycles per sample, the plan centred on zero: index 0 is the
    /// lowest tone and index M−1 the highest, which is the natural-binary order every label in
    /// this engine (and the demapper it feeds) reads.
    #[must_use]
    pub fn tone_cycles_per_sample(&self, index: usize) -> f64 {
        self.spacing * (index as f64 - (self.m - 1) as f64 / 2.0) / self.sps
    }

    /// The same waveform as continuous-phase CPM: a rect frequency pulse at modulation index
    /// `h = spacing`, over the natural odd-integer level table. This is not a coincidence to
    /// be re-derived — an M-FSK tone plan with continuous phase *is* M-ary CPFSK, and stating
    /// it as `CpmParams` is what lets the continuous tier of this entry's modulator be the
    /// crate's one CPM modulator rather than a second implementation of it (§1.2).
    #[must_use]
    pub fn as_cpm(&self) -> CpmParams {
        CpmParams::from_h(
            Mapping::natural(self.m),
            self.spacing,
            pulse::rect(self.sps, Norm::Area),
            self.sps,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_are_centred_and_evenly_spaced() {
        let p = MfskParams::orthogonal(8, 16.0);
        let f: Vec<f64> = (0..8).map(|k| p.tone_cycles_per_sample(k)).collect();
        for pair in f.windows(2) {
            assert!((pair[1] - pair[0] - 1.0 / 16.0).abs() < 1e-12, "{f:?}");
        }
        assert!((f[0] + f[7]).abs() < 1e-12, "plan is not centred: {f:?}");
        assert!(f.iter().all(|c| c.abs() < 0.5), "past Nyquist: {f:?}");
    }

    /// The identity the continuous tier stands on: adjacent CPM levels are two apart and the
    /// per-level frequency is `h·L·baud/2`, so `h = spacing` puts the tones exactly where the
    /// plan says.
    #[test]
    fn the_cpm_spelling_puts_the_tones_where_the_plan_does() {
        for (m, spacing) in [(2, 1.0), (4, 1.0), (8, 1.0), (4, 2.0)] {
            let p = MfskParams::new(m, 16.0, spacing);
            let cpm = p.as_cpm();
            for k in 0..m {
                let level = f64::from(cpm.mapping().level(k as u8));
                let cycles_per_sample = cpm.h() * level / (2.0 * p.sps());
                assert!(
                    (cycles_per_sample - p.tone_cycles_per_sample(k)).abs() < 1e-12,
                    "M={m} spacing={spacing} tone {k}"
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "tone spacing must be a whole number")]
    fn a_non_orthogonal_spacing_is_refused() {
        let _ = MfskParams::new(4, 16.0, 0.5);
    }

    #[test]
    #[should_panic(expected = "outer tones reach Nyquist")]
    fn a_plan_that_aliases_is_refused() {
        let _ = MfskParams::new(8, 6.0, 1.0);
    }
}
