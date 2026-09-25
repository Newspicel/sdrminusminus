use std::collections::HashMap;

use serde_json::{Value, json};

use super::*;
use crate::decoders::testing::event;

fn detail_of(kind: &str, data: Value) -> EventDetail {
    event_detail(&event(kind, data))
}

fn fields_of(kind: &str, data: Value) -> HashMap<&'static str, String> {
    detail_of(kind, data).fields.into_iter().collect()
}

fn labels(detail: &EventDetail) -> Vec<&'static str> {
    detail.fields.iter().map(|(label, _)| *label).collect()
}

fn has(shown: &HashMap<&'static str, String>, expected: &[(&str, &str)]) {
    for (label, value) in expected {
        assert_eq!(
            shown.get(label).map(String::as_str),
            Some(*value),
            "{label}"
        );
    }
}

#[test]
fn a_dect_base_station_breaks_into_identity_and_security() {
    let detail = detail_of(
        "dect",
        json!({
            "side": "rfp", "update": "capabilities",
            "identity": {
                "rfpi": "01234D5E6D", "pari": "02469ABCD", "arc": "a", "sari_available": false,
                "rpn": 5, "emc": 0x1234, "fpn": 0x1abcd, "multicell": true,
            },
            "carrier": 4, "carrier_hz": 1_890_432_000.0, "slot_pair": 2, "rf_carriers": 0x3ff,
            "capabilities": ["full_slot", "standard_authentication", "standard_ciphering"],
            "security": {
                "cipher_state": "active", "authentication_supported": true,
                "ciphering_supported": true, "encryption_events": 1,
            },
            "extended_carriers": false, "bursts": 40, "crc_errors": 2, "level_dbfs": -28.25,
        }),
    );
    let shown: HashMap<_, _> = detail.fields.iter().cloned().collect();
    has(
        &shown,
        &[
            ("RFPI", "01234D5E6D"),
            ("Access rights class", "A residential / small PBX"),
            ("Manufacturer code", "1234"),
            ("Cell", "multi-cell"),
            ("Frequency", "1.890432 GHz"),
            ("Authentication", "yes"),
            ("Ciphering", "yes"),
            ("Encryption", "encryption active"),
            ("Carriers available", "0, 1, 2, 3, 4, 5, 6, 7, 8, 9"),
            ("A-field CRC errors", "2"),
        ],
    );
    assert!(
        detail
            .body
            .is_some_and(|body| body.contains("standard ciphering (DSC)"))
    );
}

#[test]
fn weak_signal_spots_show_timing_and_link_measurements() {
    let detail = detail_of(
        "wspr",
        json!({
            "text": "K1ABC FN42 37", "callsign": "K1ABC", "grid": "FN42", "power_dbm": 37,
            "snr_db": -21.0, "audio_hz": 1501.25, "time_offset_s": 0.75, "drift_hz": -0.2,
        }),
    );
    let shown: HashMap<_, _> = detail.fields.iter().cloned().collect();
    has(
        &shown,
        &[
            ("Callsign", "K1ABC"),
            ("Grid", "FN42"),
            ("Power", "37 dBm"),
            ("SNR", "-21 dB"),
            ("Time offset", "+0.75 s"),
            ("Drift", "-0.2 Hz"),
        ],
    );
    assert_eq!(detail.body.as_deref(), Some("K1ABC FN42 37"));
}

#[test]
fn selcall_shows_the_plan_code_and_duration() {
    let shown = fields_of(
        "selcall",
        json!({"system": "zvei1", "code": "A11D0", "tone_ms": 70}),
    );
    assert_eq!(shown.len(), 3);
    has(
        &shown,
        &[
            ("Tone plan", "ZVEI-1"),
            ("Code", "A11D0"),
            ("Tone duration", "70 ms"),
        ],
    );
}

