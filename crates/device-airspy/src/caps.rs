use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Agc, AgcSetting, ArgumentOption, Capabilities, Coherence, DcArtifact, DeviceSettings, Duplex,
    GainKind, GainStage, GainUnit, GainValue, Range, StreamScope, any_range_holds,
};

use crate::driver::{Config, MAX_LNA_GAIN, MAX_MIXER_GAIN, MAX_VGA_GAIN};

pub(crate) const ANTENNA: &str = "RX";
pub(crate) const AGC_BOTH: &str = "both";
pub(crate) const AGC_LNA: &str = "lna";
pub(crate) const AGC_MIXER: &str = "mixer";

const FREQ_MIN_HZ: f64 = 24e6;
const FREQ_MAX_HZ: f64 = 1.8e9;

fn stage(kind: GainKind, max: u8) -> GainStage {
    GainStage::new(
        kind,
        Range {
            min: 0.0,
            max: f64::from(max),
            step: Some(1.0),
        },
    )
    .with_unit(GainUnit::Index)
}

fn agc_mode(value: &str, label: &str) -> ArgumentOption {
    ArgumentOption {
        value: value.to_string(),
        label: Some(label.to_string()),
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
            stage(GainKind::Lna, MAX_LNA_GAIN),
            stage(GainKind::Mixer, MAX_MIXER_GAIN),
            stage(GainKind::Vga, MAX_VGA_GAIN),
        ],
        antennas: vec![ANTENNA.to_string()],
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: false,
        bias_tee: true,
        agc: Agc::Modes {
            options: vec![
                agc_mode(AGC_BOTH, "LNA + mixer"),
                agc_mode(AGC_LNA, "LNA"),
                agc_mode(AGC_MIXER, "Mixer"),
            ],
        },
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::None,
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
        bias_tee: Some(config.bias_tee),
        agc: Some(agc_setting(config.lna_agc, config.mixer_agc)),
        gains: vec![
            GainValue::new(GainKind::Lna, f64::from(config.lna_gain)),
            GainValue::new(GainKind::Mixer, f64::from(config.mixer_gain)),
            GainValue::new(GainKind::Vga, f64::from(config.vga_gain)),
        ],
        ..DeviceSettings::default()
    }
}

