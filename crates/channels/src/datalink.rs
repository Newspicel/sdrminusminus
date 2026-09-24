use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::DataLinkMessage;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::ChannelFilter;

const SERIALIZATION_ERROR: &str = "serialization_error";
const CHANNEL_TAPS: usize = 129;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quality {
    pub crc_ok: bool,
    pub fec_corrected: Option<u32>,
    pub snr_db: Option<f32>,
    pub frequency_error_hz: Option<f32>,
}

pub fn channel_filter(rate: f64, half_bandwidth: f64) -> ChannelFilter {
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(CHANNEL_TAPS, half_bandwidth / rate),
        1,
    ))
}

pub fn message<T: Serialize + ?Sized>(
    body: &T,
    quality: Quality,
    raw: Option<&[u8]>,
) -> DataLinkMessage {
    let raw = raw.map(hex);
    let body = match serde_json::to_value(body) {
        Ok(value) => value,
        Err(error) => return damaged(quality, raw, &error),
    };
    let message_type = find_string(&body, &["kind", "name", "type", "label"])
        .unwrap_or_else(|| "frame".to_owned());
    let station = find_string(
        &body,
        &[
            "from", "tail", "src", "mmsi", "icao", "mes_id", "gs_name", "sat",
        ],
    );
    let text = find_string(&body, &["text", "message", "nature_description"]);
    let lat = find_number(&body, &["lat", "latitude"]);
    let lon = find_number(&body, &["lon", "longitude"]);
    DataLinkMessage {
        message_type,
        station,
        text,
        crc_ok: quality.crc_ok,
        fec_corrected: quality.fec_corrected,
        snr_db: quality.snr_db,
        frequency_error_hz: quality.frequency_error_hz,
        lat,
        lon,
        raw,
        details: body,
    }
}

fn damaged(quality: Quality, raw: Option<String>, error: &serde_json::Error) -> DataLinkMessage {
    DataLinkMessage {
        message_type: SERIALIZATION_ERROR.to_owned(),
        station: None,
        text: Some(error.to_string()),
        crc_ok: false,
        fec_corrected: quality.fec_corrected,
        snr_db: quality.snr_db,
        frequency_error_hz: quality.frequency_error_hz,
        lat: None,
        lon: None,
        raw,
        details: Value::Object(Map::from_iter([(
            SERIALIZATION_ERROR.to_owned(),
            Value::String(error.to_string()),
        )])),
    }
}

fn find_string(value: &Value, names: &[&str]) -> Option<String> {
    match value {
        Value::Object(object) => {
            for name in names {
                match object.get(*name) {
                    Some(Value::String(found)) if !found.is_empty() => return Some(found.clone()),
                    Some(Value::Number(found)) => return Some(found.to_string()),
                    _ => {}
                }
            }
            object.values().find_map(|value| find_string(value, names))
        }
        Value::Array(items) => items.iter().find_map(|item| find_string(item, names)),
        _ => None,
    }
}

fn find_number(value: &Value, names: &[&str]) -> Option<f64> {
    match value {
        Value::Object(object) => {
            for name in names {
                if let Some(found) = object.get(*name).and_then(Value::as_f64) {
                    return Some(found);
                }
            }
            object.values().find_map(|value| find_number(value, names))
        }
        Value::Array(items) => items.iter().find_map(|item| find_number(item, names)),
        _ => None,
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn normalizes_nested_protocol_fields() {
        let value = serde_json::json!({
            "type": "hfdl",
            "kind": "position",
            "details": {"icao": "ABC123", "lat": 1.5, "lon": -2.5}
        });
        assert_eq!(find_string(&value, &["kind"]), Some("position".to_owned()));
        assert_eq!(find_string(&value, &["icao"]), Some("ABC123".to_owned()));
        assert_eq!(find_number(&value, &["lat"]), Some(1.5));
        assert_eq!(find_number(&value, &["lon"]), Some(-2.5));
        assert_eq!(hex(&[0xab, 0xcd]), "abcd");
    }

    #[test]
    fn reaches_fields_nested_inside_arrays() {
        let value = serde_json::json!({
            "reports": [{"icao": "ABC123", "lat": 1.5, "lon": -2.5}]
        });
        assert_eq!(find_string(&value, &["icao"]), Some("ABC123".to_owned()));
        assert_eq!(find_number(&value, &["lat"]), Some(1.5));
        assert_eq!(find_number(&value, &["lon"]), Some(-2.5));
    }

    #[test]
    fn prefers_the_earlier_name_over_a_later_one() {
        let value = serde_json::json!({"tail": "VT-ANB", "from": "A1B2C3"});
        assert_eq!(
            find_string(&value, &["from", "tail"]),
            Some("A1B2C3".to_owned())
        );
        assert_eq!(
            find_string(&value, &["tail", "from"]),
            Some("VT-ANB".to_owned())
        );
    }

    #[test]
    fn picks_siblings_in_a_stable_order() {
        let first = serde_json::json!({"a": {"lat": 1.0}, "b": {"lat": 2.0}});
        let second = serde_json::json!({"b": {"lat": 2.0}, "a": {"lat": 1.0}});
        assert_eq!(find_number(&first, &["lat"]), Some(1.0));
        assert_eq!(find_number(&second, &["lat"]), Some(1.0));
    }

    #[test]
    fn a_serialization_failure_never_looks_decoded() {
        let mut body = BTreeMap::new();
        body.insert(vec![1u8], 2u8);
        let quality = Quality {
            crc_ok: true,
            fec_corrected: Some(2),
            snr_db: Some(12.5),
            frequency_error_hz: Some(-3.0),
        };
        let failed = message(&body, quality, Some(&[0xab, 0xcd]));
        assert_eq!(failed.message_type, SERIALIZATION_ERROR);
        assert!(!failed.crc_ok);
        assert_eq!(failed.station, None);
        assert_eq!(failed.raw.as_deref(), Some("abcd"));
        assert!(message(&serde_json::json!({"type": "undecoded"}), quality, None).crc_ok);
    }
}
