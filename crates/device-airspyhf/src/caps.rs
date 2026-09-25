use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Agc, AgcSetting, ArgumentOption, Capabilities, Coherence, DcArtifact, DeviceSettings, Duplex,
    GainKind, GainStage, GainValue, Range, StreamScope,
};

use crate::driver::{ATTENUATION_STEP_DB, Config, MAX_ATTENUATION_STEP};

pub(crate) const ANTENNA: &str = "RX";
pub(crate) const AGC_LOW: &str = "low";
pub(crate) const AGC_HIGH: &str = "high";

const PREAMP_DB: f64 = 6.0;
const HF_MAX_HZ: f64 = 31e6;
const VHF_MIN_HZ: f64 = 60e6;
const VHF_MAX_HZ: f64 = 260e6;

fn agc_mode(value: &str, label: &str) -> ArgumentOption {
    ArgumentOption {
        value: value.to_string(),
        label: Some(label.to_string()),
    }
}

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
            GainStage::new(
                GainKind::Amp,
                Range {
                    min: 0.0,
                    max: PREAMP_DB,
                    step: Some(PREAMP_DB),
                },
            ),
            GainStage::new(
                GainKind::Attenuator,
                Range {
                    min: -f64::from(MAX_ATTENUATION_STEP) * ATTENUATION_STEP_DB,
                    max: 0.0,
                    step: Some(ATTENUATION_STEP_DB),
                },
            ),
        ],
        antennas: vec![ANTENNA.to_string()],
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: false,
        bias_tee: false,
        agc: Agc::Modes {
            options: vec![
                agc_mode(AGC_LOW, "Low threshold"),
                agc_mode(AGC_HIGH, "High threshold"),
            ],
        },
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        coherence: Coherence::None,
        noise_source: false,
    }
}

pub(crate) fn settings(config: &Config) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(f64::from(config.frequency_hz)),
        sample_rate: Some(f64::from(config.sample_rate_hz)),
        antenna: Some(ANTENNA.to_string()),
        bias_tee: None,
        agc: Some(agc_setting(config.agc, config.agc_high_threshold)),
        gains: vec![
            GainValue::new(GainKind::Amp, if config.lna { PREAMP_DB } else { 0.0 }),
            GainValue::new(
                GainKind::Attenuator,
                -f64::from(config.attenuation_step) * ATTENUATION_STEP_DB,
            ),
        ],
        ..DeviceSettings::default()
    }
}

