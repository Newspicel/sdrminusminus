use super::{ac_cache::AcCache, systable::SystableAssembler};
use crate::{
    acars::block::{AcarsBlock, parse as parse_acars},
    datalink::hex,
};
use sdrmm_dsp::crc16_x25;
use serde_json::{Value, json};

const SPDU_LEN: usize = 66;
const SPDU_BODY: usize = 64;
const PERFORMANCE_DATA_LEN: usize = 47;
const FREQUENCY_DATA_HEADER: usize = 15;
const FREQUENCY_RECORD_LEN: usize = 6;
const FREQUENCY_RECORDS: usize = 6;
const SYSTABLE_HEADER: usize = 5;

fn fcs_ok(span: &[u8], trailer: &[u8]) -> bool {
    trailer.len() >= 2 && crc16_x25(span) == u16::from_le_bytes([trailer[0], trailer[1]])
}

pub fn gs_name(id: u8) -> Option<&'static str> {
    Some(match id {
        1 => "San Francisco, USA",
        2 => "Molokai, Hawaii",
        3 => "Reykjavik, Iceland",
        4 => "Riverhead, New York",
        5 => "Auckland, New Zealand",
        6 => "Hat Yai, Thailand",
        7 => "Shannon, Ireland",
        8 => "Johannesburg, South Africa",
        9 => "Barrow, Alaska",
        10 => "Muan, South Korea",
        11 => "Albrook, Panama",
        13 => "Santa Cruz, Bolivia",
        14 => "Krasnoyarsk, Russia",
        15 => "Al Muharraq, Bahrain",
        16 => "Agana, Guam",
        17 => "Canarias, Spain",
        _ => return None,
    })
}

pub fn coordinate(value: u32) -> f64 {
    let signed = ((value << 12) as i32) >> 12;
    f64::from(signed) * 180.0 / f64::from(1u32 << 19)
}

fn freq_change_cause(code: u8) -> &'static str {
    match code {
        0 => "First freq. search in this flight leg",
        1 => "Too many NACKs",
        2 => "SPDUs no longer received",
        3 => "HFDL disabled",
        4 => "GS frequency change",
        5 => "GS down / channel down",
        6 => "Poor uplink channel quality",
        7 => "No change",
        _ => "unknown",
    }
}

fn hfnpdu_type_name(kind: u8) -> Option<&'static str> {
    Some(match kind {
        0xD0 => "System table (partial)",
        0xD1 => "Performance data",
        0xD2 => "System table request",
        0xD5 => "Frequency data",
        0xDE => "Delayed echo",
        0xFF => "Enveloped data",
        _ => return None,
    })
}

fn lpdu_type_name(kind: u8) -> Option<&'static str> {
    Some(match kind {
        0x0D => "Unnumbered data",
        0x1D => "Unnumbered ack'ed data",
        0x2F => "Logon denied",
        0x3F => "Logoff request",
        0x5F => "Logon resume confirm",
        0x4F => "Logon resume",
        0x8F => "Logon request (normal)",
        0x9F => "Logon confirm",
        0xBF => "Logon request (DLS)",
        _ => return None,
    })
}

fn logoff_reason(code: u8) -> &'static str {
    match code {
        0x01 => "Not within slot boundaries",
        0x02 => "Downlink set in uplink slot",
        0x03 => "RLS protocol error",
        0x04 => "Invalid aircraft ID",
        0x05 => "HFDL Ground Station subsystem does not support RLS",
        0x06 => "Other",
        _ => "Reserved",
    }
}

fn logon_denied_reason(code: u8) -> &'static str {
    match code {
        0x01 => "Aircraft ID not available",
        0x02 => "HFDL Ground Station subsystem does not support RLS",
        _ => "Reserved",
    }
}

fn utc_hms(raw: u16) -> (u32, u32, u32) {
    let t = u32::from(raw) * 2;
    (t / 3600, (t % 3600) / 60, t % 60)
}

