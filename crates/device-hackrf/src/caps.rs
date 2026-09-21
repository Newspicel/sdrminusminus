use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Agc, BandwidthSetting, Capabilities, Coherence, DcArtifact, DeviceSettings, Duplex, GainKind,
    GainStage, GainValue, Range, StreamScope, any_range_holds,
};

use crate::driver::{Config, FILTER_WIDTHS_HZ, FilterWidth, snap_filter_width};

pub(crate) const ANTENNA: &str = "RX";

const FREQ_MIN_HZ: f64 = 1e6;
const FREQ_MAX_HZ: f64 = 6e9;
const RATE_MIN_HZ: f64 = 2e6;
const RATE_MAX_HZ: f64 = 20e6;
const LNA_MAX_DB: f64 = 40.0;
const LNA_STEP_DB: f64 = 8.0;
const VGA_MAX_DB: f64 = 62.0;
const VGA_STEP_DB: f64 = 2.0;
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
            GainStage::new(
                GainKind::Lna,
                Range {
                    min: 0.0,
                    max: LNA_MAX_DB,
                    step: Some(LNA_STEP_DB),
                },
            ),
            GainStage::new(
                GainKind::Amp,
                Range {
                    min: 0.0,
                    max: AMP_DB,
                    step: Some(AMP_DB),
                },
            ),
            GainStage::new(
                GainKind::Vga,
                Range {
                    min: 0.0,
                    max: VGA_MAX_DB,
                    step: Some(VGA_STEP_DB),
                },
            ),
        ],
        antennas: vec![ANTENNA.to_string()],
        bandwidths: FILTER_WIDTHS_HZ.iter().copied().map(f64::from).collect(),
        bandwidth_ranges: Vec::new(),
        bandwidth_auto: true,
        bias_tee: true,
        agc: Agc::None,
        extra: Vec::new(),
        ppm: false,
        duplex: Duplex::Half,
        rx_streams: 1,
        tx_streams: 1,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: true,
        coherence: Coherence::None,
        noise_source: false,
    }
}

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
        applied.filter = Some(filter_for(bandwidth)?);
    }

    if let Some(antenna) = &delta.antenna
        && !caps.antennas.contains(antenna)
    {
        return Err(DeviceError::Unsupported(format!("antenna {antenna}")));
    }

    for gain in &delta.gains {
        let stage = caps
            .stage(&gain.stage)
            .ok_or_else(|| DeviceError::Unsupported(format!("gain stage {}", gain.stage)))?;
        if !(stage.range.min..=stage.range.max).contains(&gain.value_db) {
            return Err(DeviceError::Unsupported(format!(
                "gain {} {} dB outside {}..{} dB",
                gain.stage, gain.value_db, stage.range.min, stage.range.max
            )));
        }
        let snapped = stage.snap(gain.value_db);
        match stage.kind {
            GainKind::Lna => applied.lna_gain_db = Some(snapped.round() as u8),
            GainKind::Vga => applied.vga_gain_db = Some(snapped.round() as u8),
            GainKind::Amp => applied.amp = Some(snapped > 0.0),
            _ => {
                return Err(DeviceError::Unsupported(format!(
                    "gain stage {}",
                    gain.stage
                )));
            }
        }
    }

    if delta.agc.is_some() {
        return Err(DeviceError::Unsupported(
            "agc: HackRF has no automatic gain".to_string(),
        ));
    }

    if let Some(extra) = delta.extra.first() {
        return Err(DeviceError::Unsupported(format!(
            "extra setting {}",
            extra.name
        )));
    }

    applied.bias_tee = delta.bias_tee;

    Ok(applied)
}

fn filter_for(bandwidth: BandwidthSetting) -> Result<FilterWidth, DeviceError> {
    let widest = FILTER_WIDTHS_HZ[FILTER_WIDTHS_HZ.len() - 1];
    match bandwidth {
        BandwidthSetting::Auto => Ok(FilterWidth::MatchRate),
        BandwidthSetting::Manual { hz }
            if hz.is_finite() && hz > 0.0 && hz <= f64::from(widest) =>
        {
            Ok(FilterWidth::Hz(snap_filter_width(hz.round() as u32)))
        }
        BandwidthSetting::Manual { hz } => Err(DeviceError::Unsupported(format!(
            "bandwidth {hz} outside 0..{widest} Hz"
        ))),
    }
}

