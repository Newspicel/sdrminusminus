use std::f64::consts::TAU;

use num_complex::Complex;

use crate::manifold::{Direction, LIGHT_SPEED_M_S};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneEcho {
    pub source: usize,
    pub direction: Direction,
    pub gain_db: f32,
    pub delay_samples: f64,
    pub doppler_hz: f64,
}

impl SceneEcho {
    #[must_use]
    pub const fn new(
        source: usize,
        direction: Direction,
        gain_db: f32,
        delay_samples: f64,
        doppler_hz: f64,
    ) -> Self {
        Self {
            source,
            direction,
            gain_db,
            delay_samples,
            doppler_hz,
        }
    }

    #[must_use]
    pub fn bistatic(
        source: usize,
        direction: Direction,
        gain_db: f32,
        excess_range_m: f64,
        range_rate_mps: f64,
        sample_rate: f64,
        carrier_hz: f64,
    ) -> Self {
        Self::new(
            source,
            direction,
            gain_db,
            excess_range_m / LIGHT_SPEED_M_S * sample_rate,
            -range_rate_mps * carrier_hz / LIGHT_SPEED_M_S,
        )
    }

    #[must_use]
    pub fn delay_at(&self, n: i64, carrier_hz: f64) -> f64 {
        self.delay_samples - self.doppler_hz * n as f64 / carrier_hz
    }

    #[must_use]
    pub fn rotation(&self, n: i64, sample_rate: f64) -> Complex<f64> {
        let cycles = (self.doppler_hz * n as f64 / sample_rate).rem_euclid(1.0);
        Complex::from_polar(10f64.powf(f64::from(self.gain_db) / 20.0), TAU * cycles)
    }

