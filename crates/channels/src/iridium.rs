mod burst;
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
mod receiver;
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

use crate::acars::block as acars;
use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DataLinkMessage, DecoderEvent,
    DecoderFamily, IridiumParams, IridiumSpan,
};
use serde::Serialize;
use serde_json::Value;

use self::ira::IridiumFrame;
use self::receiver::ChannelDecoder;
use self::wideband::{USABLE_FRACTION, WidebandDecoder};
use crate::datalink::{self, Quality};
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_rate};

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

enum Decoder {
    Channel(Box<ChannelDecoder>),
    Wide(Box<WidebandDecoder>),
}

impl Decoder {
    fn new(span: IridiumSpan) -> Result<Self, ChannelError> {
        match span.sample_rate_hz() {
            None => Ok(Self::Channel(Box::new(ChannelDecoder::new()))),
            Some(rate) => Ok(Self::Wide(Box::new(WidebandDecoder::new(rate)?))),
        }
    }

    fn process(&mut self, iq: &[Complex<f32>], frames: &mut Vec<IridiumFrame>) {
        match self {
            Self::Channel(decoder) => decoder.process(iq, frames),
            Self::Wide(decoder) => decoder.process(iq, frames),
        }
    }
}

pub struct IridiumChannel {
    span: IridiumSpan,
    decoder: Decoder,
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

pub(crate) fn input_rate(params: &IridiumParams) -> f64 {
    params.span.sample_rate_hz().unwrap_or(CHANNEL_RATE)
}

pub(crate) fn occupied_band(params: &IridiumParams) -> (f64, f64) {
    let half = params
        .span
        .sample_rate_hz()
        .map_or(HALF_BANDWIDTH, |rate| rate * USABLE_FRACTION);
    (-half, half)
}

pub(crate) fn channel_filter(params: &IridiumParams) -> ChannelFilter {
    match params.span {
        IridiumSpan::Channel => datalink::channel_filter(CHANNEL_RATE, HALF_BANDWIDTH),
        _ => ChannelFilter::Passthrough,
    }
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
                frequency_error_hz: frame.offset_hz,
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
                frequency_error_hz: frame.offset_hz,
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
        let params = params(&settings)?;
        check_rate(ctx, &DESCRIPTOR, input_rate(params))?;
        Ok(Self {
            span: params.span,
            decoder: Decoder::new(params.span)?,
            frames: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        if params(&settings)?.span == self.span {
            Ok(())
        } else {
            Err(ChannelError::InvalidSettings(
                "Iridium span changes need a rebuilt channel".to_owned(),
            ))
        }
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
