mod acquisition;
mod burst;
mod cchannel;
mod coherent;
mod decoder;
mod demod;
mod frame;
mod framer;
mod msk;
mod oqpsk;
mod receiver;
mod satellite;
mod state;
mod su;
mod taps;

#[cfg(test)]
mod modulate;
#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use self::{
    burst::BurstEvent,
    decoder::{AeroChannelDecoder, AeroEvent, INPUT_RATE},
    receiver::{BurstReceiver, CircuitEvent, CircuitReceiver, burst_tag},
};
use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    acars::block as acars_block,
    check_input_rate,
    datalink::{self, Quality},
};
use num_complex::Complex;
use sdrmm_wire::{
    AeroChannel, ChannelDescriptor, ChannelParams, ChannelSettings, DataLinkMessage, DecoderEvent,
    DecoderFamily, InmarsatAeroParams,
};
use serde::Serialize;
use serde_json::{Value, json};

const HALF_BANDWIDTH: f64 = 6_500.0;
const P_CHANNEL: &str = "p-channel";
const C_CHANNEL_VOICE: &str = "c-channel-voice";

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "inmarsat_aero".to_owned(),
    name: "Inmarsat Classic Aero".to_owned(),
    summary: "Aircraft datalink over Inmarsat".to_owned(),
    family: DecoderFamily::Aviation,
    bandwidth_hz: HALF_BANDWIDTH * 2.0,
    input_rate_hz: INPUT_RATE,
    has_audio: false,
    decoder_kind: Some("inmarsat_aero".to_owned()),
    ..ChannelDescriptor::default()
});

enum Receiver {
    Forward(Box<AeroChannelDecoder>, Vec<AeroEvent>),
    Burst(Box<BurstReceiver>),
    Circuit(Box<CircuitReceiver>, Vec<CircuitEvent>),
}

impl Receiver {
    fn new(channel: AeroChannel) -> Self {
        match channel {
            AeroChannel::P => Self::Forward(Box::new(AeroChannelDecoder::new()), Vec::new()),
            AeroChannel::Burst => Self::Burst(Box::new(BurstReceiver::new())),
            AeroChannel::C => Self::Circuit(Box::new(CircuitReceiver::new()), Vec::new()),
        }
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        let messages: Vec<DataLinkMessage> = match self {
            Self::Forward(decoder, events) => {
                decoder.process(iq, events);
                events.drain(..).map(|event| to_message(&event)).collect()
            }
            Self::Burst(receiver) => receiver.process(iq).iter().map(burst_message).collect(),
            Self::Circuit(receiver, events) => {
                receiver.process(iq, events);
                events.drain(..).map(circuit_message).collect()
            }
        };
        out.events
            .extend(messages.into_iter().map(DecoderEvent::InmarsatAero));
    }
}

pub struct InmarsatAeroChannel {
    channel: AeroChannel,
    receiver: Receiver,
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
    datalink::channel_filter(INPUT_RATE, HALF_BANDWIDTH)
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AcarsBody<'a, C> {
    Acars(&'a C),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AeroBody {
    Aero { kind: String, details: Value },
    Undecoded,
}

fn enrich(details: &mut Value, event: &AeroEvent) {
    let Value::Object(map) = details else {
        return;
    };
    if let Some(Value::Object(satellite)) = &event.satellite {
        for (key, value) in satellite {
            map.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if let Some(header) = event.frame_header {
        map.entry("frame_header")
            .or_insert_with(|| header.to_json());
    }
    if let Some(lock) = &event.lock {
        map.entry("superframe_lock").or_insert_with(|| lock.clone());
    }
}

fn aero_body(kind: &str, event: &AeroEvent) -> AeroBody {
    let mut details = json!({});
    enrich(&mut details, event);
    AeroBody::Aero {
        kind: kind.to_owned(),
        details,
    }
}

fn su_body(su_event: &Value, event: &AeroEvent) -> AeroBody {
    let mut details = su_event.clone();
    if let Value::Object(map) = &mut details {
        map.insert("channel".to_owned(), json!(P_CHANNEL));
        map.insert("line_bit_rate".to_owned(), json!(event.bit_rate));
    }
    enrich(&mut details, event);
    AeroBody::Aero {
        kind: su::p_su_kind(su_event),
        details,
    }
}

fn to_message(event: &AeroEvent) -> DataLinkMessage {
    let quality = |crc_ok| Quality {
        crc_ok,
        fec_corrected: event.fec_corrected,
        snr_db: None,
        frequency_error_hz: None,
    };
    let raw = Some(event.user.data.as_slice());
    if let Some(block) = &event.acars {
        return datalink::message(&AcarsBody::Acars(&block.core), quality(block.crc_ok), raw);
    }
    let body = match &event.su_event {
        Some(su_event) => su_body(su_event, event),
        None if event.lock.is_some() => aero_body("p-channel-status", event),
        None if event.satellite.is_some() || event.frame_header.is_some() => {
            aero_body("aero-frame", event)
        }
        None => AeroBody::Undecoded,
    };
    datalink::message(&body, quality(true), raw)
}

fn burst_message(event: &BurstEvent) -> DataLinkMessage {
    let quality = |crc_ok| Quality {
        crc_ok,
        fec_corrected: Some(event.fec_corrected),
        snr_db: None,
        frequency_error_hz: None,
    };
    if let Some(user) = &event.user {
        let raw = Some(user.data.as_slice());
        return match su::parse_acars(&user.data) {
            Some(block) => {
                datalink::message(&AcarsBody::Acars(&block.core), quality(block.crc_ok), raw)
            }
            None => datalink::message(&AeroBody::Undecoded, quality(true), raw),
        };
    }
    let mut details = event.su_event.clone().unwrap_or_else(|| json!({}));
    let kind = su::p_su_kind(&details);
    if let Value::Object(map) = &mut details {
        map.insert("channel".to_owned(), json!(burst_tag(event.channel)));
        map.insert("line_bit_rate".to_owned(), json!(event.bit_rate));
    }
    datalink::message(&AeroBody::Aero { kind, details }, quality(true), None)
}

fn circuit_message(event: CircuitEvent) -> DataLinkMessage {
    let body = match event {
        CircuitEvent::SignalUnit { kind, details } => AeroBody::Aero {
            kind: kind.to_owned(),
            details,
        },
        CircuitEvent::VoiceStarted => AeroBody::Aero {
            kind: C_CHANNEL_VOICE.to_owned(),
            details: json!({ "channel": "c-channel" }),
        },
    };
    datalink::message(
        &body,
        Quality {
            crc_ok: true,
            ..Quality::default()
        },
        None,
    )
}

impl ChannelRx for InmarsatAeroChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let channel = params(&settings)?.channel;
        Ok(Self {
            channel,
            receiver: Receiver::new(channel),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let channel = params(&settings)?.channel;
        if channel != self.channel {
            self.channel = channel;
            self.receiver = Receiver::new(channel);
        }
        Ok(())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.receiver.process(iq, out);
    }
}
