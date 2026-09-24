use super::{
    build::{
        acars_mpdu, lpdu_acars, lpdu_hfnpdu, lpdu_logon_confirm, mpdu_downlink, mpdu_uplink, spdu,
        with_fcs,
    },
    *,
};
use crate::hfdl::systable::{GroundStation, GsFrequency, build_gs_record, build_systable_hfnpdus};

fn parse(parser: &mut PduParser, payload: &[u8], bps: u32) -> Vec<HfdlEvent> {
    let mut out = Vec::new();
    parser.parse(payload, bps, &mut out);
    out
}

fn parse_fresh(payload: &[u8]) -> Vec<HfdlEvent> {
    parse(&mut PduParser::new(), payload, 300)
}

fn parse_hfnpdu_body(hfnpdu: &[u8]) -> Vec<HfdlEvent> {
    parse_fresh(&mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(hfnpdu)]))
}

fn find<'a>(events: &'a [HfdlEvent], kind: &str) -> &'a HfdlEvent {
    events
        .iter()
        .find(|e| e.kind == kind)
        .unwrap_or_else(|| panic!("no {kind}"))
}

fn raw_coord(deg: f64) -> u32 {
    ((deg * f64::from(1u32 << 19) / 180.0).round() as i32 as u32) & 0xFFFFF
}

fn put_fix(h: &mut [u8], lat_deg: f64, lon_deg: f64) {
    let (lat, lon) = (raw_coord(lat_deg), raw_coord(lon_deg));
    h[8] = (lat & 0xFF) as u8;
    h[9] = ((lat >> 8) & 0xFF) as u8;
    h[10] = ((lat >> 16) & 0x0F) as u8 | (((lon & 0x0F) as u8) << 4);
    h[11] = ((lon >> 4) & 0xFF) as u8;
    h[12] = ((lon >> 12) & 0xFF) as u8;
}

fn perf_with_fix(flight: &[u8; 6], lat_deg: f64, lon_deg: f64, utc_raw: u16) -> Vec<u8> {
    let mut h = vec![0u8; 47];
    h[0] = 0xFF;
    h[1] = 0xD1;
    h[2..8].copy_from_slice(flight);
    put_fix(&mut h, lat_deg, lon_deg);
    h[13..15].copy_from_slice(&utc_raw.to_le_bytes());
    h
}

fn full_performance_record() -> Vec<u8> {
    let mut h = perf_with_fix(b"UA0042", 40.0, -73.0, 10_800);
    h[15] = 3;
    h[16] = 7;
    h[17] = 0x84;
    h[18] = 2;
    h[19..21].copy_from_slice(&11u16.to_le_bytes());
    h[21..23].copy_from_slice(&5u16.to_le_bytes());
    h[23..25].copy_from_slice(&300u16.to_le_bytes());
    h[25..27].copy_from_slice(&60u16.to_le_bytes());
    h[27..31].copy_from_slice(&[18, 12, 6, 3]);
    h[31..35].copy_from_slice(&[1, 2, 3, 4]);
    h[35..37].copy_from_slice(&1000u16.to_le_bytes());
    h[37] = 9;
    h[38..42].copy_from_slice(&[80, 70, 60, 50]);
    h[42..46].copy_from_slice(&[8, 7, 6, 5]);
    h[46] = 0x04;
    h
}

fn push_gs(h: &mut Vec<u8>, gs_id: u8, prop: u32, tuned: u32) {
    h.push(gs_id);
    h.push((prop & 0xFF) as u8);
    h.push(((prop >> 8) & 0xFF) as u8);
    h.push(((prop >> 16) & 0x0F) as u8 | (((tuned & 0x0F) as u8) << 4));
    h.push(((tuned >> 4) & 0xFF) as u8);
    h.push(((tuned >> 12) & 0xFF) as u8);
}

fn frequency_record() -> Vec<u8> {
    let mut h = vec![0xFF, 0xD5];
    h.extend_from_slice(b"DLH456");
    h.extend_from_slice(&[0u8; 7]);
    push_gs(&mut h, 0x84, 0b101, 0b011);
    push_gs(&mut h, 0x0A, 0xABCDE, 0x12345);
    h
}

fn riverhead() -> GroundStation {
    GroundStation {
        gs_id: 4,
        gs_name: None,
        utc_sync: true,
        lat: 40.88,
        lon: -72.64,
        spdu_version: 2,
        frequencies: vec![
            GsFrequency {
                freq_hz: 21_931_000,
                master_frame_slot: 2,
            },
            GsFrequency {
                freq_hz: 11_387_000,
                master_frame_slot: 5,
            },
        ],
    }
}

