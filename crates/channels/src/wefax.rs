use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{FirC, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderFamily, WefaxParams};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

pub(crate) const INPUT_RATE_HZ: f64 = 12_000.0;
const FILTER_TAPS: usize = 127;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "wefax".to_owned(),
    name: "WEFAX".to_owned(),
    summary: "HF weather fax charts".to_owned(),
    family: DecoderFamily::Weather,
    bandwidth_hz: 1_600.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: true,
    decoder_kind: Some("wefax".to_owned()),
    ..ChannelDescriptor::default()
});

pub(crate) fn occupied_band(_p: &WefaxParams) -> (f64, f64) {
    (1_100.0, 2_700.0)
}

pub(crate) fn channel_filter(p: &WefaxParams) -> Result<ChannelFilter, ChannelError> {
    let (low, high) = occupied_band(p);
    let half = (high - low) / 2.0 / INPUT_RATE_HZ;
    let center = (high + low) / 2.0 / INPUT_RATE_HZ;
    Ok(ChannelFilter::Sideband(FirC::from_lowpass(
        &design_lowpass(FILTER_TAPS, half),
        center,
    )))
}

fn params(settings: &ChannelSettings) -> Result<&WefaxParams, ChannelError> {
    match &settings.params {
        ChannelParams::Wefax(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "wefax channel got {} params",
            other.type_id()
        ))),
    }
}

pub struct WefaxChannel {
    params: WefaxParams,
}

impl ChannelRx for WefaxChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        Ok(Self {
            params: *params(&settings)?,
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        self.params = *params(&settings)?;
        Ok(())
    }

    fn process(&mut self, _iq: &[Complex<f32>], _out: &mut ChannelOutputs) {}
}
