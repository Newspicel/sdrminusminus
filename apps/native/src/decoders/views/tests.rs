use serde_json::json;

use super::*;
use crate::decoders::testing::{event, record_at};

const NOW: i64 = 1_786_276_800_000;

fn record(kind: &str, data: Value) -> Arc<DecodedRecord> {
    Arc::new(record_at(event(kind, data), "2026-08-09T12:00:00Z", 0))
}

fn on_channel(kind: &str, data: Value, channel: u32) -> Arc<DecodedRecord> {
    Arc::new(record_at(
        event(kind, data),
        "2026-08-09T12:00:00Z",
        channel,
    ))
}

fn vor(station: &str, lat: f64, lon: f64, radial: f64) -> Value {
    json!({
        "station": station, "station_lat": lat, "station_lon": lon,
        "magnetic_declination_deg": 0.0, "radial_deg": radial,
        "variable_phase_deg": 0.0, "reference_phase_deg": 0.0,
        "signal_db": -12.0, "confidence": 1.0,
    })
}

fn readings(records: &[Arc<DecodedRecord>]) -> Vec<&VorReading> {
    records.iter().filter_map(|record| vor_of(record)).collect()
}

fn station(data: DecoderEvent, last_seen_ms: i64, frames: u64) -> Station {
    Station {
        kind: data.kind(),
        id: String::new(),
        event: data,
        last_seen_ms,
        freq_hz: 1_090_000_000.0,
        device_set: 1,
        channel: 0,
        frames,
    }
}

#[test]
fn an_empty_scope_holds_everything() {
    let scope = DecoderScope::default();
    assert!(scope.holds(3, 9));
    let narrow = DecoderScope {
        device_set: Some(1),
        channel: Some(0),
    };
    assert!(narrow.holds(1, 0));
    assert!(!narrow.holds(1, 2));
}

#[test]
fn two_non_parallel_radials_intersect() {
    let records = [
        record("vor", vor("A", 0.0, 0.0, 45.0)),
        on_channel("vor", vor("B", 0.0, 2.0, 315.0), 1),
    ];
    let fix = multi_vor_fix(&readings(&records));
    let fix = fix.unwrap_or(VorFix {
        lat: 0.0,
        lon: 0.0,
        residual_km: 9.0,
        stations: 0,
    });
    assert!((fix.lat - 1.0).abs() < 1e-3);
    assert!((fix.lon - 1.0).abs() < 1e-3);
    assert!(fix.residual_km < 0.001);
    assert_eq!(fix.stations, 2);
}

#[test]
fn radials_intersect_across_the_antimeridian() {
    let records = [
        record("vor", vor("A", 0.0, 179.5, 45.0)),
        on_channel("vor", vor("B", 0.0, -178.5, 315.0), 1),
    ];
    let fix = multi_vor_fix(&readings(&records));
    assert!(fix.is_some_and(|fix| (fix.lat - 1.0).abs() < 1e-3 && (fix.lon + 179.5).abs() < 1e-3));
}

#[test]
fn the_newest_reading_of_each_station_wins() {
    let newest = Arc::new(record_at(
        event("vor", vor("A", 0.0, 0.0, 45.0)),
        "2026-08-09T12:00:01Z",
        0,
    ));
    let older = record("vor", vor("A", 0.0, 0.0, 90.0));
    let latest = latest_vor_readings(&[older, Arc::clone(&newest)]);
    assert_eq!(latest, vec![Arc::clone(&newest)]);
    assert_eq!(multi_vor_fix(&readings(&latest)), None);
}

#[test]
fn a_target_dims_before_it_disappears() {
    assert_eq!(age_class(0), Age::Fresh);
    assert_eq!(age_class(TARGET_STALE_MS - 1), Age::Fresh);
    assert_eq!(age_class(TARGET_STALE_MS), Age::Stale);
    assert_eq!(age_class(TARGET_MAX_AGE_MS / 2), Age::Fading);
}

#[test]
fn age_reads_as_seconds_then_minutes_then_hours() {
    assert_eq!(format_age(0), "0s");
    assert_eq!(format_age(59_400), "59s");
    assert_eq!(format_age(60_000), "1:00");
    assert_eq!(format_age(3_599_000), "59:59");
    assert_eq!(format_age(3_600_000), "1h00");
    assert_eq!(format_age(-5_000), "0s");
}

