use serde_json::{Map, Value, json};

use super::frame::{
    ACCESS_DL, ACCESS_UL, FrameKind, MESSAGING_BCH_POLY, RINGALERT_BCH_POLY, bits_to_u8,
    bits_to_u32, classify, ecc_blocks, pair_blocks, ra_blocks, strip_fill,
};
use super::ira::{IridiumFrame, parse_bc, parse_ra};
use super::itl::decode_itl;
use super::lcw::{DaFrame, Lcw, decode_da, decode_da_soft, decode_lcw};
use super::ms::{MsBody, PagerReassembler};
use super::sbd::SbdReassembler;
use super::{iip, ms, u3, voice};
use crate::datalink::hex;

const DA_FRAME_TYPE: u8 = 2;
const DA_MAX_LCW_ERRORS: u32 = 6;
const TRAFFIC_MAX_LCW_ERRORS: u32 = 2;
const LCW_BITS: usize = 46;
const SYNC_BYTES: usize = 39;
const SYNC_PATTERN: u8 = 0xAA;

pub struct Reassembly {
    sbd: SbdReassembler,
    pager: PagerReassembler,
}

impl Reassembly {
    pub fn new() -> Self {
        Self {
            sbd: SbdReassembler::new(),
            pager: PagerReassembler::new(),
        }
    }

    pub fn handle(
        &mut self,
        bits: &[u8],
        reliability: &[f32],
        time: f64,
        freq: f64,
        out: &mut Vec<IridiumFrame>,
    ) {
        let payload = if bits.len() > 24 { &bits[24..] } else { bits };
        let ones: usize = payload.iter().map(|&b| usize::from(b)).sum();
        if ones * 10 < payload.len() {
            return;
        }
        if let Some((frame, body)) = decode_simplex(bits) {
            if let Some(full) = body.and_then(|body| self.page(&body, time)) {
                out.push(full);
            }
            out.push(frame);
            return;
        }
        let Some((lcw, data)) = lcw_frame(bits) else {
            return;
        };
        if lcw.frame_type != DA_FRAME_TYPE {
            out.push(traffic_frame(&lcw, &data[LCW_BITS..]));
            return;
        }
        let hard = decode_da(&data[LCW_BITS..]);
        let soft = || {
            let weights = reliability.get(24 + LCW_BITS..)?;
            decode_da_soft(&data[LCW_BITS..], weights)
        };
        let Some(da) = hard.clone().filter(|da| da.crc_ok).or_else(soft).or(hard) else {
            return;
        };
        out.push(ida_frame(&da, &lcw));
        let uplink = bits[..24] == ACCESS_UL[..];
        if let Some(message) = self.sbd.push(&da, time, freq, uplink) {
            out.push(IridiumFrame {
                acars: message.acars,
                ..IridiumFrame::new(message.kind, message.details)
            });
        }
    }

    fn page(&mut self, body: &MsBody, time: f64) -> Option<IridiumFrame> {
        let text = self.pager.push(body, time)?;
        Some(IridiumFrame::new(
            "msg-complete",
            json!({ "ric": body.ric, "text": text }),
        ))
    }
}

pub fn is_valid(bits: &[u8]) -> bool {
    decode_simplex(bits).is_some()
        || lcw_frame(bits).is_some_and(|(lcw, data)| {
            lcw.frame_type != DA_FRAME_TYPE
                || decode_da(&data[LCW_BITS..]).is_some_and(|da| da.crc_ok)
        })
}

fn access_payload(bits: &[u8]) -> Option<&[u8]> {
    let valid = bits.len() > 24 && (bits[..24] == ACCESS_DL[..] || bits[..24] == ACCESS_UL[..]);
    valid.then(|| &bits[24..])
}

#[cfg(test)]
pub fn decode_bits(bits: &[u8]) -> Option<IridiumFrame> {
    decode_simplex(bits).map(|(frame, _)| frame)
}

fn decode_simplex(bits: &[u8]) -> Option<(IridiumFrame, Option<MsBody>)> {
    let data = access_payload(bits)?;
    match classify(data) {
        FrameKind::Ra => {
            let mut blocks = ra_blocks(data);
            strip_fill(&mut blocks);
            let (payload, fixed) = ecc_blocks(&blocks, RINGALERT_BCH_POLY);
            parse_ra(&payload, fixed).map(|frame| (frame, None))
        }
        FrameKind::Bc => {
            let (payload, fixed) = ecc_blocks(&pair_blocks(&data[6..]), RINGALERT_BCH_POLY);
            (!payload.is_empty())
                .then(|| (parse_bc(bits_to_u32(&data[..3]), &payload, fixed), None))
        }
        FrameKind::Ms => decode_messaging(data),
        FrameKind::Itl => {
            let details = decode_itl(&data[96..]).map_or_else(
                || json!({ "type": "time-location", "payload_bits": data.len() - 96 }),
                |frame| frame.to_json(),
            );
            Some((IridiumFrame::new("itl", details), None))
        }
        FrameKind::Lw | FrameKind::Unknown => None,
    }
}

