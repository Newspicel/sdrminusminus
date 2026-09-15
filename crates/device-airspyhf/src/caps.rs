use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Capabilities, DcArtifact, DeviceSettings, Duplex, ExtraSetting, ExtraValue, GainStage,
    GainValue, Range, StreamScope,
};

use crate::driver::{ATTENUATION_STEP_DB, Config, MAX_ATTENUATION_STEP};

pub(crate) const ANTENNA: &str = "RX";
pub(crate) const LNA_STAGE: &str = "LNA";
pub(crate) const ATTENUATOR_STAGE: &str = "ATT";
pub(crate) const AGC_SETTING: &str = "agc";
pub(crate) const AGC_THRESHOLD_SETTING: &str = "agc_high_threshold";
pub(crate) const BIAS_TEE_SETTING: &str = "bias_tee";

const LNA_DB: f64 = 6.0;
const HF_MAX_HZ: f64 = 31e6;
const VHF_MIN_HZ: f64 = 60e6;
const VHF_MAX_HZ: f64 = 260e6;

pub(crate) fn capabilities(sample_rates: &[u32]) -> Capabilities {
    Capabilities {
        freq_ranges: vec![
            Range {
                min: 1e3,
                max: HF_MAX_HZ,
                step: None,
            },
            Range {
                min: VHF_MIN_HZ,
                max: VHF_MAX_HZ,
                step: None,
            },
        ],
        sample_rates: sample_rates.iter().copied().map(f64::from).collect(),
        sample_rate_ranges: Vec::new(),
        gains: vec![
            // The preamp is a switch rather than a control, so it travels as the two-setting
            // stage the wire model reserves for that.
            GainStage {
                name: LNA_STAGE.to_string(),
                range: Range {
                    min: 0.0,
                    max: LNA_DB,
                    step: Some(LNA_DB),
                },
                values: Vec::new(),
            },
            // The attenuator only ever takes signal away, so it is spelled as the negative gain
            // it is rather than as a positive number that means the opposite.
            GainStage {
                name: ATTENUATOR_STAGE.to_string(),
                range: Range {
                    min: -f64::from(MAX_ATTENUATION_STEP) * ATTENUATION_STEP_DB,
                    max: 0.0,
                    step: Some(ATTENUATION_STEP_DB),
                },
                values: Vec::new(),
            },
        ],
        antennas: vec![ANTENNA.to_string()],
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        extra: vec![
            ExtraSetting::Bool {
                name: AGC_SETTING.to_string(),
                default: true,
            },
            ExtraSetting::Bool {
                name: AGC_THRESHOLD_SETTING.to_string(),
                default: false,
            },
            ExtraSetting::Bool {
                name: BIAS_TEE_SETTING.to_string(),
                default: false,
            },
        ],
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        coherence: sdrmm_wire::Coherence::None,
        noise_source: false,
    }
}

pub(crate) fn settings(config: &Config) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(f64::from(config.frequency_hz)),
        sample_rate: Some(f64::from(config.sample_rate_hz)),
        antenna: Some(ANTENNA.to_string()),
        gains: vec![
            GainValue {
                stage: LNA_STAGE.to_string(),
                value_db: if config.lna { LNA_DB } else { 0.0 },
            },
            GainValue {
                stage: ATTENUATOR_STAGE.to_string(),
                value_db: -f64::from(config.attenuation_step) * ATTENUATION_STEP_DB,
            },
        ],
        extra: vec![
            ExtraValue {
                name: AGC_SETTING.to_string(),
                value: config.agc.into(),
            },
            ExtraValue {
                name: AGC_THRESHOLD_SETTING.to_string(),
                value: config.agc_high_threshold.into(),
            },
            ExtraValue {
                name: BIAS_TEE_SETTING.to_string(),
                value: config.bias_tee.into(),
            },
        ],
        ..DeviceSettings::default()
    }
}

pub(crate) fn validate(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
) -> Result<(), DeviceError> {
    check_stream_settings(delta, capabilities)?;
    if let Some(center_hz) = delta.center_hz
        && !sdrmm_wire::any_range_holds(&capabilities.freq_ranges, center_hz)
    {
        return Err(DeviceError::Unsupported(format!(
            "{center_hz} Hz is outside the HF and VHF windows this radio covers"
        )));
    }
    if let Some(rate) = delta.sample_rate
        && !capabilities
            .sample_rates
            .iter()
            .any(|published| (published - rate).abs() < 1.0)
    {
        return Err(DeviceError::Unsupported(format!(
            "{rate} Hz is not one of the rates this radio publishes"
        )));
    }
    if let Some(antenna) = &delta.antenna
        && antenna != ANTENNA
    {
        return Err(DeviceError::Unsupported(format!(
            "this radio has one input, {ANTENNA}, not {antenna}"
        )));
    }
    for gain in &delta.gains {
        let stage = capabilities
            .gains
            .iter()
            .find(|stage| stage.name == gain.stage)
            .ok_or_else(|| DeviceError::Unsupported(format!("no {} stage", gain.stage)))?;
        if gain.value_db < stage.range.min || gain.value_db > stage.range.max {
            return Err(DeviceError::Unsupported(format!(
                "{} takes {} to {} dB",
                gain.stage, stage.range.min, stage.range.max
            )));
        }
    }
    Ok(())
}

