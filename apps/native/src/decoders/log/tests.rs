use serde_json::{Value, json};

use super::*;
use crate::decoders::testing::event;

const LOG: &str = "decoder_log:1";

fn adsb() -> DecoderEvent {
    event(
        "adsb",
        json!({"icao": "3c6444", "df": 17, "callsign": " DLH123 ", "altitude_ft": 35000, "raw": "8d3c6444"}),
    )
}

fn ais() -> DecoderEvent {
    event(
        "ais",
        json!({
            "mmsi": 211234560, "msg_type": 1, "ais_channel": "A", "nmea": "!AIVDM,1,1,,A,x,0*00",
            "name": " NORDLICHT ", "lat": 53.5512, "lon": 9.9937,
        }),
    )
}

fn entry(id: i64, at: &str, summary: &str) -> Arc<DecoderLogEntry> {
    Arc::new(DecoderLogEntry {
        origin: None,
        id,
        at: at.to_owned(),
        device_set: 0,
        channel: 0,
        node: None,
        kind: "adsb".to_owned(),
        freq_hz: 1_090_000_000.0,
        station: Some("3c6444".to_owned()),
        summary: summary.to_owned(),
        event: adsb(),
    })
}

fn stored(id: i64) -> Arc<DecoderLogEntry> {
    entry(id, "2026-08-09T12:00:00Z", "3c6444 · DLH123")
}

fn record_of(event: DecoderEvent, at: &str, sinks: &[&str]) -> Arc<DecodedRecord> {
    Arc::new(DecodedRecord {
        origin: None,
        device_set: 0,
        channel: 0,
        at: at.to_owned(),
        freq_hz: 1_090_000_000.0,
        event,
        sinks: sinks.iter().map(|sink| (*sink).to_owned()).collect(),
    })
}

fn record(at: &str) -> Arc<DecodedRecord> {
    record_of(adsb(), at, &[LOG, "export:1"])
}

fn filter(q: &str) -> LogFilter {
    LogFilter {
        q: q.to_owned(),
        ..LogFilter::default()
    }
}

fn summary(kind: &str, data: Value) -> String {
    event(kind, data).summary()
}

fn station(kind: &str, data: Value) -> Option<String> {
    event(kind, data).station()
}

#[test]
fn every_kind_has_a_name_and_unknown_ones_shout() {
    assert_eq!(kind_label("adsb"), "ADS-B");
    assert_eq!(kind_label("navtex"), "NAVTEX");
    assert_eq!(kind_label("subghz"), "Sub-GHz");
    assert_eq!(kind_label("dmr"), "DMR");
    assert!(KIND_LABELS.iter().all(|(_, label)| !label.is_empty()));
}

#[test]
fn a_query_carries_the_sink_and_a_trimmed_search_only_when_set() {
    let blank = filter("   ").query(LOG);
    assert_eq!(blank.q, None);
    assert_eq!(blank.limit, Some(500));
    assert_eq!(blank.sink.as_deref(), Some(LOG));
    let set = LogFilter {
        q: " nord ".to_owned(),
        limit: 100,
    }
    .query(LOG);
    assert_eq!(set.q.as_deref(), Some("nord"));
    assert_eq!(set.limit, Some(100));
}

#[test]
fn a_filter_ignores_the_row_limit() {
    let wide = LogFilter {
        q: String::new(),
        limit: 100,
    };
    assert!(!wide.filtered());
    assert!(filter(" x ").filtered());
}

#[test]
fn search_reads_station_and_summary_case_insensitively() {
    let live = record("2026-08-09T12:00:01Z");
    assert!(filter("DLH").matches(&live, LOG));
    assert!(filter("3C6444").matches(&live, LOG));
    assert!(!filter("nordlicht").matches(&live, LOG));
}

#[test]
fn only_frames_that_reached_this_sink_match() {
    assert!(filter("").matches(&record("2026-08-09T12:00:01Z"), LOG));
    assert!(!filter("").matches(&record_of(adsb(), "x", &["export:1"]), LOG));
    assert!(!filter("").matches(&record_of(adsb(), "x", &[]), LOG));
}

