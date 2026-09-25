const PREFIXES: [(f64, &str); 4] = [(1e9, "G"), (1e6, "M"), (1e3, "k"), (1.0, "")];

fn trim_zeros(fixed: String) -> String {
    if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        fixed
    }
}

#[must_use]
pub fn si(value: f64, unit: &str) -> String {
    if !value.is_finite() {
        return format!("? {unit}");
    }
    let magnitude = value.abs();
    let (scale, prefix) = PREFIXES
        .iter()
        .copied()
        .find(|(step, _)| magnitude >= *step)
        .unwrap_or((1.0, ""));
    format!(
        "{} {prefix}{unit}",
        trim_zeros(format!("{:.9}", value / scale))
    )
}

#[must_use]
pub fn hz(value: f64) -> String {
    si(value, "Hz")
}

#[must_use]
pub fn sample_rate(value: f64) -> String {
    si(value, "S/s")
}

#[must_use]
pub fn bytes(value: f64) -> String {
    si(value, "B")
}

#[must_use]
pub fn mhz(value: f64) -> String {
    format!("{:.4} MHz", value / 1e6)
}

fn pad(value: u64) -> String {
    format!("{value:02}")
}

#[must_use]
pub fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0).floor() as u64;
    let (hours, minutes, rest) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{}:{}", pad(minutes), pad(rest))
    } else {
        format!("{minutes}:{}", pad(rest))
    }
}

#[must_use]
pub fn duration(seconds: f64) -> String {
    let tenths = (seconds * 10.0).round() / 10.0;
    if tenths < 60.0 {
        return format!("{tenths:.1} s");
    }
    clock(tenths.round())
}

#[must_use]
pub fn fraction_digits(step: Option<f64>) -> usize {
    let Some(step) = step.filter(|step| step.is_finite() && *step != 0.0) else {
        return 6;
    };
    let text = format!("{:e}", step.abs());
    let (mantissa, exponent) = text.split_once('e').unwrap_or((text.as_str(), "0"));
    let decimals = mantissa.split_once('.').map_or(0, |(_, tail)| tail.len()) as i64;
    let exponent: i64 = exponent.parse().unwrap_or(0);
    (decimals - exponent).clamp(0, 20) as usize
}

#[must_use]
pub fn setting_label(name: &str) -> String {
    let mut spaced = String::with_capacity(name.len() + 4);
    let mut previous: Option<char> = None;
    for c in name.chars() {
        if c.is_ascii_uppercase()
            && previous.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
        {
            spaced.push(' ');
        }
        spaced.push(if c == '_' || c == '-' { ' ' } else { c });
        previous = Some(c);
    }
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_take_the_prefix_they_read_best_in() {
        assert_eq!(hz(10.0), "10 Hz");
        assert_eq!(hz(12_500.0), "12.5 kHz");
        assert_eq!(hz(100_000.0), "100 kHz");
        assert_eq!(hz(1_000_000.0), "1 MHz");
        assert_eq!(sample_rate(2_048_000.0), "2.048 MS/s");
        assert_eq!(bytes(16_384_000.0), "16.384 MB");
        assert_eq!(mhz(100e6), "100.0000 MHz");
    }

    #[test]
    fn a_clock_reads_at_a_fixed_width_and_grows_hours_only_when_needed() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(9.9), "0:09");
        assert_eq!(clock(64.0), "1:04");
        assert_eq!(clock(599.0), "9:59");
        assert_eq!(clock(3_600.0), "1:00:00");
        assert_eq!(clock(3_725.0), "1:02:05");
        assert_eq!(clock(-5.0), "0:00");
    }

    #[test]
    fn a_short_duration_reads_in_seconds_and_a_long_one_as_a_clock() {
        assert_eq!(duration(1.0), "1.0 s");
        assert_eq!(duration(59.94), "59.9 s");
        assert_eq!(duration(64.0), "1:04");
    }

    #[test]
    fn a_step_says_how_many_decimals_to_show() {
        assert_eq!(fraction_digits(Some(0.001)), 3);
        assert_eq!(fraction_digits(Some(1.0)), 0);
        assert_eq!(fraction_digits(Some(0.00001)), 5);
        assert_eq!(fraction_digits(Some(12.5)), 1);
        assert_eq!(fraction_digits(None), 6);
    }

    #[test]
    fn a_driver_key_gets_its_words_back() {
        assert_eq!(setting_label("digital_agc"), "digital agc");
        assert_eq!(setting_label("offset_tune"), "offset tune");
        assert_eq!(setting_label("biasTee"), "bias Tee");
        assert_eq!(setting_label("direct-samp"), "direct samp");
        assert_eq!(setting_label("biastee"), "biastee");
        assert_eq!(setting_label("_rf__gain_"), "rf gain");
    }
}