#[test]
fn an_aircraft_projects_into_a_row() {
    let aircraft = station(
        event(
            "adsb",
            json!({
                "icao": "3c6444", "df": 17, "raw": "8d3c6444", "callsign": " DLH123 ",
                "altitude_ft": 37000, "ground_speed_kt": 451.4, "track_deg": 271.6,
                "lat": 52.52, "lon": 13.405,
            }),
        ),
        NOW - 12_000,
        42,
    );
    assert_eq!(
        aircraft_row(&aircraft, NOW),
        Some(TargetRow {
            id: "3C6444".to_owned(),
            label: "DLH123".to_owned(),
            primary: "37,000 ft".to_owned(),
            secondary: "451 kt · 272°".to_owned(),
            position: "52.52000, 13.40500".to_owned(),
            age_ms: 12_000,
            frames: 42,
        })
    );
}

#[test]
fn a_grounded_aircraft_shows_gnd_and_never_a_negative_age() {
    let grounded = station(
        event(
            "adsb",
            json!({"icao": "3c6444", "df": 17, "raw": "8d", "on_ground": true, "altitude_ft": 0}),
        ),
        NOW + 500,
        1,
    );
    let row = aircraft_row(&grounded, NOW);
    assert_eq!(row.as_ref().map(|r| r.primary.as_str()), Some("GND"));
    assert_eq!(row.as_ref().map(|r| r.label.as_str()), Some("-"));
    assert_eq!(row.as_ref().map(|r| r.position.as_str()), Some("-"));
    assert_eq!(row.as_ref().map(|r| r.age_ms), Some(0));
}

#[test]
fn a_ship_falls_back_from_name_to_call_sign() {
    let ship = station(
        event(
            "ais",
            json!({
                "mmsi": 211234560, "msg_type": 1, "ais_channel": "A", "nmea": "!AIVDM",
                "call_sign": "DEAB", "sog_kt": 12.4, "cog_deg": 359.7, "destination": "HAMBURG",
            }),
        ),
        NOW - 90_000,
        1,
    );
    let row = ship_row(&ship, NOW);
    assert_eq!(row.as_ref().map(|r| r.label.as_str()), Some("DEAB"));
    assert_eq!(row.as_ref().map(|r| r.primary.as_str()), Some("12 kt"));
    assert_eq!(
        row.as_ref().map(|r| r.secondary.as_str()),
        Some("0° · HAMBURG")
    );
    assert_eq!(row.as_ref().map(|r| r.age_ms), Some(90_000));
}

fn target(id: &str, age_ms: i64) -> TargetRow {
    TargetRow {
        id: id.to_owned(),
        label: String::new(),
        primary: String::new(),
        secondary: String::new(),
        position: String::new(),
        age_ms,
        frames: 1,
    }
}

fn ids(rows: &[TargetRow]) -> Vec<&str> {
    rows.iter().map(|row| row.id.as_str()).collect()
}

#[test]
fn targets_sort_by_age_or_by_identity_length_then_text() {
    let rows = [
        target("3C6444", 5_000),
        target("0A0001", 40_000),
        target("FFFFFF", 1_000),
    ];
    assert_eq!(
        ids(&sort_targets(&rows, TargetSort::Age, false)),
        ["FFFFFF", "3C6444", "0A0001"]
    );
    assert_eq!(
        ids(&sort_targets(&rows, TargetSort::Age, true)),
        ["0A0001", "3C6444", "FFFFFF"]
    );
    assert_eq!(
        ids(&sort_targets(&rows, TargetSort::Id, false)),
        ["0A0001", "3C6444", "FFFFFF"]
    );
    let mmsis = [target("9", 0), target("211234560", 0), target("100", 0)];
    assert_eq!(
        ids(&sort_targets(&mmsis, TargetSort::Id, false)),
        ["9", "100", "211234560"]
    );
}

#[test]
fn altitudes_group_and_speeds_and_bearings_round() {
    assert_eq!(format_altitude_ft(None), "-");
    assert_eq!(format_altitude_ft(Some(900.0)), "900 ft");
    assert_eq!(format_altitude_ft(Some(37_000.0)), "37,000 ft");
    assert_eq!(format_altitude_ft(Some(-1_200.0)), "−1,200 ft");
    assert_eq!(format_speed_kt(None), "-");
    assert_eq!(format_speed_kt(Some(12.6)), "13 kt");
    assert_eq!(format_bearing(None), "");
    assert_eq!(format_bearing(Some(359.7)), "0°");
    assert_eq!(format_bearing(Some(-90.0)), "270°");
    assert_eq!(format_position(Some(52.52), None), "-");
}