fn decoded_with(records: Vec<Arc<DecodedRecord>>) -> Decoded {
    let mut decoded = Decoded::default();
    decoded.publish(records.into_iter().map(|r| (*r).clone()).rev().collect());
    decoded
}

fn ats(records: &[Arc<DecodedRecord>]) -> Vec<&str> {
    records.iter().map(|record| record.at.as_str()).collect()
}

#[test]
fn live_frames_merge_newest_first_under_the_filter_and_cap() {
    let decoded = decoded_with(vec![
        record("2026-08-09T12:00:03Z"),
        record("2026-08-09T12:00:01Z"),
        record_of(ais(), "2026-08-09T12:00:02Z", &[LOG]),
    ]);
    let all = collect_live(&decoded, &filter(""), LOG, LIVE_ROW_CAP);
    assert_eq!(
        ats(&all),
        [
            "2026-08-09T12:00:03Z",
            "2026-08-09T12:00:02Z",
            "2026-08-09T12:00:01Z"
        ]
    );
    assert_eq!(
        collect_live(&decoded, &filter("nordlicht"), LOG, LIVE_ROW_CAP).len(),
        1
    );
    let capped = collect_live(&decoded, &filter(""), LOG, 2);
    assert_eq!(
        ats(&capped),
        ["2026-08-09T12:00:03Z", "2026-08-09T12:00:02Z"]
    );
}

#[test]
fn an_unstamped_frame_sorts_oldest() {
    let decoded = decoded_with(vec![record("not a date"), record("2026-08-09T12:00:03Z")]);
    let all = collect_live(&decoded, &filter(""), LOG, LIVE_ROW_CAP);
    assert_eq!(all.last().map(|r| r.at.as_str()), Some("not a date"));
}

#[test]
fn live_rows_sit_above_the_stored_page() {
    let rows = build_rows(&[stored(1)], vec![record("2026-08-09T12:00:01Z")]);
    assert_eq!(
        rows.iter().map(|r| r.live).collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(rows[0].summary, "3c6444 · DLH123 · 35000 ft");
}

#[test]
fn the_tail_and_the_page_order_as_one_table() {
    let rows = build_rows(
        &[
            entry(2, "2026-08-09T12:00:04Z", "a"),
            entry(1, "2026-08-09T12:00:00Z", "b"),
        ],
        vec![
            record("2026-08-09T12:00:05Z"),
            record("2026-08-09T12:00:02Z"),
        ],
    );
    let order: Vec<&str> = rows.iter().map(|row| row.at.as_str()).collect();
    assert_eq!(
        order,
        [
            "2026-08-09T12:00:05Z",
            "2026-08-09T12:00:04Z",
            "2026-08-09T12:00:02Z",
            "2026-08-09T12:00:00Z"
        ]
    );
}

#[test]
fn a_live_frame_the_page_carries_shows_once() {
    let page = entry(1, "2026-08-09T12:00:01Z", "3c6444 · DLH123 · 35000 ft");
    let rows = build_rows(&[page], vec![record("2026-08-09T12:00:01Z")]);
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].live);
}

#[test]
fn identical_frames_at_one_instant_keep_unique_keys() {
    let rows = build_rows(
        &[stored(1), stored(2)],
        vec![
            record("2026-08-09T12:00:01Z"),
            record("2026-08-09T12:00:01Z"),
        ],
    );
    let keys: HashSet<&str> = rows.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(keys.len(), rows.len());
}

#[test]
fn rows_project_stored_verbatim_and_derive_live() {
    let mut bare = (*stored(1)).clone();
    bare.station = None;
    let row = stored_row(Arc::new(bare));
    assert_eq!(row.key, "stored:1");
    assert_eq!(row.station, None);
    assert_eq!(row.summary, "3c6444 · DLH123");
    assert!(!row.live);
    let live = live_row(record_of(ais(), "2026-08-09T12:00:01Z", &[LOG]));
    assert_eq!(live.kind, "ais");
    assert_eq!(live.station.as_deref(), Some("211234560"));
    assert_eq!(live.summary, "211234560 · NORDLICHT · 53.5512, 9.9937");
    assert!(live.live);
}

