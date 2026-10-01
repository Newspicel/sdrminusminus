use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderFamily, RadiosondeParams,
};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

pub(crate) const INPUT_RATE_HZ: f64 = 48_000.0;
const FILTER_TAPS: usize = 127;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "radiosonde".to_owned(),
    name: "Radiosonde".to_owned(),
    summary: "Weather balloon telemetry".to_owned(),
    family: DecoderFamily::Weather,
    bandwidth_hz: 20_000.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: false,
    decoder_kind: Some("radiosonde".to_owned()),
    ..ChannelDescriptor::default()
});

pub(crate) fn occupied_band(_p: &RadiosondeParams) -> (f64, f64) {
    let half = DESCRIPTOR.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &RadiosondeParams) -> Result<ChannelFilter, ChannelError> {
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(FILTER_TAPS, half / INPUT_RATE_HZ),
        1,
    )))
}

fn params(settings: &ChannelSettings) -> Result<&RadiosondeParams, ChannelError> {
    match &settings.params {
        ChannelParams::Radiosonde(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "radiosonde channel got {} params",
            other.type_id()
        ))),
    }
}

pub struct RadiosondeChannel {
    params: RadiosondeParams,
}

impl ChannelRx for RadiosondeChannel {
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