#[test]
fn spdu_roundtrip() {
    let events = parse_fresh(&spdu(7, 1234, 52));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "squitter");
    assert_eq!(events[0].details["gs_id"], 7);
    assert_eq!(events[0].details["frame_index"], 1234);
    assert_eq!(events[0].details["systable_version"], 52);
}

#[test]
fn gs_name_roster_matches_published_list() {
    const ROSTER: &[(u8, &str)] = &[
        (1, "San Francisco, USA"),
        (2, "Molokai, Hawaii"),
        (3, "Reykjavik, Iceland"),
        (4, "Riverhead, New York"),
        (5, "Auckland, New Zealand"),
        (6, "Hat Yai, Thailand"),
        (7, "Shannon, Ireland"),
        (8, "Johannesburg, South Africa"),
        (9, "Barrow, Alaska"),
        (10, "Muan, South Korea"),
        (11, "Albrook, Panama"),
        (13, "Santa Cruz, Bolivia"),
        (14, "Krasnoyarsk, Russia"),
        (15, "Al Muharraq, Bahrain"),
        (16, "Agana, Guam"),
        (17, "Canarias, Spain"),
    ];
    for id in 0u8..=127 {
        let want = ROSTER.iter().find(|(r, _)| *r == id).map(|(_, n)| *n);
        assert_eq!(gs_name(id), want, "gs id {id}");
    }
}

#[test]
fn spdu_first_octet_flags() {
    let mut p = vec![0u8; 64];
    p[0] = 0b1110_1010;
    p[1] = 7 | 0x80;
    let events = parse_fresh(&with_fcs(p));
    let d = &events[0].details;
    assert_eq!(d["rls_in_use"], true);
    assert_eq!(d["spdu_version"], 2);
    assert_eq!(d["iso8208_supported"], true);
    assert_eq!(d["change_note"], 3);
    let mut p = vec![0u8; 64];
    p[1] = 4 | 0x80;
    let events = parse_fresh(&with_fcs(p));
    let d = &events[0].details;
    assert_eq!(d["rls_in_use"], false);
    assert_eq!(d["spdu_version"], 0);
    assert_eq!(d["iso8208_supported"], false);
    assert_eq!(d["change_note"], 0);
}

#[test]
fn spdu_slot_assignment_region_is_surfaced_raw() {
    let mut p = vec![0u8; 64];
    p[1] = 4 | 0x80;
    for (i, b) in p.iter_mut().enumerate().take(52).skip(4) {
        *b = (i as u8).wrapping_mul(3).wrapping_add(1);
    }
    p[52] = 0x0A;
    let events = parse_fresh(&with_fcs(p.clone()));
    let d = &events[0].details;
    assert_eq!(d["slot_assignment_hex"], hex(&p[4..52]));
    assert_eq!(d["slot_assignment_hex"].as_str().map(str::len), Some(96));
    assert_eq!(d["min_priority"], 0x0A);
}

#[test]
fn mpdu_with_acars_roundtrip() {
    let events = parse(&mut PduParser::new(), &acars_mpdu(), 1800);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "acars");
    let block = events[0].acars.as_ref().expect("acars");
    assert!(block.crc_ok);
    assert_eq!(block.core.label, "B6");
    assert_eq!(block.core.app.as_ref().expect("app")["app"], "adsc");
}

#[test]
fn systable_reassembles_across_mpdus() {
    let body = build_gs_record(&riverhead());
    let pdus = build_systable_hfnpdus(77, &body, 2);
    let mut parser = PduParser::new();
    let first = parse(
        &mut parser,
        &mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(&pdus[0])]),
        300,
    );
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].kind, "systable-partial");
    let second = parse(
        &mut parser,
        &mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(&pdus[1])]),
        300,
    );
    assert_eq!(second.len(), 2);
    assert_eq!(second[1].kind, "systable-complete");
    let d = &second[1].details;
    assert_eq!(d["version"], 77);
    assert_eq!(d["stations"][0]["gs_id"], 4);
    assert_eq!(d["stations"][0]["gs_name"], "Riverhead, New York");
    assert_eq!(d["stations"][0]["frequencies"][0]["freq_hz"], 21_931_000);
}

