//! Decoder output types (PLAN §5: "decoder output events … typed JSON", §13 Phase 2).
//!
//! Every wave-1 decoder emits one variant of [`DecoderEvent`]; the engine wraps it in a
//! [`DecodedRecord`] with the coordinates the DSP plane cannot know (wall-clock time) and
//! pushes it to clients as `ServerEvent::Decoded` and to the decoder-log database (PLAN §11).
//! One definition per decoder here is what makes the log table, the CSV export, the map, and
//! the React panels share a single shape.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// RDS state after a group changed it (PLAN §13: 57 kHz BPSK, group/AF/RT decode). RDS is a
/// slowly-accreting picture rather than a stream of independent frames, so an event is the
/// current best view of the station, emitted only when a field actually changed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RdsUpdate {
    /// Programme Identification, as the 4 hex digits everyone quotes it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi: Option<String>,
    /// Programme Service name (8 chars), once every segment has been seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ps: Option<String>,
    /// RadioText (up to 64 chars), once the A/B flag closes a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radiotext: Option<String>,
    /// Programme Type code (0–31).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty: Option<u8>,
    /// Programme Type name for [`RdsUpdate::pty`] under the RDS (EU) table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_name: Option<String>,
    /// Traffic Programme flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tp: Option<bool>,
    /// Traffic Announcement flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ta: Option<bool>,
    /// Music (true) / Speech (false) switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music: Option<bool>,
    /// Alternative frequencies in Hz, as advertised in group 0A.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alt_freqs_hz: Vec<f64>,
    /// Groups accepted since the channel started.
    pub groups: u64,
    /// Blocks rejected by the syndrome check since the channel started.
    pub block_errors: u64,
}

/// POCSAG message class (PLAN §13: 512/1200/2400 baud pagers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PocsagPayload {
    /// Function-only page with no data codewords.
    Tone,
    /// BCD digits (function 0 by convention).
    Numeric,
    /// 7-bit ASCII (function 3 by convention).
    Alpha,
}

/// One decoded POCSAG page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PocsagMessage {
    /// 21-bit receiver address (RIC).
    pub address: u32,
    /// Function bits 0–3 (the "A/B/C/D" a pager shows).
    pub function: u8,
    /// Bit rate the batch was decoded at.
    pub baud: u16,
    pub payload: PocsagPayload,
    pub text: String,
    /// Single-bit errors the BCH(31,21) decoder repaired across the message's codewords.
    pub errors_corrected: u32,
}

/// One decoded Mode S / ADS-B frame (PLAN §13: preamble correlation + Mode S CRC).
/// Fields are `Option` because which ones a frame carries depends on its type code — a
/// position frame has no callsign, an identification frame has no altitude.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AdsbMessage {
    /// ICAO 24-bit address as 6 hex digits — the aircraft's identity across frames.
    pub icao: String,
    /// Downlink Format (17 = ADS-B extended squitter, 11 = all-call reply).
    pub df: u8,
    /// Extended-squitter type code, when the frame has an ME field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_code: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_ft: Option<i32>,
    /// Latitude in degrees, once a CPR even/odd pair has been solved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_speed_kt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_rate_fpm: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squawk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_ground: Option<bool>,
    /// The raw frame as hex — the interop format every Mode S tool speaks.
    pub raw: String,
}

/// One decoded AIS message (PLAN §13: GMSK/NRZI over HDLC framing).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AisMessage {
    /// Maritime Mobile Service Identity — the vessel's identity across messages.
    pub mmsi: u32,
    /// ITU-R M.1371 message type (1–3 position report, 5 static data, 18/19 class B …).
    pub msg_type: u8,
    /// Which of the two AIS channels the burst arrived on (`A` = 161.975 MHz).
    pub ais_channel: char,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_sign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    /// Speed over ground in knots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sog_kt: Option<f64>,
    /// Course over ground in degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cog_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<u16>,
    /// Navigational status code (0 = under way using engine …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_status: Option<u8>,
    /// The `!AIVDM` sentence — the interop format every AIS tool speaks.
    pub nmea: String,
}

