use sdrmm_wire::decode::{DecodedRecord, DecoderEvent};
use serde_json::{Value, json};

pub fn event(kind: &str, data: Value) -> DecoderEvent {
    serde_json::from_value(json!({ "kind": kind, "data": data }))
        .unwrap_or_else(|error| panic!("{kind} does not parse: {error}"))
}

pub fn record_at(event: DecoderEvent, at: &str, channel: u32) -> DecodedRecord {
    DecodedRecord {
        origin: None,
        device_set: 1,
        channel,
        at: at.to_owned(),
        freq_hz: 1_090_000_000.0,
        event,
        sinks: Vec::new(),
    }
}