fn mpdu_stats(b: &[u8]) -> Value {
    json!({ "300bps": b[3], "600bps": b[2], "1200bps": b[1], "1800bps": b[0] })
}

fn icao(b: &[u8]) -> String {
    format!(
        "{:02X}{:02X}{:02X}",
        b[0].reverse_bits(),
        b[1].reverse_bits(),
        b[2].reverse_bits()
    )
}

fn position_obj(fix: &Fix, who: &Value) -> Option<Value> {
    if fix.lat == 0.0 && fix.lon == 0.0 {
        return None;
    }
    let mut position = json!({
        "lat": fix.lat,
        "lon": fix.lon,
        "utc_s": fix.utc_s,
        "utc": fix.utc.clone(),
        "flight": fix.flight,
    });
    if let Some(id) = who.get("aircraft_id") {
        position["aircraft_id"] = id.clone();
    }
    if let Some(icao) = who.get("icao") {
        position["icao"] = icao.clone();
    }
    Some(position)
}

struct Fix {
    flight: String,
    lat: f64,
    lon: f64,
    utc_s: u32,
    utc: Value,
}

fn fix(h: &[u8]) -> Fix {
    let flight: String = h[2..8].iter().map(|&c| char::from(c & 0x7F)).collect();
    let lat = coordinate(u32::from(h[8]) | u32::from(h[9]) << 8 | (u32::from(h[10]) & 0x0F) << 16);
    let lon = coordinate(u32::from(h[10]) >> 4 | u32::from(h[11]) << 4 | u32::from(h[12]) << 12);
    let raw = u16::from_le_bytes([h[13], h[14]]);
    let (hour, min, sec) = utc_hms(raw);
    Fix {
        flight: flight.trim().to_owned(),
        lat,
        lon,
        utc_s: u32::from(raw) * 2,
        utc: json!({ "hour": hour, "min": min, "sec": sec }),
    }
}

fn with_position(mut details: Value, fix: &Fix, who: &Value) -> Value {
    if let Some(position) = position_obj(fix, who) {
        details["position"] = position;
    }
    details
}

#[derive(Debug, Clone, PartialEq)]
pub struct HfdlEvent {
    pub kind: String,
    pub details: Value,
    pub acars: Option<AcarsBlock>,
    pub fec_corrected: Option<u32>,
    pub freq_skew_hz: Option<f32>,
    pub snr_db: Option<f32>,
    pub raw: Vec<u8>,
}

fn event(kind: &str, details: Value, raw: &[u8]) -> HfdlEvent {
    HfdlEvent {
        kind: kind.to_owned(),
        details,
        acars: None,
        fec_corrected: None,
        freq_skew_hz: None,
        snr_db: None,
        raw: raw.to_vec(),
    }
}

struct MpduHeader {
    len: usize,
    lpdu_sizes: Vec<usize>,
    who: Value,
}

fn downlink_header(p: &[u8]) -> Option<MpduHeader> {
    let count = usize::from((p[0] >> 2) & 0x0F);
    let len = 6 + count;
    if p.len() < len + 2 {
        return None;
    }
    Some(MpduHeader {
        len,
        lpdu_sizes: p[6..len]
            .iter()
            .map(|&size| usize::from(size) + 1)
            .collect(),
        who: json!({
            "dir": "downlink",
            "gs_id": p[1] & 0x7F,
            "gs_name": gs_name(p[1] & 0x7F),
            "aircraft_id": p[2],
        }),
    })
}