pub(crate) fn agc_setting(lna: bool, mixer: bool) -> AgcSetting {
    match (lna, mixer) {
        (true, true) => AgcSetting::in_mode(true, AGC_BOTH),
        (true, false) => AgcSetting::in_mode(true, AGC_LNA),
        (false, true) => AgcSetting::in_mode(true, AGC_MIXER),
        (false, false) => AgcSetting::off(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgcSwitches {
    pub(crate) lna: bool,
    pub(crate) mixer: bool,
}

pub(crate) fn agc_switches(setting: &AgcSetting) -> Result<AgcSwitches, DeviceError> {
    if !setting.on {
        return Ok(AgcSwitches {
            lna: false,
            mixer: false,
        });
    }
    match setting.mode.as_deref() {
        None | Some(AGC_BOTH) => Ok(AgcSwitches {
            lna: true,
            mixer: true,
        }),
        Some(AGC_LNA) => Ok(AgcSwitches {
            lna: true,
            mixer: false,
        }),
        Some(AGC_MIXER) => Ok(AgcSwitches {
            lna: false,
            mixer: true,
        }),
        Some(other) => Err(DeviceError::Unsupported(format!("no {other} AGC mode"))),
    }
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
                "{} takes steps {} to {}",
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
        agc_switches(agc)?;
    }
    if let Some(extra) = delta.extra.first() {
        return Err(DeviceError::Unsupported(format!(
            "no {} setting",
            extra.name
        )));
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

#[cfg(test)]
mod tests {
    use sdrmm_wire::ExtraValue;

    use super::*;

    fn caps() -> Capabilities {
        capabilities(&[10_000_000, 2_500_000])
    }

    fn with_agc(agc: AgcSetting) -> DeviceSettings {
        DeviceSettings {
            agc: Some(agc),
            ..DeviceSettings::default()
        }
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
    fn every_stage_is_a_firmware_step_index() {
        let caps = caps();
        assert_eq!(
            caps.gains
                .iter()
                .map(|stage| (stage.name.as_str(), stage.kind, stage.unit))
                .collect::<Vec<_>>(),
            vec![
                ("LNA", GainKind::Lna, GainUnit::Index),
                ("MIX", GainKind::Mixer, GainUnit::Index),
                ("VGA", GainKind::Vga, GainUnit::Index),
            ]
        );
        assert!(caps.bias_tee);
        assert!(caps.extra.is_empty());
    }

    #[test]
    fn each_stage_stops_at_its_last_step() {
        let caps = caps();
        for (kind, max) in [
            (GainKind::Lna, MAX_LNA_GAIN),
            (GainKind::Mixer, MAX_MIXER_GAIN),
            (GainKind::Vga, MAX_VGA_GAIN),
        ] {
            let inside = DeviceSettings {
                gains: vec![GainValue::new(kind, f64::from(max))],
                ..DeviceSettings::default()
            };
            assert!(validate(&inside, &caps).is_ok(), "{kind:?}");
            let beyond = DeviceSettings {
                gains: vec![GainValue::new(kind, f64::from(max) + 1.0)],
                ..DeviceSettings::default()
            };
            assert!(validate(&beyond, &caps).is_err(), "{kind:?}");
        }
    }

    #[test]
    fn an_unknown_gain_stage_is_refused() {
        let delta = DeviceSettings {
            gains: vec![GainValue::new(GainKind::Amp, 1.0)],
            ..DeviceSettings::default()
        };
        assert!(validate(&delta, &caps()).is_err());
        assert!(stage_kind(&caps(), "AMP").is_err());
        assert_eq!(stage_kind(&caps(), "MIX").expect("mixer"), GainKind::Mixer);
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
        assert!(reported.extra.is_empty());
        assert_eq!(reported.bias_tee, Some(false));
        assert_eq!(reported.agc, Some(AgcSetting::off()));
        assert_eq!(reported.antenna.as_deref(), Some(ANTENNA));
    }

    #[test]
    fn agc_modes_pick_which_of_the_two_loops_run() {
        let caps = caps();
        for (setting, lna, mixer) in [
            (AgcSetting::off(), false, false),
            (AgcSetting::switched(true), true, true),
            (AgcSetting::in_mode(true, AGC_BOTH), true, true),
            (AgcSetting::in_mode(true, AGC_LNA), true, false),
            (AgcSetting::in_mode(true, AGC_MIXER), false, true),
            (AgcSetting::in_mode(false, AGC_LNA), false, false),
        ] {
            assert!(
                validate(&with_agc(setting.clone()), &caps).is_ok(),
                "{setting:?}"
            );
            assert_eq!(
                agc_switches(&setting).expect("known mode"),
                AgcSwitches { lna, mixer },
                "{setting:?}"
            );
        }
        assert!(validate(&with_agc(AgcSetting::in_mode(true, "vga")), &caps).is_err());
        assert!(agc_switches(&AgcSetting::in_mode(true, "vga")).is_err());
    }

    #[test]
    fn agc_state_round_trips_through_settings() {
        for (lna, mixer) in [(false, false), (true, false), (false, true), (true, true)] {
            let reported = agc_setting(lna, mixer);
            assert_eq!(reported.on, lna || mixer);
            assert_eq!(
                agc_switches(&reported).expect("reported mode"),
                AgcSwitches { lna, mixer }
            );
        }
        let config = Config {
            lna_agc: true,
            ..Config::default()
        };
        assert_eq!(
            settings(&config).agc,
            Some(AgcSetting::in_mode(true, AGC_LNA))
        );
    }

    #[test]
    fn extras_and_filters_are_refused() {
        let caps = caps();
        let extra = DeviceSettings {
            extra: vec![ExtraValue {
                name: "lna_agc".to_string(),
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
