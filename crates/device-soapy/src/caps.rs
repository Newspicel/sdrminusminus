//! Pure translation from SoapySDR channel queries to the wire capability model (PLAN §6),
//! plus the pre-flight validation `apply` runs before touching the device. No I/O here, so
//! every mapping is unit-testable with fabricated `soapysdr::Range`s (public fields).

use sdrmm_device::DeviceError;
use sdrmm_wire::{Capabilities, DeviceSettings, ExtraSetting, Range};

pub(crate) fn freq_ranges(ranges: &[soapysdr::Range]) -> Vec<Range> {
    ranges
        .iter()
        .map(|r| Range {
            min: r.minimum,
            max: r.maximum,
            // Soapy reports "no step constraint" as 0; the wire model uses None.
            step: (r.step > 0.0).then_some(r.step),
        })
        .collect()
}

/// Soapy expresses a discrete rate as a zero-width range (min == max) — there is no separate
/// discrete-list query — while a genuinely continuous range spans. Devices may mix both, so
/// the capability model keeps both fields: the list, and the span over the continuous parts.
pub(crate) fn rate_capabilities(ranges: &[soapysdr::Range]) -> (Vec<f64>, Option<Range>) {
    let mut discrete = Vec::new();
    let mut span: Option<Range> = None;
    for r in ranges {
        if r.minimum == r.maximum {
            discrete.push(r.minimum);
        } else {
            span = Some(match span {
                Some(s) => Range {
                    min: s.min.min(r.minimum),
                    max: s.max.max(r.maximum),
                    step: None,
                },
                None => Range {
                    min: r.minimum,
                    max: r.maximum,
                    step: None,
                },
            });
        }
    }
    (discrete, span)
}

/// Discrete (zero-width) points only — `Capabilities::bandwidths` is a select list, and a
/// continuous filter range has no representation there yet.
pub(crate) fn discrete_points(ranges: &[soapysdr::Range]) -> Vec<f64> {
    ranges
        .iter()
        .filter(|r| r.minimum == r.maximum)
        .map(|r| r.minimum)
        .collect()
}

/// The binding exposes no getSettingInfo, so per-driver extras come from this table keyed on
/// the enumerate args' "driver" value. Names/values are the Soapy modules' wire strings.
pub(crate) fn extra_settings(driver: &str) -> Vec<ExtraSetting> {
    match driver {
        "rtlsdr" => vec![
            bool_setting("biastee"),
            ExtraSetting::Enum {
                name: "direct_samp".to_string(),
                options: vec!["0".to_string(), "1".to_string(), "2".to_string()],
                default: "0".to_string(),
            },
            bool_setting("offset_tune"),
            bool_setting("digital_agc"),
        ],
        "hackrf" => vec![bool_setting("bias_tx")],
        _ => Vec::new(),
    }
}

fn bool_setting(name: &str) -> ExtraSetting {
    ExtraSetting::Bool {
        name: name.to_string(),
        default: false,
    }
}

fn extra_name(setting: &ExtraSetting) -> &str {
    match setting {
        ExtraSetting::Bool { name, .. }
        | ExtraSetting::Range { name, .. }
        | ExtraSetting::Enum { name, .. } => name,
    }
}

/// The string `write_setting` wants: bools as "true"/"false", enums as the raw option,
/// numbers via `Display`. Unknown names and mistyped or out-of-set values are `Unsupported`.
pub(crate) fn extra_write_value(
    extra: &[ExtraSetting],
    name: &str,
    value: &serde_json::Value,
) -> Result<String, DeviceError> {
    let setting = extra
        .iter()
        .find(|s| extra_name(s) == name)
        .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {name}")))?;
    let written = match setting {
        ExtraSetting::Bool { .. } => value.as_bool().map(|b| b.to_string()),
        ExtraSetting::Enum { options, .. } => value
            .as_str()
            .filter(|v| options.iter().any(|o| o == v))
            .map(str::to_string),
        ExtraSetting::Range { .. } => value.as_f64().map(|v| v.to_string()),
    };
    written
        .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {name}: bad value {value}")))
}

