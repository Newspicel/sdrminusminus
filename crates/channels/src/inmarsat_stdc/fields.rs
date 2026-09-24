use std::fmt::Write;

use serde_json::{Value, json};

pub const PRIORITY: [&str; 4] = ["routine", "safety", "urgency", "distress"];

fn fletcher<'a>(bytes: impl IntoIterator<Item = &'a u8>) -> (u8, u8) {
    let (sum, weighted) = bytes
        .into_iter()
        .fold((0u8, 0u8), |(sum, weighted), &byte| {
            let sum = sum.wrapping_add(byte);
            (sum, weighted.wrapping_add(sum))
        });
    (
        sum.wrapping_sub(weighted),
        weighted.wrapping_sub(sum.wrapping_mul(2)),
    )
}

#[cfg(test)]
pub fn checksum(packet_with_zeroed_checksum: &[u8]) -> (u8, u8) {
    fletcher(packet_with_zeroed_checksum)
}

pub fn checksum_ok(packet: &[u8], zero_allowed: bool) -> bool {
    match packet.split_last_chunk::<2>() {
        Some((body, &[first, second])) if !body.is_empty() => {
            (zero_allowed && (first, second) == (0, 0))
                || fletcher(body.iter().chain(&[0, 0])) == (first, second)
        }
        _ => false,
    }
}

pub fn uplink_mhz(channel_word: u16) -> f64 {
    (f64::from(channel_word) - 6000.0) * 0.0025 + 1626.5
}

pub fn downlink_mhz(channel_word: u16) -> f64 {
    (f64::from(channel_word) - 8000.0) * 0.0025 + 1530.5
}

pub fn round4(mhz: f64) -> f64 {
    (mhz * 1e4).round() / 1e4
}

pub fn ocean_region_long(region: u8) -> &'static str {
    match region {
        0 => "Atlantic Ocean Region West (AOR-W)",
        1 => "Atlantic Ocean Region East (AOR-E)",
        2 => "Pacific Ocean Region (POR)",
        3 => "Indian Ocean Region (IOR)",
        _ => "Unknown",
    }
}

pub fn les_name(les_code: u16) -> Option<&'static str> {
    Some(match les_code {
        1 | 101 | 201 | 301 => "Vizada-Telenor, USA",
        2 | 102 | 302 => "Stratos Global (Burum-2), Netherlands",
        3 | 103 | 203 | 303 => "KDDI Japan",
        4 | 104 | 204 | 304 => "Vizada-Telenor, Norway",
        12 | 112 | 212 | 312 => "Stratos Global (Burum), Netherlands",
        21 | 121 | 221 | 321 => "Vizada (FT), France",
        44 | 144 | 244 | 344 => "NCS",
        105 | 335 => "Telecom, Italia",
        110 | 310 => "Turk Telecom, Turkey",
        114 => "Embratel, Brazil",
        116 | 316 => "Telekomunikacja Polska, Poland",
        117 | 217 | 317 => "Morsviazsputnik, Russia",
        120 | 305 => "OTESTAT, Greece",
        127 | 327 => "Bezeq, Israel",
        202 => "Stratos Global (Aukland), New Zealand",
        210 | 328 => "Singapore Telecom, Singapore",
        211 | 311 => "Beijing MCN, China",
        306 => "VSNL, India",
        330 => "VISHIPEL, Vietnam",
        _ => return None,
    })
}

pub fn sat_les(byte: u8) -> Value {
    const REGIONS: [&str; 4] = ["AOR-W", "AOR-E", "POR", "IOR"];
    let region = (byte >> 6) & 0x3;
    let les_code = u16::from(region) * 100 + u16::from(byte & 0x3F);
    json!({
        "region": REGIONS[usize::from(region)],
        "region_long": ocean_region_long(region),
        "les": les_code,
        "les_name": les_name(les_code),
    })
}

pub fn services_short(services: u8) -> Vec<&'static str> {
    const NAMES: [&str; 8] = [
        "MaritimeDistressAlerting",
        "SafetyNet",
        "InmarsatC",
        "StoreFwd",
        "HalfDuplex",
        "FullDuplex",
        "ClosedNetwork",
        "FleetNet",
    ];
    (0..8)
        .filter(|bit| services & (0x80 >> bit) != 0)
        .map(|bit| NAMES[bit])
        .collect()
}

