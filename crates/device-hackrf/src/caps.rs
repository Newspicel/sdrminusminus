//! Pure capability construction, gain quantisation and the pre-flight validation `apply`
//! runs before touching hardware. Nothing here does USB I/O, so every mapping and every
//! rejection path is unit-testable without a radio (PLAN §14: no hardware in CI, ever).

use hackrf_nusb::Config;
use sdrmm_device::DeviceError;
use sdrmm_wire::{
    Capabilities, DeviceSettings, ExtraSetting, ExtraValue, GainStage, GainValue, Range,
};

/// The HackRF has one SMA port shared by RX and TX; the list exists so the capability UI has
/// a name for it, and so a preset captured elsewhere round-trips.
pub(crate) const ANTENNA: &str = "RX";
/// MAX2837 RX IF gain. Named the way SoapyHackRF names its gain elements, so the controls do
/// not rename themselves when the same radio is opened through the other backend.
pub(crate) const LNA_STAGE: &str = "LNA";
/// MAX2837 baseband gain.
pub(crate) const VGA_STAGE: &str = "VGA";
/// The ~14 dB RF front-end amplifier: a switch, not a gain, so it is a typed extra rather
/// than a third gain stage with a two-point range.
pub(crate) const AMP_SETTING: &str = "amp";
/// Antenna-port bias power (bias tee), for powering an LNA up the coax.
pub(crate) const BIAS_TEE_SETTING: &str = "bias_tee";

/// Everything `hackrf-nusb` will accept (its `config` module enforces exactly these bounds,
/// which are the LPC/MAX2837/RFFC5072 limits libhackrf publishes).
const FREQ_MIN_HZ: f64 = 1e6;
const FREQ_MAX_HZ: f64 = 6e9;
const RATE_MIN_HZ: f64 = 2e6;
const RATE_MAX_HZ: f64 = 20e6;
const LNA_MAX_DB: f64 = 40.0;
const LNA_STEP_DB: f64 = 8.0;
const VGA_MAX_DB: f64 = 62.0;
const VGA_STEP_DB: f64 = 2.0;

/// What the HackRF One exposes for RX. Fixed rather than queried: the hardware limits are
/// the same on every board revision the crate opens, and it has no capability query to ask.
pub(crate) fn capabilities() -> Capabilities {
    Capabilities {
        freq_ranges: vec![Range {
            min: FREQ_MIN_HZ,
            max: FREQ_MAX_HZ,
            step: None,
        }],
        // Continuous, not a preset list: the Si5351C synthesizes the sample clock by
        // fractional division, so every rate in the band is reachable, and a non-empty
        // `sample_rates` would replace the client's free numeric field with a dropdown that
        // hides rates the hardware has.
        sample_rates: Vec::new(),
        sample_rate_range: Some(Range {
            min: RATE_MIN_HZ,
            max: RATE_MAX_HZ,
            step: None,
        }),
        // Two real stages instead of Soapy's collapsed "overall gain" — the reason this
        // backend exists. The steps are the MAX2837 register grids, not a UI preference.
        gains: vec![
            GainStage {
                name: LNA_STAGE.to_string(),
                range: Range {
                    min: 0.0,
                    max: LNA_MAX_DB,
                    step: Some(LNA_STEP_DB),
                },
            },
            GainStage {
                name: VGA_STAGE.to_string(),
                range: Range {
                    min: 0.0,
                    max: VGA_MAX_DB,
                    step: Some(VGA_STEP_DB),
                },
            },
        ],
        antennas: vec![ANTENNA.to_string()],
        // `hackrf-nusb` ties the MAX2837 baseband filter to the sample rate — its
        // `set_sample_rate_hz` issues BASEBAND_FILTER_BANDWIDTH_SET with the same value and
        // there is no independent public setter — so there is no bandwidth to offer. An
        // empty list here means "no such control", and [`validate`] rejects one rather than
        // accepting a value nothing would honour.
        bandwidths: Vec::new(),
        extra: vec![
            ExtraSetting::Bool {
                name: AMP_SETTING.to_string(),
                default: false,
            },
            ExtraSetting::Bool {
                name: BIAS_TEE_SETTING.to_string(),
                default: false,
            },
        ],
        tx_capable: false,
    }
}

/// The hardware writes one validated delta turns into. `None` is an untouched knob; every
/// `Some` is already in the unit and on the grid the device takes, so [`validate`] cannot be
/// followed by a rejection from the driver.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) frequency_hz: Option<u64>,
    pub(crate) sample_rate_hz: Option<u32>,
    pub(crate) lna_gain_db: Option<u8>,
    pub(crate) vga_gain_db: Option<u8>,
    pub(crate) amp: Option<bool>,
    pub(crate) bias_tee: Option<bool>,
}