fn decode_messaging(data: &[u8]) -> Option<(IridiumFrame, Option<MsBody>)> {
    let mut blocks = pair_blocks(&data[32..]);
    strip_fill(&mut blocks);
    let (payload, _) = ecc_blocks(&blocks, MESSAGING_BCH_POLY);
    let blocks21: Vec<Vec<u8>> = payload
        .as_chunks::<21>()
        .0
        .iter()
        .map(|block| block.to_vec())
        .collect();
    let frame = ms::parse(&blocks21)?;
    let details = serde_json::to_value(&frame)
        .unwrap_or_else(|error| json!({ "serialization_error": error.to_string() }));
    Some((IridiumFrame::new("msg", details), frame.body))
}

fn lcw_frame(bits: &[u8]) -> Option<(Lcw, &[u8])> {
    let data = access_payload(bits)?;
    let lcw = decode_lcw(data)?;
    let limit = if lcw.frame_type == DA_FRAME_TYPE {
        DA_MAX_LCW_ERRORS
    } else {
        TRAFFIC_MAX_LCW_ERRORS
    };
    (lcw.corrected <= limit).then_some((lcw, data))
}

fn ida_frame(da: &DaFrame, lcw: &Lcw) -> IridiumFrame {
    IridiumFrame::new(
        "ida",
        json!({
            "cont": da.continuation,
            "ctr": da.ctr,
            "len": da.len,
            "crc_ok": da.crc_ok,
            "bch_corrected": da.bch_corrected,
            "lcw": lcw_descriptor(lcw.control, lcw.payload),
            "data_hex": hex(&da.data[..usize::from(da.len).min(20)]),
        }),
    )
}

fn lcw_descriptor(control: u32, payload: u32) -> Value {
    let lcw_ft = (control >> 4) & 0x3;
    let code = control & 0xF;
    let f = |a: usize, b: usize| (payload >> (21 - b)) & ((1u32 << (b - a)) - 1);
    let reserved = |c: u32| json!(format!("rsrvd({c})"));
    let (kind, code): (&str, Value) = match lcw_ft {
        0 => (
            "maint",
            match code {
                6 => json!("geoloc"),
                15 => json!("<silent>"),
                12 => json!({"code": "maint[1]", "lqi": f(19, 21), "power": f(16, 19)}),
                0 => {
                    json!({"code": "sync", "status": f(1, 2), "dtoa": f(3, 13), "dfoa": f(13, 21)})
                }
                3 => json!({
                    "code": "maint[2]", "lqi": f(1, 3), "power": f(3, 6),
                    "f_dtoa": f(6, 13), "f_dfoa": f(13, 20),
                }),
                1 => json!({"code": "switch", "dtoa": f(3, 13), "dfoa": f(13, 21)}),
                c => reserved(c),
            },
        ),
        1 if code == 1 => ("acchl", json!("acchl")),
        1 => ("acchl", reserved(code)),
        2 => (
            "hndof",
            match code {
                12 => json!({"code": "handoff_cand", "cand_a": f(0, 11), "cand_b": f(11, 21)}),
                3 => json!({
                    "code": "handoff_resp",
                    "cand": if f(2, 3) == 1 { "S" } else { "P" },
                    "denied": f(3, 4), "ref": f(4, 5), "slot": 1 + f(6, 8),
                    "sband_up": f(8, 13), "sband_dn": f(13, 18), "access": 1 + f(18, 21),
                }),
                15 => json!("<silent>"),
                c => reserved(c),
            },
        ),
        _ => ("rsrvd", json!(format!("<{code}>"))),
    };
    json!({ "type": kind, "code": code })
}

fn traffic_kind(frame_type: u8) -> &'static str {
    match frame_type {
        0 => "voice",
        1 => "ip-data",
        7 => "sync",
        3 => "u3",
        6 => "u6",
        _ => "lcw",
    }
}

fn merge(details: &mut Map<String, Value>, extra: Option<Value>) {
    if let Some(Value::Object(extra)) = extra {
        details.extend(extra);
    }
}

fn traffic_frame(lcw: &Lcw, payload: &[u8]) -> IridiumFrame {
    let payload_hex: String = payload
        .chunks(8)
        .map(|c| format!("{:02x}", bits_to_u8(c)))
        .collect();
    let mut details = Map::new();
    details.insert("payload_hex".into(), json!(payload_hex));
    details.insert("payload_bits".into(), json!(payload.len()));
    details.insert("lcw".into(), lcw_descriptor(lcw.control, lcw.payload));
    if !matches!(lcw.frame_type, 0 | 1 | 7) {
        details.insert("frame_ft".into(), json!(lcw.frame_type));
    }
    match lcw.frame_type {
        0 => merge(&mut details, voice::classify_voice(payload)),
        1 => merge(&mut details, iip::parse_ip_payload(payload)),
        3 => merge(&mut details, Some(u3::parse_u3(payload))),
        7 => {
            let errors = payload
                .as_chunks::<8>()
                .0
                .iter()
                .take(SYNC_BYTES)
                .filter(|c| bits_to_u8(c.as_slice()) != SYNC_PATTERN)
                .count();
            details.insert("sync_errors".into(), json!(errors));
            details.insert(
                "sync_idle".into(),
                json!(errors == 0 && !payload.is_empty()),
            );
        }
        _ => {}
    }
    IridiumFrame::new(traffic_kind(lcw.frame_type), Value::Object(details))
}
