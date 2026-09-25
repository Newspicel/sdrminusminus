use sdrmm_wire::device::{Capabilities, Range};

const MIN_TOP_PLACE: i32 = 8;
const MAX_TOP_PLACE: i32 = 11;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    pub min: f64,
    pub max: f64,
}

pub const ANY_FREQUENCY: Reach = Reach { min: 0.0, max: 6e9 };

impl Reach {
    #[must_use]
    pub fn clamp(self, hz: f64) -> f64 {
        hz.clamp(self.min, self.max.max(self.min))
    }
}

#[must_use]
pub fn tuning_range(caps: &Capabilities) -> Reach {
    if caps.freq_ranges.is_empty() {
        return ANY_FREQUENCY;
    }
    Reach {
        min: caps
            .freq_ranges
            .iter()
            .map(|range| range.min)
            .fold(f64::INFINITY, f64::min),
        max: caps
            .freq_ranges
            .iter()
            .map(|range| range.max)
            .fold(f64::NEG_INFINITY, f64::max),
    }
}

#[must_use]
pub fn reachable_hz(ranges: &[Range], hz: f64) -> f64 {
    let Some(first) = ranges.first() else {
        return ANY_FREQUENCY.clamp(hz);
    };
    let held = |range: &Range| hz.clamp(range.min, range.max.max(range.min));
    ranges.iter().fold(held(first), |best, range| {
        let candidate = held(range);
        if (candidate - hz).abs() < (best - hz).abs() {
            candidate
        } else {
            best
        }
    })
}

#[must_use]
pub fn is_tunable(reach: Reach) -> bool {
    reach.max > reach.min
}

#[must_use]
pub fn dial_places(max_hz: f64) -> Vec<i32> {
    let needed = if max_hz >= 1.0 {
        max_hz.log10().floor() as i32
    } else {
        0
    };
    let top = needed.clamp(MIN_TOP_PLACE, MAX_TOP_PLACE);
    (0..=top).rev().collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialDigit {
    pub place: i32,
    pub digit: u8,
    pub leading: bool,
}

fn whole(hz: f64) -> u64 {
    hz.round().max(0.0) as u64
}

fn digit_at(value: u64, place: i32) -> u8 {
    ((value / 10u64.pow(place.max(0) as u32)) % 10) as u8
}

#[must_use]
pub fn dial_digits(hz: f64, places: &[i32]) -> Vec<DialDigit> {
    let value = whole(hz);
    let mut seen = false;
    places
        .iter()
        .map(|&place| {
            let digit = digit_at(value, place);
            let leading = !seen && digit == 0 && place > 6;
            seen |= digit != 0;
            DialDigit {
                place,
                digit,
                leading,
            }
        })
        .collect()
}

#[must_use]
pub fn step_dial(hz: f64, place: i32, direction: i32, reach: Reach) -> f64 {
    reach.clamp(hz.round() + f64::from(direction) * 10f64.powi(place))
}

#[must_use]
pub fn set_dial_digit(hz: f64, place: i32, digit: u8, reach: Reach) -> f64 {
    let value = whole(hz);
    let current = digit_at(value, place);
    let unit = 10f64.powi(place);
    reach.clamp(value as f64 + (f64::from(digit) - f64::from(current)) * unit)
}

fn split_number(text: &str) -> Option<(&str, &str)> {
    let end = text
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == ','))
        .map_or(text.len(), |(at, _)| at);
    let (number, rest) = text.split_at(end);
    let separators = number.chars().filter(|c| *c == '.' || *c == ',').count();
    let valid = separators <= 1
        && number.chars().last().is_some_and(|c| c.is_ascii_digit())
        && !number.is_empty();
    valid.then_some((number, rest))
}

fn unit_scale(unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "" | "mhz" | "m" => Some(1e6),
        "ghz" | "g" => Some(1e9),
        "khz" | "k" => Some(1e3),
        "hz" | "h" => Some(1.0),
        _ => None,
    }
}

#[must_use]
pub fn parse_frequency(text: &str) -> Option<f64> {
    let (number, rest) = split_number(text.trim())?;
    let scale = unit_scale(rest.trim())?;
    let value: f64 = number.replace(',', ".").parse().ok()?;
    value.is_finite().then(|| (value * scale).round())
}

