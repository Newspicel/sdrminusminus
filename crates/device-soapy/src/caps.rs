use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    ArgumentInfo, ArgumentOption, ArgumentType, Capabilities, ChannelCapabilities, DeviceSettings,
    DirectionalCapabilities, Duplex, ExtraSetting, Range,
};
use soapysdr::ArgType;

pub(crate) fn ranges(ranges: &[soapysdr::Range]) -> Vec<Range> {
    ranges
        .iter()
        .map(|range| Range {
            min: range.minimum,
            max: range.maximum,
            step: (range.step > 0.0).then_some(range.step),
        })
        .collect()
}

pub(crate) fn rate_capabilities(ranges: &[soapysdr::Range]) -> (Vec<f64>, Vec<Range>) {
    let mut discrete = Vec::new();
    let mut continuous = Vec::new();
    for range in ranges {
        if range.minimum == range.maximum {
            discrete.push(range.minimum);
        } else {
            continuous.push(Range {
                min: range.minimum,
                max: range.maximum,
                step: (range.step > 0.0).then_some(range.step),
            });
        }
    }
    (discrete, continuous)
}

pub(crate) fn argument_info(info: &soapysdr::ArgInfo) -> ArgumentInfo {
    ArgumentInfo {
        key: info.key.clone(),
        default: info.value.clone(),
        name: info.name.clone(),
        description: info.description.clone(),
        units: info.units.clone(),
        value_type: match info.data_type {
            ArgType::Bool => ArgumentType::Bool,
            ArgType::Float => ArgumentType::Float,
            ArgType::Int => ArgumentType::Int,
            ArgType::String => ArgumentType::String,
            _ => ArgumentType::String,
        },
        range: info.range.map(|range| ranges(&[range])[0]),
        options: info
            .options
            .iter()
            .map(|(value, label)| ArgumentOption {
                value: value.clone(),
                label: label.clone(),
            })
            .collect(),
    }
}

pub(crate) fn argument_infos(infos: &[soapysdr::ArgInfo]) -> Vec<ArgumentInfo> {
    infos.iter().map(argument_info).collect()
}

pub(crate) fn extra_write_value(
    extra: &[ExtraSetting],
    name: &str,
    value: &serde_json::Value,
) -> Result<String, DeviceError> {
    let setting = extra
        .iter()
        .find(|setting| setting.name() == name)
        .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {name}")))?;
    let written = match setting {
        ExtraSetting::Bool { .. } => value.as_bool().map(|value| value.to_string()),
        ExtraSetting::Enum { options, .. } => value
            .as_str()
            .filter(|value| options.iter().any(|option| option.value == *value))
            .map(str::to_string),
        ExtraSetting::Range { range, .. } => value
            .as_f64()
            .filter(|value| range.min <= *value && *value <= range.max)
            .map(|value| value.to_string()),
        ExtraSetting::String { .. } => value.as_str().map(str::to_string),
    };
    written
        .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {name}: bad value {value}")))
}

pub(crate) fn duplex(rx: &[ChannelCapabilities], tx: &[ChannelCapabilities]) -> Duplex {
    match (rx.is_empty(), tx.is_empty()) {
        (false, true) => Duplex::RxOnly,
        (true, false) => Duplex::TxOnly,
        (true, true) => Duplex::RxOnly,
        (false, false) => {
            if rx.iter().chain(tx).all(|channel| channel.full_duplex) {
                Duplex::Full
            } else {
                Duplex::Half
            }
        }
    }
}