/// One decoded AX.25 frame, with the APRS fields parsed out when the info field carries them
/// (PLAN §13: AFSK1200 + 9600 G3RUH).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AprsPacket {
    /// Source callsign with SSID, e.g. `DL1ABC-9`.
    pub source: String,
    pub destination: String,
    /// Digipeater path, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    /// Raw information field.
    pub info: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    /// APRS symbol as `table` + `code`, e.g. `/>` for a car.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_kt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_ft: Option<i32>,
    /// TNC2 monitor line (`SRC>DEST,PATH:info`) — the interop format.
    pub tnc2: String,
}

/// A run of decoded RTTY characters (PLAN §13: Baudot over FSK).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RttyText {
    pub text: String,
}

/// A run of decoded Morse characters plus the speed the tracker settled on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MorseText {
    pub text: String,
    /// Estimated sending speed in words per minute (PARIS standard).
    pub wpm: f32,
}

/// Typed decoder output (PLAN §5). Adjacently tagged so the generated TypeScript is a
/// discriminated union on `kind` that panels can exhaustively `switch` on, and so the log
/// database can index on `kind` without parsing the blob.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum DecoderEvent {
    Rds(RdsUpdate),
    Pocsag(PocsagMessage),
    Adsb(AdsbMessage),
    Ais(AisMessage),
    Aprs(AprsPacket),
    Rtty(RttyText),
    Morse(MorseText),
}

impl DecoderEvent {
    /// Stable discriminator, matching the channel `type_id` that produces it.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Rds(_) => "rds",
            Self::Pocsag(_) => "pocsag",
            Self::Adsb(_) => "adsb",
            Self::Ais(_) => "ais",
            Self::Aprs(_) => "aprs",
            Self::Rtty(_) => "rtty",
            Self::Morse(_) => "morse",
        }
    }

    /// One-line human summary: the log list's row text and the CSV export's `summary`
    /// column. Lives here so every consumer renders an event the same way.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Rds(r) => {
                let mut parts = Vec::new();
                if let Some(pi) = &r.pi {
                    parts.push(format!("PI {pi}"));
                }
                if let Some(ps) = &r.ps {
                    parts.push(ps.clone());
                }
                if let Some(pty) = &r.pty_name {
                    parts.push(pty.clone());
                }
                if let Some(rt) = &r.radiotext {
                    parts.push(rt.clone());
                }
                parts.join(" · ")
            }
            Self::Pocsag(p) => {
                if p.text.is_empty() {
                    format!("{} ({})", p.address, p.function)
                } else {
                    format!("{}: {}", p.address, p.text)
                }
            }
            Self::Adsb(a) => {
                let mut parts = vec![a.icao.clone()];
                if let Some(cs) = &a.callsign {
                    parts.push(cs.trim().to_owned());
                }
                if let Some(alt) = a.altitude_ft {
                    parts.push(format!("{alt} ft"));
                }
                if let (Some(lat), Some(lon)) = (a.lat, a.lon) {
                    parts.push(format!("{lat:.4}, {lon:.4}"));
                }
                parts.join(" · ")
            }
            Self::Ais(m) => {
                let mut parts = vec![m.mmsi.to_string()];
                if let Some(name) = &m.name {
                    parts.push(name.trim().to_owned());
                }
                if let (Some(lat), Some(lon)) = (m.lat, m.lon) {
                    parts.push(format!("{lat:.4}, {lon:.4}"));
                }
                parts.join(" · ")
            }
            Self::Aprs(p) => p.tnc2.clone(),
            Self::Rtty(t) => t.text.clone(),
            Self::Morse(m) => m.text.clone(),
        }
    }

    /// `(lat, lon)` when the event places something on the map (PLAN §13: ADS-B/AIS/APRS
    /// share one map feature), so the client never re-derives per-decoder position rules.
    #[must_use]
    pub fn position(&self) -> Option<(f64, f64)> {
        let (lat, lon) = match self {
            Self::Adsb(a) => (a.lat, a.lon),
            Self::Ais(m) => (m.lat, m.lon),
            Self::Aprs(p) => (p.lat, p.lon),
            _ => (None, None),
        };
        lat.zip(lon)
    }

    /// Stable identity of the emitter within a decoder — aircraft ICAO, vessel MMSI, APRS
    /// callsign, pager address. Map layers and the log's "latest per station" view key on it.
    #[must_use]
    pub fn station(&self) -> Option<String> {
        match self {
            Self::Rds(r) => r.pi.clone(),
            Self::Pocsag(p) => Some(p.address.to_string()),
            Self::Adsb(a) => Some(a.icao.clone()),
            Self::Ais(m) => Some(m.mmsi.to_string()),
            Self::Aprs(p) => Some(p.source.clone()),
            Self::Rtty(_) | Self::Morse(_) => None,
        }
    }
}