pub(crate) fn attenuation_step(value_db: f64) -> Result<u8, DeviceError> {
    if !value_db.is_finite() || value_db > 0.0 {
        return Err(DeviceError::Unsupported(format!(
            "{value_db} dB is not an attenuation"
        )));
    }
    let step = (-value_db / ATTENUATION_STEP_DB).round();
    if step > f64::from(MAX_ATTENUATION_STEP) {
        return Err(DeviceError::Unsupported(format!(
            "{value_db} dB is beyond the attenuator"
        )));
    }
    Ok(step as u8)
}

pub(crate) fn extra_bool(value: &serde_json::Value, name: &str) -> Result<bool, DeviceError> {
    value
        .as_bool()
        .ok_or_else(|| DeviceError::Unsupported(format!("{name} is a switch, not {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> Capabilities {
        capabilities(&[768_000, 384_000, 256_000])
    }

    fn tuned(hz: f64) -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(hz),
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn both_tuning_windows_are_offered_as_separate_ranges() {
        assert_eq!(caps().freq_ranges.len(), 2, "the gap must stay a gap");
    }

    #[test]
    fn the_gap_between_the_windows_is_refused() {
        let caps = caps();
        for (hz, ok) in [
            (14.2e6, true),
            (31e6, true),
            (45e6, false),
            (60e6, true),
            (260e6, true),
            (300e6, false),
        ] {
            assert_eq!(validate(&tuned(hz), &caps).is_ok(), ok, "{hz} Hz");
        }
    }

    #[test]
    fn the_attenuator_is_spelled_as_the_negative_gain_it_is() {
        let caps = caps();
        let stage = caps
            .gains
            .iter()
            .find(|stage| stage.name == ATTENUATOR_STAGE)
            .expect("attenuator");
        assert_eq!(stage.range.max, 0.0);
        assert_eq!(stage.range.min, -48.0);
        assert_eq!(stage.range.step, Some(6.0));
    }

    #[test]
    fn attenuation_maps_onto_whole_firmware_steps() {
        assert_eq!(attenuation_step(0.0).expect("step"), 0);
        assert_eq!(attenuation_step(-6.0).expect("step"), 1);
        assert_eq!(attenuation_step(-48.0).expect("step"), 8);
        assert_eq!(attenuation_step(-7.0).expect("step"), 1);
        assert!(attenuation_step(6.0).is_err(), "gain is not attenuation");
        assert!(attenuation_step(-54.0).is_err());
        assert!(attenuation_step(f64::NAN).is_err());
    }

    #[test]
    fn a_gain_beyond_a_stage_is_refused() {
        let caps = caps();
        let delta = DeviceSettings {
            gains: vec![GainValue {
                stage: ATTENUATOR_STAGE.to_string(),
                value_db: -54.0,
            }],
            ..DeviceSettings::default()
        };
        assert!(validate(&delta, &caps).is_err());
    }

    #[test]
    fn the_published_rates_are_the_only_ones_accepted() {
        let caps = caps();
        let ok = DeviceSettings {
            sample_rate: Some(384_000.0),
            ..DeviceSettings::default()
        };
        assert!(validate(&ok, &caps).is_ok());
        let bad = DeviceSettings {
            sample_rate: Some(912_000.0),
            ..DeviceSettings::default()
        };
        assert!(validate(&bad, &caps).is_err());
    }

    #[test]
    fn settings_report_the_preamp_as_a_switch_and_the_attenuator_as_decibels() {
        let reported = settings(&Config {
            lna: true,
            attenuation_step: 3,
            ..Config::default()
        });
        let lna = reported
            .gains
            .iter()
            .find(|gain| gain.stage == LNA_STAGE)
            .expect("lna");
        assert_eq!(lna.value_db, 6.0);
        let att = reported
            .gains
            .iter()
            .find(|gain| gain.stage == ATTENUATOR_STAGE)
            .expect("att");
        assert_eq!(att.value_db, -18.0);
    }
}