pub fn services_full(services: u16) -> Vec<&'static str> {
    const NAMES: [&str; 16] = [
        "MaritimeDistressAlerting",
        "SafetyNet",
        "InmarsatC",
        "StoreFwd",
        "HalfDuplex",
        "FullDuplex",
        "ClosedNetwork",
        "FleetNet",
        "PrefixSF",
        "LandMobileAlerting",
        "AeroC",
        "ITA2",
        "DATA",
        "BasicX400",
        "EnhancedX400",
        "LowPowerCMES",
    ];
    (0..16)
        .filter(|bit| services & (0x8000 >> bit) != 0)
        .map(|bit| NAMES[bit])
        .collect()
}

pub fn channel_type_name(channel_type: u8) -> &'static str {
    match channel_type {
        1 => "NCS",
        2 => "LES TDM",
        3 => "Joint NCS and TDM",
        4 => "ST-BY NCS",
        _ => "Reserved",
    }
}

pub fn bulletin_status(status: u8) -> Value {
    json!({
        "bauds_600": status & 0x80 != 0,
        "operational": status & 0x40 != 0,
        "in_service": status & 0x20 != 0,
        "clear": status & 0x10 != 0,
        "links_open": status & 0x08 != 0,
    })
}

pub fn parse_stations(records: &[u8], count: usize) -> Vec<Value> {
    records
        .as_chunks::<6>()
        .0
        .iter()
        .take(count)
        .map(|record| {
            json!({
                "sat_les": sat_les(record[0]),
                "services_start": record[1],
                "services": services_full(u16::from_be_bytes([record[2], record[3]])),
                "downlink_mhz": round4(downlink_mhz(u16::from_be_bytes([record[4], record[5]]))),
            })
        })
        .collect()
}

pub fn tdm_slots(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .take(7)
        .flat_map(|&byte| {
            [
                (byte >> 6) & 0x3,
                (byte >> 4) & 0x3,
                (byte >> 2) & 0x3,
                byte & 0x3,
            ]
        })
        .collect()
}

pub fn ia5(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| {
            let c = char::from(byte & 0x7F);
            if c.is_ascii_graphic() || matches!(c, ' ' | '\n' | '\r') {
                c
            } else {
                '·'
            }
        })
        .collect()
}

pub fn hex_upper(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02X}");
    }
    out
}

pub fn mes_id(bytes: &[u8]) -> String {
    hex_upper(bytes)
}

const ITA2_LTRS: [&str; 32] = [
    "\0", "E", "\n", "A", " ", "S", "I", "U", "\r", "D", "R", "J", "N", "F", "C", "K", "T", "Z",
    "L", "W", "H", "Y", "P", "Q", "O", "B", "G", "", "M", "X", "V", "",
];
const ITA2_FIGS: [&str; 32] = [
    "\0", "3", "\n", "-", " ", "'", "8", "7", "\r", "\u{5}", "4", "\u{7}", ",", "!", ":", "(", "5",
    "+", ")", "2", "\u{a3}", "6", "0", "1", "9", "?", "&", "", ".", "/", ";", "",
];
const ITA2_LETTERS_SHIFT: usize = 0x1F;
const ITA2_FIGURES_SHIFT: usize = 0x1B;

pub fn ita2_decode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut figures = false;
    for &byte in bytes {
        match usize::from(byte & 0x1F) {
            ITA2_LETTERS_SHIFT => figures = false,
            ITA2_FIGURES_SHIFT => figures = true,
            code if figures => out.push_str(ITA2_FIGS[code]),
            code => out.push_str(ITA2_LTRS[code]),
        }
    }
    out
}

pub fn egc_address_len(service: u8) -> usize {
    match service {
        0x02 | 0x72 => 5,
        0x04 | 0x14 | 0x24 | 0x34 | 0x44 => 7,
        0x11 | 0x31 => 4,
        0x13 | 0x23 | 0x33 | 0x73 => 6,
        _ => 3,
    }
}

