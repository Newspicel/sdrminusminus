use num_complex::Complex;

use crate::iir::one_pole_coeff;

const POWER_TAU_S: f64 = 1e-3;

const FLOOR_FALL_TAU_S: f64 = 0.3;
const FLOOR_RISE_TAU_S: f64 = 10.0;
const FLOOR_WARMUP_S: f64 = 50e-3;
const AUTO_MIN_LIN: f32 = 1e-12;
const AUTO_MAX_LIN: f32 = 1.0;

#[derive(Clone, Debug)]
pub struct Squelch {
    power: f32,
    coeff: f32,
    threshold_db: f32,
    open_lin: f32,
    close_lin: f32,
    hysteresis_lin: f32,
    hold_samples: u64,
    below: u64,
    open: bool,
    floor: f32,
    floor_fall: f32,
    floor_rise: f32,
    warmup: u64,
    warmup_samples: u64,
    auto_margin_db: Option<f32>,
    auto_margin_lin: f32,
}

impl Squelch {
    #[must_use]
    pub fn new(rate: f64, threshold_db: f32, hysteresis_db: f32, hold_s: f32) -> Self {
        assert!(
            rate > 0.0 && hysteresis_db >= 0.0 && hold_s >= 0.0,
            "invalid squelch parameters"
        );
        let mut s = Self {
            power: 0.0,
            coeff: one_pole_coeff(rate, POWER_TAU_S),
            threshold_db,
            open_lin: 0.0,
            close_lin: 0.0,
            hysteresis_lin: db_to_power(-hysteresis_db),
            hold_samples: (f64::from(hold_s) * rate).round() as u64,
            below: 0,
            open: false,
            floor: 0.0,
            floor_fall: one_pole_coeff(rate, FLOOR_FALL_TAU_S),
            floor_rise: one_pole_coeff(rate, FLOOR_RISE_TAU_S),
            warmup: (FLOOR_WARMUP_S * rate).round() as u64,
            warmup_samples: (FLOOR_WARMUP_S * rate).round() as u64,
            auto_margin_db: None,
            auto_margin_lin: 1.0,
        };
        s.recompute_thresholds();
        s
    }

    pub fn set_threshold_db(&mut self, db: f32) {
        self.threshold_db = db;
        self.recompute_thresholds();
    }

    pub fn set_auto_margin_db(&mut self, margin_db: Option<f32>) {
        self.auto_margin_db = margin_db;
        self.auto_margin_lin = margin_db.map_or(1.0, db_to_power);
        self.recompute_thresholds();
    }

    #[must_use]
    pub fn threshold_db(&self) -> f32 {
        match self.auto_margin_db {
            Some(_) => 10.0 * self.open_lin.max(AUTO_MIN_LIN).log10(),
            None => self.threshold_db,
        }
    }

    pub fn reset(&mut self) {
        self.power = 0.0;
        self.floor = 0.0;
        self.warmup = self.warmup_samples;
        self.below = 0;
        self.open = false;
        self.recompute_thresholds();
    }

    fn recompute_thresholds(&mut self) {
        self.open_lin = match self.auto_margin_db {
            Some(_) => (self.floor * self.auto_margin_lin).clamp(AUTO_MIN_LIN, AUTO_MAX_LIN),
            None => db_to_power(self.threshold_db),
        };
        self.close_lin = self.open_lin * self.hysteresis_lin;
    }

    #[must_use]
    pub fn process(&mut self, iq: &[Complex<f32>]) -> bool {
        if !self.power.is_finite() || !self.floor.is_finite() {
            self.power = 0.0;
            self.floor = 0.0;
            self.warmup = self.warmup_samples;
        }
        let auto = self.auto_margin_db.is_some();
        for &x in iq {
            self.power += self.coeff * (x.norm_sqr() - self.power);
            if auto {
                self.track_floor();
            }
            if self.power >= self.open_lin {
                self.open = true;
                self.below = 0;
            } else if self.power < self.close_lin {
                self.below += 1;
                if self.below >= self.hold_samples {
                    self.open = false;
                }
            } else {
                self.below = 0;
            }
        }
        self.open
    }