#[test]
fn corrupted_mpdu_header_rejected() {
    let mut mpdu = mpdu_downlink(3, 0xC7, &[lpdu_acars(b"\x01dummy")]);
    mpdu[1] ^= 0x01;
    assert!(parse_fresh(&mpdu).is_empty());
}

#[test]
fn performance_data_full_record() {
    let events = parse_hfnpdu_body(&full_performance_record());
    let d = &find(&events, "performance-data").details;
    assert_eq!(d["flight"], "UA0042");
    assert_eq!(d["version"], 3);
    assert_eq!(d["flight_leg"], 7);
    assert_eq!(d["gs_id"], 4);
    assert_eq!(d["gs_name"], "Riverhead, New York");
    assert_eq!(d["freq_id"], 2);
    assert_eq!(d["utc"], json!({"hour": 6, "min": 0, "sec": 0}));
    assert_eq!(d["freq_search_cnt"], json!({"prev_leg": 11, "cur_leg": 5}));
    assert_eq!(
        d["hfdl_disabled_duration"],
        json!({"prev_leg": 300, "cur_leg": 60})
    );
    assert_eq!(d["mpdus_rx"]["1800bps"], 18);
    assert_eq!(d["mpdus_rx"]["300bps"], 3);
    assert_eq!(d["mpdus_rx_errs"]["300bps"], 4);
    assert_eq!(d["spdus_rx"], 1000);
    assert_eq!(d["spdus_rx_errs"], 9);
    assert_eq!(d["mpdus_tx"]["1800bps"], 80);
    assert_eq!(d["mpdus_delivered"]["300bps"], 5);
    assert_eq!(d["freq_change_cause"], "GS frequency change");
    assert!((d["lat"].as_f64().unwrap_or_default() - 40.0).abs() < 0.001);
    assert!((d["lon"].as_f64().unwrap_or_default() + 73.0).abs() < 0.001);
}

#[test]
fn performance_data_too_short_falls_through() {
    let mut h = vec![0u8; 46];
    h[0] = 0xFF;
    h[1] = 0xD1;
    assert!(
        parse_hfnpdu_body(&h)
            .iter()
            .all(|e| e.kind != "performance-data")
    );
}

#[test]
fn frequency_data_per_gs_arrays() {
    let events = parse_hfnpdu_body(&frequency_record());
    let e = find(&events, "frequency-data");
    let fd = e.details["freq_data"].as_array().expect("array");
    assert_eq!(fd.len(), 2);
    assert_eq!(fd[0]["gs_id"], 4);
    assert_eq!(fd[0]["prop_freqs"], 0b101);
    assert_eq!(fd[0]["tuned_freqs"], 0b011);
    assert_eq!(fd[1]["gs_name"], "Muan, South Korea");
    assert_eq!(fd[1]["prop_freqs"], 0xABCDE);
    assert_eq!(fd[1]["tuned_freqs"], 0x12345);
    assert_eq!(e.details["flight"], "DLH456");
}

#[test]
fn named_hfnpdus_and_lpdus() {
    let request = parse_hfnpdu_body(&[0xFF, 0xD2, 0x34, 0x12]);
    assert_eq!(
        find(&request, "systable-request").details["request_data"],
        0x1234
    );
    assert!(
        parse_hfnpdu_body(&[0xFF, 0xDE, 0x00])
            .iter()
            .any(|e| e.kind == "delayed-echo")
    );
    let denied = with_fcs(vec![
        0x2F,
        0x04u8.reverse_bits(),
        0x00u8.reverse_bits(),
        0x87u8.reverse_bits(),
        0x01,
    ]);
    let events = parse_fresh(&mpdu_downlink(4, 0xC7, &[denied]));
    let e = find(&events, "logon-denied");
    assert_eq!(e.details["icao"], "040087");
    assert_eq!(e.details["reason_text"], "Aircraft ID not available");
}

#[test]
fn cache_resolves_downlink_icao_after_logon_confirm() {
    let mut parser = PduParser::new();
    let confirm = mpdu_uplink(4, 0xFF, &[lpdu_logon_confirm(0x040087, 0x42)]);
    let events = parse(&mut parser, &confirm, 300);
    assert_eq!(find(&events, "logon-confirm").details["icao"], "040087");
    assert_eq!(parser.resolve_icao(0x42), Some("040087"));
    let perf = perf_with_fix(b"UAL042", 40.0, -73.0, 10_800);
    let events = parse(
        &mut parser,
        &mpdu_downlink(4, 0x42, &[lpdu_hfnpdu(&perf)]),
        300,
    );
    let d = &find(&events, "performance-data").details;
    assert_eq!(d["who"]["icao"], "040087");
    assert_eq!(d["position"]["aircraft_id"], 0x42);
    assert_eq!(d["position"]["icao"], "040087");
}

