use sdrmm_channels::ChannelError;
use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Capabilities, ChannelDescriptor, ChannelInfo, ChannelParams, ChannelSettings, DcArtifact,
    DeviceSettings, StreamSettings,
};

use crate::{DEFAULT_CENTER_HZ, EngineError, center_of, sample_rate_of};

pub(crate) fn descriptor_for(
    params: &ChannelParams,
) -> Result<&'static ChannelDescriptor, EngineError> {
    let type_id = params.type_id();
    sdrmm_channels::descriptor(type_id)
        .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()).into())
}

pub(crate) fn validate_channel(
    descriptor: &ChannelDescriptor,
    settings: &ChannelSettings,
) -> Result<(), EngineError> {
    if !settings.frequency_hz.is_finite() {
        return Err(ChannelError::InvalidSettings(format!(
            "frequency_hz must be a finite frequency, got {}",
            settings.frequency_hz
        ))
        .into());
    }
    settings
        .check_limits()
        .map_err(|reason| EngineError::from(ChannelError::InvalidSettings(reason)))?;
    if let Err(reason) = settings.audio.validate() {
        return Err(ChannelError::InvalidSettings(reason).into());
    }
    if settings.audio.is_active() && !descriptor.has_audio {
        return Err(ChannelError::InvalidSettings(format!(
            "{} produces no audio, so it has nothing for the audio chain to process",
            descriptor.type_id
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn hears(
    capabilities: &Capabilities,
    tuning: &DeviceSettings,
    stream: u32,
    settings: &ChannelSettings,
) -> bool {
    let (low, high) = sdrmm_channels::occupied_band(&settings.params);
    let center = center_of(tuning, stream, &capabilities.per_stream);
    crate::runtime::reaches(
        settings.frequency_hz - center,
        low,
        high,
        sample_rate_of(tuning),
    )
}

pub(crate) fn tuner_reaches(capabilities: &Capabilities, hz: f64) -> bool {
    capabilities.freq_ranges.is_empty()
        || capabilities
            .freq_ranges
            .iter()
            .any(|r| hz >= r.min && hz <= r.max)
}

/// Room left between the DC term at the centre and the edge of a channel it must not sit in.
const LO_ARTIFACT_MARGIN_HZ: f64 = 2_000.0;

fn channel_half_width_hz(params: &ChannelParams) -> f64 {
    descriptor_for(params).map_or(0.0, |d| d.bandwidth_hz / 2.0) + LO_ARTIFACT_MARGIN_HZ
}

/// Whether the centre, where a zero-IF front end lands its DC term, sits inside no channel.
pub(crate) fn centre_clears_channels(
    settings: &DeviceSettings,
    capabilities: &Capabilities,
    channels: &[ChannelInfo],
) -> bool {
    channels.iter().all(|channel| {
        let center_hz = settings
            .for_stream(channel.stream, &capabilities.per_stream)
            .center_hz
            .unwrap_or(DEFAULT_CENTER_HZ);
        (center_hz - channel.settings.frequency_hz).abs()
            > channel_half_width_hz(&channel.settings.params)
    })
}

/// Whether the front end removes its DC term: the operator's choice, starting on for hardware
/// known to land one and never for a source without a front end.
pub(crate) fn dc_block(capabilities: &Capabilities, settings: &DeviceSettings) -> bool {
    match capabilities.dc_artifact {
        DcArtifact::None => false,
        DcArtifact::Managed => settings.dc_block.unwrap_or(true),
        DcArtifact::Operator => settings.dc_block.unwrap_or(false),
    }
}

pub(crate) fn validate_streams(
    capabilities: &Capabilities,
    delta: &DeviceSettings,
) -> Result<(), EngineError> {
    check_stream_settings(delta, capabilities)?;
    for entry in &delta.streams {
        if let Some(hz) = entry.center_hz
            && !tuner_reaches(capabilities, hz)
        {
            return Err(DeviceError::Unsupported(format!(
                "streams[{}].center_hz: {hz} Hz is outside this device's tuning range",
                entry.stream
            ))
            .into());
        }
    }
    Ok(())
}

const SPAN_EPSILON_HZ: f64 = 1e-6;

type Span = (f64, f64);

fn frequency_key(hz: f64) -> u64 {
    let bits = hz.to_bits();
    if hz.is_sign_negative() {
        !bits
    } else {
        bits ^ (1 << 63)
    }
}

fn key_frequency(key: u64) -> f64 {
    f64::from_bits(if key & (1 << 63) == 0 {
        !key
    } else {
        key ^ (1 << 63)
    })
}

fn first_frequency(mut holds: impl FnMut(f64) -> bool) -> f64 {
    let mut low = frequency_key(-f64::MAX);
    let mut high = frequency_key(f64::MAX);
    while low < high {
        let middle = low + (high - low) / 2;
        if holds(key_frequency(middle)) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    key_frequency(low)
}

pub(crate) fn tuning_span(frequency: f64, low: f64, high: f64, rate: f64) -> Option<Span> {
    if !frequency.is_finite()
        || !low.is_finite()
        || !high.is_finite()
        || !rate.is_finite()
        || rate <= 0.0
    {
        return None;
    }
    let lowest = first_frequency(|center| (frequency - center) + high <= rate / 2.0);
    let highest =
        -first_frequency(|negative_center| (frequency + negative_center) + low >= -rate / 2.0);
    (lowest <= highest).then_some((lowest, highest))
}

fn feasible_span(channel: &ChannelInfo, rate: f64) -> Option<Span> {
    let (low, high) = sdrmm_channels::occupied_band(&channel.settings.params);
    tuning_span(channel.settings.frequency_hz, low, high, rate)
}

fn holds(span: Span, center_hz: f64) -> bool {
    span.0 <= center_hz && center_hz <= span.1
}

fn covered_count(spans: &[Span], center_hz: f64) -> usize {
    spans.iter().filter(|span| holds(**span, center_hz)).count()
}

fn narrowest_margin_hz(spans: &[Span], center_hz: f64) -> f64 {
    spans
        .iter()
        .filter(|span| holds(**span, center_hz))
        .map(|(low, high)| (center_hz - low).min(high - center_hz))
        .fold(f64::INFINITY, f64::min)
}

fn with_center(
    settings: &DeviceSettings,
    capabilities: &Capabilities,
    stream: u32,
    center_hz: f64,
) -> DeviceSettings {
    let mut probe = settings.clone();
    if capabilities.per_stream.tuning {
        match probe.streams.iter_mut().find(|s| s.stream == stream) {
            Some(existing) => existing.center_hz = Some(center_hz),
            None => probe.streams.push(StreamSettings {
                stream,
                center_hz: Some(center_hz),
                tuning: None,
                gains: Vec::new(),
                antenna: None,
            }),
        }
    } else {
        probe.center_hz = Some(center_hz);
    }
    probe
}

fn artifact_is_clear(
    capabilities: &Capabilities,
    settings: &DeviceSettings,
    stream: u32,
    center_hz: f64,
    channels: &[ChannelInfo],
) -> bool {
    let probe = with_center(settings, capabilities, stream, center_hz);
    centre_clears_channels(&probe, capabilities, channels)
}

fn candidate_centers(spans: &[Span], channels: &[ChannelInfo], current_hz: f64) -> Vec<f64> {
    let mut candidates = vec![current_hz];
    for anchor in spans {
        let held = spans.iter().filter(|span| holds(**span, anchor.0));
        let (low, high) = held.fold((f64::NEG_INFINITY, f64::INFINITY), |(low, high), span| {
            (low.max(span.0), high.min(span.1))
        });
        if !(low.is_finite() && high.is_finite() && low <= high) {
            continue;
        }
        candidates.push(f64::midpoint(low, high));
        for (channel, span) in channels.iter().zip(spans) {
            let clear_of = channel_half_width_hz(&channel.settings.params) + 1.0;
            let frequency_hz = channel.settings.frequency_hz;
            candidates.extend(
                [
                    frequency_hz - clear_of,
                    frequency_hz + clear_of,
                    f64::midpoint(span.0, frequency_hz),
                    f64::midpoint(frequency_hz, span.1),
                ]
                .into_iter()
                .filter(|hz| (low..=high).contains(hz)),
            );
        }
    }
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|hz| seen.insert(hz.to_bits()));
    candidates
}

fn nearest_carrier_hz(channels: &[ChannelInfo], center_hz: f64) -> f64 {
    channels
        .iter()
        .map(|channel| (channel.settings.frequency_hz - center_hz).abs())
        .fold(f64::INFINITY, f64::min)
}

fn best_center_hz(
    capabilities: &Capabilities,
    settings: &DeviceSettings,
    stream: u32,
    channels: &[ChannelInfo],
    current_hz: f64,
) -> Option<f64> {
    let rate = sample_rate_of(settings);
    let spans: Vec<Span> = channels
        .iter()
        .filter_map(|channel| feasible_span(channel, rate))
        .collect();
    if spans.is_empty() {
        return None;
    }
    let rank = |center_hz: f64| {
        let clear = artifact_is_clear(capabilities, settings, stream, center_hz, channels);
        let stays = (center_hz - current_hz).abs() <= SPAN_EPSILON_HZ;
        let margin = narrowest_margin_hz(&spans, center_hz);
        let room = if clear {
            0.0
        } else {
            nearest_carrier_hz(channels, center_hz).min(margin)
        };
        (
            covered_count(&spans, center_hz),
            clear,
            clear && stays,
            room,
            margin,
        )
    };
    candidate_centers(&spans, channels, current_hz)
        .into_iter()
        .chain(
            capabilities
                .freq_ranges
                .iter()
                .flat_map(|range| [range.min, range.max]),
        )
        .filter(|hz| hz.is_finite() && tuner_reaches(capabilities, *hz))
        .max_by(|a, b| {
            rank(*a)
                .partial_cmp(&rank(*b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

pub(crate) fn plan_center(
    capabilities: &Capabilities,
    settings: &DeviceSettings,
    channels: &[ChannelInfo],
) -> Option<DeviceSettings> {
    let scope = capabilities.per_stream;
    if !scope.tuning {
        if !settings.tunes_itself() {
            return None;
        }
        let current_hz = center_of(settings, 0, &scope);
        let center_hz = best_center_hz(capabilities, settings, 0, channels, current_hz)?;
        return (center_hz != current_hz).then(|| DeviceSettings {
            center_hz: Some(center_hz),
            ..DeviceSettings::default()
        });
    }
    let mut streams = Vec::new();
    for stream in 0..capabilities.rx_streams {
        if !settings.for_stream(stream, &scope).tunes_itself() {
            continue;
        }
        let mine: Vec<ChannelInfo> = channels
            .iter()
            .filter(|channel| channel.stream == stream)
            .cloned()
            .collect();
        let current_hz = center_of(settings, stream, &scope);
        let Some(center_hz) = best_center_hz(capabilities, settings, stream, &mine, current_hz)
        else {
            continue;
        };
        if center_hz != current_hz {
            streams.push(StreamSettings {
                stream,
                center_hz: Some(center_hz),
                tuning: None,
                gains: Vec::new(),
                antenna: None,
            });
        }
    }
    (!streams.is_empty()).then(|| DeviceSettings {
        streams,
        ..DeviceSettings::default()
    })
}