#[test]
fn an_unparsable_stamp_has_a_placeholder_clock() {
    assert_eq!(format_clock("not a date"), "--:--:--");
    assert_eq!(format_clock("2026-08-09T12:34:56Z").len(), 8);
}

fn rds(data: Value) -> Arc<DecodedRecord> {
    record("rds", data)
}

#[test]
fn rds_folds_forward_without_a_later_frame_erasing_a_field() {
    let records = [
        rds(json!({"groups": 100, "blocks": 401, "block_errors": 1, "radiotext": "Now playing"})),
        rds(json!({"groups": 50, "blocks": 200, "block_errors": 0, "ps": "RADIO 1", "pi": "D389"})),
    ];
    let picture = rds_picture(&records).unwrap_or_default();
    assert_eq!(picture.groups, 100);
    assert_eq!(picture.ps.as_deref(), Some("RADIO 1"));
    assert_eq!(picture.pi.as_deref(), Some("D389"));
    assert_eq!(picture.radiotext.as_deref(), Some("Now playing"));
    assert_eq!(rds_picture(&[]), None);
}

#[test]
fn rds_drops_what_a_retune_left_behind() {
    let records = [
        rds(json!({"groups": 4, "blocks": 16, "block_errors": 0, "pi": "D392", "ps": "WDR 2"})),
        rds(
            json!({"groups": 900, "blocks": 3600, "block_errors": 0, "pi": "D3A3", "radiotext": "SWR3 news"}),
        ),
    ];
    let picture = rds_picture(&records).unwrap_or_default();
    assert_eq!(picture.radiotext, None);
    assert_eq!(picture.ps.as_deref(), Some("WDR 2"));
    let unnamed = [
        rds(json!({"groups": 4, "blocks": 16, "block_errors": 0, "ps": "WDR 2"})),
        rds(json!({"groups": 900, "blocks": 3600, "block_errors": 0, "radiotext": "SWR3 news"})),
    ];
    assert_eq!(rds_picture(&unnamed).and_then(|p| p.radiotext), None);
}

fn quality(groups: u64, blocks: u64, block_errors: u64) -> RdsQuality {
    rds_quality(&RdsUpdate {
        groups,
        blocks,
        block_errors,
        ..RdsUpdate::default()
    })
}

#[test]
fn rds_grades_errors_against_every_block_read() {
    assert_eq!(quality(0, 0, 0).grade, RdsGrade::NoLock);
    assert_eq!(quality(1000, 4010, 10).grade, RdsGrade::Good);
    assert_eq!(quality(1000, 4200, 200).grade, RdsGrade::Fair);
    assert_eq!(quality(100, 600, 200).grade, RdsGrade::Poor);
    assert_eq!(quality(1000, 4000, 0).error_rate, 0.0);
    assert!((quality(100, 0, 200).error_rate - 200.0 / 600.0).abs() < 1e-6);
}

#[test]
fn rds_names_the_programme_and_sorts_alternatives() {
    let mut update = RdsUpdate {
        pty: Some(10),
        pty_name: Some("Pop Music".to_owned()),
        ..RdsUpdate::default()
    };
    assert_eq!(pty_label(&update), "Pop Music");
    update.pty_name = None;
    assert_eq!(pty_label(&update), "PTY 10");
    assert_eq!(pty_label(&RdsUpdate::default()), "-");
    assert_eq!(
        format_alt_freqs(&[100_300_000.0, 98_500_000.0]),
        ["98.5 MHz", "100.3 MHz"]
    );
}

#[test]
fn a_transcript_drops_its_head_at_a_line_boundary() {
    assert_eq!(append_transcript("ab", "cd", 10), "abcd");
    assert_eq!(append_transcript("abcdef", "gh", 4), "efgh");
    assert_eq!(
        append_transcript("one\ntwo\nthree\n", "four\n", 12),
        "three\nfour\n"
    );
}

