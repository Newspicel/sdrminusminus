use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, Vdl2Params};
use xng_mode_vdl2::Vdl2ChannelDecoder;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    xng_adapter,
};

const RATE: f64 = 100_000.0;
const HALF_BANDWIDTH: f64 = 8_500.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "vdl2".to_owned(),
    name: "VDL Mode 2".to_owned(),
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("vdl2".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct Vdl2Channel {
    decoder: Vdl2ChannelDecoder,
}

fn params(settings: &ChannelSettings) -> Result<&Vdl2Params, ChannelError> {
    match &settings.params {
        ChannelParams::Vdl2(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "VDL2 channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band() -> (f64, f64) {
    (-HALF_BANDWIDTH, HALF_BANDWIDTH)
}

pub(crate) fn channel_filter() -> ChannelFilter {
    xng_adapter::channel_filter(RATE, HALF_BANDWIDTH)
}

impl ChannelRx for Vdl2Channel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        let decoder =
            Vdl2ChannelDecoder::new(ctx.input_rate, 0.0).map_err(ChannelError::InvalidSettings)?;
        Ok(Self { decoder })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let frames = self.decoder.process(iq);
        let level = self.decoder.level_dbfs();
        out.events.extend(frames.iter().map(|frame| {
            DecoderEvent::Vdl2(xng_adapter::structured(xng_mode_vdl2::to_message(
                frame,
                0,
                level,
                xng_adapter::provenance(),
            )))
        }));
    }
}

#[cfg(test)]
mod tests {
    use xng_mode_vdl2::{
        avlc::{AddressType, encode_address},
        modulate::burst_iq,
    };

    use super::*;
    use crate::testutil::{run_events, settings};

    #[test]
    fn decodes_an_avlc_supervisory_frame() {
        let mut frame = Vec::new();
        frame.extend(encode_address(AddressType::Aircraft, 0x800F5C, true, false));
        frame.extend(encode_address(
            AddressType::GroundIcao,
            0x10A234,
            true,
            true,
        ));
        frame.push(0x01);
        let mut iq = vec![Complex::default(); 800];
        iq.extend(burst_iq(&[frame], RATE, 0.0, 0.5));
        iq.extend(vec![Complex::default(); 60_000]);
        let mut channel = Vdl2Channel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Vdl2(Vdl2Params::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DecoderEvent::Vdl2(message) if message.crc_ok))
        );
    }
}
