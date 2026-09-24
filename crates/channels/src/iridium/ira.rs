use serde_json::{Map, Value, json};

use super::acars::AcarsBlock;
use super::frame::bits_to_u32;

#[derive(Debug, Clone)]
pub struct IridiumFrame {
    pub kind: &'static str,
    pub details: Value,
    pub acars: Option<AcarsBlock>,
    pub offset_hz: Option<f32>,
}

impl IridiumFrame {
    pub fn new(kind: &'static str, details: Value) -> Self {
        Self {
            kind,
            details,
            acars: None,
            offset_hz: None,
        }
    }
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

const IRI_EPOCHS: [(f64, u32); 3] = [
    (1_399_818_235.0, 2),
    (1_739_491_200.0, 0),
    (1_768_414_080.0, 0),
];

const FILLER_INFO: &[u8; 42] = &[
    0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 1,
    0, 0, 1, 1, 1, 1, 0, 0, 0, 0,
];

fn field(bits: &[u8], range: std::ops::Range<usize>) -> u32 {
    bits_to_u32(&bits[range])
}

fn bits_hex(bits: &[u8]) -> String {
    bits.chunks(4)
        .map(|chunk| {
            let nibble = (0..4).fold(0u8, |value, i| {
                (value << 1) | chunk.get(i).copied().unwrap_or(0)
            });
            char::from(HEX_DIGITS[usize::from(nibble)])
        })
        .collect()
}

fn pos_component(bits: &[u8], start: usize) -> i32 {
    field(bits, start + 1..start + 12) as i32 - i32::from(bits[start]) * (1 << 11)
}

pub fn parse_ra(data: &[u8], fixed: u32) -> Option<IridiumFrame> {
    if data.len() < 63 {
        return None;
    }
    let sat = field(data, 0..7);
    let x = pos_component(data, 13);
    let y = pos_component(data, 25);
    let z = pos_component(data, 37);
    if sat == 0 && x == 0 && y == 0 && z == 0 {
        return None;
    }
    let (xf, yf, zf) = (f64::from(x), f64::from(y), f64::from(z));
    let radius_km = (xf * xf + yf * yf + zf * zf).sqrt() * 4.0;
    let (pages, complete) = ra_pages(&data[63..]);
    Some(IridiumFrame::new(
        "ring-alert",
        json!({
            "sat": sat,
            "beam": field(data, 7..13),
            "x": x,
            "y": y,
            "z": z,
            "lat": zf.atan2((xf * xf + yf * yf).sqrt()).to_degrees(),
            "lon": yf.atan2(xf).to_degrees(),
            "alt_km": radius_km - 6378.0 + 23.0,
            "ra_interval": field(data, 49..56),
            "timeslot": data[56],
            "epi": data[57],
            "bc_sub_band": field(data, 58..63),
            "pages": pages,
            "pages_complete": complete,
            "bch_corrected": fixed,
        }),
    ))
}

fn ra_pages(data: &[u8]) -> (Vec<Value>, bool) {
    let mut pages = Vec::new();
    for page in data.as_chunks::<42>().0.iter() {
        if page.iter().all(|&b| b == 1) {
            return (pages, true);
        }
        pages.push(json!({
            "tmsi": format!("{:08x}", field(page, 0..32)),
            "msc_id": field(page, 34..39),
        }));
    }
    (pages, false)
}

fn iri_time_unix_for_base(iritime: u32, base: f64, leaps: u32) -> f64 {
    let mut unix = f64::from(iritime) * 90.0 / 1000.0 + base;
    if leaps >= 1 && unix > 1_435_708_799.0 {
        unix -= 1.0;
    }
    if leaps >= 2 && unix > 1_483_228_799.0 {
        unix -= 1.0;
    }
    unix
}

pub fn iri_time_unix_at(iritime: u32, now_unix: f64) -> f64 {
    let (base, leaps) = IRI_EPOCHS
        .iter()
        .take_while(|(base, _)| *base <= now_unix)
        .last()
        .copied()
        .unwrap_or(IRI_EPOCHS[0]);
    iri_time_unix_for_base(iritime, base, leaps)
}

pub fn iri_time_unix(iritime: u32) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    iri_time_unix_at(iritime, now)
}