/// Quantise a gain onto the stage's hardware grid. The MAX2837 gain registers only take 8 dB
/// (LNA) and 2 dB (VGA) increments, so a value between two steps has no representation at
/// all; snapping to the nearest one keeps a plain slider (or a preset recorded from another
/// radio) usable, and `settings()` reports the snapped value back so the choice is visible
/// rather than silent. Out-of-range values are clamped for totality — [`validate`] rejects
/// those before they reach here, because a range is a contract while a step is a rounding.
pub(crate) fn snap_gain(range: Range, value_db: f64) -> f64 {
    let clamped = value_db.clamp(range.min, range.max);
    match range.step.filter(|step| *step > 0.0) {
        Some(step) => {
            (range.min + ((clamped - range.min) / step).round() * step).clamp(range.min, range.max)
        }
        None => clamped,
    }
}

fn extra_name(setting: &ExtraSetting) -> &str {
    match setting {
        ExtraSetting::Bool { name, .. }
        | ExtraSetting::Range { name, .. }
        | ExtraSetting::Enum { name, .. } => name,
    }
}

/// Pre-flight for `apply`: reject everything the hardware cannot take *before* any control
/// transfer runs, so a bad field mid-batch cannot leave the radio half-retuned. Non-finite
/// values fail every range test (`NaN` compares false against both bounds), which is how
/// they are rejected without a special case.
pub(crate) fn validate(
    delta: &DeviceSettings,
    caps: &Capabilities,
) -> Result<Applied, DeviceError> {
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
        let in_span = caps
            .sample_rate_range
            .is_some_and(|r| r.min <= rate && rate <= r.max);
        if !in_list && !in_span {
            return Err(DeviceError::Unsupported(format!("sample_rate {rate}")));
        }
        applied.sample_rate_hz = Some(rate.round() as u32);
    }

    // No frequency-correction register exists on the HackRF and the crate exposes no
    // equivalent, so a correction cannot be honoured. Even `ppm: 0` is refused: accepting it
    // would report `ppm: None` afterwards, breaking the invariant that a successful `apply`
    // is reflected by `settings()`.
    if delta.ppm.is_some() {
        return Err(DeviceError::Unsupported(
            "ppm: HackRF has no frequency-correction register".to_string(),
        ));
    }

    // Same reasoning: the baseband filter follows the sample rate (see `capabilities`), so
    // an explicit bandwidth would be dropped rather than applied.
    if delta.bandwidth.is_some() {
        return Err(DeviceError::Unsupported(
            "bandwidth: the HackRF baseband filter follows sample_rate".to_string(),
        ));
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
        let value_db = snap_gain(stage.range, gain.value_db).round() as u8;
        match stage.name.as_str() {
            LNA_STAGE => applied.lna_gain_db = Some(value_db),
            VGA_STAGE => applied.vga_gain_db = Some(value_db),
            other => return Err(DeviceError::Unsupported(format!("gain stage {other}"))),
        }
    }

    for extra in &delta.extra {
        let setting = caps
            .extra
            .iter()
            .find(|s| extra_name(s) == extra.name)
            .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", extra.name)))?;
        // Every HackRF extra is a switch; a non-bool is a type mismatch, not a value the
        // hardware could round into range.
        let enabled = match setting {
            ExtraSetting::Bool { .. } => extra.value.as_bool(),
            ExtraSetting::Range { .. } | ExtraSetting::Enum { .. } => None,
        };
        let enabled = enabled.ok_or_else(|| {
            DeviceError::Unsupported(format!(
                "extra setting {}: bad value {}",
                extra.name, extra.value
            ))
        })?;
        match extra_name(setting) {
            AMP_SETTING => applied.amp = Some(enabled),
            BIAS_TEE_SETTING => applied.bias_tee = Some(enabled),
            other => return Err(DeviceError::Unsupported(format!("extra setting {other}"))),
        }
    }

    Ok(applied)
}