#[test]
fn fields_a_frame_did_not_carry_are_left_out() {
    let bare = detail_of("adsb", json!({"icao": "3c6444", "df": 11, "raw": "5d"}));
    assert_eq!(labels(&bare), ["ICAO", "Downlink format", "Raw"]);
    let full = fields_of(
        "adsb",
        json!({
            "icao": "3c6444", "df": 17, "raw": "8d3c6444", "callsign": " DLH123 ",
            "altitude_ft": 37000, "ground_speed_kt": 451.4, "track_deg": 271.6,
            "vertical_rate_fpm": -1088, "lat": 52.52, "lon": 13.405,
        }),
    );
    has(
        &full,
        &[
            ("Callsign", "DLH123"),
            ("Altitude", "37,000 ft"),
            ("Position", "52.52000, 13.40500"),
            ("Ground speed", "451.4 kt"),
            ("Track", "272°"),
            ("Vertical rate", "-1088 ft/min"),
        ],
    );
}

#[test]
fn a_pocsag_ric_is_padded_and_its_function_named() {
    let detail = detail_of(
        "pocsag",
        json!({
            "address": 1234, "function": 3, "baud": 1200, "payload": "alpha",
            "text": "CALL 42", "errors_corrected": 2,
        }),
    );
    let shown: HashMap<_, _> = detail.fields.iter().cloned().collect();
    has(
        &shown,
        &[
            ("RIC", "0001234"),
            ("Function", "D (3)"),
            ("Baud", "1200"),
            ("Repaired", "2"),
        ],
    );
    assert_eq!(detail.body.as_deref(), Some("CALL 42"));
}

#[test]
fn navtex_keeps_its_text_and_header() {
    let detail = detail_of(
        "navtex",
        json!({
            "station": "D", "subject": "A", "subject_name": "Navigational warning", "serial": 7,
            "text": "GALE WARNING\nGERMAN BIGHT", "errors_corrected": 3, "complete": false,
        }),
    );
    let shown: HashMap<_, _> = detail.fields.iter().cloned().collect();
    has(
        &shown,
        &[
            ("Header", "DA07"),
            ("Subject", "Navigational warning"),
            ("Serial", "07"),
            ("Ended with NNNN", "no: flushed early"),
            ("Repaired", "3 characters"),
        ],
    );
    assert_eq!(detail.body.as_deref(), Some("GALE WARNING\nGERMAN BIGHT"));
}

#[test]
fn acars_names_direction_and_continuation() {
    let shown = fields_of(
        "acars",
        json!({
            "mode": "2", "registration": "D-AIBC", "flight": "LH0400 ", "label": "H1",
            "block_id": "3", "downlink": true, "seq_no": "M01A",
            "text": "POS N52.5 E013.4\nFL370", "more": true,
        }),
    );
    has(
        &shown,
        &[
            ("Registration", "D-AIBC"),
            ("Flight", "LH0400"),
            ("Direction", "downlink"),
            ("Sequence", "M01A"),
            ("Acknowledges", "NAK"),
            ("Continues", "yes: another block follows"),
        ],
    );
}

#[test]
fn subghz_timings_pair_pulse_with_gap() {
    let detail = detail_of(
        "subghz",
        json!({
            "modulation": "ook", "encoding": "pwm", "bits": 24, "data": "A1B2C3",
            "address": 0xa1b2, "button": 3, "short_us": 320, "repeats": 6,
            "timings_us": [320, 960, 960, 320, 320],
        }),
    );
    let shown: HashMap<_, _> = detail.fields.iter().cloned().collect();
    has(
        &shown,
        &[
            ("Payload", "A1B2C3 (24 bit)"),
            ("EV1527 address", "0A1B2"),
            ("EV1527 button", "3"),
            ("Base period", "320 µs"),
            ("Repeats", "×6"),
        ],
    );
    assert!(!labels(&detail).contains(&"PT2262 tri-state"));
    assert_eq!(detail.body.as_deref(), Some("320/960  960/320  320"));
}

