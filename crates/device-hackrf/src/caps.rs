use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Capabilities, DcArtifact, DeviceSettings, Duplex, ExtraSetting, ExtraValue, GainStage,
    GainValue, Range, StreamScope, any_range_holds,
};

use crate::driver::{Config, FILTER_WIDTHS_HZ, snap_filter_width};

pub(crate) const ANTENNA: &str = "RX";
pub(crate) const LNA_STAGE: &str = "LNA";
pub(crate) const VGA_STAGE: &str = "VGA";
pub(crate) const AMP_STAGE: &str = "AMP";
pub(crate) const BIAS_TEE_SETTING: &str = "bias_tee";

const FREQ_MIN_HZ: f64 = 1e6;
const FREQ_MAX_HZ: f64 = 6e9;
const RATE_MIN_HZ: f64 = 2e6;
const RATE_MAX_HZ: f64 = 20e6;
const LNA_MAX_DB: f64 = 40.0;
const LNA_STEP_DB: f64 = 8.0;
const VGA_MAX_DB: f64 = 62.0;
const VGA_STEP_DB: f64 = 2.0;
/// The RF amplifier is a switch, not a control, so it travels as the two-setting gain stage the
/// wire model reserves for that — the same shape SoapyHackRF gives the same radio.
const AMP_DB: f64 = 14.0;

pub(crate) fn capabilities() -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: FREQ_MIN_HZ,
            max: FREQ_MAX_HZ,
            step: None,
        }],
        sample_rates: Vec::new(),
        sample_rate_ranges: vec![Range {
            min: RATE_MIN_HZ,
            max: RATE_MAX_HZ,
            step: None,
        }],
        gains: vec![
            GainStage {
                name: LNA_STAGE.to_string(),
                range: Range {
                    min: 0.0,
                    max: LNA_MAX_DB,
                    step: Some(LNA_STEP_DB),
                },
                values: Vec::new(),
            },
            GainStage {
                name: AMP_STAGE.to_string(),
                range: Range {
                    min: 0.0,
                    max: AMP_DB,
                    step: Some(AMP_DB),
                },
                values: Vec::new(),
            },
            GainStage {
                name: VGA_STAGE.to_string(),
                range: Range {
                    min: 0.0,
                    max: VGA_MAX_DB,
                    step: Some(VGA_STEP_DB),
                },
                values: Vec::new(),
            },
        ],
        antennas: vec![ANTENNA.to_string()],
        bandwidths: FILTER_WIDTHS_HZ.iter().copied().map(f64::from).collect(),
        bandwidth_ranges: Vec::new(),
        extra: vec![ExtraSetting::Bool {
            name: BIAS_TEE_SETTING.to_string(),
            default: false,
        }],
        ppm: false,
        duplex: Duplex::Half,
        rx_streams: 1,
        tx_streams: 1,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Managed,
    }
}

const FILTER_MATCH_RATE_HZ: f64 = 0.0;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) frequency_hz: Option<u64>,
    pub(crate) sample_rate_hz: Option<u32>,
    pub(crate) filter: Option<FilterWidth>,
    pub(crate) lna_gain_db: Option<u8>,
    pub(crate) vga_gain_db: Option<u8>,
    pub(crate) amp: Option<bool>,
    pub(crate) bias_tee: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FilterWidth {
    MatchRate,
    Hz(u32),
}