/// Mirror the driver's record of the last successfully applied configuration into the wire
/// model. This is the only path that writes `settings()`, so a snapped gain and a batch that
/// failed halfway are both reported as what the hardware actually holds — never as what was
/// asked for. `ppm`/`bandwidth` stay unset because [`validate`] refuses them.
pub(crate) fn settings_from_config(config: &Config) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(config.frequency_hz() as f64),
        sample_rate: Some(f64::from(config.sample_rate_hz())),
        ppm: None,
        antenna: Some(ANTENNA.to_string()),
        bandwidth: None,
        gains: vec![
            GainValue {
                stage: LNA_STAGE.to_string(),
                value_db: f64::from(config.lna_gain_db()),
            },
            GainValue {
                stage: VGA_STAGE.to_string(),
                value_db: f64::from(config.vga_gain_db()),
            },
        ],
        extra: vec![
            ExtraValue {
                name: AMP_SETTING.to_string(),
                value: config.amp_enabled().into(),
            },
            ExtraValue {
                name: BIAS_TEE_SETTING.to_string(),
                value: config.bias_tee_enabled().into(),
            },
        ],
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

    // `serde_json` is not a dependency of this crate: `ExtraValue::value` is built by
    // inference through `Into`, exactly as production code does.
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

    fn lna_range() -> Range {
        Range {
            min: 0.0,
            max: LNA_MAX_DB,
            step: Some(LNA_STEP_DB),
        }
    }

    fn vga_range() -> Range {
        Range {
            min: 0.0,
            max: VGA_MAX_DB,
            step: Some(VGA_STEP_DB),
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
            caps.sample_rate_range,
            Some(Range {
                min: 2e6,
                max: 20e6,
                step: None
            })
        );
        assert_eq!(caps.gains.len(), 2);
        assert_eq!(caps.gains[0].name, "LNA");
        assert_eq!(caps.gains[0].range.step, Some(8.0));
        assert_eq!(caps.gains[1].name, "VGA");
        assert_eq!(caps.gains[1].range.step, Some(2.0));
        assert_eq!(caps.antennas, vec!["RX".to_string()]);
        // No independent baseband-filter setter exists, so no bandwidth may be advertised.
        assert!(caps.bandwidths.is_empty());
        assert_eq!(
            caps.extra.iter().map(extra_name).collect::<Vec<_>>(),
            vec!["amp", "bias_tee"]
        );
        assert!(!caps.tx_capable);
    }

    #[test]
    fn snap_gain_quantises_the_lna_grid() {
        let range = lna_range();
        assert_eq!(snap_gain(range, -12.0), 0.0);
        assert_eq!(snap_gain(range, 0.0), 0.0);
        assert_eq!(snap_gain(range, 13.0), 16.0);
        assert_eq!(snap_gain(range, 16.0), 16.0);
        // Exact midpoint between two steps rounds away from zero (f64::round).
        assert_eq!(snap_gain(range, 12.0), 16.0);
        assert_eq!(snap_gain(range, 40.0), 40.0);
        assert_eq!(snap_gain(range, 1_000.0), 40.0);
    }

    #[test]
    fn snap_gain_quantises_the_vga_grid() {
        let range = vga_range();
        assert_eq!(snap_gain(range, -0.5), 0.0);
        assert_eq!(snap_gain(range, 0.0), 0.0);
        assert_eq!(snap_gain(range, 1.0), 2.0);
        assert_eq!(snap_gain(range, 20.0), 20.0);
        assert_eq!(snap_gain(range, 20.9), 20.0);
        assert_eq!(snap_gain(range, 62.0), 62.0);
        assert_eq!(snap_gain(range, 99.0), 62.0);
    }

    #[test]
    fn snap_gain_without_a_step_only_clamps() {
        let range = Range {
            min: 0.0,
            max: 10.0,
            step: None,
        };
        assert_eq!(snap_gain(range, 3.7), 3.7);
        assert_eq!(snap_gain(range, 11.0), 10.0);
    }

    #[test]
    fn validate_maps_a_full_delta_to_hardware_units() {
        let delta = DeviceSettings {
            center_hz: Some(433_920_000.0),
            sample_rate: Some(8_000_000.0),
            antenna: Some("RX".to_string()),
            gains: vec![gain("LNA", 24.0), gain("VGA", 20.0)],
            extra: vec![extra_bool("amp", true), extra_bool("bias_tee", false)],
            ..DeviceSettings::default()
        };
        assert_eq!(
            validate(&delta, &capabilities()).unwrap(),
            Applied {
                frequency_hz: Some(433_920_000),
                sample_rate_hz: Some(8_000_000),
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
            gains: vec![gain("AMP", 14.0)],
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
            extra: vec![extra_text("amp", "yes")],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&mistyped, &capabilities()),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_rejects_ppm_and_bandwidth_the_hardware_cannot_honour() {
        for delta in [
            DeviceSettings {
                ppm: Some(1.5),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                ppm: Some(0.0),
                ..DeviceSettings::default()
            },
            DeviceSettings {
                bandwidth: Some(5e6),
                ..DeviceSettings::default()
            },
        ] {
            assert!(
                matches!(
                    validate(&delta, &capabilities()),
                    Err(DeviceError::Unsupported(_))
                ),
                "{delta:?} must be rejected"
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
    fn settings_mirror_the_drivers_applied_config() {
        let config = Config::builder()
            .frequency_hz(100_000_000)
            .sample_rate_hz(2_000_000)
            .lna_gain_db(16)
            .vga_gain_db(30)
            .amp_enable(true)
            .bias_tee(false)
            .build()
            .unwrap();
        let settings = settings_from_config(&config);
        assert_eq!(settings.center_hz, Some(100e6));
        assert_eq!(settings.sample_rate, Some(2e6));
        assert_eq!(settings.antenna.as_deref(), Some("RX"));
        assert_eq!(settings.ppm, None);
        assert_eq!(settings.bandwidth, None);
        assert_eq!(settings.gains, vec![gain("LNA", 16.0), gain("VGA", 30.0)]);
        assert_eq!(
            settings.extra,
            vec![extra_bool("amp", true), extra_bool("bias_tee", false)]
        );
        // Every reported value must be one the capability model can express back.
        assert!(validate(&settings, &capabilities()).is_ok());
    }
}
