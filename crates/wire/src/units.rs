const PREFIXES: [(f64, &str); 4] = [(1e9, "G"), (1e6, "M"), (1e3, "k"), (1.0, "")];

const DECIMALS: usize = 9;

#[must_use]
pub fn si(value: f64, unit: &str) -> String {
    if !value.is_finite() {
        return format!("? {unit}");
    }
    let magnitude = value.abs();
    let (scale, prefix) = PREFIXES
        .into_iter()
        .find(|(scale, _)| magnitude >= *scale)
        .unwrap_or((1.0, ""));
    let scaled = trim_zeros(&format!("{:.*}", DECIMALS, value / scale));
    format!("{scaled} {prefix}{unit}")
}

#[must_use]
pub fn hertz(hz: f64) -> String {
    si(hz, "Hz")
}

#[must_use]
pub fn sample_rate(samples_per_second: f64) -> String {
    si(samples_per_second, "S/s")
}

#[must_use]
pub fn bit_rate(bits_per_second: f64) -> String {
    si(bits_per_second, "bit/s")
}

#[must_use]
pub fn bytes(count: f64) -> String {
    si(count, "B")
}

fn trim_zeros(fixed: &str) -> String {
    match fixed.split_once('.') {
        Some(_) => fixed.trim_end_matches('0').trim_end_matches('.').to_owned(),
        None => fixed.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequencies_climb_through_the_prefixes() {
        assert_eq!(hertz(0.0), "0 Hz");
        assert_eq!(hertz(50.5), "50.5 Hz");
        assert_eq!(hertz(455_000.0), "455 kHz");
        assert_eq!(hertz(999_999.0), "999.999 kHz");
        assert_eq!(hertz(1_000_000.0), "1 MHz");
        assert_eq!(hertz(145_500_000.0), "145.5 MHz");
        assert_eq!(hertz(1_090_000_000.0), "1.09 GHz");
        assert_eq!(hertz(446_006_300.0), "446.0063 MHz");
        assert_eq!(hertz(1_890_400_000.0), "1.8904 GHz");
    }

    #[test]
    fn a_sample_rate_reads_in_samples_per_second() {
        assert_eq!(sample_rate(2_048_000.0), "2.048 MS/s");
        assert_eq!(sample_rate(250_000.0), "250 kS/s");
        assert_eq!(sample_rate(48_000.0), "48 kS/s");
    }

    #[test]
    fn rates_and_sizes_use_the_same_scaling() {
        assert_eq!(bit_rate(96_000.0), "96 kbit/s");
        assert_eq!(bytes(32_800_000.0), "32.8 MB");
        assert_eq!(bytes(512.0), "512 B");
    }

    #[test]
    fn a_negative_value_keeps_its_sign_and_its_prefix() {
        assert_eq!(hertz(-12_500.0), "-12.5 kHz");
    }

    #[test]
    fn a_value_no_radio_can_produce_does_not_print_as_a_number() {
        assert_eq!(hertz(f64::INFINITY), "? Hz");
        assert_eq!(hertz(f64::NAN), "? Hz");
    }
}
