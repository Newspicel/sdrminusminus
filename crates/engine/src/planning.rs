use sdrmm_channels::ChannelError;
use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Capabilities, ChannelDescriptor, ChannelInfo, ChannelParams, ChannelSettings, DcArtifact,
    DeviceSettings, StreamSettings,
};

use crate::{DEFAULT_CENTER_HZ, EngineError, sample_rate_of};

pub(crate) fn channel_input_rate(descriptor: &ChannelDescriptor, device_rate: f64) -> f64 {
    match descriptor.native_rate_range() {
        Some(_) => device_rate,
        None => descriptor.input_rate_hz,
    }
}

pub(crate) fn descriptor_for(params: &ChannelParams) -> Result<ChannelDescriptor, EngineError> {
    let type_id = params.type_id();
    sdrmm_channels::descriptors()
        .into_iter()
        .find(|d| d.type_id == type_id)
        .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()).into())
}

pub(crate) fn validate_channel(
    descriptor: &ChannelDescriptor,
    settings: &ChannelSettings,
    device_rate: f64,
) -> Result<(), EngineError> {
    if !settings.offset_hz.is_finite() {
        return Err(ChannelError::InvalidSettings(format!(
            "offset_hz must be finite, got {}",
            settings.offset_hz
        ))
        .into());
    }
    if let Some(db) = settings.squelch_db
        && !db.is_finite()
    {
        return Err(
            ChannelError::InvalidSettings(format!("squelch_db must be finite, got {db}")).into(),
        );
    }
    if let Some(margin) = settings.squelch_auto_db
        && (!margin.is_finite()
            || !(sdrmm_wire::MIN_SQUELCH_AUTO_MARGIN_DB..=sdrmm_wire::MAX_SQUELCH_AUTO_MARGIN_DB)
                .contains(&margin))
    {
        return Err(ChannelError::InvalidSettings(format!(
            "squelch_auto_db must be in {}..={} dB above the noise floor, got {margin}",
            sdrmm_wire::MIN_SQUELCH_AUTO_MARGIN_DB,
            sdrmm_wire::MAX_SQUELCH_AUTO_MARGIN_DB
        ))
        .into());
    }
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
    if device_rate < descriptor.input_rate_hz {
        return Err(ChannelError::InvalidSettings(format!(
            "{} needs a device rate of at least {} Hz, device runs at {device_rate} Hz",
            descriptor.type_id, descriptor.input_rate_hz
        ))
        .into());
    }
    let (low, high) = sdrmm_channels::occupied_band(&settings.params);
    let band_low = settings.offset_hz + low;
    let band_high = settings.offset_hz + high;
    let nyquist = device_rate / 2.0;
    if band_low < -nyquist || band_high > nyquist {
        return Err(ChannelError::InvalidSettings(format!(
            "channel band [{band_low}, {band_high}] Hz exceeds the ±{nyquist} Hz device passband"
        ))
        .into());
    }
    if let Some((low, high)) = descriptor.native_rate_range() {
        if device_rate > high {
            return Err(ChannelError::InvalidSettings(format!(
                "{} reads the radio's own samples, so it runs with the receiver between \
                 {:.3} and {:.3} MHz — above that there is nothing left for a slicer to gain \
                 and the scan costs more than the smallest machine this has to run on can \
                 spare. The receiver is at {:.3} MHz.",
                descriptor.name,
                low / 1e6,
                high / 1e6,
                device_rate / 1e6,
            ))
            .into());
        }
        return Ok(());
    }
    if device_rate != descriptor.input_rate_hz {
        let widest = sdrmm_dsp::resamplable_bandwidth_hz(descriptor.input_rate_hz);
        if high - low >= widest {
            return Err(ChannelError::InvalidSettings(format!(
                "{} fills its whole {:.3} MHz channel, so there is no guard band left for a \
                 resampler to filter in — at {:.3} MHz the signal would arrive smeared and \
                 decode nothing. Set the receiver to exactly {:.3} MHz.",
                descriptor.name,
                (high - low) / 1e6,
                device_rate / 1e6,
                descriptor.input_rate_hz / 1e6,
            ))
            .into());
        }
    }
    Ok(())
}

pub(crate) fn tuner_reaches(capabilities: &Capabilities, hz: f64) -> bool {
    capabilities.freq_ranges.is_empty()
        || capabilities
            .freq_ranges
            .iter()
            .any(|r| hz >= r.min && hz <= r.max)
}

/// Where the front end's DC term is parked, relative to the offset that was asked for.
///
/// The first placement that clears every channel wins, so an operator who asks for 250 kHz keeps
/// 250 kHz unless a decoder is sitting there.
const LO_PLACEMENTS: [f64; 8] = [1.0, -1.0, 0.75, -0.75, 1.25, -1.25, 0.5, -0.5];

/// Room left between the artifact and the edge of a channel it must not sit in.
const LO_ARTIFACT_MARGIN_HZ: f64 = 2_000.0;

