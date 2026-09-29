use std::f64::consts::PI;

const KAISER_BETA: f64 = 8.0;
const FULL_SCALE: f64 = 32768.0;
const TAP_LENGTHS: [usize; 8] = [16, 32, 48, 64, 80, 96, 112, 128];

pub(super) fn taps_for(budget: usize) -> usize {
    TAP_LENGTHS
        .iter()
        .rev()
        .copied()
        .find(|taps| *taps <= budget)
        .unwrap_or(TAP_LENGTHS[0])
}

pub(super) fn design(taps: usize, factor: u32, dc_gain: f64) -> Vec<i16> {
    let cutoff = 0.5 / f64::from(factor.max(1));
    let middle = (taps as f64 - 1.0) / 2.0;
    let ideal: Vec<f64> = (0..taps)
        .map(|n| {
            let t = n as f64 - middle;
            2.0 * cutoff * sinc(2.0 * cutoff * t) * kaiser(t / (middle + 1.0))
        })
        .collect();
    let sum: f64 = ideal.iter().sum();
    let target = dc_gain * FULL_SCALE;
    let peak = ideal.iter().fold(0.0f64, |peak, tap| peak.max(tap.abs())) * target / sum;
    let fit = if peak > f64::from(i16::MAX) {
        f64::from(i16::MAX) / peak
    } else {
        1.0
    };
    ideal
        .iter()
        .map(|tap| (tap * target / sum * fit).round() as i16)
        .collect()
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

fn kaiser(position: f64) -> f64 {
    let inside = (1.0 - position * position).max(0.0);
    bessel_i0(KAISER_BETA * inside.sqrt()) / bessel_i0(KAISER_BETA)
}

fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64 {
        term *= quarter / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(taps: &[i16], cycles_per_sample: f64) -> f64 {
        let (re, im) = taps
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (n, tap)| {
                let phase = -2.0 * PI * cycles_per_sample * n as f64;
                (
                    re + f64::from(*tap) * phase.cos(),
                    im + f64::from(*tap) * phase.sin(),
                )
            });
        (re * re + im * im).sqrt()
    }

    #[test]
    fn the_tap_count_is_the_largest_the_budget_allows() {
        assert_eq!(taps_for(256), 128);
        assert_eq!(taps_for(100), 96);
        assert_eq!(taps_for(64), 64);
        assert_eq!(taps_for(8), 16);
    }

    #[test]
    fn a_decimate_by_two_filter_passes_the_band_and_stops_the_alias() {
        let taps = design(128, 2, 2.0);
        let dc = response(&taps, 0.0);
        assert!((dc / FULL_SCALE - 2.0).abs() < 0.01, "{dc}");
        assert!(response(&taps, 0.2) / dc > 0.99);
        assert!(response(&taps, 0.3) / dc < 1e-3);
        assert!(taps.iter().all(|tap| *tap < i16::MAX));
    }

    #[test]
    fn a_decimate_by_four_filter_cuts_at_an_eighth_of_its_clock() {
        let taps = design(128, 4, 2.0);
        let dc = response(&taps, 0.0);
        assert!(response(&taps, 0.1) / dc > 0.98);
        assert!(response(&taps, 0.16) / dc < 1e-3);
    }

    #[test]
    fn a_gain_too_large_for_the_coefficients_is_scaled_to_fit() {
        let taps = design(48, 4, 8.0);
        assert!(taps.contains(&i16::MAX));
    }

    #[test]
    fn the_filter_is_symmetric() {
        let taps = design(64, 2, 2.0);
        assert!(taps.iter().eq(taps.iter().rev()));
    }
}
