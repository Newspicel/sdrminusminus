use super::error::{Error, Result};

/// The attenuator moves in 6 dB steps, and the firmware takes the step rather than the level.
pub(crate) const ATTENUATION_STEP_DB: f64 = 6.0;
pub(crate) const MAX_ATTENUATION_STEP: u8 = 8;

pub(crate) const HF_MAX_HZ: u32 = 31_000_000;
pub(crate) const VHF_MIN_HZ: u32 = 60_000_000;
pub(crate) const VHF_MAX_HZ: u32 = 260_000_000;

pub(crate) const MAX_PPM: f64 = 200.0;

const ZERO_IF_LOWEST_LO_KHZ: u32 = 180;
const LOW_IF_LOWEST_LO_KHZ: u32 = 84;
const CALIBRATION_MAGIC: u32 = 0xA5CA_71B0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Config {
    pub(crate) asked_hz: u32,
    pub(crate) lo_khz: u32,
    pub(crate) ppm: f64,
    pub(crate) sample_rate_hz: u32,
    pub(crate) attenuation_step: u8,
    pub(crate) lna: bool,
    pub(crate) agc: bool,
    pub(crate) agc_high_threshold: bool,
    pub(crate) filter_gain_db: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            asked_hz: 14_200_000,
            lo_khz: 0,
            ppm: 0.0,
            sample_rate_hz: 0,
            attenuation_step: 0,
            lna: false,
            agc: true,
            agc_high_threshold: false,
            filter_gain_db: 0,
        }
    }
}

impl Config {
    pub(crate) fn center_hz(&self) -> f64 {
        f64::from(self.lo_khz) * 1000.0 * (1.0 + self.ppm * 1e-6)
    }
}

/// The receiver covers HF up to 31 MHz and a VHF window from 60 to 260 MHz, with nothing between
/// the two: a frequency in the gap has no tuning to reach it rather than a poor one.
pub(crate) fn validate_frequency(frequency_hz: u32) -> Result<()> {
    if frequency_hz <= HF_MAX_HZ || (VHF_MIN_HZ..=VHF_MAX_HZ).contains(&frequency_hz) {
        return Ok(());
    }
    Err(Error::invalid_config(
        "frequency",
        "an HF+ covers up to 31 MHz and 60 to 260 MHz, and nothing between them",
    ))
}

pub(crate) fn tuned_khz(frequency_hz: u32, low_if: bool, ppm: f64) -> u32 {
    let khz = (f64::from(frequency_hz) / (1.0 + ppm * 1e-6) / 1000.0).round() as u32;
    khz.max(lowest_lo_khz(low_if))
}

pub(crate) fn validate_ppm(ppm: f64) -> Result<()> {
    if ppm.is_finite() && (-MAX_PPM..=MAX_PPM).contains(&ppm) {
        return Ok(());
    }
    Err(Error::invalid_config(
        "ppm",
        "the correction stops at 200 ppm",
    ))
}

pub(crate) fn decode_calibration_ppm(bytes: &[u8]) -> Option<f64> {
    let magic = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    if magic != CALIBRATION_MAGIC {
        return None;
    }
    let ppb = i32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    let ppm = -f64::from(ppb) / 1000.0;
    validate_ppm(ppm).ok().map(|()| ppm)
}

pub(crate) const fn lowest_lo_khz(low_if: bool) -> u32 {
    if low_if {
        LOW_IF_LOWEST_LO_KHZ
    } else {
        ZERO_IF_LOWEST_LO_KHZ
    }
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
    fn tuning_rounds_to_the_nearest_kilohertz() {
        assert_eq!(tuned_khz(14_200_499, false, 0.0), 14_200);
        assert_eq!(tuned_khz(14_200_500, false, 0.0), 14_201);
        assert_eq!(tuned_khz(VHF_MAX_HZ, true, 0.0), 260_000);
    }

    #[test]
    fn the_oscillator_never_drops_below_the_floor_of_the_rate_in_use() {
        assert_eq!(tuned_khz(1_000, false, 0.0), 180);
        assert_eq!(tuned_khz(100_000, false, 0.0), 180);
        assert_eq!(tuned_khz(180_000, false, 0.0), 180);
        assert_eq!(tuned_khz(1_000, true, 0.0), 84);
        assert_eq!(tuned_khz(100_000, true, 0.0), 100);
        assert_eq!(tuned_khz(1_000, true, 200.0), 84);
    }

    #[test]
    fn a_fast_clock_is_tuned_low_and_reported_where_it_lands() {
        let ppm = 50.0;
        let khz = tuned_khz(100_000_000, false, ppm);
        assert_eq!(khz, 99_995);
        let config = Config {
            lo_khz: khz,
            ppm,
            ..Config::default()
        };
        assert!((config.center_hz() - 99_999_999.75).abs() < 1e-3);
        assert!((config.center_hz() - 100e6).abs() < 500.0 * (1.0 + ppm * 1e-6));
    }

    #[test]
    fn ppm_stops_at_its_limit() {
        assert!(validate_ppm(0.0).is_ok());
        assert!(validate_ppm(-MAX_PPM).is_ok());
        assert!(validate_ppm(MAX_PPM).is_ok());
        assert!(validate_ppm(MAX_PPM + 0.1).is_err());
        assert!(validate_ppm(f64::NAN).is_err());
    }

    #[test]
    fn a_stored_calibration_reads_as_the_clock_error_it_corrects() {
        let mut bytes = vec![0xff; 256];
        bytes[..4].copy_from_slice(&CALIBRATION_MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&(-1_500i32).to_le_bytes());
        assert_eq!(decode_calibration_ppm(&bytes), Some(1.5));
        bytes[4..8].copy_from_slice(&2_250i32.to_le_bytes());
        assert_eq!(decode_calibration_ppm(&bytes), Some(-2.25));
    }

    #[test]
    fn blank_or_absurd_flash_holds_no_calibration() {
        assert_eq!(decode_calibration_ppm(&[0xff; 256]), None);
        assert_eq!(decode_calibration_ppm(&[]), None);
        let mut bytes = vec![0; 8];
        bytes[..4].copy_from_slice(&CALIBRATION_MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert_eq!(decode_calibration_ppm(&bytes), None);
    }

    #[test]
    fn the_attenuator_stops_at_its_last_step() {
        assert!(validate_attenuation(0).is_ok());
        assert!(validate_attenuation(MAX_ATTENUATION_STEP).is_ok());
        assert!(validate_attenuation(MAX_ATTENUATION_STEP + 1).is_err());
        assert!((f64::from(MAX_ATTENUATION_STEP) * ATTENUATION_STEP_DB - 48.0).abs() < 1e-9);
    }
}