fn channel_half_width_hz(params: &ChannelParams) -> f64 {
    descriptor_for(params).map_or(0.0, |d| d.bandwidth_hz / 2.0) + LO_ARTIFACT_MARGIN_HZ
}

/// Whether parking the LO here would drop the DC term inside a channel being demodulated.
pub(crate) fn artifact_clears_channels(
    offset_hz: f64,
    settings: &DeviceSettings,
    capabilities: &Capabilities,
    channels: &[ChannelInfo],
) -> bool {
    channels.iter().all(|channel| {
        let center_hz = settings
            .for_stream(channel.stream, &capabilities.per_stream)
            .center_hz
            .unwrap_or(DEFAULT_CENTER_HZ);
        let artifact = center_hz - offset_hz;
        let tuned = center_hz + channel.settings.offset_hz;
        (artifact - tuned).abs() > channel_half_width_hz(&channel.settings.params)
    })
}

/// Where the front end's DC artifact ends up, and whether it is safe to remove it there.
///
/// The blocker notches whatever sits at 0 Hz, so it may only run once the artifact has been
/// parked clear of every channel. Blocking without that placement is what puts a notch through
/// a decoder tuned to the centre.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrontEndPlan {
    pub lo_offset_hz: f64,
    pub dc_block: bool,
}

/// Walks the placement ladder for the first displacement the tuner can reach with no decoder
/// sitting on it. `None` when every placement is spoken for.
fn place_artifact(
    wanted_hz: f64,
    capabilities: &Capabilities,
    settings: &DeviceSettings,
    channels: &[ChannelInfo],
    rate: f64,
) -> Option<f64> {
    if wanted_hz == 0.0 {
        return None;
    }
    let limit = sdrmm_wire::lo_offset_limit_hz(rate);
    LO_PLACEMENTS.into_iter().find_map(|scale| {
        let candidate = (wanted_hz * scale).clamp(-limit, limit);
        let placed = DeviceSettings {
            lo_offset_hz: Some(candidate),
            ..settings.clone()
        };
        (placed.effective_lo_offset_hz(capabilities, rate) == candidate
            && artifact_clears_channels(candidate, settings, capabilities, channels))
        .then_some(candidate)
    })
}

/// Settles the front end: where the LO sits and whether the DC term is removed.
///
/// Moving the LO is invisible downstream because the front end mixes the displacement back out,
/// so the placement is free to shift as channels come and go. Hardware the engine recognises
/// gets both decided for it; anything else is left to the operator, who may have a front end
/// that already corrects itself.
pub(crate) fn plan_front_end(
    capabilities: &Capabilities,
    settings: &DeviceSettings,
    channels: &[ChannelInfo],
) -> FrontEndPlan {
    if capabilities.dc_artifact == DcArtifact::None {
        return FrontEndPlan {
            lo_offset_hz: 0.0,
            dc_block: false,
        };
    }
    let rate = sample_rate_of(settings);
    // A centre the radio has not reported cannot be displaced: there is nothing to subtract the
    // offset from, and the front end would mix back a shift the hardware never took.
    if settings.center_hz.is_none() {
        return FrontEndPlan {
            lo_offset_hz: 0.0,
            dc_block: !capabilities.dc_artifact.is_managed() && settings.dc_block.unwrap_or(false),
        };
    }
    if capabilities.dc_artifact.is_managed() {
        let placed = place_artifact(
            sdrmm_wire::managed_lo_offset_hz(rate),
            capabilities,
            settings,
            channels,
            rate,
        );
        return FrontEndPlan {
            lo_offset_hz: placed.unwrap_or(0.0),
            dc_block: placed.is_some(),
        };
    }
    let asked = settings.effective_lo_offset_hz(capabilities, rate);
    FrontEndPlan {
        lo_offset_hz: place_artifact(asked, capabilities, settings, channels, rate)
            .unwrap_or(asked),
        dc_block: settings.dc_block.unwrap_or(false),
    }
}

/// Turns an operator-frame patch into the one the driver receives.
///
/// Changing the offset retunes the hardware even when the patch says nothing about frequency, so
/// a centre the patch left alone has to be restated before it is displaced.
pub(crate) fn hardware_delta(
    delta: &DeviceSettings,
    wanted: &DeviceSettings,
    offset_hz: f64,
    previous_hz: f64,
) -> DeviceSettings {
    let mut restated = delta.clone();
    if offset_hz != previous_hz {
        if restated.center_hz.is_none() {
            restated.center_hz = wanted.center_hz;
        }
        for stream in &wanted.streams {
            let Some(center_hz) = stream.center_hz else {
                continue;
            };
            match restated
                .streams
                .iter_mut()
                .find(|s| s.stream == stream.stream)
            {
                Some(existing) if existing.center_hz.is_none() => {
                    existing.center_hz = Some(center_hz);
                }
                Some(_) => {}
                None => restated.streams.push(StreamSettings {
                    stream: stream.stream,
                    center_hz: Some(center_hz),
                    gains: Vec::new(),
                    antenna: None,
                }),
            }
        }
    }
    restated.to_hardware(offset_hz)
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
