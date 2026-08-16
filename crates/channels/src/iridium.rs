use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, IridiumParams};
use xng_mode_iridium::IridiumChannelDecoder;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    xng_adapter,
};

const RATE: f64 = 250_000.0;
const HALF_BANDWIDTH: f64 = 25_000.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "iridium".to_owned(),
    name: "Iridium bursts".to_owned(),
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("iridium".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct IridiumChannel {
    decoder: IridiumChannelDecoder,
}

fn params(settings: &ChannelSettings) -> Result<&IridiumParams, ChannelError> {
    match &settings.params {
        ChannelParams::Iridium(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "Iridium channel got {} params",
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

impl ChannelRx for IridiumChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        let decoder = IridiumChannelDecoder::new(ctx.input_rate, 0.0)
            .map_err(ChannelError::InvalidSettings)?;
        Ok(Self { decoder })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let frames = self.decoder.process(iq);
        let level = self.decoder.level_dbfs();
        out.events.extend(frames.iter().map(|frame| {
            DecoderEvent::Iridium(xng_adapter::structured(xng_mode_iridium::to_message(
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
    use xng_mode_iridium::{frame, modulate};

    use super::*;
    use crate::testutil::{run_events, settings};

    #[test]
    fn decodes_a_remodulated_off_air_ring_alert() {
        let raw = "0011000000110000111100111111100001001010010011010011101101101100001001101011100001110011001100110000000111100010010011010011101011110101110100010010011010000111000101000111100110001000111111111111111111111111111111111111111111111111111111111111111110010111";
        let symbols: Vec<u8> = raw.bytes().map(|value| u8::from(value == b'1')).collect();
        let bits = frame::symbol_reverse(&symbols);
        let mut iq = vec![Complex::default(); 4_000];
        iq.extend(modulate::modulate(&bits, 64, RATE, 0.0, 0.5));
        iq.extend(vec![Complex::default(); 30_000]);
        let mut channel = IridiumChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Iridium(IridiumParams::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        assert!(events.iter().any(
            |event| matches!(event, DecoderEvent::Iridium(message) if message.message_type == "ring-alert" && message.crc_ok)
        ));
    }
}
