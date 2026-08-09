//! The validated receiver configuration, mirroring what the hardware last accepted.

use crate::error::{Error, Result};

/// Defaults `hackrf_transfer` starts from.
const DEFAULT_FREQUENCY_HZ: u64 = 900_000_000;
const DEFAULT_SAMPLE_RATE_HZ: u32 = 10_000_000;
const DEFAULT_LNA_GAIN_DB: u8 = 8;
const DEFAULT_VGA_GAIN_DB: u8 = 20;

/// What the radio holds.
///
/// A plain value type. [`Device::config`](crate::Device::config) hands out a snapshot whose
/// fields only moved once their control transfer succeeded, so it is the device's own truth —
/// including gains as the MAX2837's step grid snapped them, never as they were asked for. The
/// setters exist so a caller can describe a configuration without a radio attached; validation
/// lives on the `Device` setters, which are the only things that reach hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    frequency_hz: u64,
    sample_rate_hz: u32,
    lna_gain_db: u8,
    vga_gain_db: u8,
    amp_enabled: bool,
    bias_tee_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            frequency_hz: DEFAULT_FREQUENCY_HZ,
            sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
            lna_gain_db: DEFAULT_LNA_GAIN_DB,
            vga_gain_db: DEFAULT_VGA_GAIN_DB,
            amp_enabled: false,
            bias_tee_enabled: false,
        }
    }
}

impl Config {
    /// Tuned centre frequency in Hz.
    #[must_use]
    pub const fn frequency_hz(&self) -> u64 {
        self.frequency_hz
    }

    /// Complex IQ sample rate in Hz.
    #[must_use]
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    /// MAX2837 RX IF/LNA gain in dB.
    #[must_use]
    pub const fn lna_gain_db(&self) -> u8 {
        self.lna_gain_db
    }

    /// MAX2837 baseband/VGA gain in dB.
    #[must_use]
    pub const fn vga_gain_db(&self) -> u8 {
        self.vga_gain_db
    }

    /// Whether the RF amplifier is on.
    #[must_use]
    pub const fn amp_enabled(&self) -> bool {
        self.amp_enabled
    }

    /// Whether the antenna port is powered.
    #[must_use]
    pub const fn bias_tee_enabled(&self) -> bool {
        self.bias_tee_enabled
    }

    /// Set the centre frequency this configuration describes.
    pub const fn set_frequency_hz(&mut self, value: u64) {
        self.frequency_hz = value;
    }

    /// Set the sample rate this configuration describes.
    pub const fn set_sample_rate_hz(&mut self, value: u32) {
        self.sample_rate_hz = value;
    }

    /// Set the LNA gain this configuration describes.
    pub const fn set_lna_gain_db(&mut self, value: u8) {
        self.lna_gain_db = value;
    }

    /// Set the VGA gain this configuration describes.
    pub const fn set_vga_gain_db(&mut self, value: u8) {
        self.vga_gain_db = value;
    }

    /// Set the RF amplifier state this configuration describes.
    pub const fn set_amp_enabled(&mut self, value: bool) {
        self.amp_enabled = value;
    }

    /// Set the bias-tee state this configuration describes.
    pub const fn set_bias_tee_enabled(&mut self, value: bool) {
        self.bias_tee_enabled = value;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_hackrf_transfer() {
        let config = Config::default();
        assert_eq!(config.frequency_hz(), 900_000_000);
        assert_eq!(config.sample_rate_hz(), 10_000_000);
        assert_eq!(config.lna_gain_db(), 8);
        assert_eq!(config.vga_gain_db(), 20);
        assert!(!config.amp_enabled());
        assert!(!config.bias_tee_enabled());
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
    }
}