pub fn parse_bc(bc_type: u32, data: &[u8], fixed: u32) -> IridiumFrame {
    let mut blocks: Vec<&[u8]> = data
        .as_chunks::<42>()
        .0
        .iter()
        .map(|b| b.as_slice())
        .collect();
    let mut details = Map::new();
    details.insert("bc_type".into(), json!(bc_type));
    if blocks.len() > 4 {
        blocks.truncate(4);
        details.insert("block_trailer".into(), json!("LONG"));
    } else if blocks.len() < 4 {
        details.insert("block_trailer".into(), json!("SHORT"));
    }
    let mut rest = blocks.as_slice();
    if bc_type == 0 {
        if let Some((descriptor, tail)) = rest.split_first() {
            insert_descriptor(&mut details, descriptor);
            rest = tail;
        }
        if let Some((info, tail)) = rest.split_first() {
            insert_info(&mut details, info);
            rest = tail;
        }
    }
    let assignments: Vec<Value> = rest
        .iter()
        .filter(|block| !is_assignment_filler(block))
        .map(|block| assignment(block))
        .collect();
    if !assignments.is_empty() {
        details.insert("assignments".into(), json!(assignments));
    }
    details.insert("bch_corrected".into(), json!(fixed));
    IridiumFrame::new("broadcast", Value::Object(details))
}

fn insert_descriptor(details: &mut Map<String, Value>, b: &[u8]) {
    details.insert("sat".into(), json!(field(b, 0..7)));
    details.insert("beam".into(), json!(field(b, 7..13)));
    details.insert("unknown01".into(), json!(b[13]));
    details.insert("slot".into(), json!(b[14]));
    details.insert("sv_blocking".into(), json!(b[15]));
    details.insert("acq_classes".into(), json!(field(b, 16..32)));
    details.insert("acq_sub_band".into(), json!(field(b, 32..37)));
    details.insert("acq_channels".into(), json!(field(b, 37..40)));
    details.insert("unknown02".into(), json!(field(b, 40..42)));
}

fn insert_info(details: &mut Map<String, Value>, b: &[u8]) {
    let info_type = field(b, 0..6);
    details.insert("info_type".into(), json!(info_type));
    match info_type {
        0 => {
            details.insert("max_uplink_pwr".into(), json!(field(b, 36..42)));
        }
        1 => {
            let time = field(b, 10..42);
            details.insert("iri_time".into(), json!(time));
            details.insert("iri_time_unix".into(), json!(iri_time_unix(time)));
        }
        2 => {
            let expiry = field(b, 10..42);
            details.insert("tmsi_expiry".into(), json!(expiry));
            details.insert("tmsi_expiry_unix".into(), json!(iri_time_unix(expiry)));
        }
        4 if b == &FILLER_INFO[..] => {}
        _ => {
            details.insert("info_raw".into(), json!(bits_hex(b)));
        }
    }
}

fn is_assignment_filler(b: &[u8]) -> bool {
    b[..3].iter().all(|&v| v == 1) && b[3..].iter().all(|&v| v == 0)
}

