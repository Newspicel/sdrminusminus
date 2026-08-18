use crate::iir::one_pole_coeff;

const ATTACK_S: f64 = 3e-3;
const RELEASE_S: f64 = 13.5e-3;
const RIPPLE_TAU_S: f64 = ATTACK_S / 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Law {
    Compress,
    Expand,
}

#[derive(Clone, Debug)]
pub struct Compander {
    law: Law,
    reference: f32,
    min_level: f32,
    max_level: f32,
    power: f32,
    level: f32,
    gain: f32,
    power_coeff: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

fn from_db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

impl Compander {
    #[must_use]
    pub fn compressor(rate: f64, reference_rms: f32, range_db: f32, headroom_db: f32) -> Self {
        Self::new(rate, Law::Compress, reference_rms, range_db, headroom_db)
    }

    #[must_use]
    pub fn expander(rate: f64, reference_rms: f32, range_db: f32, headroom_db: f32) -> Self {
        Self::new(rate, Law::Expand, reference_rms, range_db, headroom_db)
    }

    fn new(rate: f64, law: Law, reference_rms: f32, range_db: f32, headroom_db: f32) -> Self {
        assert!(
            rate > 0.0 && reference_rms > 0.0 && range_db > 0.0 && headroom_db > 0.0,
            "compander parameters must be positive"
        );
        let span = match law {
            Law::Compress => 1.0,
            Law::Expand => 0.5,
        };
        Self {
            law,
            reference: reference_rms,
            min_level: reference_rms * from_db(-range_db * span),
            max_level: reference_rms * from_db(headroom_db * span),
            power: 0.0,
            level: 0.0,
            gain: 1.0,
            power_coeff: one_pole_coeff(rate, RIPPLE_TAU_S),
            attack_coeff: one_pole_coeff(rate, ATTACK_S),
            release_coeff: one_pole_coeff(rate, RELEASE_S),
        }
    }

    #[must_use]
    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn reset(&mut self) {
        self.power = 0.0;
        self.level = 0.0;
        self.gain = 1.0;
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        self.settle();
        for s in samples {
            *s *= self.follow(*s);
        }
    }

    pub fn process_keyed(&mut self, samples: &mut [f32], key: &[f32]) {
        self.settle();
        for (s, &k) in samples.iter_mut().zip(key) {
            *s *= self.follow(k);
        }
    }

    fn settle(&mut self) {
        if !self.power.is_finite() || !self.level.is_finite() || !self.gain.is_finite() {
            self.reset();
        }
    }