pub(crate) fn validate(
    delta: &DeviceSettings,
    caps: &Capabilities,
) -> Result<Applied, DeviceError> {
    check_stream_settings(delta, caps)?;
    let mut applied = Applied::default();

    if let Some(hz) = delta.center_hz {
        if !caps.freq_ranges.iter().any(|r| r.min <= hz && hz <= r.max) {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz} outside tuner range"
            )));
        }
        applied.frequency_hz = Some(hz.round() as u64);
    }

    if let Some(rate) = delta.sample_rate {
        let in_list = caps.sample_rates.contains(&rate);
        if !in_list && !any_range_holds(&caps.sample_rate_ranges, rate) {
            return Err(DeviceError::Unsupported(format!("sample_rate {rate}")));
        }
        applied.sample_rate_hz = Some(rate.round() as u32);
    }

    if delta.ppm.is_some() {
        return Err(DeviceError::Unsupported(
            "ppm: HackRF has no frequency-correction register".to_string(),
        ));
    }

    if let Some(bandwidth) = delta.bandwidth {
        let widest = FILTER_WIDTHS_HZ[FILTER_WIDTHS_HZ.len() - 1];
        applied.filter = Some(if bandwidth == FILTER_MATCH_RATE_HZ {
            FilterWidth::MatchRate
        } else if bandwidth.is_finite() && bandwidth > 0.0 && bandwidth <= f64::from(widest) {
            FilterWidth::Hz(snap_filter_width(bandwidth.round() as u32))
        } else {
            return Err(DeviceError::Unsupported(format!(
                "bandwidth {bandwidth} outside 0..{widest} Hz (0 matches sample_rate)"
            )));
        });
    }

    if let Some(antenna) = &delta.antenna
        && !caps.antennas.contains(antenna)
    {
        return Err(DeviceError::Unsupported(format!("antenna {antenna}")));
    }

    for gain in &delta.gains {
        let stage = caps
            .gains
            .iter()
            .find(|s| s.name == gain.stage)
            .ok_or_else(|| DeviceError::Unsupported(format!("gain stage {}", gain.stage)))?;
        if !(stage.range.min..=stage.range.max).contains(&gain.value_db) {
            return Err(DeviceError::Unsupported(format!(
                "gain {} {} dB outside {}..{} dB",
                gain.stage, gain.value_db, stage.range.min, stage.range.max
            )));
        }
        let snapped = stage.snap(gain.value_db);
        match stage.name.as_str() {
            LNA_STAGE => applied.lna_gain_db = Some(snapped.round() as u8),
            VGA_STAGE => applied.vga_gain_db = Some(snapped.round() as u8),
            AMP_STAGE => applied.amp = Some(snapped > 0.0),
            other => return Err(DeviceError::Unsupported(format!("gain stage {other}"))),
        }
    }

    for extra in &delta.extra {
        let setting = caps
            .extra
            .iter()
            .find(|s| s.name() == extra.name)
            .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", extra.name)))?;
        let enabled = match setting {
            ExtraSetting::Bool { .. } => extra.value.as_bool(),
            ExtraSetting::Range { .. }
            | ExtraSetting::Enum { .. }
            | ExtraSetting::String { .. } => None,
        };
        let enabled = enabled.ok_or_else(|| {
            DeviceError::Unsupported(format!(
                "extra setting {}: bad value {}",
                extra.name, extra.value
            ))
        })?;
        match setting.name() {
            BIAS_TEE_SETTING => applied.bias_tee = Some(enabled),
            other => return Err(DeviceError::Unsupported(format!("extra setting {other}"))),
        }
    }

    Ok(applied)
}

