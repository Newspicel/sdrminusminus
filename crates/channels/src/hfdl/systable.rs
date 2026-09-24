use serde::Serialize;

use super::pdu::{coordinate, gs_name};

const MAX_FREQS: usize = 20;
const GS_RECORD_MIN: usize = 8;
const GS_FIXED_LEN: usize = 7;
const FREQ_RECORD_LEN: usize = 4;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GsFrequency {
    pub freq_hz: u32,
    pub master_frame_slot: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroundStation {
    pub gs_id: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gs_name: Option<String>,
    pub utc_sync: bool,
    pub lat: f64,
    pub lon: f64,
    pub spdu_version: u8,
    pub frequencies: Vec<GsFrequency>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemTable {
    pub version: u16,
    pub stations: Vec<GroundStation>,
}

fn bcd_frequency(b: &[u8]) -> u32 {
    let nibble = |x: u8| u32::from(x & 0x0F);
    100 * nibble(b[0])
        + 1_000 * nibble(b[0] >> 4)
        + 10_000 * nibble(b[1])
        + 100_000 * nibble(b[1] >> 4)
        + 1_000_000 * nibble(b[2])
        + 10_000_000 * nibble(b[2] >> 4)
}

fn parse_station(buf: &[u8]) -> Option<(GroundStation, usize)> {
    let freq_count = usize::from((buf[6] >> 3) & 0x1F);
    if freq_count > MAX_FREQS {
        return None;
    }
    let record_len = GS_FIXED_LEN + freq_count * FREQ_RECORD_LEN;
    if buf.len() < record_len {
        return None;
    }
    let lat =
        coordinate(u32::from(buf[1]) | u32::from(buf[2]) << 8 | (u32::from(buf[3]) & 0x0F) << 16);
    let lon = coordinate(u32::from(buf[3]) >> 4 | u32::from(buf[4]) << 4 | u32::from(buf[5]) << 12);
    let frequencies = buf[GS_FIXED_LEN..record_len]
        .as_chunks::<FREQ_RECORD_LEN>()
        .0
        .iter()
        .map(|record| GsFrequency {
            freq_hz: bcd_frequency(&record[..3]),
            master_frame_slot: record[3] & 0x0F,
        })
        .collect();
    let gs_id = buf[0] & 0x7F;
    let station = GroundStation {
        gs_id,
        gs_name: gs_name(gs_id).map(str::to_owned),
        utc_sync: buf[0] & 0x80 != 0,
        lat,
        lon,
        spdu_version: buf[6] & 0x07,
        frequencies,
    };
    Some((station, record_len))
}

pub fn parse_stations(mut buf: &[u8]) -> Option<Vec<GroundStation>> {
    let mut stations = Vec::new();
    while buf.len() >= GS_RECORD_MIN {
        let (station, len) = parse_station(buf)?;
        stations.push(station);
        buf = &buf[len..];
    }
    (!stations.is_empty()).then_some(stations)
}

#[derive(Debug, Default)]
pub struct SystableAssembler {
    version: u16,
    parts: Vec<Option<Vec<u8>>>,
}

impl SystableAssembler {
    pub fn store(
        &mut self,
        version: u16,
        seq: u8,
        total: u8,
        payload: &[u8],
    ) -> Option<SystemTable> {
        let (seq, total) = (usize::from(seq), usize::from(total));
        if total == 0 || seq >= total || payload.is_empty() {
            return None;
        }
        if self.version != version || self.parts.len() != total {
            self.parts = vec![None; total];
            self.version = version;
        }
        if self.parts[seq].as_deref() != Some(payload) {
            self.parts[seq] = Some(payload.to_vec());
        }
        if self.parts.iter().any(Option::is_none) {
            return None;
        }
        let body: Vec<u8> = self.parts.drain(..).flatten().flatten().collect();
        parse_stations(&body).map(|stations| SystemTable { version, stations })
    }
}

#[cfg(test)]
pub fn build_gs_record(station: &GroundStation) -> Vec<u8> {
    let coord = |deg: f64| -> u32 {
        ((deg * f64::from(1u32 << 19) / 180.0).round() as i32 as u32) & 0xFFFFF
    };
    let (lat, lon) = (coord(station.lat), coord(station.lon));
    let mut record = vec![
        station.gs_id & 0x7F | if station.utc_sync { 0x80 } else { 0 },
        (lat & 0xFF) as u8,
        ((lat >> 8) & 0xFF) as u8,
        ((lat >> 16) & 0x0F) as u8 | (((lon & 0x0F) as u8) << 4),
        ((lon >> 4) & 0xFF) as u8,
        ((lon >> 12) & 0xFF) as u8,
        (station.spdu_version & 0x07) | ((station.frequencies.len() as u8 & 0x1F) << 3),
    ];
    for frequency in &station.frequencies {
        let units = frequency.freq_hz / 100;
        let mut bcd = [0u8; 3];
        for (i, digit) in (0..6).map(|i| (units / 10u32.pow(i)) % 10).enumerate() {
            bcd[i / 2] |= (digit as u8) << ((i % 2) * 4);
        }
        record.extend_from_slice(&bcd);
        record.push(frequency.master_frame_slot & 0x0F);
    }
    record
}

#[cfg(test)]
pub fn build_systable_hfnpdus(version: u16, body: &[u8], total: u8) -> Vec<Vec<u8>> {
    let total = usize::from(total.max(1));
    let chunk = body.len().div_ceil(total);
    (0..total)
        .map(|i| {
            let mut hfnpdu = vec![
                0xFF,
                0xD0,
                (((total - 1) as u8) << 4) | i as u8,
                ((version & 0x0F) as u8) << 4,
                (version >> 4) as u8,
            ];
            hfnpdu.extend_from_slice(&body[i * chunk..body.len().min((i + 1) * chunk)]);
            hfnpdu
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_stations() -> Vec<GroundStation> {
        let frequency = |freq_hz, master_frame_slot| GsFrequency {
            freq_hz,
            master_frame_slot,
        };
        vec![
            GroundStation {
                gs_id: 1,
                gs_name: None,
                utc_sync: true,
                lat: 37.0179,
                lon: -122.9059,
                spdu_version: 2,
                frequencies: vec![
                    frequency(21_934_000, 0),
                    frequency(17_919_000, 3),
                    frequency(13_276_000, 7),
                ],
            },
            GroundStation {
                gs_id: 13,
                gs_name: None,
                utc_sync: true,
                lat: -37.6691,
                lon: 144.8410,
                spdu_version: 2,
                frequencies: vec![frequency(21_949_000, 1)],
            },
        ]
    }

    fn body() -> Vec<u8> {
        sample_stations().iter().flat_map(build_gs_record).collect()
    }

    #[test]
    fn gs_record_roundtrip() {
        let parsed = parse_stations(&body()).expect("parses");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].gs_id, 1);
        assert_eq!(parsed[0].frequencies[0].freq_hz, 21_934_000);
        assert_eq!(parsed[0].frequencies[2].master_frame_slot, 7);
        assert!((parsed[0].lat - 37.0179).abs() < 0.001);
        assert!((parsed[0].lon + 122.9059).abs() < 0.001);
        assert_eq!(parsed[1].gs_id, 13);
        assert!((parsed[1].lat + 37.6691).abs() < 0.001);
        assert_eq!(parsed[0].gs_name.as_deref(), Some("San Francisco, USA"));
        assert_eq!(parsed[1].gs_name.as_deref(), Some("Santa Cruz, Bolivia"));
    }

    #[test]
    fn gs_name_none_for_unassigned_id() {
        let mut station = sample_stations()[0].clone();
        station.gs_id = 12;
        let parsed = parse_stations(&build_gs_record(&station)).expect("parses");
        assert_eq!(parsed[0].gs_id, 12);
        assert_eq!(parsed[0].gs_name, None);
        let json = serde_json::to_string(&parsed[0]).expect("json");
        assert!(!json.contains("gs_name"));
    }

    #[test]
    fn bcd_frequency_decodes() {
        assert_eq!(bcd_frequency(&[0x67, 0x27, 0x13]), 13_276_700);
        assert_eq!(bcd_frequency(&[0x00, 0x00, 0x00]), 0);
    }

    #[test]
    fn reassembly_out_of_order() {
        let pdus = build_systable_hfnpdus(52, &body(), 3);
        let mut assembler = SystableAssembler::default();
        assert!(assembler.store(52, 2, 3, &pdus[2][5..]).is_none());
        assert!(assembler.store(52, 0, 3, &pdus[0][5..]).is_none());
        let table = assembler.store(52, 1, 3, &pdus[1][5..]).expect("complete");
        assert_eq!(table.version, 52);
        assert_eq!(table.stations.len(), 2);
        assert_eq!(table.stations[1].frequencies[0].freq_hz, 21_949_000);
    }

    #[test]
    fn version_change_discards_partial_set() {
        let old = build_systable_hfnpdus(51, &body(), 2);
        let new = build_systable_hfnpdus(52, &body(), 2);
        let mut assembler = SystableAssembler::default();
        assert!(assembler.store(51, 0, 2, &old[0][5..]).is_none());
        assert!(assembler.store(52, 1, 2, &new[1][5..]).is_none());
        assert!(assembler.store(52, 0, 2, &new[0][5..]).is_some());
    }

    #[test]
    fn malformed_body_rejected() {
        let mut record = build_gs_record(&sample_stations()[0]);
        record[6] |= 0x1F << 3;
        assert!(parse_stations(&record).is_none());
        assert!(parse_stations(&[]).is_none());
    }

    #[test]
    fn table_json_matches_xng() {
        let parsed = parse_stations(&body()).expect("parses");
        let reference = xng_mode_hfdl::systable::parse_stations(&body()).expect("xng parses");
        let ours = serde_json::to_value(SystemTable {
            version: 52,
            stations: parsed,
        })
        .expect("json");
        let theirs = serde_json::to_value(xng_mode_hfdl::systable::SystemTable {
            version: 52,
            stations: reference,
        })
        .expect("json");
        assert_eq!(ours, theirs);
    }
}
