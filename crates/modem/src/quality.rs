use num_complex::Complex;

use crate::{constellation::Constellation, cpm::Mapping};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    pub evm: f64,
    pub mer_db: f64,
    pub margin: f64,
}

impl Quality {
    fn from_errors(error_power: f64, reference_power: f64, half_spacing: f64) -> Option<Self> {
        if reference_power <= 0.0 || half_spacing <= 0.0 {
            return None;
        }
        let evm = (error_power / reference_power).sqrt();
        let rms_error = error_power.sqrt();
        Some(Self {
            evm,
            mer_db: if evm > 0.0 {
                -20.0 * evm.log10()
            } else {
                f64::INFINITY
            },
            margin: if rms_error > 0.0 {
                half_spacing / rms_error
            } else {
                f64::INFINITY
            },
        })
    }
}

fn scale_to(measured: f64, reference: f64) -> f64 {
    if measured > 0.0 {
        (reference / measured).sqrt()
    } else {
        1.0
    }
}

#[must_use]
pub fn measure_complex(symbols: &[Complex<f32>], table: &Constellation) -> Option<Quality> {
    if symbols.is_empty() || table.is_empty() {
        return None;
    }
    let n = symbols.len() as f64;
    let measured = symbols.iter().map(|y| f64::from(y.norm_sqr())).sum::<f64>() / n;
    let ideal_power = table
        .points()
        .iter()
        .map(|p| f64::from(p.norm_sqr()))
        .sum::<f64>()
        / table.len() as f64;
    let blind = scale_to(measured, ideal_power);

    let decided = symbols
        .iter()
        .map(|y| f64::from(table.nearest(y * blind as f32).norm_sqr()))
        .sum::<f64>()
        / n;
    let gain = blind * scale_to(measured * blind * blind, decided);

    let mut error_power = 0.0;
    let mut reference_power = 0.0;
    for &y in symbols {
        let scaled = y * gain as f32;
        let ideal = table.nearest(scaled);
        error_power += f64::from((scaled - ideal).norm_sqr());
        reference_power += f64::from(ideal.norm_sqr());
    }
    Quality::from_errors(
        error_power / n,
        reference_power / n,
        table.min_distance() / 2.0,
    )
}

