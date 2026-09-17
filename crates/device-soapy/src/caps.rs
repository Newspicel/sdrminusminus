use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Agc, ArgumentInfo, ArgumentOption, ArgumentType, BandwidthSetting, Capabilities,
    ChannelCapabilities, Coherence, DcArtifact, DeviceSettings, DirectionalCapabilities, Duplex,
    ExtraSetting, GainKind, GainStage, Range,
};

use crate::soapy::ArgType;

const BIAS_TEE_KEYS: [&str; 4] = ["biastee", "bias_tee", "bias_tx", "biasT_ctrl"];

pub(crate) fn bias_tee_key(capabilities: &Capabilities) -> Option<&str> {
    capabilities
        .directional
        .as_ref()?
        .device_settings
        .iter()
        .map(|info| info.key.as_str())
        .find(|key| is_bias_tee_key(key))
}

fn is_bias_tee_key(key: &str) -> bool {
    BIAS_TEE_KEYS.contains(&key)
}

pub(crate) fn gain_stage(name: &str, range: Range) -> GainStage {
    let stage = GainStage::named(name, GainKind::from_name(name), range);
    if stage.is_switch() && stage.setting_count() != 2 {
        return GainStage::named(name, GainKind::Other, range);
    }
    stage
}

pub(crate) fn ranges(ranges: &[crate::soapy::Range]) -> Vec<Range> {
    ranges
        .iter()
        .map(|range| Range {
            min: range.minimum,
            max: range.maximum,
            step: (range.step > 0.0).then_some(range.step),
        })
        .collect()
}

