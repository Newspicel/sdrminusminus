use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Capabilities, DcArtifact, DeviceSettings, Duplex, ExtraSetting, ExtraValue, GainStage,
    GainValue, Range, StreamScope, any_range_holds,
};

use crate::driver::{Config, MAX_LNA_GAIN, MAX_MIXER_GAIN, MAX_VGA_GAIN};

pub(crate) const ANTENNA: &str = "RX";
pub(crate) const LNA_STAGE: &str = "LNA";
pub(crate) const MIXER_STAGE: &str = "MIX";
pub(crate) const VGA_STAGE: &str = "VGA";
pub(crate) const LNA_AGC_SETTING: &str = "lna_agc";
pub(crate) const MIXER_AGC_SETTING: &str = "mixer_agc";
pub(crate) const BIAS_TEE_SETTING: &str = "bias_tee";

const FREQ_MIN_HZ: f64 = 24e6;
const FREQ_MAX_HZ: f64 = 1.8e9;

/// Every stage is a step index rather than a level in dB: the firmware takes the index, and the
/// decibels each one buys change with the band.
fn stage(name: &str, max: u8) -> GainStage {
    GainStage {
        name: name.to_string(),
        range: Range {
            min: 0.0,
            max: f64::from(max),
            step: Some(1.0),
        },
        values: Vec::new(),
    }
}

pub(crate) fn capabilities(sample_rates: &[u32]) -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: FREQ_MIN_HZ,
            max: FREQ_MAX_HZ,
            step: None,
        }],
        sample_rates: sample_rates.iter().copied().map(f64::from).collect(),
        sample_rate_ranges: Vec::new(),
        gains: vec![
            stage(LNA_STAGE, MAX_LNA_GAIN),
            stage(MIXER_STAGE, MAX_MIXER_GAIN),
            stage(VGA_STAGE, MAX_VGA_GAIN),
        ],
        antennas: vec![ANTENNA.to_string()],
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        extra: vec![
            ExtraSetting::Bool {
                name: LNA_AGC_SETTING.to_string(),
                default: false,
            },
            ExtraSetting::Bool {
                name: MIXER_AGC_SETTING.to_string(),
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
        dc_artifact: DcArtifact::None,
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
                value_db: f64::from(config.lna_gain),
            },
            GainValue {
                stage: MIXER_STAGE.to_string(),
                value_db: f64::from(config.mixer_gain),
            },
            GainValue {
                stage: VGA_STAGE.to_string(),
                value_db: f64::from(config.vga_gain),
            },
        ],
        extra: vec![
            ExtraValue {
                name: LNA_AGC_SETTING.to_string(),
                value: config.lna_agc.into(),
            },
            ExtraValue {
                name: MIXER_AGC_SETTING.to_string(),
                value: config.mixer_agc.into(),
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
        && !any_range_holds(&capabilities.freq_ranges, center_hz)
    {
        return Err(DeviceError::Unsupported(format!(
            "{center_hz} Hz is outside 24 MHz to 1.8 GHz"
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
                "{} takes steps {} to {}",
                gain.stage, stage.range.min, stage.range.max
            )));
        }
    }
    Ok(())
}

pub(crate) fn gain_step(value_db: f64) -> Result<u8, DeviceError> {
    if !value_db.is_finite() || value_db < 0.0 {
        return Err(DeviceError::Unsupported(format!(
            "{value_db} is not a gain step"
        )));
    }
    u8::try_from(value_db.round() as i64)
        .map_err(|_| DeviceError::Unsupported(format!("{value_db} is beyond any gain step")))
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
        capabilities(&[10_000_000, 2_500_000])
    }

    #[test]
    fn the_published_rates_are_the_only_ones_accepted() {
        let caps = caps();
        for rate in [10_000_000.0, 2_500_000.0] {
            assert!(
                validate(
                    &DeviceSettings {
                        sample_rate: Some(rate),
                        ..DeviceSettings::default()
                    },
                    &caps
                )
                .is_ok(),
                "{rate}"
            );
        }
        assert!(
            validate(
                &DeviceSettings {
                    sample_rate: Some(6_000_000.0),
                    ..DeviceSettings::default()
                },
                &caps
            )
            .is_err(),
            "a rate the firmware never published has no index to send"
        );
    }

    #[test]
    fn tuning_outside_the_range_is_refused() {
        let caps = caps();
        for (hz, ok) in [(24e6, true), (1.8e9, true), (10e6, false), (2e9, false)] {
            let result = validate(
                &DeviceSettings {
                    center_hz: Some(hz),
                    ..DeviceSettings::default()
                },
                &caps,
            );
            assert_eq!(result.is_ok(), ok, "{hz} Hz");
        }
    }

    #[test]
    fn each_stage_stops_at_its_last_step() {
        let caps = caps();
        for (stage, max) in [
            (LNA_STAGE, MAX_LNA_GAIN),
            (MIXER_STAGE, MAX_MIXER_GAIN),
            (VGA_STAGE, MAX_VGA_GAIN),
        ] {
            let inside = DeviceSettings {
                gains: vec![GainValue {
                    stage: stage.to_string(),
                    value_db: f64::from(max),
                }],
                ..DeviceSettings::default()
            };
            assert!(validate(&inside, &caps).is_ok(), "{stage}");
            let beyond = DeviceSettings {
                gains: vec![GainValue {
                    stage: stage.to_string(),
                    value_db: f64::from(max) + 1.0,
                }],
                ..DeviceSettings::default()
            };
            assert!(validate(&beyond, &caps).is_err(), "{stage}");
        }
    }

    #[test]
    fn an_unknown_gain_stage_is_refused() {
        let delta = DeviceSettings {
            gains: vec![GainValue {
                stage: "AMP".to_string(),
                value_db: 1.0,
            }],
            ..DeviceSettings::default()
        };
        assert!(validate(&delta, &caps()).is_err());
    }

    #[test]
    fn the_only_input_is_the_one_the_radio_has() {
        let caps = caps();
        let rx = DeviceSettings {
            antenna: Some(ANTENNA.to_string()),
            ..DeviceSettings::default()
        };
        assert!(validate(&rx, &caps).is_ok());
        let other = DeviceSettings {
            antenna: Some("HF".to_string()),
            ..DeviceSettings::default()
        };
        assert!(validate(&other, &caps).is_err());
    }

    #[test]
    fn gain_steps_round_to_whole_indices() {
        assert_eq!(gain_step(7.0).expect("step"), 7);
        assert_eq!(gain_step(7.4).expect("step"), 7);
        assert_eq!(gain_step(6.6).expect("step"), 7);
        assert!(gain_step(-1.0).is_err());
        assert!(gain_step(f64::NAN).is_err());
        assert!(gain_step(400.0).is_err());
    }

    #[test]
    fn settings_report_every_stage_and_switch_the_radio_holds() {
        let reported = settings(&Config::default());
        assert_eq!(reported.gains.len(), 3);
        assert_eq!(reported.extra.len(), 3);
        assert_eq!(reported.antenna.as_deref(), Some(ANTENNA));
    }

    #[test]
    fn a_switch_only_takes_a_boolean() {
        assert!(extra_bool(&serde_json::Value::Bool(true), BIAS_TEE_SETTING).expect("bool"));
        assert!(extra_bool(&serde_json::json!("yes"), BIAS_TEE_SETTING).is_err());
    }
}
