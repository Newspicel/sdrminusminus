use crate::{
    cpm::{CpmParams, Mapping},
    pulse::{self, Norm},
};

pub const MAX_TONES: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct MfskParams {
    m: usize,
    sps: f64,
    spacing: f64,
}

impl MfskParams {
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

    #[must_use]
    pub fn tone_cycles_per_sample(&self, index: usize) -> f64 {
        self.spacing * (index as f64 - (self.m - 1) as f64 / 2.0) / self.sps
    }

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