/// Pre-flight for `apply`: reject values the hardware cannot take before any setter runs —
/// otherwise a bad field mid-batch leaves the device half-retuned — and pre-compute the
/// `write_setting` strings so extras cannot fail halfway through either. `ppm_supported` is
/// whether the tuner exposes a "CORR" frequency component — without it ppm must error, never
/// be dropped silently.
pub(crate) fn validate(
    delta: &DeviceSettings,
    caps: &Capabilities,
    ppm_supported: bool,
) -> Result<Vec<(String, String)>, DeviceError> {
    if let Some(f) = delta.center_hz
        && !caps.freq_ranges.is_empty()
        && !caps.freq_ranges.iter().any(|r| r.min <= f && f <= r.max)
    {
        return Err(DeviceError::Unsupported(format!(
            "center_hz {f} outside tuner range"
        )));
    }
    // A device may expose a discrete list, a continuous span, or both; satisfying either is
    // enough, and a device constraining neither accepts any value.
    if let Some(rate) = delta.sample_rate {
        let constrained = !caps.sample_rates.is_empty() || caps.sample_rate_range.is_some();
        let in_list = caps.sample_rates.contains(&rate);
        let in_span = caps
            .sample_rate_range
            .is_some_and(|r| r.min <= rate && rate <= r.max);
        if constrained && !in_list && !in_span {
            return Err(DeviceError::Unsupported(format!("sample_rate {rate}")));
        }
    }
    // An empty bandwidth list means a continuous filter — any value is accepted there.
    if let Some(bw) = delta.bandwidth
        && !caps.bandwidths.is_empty()
        && !caps.bandwidths.contains(&bw)
    {
        return Err(DeviceError::Unsupported(format!("bandwidth {bw}")));
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
    }
    if let Some(antenna) = &delta.antenna
        && !caps.antennas.is_empty()
        && !caps.antennas.contains(antenna)
    {
        return Err(DeviceError::Unsupported(format!("antenna {antenna}")));
    }
    if delta.ppm.is_some() && !ppm_supported {
        return Err(DeviceError::Unsupported(
            "ppm: tuner has no CORR frequency component".to_string(),
        ));
    }
    delta
        .extra
        .iter()
        .map(|e| {
            Ok((
                e.name.clone(),
                extra_write_value(&caps.extra, &e.name, &e.value)?,
            ))
        })
        .collect()
}