    pub(super) fn is_valid(&self) -> bool {
        self.delay_samples.is_finite()
            && self.doppler_hz.is_finite()
            && self.gain_db.is_finite()
            && self.direction.azimuth_deg.is_finite()
            && self.direction.elevation_deg.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::{Geometry, Winding, steer};
    use crate::scene::{ArrayScene, SceneSignal, SceneSource};

    const FREQ: f64 = 433.92e6;
    const FS: f64 = 1e6;

    fn kraken() -> Geometry {
        Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap()
    }

    fn direct(signal: SceneSignal) -> ArrayScene {
        ArrayScene::new(kraken(), FREQ, FS)
            .with_source(SceneSource::new(Direction::horizon(137.0), 0.0, signal))
            .with_seed(7)
    }

    fn echo_only(
        mut with: ArrayScene,
        mut without: ArrayScene,
        len: usize,
    ) -> Vec<Vec<Complex<f32>>> {
        let with = with.render(len).unwrap();
        let without = without.render(len).unwrap();
        with.iter()
            .zip(&without)
            .map(|(a, b)| a.iter().zip(b).map(|(x, y)| x - y).collect())
            .collect()
    }

    fn inner(a: &[Complex<f32>], b: &[Complex<f32>]) -> f32 {
        let dot: Complex<f32> = a.iter().zip(b).map(|(x, y)| x.conj() * y).sum();
        let norms = a.iter().map(Complex::norm_sqr).sum::<f32>()
            * b.iter().map(Complex::norm_sqr).sum::<f32>();
        dot.norm() / norms.sqrt()
    }

    #[test]
    fn an_echo_at_the_emitter_azimuth_matches_the_source_steering() {
        let tone = SceneSignal::Tone { offset_hz: 2e4 };
        let echo = SceneEcho::new(0, Direction::horizon(137.0), -10.0, 10.5, 50.0);
        let lanes = echo_only(direct(tone).with_echo(echo), direct(tone), 2000);
        let mut source = direct(tone);
        let plain = source.render(2000).unwrap();
        let mut steering = [Complex::new(0.0f32, 0.0); 5];
        steer(
            kraken().positions(),
            FREQ + 2e4,
            Direction::horizon(137.0),
            &mut steering,
        );
        for n in [200, 777, 1999] {
            let snapshot: Vec<Complex<f32>> = lanes.iter().map(|lane| lane[n]).collect();
            let emitted: Vec<Complex<f32>> = plain.iter().map(|lane| lane[n]).collect();
            assert!(inner(&steering, &snapshot) > 0.9999, "{n}");
            assert!(inner(&emitted, &snapshot) > 0.9999, "{n}");
            let power: f32 = snapshot.iter().map(Complex::norm_sqr).sum::<f32>() / 5.0;
            assert!((10.0 * power.log10() + 10.0).abs() < 0.01);
        }
        let step = lanes[0][1001] / lanes[0][1000];
        let expected = TAU * (2e4 + 50.0) / FS;
        assert!((f64::from(step.arg()) - expected).abs() < 1e-4);
    }

    #[test]
    fn an_echo_is_the_source_delayed_and_shifted() {
        let noise = SceneSignal::Noise {
            offset_hz: 0.0,
            bandwidth_hz: 3e5,
        };
        let echo = SceneEcho::new(0, Direction::horizon(20.0), 0.0, 37.25, 0.0);
        let lanes = echo_only(direct(noise).with_echo(echo), direct(noise), 20_000);
        let plain = direct(noise).render(20_000).unwrap();
        let score = |lag: usize| {
            (2000..19_000)
                .map(|n| lanes[0][n] * plain[0][n - lag].conj())
                .sum::<Complex<f32>>()
                .norm()
        };
        let best = (30..45)
            .max_by(|a, b| score(*a).total_cmp(&score(*b)))
            .unwrap();
        assert!(best == 37 || best == 38, "{best}");
    }

    #[test]
    fn a_moving_echo_migrates_in_delay() {
        let carrier = 1e6;
        let noise = SceneSignal::Noise {
            offset_hz: 0.0,
            bandwidth_hz: 3e5,
        };
        let scene = |echo: Option<SceneEcho>| {
            let scene = ArrayScene::new(kraken(), carrier, FS)
                .with_source(SceneSource::new(Direction::horizon(0.0), 0.0, noise))
                .with_seed(3);
            match echo {
                Some(echo) => scene.with_echo(echo),
                None => scene,
            }
        };
        let echo = SceneEcho::new(0, Direction::horizon(90.0), 0.0, 60.0, 1000.0);
        assert!((echo.delay_at(40_000, carrier) - 20.0).abs() < 1e-9);
        let lanes = echo_only(scene(Some(echo)), scene(None), 44_000);
        let plain = scene(None).render(44_000).unwrap();
        let best_lag = |at: usize| {
            let score = |lag: usize| {
                (at..at + 1000)
                    .map(|n| {
                        let turn = Complex::from_polar(1.0, -(TAU * 1000.0 * n as f64 / FS) as f32);
                        lanes[0][n] * turn * plain[0][n - lag].conj()
                    })
                    .sum::<Complex<f32>>()
                    .norm()
            };
            (0..80)
                .max_by(|a, b| score(*a).total_cmp(&score(*b)))
                .unwrap()
        };
        assert!(best_lag(4500).abs_diff(55) <= 1, "{}", best_lag(4500));
        assert!(best_lag(40_000).abs_diff(20) <= 1, "{}", best_lag(40_000));
    }

    #[test]
    fn bistatic_echoes_convert_range_and_rate() {
        let echo = SceneEcho::bistatic(
            0,
            Direction::horizon(30.0),
            -20.0,
            42_300.0,
            -90.0,
            266_666.67,
            100e6,
        );
        assert!((echo.delay_samples - 42_300.0 / LIGHT_SPEED_M_S * 266_666.67).abs() < 1e-9);
        assert!(echo.doppler_hz > 0.0);
        assert!((echo.doppler_hz - 90.0 * 100e6 / LIGHT_SPEED_M_S).abs() < 1e-9);
    }
}
