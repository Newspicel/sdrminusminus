use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, InmarsatAeroParams,
};
use xng_mode_aero::AeroChannelDecoder;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    xng_adapter,
};

const RATE: f64 = 48_000.0;
const HALF_BANDWIDTH: f64 = 6_500.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "inmarsat_aero".to_owned(),
    name: "Inmarsat Classic Aero".to_owned(),
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("inmarsat_aero".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct InmarsatAeroChannel {
    decoder: AeroChannelDecoder,
}

fn params(settings: &ChannelSettings) -> Result<&InmarsatAeroParams, ChannelError> {
    match &settings.params {
        ChannelParams::InmarsatAero(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "inmarsat Aero channel got {} params",
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

impl ChannelRx for InmarsatAeroChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        let decoder =
            AeroChannelDecoder::new(ctx.input_rate, 0.0).map_err(ChannelError::InvalidSettings)?;
        Ok(Self { decoder })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let events = self.decoder.process(iq);
        let level = self.decoder.level_dbfs();
        out.events.extend(events.iter().map(|event| {
            DecoderEvent::InmarsatAero(xng_adapter::structured(xng_mode_aero::to_message(
                event,
                0,
                level,
                xng_adapter::provenance(),
            )))
        }));
    }
}

#[cfg(test)]
mod tests {
    use xng_mode_aero::{frame::FrameEncoder, modulate::modulate, su};

    use super::*;
    use crate::testutil::{run_events, settings};

    #[test]
    fn decodes_a_p_channel_intermediate_signal_unit() {
        let mut units = su::build_isu_chain(0xA1B2C3, 0x44, 1, 7, b"TEST");
        while !units.len().is_multiple_of(6) {
            units.push(su::fill_su());
        }
        let mut encoder = FrameEncoder::new(600);
        let mut bits: Vec<u8> = (0..160).map(|index| (index % 2) as u8).collect();
        for (index, chunk) in units.chunks(6).enumerate() {
            let bytes: Vec<u8> = chunk.iter().flatten().copied().collect();
            bits.extend(encoder.encode(&bytes, index as u8));
        }
        bits.extend((0..64).map(|index| (index % 2) as u8));
        let iq = modulate(&bits, 600.0, RATE, 0.0, 0.5);
        let mut channel = InmarsatAeroChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::InmarsatAero(InmarsatAeroParams::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DecoderEvent::InmarsatAero(_)))
        );
    }
}