pub(crate) fn rate_capabilities(ranges: &[crate::soapy::Range]) -> (Vec<f64>, Vec<Range>) {
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

pub(crate) fn argument_info(info: &crate::soapy::ArgInfo) -> ArgumentInfo {
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

pub(crate) fn argument_infos(infos: &[crate::soapy::ArgInfo]) -> Vec<ArgumentInfo> {
    infos
        .iter()
        .filter(|info| !info.key.is_empty())
        .map(argument_info)
        .collect()
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
    let coherence = coherence(&directional, rx_streams);
    let bias_tee = directional
        .device_settings
        .iter()
        .any(|info| is_bias_tee_key(&info.key));
    let agc = if primary.is_some_and(|channel| channel.gain_mode) {
        Agc::Switch
    } else {
        Agc::None
    };
    Capabilities {
        freq_ranges,
        sample_rates,
        sample_rate_ranges,
        gains,
        antennas,
        bandwidths,
        bandwidth_ranges,
        bandwidth_auto: false,
        bias_tee,
        agc,
        extra,
        ppm,
        duplex,
        rx_streams,
        tx_streams,
        per_stream: sdrmm_wire::StreamScope::default(),
        directional: Some(directional),
        dc_artifact: DcArtifact::Operator,
        hardware_sweep: false,
        coherence,
        noise_source: false,
    }
}

/// Hardware whose receive chains are known to share one synthesizer, not just one clock. Anything
/// else with more than one channel on a single device shares a clock by construction, which makes
/// a delay between its lanes meaningful and a phase between them not.
///
/// A bank of dongles on one clock does not belong here however coherent its marketing is: each
/// chain keeps its own synthesizer, and the phase between them is whatever the last retune left,
/// which is why those radios carry a noise source to solve it again.
const PHASE_COHERENT_HARDWARE: [&str; 1] = ["rspduo"];

fn coherence(directional: &DirectionalCapabilities, rx_streams: u32) -> Coherence {
    if rx_streams < 2 {
        return Coherence::None;
    }
    let named = directional
        .hardware_info
        .values()
        .chain(
            directional
                .rx
                .iter()
                .flat_map(|channel| channel.info.values()),
        )
        .any(|value| {
            let value = value.to_ascii_lowercase().replace([' ', '-', '_'], "");
            PHASE_COHERENT_HARDWARE
                .iter()
                .any(|known| value.contains(known))
        });
    if named {
        Coherence::PhaseCoherent
    } else {
        Coherence::TimeSync
    }
}

fn extra_settings_from_wire(infos: &[ArgumentInfo]) -> Vec<ExtraSetting> {
    infos
        .iter()
        .filter(|info| !is_bias_tee_key(&info.key))
        .map(extra_setting)
        .collect()
}

fn extra_setting(info: &ArgumentInfo) -> ExtraSetting {
    let name = info.key.clone();
    let label = info
        .name
        .clone()
        .filter(|label| !label.is_empty() && *label != info.key);
    if !info.options.is_empty() {
        return ExtraSetting::Enum {
            name,
            label,
            options: info.options.clone(),
            default: info.default.clone(),
        };
    }
    match (info.value_type, info.range) {
        (ArgumentType::Bool, _) => ExtraSetting::Bool {
            name,
            label,
            default: is_true(&info.default),
        },
        (ArgumentType::Float | ArgumentType::Int, Some(range)) => ExtraSetting::Range {
            name,
            label,
            range,
            unit: info.units.clone().unwrap_or_default(),
        },
        _ => ExtraSetting::String {
            name,
            label,
            default: info.default.clone(),
        },
    }
}

pub(crate) fn is_true(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "true" | "1")
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
    if let Some(bandwidth) = settings.bandwidth.and_then(BandwidthSetting::hz)
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
) -> Result<(), DeviceError> {
    check_stream_settings(delta, capabilities)?;
    if delta.bandwidth.is_some_and(BandwidthSetting::is_auto) {
        return Err(DeviceError::Unsupported(
            "bandwidth: this radio does not pick its own filter width".to_string(),
        ));
    }
    if let Some(agc) = &delta.agc
        && !capabilities.agc.admits(agc)
    {
        return Err(DeviceError::Unsupported(match &agc.mode {
            Some(mode) => format!("agc: this radio has no {mode} mode"),
            None => "agc: this radio has no automatic gain".to_string(),
        }));
    }
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
    Ok(())
}

pub(crate) fn setting_writes(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
) -> Result<Vec<(String, String)>, DeviceError> {
    let mut writes = Vec::new();
    if let Some(on) = delta.bias_tee {
        let key = bias_tee_key(capabilities)
            .ok_or_else(|| DeviceError::Unsupported("bias_tee: this radio has none".to_string()))?;
        writes.push((key.to_string(), on.to_string()));
    }
    for extra in &delta.extra {
        writes.push((
            extra.name.clone(),
            extra_write_value(&capabilities.extra, &extra.name, &extra.value)?,
        ));
    }
    Ok(writes)
}

pub(crate) fn automatic_gain_to_reassert(delta: &DeviceSettings) -> bool {
    writes_a_gain_stage(delta) && delta.agc.as_ref().is_some_and(|agc| agc.on)
}

fn writes_a_gain_stage(delta: &DeviceSettings) -> bool {
    !delta.gains.is_empty() || delta.streams.iter().any(|stream| !stream.gains.is_empty())
}

/// Whether a gain the operator asked for would land on a radio that is already choosing its own.
/// A patch that sets the mode itself is left alone: that is the caller saying which of the two
/// wins, and [`automatic_gain_to_reassert`] puts the mode back afterwards.
pub(crate) fn gain_needs_manual_mode(delta: &DeviceSettings, automatic: bool) -> bool {
    automatic && writes_a_gain_stage(delta) && delta.agc.is_none()
}

/// The settings a radio can only take with its stream torn down. A bladeRF resets the sample
/// counter the sync worker is reading against inside its own `setSampleRate`, and reshaping the
/// analog filter walks the same converter; both leave the capture thread reading into nothing.
pub(crate) const fn reshapes_the_stream(delta: &DeviceSettings) -> bool {
    delta.sample_rate.is_some() || delta.bandwidth.is_some()
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
    use sdrmm_wire::AgcSetting;

    use super::*;

    fn soapy_arg(key: &str, data_type: ArgType) -> crate::soapy::ArgInfo {
        crate::soapy::ArgInfo {
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

    fn named(hardware: &str, channels: usize) -> DirectionalCapabilities {
        DirectionalCapabilities {
            rx: (0..channels).map(|_| channel(24e6, false)).collect(),
            hardware_info: [("hardware".to_string(), hardware.to_string())]
                .into_iter()
                .collect(),
            ..DirectionalCapabilities::default()
        }
    }

    #[test]
    fn only_one_synthesizer_behind_every_lane_is_phase_coherent() {
        assert_eq!(coherence(&named("RSPduo", 2), 2), Coherence::PhaseCoherent);
        assert_eq!(coherence(&named("RSPduo", 1), 1), Coherence::None);
        assert_eq!(coherence(&named("LimeSDR", 2), 2), Coherence::TimeSync);
        assert_eq!(
            coherence(&named("KrakenSDR", 5), 5),
            Coherence::TimeSync,
            "five tuners on one clock come up at five phases"
        );
    }

    #[test]
    fn automatic_gain_is_reasserted_after_a_gain_stage_in_the_same_delta() {
        let delta = DeviceSettings {
            agc: Some(AgcSetting::switched(true)),
            gains: vec![gain("TUNER", 43.3)],
            ..DeviceSettings::default()
        };
        assert!(automatic_gain_to_reassert(&delta));

        let streamed = DeviceSettings {
            agc: Some(AgcSetting::switched(true)),
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 1,
                gains: vec![gain("TUNER", 43.3)],
                ..sdrmm_wire::StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        assert!(automatic_gain_to_reassert(&streamed));
    }

    #[test]
    fn a_single_control_delta_is_left_alone() {
        assert!(!automatic_gain_to_reassert(&DeviceSettings {
            agc: Some(AgcSetting::switched(true)),
            ..DeviceSettings::default()
        }));
        assert!(!automatic_gain_to_reassert(&DeviceSettings {
            agc: Some(AgcSetting::off()),
            gains: vec![gain("TUNER", 43.3)],
            ..DeviceSettings::default()
        }));
        assert!(!automatic_gain_to_reassert(&DeviceSettings {
            gains: vec![gain("TUNER", 43.3)],
            ..DeviceSettings::default()
        }));
    }

    #[test]
    fn a_gain_asked_for_under_agc_is_refused_rather_than_swallowed() {
        let bare_gain = DeviceSettings {
            gains: vec![gain("full", 30.0)],
            ..DeviceSettings::default()
        };
        assert!(gain_needs_manual_mode(&bare_gain, true));
        assert!(!gain_needs_manual_mode(&bare_gain, false));
        assert!(
            !gain_needs_manual_mode(&DeviceSettings::default(), true),
            "a delta with no gain in it has nothing to refuse"
        );
    }

    #[test]
    fn a_delta_that_sets_the_mode_itself_decides_which_of_the_two_wins() {
        let streamed = DeviceSettings {
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 1,
                gains: vec![gain("full", 30.0)],
                ..sdrmm_wire::StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        let decided = DeviceSettings {
            agc: Some(AgcSetting::off()),
            ..streamed.clone()
        };
        assert!(gain_needs_manual_mode(&streamed, true));
        assert!(!gain_needs_manual_mode(&decided, true));
    }

    #[test]
    fn a_soapy_gain_element_keeps_its_name_and_takes_the_kind_it_spells() {
        let wide = Range {
            min: 0.0,
            max: 49.6,
            step: Some(0.1),
        };
        let switch = Range {
            min: 0.0,
            max: 14.0,
            step: Some(14.0),
        };
        let lna = gain_stage("LNA", wide);
        assert_eq!(lna.name, "LNA");
        assert_eq!(lna.kind, GainKind::Lna);
        assert_eq!(gain_stage("TIA", wide).kind, GainKind::Mixer);
        assert_eq!(gain_stage("PGA", wide).kind, GainKind::Vga);
        assert_eq!(gain_stage("IFGR", wide).kind, GainKind::Vga);
        assert_eq!(gain_stage("AMP", switch).kind, GainKind::Amp);
        assert_eq!(
            gain_stage("AMP", wide).kind,
            GainKind::Other,
            "a preamp with more than two settings is not a switch"
        );
        assert_eq!(gain_stage("PAD", wide).kind, GainKind::Tx);
        assert_eq!(gain_stage("MYSTERY", wide).kind, GainKind::Other);
        assert_eq!(gain_stage("MYSTERY", wide).name, "MYSTERY");
    }

    #[test]
    fn a_vendor_bias_tee_argument_is_the_typed_bias_tee_and_not_an_extra() {
        for key in ["biastee", "bias_tee", "bias_tx", "biasT_ctrl"] {
            let caps = capabilities(DirectionalCapabilities {
                rx: vec![channel(24e6, false)],
                device_settings: argument_infos(&[
                    soapy_arg(key, ArgType::Bool),
                    soapy_arg("iq_swap", ArgType::Bool),
                ]),
                ..DirectionalCapabilities::default()
            });
            assert!(caps.bias_tee, "{key}");
            assert_eq!(bias_tee_key(&caps), Some(key));
            assert_eq!(
                caps.extra
                    .iter()
                    .map(ExtraSetting::name)
                    .collect::<Vec<_>>(),
                ["iq_swap"],
                "{key}"
            );
            let writes = setting_writes(
                &DeviceSettings {
                    bias_tee: Some(true),
                    extra: vec![sdrmm_wire::ExtraValue {
                        name: "iq_swap".to_string(),
                        value: serde_json::json!(false),
                    }],
                    ..DeviceSettings::default()
                },
                &caps,
            )
            .unwrap();
            assert_eq!(
                writes,
                vec![
                    (key.to_string(), "true".to_string()),
                    ("iq_swap".to_string(), "false".to_string())
                ]
            );
        }
    }

    #[test]
    fn a_radio_without_a_bias_tee_refuses_one() {
        let caps = capabilities(DirectionalCapabilities {
            rx: vec![channel(24e6, false)],
            ..DirectionalCapabilities::default()
        });
        assert!(!caps.bias_tee);
        assert_eq!(bias_tee_key(&caps), None);
        assert!(
            setting_writes(
                &DeviceSettings {
                    bias_tee: Some(true),
                    ..DeviceSettings::default()
                },
                &caps,
            )
            .is_err()
        );
    }

    #[test]
    fn a_channel_with_a_gain_mode_offers_a_switched_agc() {
        let with = capabilities(DirectionalCapabilities {
            rx: vec![ChannelCapabilities {
                gain_mode: true,
                ..channel(24e6, false)
            }],
            ..DirectionalCapabilities::default()
        });
        assert_eq!(with.agc, Agc::Switch);
        assert!(
            validate(
                &DeviceSettings {
                    agc: Some(AgcSetting::switched(true)),
                    ..DeviceSettings::default()
                },
                &with
            )
            .is_ok()
        );
        assert!(
            validate(
                &DeviceSettings {
                    agc: Some(AgcSetting::in_mode(true, "slow")),
                    ..DeviceSettings::default()
                },
                &with
            )
            .is_err()
        );

        let without = capabilities(DirectionalCapabilities {
            rx: vec![channel(24e6, false)],
            ..DirectionalCapabilities::default()
        });
        assert_eq!(without.agc, Agc::None);
        assert!(
            validate(
                &DeviceSettings {
                    agc: Some(AgcSetting::switched(true)),
                    ..DeviceSettings::default()
                },
                &without
            )
            .is_err()
        );
    }

    #[test]
    fn an_automatic_filter_width_is_refused_and_a_manual_one_is_checked() {
        let caps = capabilities(DirectionalCapabilities {
            rx: vec![ChannelCapabilities {
                bandwidth_ranges: vec![Range {
                    min: 2e6,
                    max: 28e6,
                    step: None,
                }],
                ..channel(24e6, false)
            }],
            ..DirectionalCapabilities::default()
        });
        assert!(!caps.bandwidth_auto);
        let auto = DeviceSettings {
            bandwidth: Some(BandwidthSetting::Auto),
            ..DeviceSettings::default()
        };
        assert!(
            matches!(validate(&auto, &caps), Err(DeviceError::Unsupported(message)) if message.contains("bandwidth"))
        );
        let manual = |hz| DeviceSettings {
            bandwidth: Some(BandwidthSetting::Manual { hz }),
            ..DeviceSettings::default()
        };
        assert!(validate(&manual(5e6), &caps).is_ok());
        assert!(validate(&manual(50e6), &caps).is_err());
    }

    #[test]
    fn only_rate_and_filter_need_the_stream_torn_down() {
        assert!(reshapes_the_stream(&DeviceSettings {
            sample_rate: Some(8e6),
            ..DeviceSettings::default()
        }));
        assert!(reshapes_the_stream(&DeviceSettings {
            bandwidth: Some(BandwidthSetting::Manual { hz: 5e6 }),
            ..DeviceSettings::default()
        }));
        assert!(
            !reshapes_the_stream(&DeviceSettings {
                center_hz: Some(100e6),
                gains: vec![gain("full", 30.0)],
                ..DeviceSettings::default()
            }),
            "retuning and gain stay live or scanning would stutter on every step"
        );
    }

    #[test]
    fn arg_info_conversion_preserves_all_metadata() {
        let mut info = soapy_arg("direct_samp", ArgType::Int);
        info.value = "0".to_string();
        info.units = Some("mode".to_string());
        info.range = Some(crate::soapy::Range {
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
    fn keyless_arguments_are_dropped() {
        let infos = vec![
            soapy_arg("buffers", ArgType::Int),
            soapy_arg("", ArgType::String),
            soapy_arg("transfers", ArgType::Int),
        ];
        let mapped = argument_infos(&infos);
        assert_eq!(
            mapped
                .iter()
                .map(|arg| arg.key.as_str())
                .collect::<Vec<_>>(),
            ["buffers", "transfers"]
        );
    }

    #[test]
    fn disjoint_sample_rate_ranges_keep_their_gap() {
        let (_, ranges) = rate_capabilities(&[
            crate::soapy::Range {
                minimum: 225_001.0,
                maximum: 300_000.0,
                step: 1.0,
            },
            crate::soapy::Range {
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
            ExtraSetting::Bool { name, label, default }
                if name == "iq_swap" && label.as_deref() == Some("I/Q swap") && !default
        ));
        assert_eq!(
            extra_write_value(&extras, "iq_swap", &serde_json::json!(true)).unwrap(),
            "true"
        );
    }

    #[test]
    fn an_extra_is_labelled_only_by_a_name_that_says_more_than_its_key() {
        let mut same = soapy_arg("buffers", ArgType::Int);
        same.name = Some("buffers".to_string());
        let mut empty = soapy_arg("transfers", ArgType::Int);
        empty.name = Some(String::new());
        let mut none = soapy_arg("timeout", ArgType::Int);
        none.name = None;
        let extras = extra_settings_from_wire(&argument_infos(&[
            soapy_arg("iq_swap", ArgType::Bool),
            same,
            empty,
            none,
        ]));
        let labels: Vec<Option<&str>> = extras.iter().map(ExtraSetting::label).collect();
        assert_eq!(labels, vec![Some("I/Q swap"), None, None, None]);
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