pub(crate) fn capabilities(directional: DirectionalCapabilities) -> Capabilities {
    let primary = directional.rx.first();
    let (
        freq_ranges,
        sample_rates,
        sample_rate_ranges,
        gains,
        antennas,
        bandwidths,
        bandwidth_ranges,
        ppm,
    ) = match primary {
        Some(channel) => (
            channel.freq_ranges.clone(),
            channel.sample_rates.clone(),
            channel.sample_rate_ranges.clone(),
            channel.gains.clone(),
            channel.antennas.clone(),
            // A Soapy range whose ends meet is one discrete width; the rest are the continuous
            // envelopes, and both halves have to survive or a radio loses filter settings it has.
            channel
                .bandwidth_ranges
                .iter()
                .filter(|range| range.min == range.max)
                .map(|range| range.min)
                .collect(),
            channel
                .bandwidth_ranges
                .iter()
                .filter(|range| range.min < range.max)
                .copied()
                .collect(),
            channel
                .frequency_components
                .iter()
                .any(|component| component == "CORR"),
        ),
        None => (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            false,
        ),
    };
    let rx_streams = u32::try_from(directional.rx.len()).unwrap_or(u32::MAX);
    let tx_streams = u32::try_from(directional.tx.len()).unwrap_or(u32::MAX);
    let duplex = duplex(&directional.rx, &directional.tx);
    let extra = extra_settings_from_wire(&directional.device_settings);
    Capabilities {
        freq_ranges,
        sample_rates,
        sample_rate_ranges,
        gains,
        antennas,
        bandwidths,
        bandwidth_ranges,
        extra,
        ppm,
        duplex,
        rx_streams,
        tx_streams,
        per_stream: sdrmm_wire::StreamScope::default(),
        directional: Some(directional),
    }
}

fn extra_settings_from_wire(infos: &[ArgumentInfo]) -> Vec<ExtraSetting> {
    infos
        .iter()
        .map(|info| {
            if !info.options.is_empty() {
                return ExtraSetting::Enum {
                    name: info.key.clone(),
                    options: info.options.clone(),
                    default: info.default.clone(),
                };
            }
            match (info.value_type, info.range) {
                (ArgumentType::Bool, _) => ExtraSetting::Bool {
                    name: info.key.clone(),
                    default: matches!(info.default.to_ascii_lowercase().as_str(), "true" | "1"),
                },
                (ArgumentType::Float | ArgumentType::Int, Some(range)) => ExtraSetting::Range {
                    name: info.key.clone(),
                    range,
                    unit: info.units.clone().unwrap_or_default(),
                },
                _ => ExtraSetting::String {
                    name: info.key.clone(),
                    default: info.default.clone(),
                },
            }
        })
        .collect()
}

fn validates_channel(
    settings: &DeviceSettings,
    channel: &ChannelCapabilities,
) -> Result<(), DeviceError> {
    if let Some(frequency) = settings.center_hz
        && !channel.freq_ranges.is_empty()
        && !channel
            .freq_ranges
            .iter()
            .any(|range| range.min <= frequency && frequency <= range.max)
    {
        return Err(DeviceError::Unsupported(format!(
            "center_hz {frequency} outside channel {} range",
            channel.channel
        )));
    }
    if let Some(rate) = settings.sample_rate {
        let constrained =
            !channel.sample_rates.is_empty() || !channel.sample_rate_ranges.is_empty();
        let listed = channel.sample_rates.contains(&rate);
        let ranged = channel
            .sample_rate_ranges
            .iter()
            .any(|range| range.min <= rate && rate <= range.max);
        if constrained && !listed && !ranged {
            return Err(DeviceError::Unsupported(format!(
                "sample_rate {rate} on channel {}",
                channel.channel
            )));
        }
    }
    if let Some(bandwidth) = settings.bandwidth
        && !channel.bandwidth_ranges.is_empty()
        && !channel
            .bandwidth_ranges
            .iter()
            .any(|range| range.min <= bandwidth && bandwidth <= range.max)
    {
        return Err(DeviceError::Unsupported(format!(
            "bandwidth {bandwidth} on channel {}",
            channel.channel
        )));
    }
    for gain in &settings.gains {
        let stage = channel
            .gains
            .iter()
            .find(|stage| stage.name == gain.stage)
            .ok_or_else(|| DeviceError::Unsupported(format!("gain stage {}", gain.stage)))?;
        if !(stage.range.min..=stage.range.max).contains(&gain.value_db) {
            return Err(DeviceError::Unsupported(format!(
                "gain {} {} dB outside {}..{} dB",
                gain.stage, gain.value_db, stage.range.min, stage.range.max
            )));
        }
    }
    if let Some(antenna) = &settings.antenna
        && !channel.antennas.is_empty()
        && !channel.antennas.contains(antenna)
    {
        return Err(DeviceError::Unsupported(format!("antenna {antenna}")));
    }
    Ok(())
}