pub fn egc_service_name(service: u8) -> &'static str {
    match service {
        0x00 => "system/all-ships",
        0x02 => "fleetnet/group-call",
        0x04 => "safetynet/warning-rect",
        0x11 => "system/inmarsat",
        0x13 => "safetynet/coastal-warning",
        0x14 => "safetynet/distress-circ",
        0x23 => "system/egc",
        0x24 => "safetynet/warning-circ",
        0x31 => "safetynet/navarea-warning",
        0x33 => "system/download-group-id",
        0x34 => "safetynet/sar-rect",
        0x44 => "safetynet/sar-circ",
        0x72 => "fleetnet/chart-correction",
        0x73 => "safetynet/chart-correction",
        _ => "unknown",
    }
}

pub fn egc_service_long_name(service: u8) -> Option<&'static str> {
    Some(match service {
        0x00 => "System, All ships (general call)",
        0x02 => "FleetNET, Group Call",
        0x04 => "SafetyNET, Navigational, Meteorological or Piracy Warning to a Rectangular Area",
        0x11 => "System, Inmarsat System Message",
        0x13 => "SafetyNET, Navigational, Meteorological or Piracy Coastal Warning",
        0x14 => "SafetyNET, Shore-to-Ship Distress Alert to Circular Area",
        0x23 => "System, EGC System Message",
        0x24 => "SafetyNET, Navigational, Meteorological or Piracy Warning to a Circular Area",
        0x31 => {
            "SafetyNET, NAVAREA/METAREA Warning, MET Forecast or Piracy Warning to NAVAREA/METAREA"
        }
        0x33 => "System, Download Group Identity",
        0x34 => "SafetyNET, SAR Coordination to a Rectangular Area",
        0x44 => "SafetyNET, SAR Coordination to a Circular Area",
        0x72 => "FleetNET, Chart Correction Service",
        0x73 => "SafetyNET, Chart Correction Service for Fixed Areas",
        _ => return None,
    })
}

pub fn frame_to_utc_hms(frame_number: u16) -> Option<String> {
    if frame_number > 9999 {
        return None;
    }
    let seconds = (f64::from(frame_number) * 8.64).floor() as u32;
    Some(format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaShape {
    Rectangular,
    Circular,
    NavMetArea,
    Coastal,
    AllShips,
}

impl AreaShape {
    fn name(self) -> &'static str {
        match self {
            Self::Rectangular => "rectangular",
            Self::Circular => "circular",
            Self::NavMetArea => "navarea-metarea",
            Self::Coastal => "coastal",
            Self::AllShips => "all-ships",
        }
    }
}

pub fn area_shape(service: u8) -> Option<(AreaShape, &'static str)> {
    Some(match service {
        0x04 | 0x34 => (AreaShape::Rectangular, "D1D2La D3D4D5Lo D6D7 D8D9D10"),
        0x14 | 0x24 | 0x44 => (AreaShape::Circular, "D1D2La D3D4D5Lo R1R2R3"),
        0x31 => (AreaShape::NavMetArea, "X1X2"),
        0x13 | 0x73 => (AreaShape::Coastal, "X1X2 B1 B2"),
        0x00 => (AreaShape::AllShips, "00"),
        _ => return None,
    })
}

pub fn nav_met_area_coordinator(area: u8) -> Option<&'static str> {
    Some(match area {
        1 => "United Kingdom",
        2 => "France",
        3 => "Spain",
        4 => "United States of America (East)",
        5 => "Brazil",
        6 => "Argentina",
        7 => "South Africa",
        8 => "India",
        9 => "Pakistan",
        10 => "Australia",
        11 => "Japan",
        12 => "United States of America (West)",
        13 | 20 | 21 => "Russian Federation",
        14 => "New Zealand",
        15 => "Chile",
        16 => "Peru",
        17 | 18 => "Canada",
        19 => "Norway",
        _ => return None,
    })
}