pub(crate) fn agc_setting(on: bool, high_threshold: bool) -> AgcSetting {
    AgcSetting::in_mode(on, if high_threshold { AGC_HIGH } else { AGC_LOW })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgcWrites {
    pub(crate) on: bool,
    pub(crate) high_threshold: Option<bool>,
}

pub(crate) fn agc_writes(setting: &AgcSetting) -> Result<AgcWrites, DeviceError> {
    let high_threshold = match setting.mode.as_deref() {
        None => None,
        Some(AGC_LOW) => Some(false),
        Some(AGC_HIGH) => Some(true),
        Some(other) => return Err(DeviceError::Unsupported(format!("no {other} AGC mode"))),
    };
    Ok(AgcWrites {
        on: setting.on,
        high_threshold,
    })
}

pub(crate) fn stage_kind(capabilities: &Capabilities, name: &str) -> Result<GainKind, DeviceError> {
    capabilities
        .stage(name)
        .map(|stage| stage.kind)
        .ok_or_else(|| DeviceError::Unsupported(format!("no {name} stage")))
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
    if delta.bandwidth.is_some() {
        return Err(DeviceError::Unsupported(
            "this radio has no selectable filter".to_string(),
        ));
    }
    for gain in &delta.gains {
        let stage = capabilities
            .stage(&gain.stage)
            .ok_or_else(|| DeviceError::Unsupported(format!("no {} stage", gain.stage)))?;
        if gain.value_db < stage.range.min || gain.value_db > stage.range.max {
            return Err(DeviceError::Unsupported(format!(
                "{} takes {} to {} dB",
                gain.stage, stage.range.min, stage.range.max
            )));
        }
    }
    if let Some(agc) = &delta.agc {
        if !capabilities.agc.admits(agc) {
            return Err(DeviceError::Unsupported(format!(
                "no {} AGC mode",
                agc.mode.as_deref().unwrap_or("(none)")
            )));
        }
        agc_writes(agc)?;
    }
    if let Some(extra) = delta.extra.first() {
        return Err(DeviceError::Unsupported(format!(
            "no {} setting",
            extra.name
        )));
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

#[cfg(test)]
mod tests {
    use sdrmm_wire::ExtraValue;

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

    fn with_agc(agc: AgcSetting) -> DeviceSettings {
        DeviceSettings {
            agc: Some(agc),
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
    fn the_preamp_is_a_switched_amp_stage() {
        let caps = caps();
        let amp = caps.stage(GainKind::Amp.name()).expect("preamp");
        assert_eq!(amp.name, "AMP");
        assert!(amp.is_switch());
        assert_eq!(amp.setting_count(), 2);
        assert_eq!(amp.off(), 0.0);
        assert_eq!(amp.on(), 6.0);
        assert!(caps.stage("LNA").is_none());
        assert!(!caps.bias_tee);
        assert!(caps.extra.is_empty());
    }

    #[test]
    fn the_attenuator_is_spelled_as_the_negative_gain_it_is() {
        let caps = caps();
        let stage = caps.stage(GainKind::Attenuator.name()).expect("attenuator");
        assert_eq!(stage.name, "ATT");
        assert_eq!(stage.range.max, 0.0);
        assert_eq!(stage.range.min, -48.0);
        assert_eq!(stage.range.step, Some(6.0));
        assert_eq!(stage_kind(&caps, "ATT").expect("att"), GainKind::Attenuator);
        assert!(stage_kind(&caps, "LNA").is_err());
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
            gains: vec![GainValue::new(GainKind::Attenuator, -54.0)],
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
            bias_tee: true,
            ..Config::default()
        });
        assert_eq!(reported.gain(GainKind::Amp.name()), Some(6.0));
        assert_eq!(reported.gain(GainKind::Attenuator.name()), Some(-18.0));
        assert_eq!(reported.bias_tee, None);
        assert!(reported.extra.is_empty());
    }

    #[test]
    fn agc_modes_pick_the_threshold() {
        let caps = caps();
        for (setting, on, high) in [
            (AgcSetting::off(), false, None),
            (AgcSetting::switched(true), true, None),
            (AgcSetting::in_mode(true, AGC_LOW), true, Some(false)),
            (AgcSetting::in_mode(true, AGC_HIGH), true, Some(true)),
            (AgcSetting::in_mode(false, AGC_HIGH), false, Some(true)),
        ] {
            assert!(
                validate(&with_agc(setting.clone()), &caps).is_ok(),
                "{setting:?}"
            );
            assert_eq!(
                agc_writes(&setting).expect("known mode"),
                AgcWrites {
                    on,
                    high_threshold: high
                },
                "{setting:?}"
            );
        }
        assert!(validate(&with_agc(AgcSetting::in_mode(true, "mid")), &caps).is_err());
        assert!(agc_writes(&AgcSetting::in_mode(true, "mid")).is_err());
    }

    #[test]
    fn agc_state_round_trips_through_settings() {
        for (on, high) in [(false, false), (true, false), (false, true), (true, true)] {
            let reported = agc_setting(on, high);
            assert_eq!(reported.on, on);
            assert_eq!(
                agc_writes(&reported).expect("reported mode"),
                AgcWrites {
                    on,
                    high_threshold: Some(high)
                }
            );
        }
        assert_eq!(
            settings(&Config::default()).agc,
            Some(AgcSetting::in_mode(true, AGC_LOW))
        );
    }

    #[test]
    fn extras_and_filters_are_refused() {
        let caps = caps();
        let extra = DeviceSettings {
            extra: vec![ExtraValue {
                name: "agc".to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        };
        assert!(validate(&extra, &caps).is_err());
        let filter = DeviceSettings {
            bandwidth: Some(sdrmm_wire::BandwidthSetting::Auto),
            ..DeviceSettings::default()
        };
        assert!(validate(&filter, &caps).is_err());
    }
}
