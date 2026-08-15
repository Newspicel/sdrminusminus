use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_wire::DataLinkMessage;
use serde_json::{Map, Value};
use xng_types::{AppInfo, Message, Provenance, StationIdentity};

use crate::ChannelFilter;

pub fn channel_filter(rate: f64, half_bandwidth: f64) -> ChannelFilter {
    ChannelFilter::Symmetric(Decimator::new(
        &design_lowpass(129, half_bandwidth / rate),
        1,
    ))
}

pub fn provenance() -> Provenance {
    Provenance {
        station: StationIdentity::new("sdr--"),
        app: AppInfo {
            name: "sdr--".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        sdr: None,
        channel: None,
    }
}

pub fn structured(message: Message) -> DataLinkMessage {
    let body = match serde_json::to_value(&message.body) {
        Ok(value) => value,
        Err(error) => Value::Object(Map::from_iter([(
            "serialization_error".to_owned(),
            Value::String(error.to_string()),
        )])),
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
        crc_ok: message.decode.crc_ok,
        fec_corrected: message.decode.fec_corrected,
        snr_db: message.signal.snr_db,
        frequency_error_hz: message.signal.freq_skew_hz,
        lat,
        lon,
        raw: message.raw.map(|bytes| hex(&bytes)),
        details: body,
    }
}

fn find_string(value: &Value, names: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    for name in names {
        if let Some(value) = object.get(*name) {
            match value {
                Value::String(value) if !value.is_empty() => return Some(value.clone()),
                Value::Number(value) => return Some(value.to_string()),
                _ => {}
            }
        }
    }
    object.values().find_map(|value| find_string(value, names))
}

fn find_number(value: &Value, names: &[&str]) -> Option<f64> {
    let object = value.as_object()?;
    for name in names {
        if let Some(value) = object.get(*name).and_then(Value::as_f64) {
            return Some(value);
        }
    }
    object.values().find_map(|value| find_number(value, names))
}

fn hex(bytes: &[u8]) -> String {
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
}
