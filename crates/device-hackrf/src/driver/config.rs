//! The validated receiver configuration, mirroring what the hardware last accepted.

use super::error::{Error, Result};

/// Defaults `hackrf_transfer` starts from.
const DEFAULT_FREQUENCY_HZ: u64 = 900_000_000;
const DEFAULT_SAMPLE_RATE_HZ: u32 = 10_000_000;
const DEFAULT_LNA_GAIN_DB: u8 = 8;
const DEFAULT_VGA_GAIN_DB: u8 = 20;
const DEFAULT_TX_VGA_GAIN_DB: u8 = 0;

/// Every width the MAX2837's baseband filter can actually be programmed to, ascending —
/// libhackrf's `max2837_ft` table. The part has no continuous cutoff, so a width off this list
/// has no register encoding at all and the firmware would substitute one of its own.
pub(crate) const FILTER_WIDTHS_HZ: [u32; 16] = [
    1_750_000, 2_500_000, 3_500_000, 5_000_000, 5_500_000, 6_000_000, 7_000_000, 8_000_000,
    9_000_000, 10_000_000, 12_000_000, 14_000_000, 15_000_000, 20_000_000, 24_000_000, 28_000_000,
];

/// Fraction of the sample rate the filter is set to, as a rational so the arithmetic is exact.
/// libhackrf's `hackrf_set_sample_rate_manual` picks `0.75 × rate`: three quarters of the
/// complex bandwidth is what is left flat after the filter's own transition, so the passband is
/// usable to its edges instead of rolling off inside the span the client is shown.
const FILTER_RATE_FRACTION_NUM: u64 = 3;
const FILTER_RATE_FRACTION_DEN: u64 = 4;

/// What the radio holds.
///
/// A plain value record with no invariants of its own — validation lives on the `Device`
/// setters, which are the only things that reach hardware.
/// [`Device::config`](crate::Device::config) hands out a snapshot whose fields only moved once
/// their control transfer succeeded, so it is the device's own truth, including gains as the
/// MAX2837's step grid snapped them rather than as they were asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Config {
    /// Tuned centre frequency in Hz.
    pub(crate) frequency_hz: u64,
    /// Complex IQ sample rate in Hz.
    pub(crate) sample_rate_hz: u32,
    /// MAX2837 RX IF/LNA gain in dB.
    pub(crate) lna_gain_db: u8,
    /// MAX2837 baseband/VGA gain in dB.
    pub(crate) vga_gain_db: u8,
    /// MAX2837 transmit VGA gain in dB. Powers up at zero, so a transmit that was never asked
    /// for cannot reach the antenna at full drive.
    pub(crate) tx_vga_gain_db: u8,
    /// Width the MAX2837 baseband filter is programmed to, in Hz — always one of
    /// [`FILTER_WIDTHS_HZ`], whether it was asked for or derived from the sample rate.
    pub(crate) filter_width_hz: u32,
    /// Whether the RF amplifier is on.
    pub(crate) amp_enabled: bool,
    /// Whether the antenna port is powered.
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

/// The MAX2831 synthesiser's range.
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

/// Below 2 Msps the sample clock cannot be divided down; above 20 the USB path cannot keep up.
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

/// The widest listed filter that is no wider than `bandwidth_hz`, or the narrowest one when the
/// request is under the whole table — libhackrf's `hackrf_compute_baseband_filter_bw`. Rounding
/// *down* is the safe direction: a filter wider than asked for passes energy the caller said it
/// did not want, while a narrower one only costs span.
pub(crate) fn snap_filter_width(bandwidth_hz: u32) -> u32 {
    FILTER_WIDTHS_HZ
        .iter()
        .rev()
        .copied()
        .find(|width| *width <= bandwidth_hz)
        .unwrap_or(FILTER_WIDTHS_HZ[0])
}

/// The width the filter is carried to by a sample rate of `sample_rate_hz`.
pub(crate) fn filter_width_for_rate(sample_rate_hz: u32) -> u32 {
    let target = u64::from(sample_rate_hz) * FILTER_RATE_FRACTION_NUM / FILTER_RATE_FRACTION_DEN;
    snap_filter_width(target as u32)
}

/// The filter takes only the widths its register encodes; [`snap_filter_width`] is how a caller
/// turns an arbitrary request into one.
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

/// The MAX2837 IF amplifier has five 8 dB steps; anything else the firmware rejects.
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

/// The MAX2837 baseband VGA steps in 2 dB up to 62.
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

/// The transmit VGA is a 47 dB range with no step constraint.
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
        // 0.75 × 10 Msps is 7.5 MHz, which the table has no entry for.
        assert_eq!(config.filter_width_hz, 7_000_000);
        assert!(!config.amp_enabled);
        assert!(!config.bias_tee_enabled);
    }

    /// Golden vectors against libhackrf's `hackrf_compute_baseband_filter_bw`, which is what
    /// every other host tool snaps with — a HackRF opened here and in `hackrf_transfer` must
    /// land on the same filter.
    #[test]
    fn filter_widths_snap_down_onto_the_max2837_table() {
        assert_eq!(snap_filter_width(1_750_000), 1_750_000);
        assert_eq!(snap_filter_width(2_499_999), 1_750_000);
        assert_eq!(snap_filter_width(2_500_000), 2_500_000);
        assert_eq!(snap_filter_width(7_500_000), 7_000_000);
        assert_eq!(snap_filter_width(28_000_000), 28_000_000);
        assert_eq!(snap_filter_width(100_000_000), 28_000_000);
        // Under the narrowest entry libhackrf returns the first one rather than nothing.
        assert_eq!(snap_filter_width(0), 1_750_000);
        assert_eq!(snap_filter_width(1_000_000), 1_750_000);
    }

    /// Every listed width is its own snap, so the table and the snapping cannot disagree.
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

    /// The rate-derived width is 0.75 × rate snapped down, and never wider than the rate
    /// itself — a filter wider than the complex bandwidth would alias its own skirts back in.
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

    /// Off-grid gains are the failure the M5 field session found downstream: the radio silently
    /// snapped 13 dB to 16 and reported back what was asked for.
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
        // The transmit VGA has a range but no grid.
        assert!(validate_tx_vga_gain(47).is_ok());
        assert!(validate_tx_vga_gain(13).is_ok());
        assert!(validate_tx_vga_gain(48).is_err());
    }
}
