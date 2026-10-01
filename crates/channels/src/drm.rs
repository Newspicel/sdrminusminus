pub(crate) mod aac;
pub(crate) mod audio;
pub(crate) mod bits;
pub(crate) mod cells;
mod channel;
pub(crate) mod coding;
mod estimate;
pub(crate) mod fac;
pub(crate) mod mlc;
pub(crate) mod mode;
pub(crate) mod msc;
mod ofdm;
mod receiver;
pub(crate) mod sdc;
pub(crate) mod text;

use sdrmm_dsp::{FirC, design_lowpass};
use sdrmm_wire::{ChannelParams, ChannelSettings, DrmMode, DrmParams};

pub use channel::DrmChannel;

use crate::{ChannelError, ChannelFilter};

const INPUT_RATE_HZ: f64 = 192_000.0;
const MIN_DRM30_BANDWIDTH_HZ: f64 = 4_500.0;
const MAX_DRM30_BANDWIDTH_HZ: f64 = 20_000.0;
const DRM_PLUS_BANDWIDTH_HZ: f64 = 100_000.0;
const FILTER_TAPS: usize = 127;
const FILTER_MARGIN_HZ: f64 = 1_000.0;

fn params(settings: &ChannelSettings) -> Result<DrmParams, ChannelError> {
    let ChannelParams::Drm(p) = settings.params else {
        return Err(ChannelError::InvalidSettings(format!(
            "drm channel got {} params",
            settings.params.type_id()
        )));
    };
    let valid = p.bandwidth_hz.is_finite()
        && match p.mode {
            DrmMode::Drm30 => {
                (MIN_DRM30_BANDWIDTH_HZ..=MAX_DRM30_BANDWIDTH_HZ).contains(&p.bandwidth_hz)
            }
            DrmMode::DrmPlus | DrmMode::Auto => {
                (p.bandwidth_hz - DRM_PLUS_BANDWIDTH_HZ).abs() < 1.0
            }
        }
        && p.service.is_none_or(|service| service < 4);
    if valid {
        Ok(p)
    } else {
        Err(ChannelError::InvalidSettings(format!(
            "DRM bandwidth {} Hz or service {:?} is invalid for {:?}",
            p.bandwidth_hz, p.service, p.mode
        )))
    }
}

pub(crate) fn occupied_band(p: &DrmParams) -> (f64, f64) {
    match p.mode {
        DrmMode::Drm30 if p.bandwidth_hz < 6_000.0 => (0.0, p.bandwidth_hz),
        DrmMode::Drm30 if p.bandwidth_hz < 15_000.0 => {
            (-p.bandwidth_hz / 2.0, p.bandwidth_hz / 2.0)
        }
        DrmMode::Drm30 => (-p.bandwidth_hz / 4.0, 3.0 * p.bandwidth_hz / 4.0),
        DrmMode::Auto | DrmMode::DrmPlus => {
            (-DRM_PLUS_BANDWIDTH_HZ / 2.0, DRM_PLUS_BANDWIDTH_HZ / 2.0)
        }
    }
}

pub(crate) fn channel_filter(p: &DrmParams) -> Result<ChannelFilter, ChannelError> {
    let p = params(&ChannelSettings {
        frequency_hz: 0.0,
        squelch: sdrmm_wire::Squelch::Off,
        params: ChannelParams::Drm(*p),
        blanker: Default::default(),
    })?;
    let (low, high) = occupied_band(&p);
    let low = (low - FILTER_MARGIN_HZ).max(-INPUT_RATE_HZ * 0.45);
    let high = (high + FILTER_MARGIN_HZ).min(INPUT_RATE_HZ * 0.45);
    let half_width = (high - low) / 2.0 / INPUT_RATE_HZ;
    let centre = (high + low) / 2.0 / INPUT_RATE_HZ;
    Ok(ChannelFilter::Sideband(FirC::from_lowpass(
        &design_lowpass(FILTER_TAPS, half_width),
        centre,
    )))
}