pub fn area_roman(number: u8) -> String {
    const TENS: [&str; 4] = ["", "X", "XX", "XXX"];
    const ONES: [&str; 10] = ["", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX"];
    if number == 0 || number >= 40 {
        return number.to_string();
    }
    format!(
        "{}{}",
        TENS[usize::from(number / 10)],
        ONES[usize::from(number % 10)]
    )
}

pub fn egc_area(service: u8, address: &[u8]) -> Option<Value> {
    let (shape, c3_format) = area_shape(service)?;
    let payload = address.get(1..).unwrap_or(&[]);
    let geometry = match shape {
        AreaShape::Rectangular => rectangular_geometry(payload),
        AreaShape::Circular => circular_geometry(payload),
        AreaShape::NavMetArea | AreaShape::Coastal => nav_met_geometry(shape, payload),
        AreaShape::AllShips => None,
    };
    let mut area = json!({
        "shape": shape.name(),
        "c2": service,
        "c3_format": c3_format,
        "address_payload_hex": hex_upper(payload),
    });
    if let (Some(geometry), Some(object)) = (geometry, area.as_object_mut()) {
        object.insert("geometry".to_owned(), geometry);
    }
    Some(area)
}

fn signed(magnitude: i32, negative: bool) -> i32 {
    if negative { -magnitude } else { magnitude }
}

fn hemispheres(lat_byte: u8, lon_byte: u8) -> (&'static str, &'static str) {
    (
        if lat_byte & 0x80 != 0 { "S" } else { "N" },
        if lon_byte & 0x80 != 0 { "W" } else { "E" },
    )
}

pub fn rectangular_geometry(payload: &[u8]) -> Option<Value> {
    let &[lat, lon, north, east] = payload.first_chunk::<4>()?;
    let (lat_hemisphere, lon_hemisphere) = hemispheres(lat, north);
    Some(json!({
        "sw_corner": {
            "lat_deg": signed(i32::from(lat & 0x7F), lat & 0x80 != 0),
            "lon_deg": signed(i32::from(lon), north & 0x80 != 0),
        },
        "north_extent_nm": i32::from(north & 0x7F),
        "east_extent_nm": i32::from(east),
        "lat_hemisphere": lat_hemisphere,
        "lon_hemisphere": lon_hemisphere,
    }))
}

pub fn circular_geometry(payload: &[u8]) -> Option<Value> {
    let &[lat, lon, radius_high, radius_low] = payload.first_chunk::<4>()?;
    let (lat_hemisphere, lon_hemisphere) = hemispheres(lat, radius_high);
    Some(json!({
        "center": {
            "lat_deg": signed(i32::from(lat & 0x7F), lat & 0x80 != 0),
            "lon_deg": signed(i32::from(lon), radius_high & 0x80 != 0),
        },
        "radius_nm": (i32::from(radius_high & 0x7F) << 8) | i32::from(radius_low),
        "lat_hemisphere": lat_hemisphere,
        "lon_hemisphere": lon_hemisphere,
    }))
}

fn coastal_subject(indicator: char) -> Option<&'static str> {
    match indicator {
        'A' => Some("navigational-warnings"),
        'L' => Some("other-navigational-warnings"),
        'B' => Some("meteorological-warnings"),
        'E' => Some("meteorological-forecasts"),
        _ => None,
    }
}

pub fn nav_met_geometry(shape: AreaShape, payload: &[u8]) -> Option<Value> {
    let number = *payload.first()?;
    let mut geometry = json!({
        "area_number": number,
        "area_roman": area_roman(number),
        "coordinator": nav_met_area_coordinator(number),
    });
    let Some(object) = geometry.as_object_mut() else {
        return Some(geometry);
    };
    if shape == AreaShape::Coastal {
        if let Some(letter) = payload.get(1).map(|byte| byte & 0x7F)
            && letter.is_ascii_uppercase()
        {
            object.insert(
                "coastal_area".to_owned(),
                json!(char::from(letter).to_string()),
            );
        }
        if let Some(indicator) = payload.get(2).map(|byte| char::from(byte & 0x7F))
            && let Some(subject) = coastal_subject(indicator)
        {
            object.insert("subject_indicator".to_owned(), json!(indicator.to_string()));
            object.insert("subject".to_owned(), json!(subject));
        }
    }
    Some(geometry)
}

pub fn looks_textual(payload: &[u8]) -> bool {
    if payload.is_empty() {
        return false;
    }
    let printable = payload
        .iter()
        .filter(|&&byte| {
            let c = byte & 0x7F;
            (0x20..0x7F).contains(&c) || matches!(c, b'\r' | b'\n' | 0x07)
        })
        .count();
    printable * 100 >= payload.len() * 85
}