#[test]
fn a_drop_notice_stays_silent_only_when_nothing_was_lost() {
    assert_eq!(dropped_notice(0, 0), None);
    assert_eq!(
        dropped_notice(1, 0).as_deref(),
        Some("1 live frame dropped")
    );
    assert_eq!(
        dropped_notice(0, 12).as_deref(),
        Some("12 frames never reached the log")
    );
    assert_eq!(
        dropped_notice(2, 12).as_deref(),
        Some("2 live frames dropped · 12 frames never reached the log")
    );
}

#[test]
fn columns_clamp_and_resize() {
    let widths = ColumnWidths::default();
    assert_eq!(widths.0, COLUMNS.map(|(_, _, w)| w));
    assert_eq!(
        clamp_column_width(MIN_COLUMN_WIDTH - 40.0),
        MIN_COLUMN_WIDTH
    );
    assert_eq!(
        clamp_column_width(MAX_COLUMN_WIDTH + 40.0),
        MAX_COLUMN_WIDTH
    );
    assert_eq!(clamp_column_width(120.4), 120.0);
    assert_eq!(clamp_column_width(f32::NAN), MIN_COLUMN_WIDTH);
    let next = widths.resized(Column::Station, 240.0);
    assert_eq!(next.of(Column::Station), 240.0);
    assert_eq!(next.of(Column::Summary), widths.of(Column::Summary));
}