#[test]
fn a_transcript_reads_oldest_first() {
    let records = [
        record("rtty", json!({"text": "CQ "})),
        record("rtty", json!({"text": "TEST "})),
    ];
    assert_eq!(build_transcript(&records, TRANSCRIPT_LIMIT), "TEST CQ ");
    assert_eq!(build_transcript(&[], TRANSCRIPT_LIMIT), "");
    let morse = [
        record("morse", json!({"text": "E", "wpm": 22.0})),
        record("morse", json!({"text": "T", "wpm": 18.0})),
    ];
    assert_eq!(latest_wpm(&morse), Some(22.0));
    assert_eq!(latest_wpm(&[]), None);
}

fn spot(offset_hz: f64, text: &str, wpm: f64, snr_db: f64) -> Arc<DecodedRecord> {
    record(
        "cw_skimmer",
        json!({"offset_hz": offset_hz, "text": text, "wpm": wpm, "snr_db": snr_db}),
    )
}

#[test]
fn cw_spots_group_by_carrier_in_arrival_order() {
    let rows = cw_signal_rows(
        &[
            spot(4_210.0, "K", 27.0, 14.0),
            spot(-3_500.0, "DE DL1AAA ", 18.0, 21.0),
            spot(4_180.0, "CQ ", 26.0, 15.0),
            spot(-3_480.0, "CQ ", 17.0, 20.0),
        ],
        CW_SPOT_TEXT_LIMIT,
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].offset_hz, -3_500.0);
    assert_eq!(rows[0].frequency_hz, 1_090_000_000.0 - 3_500.0);
    assert_eq!(rows[0].text, "CQ DE DL1AAA ");
    assert_eq!(rows[1].offset_hz, 4_210.0);
    assert_eq!(rows[1].text, "CQ K");
    let capped = cw_signal_rows(&[spot(0.0, "ABCDEF", 20.0, 9.0)], 4);
    assert_eq!(capped.first().map(|row| row.text.as_str()), Some("CDEF"));
    let apart = cw_signal_rows(
        &[spot(700.0, "TU", 20.0, 9.0), spot(600.0, "VVV", 21.0, 8.0)],
        4,
    );
    assert_eq!(
        apart.iter().map(|row| row.offset_hz).collect::<Vec<_>>(),
        [600.0, 700.0]
    );
    assert!(cw_signal_rows(&[], CW_SPOT_TEXT_LIMIT).is_empty());
}

#[test]
fn a_near_bottom_scroll_counts_as_bottom() {
    assert!(is_at_bottom(900.0, 1000.0, 100.0));
    assert!(is_at_bottom(895.0, 1000.0, 100.0));
    assert!(!is_at_bottom(500.0, 1000.0, 100.0));
}

#[test]
fn a_tone_reads_the_way_a_radio_names_it() {
    assert_eq!(tone_label(None, None), "");
    assert_eq!(tone_label(Some(88.5), None), "CTCSS 88.5 Hz");
    assert_eq!(tone_label(Some(100.0), None), "CTCSS 100.0 Hz");
    assert_eq!(tone_label(None, Some(23)), "DCS 023");
    assert_eq!(tone_label(Some(88.5), Some(23)), "CTCSS 88.5 Hz · DCS 023");
}

fn dv(mode: DvMode, color_code: Option<u16>, slot: Option<u8>) -> DvFrame {
    DvFrame {
        color_code,
        slot,
        ..DvFrame::new(mode, sdrmm_wire::decode::DvFrameKind::Header)
    }
}

#[test]
fn a_network_reads_the_way_each_mode_publishes_it() {
    assert_eq!(dv_network(&dv(DvMode::Dmr, Some(1), Some(2))), "TS2 CC 1");
    assert_eq!(dv_network(&dv(DvMode::P25, Some(0x293), None)), "NAC 293");
    assert_eq!(dv_network(&dv(DvMode::Nxdn, Some(5), None)), "RAN 5");
    assert_eq!(dv_network(&dv(DvMode::M17, None, None)), "");
}

#[test]
fn modulation_names_the_sideband_it_found() {
    assert_eq!(modulation_label(Modulation::Fsk2, None), "2-FSK");
    assert_eq!(
        modulation_label(Modulation::Ssb, Some(Sideband::Usb)),
        "SSB (USB)"
    );
    let confirmed = ProtocolMatch {
        name: "POCSAG".to_owned(),
        type_id: None,
        score: 0.9,
        confirmed: true,
        why: String::new(),
    };
    assert_eq!(candidate_score(&confirmed), "confirmed");
    assert_eq!(
        candidate_score(&ProtocolMatch {
            confirmed: false,
            ..confirmed
        }),
        "90%"
    );
}