pub fn decode_payload(presentation: u8, payload: &[u8]) -> (Option<String>, Value) {
    match presentation {
        0 => (Some(ia5(payload)), json!({})),
        6 => (Some(ita2_decode(payload)), json!({ "encoding": "ita2" })),
        _ if looks_textual(payload) => (
            Some(ia5(payload)),
            json!({ "presentation_heuristic": true }),
        ),
        _ => (None, json!({ "payload_hex": hex_upper(payload) })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_the_zeroed_packet_form() {
        let mut packet = vec![0xAA, 0x09, 0x10, 0x02, 0x01, b'H', b'I', 0, 0];
        let (first, second) = checksum(&packet);
        packet[7] = first;
        packet[8] = second;
        assert!(checksum_ok(&packet, false));
        packet[4] ^= 1;
        assert!(!checksum_ok(&packet, false));
        assert!(!checksum_ok(&[1, 2], true));
        let unsummed = [0x27, 1, 2, 3, 4, 5, 0, 0];
        assert!(checksum_ok(&unsummed, true));
        assert!(!checksum_ok(&unsummed, false));
    }

    #[test]
    fn channel_type_names_match_inmarsatc() {
        assert_eq!(channel_type_name(1), "NCS");
        assert_eq!(channel_type_name(2), "LES TDM");
        assert_eq!(channel_type_name(3), "Joint NCS and TDM");
        assert_eq!(channel_type_name(4), "ST-BY NCS");
        assert_eq!(channel_type_name(0), "Reserved");
    }

    #[test]
    fn bulletin_status_flags_match_inmarsatc() {
        let status = bulletin_status(0xE8);
        assert_eq!(status["bauds_600"], true);
        assert_eq!(status["operational"], true);
        assert_eq!(status["in_service"], true);
        assert_eq!(status["clear"], false);
        assert_eq!(status["links_open"], true);
    }

    #[test]
    fn parse_stations_decodes_record_layout() {
        let records = [0x44, 0x01, 0x40, 0x20, 0x20, 0xD0];
        let stations = parse_stations(&records, 1);
        assert_eq!(stations[0]["sat_les"]["region"], "AOR-E");
        assert_eq!(stations[0]["sat_les"]["les"], 104);
        assert_eq!(stations[0]["services"], json!(["SafetyNet", "AeroC"]));
        assert_eq!(stations[0]["downlink_mhz"], 1531.5);
        assert!(parse_stations(&records[..4], 2).is_empty());
    }

    #[test]
    fn frame_to_utc_matches_offair_oracle() {
        assert_eq!(frame_to_utc_hms(5987).as_deref(), Some("14:22:07"));
        assert_eq!(frame_to_utc_hms(0).as_deref(), Some("00:00:00"));
        assert_eq!(frame_to_utc_hms(9999).as_deref(), Some("23:59:51"));
        assert_eq!(frame_to_utc_hms(10000), None);
        assert_eq!(frame_to_utc_hms(417).as_deref(), Some("01:00:02"));
    }

    #[test]
    fn channel_frequency_formula_matches_oracle() {
        assert!((uplink_mhz(6000) - 1626.5).abs() < 1e-9);
        assert!((downlink_mhz(8000) - 1530.5).abs() < 1e-9);
        assert!((uplink_mhz(0x2748) - 1636.64).abs() < 1e-6);
    }

    #[test]
    fn services_bits_match_inmarsatc() {
        assert_eq!(
            services_short(0xB4),
            vec![
                "MaritimeDistressAlerting",
                "InmarsatC",
                "StoreFwd",
                "FullDuplex"
            ]
        );
        assert_eq!(services_full(0x4020), vec!["SafetyNet", "AeroC"]);
        assert_eq!(services_full(0x0001), vec!["LowPowerCMES"]);
        assert_eq!(services_full(0xB400)[..], services_short(0xB4)[..]);
        assert!(services_full(0).is_empty());
    }

    #[test]
    fn tdm_slots_unpacks_two_bit_codes() {
        let slots = tdm_slots(&[0x02, 0x00, 0x08, 0x00, 0x08, 0x00, 0x02]);
        assert_eq!(slots.len(), 28);
        assert_eq!(&slots[0..4], &[0, 0, 0, 2]);
        assert_eq!(&slots[8..12], &[0, 0, 2, 0]);
        assert_eq!(tdm_slots(&[0xFF])[0..4], [3, 3, 3, 3]);
    }

    #[test]
    fn les_names_match_inmarsatc_oracle() {
        assert_eq!(les_name(2), Some("Stratos Global (Burum-2), Netherlands"));
        assert_eq!(les_name(202), Some("Stratos Global (Aukland), New Zealand"));
        assert_eq!(les_name(344), Some("NCS"));
        assert_eq!(les_name(104), Some("Vizada-Telenor, Norway"));
        assert_eq!(les_name(999), None);
        let station = sat_les(0x44);
        assert_eq!(station["region_long"], "Atlantic Ocean Region East (AOR-E)");
        assert_eq!(station["les_name"], "Vizada-Telenor, Norway");
    }

    #[test]
    fn area_shapes_follow_the_safetynet_manual() {
        assert_eq!(
            area_shape(0x04),
            Some((AreaShape::Rectangular, "D1D2La D3D4D5Lo D6D7 D8D9D10"))
        );
        assert_eq!(area_shape(0x44).map(|x| x.0), Some(AreaShape::Circular));
        assert_eq!(area_shape(0x31), Some((AreaShape::NavMetArea, "X1X2")));
        assert_eq!(area_shape(0x73).map(|x| x.0), Some(AreaShape::Coastal));
        assert_eq!(area_shape(0x02), None);
        let area = egc_area(0x04, &[0x04, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66]).expect("area");
        assert_eq!(area["address_payload_hex"], "112233445566");
        assert!(egc_area(0x02, &[0x02, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn geometry_matches_manual_worked_examples() {
        let rectangle = rectangular_geometry(&[0x3C, 0x0A, 0x9E, 0x19]).expect("rectangle");
        assert_eq!(rectangle["sw_corner"]["lat_deg"], 60);
        assert_eq!(rectangle["sw_corner"]["lon_deg"], -10);
        assert_eq!(rectangle["north_extent_nm"], 30);
        assert_eq!(rectangle["east_extent_nm"], 25);
        let circle = circular_geometry(&[0x0E, 0x42, 0x81, 0x2C]).expect("circle");
        assert_eq!(circle["center"]["lat_deg"], 14);
        assert_eq!(circle["center"]["lon_deg"], -66);
        assert_eq!(circle["radius_nm"], 300);
        let south = circular_geometry(&[0xA6, 0xA4, 0x03, 0xE7]).expect("circle");
        assert_eq!(south["center"]["lat_deg"], -38);
        assert_eq!(south["lon_hemisphere"], "E");
        assert_eq!(south["radius_nm"], 999);
    }

    #[test]
    fn navarea_and_coastal_geometry() {
        let navarea = nav_met_geometry(AreaShape::NavMetArea, &[12]).expect("navarea");
        assert_eq!(navarea["area_roman"], "XII");
        assert_eq!(navarea["coordinator"], "United States of America (West)");
        let coastal = nav_met_geometry(AreaShape::Coastal, b"\x02CA").expect("coastal");
        assert_eq!(coastal["coordinator"], "France");
        assert_eq!(coastal["coastal_area"], "C");
        assert_eq!(coastal["subject"], "navigational-warnings");
        assert_eq!(area_roman(17), "XVII");
        assert_eq!(area_roman(21), "XXI");
    }

    #[test]
    fn ita2_decode_matches_standard_alphabet() {
        assert_eq!(ita2_decode(&[0x14, 0x01, 0x12, 0x12, 0x18]), "HELLO");
        assert_eq!(ita2_decode(&[0x1B, 0x17, 0x13, 0x01, 0x0A, 0x10]), "12345");
        assert_eq!(ita2_decode(&[0x03, 0x04, 0x19]), "A B");
        assert_eq!(ita2_decode(&[0x1B, 0x10, 0x1F, 0x01]), "5E");
    }
}
