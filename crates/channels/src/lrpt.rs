use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderFamily, LrptParams};

use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

pub(crate) const INPUT_RATE_HZ: f64 = 288_000.0;
const FILTER_TAPS: usize = 127;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "lrpt".to_owned(),
    name: "Meteor LRPT".to_owned(),
    summary: "Meteor-M weather satellite pictures".to_owned(),
    family: DecoderFamily::Weather,
    bandwidth_hz: 140_000.0,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: false,
    has_video: true,
    decoder_kind: Some("lrpt".to_owned()),
    ..ChannelDescriptor::default()
});

pub(crate) fn occupied_band(_p: &LrptParams) -> (f64, f64) {
    let half = DESCRIPTOR.bandwidth_hz / 2.0;
    (-half, half)
}

pub(crate) fn channel_filter(p: &LrptParams) -> Result<ChannelFilter, ChannelError> {
    let (_, half) = occupied_band(p);
    Ok(ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(FILTER_TAPS, half / INPUT_RATE_HZ),
        1,
    )))
}

fn params(settings: &ChannelSettings) -> Result<&LrptParams, ChannelError> {
    match &settings.params {
        ChannelParams::Lrpt(p) => Ok(p),
        other => Err(ChannelError::InvalidSettings(format!(
            "lrpt channel got {} params",
            other.type_id()
        ))),
    }
}

pub struct LrptChannel {
    params: LrptParams,
}

impl ChannelRx for LrptChannel {
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