#[test]
fn cache_evicts_on_logoff() {
    let mut parser = PduParser::new();
    let confirm = mpdu_uplink(4, 0xFF, &[lpdu_logon_confirm(0x04C11B, 0x55)]);
    parse(&mut parser, &confirm, 300);
    assert_eq!(parser.resolve_icao(0x55), Some("04C11B"));
    let logoff = with_fcs(vec![
        0x3F,
        0x04u8.reverse_bits(),
        0xC1u8.reverse_bits(),
        0x1Bu8.reverse_bits(),
        0x04,
    ]);
    let events = parse(&mut parser, &mpdu_downlink(4, 0x55, &[logoff]), 300);
    assert_eq!(
        find(&events, "logoff-request").details["reason_text"],
        "Invalid aircraft ID"
    );
    assert_eq!(parser.resolve_icao(0x55), None);
}

#[test]
fn cache_ttl_expiry_drops_resolution() {
    let mut parser = PduParser::with_ac_cache_ttl(std::time::Duration::ZERO);
    let confirm = mpdu_uplink(4, 0xFF, &[lpdu_logon_confirm(0x040087, 0x42)]);
    parse(&mut parser, &confirm, 300);
    std::thread::sleep(std::time::Duration::from_millis(1));
    assert_eq!(parser.resolve_icao(0x42), None);
}

#[test]
fn position_suppressed_for_null_island_fix() {
    let events = parse_hfnpdu_body(&perf_with_fix(b"AAL999", 0.0, 0.0, 0));
    let e = find(&events, "performance-data");
    assert!(e.details.get("position").is_none());
    assert_eq!(e.details["lat"], 0.0);
}

#[test]
fn position_object_extracted_from_frequency_data() {
    let mut h = vec![0u8; 15];
    h[0] = 0xFF;
    h[1] = 0xD5;
    h[2..8].copy_from_slice(b"DLH456");
    put_fix(&mut h, 51.5, -0.1);
    h[13] = 0x10;
    let events = parse_hfnpdu_body(&h);
    let position = &find(&events, "frequency-data").details["position"];
    assert!((position["lat"].as_f64().unwrap_or_default() - 51.5).abs() < 0.001);
    assert_eq!(position["utc_s"], 32);
    assert_eq!(position["flight"], "DLH456");
}

fn equivalence_corpus() -> Vec<Vec<u8>> {
    let systable = build_systable_hfnpdus(77, &build_gs_record(&riverhead()), 2);
    let mut corpus = vec![
        spdu(7, 1234, 52),
        acars_mpdu(),
        mpdu_uplink(4, 0xFF, &[lpdu_logon_confirm(0x040087, 0x42)]),
        mpdu_downlink(
            4,
            0x42,
            &[lpdu_hfnpdu(&perf_with_fix(b"UAL042", 40.0, -73.0, 10_800))],
        ),
        mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(&full_performance_record())]),
        mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(&frequency_record())]),
        mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(&[0xFF, 0xD2, 0x34, 0x12])]),
        mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(&[0xFF, 0xDE])]),
        mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(&[0xFF, 0xAA, 1])]),
        mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(&[0x12, 0x34])]),
        mpdu_downlink(4, 0xC7, &[lpdu_acars(b"\x01broken")]),
        mpdu_downlink(4, 0xC7, &[with_fcs(vec![0x77, 1, 2])]),
    ];
    corpus.extend(
        systable
            .iter()
            .map(|pdu| mpdu_downlink(4, 0xC7, &[lpdu_hfnpdu(pdu)])),
    );
    corpus
}

#[test]
fn parser_matches_xng() {
    let mut ours = PduParser::new();
    let mut theirs = xng_mode_hfdl::pdu::PduParser::new();
    for payload in equivalence_corpus() {
        let expected = theirs.parse(&payload, 600);
        let got = parse(&mut ours, &payload, 600);
        assert_eq!(got.len(), expected.len(), "{payload:02x?}");
        for (a, b) in got.iter().zip(&expected) {
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.details, b.details);
            assert_eq!(a.raw, b.raw);
            assert_eq!(
                serde_json::to_value(&a.acars).ok(),
                serde_json::to_value(&b.acars).ok()
            );
        }
    }
}