pub(crate) fn validate(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
) -> Result<Vec<(String, String)>, DeviceError> {
    check_stream_settings(delta, capabilities)?;
    if capabilities
        .directional
        .as_ref()
        .is_some_and(|directional| directional.rx.is_empty())
        && (delta.center_hz.is_some()
            || delta.sample_rate.is_some()
            || delta.ppm.is_some()
            || delta.antenna.is_some()
            || delta.bandwidth.is_some()
            || !delta.gains.is_empty()
            || !delta.streams.is_empty())
    {
        return Err(DeviceError::Unsupported(
            "receive settings on a TX-only device".to_string(),
        ));
    }
    let channels = capabilities
        .directional
        .as_ref()
        .map(|directional| directional.rx.as_slice())
        .unwrap_or(&[]);
    if channels.is_empty() {
        let fallback = ChannelCapabilities {
            channel: 0,
            freq_ranges: capabilities.freq_ranges.clone(),
            sample_rates: capabilities.sample_rates.clone(),
            sample_rate_ranges: capabilities.sample_rate_ranges.clone(),
            gains: capabilities.gains.clone(),
            antennas: capabilities.antennas.clone(),
            bandwidth_ranges: capabilities
                .bandwidths
                .iter()
                .map(|value| Range {
                    min: *value,
                    max: *value,
                    step: None,
                })
                .collect(),
            ..ChannelCapabilities::default()
        };
        validates_channel(delta, &fallback)?;
    } else {
        for channel in channels {
            validates_channel(delta, channel)?;
        }
        for stream in &delta.streams {
            let channel = channels
                .get(stream.stream as usize)
                .ok_or_else(|| DeviceError::Unsupported(format!("streams[{}]", stream.stream)))?;
            validates_channel(
                &delta.for_stream(stream.stream, &capabilities.per_stream),
                channel,
            )?;
        }
    }
    if delta.ppm.is_some() && !capabilities.ppm {
        return Err(DeviceError::Unsupported(
            "ppm: tuner has no CORR frequency component".to_string(),
        ));
    }
    delta
        .extra
        .iter()
        .map(|extra| {
            Ok((
                extra.name.clone(),
                extra_write_value(&capabilities.extra, &extra.name, &extra.value)?,
            ))
        })
        .collect()
}

pub(crate) fn automatic_gain_to_reassert<'a>(
    writes: &'a [(String, String)],
    delta: &DeviceSettings,
) -> Option<&'a str> {
    if !writes_a_gain_stage(delta) {
        return None;
    }
    writes
        .iter()
        .find(|(name, _)| name == crate::GAIN_MODE_SETTING)
        .map(|(_, value)| value.as_str())
        .filter(|value| value.split(',').any(is_automatic))
}

fn is_automatic(value: &str) -> bool {
    value.eq_ignore_ascii_case("true") || value == "1"
}

