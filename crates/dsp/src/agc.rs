use crate::iir::one_pole_coeff;

#[derive(Clone, Debug)]
pub struct Agc {
    target: f32,
    max_gain: f32,
    gain: f32,
    /// Smoothed mean-square power of the input.
    env: f32,
    env_coeff: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

impl Agc {
    #[must_use]
    pub fn new(rate: f64, target_rms: f32, attack_s: f32, release_s: f32, max_gain: f32) -> Self {
        assert!(
            rate > 0.0 && target_rms > 0.0 && attack_s > 0.0 && release_s > 0.0 && max_gain > 0.0,
            "agc parameters must be positive"
        );
        // The envelope only has to average over an audio cycle; the user-facing dynamics are
        // the gain time constants, so keep it fast relative to both.
        let env_tau = f64::from(attack_s.min(release_s)).clamp(4e-4, 8e-3) / 4.0;
        Self {
            target: target_rms,
            max_gain,
            gain: 1.0,
            env: 0.0,
            env_coeff: one_pole_coeff(rate, env_tau),
            attack_coeff: one_pole_coeff(rate, f64::from(attack_s)),
            release_coeff: one_pole_coeff(rate, f64::from(release_s)),
        }
    }

    #[must_use]
    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        // A non-finite sample latches `env` (NaN reads as rms 0 → gain pinned at max even
        // after the input recovers); heal per block so the fault costs one block.
        if !self.env.is_finite() {
            self.env = 0.0;
        }
        if !self.gain.is_finite() {
            self.gain = 1.0;
        }
        for s in samples {
            self.env += self.env_coeff * (*s * *s - self.env);
            let rms = self.env.max(0.0).sqrt();
            let desired = if rms > f32::MIN_POSITIVE {
                (self.target / rms).min(self.max_gain)
            } else {
                self.max_gain
            };
            let coeff = if desired < self.gain {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.gain += coeff * (desired - self.gain);
            *s *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{real_tone, rms_r};

    const RATE: f64 = 48_000.0;
    const TARGET: f32 = 0.25;
    const ATTACK: f32 = 0.005;
    const RELEASE: f32 = 0.05;
    const MAX_GAIN: f32 = 100.0;

    fn agc() -> Agc {
        Agc::new(RATE, TARGET, ATTACK, RELEASE, MAX_GAIN)
    }

    fn within_3_db_of_target(rms: f32) -> bool {
        let lo = TARGET * 10f32.powf(-3.0 / 20.0);
        let hi = TARGET * 10f32.powf(3.0 / 20.0);
        (lo..hi).contains(&rms)
    }

    #[test]
    fn quiet_tone_converges_within_4_release_time_constants() {
        let mut agc = agc();
        let mut x: Vec<f32> = real_tone(1_000.0 / RATE, 48_000)
            .iter()
            .map(|v| v * 0.01 * std::f32::consts::SQRT_2)
            .collect();
        agc.process(&mut x);
        let four_tau = (4.0 * RELEASE * RATE as f32) as usize;
        let rms = rms_r(&x[four_tau..four_tau + 2_400]);
        assert!(within_3_db_of_target(rms), "rms {rms} after 4 release tau");
    }

    #[test]
    fn sudden_loud_tone_pulled_down_within_attack_window() {
        let mut agc = agc();
        let mut quiet: Vec<f32> = real_tone(1_000.0 / RATE, 48_000)
            .iter()
            .map(|v| v * 0.01 * std::f32::consts::SQRT_2)
            .collect();
        agc.process(&mut quiet);

        // Step to −6 dBFS RMS; gain must collapse from ~25 on the attack constant, not the
        // release constant (which would still be ~11x at this point).
        let mut loud: Vec<f32> = real_tone(1_000.0 / RATE, 9_600)
            .iter()
            .map(|v| v * 0.5 * std::f32::consts::SQRT_2)
            .collect();
        agc.process(&mut loud);
        let window = (8.0 * ATTACK * RATE as f32) as usize;
        let rms = rms_r(&loud[window..window + 480]);
        assert!(within_3_db_of_target(rms), "rms {rms} after attack window");
    }

    #[test]
    fn recovers_after_non_finite_sample() {
        let mut agc = agc();
        let mut poisoned = vec![0.1f32; 480];
        poisoned[0] = f32::NAN;
        agc.process(&mut poisoned);

        // A loud tone after the fault must converge back to target — not stay pinned at
        // max_gain by a latched NaN envelope.
        let mut loud: Vec<f32> = real_tone(1_000.0 / RATE, 9_600)
            .iter()
            .map(|v| v * 0.5 * std::f32::consts::SQRT_2)
            .collect();
        agc.process(&mut loud);
        assert!(loud.iter().all(|v| v.is_finite()), "state still poisoned");
        assert!(agc.gain().is_finite());
        let rms = rms_r(&loud[4_800..7_200]);
        assert!(within_3_db_of_target(rms), "rms {rms} after recovery");
    }

    #[test]
    fn silence_stays_silent_and_gain_clamps() {
        let mut agc = agc();
        let mut x = vec![0.0f32; 20_000];
        agc.process(&mut x);
        assert!(x.iter().all(|v| *v == 0.0), "silence must stay silent");
        assert!(agc.gain().is_finite());
        assert!(
            (0.99 * MAX_GAIN..=MAX_GAIN).contains(&agc.gain()),
            "gain {} not clamped at max",
            agc.gain()
        );
    }
}