#[test]
fn a_sensor_reading_leads_the_framing() {
    let detail = detail_of(
        "subghz",
        json!({
            "modulation": "ook", "encoding": "ppm", "bits": 36, "data": "8F80D5F2F",
            "short_us": 1000, "repeats": 8,
            "reading": {
                "model": "Nexus-TH", "id": 0x8f, "channel": 1, "battery_ok": true,
                "temperature_c": 21.3, "humidity_pct": 47.0,
            },
        }),
    );
    let first: Vec<(&str, &str)> = detail
        .fields
        .iter()
        .take(6)
        .map(|(label, value)| (*label, value.as_str()))
        .collect();
    assert_eq!(
        first,
        [
            ("Model", "Nexus-TH"),
            ("Sensor id", "8F"),
            ("Channel", "1"),
            ("Temperature", "21.3 °C"),
            ("Humidity", "47 %"),
            ("Battery", "yes"),
        ]
    );
}

#[test]
fn a_raw_capture_has_no_payload() {
    let detail = detail_of(
        "subghz",
        json!({"modulation": "ook", "encoding": "raw", "bits": 0, "data": "", "short_us": 0, "repeats": 1}),
    );
    assert_eq!(labels(&detail), ["Modulation", "Encoding"]);
    assert_eq!(detail.body, None);
}

#[test]
fn not_encrypted_differs_from_unsaid() {
    let said = fields_of(
        "dv",
        json!({"mode": "dmr", "kind": "header", "errors_corrected": 0, "encrypted": false}),
    );
    assert_eq!(said.get("Encrypted").map(String::as_str), Some("no"));
    let silent = fields_of(
        "dv",
        json!({"mode": "dmr", "kind": "header", "errors_corrected": 0}),
    );
    assert!(!silent.contains_key("Encrypted"));
    assert!(!silent.contains_key("Trunking"));
    assert!(!silent.contains_key("Checksum"));
}

#[test]
fn dmr_metadata_is_exposed_and_packet_data_kept() {
    let detail = detail_of(
        "dv",
        json!({
            "mode": "dmr", "kind": "control", "errors_corrected": 2, "vendor": "hytera",
            "manufacturer_id": 8, "talker_alias": "Dispatcher", "lat": 52.52, "lon": 13.405,
            "position_error_m": 20, "channel": 407, "emergency": true, "algorithm_id": 5,
            "key_id": 42, "message_indicator": "001122334455667788",
            "slot_activity": [{"slot": 2, "activity": "group voice", "destination_hash": 0xab}],
            "data": "A1B2C3",
        }),
    );
    let shown: HashMap<_, _> = detail.fields.iter().cloned().collect();
    has(
        &shown,
        &[
            ("Vendor", "Hytera (0x08)"),
            ("Position error", "≤ 20 m"),
            ("Channel", "407"),
            ("Emergency", "yes"),
            ("Slot activity", "TS2 group voice (hash 0xAB)"),
            ("Algorithm", "0x05"),
            ("Key ID", "0x002A"),
            ("Repaired", "2 bits"),
        ],
    );
    assert_eq!(detail.body.as_deref(), Some("A1B2C3"));
}

#[test]
fn a_trunked_burst_says_its_side_and_checksum() {
    let checked = fields_of(
        "dv",
        json!({
            "mode": "dmr", "kind": "control", "errors_corrected": 0,
            "trunk_protocol": "tier_three", "control_channel": true, "crc_verified": true,
        }),
    );
    has(
        &checked,
        &[
            ("Trunking", "Tier III · control channel"),
            ("Checksum", "verified"),
        ],
    );
    let unchecked = fields_of(
        "dv",
        json!({
            "mode": "dmr", "kind": "control", "errors_corrected": 0,
            "trunk_protocol": "tier_three", "control_channel": false, "crc_verified": false,
        }),
    );
    has(
        &unchecked,
        &[
            ("Trunking", "Tier III · traffic channel"),
            ("Checksum", "not verified: read on error correction alone"),
        ],
    );
}

