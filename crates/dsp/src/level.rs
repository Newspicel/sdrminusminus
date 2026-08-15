use num_complex::Complex;

use crate::iir::one_pole_coeff;

const DETECT_TAU_S: f64 = 3e-3;
const ATTACK_TAU_S: f64 = 5e-3;
const RELEASE_TAU_S: f64 = 150e-3;
const PEAK_HOLD_S: f64 = 1.0;
const PEAK_FALL_DB_PER_S: f32 = 20.0;

pub const LEVEL_FLOOR_DB: f32 = -140.0;

#[derive(Clone, Debug)]
pub struct LevelMeter {
    mean: f32,
    power: f32,
    detect: f32,
    attack: f32,
    release: f32,
    peak_db: f32,
    held: u64,
    hold_samples: u64,
    peak_fall_per_sample: f32,
}

impl LevelMeter {
    #[must_use]
    pub fn new(rate: f64) -> Self {
        assert!(rate > 0.0, "level meter needs a positive sample rate");
        Self {
            mean: 0.0,
            power: 0.0,
            detect: one_pole_coeff(rate, DETECT_TAU_S),
            attack: one_pole_coeff(rate, ATTACK_TAU_S),
            release: one_pole_coeff(rate, RELEASE_TAU_S),
            peak_db: LEVEL_FLOOR_DB,
            held: 0,
            hold_samples: (PEAK_HOLD_S * rate).round() as u64,
            peak_fall_per_sample: PEAK_FALL_DB_PER_S / rate as f32,
        }
    }

    pub fn process(&mut self, iq: &[Complex<f32>]) {
        if !self.power.is_finite() || !self.mean.is_finite() {
            self.mean = 0.0;
            self.power = 0.0;
        }
        for &x in iq {
            self.mean += self.detect * (x.norm_sqr() - self.mean);
            let coeff = if self.mean > self.power {
                self.attack
            } else {
                self.release
            };
            self.power += coeff * (self.mean - self.power);
        }
        let now = self.level_db();
        if now >= self.peak_db {
            self.peak_db = now;
            self.held = 0;
        } else {
            self.held += iq.len() as u64;
            if self.held > self.hold_samples {
                self.peak_db =
                    (self.peak_db - self.peak_fall_per_sample * iq.len() as f32).max(now);
            }
        }
    }

    #[must_use]
    pub fn level_db(&self) -> f32 {
        if !self.power.is_finite() || self.power <= 0.0 {
            return LEVEL_FLOOR_DB;
        }
        (10.0 * self.power.log10()).max(LEVEL_FLOOR_DB)
    }

    #[must_use]
    pub fn peak_db(&self) -> f32 {
        self.peak_db.max(LEVEL_FLOOR_DB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{XorShift32, complex_tone};

    const RATE: f64 = 48_000.0;

    fn tone_at_db(db: f32, len: usize) -> Vec<Complex<f32>> {
        let amp = 10f32.powf(db / 20.0);
        complex_tone(0.02, len).iter().map(|v| v * amp).collect()
    }

    #[test]
    fn a_steady_tone_reads_its_own_level() {
        let mut meter = LevelMeter::new(RATE);
        for _ in 0..40 {
            meter.process(&tone_at_db(-20.0, 480));
        }
        assert!(
            (meter.level_db() - -20.0).abs() < 0.5,
            "read {:.2} dB for a −20 dB tone",
            meter.level_db()
        );
    }

    #[test]
    fn a_fluctuating_signal_reads_its_level_and_not_its_sample_peaks() {
        let mut rng = XorShift32(0x1234_5678);
        let mut meter = LevelMeter::new(RATE);
        let mut power = 0.0f64;
        let mut samples = 0u64;
        for _ in 0..200 {
            let block: Vec<Complex<f32>> = (0..480)
                .map(|_| Complex::new(rng.next_f32() - 0.5, rng.next_f32() - 0.5) * 0.1)
                .collect();
            for x in &block {
                power += f64::from(x.norm_sqr());
                samples += 1;
            }
            meter.process(&block);
        }
        let truth = 10.0 * (power / samples as f64).log10();
        assert!(
            (f64::from(meter.level_db()) - truth).abs() < 2.0,
            "read {:.2} dB for a signal whose true level is {truth:.2} dB",
            meter.level_db()
        );
    }

    #[test]
    fn an_untouched_meter_reads_the_floor_rather_than_negative_infinity() {
        let meter = LevelMeter::new(RATE);
        assert_eq!(meter.level_db(), LEVEL_FLOOR_DB);
        assert_eq!(meter.peak_db(), LEVEL_FLOOR_DB);
        assert!(meter.level_db().is_finite());
    }

    #[test]
    fn it_attacks_faster_than_it_releases() {
        let mut meter = LevelMeter::new(RATE);
        meter.process(&tone_at_db(-10.0, 480));
        let risen = meter.level_db();
        assert!(risen > -20.0, "attack too slow: {risen:.2} dB after 10 ms");

        meter.process(&vec![Complex::new(0.0, 0.0); 480]);
        assert!(
            meter.level_db() > risen - 6.0,
            "release too fast: {:.2} dB from {risen:.2}",
            meter.level_db()
        );
    }

    #[test]
    fn the_peak_holds_above_a_signal_that_has_stopped() {
        let mut meter = LevelMeter::new(RATE);
        for _ in 0..40 {
            meter.process(&tone_at_db(-10.0, 480));
        }
        let peak = meter.peak_db();
        assert!((peak - -10.0).abs() < 0.5, "peak read {peak:.2} dB");

        for _ in 0..50 {
            meter.process(&vec![Complex::new(0.0, 0.0); 480]);
        }
        assert!(
            meter.level_db() < peak - 10.0,
            "level did not fall: {:.2} against a {peak:.2} peak",
            meter.level_db()
        );
        assert!(
            (meter.peak_db() - peak).abs() < 0.01,
            "peak fell during its hold: {:.2} from {peak:.2}",
            meter.peak_db()
        );
    }

    #[test]
    fn the_peak_falls_back_once_its_hold_has_passed() {
        let mut meter = LevelMeter::new(RATE);
        for _ in 0..40 {
            meter.process(&tone_at_db(-10.0, 480));
        }
        for _ in 0..300 {
            meter.process(&vec![Complex::new(0.0, 0.0); 480]);
        }
        assert!(
            meter.peak_db() < -20.0,
            "peak never decayed: {:.2}",
            meter.peak_db()
        );
        assert!(meter.peak_db() >= meter.level_db());
    }

    #[test]
    fn a_non_finite_sample_does_not_wedge_the_meter() {
        let mut meter = LevelMeter::new(RATE);
        meter.process(&[Complex::new(f32::NAN, 0.0); 16]);
        for _ in 0..40 {
            meter.process(&tone_at_db(-20.0, 480));
        }
        assert!(
            (meter.level_db() - -20.0).abs() < 0.5,
            "meter stayed wedged at {:.2} dB",
            meter.level_db()
        );
    }

    #[test]
    fn levels_are_ordered_the_way_the_signals_are() {
        let read = |db: f32| {
            let mut meter = LevelMeter::new(RATE);
            for _ in 0..40 {
                meter.process(&tone_at_db(db, 480));
            }
            meter.level_db()
        };
        assert!(read(-60.0) < read(-30.0));
        assert!(read(-30.0) < read(-10.0));
    }
}