/// A decoder event with the coordinates the DSP plane cannot supply. The engine stamps `at`
/// on the control plane (the DSP thread never formats time) and computes `freq_hz` from the
/// device center plus the channel offset at the moment the frame was produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DecodedRecord {
    pub device_set: u32,
    pub channel: u32,
    /// RFC3339 UTC.
    pub at: String,
    /// Absolute RF frequency of the channel when the frame arrived, in Hz.
    pub freq_hz: f64,
    pub event: DecoderEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `{"kind": …, "data": …}` tagging is what the generated TS union discriminates on,
    /// and what the log table's `kind` column mirrors; lock it.
    #[test]
    fn decoder_event_is_adjacently_tagged() {
        let ev = DecoderEvent::Pocsag(PocsagMessage {
            address: 1_234_567,
            function: 3,
            baud: 1_200,
            payload: PocsagPayload::Alpha,
            text: "TEST".to_owned(),
            errors_corrected: 1,
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "pocsag");
        assert_eq!(json["data"]["address"], 1_234_567);
        assert_eq!(json["data"]["payload"], "alpha");
        assert_eq!(ev.kind(), "pocsag");

        let back: DecoderEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }

    /// `kind()` must equal the serialized tag for every variant, or the log's indexed column
    /// and the client's union discriminator drift apart.
    #[test]
    fn kind_matches_the_serialized_tag() {
        for ev in [
            DecoderEvent::Rds(RdsUpdate::default()),
            DecoderEvent::Pocsag(PocsagMessage {
                address: 1,
                function: 0,
                baud: 512,
                payload: PocsagPayload::Tone,
                text: String::new(),
                errors_corrected: 0,
            }),
            DecoderEvent::Adsb(AdsbMessage::default()),
            DecoderEvent::Ais(AisMessage::default()),
            DecoderEvent::Aprs(AprsPacket::default()),
            DecoderEvent::Rtty(RttyText {
                text: String::new(),
            }),
            DecoderEvent::Morse(MorseText {
                text: String::new(),
                wpm: 0.0,
            }),
        ] {
            let json = serde_json::to_value(&ev).unwrap();
            assert_eq!(json["kind"], ev.kind());
        }
    }

    #[test]
    fn position_is_reported_only_when_both_coordinates_are_known() {
        let mut a = AdsbMessage {
            icao: "3C6444".to_owned(),
            lat: Some(52.5),
            ..AdsbMessage::default()
        };
        assert_eq!(DecoderEvent::Adsb(a.clone()).position(), None);
        a.lon = Some(13.4);
        assert_eq!(DecoderEvent::Adsb(a).position(), Some((52.5, 13.4)));
        assert_eq!(
            DecoderEvent::Rtty(RttyText {
                text: "CQ".to_owned()
            })
            .position(),
            None
        );
    }

    #[test]
    fn decoded_record_roundtrips() {
        let rec = DecodedRecord {
            device_set: 1,
            channel: 2,
            at: "2026-08-09T12:00:00Z".to_owned(),
            freq_hz: 1_090_000_000.0,
            event: DecoderEvent::Adsb(AdsbMessage {
                icao: "3C6444".to_owned(),
                df: 17,
                type_code: Some(4),
                callsign: Some("DLH123".to_owned()),
                raw: "8D3C6444".to_owned(),
                ..AdsbMessage::default()
            }),
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["event"]["kind"], "adsb");
        assert_eq!(json["freq_hz"], 1_090_000_000.0);
        let back: DecodedRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, rec);
    }
}
