use std::f64::consts::TAU;

pub const UNIX_EPOCH_JD: f64 = 2_440_587.5;
pub const SECONDS_PER_DAY: f64 = 86_400.0;

#[must_use]
pub fn julian_date_of_year(year: i32) -> f64 {
    let year = f64::from(year);
    367.0 * year - (7.0 * year / 4.0).floor() + 31.0 + 1_721_013.5
}

#[must_use]
pub fn julian_date(unix_seconds: f64) -> f64 {
    unix_seconds / SECONDS_PER_DAY + UNIX_EPOCH_JD
}

#[must_use]
pub fn unix_seconds(julian_date: f64) -> f64 {
    (julian_date - UNIX_EPOCH_JD) * SECONDS_PER_DAY
}

#[must_use]
pub fn sidereal_angle(julian_date: f64) -> f64 {
    let centuries = (julian_date - 2_451_545.0) / 36_525.0;
    let seconds = -6.2e-6 * centuries.powi(3)
        + 0.093_104 * centuries.powi(2)
        + (876_600.0 * 3_600.0 + 8_640_184.812_866) * centuries
        + 67_310.548_41;
    (seconds.to_radians() / 240.0).rem_euclid(TAU)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_start_on_their_first_midnight() {
        assert_eq!(julian_date_of_year(2024), 2_460_310.5);
        assert_eq!(julian_date_of_year(2000), 2_451_544.5);
    }

    #[test]
    fn unix_time_round_trips() {
        let now = 1_727_000_000.25;
        assert!((unix_seconds(julian_date(now)) - now).abs() < 1e-4);
        assert_eq!(julian_date(0.0), UNIX_EPOCH_JD);
    }

    #[test]
    fn sidereal_time_at_j2000_matches_the_almanac() {
        let degrees = sidereal_angle(2_451_545.0).to_degrees();
        assert!((degrees - 280.460_618_37).abs() < 1e-6, "{degrees}");
    }
}
