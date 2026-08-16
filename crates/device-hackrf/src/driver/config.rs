use super::error::{Error, Result};

const DEFAULT_FREQUENCY_HZ: u64 = 900_000_000;
const DEFAULT_SAMPLE_RATE_HZ: u32 = 10_000_000;
const DEFAULT_LNA_GAIN_DB: u8 = 8;
const DEFAULT_VGA_GAIN_DB: u8 = 20;
const DEFAULT_TX_VGA_GAIN_DB: u8 = 0;

pub(crate) const FILTER_WIDTHS_HZ: [u32; 16] = [
    1_750_000, 2_500_000, 3_500_000, 5_000_000, 5_500_000, 6_000_000, 7_000_000, 8_000_000,
    9_000_000, 10_000_000, 12_000_000, 14_000_000, 15_000_000, 20_000_000, 24_000_000, 28_000_000,
];

const FILTER_RATE_FRACTION_NUM: u64 = 3;
const FILTER_RATE_FRACTION_DEN: u64 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Config {
    pub(crate) frequency_hz: u64,
    pub(crate) sample_rate_hz: u32,
    pub(crate) lna_gain_db: u8,
    pub(crate) vga_gain_db: u8,
    pub(crate) tx_vga_gain_db: u8,
    pub(crate) filter_width_hz: u32,
    pub(crate) amp_enabled: bool,
    pub(crate) bias_tee_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            frequency_hz: DEFAULT_FREQUENCY_HZ,
            sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
            lna_gain_db: DEFAULT_LNA_GAIN_DB,
            vga_gain_db: DEFAULT_VGA_GAIN_DB,
            tx_vga_gain_db: DEFAULT_TX_VGA_GAIN_DB,
            filter_width_hz: filter_width_for_rate(DEFAULT_SAMPLE_RATE_HZ),
            amp_enabled: false,
            bias_tee_enabled: false,
        }
    }
}

pub(crate) fn validate_frequency(value: u64) -> Result<()> {
    if (1_000_000..=6_000_000_000).contains(&value) {
        Ok(())
    } else {
        Err(Error::invalid_config(
            "frequency_hz",
            "must be between 1 MHz and 6 GHz inclusive",
        ))
    }
}

pub(crate) fn validate_sample_rate(value: u32) -> Result<()> {
    if (2_000_000..=20_000_000).contains(&value) {
        Ok(())
    } else {
        Err(Error::invalid_config(
            "sample_rate_hz",
            "must be between 2 MHz and 20 MHz inclusive",
        ))
    }
}

pub(crate) fn snap_filter_width(bandwidth_hz: u32) -> u32 {
    FILTER_WIDTHS_HZ
        .iter()
        .rev()
        .copied()
        .find(|width| *width <= bandwidth_hz)
        .unwrap_or(FILTER_WIDTHS_HZ[0])
}

pub(crate) fn filter_width_for_rate(sample_rate_hz: u32) -> u32 {
    let target = u64::from(sample_rate_hz) * FILTER_RATE_FRACTION_NUM / FILTER_RATE_FRACTION_DEN;
    snap_filter_width(target as u32)
}

pub(crate) fn validate_filter_width(value: u32) -> Result<()> {
    if FILTER_WIDTHS_HZ.contains(&value) {
        Ok(())
    } else {
        Err(Error::invalid_config(
            "filter_width_hz",
            "must be one of the MAX2837 baseband filter widths, 1.75 MHz through 28 MHz",
        ))
    }
}

pub(crate) fn validate_lna_gain(value: u8) -> Result<()> {
    if value <= 40 && value.is_multiple_of(8) {
        Ok(())
    } else {
        Err(Error::invalid_config(
            "lna_gain_db",
            "must be 0 through 40 dB in 8 dB steps",
        ))
    }
}

pub(crate) fn validate_vga_gain(value: u8) -> Result<()> {
    if value <= 62 && value.is_multiple_of(2) {
        Ok(())
    } else {
        Err(Error::invalid_config(
            "vga_gain_db",
            "must be 0 through 62 dB in 2 dB steps",
        ))
    }
}