#[test]
fn column_widths_round_trip_and_survive_bad_storage() {
    let wide = ColumnWidths::default().resized(Column::Kind, 200.0);
    assert_eq!(
        ColumnWidths::read(Some(&wide.write())).of(Column::Kind),
        200.0
    );
    let flexed = ColumnWidths::default().resized(Column::Summary, 900.0);
    assert_eq!(
        ColumnWidths::read(Some(&flexed.write())),
        ColumnWidths::default()
    );
    assert_eq!(ColumnWidths::read(None), ColumnWidths::default());
    assert_eq!(
        ColumnWidths::read(Some("{not json")),
        ColumnWidths::default()
    );
    let bogus = ColumnWidths::read(Some(r#"{"kind": "wide", "station": 9000, "bogus": 12}"#));
    assert_eq!(
        bogus.of(Column::Kind),
        ColumnWidths::default().of(Column::Kind)
    );
    assert_eq!(bogus.of(Column::Station), MAX_COLUMN_WIDTH);
}

#[test]
fn both_exports_share_one_filter() {
    let query = DecoderLogQuery {
        limit: Some(200),
        sink: Some("export:1".to_owned()),
        q: Some("wx".to_owned()),
        ..DecoderLogQuery::default()
    };
    for format in ["csv", "json"] {
        let path = export_path(format, &query);
        assert!(path.contains("sink=export%3A1"));
        assert!(path.contains("q=wx"));
        assert!(path.contains(format));
    }
}

#[test]
fn summaries_match_the_server_rendering() {
    assert_eq!(
        summary(
            "aprs",
            json!({"source": "DL1ABC-9", "destination": "APRS", "info": "hi", "tnc2": "DL1ABC-9>APRS:hi"})
        ),
        "DL1ABC-9>APRS:hi"
    );
    assert_eq!(summary("rtty", json!({"text": "CQ CQ"})), "CQ CQ");
    assert_eq!(
        summary("tone", json!({"ctcss_hz": 88.5, "open": true})),
        "CTCSS 88.5 Hz · open"
    );
    assert_eq!(
        summary("tone", json!({"dcs_code": 23, "open": false})),
        "DCS 023 · muted"
    );
    assert_eq!(summary("tone", json!({"open": false})), "no tone · muted");
    assert_eq!(
        summary(
            "scrambler",
            json!({"inversion_hz": 3300.0, "confidence": 0.82})
        ),
        "inversion 3300 Hz · 82% confidence"
    );
    assert_eq!(
        summary("scrambler", json!({"confidence": 0.0})),
        "no inversion"
    );
    assert_eq!(
        summary(
            "rds",
            json!({"block_errors": 0, "blocks": 40, "groups": 10, "pi": "D3C2", "ps": "NDR2"})
        ),
        "PI D3C2 · NDR2"
    );
    assert_eq!(
        summary("rds", json!({"block_errors": 3, "blocks": 3, "groups": 0})),
        ""
    );
}

#[test]
fn pages_summarise_by_address() {
    let tone = json!({
        "address": 1234567, "baud": 1200, "errors_corrected": 0, "function": 3,
        "payload": "tone", "text": "",
    });
    assert_eq!(summary("pocsag", tone), "1234567 (3)");
    let flex = json!({
        "address": 123456, "payload": "alpha", "text": "CALL 42", "baud": 3200, "levels": 4,
        "cycle": 2, "frame": 17, "phase": "C", "errors_corrected": 1,
    });
    assert_eq!(summary("flex", flex.clone()), "123456: CALL 42");
    assert_eq!(station("flex", flex).as_deref(), Some("123456"));
}

#[test]
fn stream_decoders_and_selcall_name_no_station() {
    assert_eq!(station("rtty", json!({"text": "x"})), None);
    assert_eq!(station("morse", json!({"text": "x", "wpm": 12.0})), None);
    let selcall = json!({"system": "ccir1", "code": "12234", "tone_ms": 100});
    assert_eq!(summary("selcall", selcall.clone()), "CCIR-1 · 12234");
    assert_eq!(station("selcall", selcall), None);
    let spot = json!({"offset_hz": -742.4, "text": "CQ W1AW", "wpm": 23.6, "snr_db": 14.2});
    assert_eq!(summary("cw_skimmer", spot), "-742 Hz · 24 WPM · CQ W1AW");
}

#[test]
fn clock_gnss_navtex_and_acars_summaries() {
    let gnss =
        json!({"prn": 7, "doppler_hz": 1000.0, "code_phase_chips": 158.34, "cn0_db_hz": 44.5});
    assert_eq!(
        summary("gnss", gnss.clone()),
        "GPS PRN 7 · +1000 Hz · 44.5 dB-Hz · acquired"
    );
    assert_eq!(station("gnss", gnss).as_deref(), Some("GPS-7"));
    let navtex = json!({
        "station": "D", "subject": "A", "subject_name": "Navigational warning", "serial": 7,
        "text": "GALE WARNING\nGERMAN BIGHT", "errors_corrected": 0, "complete": true,
    });
    assert_eq!(
        summary("navtex", navtex),
        "DA07 · Navigational warning · GALE WARNING GERMAN BIGHT"
    );
    let acars = json!({
        "mode": "2", "registration": "D-AIBC", "label": "H1", "block_id": "3",
        "downlink": true, "flight": "LH0400", "text": "", "more": false,
    });
    assert_eq!(summary("acars", acars), "D-AIBC · LH0400 · [H1]");
}

#[test]
fn subghz_summaries_lead_with_the_reading() {
    let frame = json!({
        "modulation": "ook", "encoding": "pwm", "bits": 24, "data": "0A1B23",
        "address": 0xa1b2, "button": 3, "short_us": 320, "repeats": 6,
    });
    assert_eq!(
        summary("subghz", frame.clone()),
        "24 bit 0A1B23 · addr 0A1B2 · btn 3 · ×6"
    );
    assert_eq!(station("subghz", frame).as_deref(), Some("0A1B2"));
    let raw = json!({
        "modulation": "fsk", "encoding": "raw", "bits": 0, "data": "", "short_us": 250,
        "repeats": 1, "timings_us": [320, 960, 320, 960],
    });
    assert_eq!(summary("subghz", raw.clone()), "raw, 4 edges");
    assert_eq!(station("subghz", raw), None);
    let mast = json!({
        "modulation": "ook", "encoding": "pwm", "bits": 64, "data": "", "short_us": 500,
        "repeats": 1,
        "reading": {"model": "WS2032", "id": 7, "wind_avg_kmh": 12.5, "wind_dir_deg": 270.0, "rain_mm": 3.2},
    });
    let said = summary("subghz", mast);
    assert!(said.contains("wind 12.5 km/h"));
    assert!(said.contains("rain 3.2 mm"));
}
