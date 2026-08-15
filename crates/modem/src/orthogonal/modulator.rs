use num_complex::Complex;

use super::params::MfskParams;
use crate::cpm::CpmMod;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TonePhase {
    Continuous,
    Independent,
}

pub struct MfskMod {
    params: MfskParams,
    policy: TonePhase,
    cpm: Option<CpmMod>,
}

impl MfskMod {
    #[must_use]
    pub fn new(params: MfskParams, policy: TonePhase) -> Self {
        let cpm = match policy {
            TonePhase::Continuous => Some(CpmMod::new(params.as_cpm())),
            TonePhase::Independent => None,
        };
        Self {
            params,
            policy,
            cpm,
        }
    }

    #[must_use]
    pub fn params(&self) -> &MfskParams {
        &self.params
    }

    #[must_use]
    pub fn policy(&self) -> TonePhase {
        self.policy
    }

    pub fn modulate(&mut self, symbols: &[u8], out: &mut Vec<Complex<f32>>) {
        match &mut self.cpm {
            Some(cpm) => cpm.modulate(symbols, out),
            None => self.tones(symbols, out),
        }
    }

    pub fn flush(&mut self, out: &mut Vec<Complex<f32>>) {
        if let Some(cpm) = &mut self.cpm {
            cpm.flush(out);
        }
    }

    fn tones(&self, symbols: &[u8], out: &mut Vec<Complex<f32>>) {
        let window = self.params.window();
        out.reserve(symbols.len() * window);
        for &symbol in symbols {
            let index = symbol as usize & (self.params.m() - 1);
            let step = std::f64::consts::TAU * self.params.tone_cycles_per_sample(index);
            for n in 0..window {
                let (sin, cos) = (step * n as f64).sin_cos();
                out.push(Complex::new(cos as f32, sin as f32));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(params: &MfskParams, policy: TonePhase, symbols: &[u8]) -> Vec<Complex<f32>> {
        let mut m = MfskMod::new(params.clone(), policy);
        let mut out = Vec::new();
        m.modulate(symbols, &mut out);
        m.flush(&mut out);
        out
    }

    #[test]
    fn both_policies_radiate_the_plan_symbol_by_symbol() {
        let params = MfskParams::orthogonal(8, 16.0);
        let symbols = [0u8, 7, 3, 4, 1, 6];
        for policy in [TonePhase::Continuous, TonePhase::Independent] {
            let w = wave(&params, policy, &symbols);
            for (k, &symbol) in symbols.iter().enumerate() {
                let at = k * 16 + 4;
                let step = (w[at + 1] * w[at].conj()).arg();
                let cycles = f64::from(step) / std::f64::consts::TAU;
                let expect = params.tone_cycles_per_sample(symbol as usize);
                assert!(
                    (cycles - expect).abs() < 1e-4,
                    "{policy:?} symbol {k}: {cycles} vs {expect}"
                );
            }
        }
    }

    #[test]
    fn the_continuous_policy_has_no_phase_step_at_a_boundary() {
        let params = MfskParams::orthogonal(8, 16.0);
        let symbols = [0u8, 7, 0, 7];
        let w = wave(&params, TonePhase::Continuous, &symbols);
        for boundary in [16usize, 32, 48] {
            let step =
                f64::from((w[boundary] * w[boundary - 1].conj()).arg()) / std::f64::consts::TAU;
            let plausible = (0..8).any(|k| (step - params.tone_cycles_per_sample(k)).abs() < 1e-4);
            assert!(plausible, "phase step {step} at sample {boundary}");
        }
        let independent = wave(&params, TonePhase::Independent, &symbols);
        let jump =
            f64::from((independent[16] * independent[15].conj()).arg()) / std::f64::consts::TAU;
        assert!(
            (0..8).all(|k| (jump - params.tone_cycles_per_sample(k)).abs() > 1e-3),
            "the independent policy must show a phase step, measured {jump}"
        );
    }

    #[test]
    fn symbol_k_occupies_samples_k_sps_onwards() {
        let params = MfskParams::orthogonal(4, 8.0);
        let w = wave(&params, TonePhase::Continuous, &[0, 3]);
        assert!(w.len() >= 16);
        for (k, symbol) in [0usize, 3].into_iter().enumerate() {
            let expect = params.tone_cycles_per_sample(symbol);
            for n in 0..7 {
                let at = k * 8 + n;
                let cycles = f64::from((w[at + 1] * w[at].conj()).arg()) / std::f64::consts::TAU;
                assert!(
                    (cycles - expect).abs() < 1e-4,
                    "symbol {k} sample {n}: {cycles} vs {expect}"
                );
            }
        }
    }

    #[test]
    fn tones_are_unit_amplitude_under_both_policies() {
        let params = MfskParams::orthogonal(4, 8.0);
        for policy in [TonePhase::Continuous, TonePhase::Independent] {
            let w = wave(&params, policy, &[1, 2, 0, 3]);
            for (i, s) in w.iter().take(32).enumerate() {
                assert!((s.norm() - 1.0).abs() < 1e-5, "{policy:?} sample {i}: {s}");
            }
        }
    }
}