pub(crate) fn settings_from_config(config: &Config) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(config.frequency_hz as f64),
        tuning: None,
        sample_rate: Some(f64::from(config.sample_rate_hz)),
        ppm: None,
        offset_hz: None,
        antenna: Some(ANTENNA.to_string()),
        bandwidth: Some(match config.filter {
            FilterWidth::MatchRate => BandwidthSetting::Auto,
            FilterWidth::Hz(hz) => BandwidthSetting::Manual { hz: f64::from(hz) },
        }),
        dc_block: None,
        bias_tee: Some(config.bias_tee_enabled),
        agc: None,
        gains: vec![
            GainValue::new(GainKind::Lna, f64::from(config.lna_gain_db)),
            GainValue::new(GainKind::Amp, if config.amp_enabled { AMP_DB } else { 0.0 }),
            GainValue::new(GainKind::Vga, f64::from(config.vga_gain_db)),
        ],
        extra: Vec::new(),
        streams: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AgcSetting, ExtraValue};

    use super::*;

    fn gain(kind: GainKind, value_db: f64) -> GainValue {
        GainValue::new(kind, value_db)
    }

    fn manual(hz: f64) -> Option<BandwidthSetting> {
        Some(BandwidthSetting::Manual { hz })
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
        assert_eq!(caps.gains[0].kind, GainKind::Lna);
        assert_eq!(caps.gains[0].range.step, Some(8.0));
        assert_eq!(caps.gains[1].name, "AMP");
        assert_eq!(caps.gains[1].kind, GainKind::Amp);
        assert_eq!(caps.gains[2].name, "VGA");
        assert_eq!(caps.gains[2].kind, GainKind::Vga);
        assert_eq!(caps.gains[2].range.step, Some(2.0));
        assert_eq!(caps.antennas, vec!["RX".to_string()]);
        assert_eq!(caps.bandwidths.len(), 16);
        assert_eq!(caps.bandwidths.first(), Some(&1.75e6));
        assert_eq!(caps.bandwidths.last(), Some(&28e6));
        assert!(caps.bandwidths.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(caps.bandwidth_auto);
        assert!(caps.bias_tee);
        assert_eq!(caps.agc, Agc::None);
        assert!(caps.extra.is_empty());
        assert_eq!(caps.duplex, Duplex::Half);
    }

    #[test]
    fn a_stage_quantises_to_its_own_grid() {
        let caps = capabilities();
        let lna = stage(&caps, GainKind::Lna);
        assert_eq!(lna.snap(-12.0), 0.0);
        assert_eq!(lna.snap(0.0), 0.0);
        assert_eq!(lna.snap(13.0), 16.0);
        assert_eq!(lna.snap(16.0), 16.0);
        assert_eq!(lna.snap(12.0), 16.0);
        assert_eq!(lna.snap(40.0), 40.0);
        assert_eq!(lna.snap(1_000.0), 40.0);
    }

    fn stage(caps: &Capabilities, kind: GainKind) -> &GainStage {
        caps.stage(kind.name()).expect("the stage is advertised")
    }

    #[test]
    fn the_rf_amp_is_a_switched_gain_stage_not_a_boolean() {
        let caps = capabilities();
        let amp = stage(&caps, GainKind::Amp);
        assert!(amp.is_switch(), "an amp that is on or off is a switch");
        assert_eq!(amp.setting_count(), 2);
        assert_eq!(amp.off(), 0.0);
        assert_eq!(amp.on(), AMP_DB);
        assert_eq!(amp.snap(0.0), 0.0);
        assert_eq!(amp.snap(6.0), 0.0, "below halfway stays off");
        assert_eq!(amp.snap(8.0), AMP_DB);
        assert_eq!(amp.snap(100.0), AMP_DB);
        assert!(!stage(&caps, GainKind::Lna).is_switch());
        assert!(!stage(&caps, GainKind::Vga).is_switch());
    }

    #[test]
    fn a_stage_quantises_to_the_vga_grid() {
        let caps = capabilities();
        let vga = stage(&caps, GainKind::Vga);
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
            bandwidth: manual(5_000_000.0),
            bias_tee: Some(false),
            gains: vec![
                gain(GainKind::Lna, 24.0),
                gain(GainKind::Amp, 14.0),
                gain(GainKind::Vga, 20.0),
            ],
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
            gains: vec![gain(GainKind::Lna, 13.0), gain(GainKind::Vga, 21.0)],
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
                gains: vec![gain(GainKind::Lna, f64::NAN)],
                ..DeviceSettings::default()
            },
            DeviceSettings {
                gains: vec![gain(GainKind::Vga, f64::INFINITY)],
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
            gains: vec![gain(GainKind::Mixer, 14.0)],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&delta, &capabilities()),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_rejects_gain_outside_the_stage_range() {
        for bad in [
            gain(GainKind::Lna, 48.0),
            gain(GainKind::Lna, -8.0),
            gain(GainKind::Vga, 64.0),
        ] {
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
    fn validate_rejects_every_extra_and_any_agc() {
        let extra = DeviceSettings {
            extra: vec![ExtraValue {
                name: "bias_tee".to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&extra, &capabilities()),
            Err(DeviceError::Unsupported(_))
        ));

        let agc = DeviceSettings {
            agc: Some(AgcSetting::switched(true)),
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&agc, &capabilities()),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn the_bias_tee_is_a_typed_switch() {
        let delta = DeviceSettings {
            bias_tee: Some(true),
            ..DeviceSettings::default()
        };
        assert_eq!(
            validate(&delta, &capabilities()).unwrap().bias_tee,
            Some(true)
        );
        let config = Config {
            bias_tee_enabled: true,
            ..Config::default()
        };
        assert_eq!(settings_from_config(&config).bias_tee, Some(true));
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

    fn filter_of(bandwidth: BandwidthSetting) -> Result<Option<FilterWidth>, DeviceError> {
        let delta = DeviceSettings {
            bandwidth: Some(bandwidth),
            ..DeviceSettings::default()
        };
        validate(&delta, &capabilities()).map(|applied| applied.filter)
    }

    fn filter_of_hz(hz: f64) -> Result<Option<FilterWidth>, DeviceError> {
        filter_of(BandwidthSetting::Manual { hz })
    }

    #[test]
    fn validate_takes_every_listed_filter_width_as_it_is() {
        for width in FILTER_WIDTHS_HZ {
            assert_eq!(
                filter_of_hz(f64::from(width)).unwrap(),
                Some(FilterWidth::Hz(width)),
                "{width}"
            );
        }
    }

    #[test]
    fn validate_snaps_a_width_between_two_register_steps() {
        assert_eq!(
            filter_of_hz(7.5e6).unwrap(),
            Some(FilterWidth::Hz(7e6 as u32))
        );
        assert_eq!(filter_of_hz(1.0).unwrap(), Some(FilterWidth::Hz(1_750_000)));
        assert_eq!(
            filter_of_hz(27_999_999.0).unwrap(),
            Some(FilterWidth::Hz(24_000_000))
        );
    }

    #[test]
    fn validate_reads_auto_as_matching_the_sample_rate() {
        assert_eq!(
            filter_of(BandwidthSetting::Auto).unwrap(),
            Some(FilterWidth::MatchRate)
        );
        assert_eq!(
            validate(&DeviceSettings::default(), &capabilities())
                .unwrap()
                .filter,
            None
        );
    }

    #[test]
    fn validate_rejects_a_width_no_filter_could_hold() {
        for bad in [-1.0, 0.0, 28_000_001.0, 1e9, f64::NAN, f64::INFINITY] {
            assert!(
                matches!(filter_of_hz(bad), Err(DeviceError::Unsupported(_))),
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
    fn a_matched_filter_reports_as_auto_and_re_applies_as_one() {
        let settings = settings_from_config(&Config::default());
        assert_eq!(settings.bandwidth, Some(BandwidthSetting::Auto));
        assert_eq!(
            validate(&settings, &capabilities()).unwrap().filter,
            Some(FilterWidth::MatchRate)
        );
        assert!(capabilities().admits_bandwidth(BandwidthSetting::Auto));
        assert!(!capabilities().bandwidths.contains(&0.0));
    }

    #[test]
    fn settings_mirror_the_drivers_applied_config() {
        let config = Config {
            frequency_hz: 100_000_000,
            sample_rate_hz: 2_000_000,
            lna_gain_db: 16,
            vga_gain_db: 30,
            tx_vga_gain_db: 0,
            filter: FilterWidth::Hz(1_750_000),
            amp_enabled: true,
            bias_tee_enabled: false,
        };
        let settings = settings_from_config(&config);
        assert_eq!(settings.center_hz, Some(100e6));
        assert_eq!(settings.sample_rate, Some(2e6));
        assert_eq!(settings.antenna.as_deref(), Some("RX"));
        assert_eq!(settings.ppm, None);
        assert_eq!(settings.bandwidth, manual(1.75e6));
        assert_eq!(
            settings.gains,
            vec![
                gain(GainKind::Lna, 16.0),
                gain(GainKind::Amp, 14.0),
                gain(GainKind::Vga, 30.0)
            ],
            "the amp reports as the stage it is, at the gain it contributes"
        );
        assert_eq!(settings.bias_tee, Some(false));
        assert_eq!(settings.agc, None);
        assert!(settings.extra.is_empty());
        let round_trip = validate(&settings, &capabilities()).expect("reported settings re-apply");
        assert_eq!(round_trip.sample_rate_hz, Some(2_000_000));
        assert_eq!(round_trip.filter, Some(FilterWidth::Hz(1_750_000)));
        assert_eq!(round_trip.bias_tee, Some(false));
    }
}
