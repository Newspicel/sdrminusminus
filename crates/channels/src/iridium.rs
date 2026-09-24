mod ddc;
mod decode;
mod demod;
#[cfg(test)]
mod encode;
mod frame;
mod gsm;
mod iip;
mod ira;
mod itl;
mod itl_tables;
mod lcw;
#[cfg(test)]
mod modulate;
mod ms;
mod mtpos;
mod rs;
mod sbd;
#[cfg(test)]
mod sensitivity;
#[cfg(test)]
mod tests;
mod u3;
mod voice;
mod wideband;

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DataLinkMessage, DecoderEvent,
    DecoderFamily, IridiumParams,
};
use serde::Serialize;
use serde_json::Value;
use xng_acars::block as acars;

use self::decode::ChannelDecoder;
use self::ira::IridiumFrame;
use crate::datalink::{self, Quality};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const CHANNEL_RATE: f64 = 250_000.0;
const HALF_BANDWIDTH: f64 = 25_000.0;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "iridium".to_owned(),
    name: "Iridium bursts".to_owned(),
    summary: "Iridium satellite bursts".to_owned(),
    family: DecoderFamily::Utility,
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: CHANNEL_RATE,
    has_audio: false,
    decoder_kind: Some("iridium".to_owned()),
    ..ChannelDescriptor::default()
});

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Body<'a, A: Serialize> {
    Acars(&'a A),
    Iridium { kind: &'a str, details: &'a Value },
}

pub struct IridiumChannel {
    decoder: ChannelDecoder,
    frames: Vec<IridiumFrame>,
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
    datalink::channel_filter(CHANNEL_RATE, HALF_BANDWIDTH)
}

fn message(frame: &IridiumFrame) -> DataLinkMessage {
    let fec_corrected = frame
        .details
        .get("bch_corrected")
        .and_then(Value::as_u64)
        .and_then(|fixed| u32::try_from(fixed).ok());
    match &frame.acars {
        Some(block) => datalink::message(
            &Body::Acars(&block.core),
            Quality {
                crc_ok: block.crc_ok,
                fec_corrected,
                ..Quality::default()
            },
            None,
        ),
        None => datalink::message(
            &Body::<()>::Iridium {
                kind: frame.kind,
                details: &frame.details,
            },
            Quality {
                crc_ok: frame
                    .details
                    .get("crc_ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                fec_corrected,
                ..Quality::default()
            },
            None,
        ),
    }
}

impl ChannelRx for IridiumChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        Ok(Self {
            decoder: ChannelDecoder::new(),
            frames: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.frames.clear();
        self.decoder.process(iq, &mut self.frames);
        out.events.extend(
            self.frames
                .iter()
                .map(|frame| DecoderEvent::Iridium(message(frame))),
        );
    }
}
