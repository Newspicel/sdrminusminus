use num_complex::Complex;

use crate::iir::one_pole_coeff;

/// Fast enough to catch short bursts, slow enough to ride over single-sample spikes.
const POWER_TAU_S: f64 = 1e-3;

#[derive(Clone, Debug)]
pub struct Squelch {
    power: f32,
    coeff: f32,
    threshold_db: f32,
    hysteresis_db: f32,
    open_lin: f32,
    close_lin: f32,
    hold_samples: u64,
    below: u64,
    open: bool,
}

impl Squelch {
    /// `threshold_db`/`hysteresis_db` are dBFS of smoothed power (unit-magnitude IQ = 0 dBFS).
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
            hysteresis_db,
            open_lin: 0.0,
            close_lin: 0.0,
            hold_samples: (f64::from(hold_s) * rate).round() as u64,
            below: 0,
            open: false,
        };
        s.recompute_thresholds();
        s
    }

    pub fn set_threshold_db(&mut self, db: f32) {
        self.threshold_db = db;
        self.recompute_thresholds();
    }

    fn recompute_thresholds(&mut self) {
        self.open_lin = db_to_power(self.threshold_db);
        self.close_lin = db_to_power(self.threshold_db - self.hysteresis_db);
    }

    /// Opens above threshold; closes only after the smoothed power has stayed below
    /// threshold − hysteresis for the whole hold time. Returns the state after this block.
    #[must_use]
    pub fn process(&mut self, iq: &[Complex<f32>]) -> bool {
        // NaN power fails both comparisons, freezing the gate in whatever state it was in;
        // heal per block so one bad sample cannot silence (or jam open) a channel forever.
        if !self.power.is_finite() {
            self.power = 0.0;
        }
        for &x in iq {
            self.power += self.coeff * (x.norm_sqr() - self.power);
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
        // Open state: levels wandering in (threshold − hyst, threshold) must not close it.
        let mut sq = squelch();
        assert!(sq.process(&tone_at_db(-10.0, 480)));
        for i in 0..100 {
            let db = if i % 2 == 0 { -33.0 } else { -35.0 };
            assert!(sq.process(&tone_at_db(db, 480)), "closed at block {i}");
        }

        // Closed state: the same dithering must not open it.
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
        // 10 ms blocks: still open through 90 ms, closed by 130 ms (hold 100 ms + ~6 ms for
        // the smoothed power to decay below the close threshold).
        assert!(states[8], "closed before the hold time");
        assert!(!states[12], "still open after the hold time");
    }

    #[test]
    fn recovers_after_non_finite_sample() {
        // Open gate: a NaN followed by sustained silence must still close it.
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

        // Closed gate: a NaN followed by a strong carrier must still open it.
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
}
