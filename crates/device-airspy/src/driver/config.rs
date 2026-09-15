use super::error::{Error, Result};

pub(crate) const MAX_LNA_GAIN: u8 = 14;
pub(crate) const MAX_MIXER_GAIN: u8 = 15;
pub(crate) const MAX_VGA_GAIN: u8 = 15;

pub(crate) const FREQ_MIN_HZ: u32 = 24_000_000;
pub(crate) const FREQ_MAX_HZ: u32 = 1_800_000_000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Config {
    pub(crate) frequency_hz: u32,
    pub(crate) sample_rate_hz: u32,
    pub(crate) lna_gain: u8,
    pub(crate) mixer_gain: u8,
    pub(crate) vga_gain: u8,
    pub(crate) lna_agc: bool,
    pub(crate) mixer_agc: bool,
    pub(crate) bias_tee: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            frequency_hz: 100_000_000,
            sample_rate_hz: 0,
            lna_gain: 7,
            mixer_gain: 8,
            vga_gain: 10,
            lna_agc: false,
            mixer_agc: false,
            bias_tee: false,
        }
    }
}

pub(crate) fn validate_frequency(frequency_hz: u32) -> Result<()> {
    if !(FREQ_MIN_HZ..=FREQ_MAX_HZ).contains(&frequency_hz) {
        return Err(Error::invalid_config(
            "frequency",
            "an Airspy tunes from 24 MHz to 1.8 GHz",
        ));
    }
    Ok(())
}

pub(crate) fn validate_gain(stage: &'static str, gain: u8, max: u8) -> Result<()> {
    if gain > max {
        return Err(Error::invalid_config(stage, "gain index beyond this stage"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tuning_range_is_the_one_the_radio_covers() {
        assert!(validate_frequency(24_000_000).is_ok());
        assert!(validate_frequency(1_800_000_000).is_ok());
        assert!(validate_frequency(23_999_999).is_err());
        assert!(validate_frequency(1_800_000_001).is_err());
    }

    #[test]
    fn each_gain_stage_stops_at_its_own_last_step() {
        assert!(validate_gain("LNA", MAX_LNA_GAIN, MAX_LNA_GAIN).is_ok());
        assert!(validate_gain("LNA", MAX_LNA_GAIN + 1, MAX_LNA_GAIN).is_err());
        assert!(validate_gain("VGA", MAX_VGA_GAIN, MAX_VGA_GAIN).is_ok());
        assert!(validate_gain("VGA", MAX_VGA_GAIN + 1, MAX_VGA_GAIN).is_err());
    }
}
