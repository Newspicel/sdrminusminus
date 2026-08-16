use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DscParams};
use xng_mode_dsc::DscChannelDecoder;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    xng_adapter,
};

const RATE: f64 = 8_000.0;
const HALF_BANDWIDTH: f64 = 250.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "dsc".to_owned(),
    name: "Digital Selective Calling".to_owned(),
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("dsc".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct DscChannel {
    decoder: DscChannelDecoder,
}

fn params(settings: &ChannelSettings) -> Result<&DscParams, ChannelError> {
    match &settings.params {
        ChannelParams::Dsc(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "dsc channel got {} params",
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

impl ChannelRx for DscChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        let decoder =
            DscChannelDecoder::new(ctx.input_rate, 0.0).map_err(ChannelError::InvalidSettings)?;
        Ok(Self { decoder })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let messages = self.decoder.process(iq);
        let level = self.decoder.level_dbfs();
        out.events.extend(messages.iter().map(|message| {
            DecoderEvent::Dsc(xng_adapter::structured(xng_mode_dsc::to_message(
                message,
                0,
                level,
                xng_adapter::provenance(),
            )))
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{run_events, settings};

    #[test]
    fn decodes_a_distress_alert_fixture() {
        let symbols = [
            112, 112, 12, 34, 56, 78, 90, 100, 12, 34, 45, 67, 89, 12, 12, 34, 117, 88,
        ];
        let iq = xng_mode_dsc::modulate::call_iq(&symbols, RATE, 0.0, 0.8);
        let mut channel = DscChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Dsc(DscParams::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        let message = events
            .iter()
            .find_map(|event| match event {
                DecoderEvent::Dsc(message) => Some(message),
                _ => None,
            })
            .expect("DSC message");
        assert!(message.crc_ok);
        assert_eq!(message.message_type, "distress_alert");
        assert_eq!(message.station.as_deref(), Some("123456789"));
    }
}
