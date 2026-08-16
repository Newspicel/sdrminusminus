use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, InmarsatStdcParams,
};
use xng_mode_stdc::StdcChannelDecoder;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    xng_adapter,
};

const RATE: f64 = 12_000.0;
const HALF_BANDWIDTH: f64 = 2_000.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "inmarsat_stdc".to_owned(),
    name: "Inmarsat STD-C / EGC".to_owned(),
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("inmarsat_stdc".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct InmarsatStdcChannel {
    decoder: StdcChannelDecoder,
}

fn params(settings: &ChannelSettings) -> Result<&InmarsatStdcParams, ChannelError> {
    match &settings.params {
        ChannelParams::InmarsatStdc(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "inmarsat STD-C channel got {} params",
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

impl ChannelRx for InmarsatStdcChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        let decoder =
            StdcChannelDecoder::new(ctx.input_rate, 0.0).map_err(ChannelError::InvalidSettings)?;
        Ok(Self { decoder })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let packets = self.decoder.process(iq);
        let level = self.decoder.level_dbfs();
        out.events.extend(packets.iter().map(|packet| {
            DecoderEvent::InmarsatStdc(xng_adapter::structured(xng_mode_stdc::to_message(
                packet,
                0,
                level,
                xng_adapter::provenance(),
            )))
        }));
    }
}

#[cfg(test)]
mod tests {
    use xng_mode_stdc::{frame::encode_frame, modulate::modulate, packet::build_packet};

    use super::*;
    use crate::testutil::{run_events, settings};

    #[test]
    fn decodes_a_bulletin_board_frame() {
        let mut payload = build_packet(&[0x7D, 1, 0x03, 0xE8, 0, 0, 1, 0x10, 0, 0, 0, 0]);
        payload.resize(639, 0);
        let symbols = encode_frame(&payload);
        let mut bits: Vec<u8> = (0..4_000).map(|index| (index % 2) as u8).collect();
        bits.extend(&symbols);
        bits.extend(&symbols);
        let iq = modulate(&bits, 1_200.0, RATE, 0.0, 0.5);
        let mut channel = InmarsatStdcChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::InmarsatStdc(InmarsatStdcParams::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        assert!(
            events.iter().any(
                |event| matches!(event, DecoderEvent::InmarsatStdc(message) if message.crc_ok)
            )
        );
    }
}