    fn follow(&mut self, key: f32) -> f32 {
        self.power += self.power_coeff * (key * key - self.power);
        let instant = self.power.max(0.0).sqrt();
        let coeff = if instant > self.level {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.level += coeff * (instant - self.level);
        let level = self.level.clamp(self.min_level, self.max_level);
        self.gain = match self.law {
            Law::Compress => (self.reference / level).sqrt(),
            Law::Expand => level / self.reference,
        };
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{real_tone, rms_r};

    const RATE: f64 = 48_000.0;
    const REFERENCE: f32 = 0.25;
    const RANGE_DB: f32 = 40.0;
    const HEADROOM_DB: f32 = 9.0;
    const LEN: usize = 48_000;

    fn expander() -> Compander {
        Compander::expander(RATE, REFERENCE, RANGE_DB, HEADROOM_DB)
    }

    fn compressor() -> Compander {
        Compander::compressor(RATE, REFERENCE, RANGE_DB, HEADROOM_DB)
    }

    fn tone_at(db_re_reference: f32) -> Vec<f32> {
        let amplitude = REFERENCE * from_db(db_re_reference) * std::f32::consts::SQRT_2;
        real_tone(1_000.0 / RATE, LEN)
            .iter()
            .map(|v| v * amplitude)
            .collect()
    }

    fn settled_db(x: &[f32]) -> f32 {
        20.0 * (rms_r(&x[x.len() * 3 / 4..]) / REFERENCE).log10()
    }

    #[test]
    fn the_reference_level_passes_through_either_law_untouched() {
        for mut law in [expander(), compressor()] {
            let mut x = tone_at(0.0);
            law.process(&mut x);
            let settled = settled_db(&x);
            assert!(settled.abs() < 0.3, "reference came out at {settled:.2} dB");
        }
    }

    #[test]
    fn the_expander_doubles_every_decibel_away_from_the_reference() {
        for sent in [-18.0f32, -12.0, -4.0, 3.0] {
            let mut x = tone_at(sent);
            expander().process(&mut x);
            let settled = settled_db(&x);
            assert!(
                (settled - 2.0 * sent).abs() < 0.4,
                "{sent} dB expanded to {settled:.2} dB, wanted {:.2}",
                2.0 * sent
            );
        }
    }

    #[test]
    fn the_compressor_halves_every_decibel_away_from_the_reference() {
        for sent in [-36.0f32, -20.0, -6.0, 6.0] {
            let mut x = tone_at(sent);
            compressor().process(&mut x);
            let settled = settled_db(&x);
            assert!(
                (settled - sent / 2.0).abs() < 0.4,
                "{sent} dB compressed to {settled:.2} dB, wanted {:.2}",
                sent / 2.0
            );
        }
    }

    #[test]
    fn compressing_and_then_expanding_gives_back_the_level_that_went_in() {
        for sent in [-30.0f32, -12.0, 0.0, 6.0] {
            let mut x = tone_at(sent);
            compressor().process(&mut x);
            expander().process(&mut x);
            let settled = settled_db(&x);
            assert!(
                (settled - sent).abs() < 0.5,
                "{sent} dB round-tripped to {settled:.2} dB"
            );
        }
    }

    #[test]
    fn a_compressed_pair_of_levels_comes_back_as_far_apart_as_it_started() {
        let round_trip = |db| {
            let mut x = tone_at(db);
            compressor().process(&mut x);
            let squeezed = settled_db(&x);
            expander().process(&mut x);
            (squeezed, settled_db(&x))
        };
        let (quiet_squeezed, quiet) = round_trip(-36.0);
        let (loud_squeezed, loud) = round_trip(4.0);
        assert!(
            ((loud_squeezed - quiet_squeezed) - 20.0).abs() < 1.0,
            "40 dB was carried as {:.2} dB",
            loud_squeezed - quiet_squeezed
        );
        assert!(
            ((loud - quiet) - 40.0).abs() < 1.0,
            "40 dB came back as {:.2} dB",
            loud - quiet
        );
    }

    #[test]
    fn expansion_stops_at_the_bottom_of_its_range() {
        let mut x = tone_at(-70.0);
        expander().process(&mut x);
        let settled = settled_db(&x);
        assert!(
            (settled - (-70.0 - RANGE_DB / 2.0)).abs() < 0.5,
            "a signal past the range floor was pushed to {settled:.2} dB"
        );
    }

    #[test]
    fn silence_stays_silent_and_the_gain_stays_finite() {
        for mut law in [expander(), compressor()] {
            let mut x = vec![0.0f32; LEN];
            law.process(&mut x);
            assert!(x.iter().all(|v| *v == 0.0), "silence did not stay silent");
            assert!(law.gain().is_finite());
        }
    }

    #[test]
    fn a_non_finite_sample_does_not_wedge_either_law() {
        for mut law in [expander(), compressor()] {
            let mut poisoned = vec![0.1f32; 480];
            poisoned[0] = f32::NAN;
            law.process(&mut poisoned);

            let mut x = tone_at(-10.0);
            law.process(&mut x);
            assert!(x.iter().all(|v| v.is_finite()), "still poisoned");
            assert!(law.gain().is_finite());
        }
    }

    #[test]
    fn a_key_input_sets_the_gain_that_the_audio_is_given() {
        let quiet = tone_at(-12.0);
        let mut audio = tone_at(0.0);
        expander().process_keyed(&mut audio, &quiet);
        let settled = settled_db(&audio);
        assert!(
            (settled - -12.0).abs() < 0.4,
            "the key gave {settled:.2} dB, wanted the -12 dB it asked for"
        );
    }

    #[test]
    fn a_level_that_rises_is_followed_faster_than_one_that_falls() {
        let mut law = expander();
        let quiet = (0.02 * RATE) as usize;
        let mut x = tone_at(-24.0);
        law.process(&mut x[..quiet]);
        let low = law.gain();

        let mut loud = tone_at(0.0);
        law.process(&mut loud[..quiet]);
        let risen = law.gain();
        assert!(risen > 4.0 * low, "attack too slow: {low} to {risen}");

        law.process(&mut x[..quiet]);
        assert!(
            law.gain() > risen / 4.0,
            "release too fast: {risen} to {}",
            law.gain()
        );
    }
}