    fn track_floor(&mut self) {
        if self.warmup > 0 {
            self.warmup -= 1;
            self.floor = self.power;
        } else if self.power < self.floor {
            self.floor += self.floor_fall * (self.power - self.floor);
        } else if !self.open {
            self.floor += self.floor_rise * (self.power - self.floor);
        }
        self.open_lin = (self.floor * self.auto_margin_lin).clamp(AUTO_MIN_LIN, AUTO_MAX_LIN);
        self.close_lin = self.open_lin * self.hysteresis_lin;
    }
}

fn db_to_power(db: f32) -> f32 {
    10f32.powf(db / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{XorShift32, complex_tone};

    const RATE: f64 = 48_000.0;
    const THRESHOLD_DB: f32 = -30.0;
    const HYSTERESIS_DB: f32 = 6.0;
    const HOLD_S: f32 = 0.1;

    fn squelch() -> Squelch {
        Squelch::new(RATE, THRESHOLD_DB, HYSTERESIS_DB, HOLD_S)
    }

    fn tone_at_db(db: f32, len: usize) -> Vec<Complex<f32>> {
        let amp = 10f32.powf(db / 20.0);
        complex_tone(0.02, len).iter().map(|v| v * amp).collect()
    }

    #[test]
    fn noise_floor_below_threshold_stays_closed() {
        let mut sq = squelch();
        let mut rng = XorShift32(0xdead_beef);
        let scale = (1e-6f32 / (2.0 / 3.0)).sqrt();
        for _ in 0..100 {
            let block: Vec<Complex<f32>> = (0..480)
                .map(|_| Complex::new(rng.next_f32(), rng.next_f32()) * scale)
                .collect();
            assert!(!sq.process(&block), "opened on noise floor");
        }
    }

    #[test]
    fn burst_above_threshold_opens() {
        let mut sq = squelch();
        assert!(sq.process(&tone_at_db(-10.0, 480)));
    }

    #[test]
    fn dithering_inside_hysteresis_band_never_chatters() {
        let mut sq = squelch();
        assert!(sq.process(&tone_at_db(-10.0, 480)));
        for i in 0..100 {
            let db = if i % 2 == 0 { -33.0 } else { -35.0 };
            assert!(sq.process(&tone_at_db(db, 480)), "closed at block {i}");
        }

        let mut sq = squelch();
        for i in 0..100 {
            let db = if i % 2 == 0 { -33.0 } else { -35.0 };
            assert!(!sq.process(&tone_at_db(db, 480)), "opened at block {i}");
        }
    }

    #[test]
    fn closes_only_after_hold_time() {
        let mut sq = squelch();
        assert!(sq.process(&tone_at_db(-10.0, 480)));
        let silence = vec![Complex::new(0.0f32, 0.0); 480];
        let mut states = Vec::new();
        for _ in 0..15 {
            states.push(sq.process(&silence));
        }
        assert!(states[8], "closed before the hold time");
        assert!(!states[12], "still open after the hold time");
    }

    #[test]
    fn recovers_after_non_finite_sample() {
        let mut sq = squelch();
        assert!(sq.process(&tone_at_db(-10.0, 480)));
        let mut poisoned = tone_at_db(-10.0, 480);
        poisoned[0] = Complex::new(f32::NAN, 0.0);
        let _ = sq.process(&poisoned);
        let silence = vec![Complex::new(0.0f32, 0.0); 480];
        let mut open = true;
        for _ in 0..15 {
            open = sq.process(&silence);
        }
        assert!(!open, "gate frozen open after NaN");

        let mut sq = squelch();
        let mut poisoned = tone_at_db(-60.0, 480);
        poisoned[0] = Complex::new(f32::NAN, f32::NAN);
        assert!(!sq.process(&poisoned));
        assert!(
            sq.process(&tone_at_db(-10.0, 480)),
            "gate frozen closed after NaN"
        );
    }

    #[test]
    fn set_threshold_moves_the_open_point() {
        let mut sq = squelch();
        assert!(!sq.process(&tone_at_db(-40.0, 4_800)));
        sq.set_threshold_db(-50.0);
        assert!(sq.process(&tone_at_db(-40.0, 4_800)));
    }

    const MARGIN_DB: f32 = 8.0;

    fn auto() -> Squelch {
        let mut sq = Squelch::new(RATE, 0.0, HYSTERESIS_DB, HOLD_S);
        sq.set_auto_margin_db(Some(MARGIN_DB));
        sq
    }

    #[test]
    fn the_threshold_settles_a_margin_above_the_noise_it_hears() {
        for noise_db in [-70.0f32, -50.0, -30.0] {
            let mut sq = auto();
            let mut open = true;
            for _ in 0..200 {
                open = sq.process(&tone_at_db(noise_db, 480));
            }
            assert!(!open, "gate open on the noise floor at {noise_db} dB");
            let landed = sq.threshold_db();
            assert!(
                (landed - (noise_db + MARGIN_DB)).abs() < 1.5,
                "threshold landed at {landed:.1} dB for a {noise_db} dB floor"
            );
        }
    }

    #[test]
    fn a_burst_above_the_learned_floor_opens_the_gate() {
        let mut sq = auto();
        for _ in 0..200 {
            assert!(!sq.process(&tone_at_db(-60.0, 480)));
        }
        assert!(sq.process(&tone_at_db(-40.0, 480)), "burst did not open it");
    }

    #[test]
    fn a_long_transmission_does_not_squelch_itself() {
        let mut sq = auto();
        for _ in 0..200 {
            let _ = sq.process(&tone_at_db(-60.0, 480));
        }
        assert!(sq.process(&tone_at_db(-40.0, 480)));
        for i in 0..3_000 {
            assert!(sq.process(&tone_at_db(-40.0, 480)), "closed at block {i}");
        }
    }

    #[test]
    fn the_threshold_follows_a_floor_that_creeps_up() {
        let mut sq = auto();
        for _ in 0..200 {
            let _ = sq.process(&tone_at_db(-80.0, 480));
        }
        for step in 0..12_000 {
            let db = -80.0 + 20.0 * step as f32 / 12_000.0;
            assert!(!sq.process(&tone_at_db(db, 480)), "opened on its own noise");
        }
        let landed = sq.threshold_db();
        assert!(
            (landed - (-60.0 + MARGIN_DB)).abs() < 1.5,
            "threshold stayed at {landed:.1} dB"
        );
    }

    #[test]
    fn auto_and_manual_thresholds_hand_over_to_each_other() {
        let mut sq = auto();
        for _ in 0..200 {
            let _ = sq.process(&tone_at_db(-60.0, 480));
        }
        let learned = sq.threshold_db();

        sq.set_auto_margin_db(None);
        sq.set_threshold_db(-90.0);
        assert_eq!(sq.threshold_db(), -90.0);
        assert!(
            sq.process(&tone_at_db(-60.0, 480)),
            "manual gate did not open"
        );

        sq.set_auto_margin_db(Some(MARGIN_DB));
        assert!(
            (sq.threshold_db() - learned).abs() < 1.0,
            "auto restarted at {:.1} instead of resuming {learned:.1}",
            sq.threshold_db()
        );
    }

    #[test]
    fn a_reset_forgets_the_floor_the_channel_taught_it() {
        let mut sq = auto();
        for _ in 0..200 {
            let _ = sq.process(&tone_at_db(-30.0, 480));
        }
        sq.reset();
        let mut open = true;
        for _ in 0..200 {
            open = sq.process(&tone_at_db(-70.0, 480));
        }
        assert!(!open);
        assert!(
            sq.threshold_db() < -50.0,
            "the old floor survived a reset: {:.1} dB",
            sq.threshold_db()
        );
    }

    #[test]
    fn an_automatic_gate_recovers_after_a_non_finite_sample() {
        let mut sq = auto();
        let mut poisoned = tone_at_db(-60.0, 480);
        poisoned[0] = Complex::new(f32::NAN, f32::NAN);
        let _ = sq.process(&poisoned);
        for _ in 0..200 {
            let _ = sq.process(&tone_at_db(-60.0, 480));
        }
        assert!(sq.threshold_db().is_finite());
        assert!(sq.process(&tone_at_db(-30.0, 480)), "gate wedged after NaN");
    }
}