fn uplink_header(p: &[u8]) -> Option<MpduHeader> {
    let aircraft_count = usize::from((p[0] >> 4) & 0x7) + 1;
    let mut lpdu_sizes = Vec::new();
    let mut index = 2;
    let mut aircraft = Vec::new();
    for _ in 0..aircraft_count {
        if index + 2 > p.len() {
            return None;
        }
        let ac_id = p[index];
        let count = usize::from(p[index + 1] >> 4);
        index += 2;
        let sizes = p.get(index..index + count)?;
        lpdu_sizes.extend(sizes.iter().map(|&size| usize::from(size) + 1));
        index += count;
        aircraft.push(json!({ "aircraft_id": ac_id, "lpdus": count }));
    }
    Some(MpduHeader {
        len: index,
        lpdu_sizes,
        who: json!({
            "dir": "uplink",
            "gs_id": p[1] & 0x7F,
            "gs_name": gs_name(p[1] & 0x7F),
            "aircraft": aircraft,
        }),
    })
}

pub struct PduParser {
    systable: SystableAssembler,
    ac_cache: AcCache,
}

impl PduParser {
    pub fn new() -> Self {
        Self {
            systable: SystableAssembler::default(),
            ac_cache: AcCache::new(),
        }
    }

    #[cfg(test)]
    pub fn with_ac_cache_ttl(ttl: std::time::Duration) -> Self {
        Self {
            systable: SystableAssembler::default(),
            ac_cache: AcCache::with_ttl(ttl),
        }
    }

    #[cfg(test)]
    pub fn resolve_icao(&self, ac_id: u8) -> Option<&str> {
        self.ac_cache.lookup(ac_id)
    }

    pub fn parse(&mut self, payload: &[u8], bps: u32, out: &mut Vec<HfdlEvent>) {
        let Some(&first) = payload.first() else {
            return;
        };
        if first & 1 == 0 {
            parse_spdu(payload, out);
        } else {
            self.parse_mpdu(payload, bps, out);
        }
    }

    fn parse_mpdu(&mut self, p: &[u8], bps: u32, out: &mut Vec<HfdlEvent>) {
        let header = if p[0] & 2 != 0 {
            downlink_header(p)
        } else {
            uplink_header(p)
        };
        let Some(header) = header else {
            return;
        };
        if p.len() < header.len + 2 || !fcs_ok(&p[..header.len], &p[header.len..]) {
            return;
        }
        let mut offset = header.len + 2;
        for size in header.lpdu_sizes {
            let Some(lpdu) = p.get(offset..offset + size) else {
                break;
            };
            self.parse_lpdu(lpdu, &header.who, bps, out);
            offset += size;
        }
    }

    fn resolved(&self, who: &Value) -> Value {
        if who.get("dir").and_then(Value::as_str) == Some("downlink")
            && let Some(ac_id) = who.get("aircraft_id").and_then(Value::as_u64)
            && let Some(found) = u8::try_from(ac_id)
                .ok()
                .and_then(|id| self.ac_cache.lookup(id))
        {
            let mut resolved = who.clone();
            resolved["icao"] = Value::String(found.to_owned());
            return resolved;
        }
        who.clone()
    }

    fn parse_lpdu(&mut self, l: &[u8], who: &Value, bps: u32, out: &mut Vec<HfdlEvent>) {
        if l.len() < 3 || !fcs_ok(&l[..l.len() - 2], &l[l.len() - 2..]) {
            return;
        }
        let body = &l[..l.len() - 2];
        match body[0] {
            0x0D | 0x1D => {
                let who = self.resolved(who);
                self.parse_hfnpdu(&body[1..], &who, bps, out);
            }
            0x8F | 0xBF if body.len() >= 4 => out.push(event(
                "logon-request",
                json!({ "icao": icao(&body[1..4]), "who": who }),
                l,
            )),
            0x4F if body.len() >= 4 => out.push(event(
                "logon-resume",
                json!({ "icao": icao(&body[1..4]), "who": who }),
                l,
            )),
            0x9F | 0x5F if body.len() >= 5 => {
                let icao = icao(&body[1..4]);
                self.ac_cache.insert(body[4], &icao);
                out.push(event(
                    "logon-confirm",
                    json!({ "icao": icao, "assigned_id": body[4], "who": who }),
                    l,
                ));
            }
            0x3F if body.len() >= 5 => {
                out.push(self.release("logoff-request", body, logoff_reason, who, l));
            }
            0x2F if body.len() >= 5 => {
                out.push(self.release("logon-denied", body, logon_denied_reason, who, l));
            }
            kind => out.push(event(
                "lpdu",
                json!({ "type": kind, "type_name": lpdu_type_name(kind), "who": who }),
                l,
            )),
        }
    }