pub(crate) fn validate_tx_vga_gain(value: u8) -> Result<()> {
    if value <= 47 {
        Ok(())
    } else {
        Err(Error::invalid_config(
            "tx_vga_gain_db",
            "must be between 0 dB and 47 dB inclusive",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_hackrf_transfer() {
        let config = Config::default();
        assert_eq!(config.frequency_hz, 900_000_000);
        assert_eq!(config.sample_rate_hz, 10_000_000);
        assert_eq!(config.lna_gain_db, 8);
        assert_eq!(config.vga_gain_db, 20);
        assert_eq!(config.tx_vga_gain_db, 0);
        assert_eq!(config.filter_width_hz, 7_000_000);
        assert!(!config.amp_enabled);
        assert!(!config.bias_tee_enabled);
    }

    #[test]
    fn filter_widths_snap_down_onto_the_max2837_table() {
        assert_eq!(snap_filter_width(1_750_000), 1_750_000);
        assert_eq!(snap_filter_width(2_499_999), 1_750_000);
        assert_eq!(snap_filter_width(2_500_000), 2_500_000);
        assert_eq!(snap_filter_width(7_500_000), 7_000_000);
        assert_eq!(snap_filter_width(28_000_000), 28_000_000);
        assert_eq!(snap_filter_width(100_000_000), 28_000_000);
        assert_eq!(snap_filter_width(0), 1_750_000);
        assert_eq!(snap_filter_width(1_000_000), 1_750_000);
    }

    #[test]
    fn every_listed_width_snaps_to_itself_and_validates() {
        for width in FILTER_WIDTHS_HZ {
            assert_eq!(snap_filter_width(width), width);
            assert!(validate_filter_width(width).is_ok(), "{width}");
        }
        for off_table in [0, 1_000_000, 7_500_000, 11_000_000, 28_000_001] {
            assert!(validate_filter_width(off_table).is_err(), "{off_table}");
        }
    }

    #[test]
    fn the_rate_derived_width_is_three_quarters_of_the_rate() {
        assert_eq!(filter_width_for_rate(2_000_000), 1_750_000);
        assert_eq!(filter_width_for_rate(8_000_000), 6_000_000);
        assert_eq!(filter_width_for_rate(10_000_000), 7_000_000);
        assert_eq!(filter_width_for_rate(20_000_000), 15_000_000);
        for rate in (2_000_000..=20_000_000).step_by(250_000) {
            let width = filter_width_for_rate(rate);
            assert!(width <= rate, "{width} Hz filter at {rate} Hz");
            assert!(validate_filter_width(width).is_ok(), "rate {rate}");
        }
    }

    #[test]
    fn frequency_is_bounded_at_both_ends() {
        assert!(validate_frequency(999_999).is_err());
        assert!(validate_frequency(1_000_000).is_ok());
        assert!(validate_frequency(6_000_000_000).is_ok());
        assert!(validate_frequency(6_000_000_001).is_err());
    }

    #[test]
    fn sample_rate_is_bounded_at_both_ends() {
        assert!(validate_sample_rate(1_999_999).is_err());
        assert!(validate_sample_rate(2_000_000).is_ok());
        assert!(validate_sample_rate(20_000_000).is_ok());
        assert!(validate_sample_rate(20_000_001).is_err());
    }

    #[test]
    fn gains_must_land_on_the_step_grid() {
        assert!(validate_lna_gain(13).is_err());
        assert!(validate_lna_gain(48).is_err());
        for db in [0, 8, 16, 24, 32, 40] {
            assert!(validate_lna_gain(db).is_ok(), "lna {db}");
        }
        assert!(validate_vga_gain(3).is_err());
        assert!(validate_vga_gain(64).is_err());
        for db in (0..=62).step_by(2) {
            assert!(validate_vga_gain(db).is_ok(), "vga {db}");
        }
        assert!(validate_tx_vga_gain(47).is_ok());
        assert!(validate_tx_vga_gain(13).is_ok());
        assert!(validate_tx_vga_gain(48).is_err());
    }
}
