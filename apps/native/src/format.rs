pub const DIAL_DIGITS: usize = 10;

const MAX_DIAL_HZ: f64 = 9_999_999_999.0;

#[must_use]
pub fn dial_digits(hz: f64) -> [u8; DIAL_DIGITS] {
    let mut whole = hz.clamp(0.0, MAX_DIAL_HZ).round() as u64;
    let mut digits = [0u8; DIAL_DIGITS];
    for slot in digits.iter_mut().rev() {
        *slot = (whole % 10) as u8;
        whole /= 10;
    }
    digits
}

#[must_use]
pub fn digit_step_hz(index: usize) -> f64 {
    10f64.powi(9 - index.min(DIAL_DIGITS - 1) as i32)
}

#[must_use]
pub fn nudge(hz: f64, index: usize, steps: i32) -> f64 {
    let moved = hz + digit_step_hz(index) * f64::from(steps);
    moved.clamp(0.0, MAX_DIAL_HZ)
}

#[must_use]
pub fn frequency(hz: f64) -> String {
    let mhz = hz / 1e6;
    format!("{mhz:.6} MHz")
}

#[must_use]
pub fn span(hz: f64) -> String {
    if hz >= 1e6 {
        format!("{:.3} MHz", hz / 1e6)
    } else if hz >= 1e3 {
        format!("{:.1} kHz", hz / 1e3)
    } else {
        format!("{hz:.0} Hz")
    }
}

#[must_use]
pub fn decibels(value: f32) -> String {
    format!("{value:.1} dB")
}

#[must_use]
pub fn clock(iso: &str) -> String {
    iso.split('T').nth(1).map_or_else(
        || iso.to_owned(),
        |rest| rest[..rest.len().min(8)].to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dial_splits_a_frequency_into_ten_weighted_digits() {
        assert_eq!(dial_digits(100_300_000.0), [0, 1, 0, 0, 3, 0, 0, 0, 0, 0]);
        assert_eq!(dial_digits(0.0), [0; DIAL_DIGITS]);
        assert_eq!(dial_digits(9_999_999_999.0), [9; DIAL_DIGITS]);
    }

    #[test]
    fn a_frequency_over_the_dial_clamps_rather_than_wrapping() {
        assert_eq!(dial_digits(2e10), [9; DIAL_DIGITS]);
        assert_eq!(dial_digits(-5.0), [0; DIAL_DIGITS]);
    }

    #[test]
    fn each_digit_steps_by_its_own_weight() {
        assert_eq!(digit_step_hz(0), 1e9);
        assert_eq!(digit_step_hz(3), 1e6);
        assert_eq!(digit_step_hz(9), 1.0);
        assert_eq!(digit_step_hz(99), 1.0);
    }

    #[test]
    fn nudging_moves_one_weight_and_stops_at_the_ends() {
        assert_eq!(nudge(100_000_000.0, 3, 1), 101_000_000.0);
        assert_eq!(nudge(100_000_000.0, 9, -1), 99_999_999.0);
        assert_eq!(nudge(0.0, 3, -1), 0.0);
        assert_eq!(nudge(MAX_DIAL_HZ, 0, 1), MAX_DIAL_HZ);
    }

    #[test]
    fn readouts_pick_the_unit_the_number_belongs_in() {
        assert_eq!(span(2_400_000.0), "2.400 MHz");
        assert_eq!(span(12_500.0), "12.5 kHz");
        assert_eq!(span(800.0), "800 Hz");
        assert_eq!(frequency(100_300_000.0), "100.300000 MHz");
        assert_eq!(decibels(-14.04), "-14.0 dB");
    }

    #[test]
    fn a_timestamp_shows_its_clock_and_survives_one_that_has_none() {
        assert_eq!(clock("2026-09-14T12:34:56.789Z"), "12:34:56");
        assert_eq!(clock("no time here"), "no time here");
    }
}