    fn release(
        &mut self,
        kind: &str,
        body: &[u8],
        reason: fn(u8) -> &'static str,
        who: &Value,
        raw: &[u8],
    ) -> HfdlEvent {
        let icao = icao(&body[1..4]);
        self.ac_cache.remove_by_icao(&icao);
        event(
            kind,
            json!({
                "icao": icao,
                "reason": body[4],
                "reason_text": reason(body[4]),
                "who": who,
            }),
            raw,
        )
    }

    fn parse_hfnpdu(&mut self, h: &[u8], who: &Value, bps: u32, out: &mut Vec<HfdlEvent>) {
        if h.len() < 2 || h[0] != 0xFF {
            out.push(envelope(h, who));
            return;
        }
        match h[1] {
            0xFF => out.push(enveloped_acars(h, who, bps)),
            0xD1 if h.len() >= PERFORMANCE_DATA_LEN => out.push(performance_data(h, who)),
            0xD5 if h.len() >= FREQUENCY_DATA_HEADER => out.push(frequency_data(h, who)),
            0xD0 if h.len() >= SYSTABLE_HEADER => self.systable_partial(h, out),
            0xD2 if h.len() >= 4 => out.push(event(
                "systable-request",
                json!({
                    "request_data": u16::from_le_bytes([h[2], h[3]]),
                    "who": who,
                }),
                h,
            )),
            0xDE => out.push(event("delayed-echo", json!({ "who": who }), h)),
            kind => out.push(event(
                "hfnpdu",
                json!({
                    "type": kind,
                    "type_name": hfnpdu_type_name(kind),
                    "who": who,
                }),
                h,
            )),
        }
    }

    fn systable_partial(&mut self, h: &[u8], out: &mut Vec<HfdlEvent>) {
        let (seq, total) = (h[2] & 0x0F, (h[2] >> 4) + 1);
        let version = (u16::from(h[3]) >> 4) | (u16::from(h[4]) << 4);
        out.push(event(
            "systable-partial",
            json!({ "seq": seq, "total": total, "version": version }),
            h,
        ));
        if let Some(table) = self
            .systable
            .store(version, seq, total, &h[SYSTABLE_HEADER..])
        {
            let details = serde_json::to_value(&table)
                .unwrap_or_else(|error| json!({ "error": error.to_string() }));
            out.push(event("systable-complete", details, &[]));
        }
    }
}

fn parse_spdu(p: &[u8], out: &mut Vec<HfdlEvent>) {
    if p.len() < SPDU_LEN || !fcs_ok(&p[..SPDU_BODY], &p[SPDU_BODY..SPDU_LEN]) {
        return;
    }
    let freq_bitmap =
        |a: u8, b: u8, c: u8| -> u32 { u32::from(a >> 4) | u32::from(b) << 4 | u32::from(c) << 12 };
    out.push(event(
        "squitter",
        json!({
            "gs_id": p[1] & 0x7F,
            "gs_name": gs_name(p[1] & 0x7F),
            "utc_sync": p[1] >> 7 == 1,
            "frame_index": u16::from(p[2]) | ((u16::from(p[3]) & 0x0F) << 8),
            "frame_offset": p[3] >> 4,
            "spdu_version": (p[0] >> 2) & 0x3,
            "rls_in_use": p[0] & 0x02 != 0,
            "iso8208_supported": p[0] & 0x20 != 0,
            "change_note": (p[0] >> 6) & 0x3,
            "slot_assignment_hex": hex(&p[4..52]),
            "min_priority": p[52] & 0x0F,
            "systable_version": u16::from(p[53]) | ((u16::from(p[54]) & 0x0F) << 8),
            "freqs_in_use": freq_bitmap(p[54], p[55], p[56]),
            "neighbor2": {
                "gs_id": p[57] & 0x7F,
                "freqs": freq_bitmap(p[58].rotate_right(4), p[59], p[60]),
            },
            "neighbor3_gs_id": u16::from(p[60] >> 4) | ((u16::from(p[61]) & 0x7) << 4),
        }),
        &p[..SPDU_LEN],
    ));
}

