use super::error::{Error, Result};

/// The attenuator moves in 6 dB steps, and the firmware takes the step rather than the level.
pub(crate) const ATTENUATION_STEP_DB: f64 = 6.0;
pub(crate) const MAX_ATTENUATION_STEP: u8 = 8;

pub(crate) const HF_MAX_HZ: u32 = 31_000_000;
pub(crate) const VHF_MIN_HZ: u32 = 60_000_000;
pub(crate) const VHF_MAX_HZ: u32 = 260_000_000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Config {
    pub(crate) frequency_hz: u32,
    pub(crate) sample_rate_hz: u32,
    pub(crate) attenuation_step: u8,
    pub(crate) lna: bool,
    pub(crate) agc: bool,
    pub(crate) agc_high_threshold: bool,
    pub(crate) bias_tee: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            frequency_hz: 14_200_000,
            sample_rate_hz: 0,
            attenuation_step: 0,
            lna: false,
            agc: true,
            agc_high_threshold: false,
            bias_tee: false,
        }
    }
}

/// The receiver covers HF up to 31 MHz and a VHF window from 60 to 260 MHz, with nothing between
/// the two — a frequency in the gap has no tuning to reach it rather than a poor one.
pub(crate) fn validate_frequency(frequency_hz: u32) -> Result<()> {
    if frequency_hz <= HF_MAX_HZ || (VHF_MIN_HZ..=VHF_MAX_HZ).contains(&frequency_hz) {
        return Ok(());
    }
    Err(Error::invalid_config(
        "frequency",
        "an HF+ covers up to 31 MHz and 60 to 260 MHz, and nothing between them",
    ))
}

pub(crate) fn validate_attenuation(step: u8) -> Result<()> {
    if step > MAX_ATTENUATION_STEP {
        return Err(Error::invalid_config(
            "attenuation",
            "the attenuator stops at 48 dB",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_tuning_windows_are_reachable() {
        for hz in [
            1_000,
            14_200_000,
            HF_MAX_HZ,
            VHF_MIN_HZ,
            144_000_000,
            VHF_MAX_HZ,
        ] {
            assert!(validate_frequency(hz).is_ok(), "{hz} Hz");
        }
    }

    #[test]
    fn the_gap_between_the_windows_is_refused_rather_than_tuned_badly() {
        for hz in [HF_MAX_HZ + 1, 40_000_000, VHF_MIN_HZ - 1, VHF_MAX_HZ + 1] {
            assert!(validate_frequency(hz).is_err(), "{hz} Hz");
        }
    }

    #[test]
    fn the_attenuator_stops_at_its_last_step() {
        assert!(validate_attenuation(0).is_ok());
        assert!(validate_attenuation(MAX_ATTENUATION_STEP).is_ok());
        assert!(validate_attenuation(MAX_ATTENUATION_STEP + 1).is_err());
        assert!((f64::from(MAX_ATTENUATION_STEP) * ATTENUATION_STEP_DB - 48.0).abs() < 1e-9);
    }
}