fn writes_a_gain_stage(delta: &DeviceSettings) -> bool {
    !delta.gains.is_empty() || delta.streams.iter().any(|stream| !stream.gains.is_empty())
}

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
    use super::*;

    fn soapy_arg(key: &str, data_type: ArgType) -> soapysdr::ArgInfo {
        soapysdr::ArgInfo {
            key: key.to_string(),
            value: "false".to_string(),
            name: Some("I/Q swap".to_string()),
            description: Some("Exchange I and Q".to_string()),
            units: None,
            data_type,
            range: None,
            options: Vec::new(),
        }
    }

    fn channel(min: f64, full_duplex: bool) -> ChannelCapabilities {
        ChannelCapabilities {
            channel: 0,
            freq_ranges: vec![Range {
                min,
                max: 1.8e9,
                step: None,
            }],
            sample_rates: vec![2.048e6],
            full_duplex,
            ..ChannelCapabilities::default()
        }
    }

    fn gain(stage: &str, value_db: f64) -> sdrmm_wire::GainValue {
        sdrmm_wire::GainValue {
            stage: stage.to_string(),
            value_db,
        }
    }

    #[test]
    fn automatic_gain_is_reasserted_after_a_gain_stage_in_the_same_delta() {
        let writes = vec![
            ("biastee".to_string(), "false".to_string()),
            (crate::GAIN_MODE_SETTING.to_string(), "true".to_string()),
        ];
        let delta = DeviceSettings {
            gains: vec![gain("TUNER", 43.3)],
            ..DeviceSettings::default()
        };
        assert_eq!(automatic_gain_to_reassert(&writes, &delta), Some("true"));

        let streamed = DeviceSettings {
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 1,
                gains: vec![gain("TUNER", 43.3)],
                ..sdrmm_wire::StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        assert_eq!(automatic_gain_to_reassert(&writes, &streamed), Some("true"));
    }

    #[test]
    fn a_single_control_delta_is_left_alone() {
        let automatic = vec![(crate::GAIN_MODE_SETTING.to_string(), "true".to_string())];
        let manual = vec![(crate::GAIN_MODE_SETTING.to_string(), "false".to_string())];
        let bare_gain = DeviceSettings {
            gains: vec![gain("TUNER", 43.3)],
            ..DeviceSettings::default()
        };

        assert_eq!(
            automatic_gain_to_reassert(&automatic, &DeviceSettings::default()),
            None
        );
        assert_eq!(automatic_gain_to_reassert(&manual, &bare_gain), None);
        assert_eq!(automatic_gain_to_reassert(&[], &bare_gain), None);
    }

    #[test]
    fn arg_info_conversion_preserves_all_metadata() {
        let mut info = soapy_arg("direct_samp", ArgType::Int);
        info.value = "0".to_string();
        info.units = Some("mode".to_string());
        info.range = Some(soapysdr::Range {
            minimum: 0.0,
            maximum: 2.0,
            step: 1.0,
        });
        info.options = vec![("0".to_string(), Some("Off".to_string()))];
        let mapped = argument_info(&info);
        assert_eq!(mapped.key, "direct_samp");
        assert_eq!(mapped.name.as_deref(), Some("I/Q swap"));
        assert_eq!(mapped.description.as_deref(), Some("Exchange I and Q"));
        assert_eq!(mapped.units.as_deref(), Some("mode"));
        assert_eq!(mapped.value_type, ArgumentType::Int);
        assert_eq!(mapped.range.expect("range").step, Some(1.0));
        assert_eq!(mapped.options[0].label.as_deref(), Some("Off"));
    }

    #[test]
    fn disjoint_sample_rate_ranges_keep_their_gap() {
        let (_, ranges) = rate_capabilities(&[
            soapysdr::Range {
                minimum: 225_001.0,
                maximum: 300_000.0,
                step: 1.0,
            },
            soapysdr::Range {
                minimum: 900_001.0,
                maximum: 3_200_000.0,
                step: 1.0,
            },
        ]);
        assert_eq!(ranges.len(), 2);
        assert!(
            !ranges
                .iter()
                .any(|range| range.min <= 500_000.0 && 500_000.0 <= range.max)
        );
    }

    #[test]
    fn every_window_and_filter_a_channel_reports_reaches_the_flat_capabilities() {
        let windows = vec![
            Range {
                min: 225_001.0,
                max: 300_000.0,
                step: None,
            },
            Range {
                min: 900_001.0,
                max: 3_200_000.0,
                step: None,
            },
        ];
        let rx = ChannelCapabilities {
            sample_rate_ranges: windows.clone(),
            bandwidth_ranges: vec![
                Range {
                    min: 1.75e6,
                    max: 1.75e6,
                    step: None,
                },
                Range {
                    min: 2e6,
                    max: 28e6,
                    step: None,
                },
            ],
            ..channel(24e6, false)
        };
        let caps = capabilities(DirectionalCapabilities {
            rx: vec![rx],
            ..DirectionalCapabilities::default()
        });
        assert_eq!(
            caps.sample_rate_ranges, windows,
            "a radio with two windows must not lose both of them on the way out"
        );
        assert_eq!(
            caps.bandwidths,
            vec![1.75e6],
            "a range whose ends meet is one discrete filter width"
        );
        assert_eq!(
            caps.bandwidth_ranges,
            vec![Range {
                min: 2e6,
                max: 28e6,
                step: None
            }],
            "and a range with room in it is a continuous filter"
        );
    }

    #[test]
    fn iq_swap_is_an_independent_boolean_control() {
        let extras =
            extra_settings_from_wire(&argument_infos(&[soapy_arg("iq_swap", ArgType::Bool)]));
        assert!(matches!(
            &extras[0],
            ExtraSetting::Bool { name, default } if name == "iq_swap" && !default
        ));
        assert_eq!(
            extra_write_value(&extras, "iq_swap", &serde_json::json!(true)).unwrap(),
            "true"
        );
    }

    #[test]
    fn an_enum_setting_keeps_the_words_its_driver_gives_each_value() {
        let mut info = soapy_arg("direct_samp", ArgType::Int);
        info.value = "0".to_string();
        info.options = vec![
            ("0".to_string(), Some("Off".to_string())),
            ("1".to_string(), Some("I-ADC".to_string())),
            ("2".to_string(), None),
        ];
        let extras = extra_settings_from_wire(&argument_infos(&[info]));
        let ExtraSetting::Enum { options, .. } = &extras[0] else {
            panic!("an argument with options is an enum control");
        };
        assert_eq!(options[0].label.as_deref(), Some("Off"));
        assert_eq!(options[1].label.as_deref(), Some("I-ADC"));
        assert_eq!(options[2].label, None);
        assert_eq!(
            extra_write_value(&extras, "direct_samp", &serde_json::json!("1")).unwrap(),
            "1"
        );
        assert!(extra_write_value(&extras, "direct_samp", &serde_json::json!("I-ADC")).is_err());
    }

    #[test]
    fn duplex_and_channel_counts_follow_both_directions() {
        let rx = vec![
            channel(24e6, true),
            ChannelCapabilities {
                channel: 1,
                ..channel(24e6, true)
            },
        ];
        let tx = vec![channel(1e6, true)];
        let caps = capabilities(DirectionalCapabilities {
            rx,
            tx,
            ..DirectionalCapabilities::default()
        });
        assert_eq!(caps.rx_streams, 2);
        assert_eq!(caps.tx_streams, 1);
        assert_eq!(caps.duplex, Duplex::Full);
        assert_eq!(caps.per_stream, sdrmm_wire::StreamScope::default());
    }

    #[test]
    fn any_half_duplex_channel_makes_the_device_half_duplex() {
        assert_eq!(
            duplex(&[channel(24e6, false)], &[channel(1e6, false)]),
            Duplex::Half
        );
    }

    #[test]
    fn tx_only_devices_do_not_accept_receiver_settings() {
        let caps = capabilities(DirectionalCapabilities {
            tx: vec![channel(1e6, false)],
            ..DirectionalCapabilities::default()
        });
        let delta = DeviceSettings {
            center_hz: Some(100e6),
            ..DeviceSettings::default()
        };
        assert!(
            matches!(validate(&delta, &caps), Err(DeviceError::Unsupported(message)) if message.contains("TX-only"))
        );
        assert!(caps.freq_ranges.is_empty());
        assert!(caps.sample_rates.is_empty());
        assert!(caps.gains.is_empty());
        assert!(caps.antennas.is_empty());
    }

    #[test]
    fn direct_sampling_refresh_allows_the_hf_range() {
        let before = capabilities(DirectionalCapabilities {
            rx: vec![channel(24e6, false)],
            ..DirectionalCapabilities::default()
        });
        let after = capabilities(DirectionalCapabilities {
            rx: vec![channel(0.0, false)],
            ..DirectionalCapabilities::default()
        });
        let hf = DeviceSettings {
            center_hz: Some(7.1e6),
            ..DeviceSettings::default()
        };
        assert!(validate(&hf, &before).is_err());
        assert!(validate(&hf, &after).is_ok());
    }

    #[test]
    fn readback_rejects_ignored_settings() {
        assert!(!read_back_confirms("true", ""));
        assert!(read_back_confirms("true", "True"));
        assert!(!read_back_confirms("2", "0"));
    }
}
