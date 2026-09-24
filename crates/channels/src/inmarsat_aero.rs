mod decoder;
mod demod;
mod frame;
mod framer;
mod oqpsk;
mod satellite;
mod state;
mod su;
mod taps;

#[cfg(test)]
mod burst;
#[cfg(test)]
mod cchannel;
#[cfg(test)]
mod coherent;
#[cfg(test)]
mod modulate;
#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DataLinkMessage, DecoderEvent,
    DecoderFamily, InmarsatAeroParams,
};
use serde::Serialize;
use serde_json::{Value, json};
use self::decoder::{AeroChannelDecoder, AeroEvent, INPUT_RATE};
use crate::{
    ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx,
    acars::block as acars_block, check_input_rate,
    datalink::{self, Quality},
};

const HALF_BANDWIDTH: f64 = 6_500.0;
const P_CHANNEL: &str = "p-channel";

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

pub struct InmarsatAeroChannel {
    decoder: AeroChannelDecoder,
    events: Vec<AeroEvent>,
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
        map.entry("frame_header").or_insert_with(|| header.to_json());
    }
    if let Some(lock) = &event.lock {
        map.entry("superframe_lock")
            .or_insert_with(|| lock.clone());
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

impl ChannelRx for InmarsatAeroChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        params(&settings)?;
        Ok(Self {
            decoder: AeroChannelDecoder::new(),
            events: Vec::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        params(&settings).map(|_| ())
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        self.decoder.process(iq, &mut self.events);
        out.events.extend(
            self.events
                .drain(..)
                .map(|event| DecoderEvent::InmarsatAero(to_message(&event))),
        );
    }
}