fn envelope(h: &[u8], who: &Value) -> HfdlEvent {
    event(
        "unnumbered-data",
        json!({ "who": who, "data_hex": hex(h) }),
        h,
    )
}

fn enveloped_acars(h: &[u8], who: &Value, bps: u32) -> HfdlEvent {
    match parse_acars(&h[2..]) {
        Some(block) => HfdlEvent {
            acars: Some(block),
            ..event("acars", json!({ "who": who, "bps": bps }), h)
        },
        None => envelope(h, who),
    }
}

fn performance_data(h: &[u8], who: &Value) -> HfdlEvent {
    let fix = fix(h);
    let freq_change_code = h[46] & 0x0F;
    let details = json!({
        "flight": fix.flight,
        "lat": fix.lat, "lon": fix.lon,
        "utc_s": fix.utc_s,
        "utc": fix.utc.clone(),
        "version": h[15],
        "flight_leg": h[16],
        "gs_id": h[17] & 0x7F,
        "gs_name": gs_name(h[17] & 0x7F),
        "freq_id": h[18],
        "freq_search_cnt": {
            "prev_leg": u16::from_le_bytes([h[19], h[20]]),
            "cur_leg": u16::from_le_bytes([h[21], h[22]]),
        },
        "hfdl_disabled_duration": {
            "prev_leg": u16::from_le_bytes([h[23], h[24]]),
            "cur_leg": u16::from_le_bytes([h[25], h[26]]),
        },
        "mpdus_rx": mpdu_stats(&h[27..31]),
        "mpdus_rx_errs": mpdu_stats(&h[31..35]),
        "spdus_rx": u16::from_le_bytes([h[35], h[36]]),
        "spdus_rx_errs": h[37],
        "mpdus_tx": mpdu_stats(&h[38..42]),
        "mpdus_delivered": mpdu_stats(&h[42..46]),
        "freq_change_code": freq_change_code,
        "freq_change_cause": freq_change_cause(freq_change_code),
        "who": who,
    });
    event("performance-data", with_position(details, &fix, who), h)
}

fn frequency_data(h: &[u8], who: &Value) -> HfdlEvent {
    let fix = fix(h);
    let freq_data: Vec<Value> = h[FREQUENCY_DATA_HEADER..]
        .as_chunks::<FREQUENCY_RECORD_LEN>()
        .0
        .iter()
        .take(FREQUENCY_RECORDS)
        .map(|r| {
            let prop_freqs =
                u32::from(r[1]) | u32::from(r[2]) << 8 | (u32::from(r[3]) & 0x0F) << 16;
            let tuned_freqs = u32::from(r[3]) >> 4 | u32::from(r[4]) << 4 | u32::from(r[5]) << 12;
            json!({
                "gs_id": r[0] & 0x7F,
                "gs_name": gs_name(r[0] & 0x7F),
                "prop_freqs": prop_freqs,
                "tuned_freqs": tuned_freqs,
            })
        })
        .collect();
    let details = json!({
        "flight": fix.flight,
        "lat": fix.lat, "lon": fix.lon,
        "utc_s": fix.utc_s,
        "utc": fix.utc.clone(),
        "freq_data": freq_data,
        "who": who,
    });
    event("frequency-data", with_position(details, &fix, who), h)
}

#[cfg(test)]
pub mod build;

#[cfg(test)]
mod tests;
