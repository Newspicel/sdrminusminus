use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, HfdlParams};
use xng_mode_hfdl::HfdlChannelDecoder;

use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate,
    xng_adapter,
};

const RATE: f64 = 12_000.0;
const HALF_BANDWIDTH: f64 = 3_000.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "hfdl".to_owned(),
    name: "High Frequency Data Link".to_owned(),
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: RATE,
    has_audio: false,
    decoder_kind: Some("hfdl".to_owned()),
    ..ChannelDescriptor::default()
});

pub struct HfdlChannel {
    decoder: HfdlChannelDecoder,
}

fn params(settings: &ChannelSettings) -> Result<&HfdlParams, ChannelError> {
    match &settings.params {
        ChannelParams::Hfdl(params) => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "HFDL channel got {} params",
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

impl ChannelRx for HfdlChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        let decoder =
            HfdlChannelDecoder::new(ctx.input_rate, 0.0).map_err(ChannelError::InvalidSettings)?;
        Ok(Self { decoder })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let events = self.decoder.process(iq);
        let level = self.decoder.level_dbfs();
        out.events.extend(events.iter().map(|event| {
            DecoderEvent::Hfdl(xng_adapter::structured(xng_mode_hfdl::to_message(
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
    use xng_mode_hfdl::{
        fec::SETTINGS,
        modulate::{burst_symbols, modulate},
        pdu,
    };

    use super::*;
    use crate::testutil::{run_events, settings};

    #[test]
    fn decodes_a_ground_station_squitter() {
        let payload = pdu::build_spdu(7, 1_234, 52);
        let symbols = burst_symbols(&payload, &SETTINGS[0]);
        let mut iq = vec![Complex::default(); 3_000];
        iq.extend(modulate(&symbols, RATE, 1_440.0, 0.5));
        iq.extend(vec![Complex::default(); 3_000]);
        let mut channel = HfdlChannel::new(
            ChannelCtx { input_rate: RATE },
            settings(ChannelParams::Hfdl(HfdlParams::default())),
        )
        .expect("channel");
        let events = run_events(&mut channel, &iq);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DecoderEvent::Hfdl(message) if message.crc_ok))
        );
    }
}