pub(crate) fn settings_from_config(config: &Config) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(config.frequency_hz as f64),
        sample_rate: Some(f64::from(config.sample_rate_hz)),
        ppm: None,
        antenna: Some(ANTENNA.to_string()),
        bandwidth: Some(f64::from(config.filter_width_hz)),
        dc_block: None,
        lo_offset_hz: None,
        gains: vec![
            GainValue {
                stage: LNA_STAGE.to_string(),
                value_db: f64::from(config.lna_gain_db),
            },
            GainValue {
                stage: AMP_STAGE.to_string(),
                value_db: if config.amp_enabled { AMP_DB } else { 0.0 },
            },
            GainValue {
                stage: VGA_STAGE.to_string(),
                value_db: f64::from(config.vga_gain_db),
            },
        ],
        extra: vec![ExtraValue {
            name: BIAS_TEE_SETTING.to_string(),
            value: config.bias_tee_enabled.into(),
        }],
        streams: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain(stage: &str, value_db: f64) -> GainValue {
        GainValue {
            stage: stage.to_string(),
            value_db,
        }
    }

    fn extra_bool(name: &str, value: bool) -> ExtraValue {
        ExtraValue {
            name: name.to_string(),
            value: value.into(),
        }
    }

    fn extra_text(name: &str, value: &str) -> ExtraValue {
        ExtraValue {
            name: name.to_string(),
            value: value.into(),
        }
    }

    #[test]
    fn capabilities_describe_the_hackrf_one() {
        let caps = capabilities();
        assert_eq!(
            caps.freq_ranges,
            vec![Range {
                min: 1e6,
                max: 6e9,
                step: None
            }]
        );
        assert!(caps.sample_rates.is_empty());
        assert_eq!(
            caps.sample_rate_ranges,
            vec![Range {
                min: 2e6,
                max: 20e6,
                step: None
            }]
        );
        assert_eq!(caps.gains.len(), 3);
        assert_eq!(caps.gains[0].name, "LNA");
        assert_eq!(caps.gains[0].range.step, Some(8.0));
        assert_eq!(caps.gains[1].name, "AMP");
        assert_eq!(caps.gains[2].name, "VGA");
        assert_eq!(caps.gains[2].range.step, Some(2.0));
        assert_eq!(caps.antennas, vec!["RX".to_string()]);
        assert_eq!(caps.bandwidths.len(), 16);
        assert_eq!(caps.bandwidths.first(), Some(&1.75e6));
        assert_eq!(caps.bandwidths.last(), Some(&28e6));
        assert!(caps.bandwidths.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            caps.extra
                .iter()
                .map(ExtraSetting::name)
                .collect::<Vec<_>>(),
            vec!["bias_tee"]
        );
        assert_eq!(caps.duplex, Duplex::Half);
    }

    #[test]
    fn a_stage_quantises_to_its_own_grid() {
        let caps = capabilities();
        let lna = stage(&caps, LNA_STAGE);
        assert_eq!(lna.snap(-12.0), 0.0);
        assert_eq!(lna.snap(0.0), 0.0);
        assert_eq!(lna.snap(13.0), 16.0);
        assert_eq!(lna.snap(16.0), 16.0);
        assert_eq!(lna.snap(12.0), 16.0);
        assert_eq!(lna.snap(40.0), 40.0);
        assert_eq!(lna.snap(1_000.0), 40.0);
    }

    fn stage<'a>(caps: &'a Capabilities, name: &str) -> &'a GainStage {
        caps.gains
            .iter()
            .find(|stage| stage.name == name)
            .expect("the stage is advertised")
    }

    #[test]
    fn the_rf_amp_is_a_switched_gain_stage_not_a_boolean() {
        let caps = capabilities();
        let amp = stage(&caps, AMP_STAGE);
        assert!(amp.is_switch(), "an amp that is on or off is a switch");
        assert_eq!(amp.snap(0.0), 0.0);
        assert_eq!(amp.snap(6.0), 0.0, "below halfway stays off");
        assert_eq!(amp.snap(8.0), AMP_DB);
        assert_eq!(amp.snap(100.0), AMP_DB);
        assert!(
            !caps.extra.iter().any(|setting| setting.name() == "amp"),
            "the amp must not also be an extra"
        );
        assert!(!stage(&caps, LNA_STAGE).is_switch());
        assert!(!stage(&caps, VGA_STAGE).is_switch());
    }

    #[test]
    fn a_stage_quantises_to_the_vga_grid() {
        let caps = capabilities();
        let vga = stage(&caps, VGA_STAGE);
        assert_eq!(vga.snap(-0.5), 0.0);
        assert_eq!(vga.snap(0.0), 0.0);
        assert_eq!(vga.snap(1.0), 2.0);
        assert_eq!(vga.snap(20.0), 20.0);
        assert_eq!(vga.snap(20.9), 20.0);
        assert_eq!(vga.snap(62.0), 62.0);
        assert_eq!(vga.snap(99.0), 62.0);
    }

    #[test]
    fn validate_maps_a_full_delta_to_hardware_units() {
        let delta = DeviceSettings {
            center_hz: Some(433_920_000.0),
            sample_rate: Some(8_000_000.0),
            antenna: Some("RX".to_string()),
            bandwidth: Some(5_000_000.0),
            gains: vec![gain("LNA", 24.0), gain("AMP", 14.0), gain("VGA", 20.0)],
            extra: vec![extra_bool("bias_tee", false)],
            ..DeviceSettings::default()
        };
        assert_eq!(
            validate(&delta, &capabilities()).unwrap(),
            Applied {
                frequency_hz: Some(433_920_000),
                sample_rate_hz: Some(8_000_000),
                filter: Some(FilterWidth::Hz(5_000_000)),
                lna_gain_db: Some(24),
                vga_gain_db: Some(20),
                amp: Some(true),
                bias_tee: Some(false),
            }
        );
    }

    #[test]
    fn validate_snaps_gains_to_the_hardware_grid() {
        let delta = DeviceSettings {
            gains: vec![gain("LNA", 13.0), gain("VGA", 21.0)],
            ..DeviceSettings::default()
        };
        let applied = validate(&delta, &capabilities()).unwrap();
        assert_eq!(applied.lna_gain_db, Some(16));
        assert_eq!(applied.vga_gain_db, Some(22));
    }

    #[test]
    fn validate_rejects_center_outside_1mhz_to_6ghz() {
        for hz in [999_999.0, 6_000_000_001.0, -100e6] {
            let delta = DeviceSettings {
                center_hz: Some(hz),
                ..DeviceSettings::default()
            };
            assert!(
                matches!(
                    validate(&delta, &capabilities()),
                    Err(DeviceError::Unsupported(_))
                ),
                "center {hz} must be rejected"
            );
        }
        for hz in [1e6, 6e9] {
            let delta = DeviceSettings {
                center_hz: Some(hz),
                ..DeviceSettings::default()
            };
            assert!(validate(&delta, &capabilities()).is_ok(), "center {hz}");
        }
    }

    #[test]
    fn validate_rejects_sample_rate_outside_2_to_20_msps() {
        for rate in [1_999_999.0, 20_000_001.0, 0.0] {
            let delta = DeviceSettings {
                sample_rate: Some(rate),
                ..DeviceSettings::default()
            };
            assert!(
                matches!(
                    validate(&delta, &capabilities()),
                    Err(DeviceError::Unsupported(_))
                ),
                "rate {rate} must be rejected"
            );
        }
        for rate in [2e6, 10e6, 20e6] {
            let delta = DeviceSettings {
                sample_rate: Some(rate),
                ..DeviceSettings::default()
            };
            assert!(validate(&delta, &capabilities()).is_ok(), "rate {rate}");
        }
    }

    #[test]
    fn validate_rejects_non_finite_values() {
        let bad = [
            DeviceSettings {
                center_hz: Some(f64::NAN),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                center_hz: Some(f64::INFINITY),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                sample_rate: Some(f64::NAN),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                sample_rate: Some(f64::NEG_INFINITY),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                gains: vec![gain("LNA", f64::NAN)],
                ..DeviceSettings::default()
            },
            DeviceSettings {
                gains: vec![gain("VGA", f64::INFINITY)],
                ..DeviceSettings::default()
            },
        ];
        for delta in &bad {
            assert!(
                matches!(
                    validate(delta, &capabilities()),
                    Err(DeviceError::Unsupported(_))
                ),
                "{delta:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_unknown_gain_stage() {
        let delta = DeviceSettings {
            gains: vec![gain("MIXER", 14.0)],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&delta, &capabilities()),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_rejects_gain_outside_the_stage_range() {
        for bad in [gain("LNA", 48.0), gain("LNA", -8.0), gain("VGA", 64.0)] {
            let delta = DeviceSettings {
                gains: vec![bad.clone()],
                ..DeviceSettings::default()
            };
            assert!(
                matches!(
                    validate(&delta, &capabilities()),
                    Err(DeviceError::Unsupported(_))
                ),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_unknown_and_mistyped_extras() {
        let unknown = DeviceSettings {
            extra: vec![extra_text("direct_samp", "1")],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&unknown, &capabilities()),
            Err(DeviceError::Unsupported(_))
        ));

        let mistyped = DeviceSettings {
            extra: vec![extra_text("bias_tee", "yes")],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&mistyped, &capabilities()),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_rejects_ppm_the_hardware_cannot_honour() {
        for ppm in [1.5, 0.0] {
            let delta = DeviceSettings {
                ppm: Some(ppm),
                ..DeviceSettings::default()
            };
            assert!(
                matches!(
                    validate(&delta, &capabilities()),
                    Err(DeviceError::Unsupported(_))
                ),
                "ppm {ppm} must be rejected"
            );
        }
        assert!(!capabilities().ppm);
    }

    fn filter_of(bandwidth: f64) -> Result<Option<FilterWidth>, DeviceError> {
        let delta = DeviceSettings {
            bandwidth: Some(bandwidth),
            ..DeviceSettings::default()
        };
        validate(&delta, &capabilities()).map(|applied| applied.filter)
    }

    #[test]
    fn validate_takes_every_listed_filter_width_as_it_is() {
        for width in FILTER_WIDTHS_HZ {
            assert_eq!(
                filter_of(f64::from(width)).unwrap(),
                Some(FilterWidth::Hz(width)),
                "{width}"
            );
        }
    }

    #[test]
    fn validate_snaps_a_width_between_two_register_steps() {
        assert_eq!(filter_of(7.5e6).unwrap(), Some(FilterWidth::Hz(7e6 as u32)));
        assert_eq!(filter_of(1.0).unwrap(), Some(FilterWidth::Hz(1_750_000)));
        assert_eq!(
            filter_of(27_999_999.0).unwrap(),
            Some(FilterWidth::Hz(24_000_000))
        );
    }

    #[test]
    fn validate_reads_zero_as_matching_the_sample_rate() {
        assert_eq!(filter_of(0.0).unwrap(), Some(FilterWidth::MatchRate));
        assert_eq!(
            validate(&DeviceSettings::default(), &capabilities())
                .unwrap()
                .filter,
            None
        );
    }

    #[test]
    fn validate_rejects_a_width_no_filter_could_hold() {
        for bad in [-1.0, 28_000_001.0, 1e9, f64::NAN, f64::INFINITY] {
            assert!(
                matches!(filter_of(bad), Err(DeviceError::Unsupported(_))),
                "bandwidth {bad} must be rejected"
            );
        }
    }

    #[test]
    fn validate_rejects_unknown_antenna() {
        let delta = DeviceSettings {
            antenna: Some("TX".to_string()),
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&delta, &capabilities()),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_of_an_empty_delta_writes_nothing() {
        assert_eq!(
            validate(&DeviceSettings::default(), &capabilities()).unwrap(),
            Applied::default()
        );
    }

    #[test]
    fn validate_refuses_per_stream_overrides() {
        let delta = DeviceSettings {
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 0,
                center_hz: Some(433_920_000.0),
                ..sdrmm_wire::StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        match validate(&delta, &capabilities()) {
            Err(DeviceError::Unsupported(message)) => {
                assert!(message.contains("streams[0]"), "{message}");
            }
            other => panic!("a streams entry must be Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn settings_mirror_the_drivers_applied_config() {
        let config = Config {
            frequency_hz: 100_000_000,
            sample_rate_hz: 2_000_000,
            lna_gain_db: 16,
            vga_gain_db: 30,
            tx_vga_gain_db: 0,
            filter_width_hz: 1_750_000,
            amp_enabled: true,
            bias_tee_enabled: false,
        };
        let settings = settings_from_config(&config);
        assert_eq!(settings.center_hz, Some(100e6));
        assert_eq!(settings.sample_rate, Some(2e6));
        assert_eq!(settings.antenna.as_deref(), Some("RX"));
        assert_eq!(settings.ppm, None);
        assert_eq!(settings.bandwidth, Some(1.75e6));
        assert_eq!(
            settings.gains,
            vec![gain("LNA", 16.0), gain("AMP", 14.0), gain("VGA", 30.0)],
            "the amp reports as the stage it is, at the gain it contributes"
        );
        assert_eq!(settings.extra, vec![extra_bool("bias_tee", false)]);
        let round_trip = validate(&settings, &capabilities()).expect("reported settings re-apply");
        assert_eq!(round_trip.sample_rate_hz, Some(2_000_000));
        assert_eq!(round_trip.filter, Some(FilterWidth::Hz(1_750_000)));
    }
}