/// Whether a `read_setting` echo confirms a value `write_setting` claimed to accept.
/// SoapyRTLSDR's writeSetting silently ignores unknown keys (and its biastee branch is
/// compiled out under an old librtlsdr) while readSetting returns "" for them, so an empty
/// echo means the write never took effect. Modules disagree on bool casing, hence the
/// case-insensitive bool compare; enum options must echo exactly.
pub(crate) fn read_back_confirms(written: &str, echoed: &str) -> bool {
    if echoed.is_empty() {
        return false;
    }
    if written.eq_ignore_ascii_case("true") || written.eq_ignore_ascii_case("false") {
        return written.eq_ignore_ascii_case(echoed);
    }
    written == echoed
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{ExtraValue, GainStage, GainValue};

    use super::*;

    fn soapy_range(minimum: f64, maximum: f64, step: f64) -> soapysdr::Range {
        soapysdr::Range {
            minimum,
            maximum,
            step,
        }
    }

    #[test]
    fn freq_ranges_map_zero_step_to_none() {
        let mapped = freq_ranges(&[soapy_range(24e6, 1.766e9, 0.0), soapy_range(2e9, 6e9, 1.0)]);
        assert_eq!(
            mapped,
            vec![
                Range {
                    min: 24e6,
                    max: 1.766e9,
                    step: None
                },
                Range {
                    min: 2e9,
                    max: 6e9,
                    step: Some(1.0)
                },
            ]
        );
    }

    #[test]
    fn discrete_rates_become_the_list() {
        let (discrete, span) = rate_capabilities(&[
            soapy_range(250_000.0, 250_000.0, 0.0),
            soapy_range(2_048_000.0, 2_048_000.0, 0.0),
        ]);
        assert_eq!(discrete, vec![250_000.0, 2_048_000.0]);
        assert_eq!(span, None);
    }

    #[test]
    fn continuous_rates_become_the_spanning_range() {
        let (discrete, span) =
            rate_capabilities(&[soapy_range(1e6, 10e6, 0.0), soapy_range(15e6, 20e6, 0.0)]);
        assert!(discrete.is_empty());
        assert_eq!(
            span,
            Some(Range {
                min: 1e6,
                max: 20e6,
                step: None
            })
        );
    }

    #[test]
    fn mixed_rates_keep_both_fields() {
        let (discrete, span) = rate_capabilities(&[
            soapy_range(250_000.0, 250_000.0, 0.0),
            soapy_range(1e6, 10e6, 0.0),
        ]);
        assert_eq!(discrete, vec![250_000.0]);
        assert_eq!(
            span,
            Some(Range {
                min: 1e6,
                max: 10e6,
                step: None
            })
        );
    }

    #[test]
    fn bandwidths_keep_only_discrete_points() {
        let points = discrete_points(&[
            soapy_range(290_000.0, 290_000.0, 0.0),
            soapy_range(1e6, 8e6, 0.0),
            soapy_range(3_570_000.0, 3_570_000.0, 0.0),
        ]);
        assert_eq!(points, vec![290_000.0, 3_570_000.0]);
    }

    #[test]
    fn extra_table_matches_known_drivers() {
        let rtl = extra_settings("rtlsdr");
        assert_eq!(rtl.len(), 4);
        assert!(matches!(
            &rtl[1],
            ExtraSetting::Enum { name, default, .. } if name == "direct_samp" && default == "0"
        ));
        assert_eq!(extra_settings("hackrf").len(), 1);
        assert!(extra_settings("airspy").is_empty());
    }

    #[test]
    fn extra_values_serialize_to_soapy_strings() {
        let extra = extra_settings("rtlsdr");
        assert_eq!(
            extra_write_value(&extra, "biastee", &serde_json::json!(true)).unwrap(),
            "true"
        );
        assert_eq!(
            extra_write_value(&extra, "digital_agc", &serde_json::json!(false)).unwrap(),
            "false"
        );
        assert_eq!(
            extra_write_value(&extra, "direct_samp", &serde_json::json!("2")).unwrap(),
            "2"
        );
    }

    #[test]
    fn extra_rejects_unknown_names_and_bad_values() {
        let extra = extra_settings("rtlsdr");
        assert!(matches!(
            extra_write_value(&extra, "nonexistent", &serde_json::json!(true)),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            extra_write_value(&extra, "biastee", &serde_json::json!("yes")),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            extra_write_value(&extra, "direct_samp", &serde_json::json!("3")),
            Err(DeviceError::Unsupported(_))
        ));
    }

    fn caps() -> Capabilities {
        Capabilities {
            freq_ranges: vec![Range {
                min: 24e6,
                max: 1.766e9,
                step: None,
            }],
            sample_rates: vec![2_048_000.0],
            sample_rate_range: None,
            gains: vec![GainStage {
                name: "TUNER".to_string(),
                range: Range {
                    min: 0.0,
                    max: 49.6,
                    step: None,
                },
            }],
            antennas: vec!["RX".to_string()],
            bandwidths: Vec::new(),
            extra: extra_settings("rtlsdr"),
            tx_capable: false,
        }
    }

    #[test]
    fn validate_rejects_center_outside_every_range() {
        let delta = DeviceSettings {
            center_hz: Some(10e9),
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&delta, &caps(), true),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_rejects_sample_rate_outside_list_and_span() {
        let delta = DeviceSettings {
            sample_rate: Some(1_000_000.0),
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&delta, &caps(), true),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_accepts_sample_rate_from_list_or_span() {
        let listed = DeviceSettings {
            sample_rate: Some(2_048_000.0),
            ..DeviceSettings::default()
        };
        assert!(validate(&listed, &caps(), true).is_ok());

        let mut spanned = caps();
        spanned.sample_rate_range = Some(Range {
            min: 1e6,
            max: 10e6,
            step: None,
        });
        let in_span = DeviceSettings {
            sample_rate: Some(5e6),
            ..DeviceSettings::default()
        };
        assert!(validate(&in_span, &spanned, true).is_ok());
        // With both fields present, satisfying either one passes.
        assert!(validate(&listed, &spanned, true).is_ok());
    }

    #[test]
    fn validate_accepts_sample_rate_when_caps_do_not_constrain_it() {
        let mut unconstrained = caps();
        unconstrained.sample_rates = Vec::new();
        unconstrained.sample_rate_range = None;
        let delta = DeviceSettings {
            sample_rate: Some(123.0),
            ..DeviceSettings::default()
        };
        assert!(validate(&delta, &unconstrained, true).is_ok());
    }

    #[test]
    fn validate_checks_bandwidth_only_against_a_non_empty_list() {
        let delta = DeviceSettings {
            bandwidth: Some(1e6),
            ..DeviceSettings::default()
        };
        // caps() has no bandwidth list: continuous filter, anything goes.
        assert!(validate(&delta, &caps(), true).is_ok());

        let mut discrete = caps();
        discrete.bandwidths = vec![290_000.0, 3_570_000.0];
        assert!(matches!(
            validate(&delta, &discrete, true),
            Err(DeviceError::Unsupported(_))
        ));
        let listed = DeviceSettings {
            bandwidth: Some(290_000.0),
            ..DeviceSettings::default()
        };
        assert!(validate(&listed, &discrete, true).is_ok());
    }

    #[test]
    fn validate_rejects_gain_value_outside_stage_range() {
        let over = DeviceSettings {
            gains: vec![GainValue {
                stage: "TUNER".to_string(),
                value_db: 55.0,
            }],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&over, &caps(), true),
            Err(DeviceError::Unsupported(_))
        ));
        let at_max = DeviceSettings {
            gains: vec![GainValue {
                stage: "TUNER".to_string(),
                value_db: 49.6,
            }],
            ..DeviceSettings::default()
        };
        assert!(validate(&at_max, &caps(), true).is_ok());
    }

    #[test]
    fn read_back_confirms_bools_case_insensitively() {
        assert!(read_back_confirms("true", "true"));
        assert!(read_back_confirms("true", "True"));
        assert!(read_back_confirms("false", "FALSE"));
        assert!(!read_back_confirms("true", "false"));
    }

    #[test]
    fn read_back_rejects_empty_echo_and_inexact_enums() {
        assert!(!read_back_confirms("true", ""));
        assert!(!read_back_confirms("2", ""));
        assert!(read_back_confirms("2", "2"));
        assert!(!read_back_confirms("2", "0"));
        assert!(!read_back_confirms("Auto", "auto"));
    }

    #[test]
    fn validate_rejects_unknown_gain_stage() {
        let delta = DeviceSettings {
            gains: vec![GainValue {
                stage: "LNA".to_string(),
                value_db: 10.0,
            }],
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&delta, &caps(), true),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_rejects_ppm_without_corr_component() {
        let delta = DeviceSettings {
            ppm: Some(1.5),
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&delta, &caps(), false),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(validate(&delta, &caps(), true).is_ok());
    }

    #[test]
    fn validate_rejects_unknown_antenna() {
        let delta = DeviceSettings {
            antenna: Some("TX/RX".to_string()),
            ..DeviceSettings::default()
        };
        assert!(matches!(
            validate(&delta, &caps(), true),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_passes_a_full_delta_and_prepares_extra_writes() {
        let delta = DeviceSettings {
            center_hz: Some(100e6),
            sample_rate: Some(2_048_000.0),
            antenna: Some("RX".to_string()),
            gains: vec![GainValue {
                stage: "TUNER".to_string(),
                value_db: 33.8,
            }],
            extra: vec![
                ExtraValue {
                    name: "biastee".to_string(),
                    value: serde_json::json!(true),
                },
                ExtraValue {
                    name: "direct_samp".to_string(),
                    value: serde_json::json!("2"),
                },
            ],
            ..DeviceSettings::default()
        };
        let writes = validate(&delta, &caps(), true).unwrap();
        assert_eq!(
            writes,
            vec![
                ("biastee".to_string(), "true".to_string()),
                ("direct_samp".to_string(), "2".to_string()),
            ]
        );
    }
}