#[test]
fn rds_reads_as_fields_with_radiotext_as_body() {
    let detail = detail_of(
        "rds",
        json!({
            "groups": 100, "blocks": 402, "block_errors": 2, "pi": "D389", "ps": "RADIO 1 ",
            "pty_name": "Pop Music", "tp": true, "music": false,
            "alt_freqs_hz": [98_000_000.0, 100_500_000.0], "radiotext": "Now playing something",
        }),
    );
    let shown: HashMap<_, _> = detail.fields.iter().cloned().collect();
    has(
        &shown,
        &[
            ("Station", "RADIO 1"),
            ("Programme type", "Pop Music"),
            ("Traffic programme", "yes"),
            ("Content", "speech"),
            ("Alternative frequencies", "98 MHz, 100.5 MHz"),
            ("Block errors", "2"),
        ],
    );
    assert_eq!(detail.body.as_deref(), Some("Now playing something"));
}

#[test]
fn broadcast_audio_failures_show_regardless_of_lock() {
    let shown = fields_of(
        "broadcast",
        json!({
            "system": "dab", "locked": true, "snr_db": 20.0, "frequency_error_hz": 0.0,
            "audio_frames_ok": 41, "audio_frames_bad": 2,
            "audio_error": "Broadcast audio input queue overflow",
        }),
    );
    has(
        &shown,
        &[
            ("Lock", "locked"),
            ("Audio frames", "41"),
            ("Audio failures", "2"),
            ("Audio error", "Broadcast audio input queue overflow"),
        ],
    );
}

#[test]
fn a_broadcast_acquisition_invents_no_multiplex_metadata() {
    let detail = detail_of(
        "broadcast",
        json!({
            "system": "dvb_s2", "locked": true, "snr_db": 18.26, "frequency_error_hz": -32.4,
            "symbol_rate": 333000.0,
        }),
    );
    let shown: Vec<(&str, &str)> = detail
        .fields
        .iter()
        .map(|(label, value)| (*label, value.as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            ("System", "DVB-S2"),
            ("Lock", "locked"),
            ("SNR", "18.3 dB"),
            ("Frequency error", "-32 Hz"),
            ("Symbol rate", "333000 Bd"),
        ]
    );
    assert_eq!(detail.body, None);
}

#[test]
fn aprs_unpacks_its_monitor_line() {
    let detail = detail_of(
        "aprs",
        json!({
            "source": "DL1ABC-9", "destination": "S32U6T", "path": ["WIDE1-1", "WIDE2-1"],
            "info": "`(_fn\"Oj/", "tnc2": "DL1ABC-9>S32U6T:`(_fn\"Oj/", "lat": 52.52,
            "lon": 13.405, "course_deg": 251.0, "speed_kt": 20.0, "altitude_ft": 1500,
            "mic_e_message": "En Route",
        }),
    );
    let shown: HashMap<_, _> = detail.fields.iter().cloned().collect();
    has(
        &shown,
        &[
            ("Path", "WIDE1-1 → WIDE2-1"),
            ("Course", "251°"),
            ("Speed", "20.0 kt"),
            ("Altitude", "1,500 ft"),
            ("Mic-E message", "En Route"),
        ],
    );
    assert_eq!(detail.body.as_deref(), Some("DL1ABC-9>S32U6T:`(_fn\"Oj/"));
}

#[test]
fn an_empty_text_frame_adds_nothing() {
    let detail = detail_of("rtty", json!({"text": ""}));
    assert!(detail.fields.is_empty());
    assert_eq!(detail.body, None);
}

#[test]
fn a_utc_offset_carries_its_sign() {
    assert_eq!(utc_offset(Some(120)).as_deref(), Some("UTC+02:00"));
    assert_eq!(utc_offset(Some(-330)).as_deref(), Some("UTC−05:30"));
    assert_eq!(utc_offset(None), None);
}