#[must_use]
pub fn in_tuning_range(hz: f64, reach: Reach) -> Option<f64> {
    (hz >= reach.min && hz <= reach.max).then_some(hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tune_target_hz(text: &str, reach: Reach) -> Option<f64> {
        parse_frequency(text).and_then(|hz| in_tuning_range(hz, reach))
    }

    const WIDE: Reach = Reach { min: 0.0, max: 6e9 };

    fn span(min: f64, max: f64) -> Range {
        Range {
            min,
            max,
            step: None,
        }
    }

    #[test]
    fn a_radio_with_somewhere_to_go_is_tunable_and_a_point_is_not() {
        assert!(is_tunable(WIDE));
        assert!(is_tunable(Reach {
            min: 100e6,
            max: 100e6 + 1.0
        }));
        assert!(!is_tunable(Reach {
            min: 100e6,
            max: 100e6
        }));
        assert!(!is_tunable(Reach { min: 0.0, max: 0.0 }));
    }

    #[test]
    fn a_frequency_in_a_gap_moves_to_the_nearest_edge() {
        let v4 = [span(500e3, 28.8e6), span(24e6, 1.766e9)];
        assert_eq!(reachable_hz(&v4, 7.1e6), 7.1e6);
        assert_eq!(reachable_hz(&v4, 145.5e6), 145.5e6);
        let hf = [span(1e3, 31e6), span(60e6, 260e6)];
        assert_eq!(reachable_hz(&hf, 40e6), 31e6);
        assert_eq!(reachable_hz(&hf, 55e6), 60e6);
        assert_eq!(reachable_hz(&v4, 10.0), 500e3);
        assert_eq!(reachable_hz(&v4, 9e9), 1.766e9);
        assert_eq!(reachable_hz(&[], 1.2e9), 1.2e9);
    }

    #[test]
    fn the_dial_never_draws_fewer_than_four_megahertz_digits() {
        assert_eq!(dial_places(2.4e9), vec![9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(dial_places(1.766e9).len(), 10);
        assert_eq!(dial_places(30e6), vec![8, 7, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(dial_places(6e9)[0], 9);
        assert_eq!(dial_places(1e13)[0], 11);
    }

    #[test]
    fn only_zeros_left_of_the_first_significant_digit_are_leading() {
        let digits = dial_digits(100_000_000.0, &dial_places(1.766e9));
        assert_eq!(
            digits.iter().map(|d| d.digit).collect::<Vec<_>>(),
            vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let leading = dial_digits(88_500_000.0, &dial_places(1.766e9));
        assert_eq!(
            leading
                .iter()
                .filter(|d| d.leading)
                .map(|d| d.place)
                .collect::<Vec<_>>(),
            vec![9, 8]
        );
        let low = dial_digits(198_000.0, &dial_places(30e6));
        assert_eq!(
            low.iter().take(3).map(|d| d.leading).collect::<Vec<_>>(),
            vec![true, true, false]
        );
    }

    #[test]
    fn a_step_moves_one_unit_of_its_place_and_clamps() {
        assert_eq!(step_dial(100_000_000.0, 6, 1, WIDE), 101_000_000.0);
        assert_eq!(step_dial(100_000_000.0, 3, -1, WIDE), 99_999_000.0);
        assert_eq!(step_dial(5_999_000_000.0, 9, 1, WIDE), 6e9);
        let v4 = Reach {
            min: 24e6,
            max: 1.766e9,
        };
        assert_eq!(step_dial(1_000.0, 6, -1, v4), 24e6);
    }

    #[test]
    fn a_digit_write_touches_one_place_and_clamps() {
        assert_eq!(set_dial_digit(145_500_000.0, 6, 8, WIDE), 148_500_000.0);
        assert_eq!(set_dial_digit(145_500_000.0, 8, 0, WIDE), 45_500_000.0);
        assert_eq!(set_dial_digit(145_500_000.0, 5, 5, WIDE), 145_500_000.0);
        let v4 = Reach {
            min: 24e6,
            max: 1.766e9,
        };
        assert_eq!(set_dial_digit(145_500_000.0, 9, 9, v4), 1.766e9);
    }

    #[test]
    fn a_bare_number_reads_as_megahertz_and_a_unit_wins() {
        assert_eq!(parse_frequency("145.5"), Some(145_500_000.0));
        assert_eq!(parse_frequency("1090"), Some(1_090_000_000.0));
        assert_eq!(parse_frequency("433800k"), Some(433_800_000.0));
        assert_eq!(parse_frequency("7.1 MHz"), Some(7_100_000.0));
        assert_eq!(parse_frequency("162550000 Hz"), Some(162_550_000.0));
        assert_eq!(parse_frequency("2.4g"), Some(2_400_000_000.0));
        assert_eq!(parse_frequency("145,5"), Some(145_500_000.0));
    }

    #[test]
    fn nothing_readable_tunes_nowhere() {
        for text in ["", "abc", "145.5.5", "145 MHz extra", ".", "5."] {
            assert_eq!(parse_frequency(text), None, "{text}");
        }
    }

    #[test]
    fn a_typed_target_outside_the_reach_is_refused() {
        let reach = Reach {
            min: 24e6,
            max: 1.766e9,
        };
        assert_eq!(tune_target_hz("145.5", reach), Some(145_500_000.0));
        assert_eq!(tune_target_hz("433800k", reach), Some(433_800_000.0));
        assert_eq!(tune_target_hz("10", reach), None);
        assert_eq!(tune_target_hz("2.4g", reach), None);
        assert_eq!(tune_target_hz("24", reach), Some(24_000_000.0));
        assert_eq!(tune_target_hz("1766", reach), Some(1_766_000_000.0));
        assert_eq!(tune_target_hz("", reach), None);
        assert_eq!(in_tuning_range(reach.min, reach), Some(reach.min));
        assert_eq!(in_tuning_range(reach.max, reach), Some(reach.max));
    }
}