#[must_use]
pub fn measure_levels(symbols: &[f32], mapping: &Mapping) -> Option<Quality> {
    if symbols.is_empty() {
        return None;
    }
    let level_of = |y: f32| f64::from(mapping.level(mapping.slice(y)));
    let n = symbols.len() as f64;
    let measured = symbols
        .iter()
        .map(|&y| f64::from(y) * f64::from(y))
        .sum::<f64>()
        / n;
    let ideal_power = mapping
        .levels()
        .iter()
        .map(|&l| f64::from(l) * f64::from(l))
        .sum::<f64>()
        / mapping.m() as f64;
    let blind = scale_to(measured, ideal_power);

    let decided = symbols
        .iter()
        .map(|&y| {
            let ideal = level_of(y * blind as f32);
            ideal * ideal
        })
        .sum::<f64>()
        / n;
    let gain = blind * scale_to(measured * blind * blind, decided);

    let mut error_power = 0.0;
    let mut reference_power = 0.0;
    for &y in symbols {
        let scaled = y * gain as f32;
        let ideal = level_of(scaled);
        let error = f64::from(scaled) - ideal;
        error_power += error * error;
        reference_power += ideal * ideal;
    }
    Quality::from_errors(
        error_power / n,
        reference_power / n,
        f64::from(mapping.min_spacing()) / 2.0,
    )
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::rng::Rng;

    use super::*;
    use crate::constellation::tables;

    fn noisy(points: &[Complex<f32>], sigma: f64, seed: u64) -> Vec<Complex<f32>> {
        let mut rng = Rng::new(seed);
        points
            .iter()
            .map(|&p| {
                let (n0, n1) = rng.normal_pair();
                Complex::new(p.re + (n0 * sigma) as f32, p.im + (n1 * sigma) as f32)
            })
            .collect()
    }

    fn repeated(table: &Constellation, count: usize) -> Vec<Complex<f32>> {
        (0..count)
            .map(|i| table.points()[i % table.len()])
            .collect()
    }

    #[test]
    fn a_clean_constellation_measures_no_error() {
        let table = tables::psk(4).expect("qpsk");
        let quality = measure_complex(&repeated(&table, 64), &table).expect("measurable");
        assert!(quality.evm < 1e-6, "clean EVM {}", quality.evm);
        assert!(quality.mer_db > 100.0, "clean MER {}", quality.mer_db);
    }

    #[test]
    fn evm_tracks_the_noise_that_was_injected() {
        let table = tables::psk(4).expect("qpsk");
        let clean = repeated(&table, 4096);
        let reference_power: f64 =
            clean.iter().map(|p| f64::from(p.norm_sqr())).sum::<f64>() / clean.len() as f64;
        for sigma in [0.02, 0.05, 0.1] {
            let quality = measure_complex(&noisy(&clean, sigma, 7), &table).expect("measurable");
            let expected = (2.0 * sigma * sigma / reference_power).sqrt();
            let ratio = quality.evm / expected;
            assert!(
                (0.9..1.1).contains(&ratio),
                "sigma {sigma}: EVM {} against an expected {expected}",
                quality.evm
            );
        }
    }

    #[test]
    fn a_gain_error_alone_does_not_count_as_distortion() {
        let table = tables::qam_square(16).expect("16-QAM");
        let scaled: Vec<_> = repeated(&table, 256).iter().map(|p| p * 3.7).collect();
        let quality = measure_complex(&scaled, &table).expect("measurable");
        assert!(
            quality.evm < 1e-5,
            "a pure gain read as {} EVM",
            quality.evm
        );
    }

    #[test]
    fn a_rotated_constellation_is_reported_as_the_fault_it_is() {
        let table = tables::psk(4).expect("qpsk");
        let turn = Complex::new(0.0f32, 0.35).exp();
        let rotated: Vec<_> = repeated(&table, 256).iter().map(|p| p * turn).collect();
        let quality = measure_complex(&rotated, &table).expect("measurable");
        assert!(quality.evm > 0.3, "rotation hidden at {} EVM", quality.evm);
    }

    #[test]
    fn margin_tracks_the_noise_until_the_error_reaches_the_decision_boundary() {
        let table = tables::psk(4).expect("qpsk");
        let clean = repeated(&table, 8192);
        let half = table.min_distance() / 2.0;
        for k in [0.1, 0.2, 0.4] {
            let quality = measure_complex(&noisy(&clean, half * k, 3), &table).expect("measurable");
            let expected = 1.0 / (k * std::f64::consts::SQRT_2);
            let ratio = quality.margin / expected;
            assert!(
                (0.85..1.15).contains(&ratio),
                "sigma {k}*half: margin {} against an expected {expected}",
                quality.margin
            );
        }
    }

    #[test]
    fn a_swamped_channel_saturates_rather_than_reporting_a_depth_it_cannot_see() {
        let table = tables::psk(4).expect("qpsk");
        let clean = repeated(&table, 8192);
        let half = table.min_distance() / 2.0;
        let healthy = measure_complex(&noisy(&clean, half * 0.1, 3), &table).expect("measurable");
        let swamped = measure_complex(&noisy(&clean, half * 3.0, 3), &table).expect("measurable");
        assert!(healthy.margin > 5.0, "healthy margin {}", healthy.margin);
        assert!(
            (1.0..1.3).contains(&swamped.margin),
            "a slicer cannot see past its own decision region; margin {}",
            swamped.margin
        );
    }

    #[test]
    fn four_level_symbols_measure_against_their_own_spacing() {
        let mapping = Mapping::natural(4);
        let levels: Vec<f32> = (0..4096).map(|i| mapping.level((i % 4) as u8)).collect();
        let clean = measure_levels(&levels, &mapping).expect("measurable");
        assert!(clean.evm < 1e-6, "clean level EVM {}", clean.evm);

        let mut rng = Rng::new(11);
        let sigma = f64::from(mapping.min_spacing()) * 0.1;
        let dirty: Vec<f32> = levels
            .iter()
            .map(|&l| l + (rng.normal_pair().0 * sigma) as f32)
            .collect();
        let noisy = measure_levels(&dirty, &mapping).expect("measurable");
        assert!(noisy.margin > 3.0, "level margin {}", noisy.margin);
        assert!(noisy.mer_db < clean.mer_db);
    }

    #[test]
    fn nothing_to_measure_is_reported_as_nothing() {
        assert!(measure_complex(&[], &tables::psk(4).expect("qpsk")).is_none());
        assert!(measure_levels(&[], &Mapping::natural(4)).is_none());
    }
}