fn assignment(b: &[u8]) -> Value {
    json!({
        "random_id": field(b, 3..11),
        "timeslot": 1 + field(b, 11..13),
        "uplink_sub_band": field(b, 13..18),
        "downlink_sub_band": field(b, 18..23),
        "access": 1 + field(b, 23..26),
        "dtoa": field(b, 26..34),
        "dfoa": field(b, 34..40),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(s: &str) -> Vec<u8> {
        s.bytes().map(|c| u8::from(c == b'1')).collect()
    }

    const ASG: &str = "111000000010000011101101011101010001000100";
    const DESC: &str = "000110100111100011111111111111111010001000";
    const FILLER_ASG: &str = "111000000000000000000000000000000000000000";

    #[test]
    fn nonzero_bc_type_blocks_are_not_misparsed_as_descriptor() {
        let data = bits(&format!("{ASG}{FILLER_ASG}{FILLER_ASG}{FILLER_ASG}"));
        let d = parse_bc(1, &data, 0).details;
        assert_eq!(d["bc_type"], 1);
        assert!(d.get("sat").is_none());
        assert!(d.get("info_type").is_none());
        let a = d["assignments"].as_array().expect("assignments");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0]["random_id"], 1);
        assert_eq!(a[0]["timeslot"], 1);
        assert_eq!(a[0]["uplink_sub_band"], 3);
        assert_eq!(a[0]["downlink_sub_band"], 22);
        assert_eq!(a[0]["access"], 6);
        assert_eq!(a[0]["dtoa"], 212);
        assert_eq!(a[0]["dfoa"], 17);
    }

    #[test]
    fn descriptor_unknown_bits_are_surfaced() {
        let data = bits(&format!("{DESC}{}", "0".repeat(42)));
        let d = parse_bc(0, &data, 0).details;
        assert_eq!(d["sat"], 13);
        assert_eq!(d["beam"], 15);
        assert_eq!(d["unknown01"], 0);
        assert_eq!(d["unknown02"], 0);
        assert_eq!(d["acq_sub_band"], 20);
        assert_eq!(d["acq_channels"], 2);
    }

    #[test]
    fn info_type_4_known_filler_is_silent() {
        let data = bits(&format!("{DESC}000100000000100001110000110000110011110000"));
        let d = parse_bc(0, &data, 0).details;
        assert_eq!(d["info_type"], 4);
        assert!(d.get("info_raw").is_none());
    }

    #[test]
    fn unrecognized_info_type_surfaces_raw_payload() {
        let data = bits(&format!("{DESC}000011{}", "0".repeat(36)));
        let d = parse_bc(0, &data, 0).details;
        assert_eq!(d["info_type"], 3);
        assert_eq!(d["info_raw"], "0c000000000");
    }

    #[test]
    fn block_count_anomaly_is_flagged() {
        let short = bits(&format!("{DESC}{}", "0".repeat(42)));
        assert_eq!(parse_bc(0, &short, 0).details["block_trailer"], "SHORT");
        let long = bits(&format!("{DESC}{}{ASG}{ASG}{ASG}", "0".repeat(42)));
        assert_eq!(parse_bc(0, &long, 0).details["block_trailer"], "LONG");
    }

    const T_2020: f64 = 1_590_000_000.0;
    const T_2025: f64 = 1_745_000_000.0;
    const T_2026: f64 = 1_775_000_000.0;

    #[test]
    fn era2_matches_toolkit_oracle() {
        assert_eq!(iri_time_unix_at(0, T_2020), 1_399_818_235.0);
        assert_eq!(iri_time_unix_at(1_000_000, T_2020), 1_399_908_235.0);
        assert_eq!(iri_time_unix_at(2_400_000_000, T_2020), 1_615_818_233.0);
        assert_eq!(iri_time_unix_at(3_000_000_000, T_2020), 1_669_818_233.0);
    }

    #[test]
    fn era3_reepoch_does_not_decode_into_the_past() {
        let counter = 60_500_000u32;
        let unix = iri_time_unix_at(counter, T_2025);
        assert_eq!(unix, 1_739_491_200.0 + 60_500_000.0 * 0.09);
        let era2 = iri_time_unix_for_base(counter, 1_399_818_235.0, 2);
        assert!(era2 < 1_420_000_000.0);
        assert!(unix - era2 > 330_000_000.0);
    }

    #[test]
    fn era_selected_by_reference_time() {
        let counter = 300_000_000u32;
        let as_era2 = iri_time_unix_at(counter, T_2020);
        let as_era3 = iri_time_unix_at(counter, T_2025);
        let as_era4 = iri_time_unix_at(counter, T_2026);
        assert_eq!(as_era2, iri_time_unix_for_base(counter, 1_399_818_235.0, 2));
        assert_eq!(as_era3, iri_time_unix_for_base(counter, 1_739_491_200.0, 0));
        assert_eq!(as_era4, iri_time_unix_for_base(counter, 1_768_414_080.0, 0));
    }

    #[test]
    fn epoch_boundary_is_inclusive_of_newer_era() {
        const ERA4_BASE: f64 = 1_768_414_080.0;
        assert_eq!(
            iri_time_unix_at(0, ERA4_BASE - 1.0),
            iri_time_unix_for_base(0, 1_739_491_200.0, 0)
        );
        assert_eq!(
            iri_time_unix_at(0, ERA4_BASE),
            iri_time_unix_for_base(0, ERA4_BASE, 0)
        );
    }

    #[test]
    fn rejects_all_zero_ring_alert() {
        assert!(parse_ra(&[0u8; 96], 0).is_none());
    }
}
