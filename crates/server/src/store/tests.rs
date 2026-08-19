use sdrmm_wire::{
    AdsbMessage, AprsPacket, ChannelParams, ChannelSettings, DecoderEvent, DeviceSettings,
    NfmParams, PRESET_SNAPSHOT_VERSION, PresetDevice,
};

use super::*;

fn snapshot() -> PresetSnapshot {
    PresetSnapshot {
        version: PRESET_SNAPSHOT_VERSION,
        devices: vec![PresetDevice {
            node: "device".to_string(),
            device_id: "virtual:siggen".to_string(),
            settings: DeviceSettings {
                center_hz: Some(100_000_000.0),
                sample_rate: Some(2_048_000.0),
                ..DeviceSettings::default()
            },
            channels: vec![ChannelSettings {
                offset_hz: 100_000.0,
                squelch_db: Some(-60.0),
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            }],
        }],
    }
}

#[test]
fn migration_is_idempotent() {
    let conn = Connection::open_in_memory().expect("open");
    migrate(&conn).expect("first migrate");
    migrate(&conn).expect("second migrate");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, MIGRATIONS.len() as i64);
}

#[test]
fn preset_crud_roundtrip() {
    let store = Store::open(None).expect("open");
    assert!(store.list_presets().expect("list").is_empty());

    let snap = snapshot();
    let id = store.create_preset("fm broadcast", &snap).expect("create");
    let listed = store.list_presets().expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].name, "fm broadcast");
    assert_eq!(listed[0].devices, 1);
    assert!(
        listed[0].created_at.ends_with('Z'),
        "{}",
        listed[0].created_at
    );
    listed[0]
        .created_at
        .parse::<jiff::Timestamp>()
        .expect("rfc3339 timestamp");

    assert_eq!(store.preset_snapshot(id).expect("snapshot"), snap);

    store.delete_preset(id).expect("delete");
    assert!(store.list_presets().expect("list").is_empty());
    assert!(matches!(
        store.delete_preset(id),
        Err(StoreError::PresetNotFound(_))
    ));
    assert!(matches!(
        store.preset_snapshot(id),
        Err(StoreError::PresetNotFound(_))
    ));
}

#[test]
fn bookmark_crud_roundtrip() {
    let store = Store::open(None).expect("open");
    assert!(store.list_bookmarks().expect("list").is_empty());

    let id = store
        .create_bookmark(&CreateBookmarkRequest {
            label: "tower".to_string(),
            freq_hz: 118_700_000.0,
            mode: Some("am".to_string()),
            group: Some("airband".to_string()),
        })
        .expect("create");
    let bare_id = store
        .create_bookmark(&CreateBookmarkRequest {
            label: "repeater".to_string(),
            freq_hz: 439_000_000.0,
            mode: None,
            group: None,
        })
        .expect("create");

    let listed = store.list_bookmarks().expect("list");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].label, "tower");
    assert_eq!(listed[0].freq_hz, 118_700_000.0);
    assert_eq!(listed[0].mode.as_deref(), Some("am"));
    assert_eq!(listed[0].group.as_deref(), Some("airband"));
    assert_eq!(listed[1].mode, None);
    assert_eq!(listed[1].group, None);

    store.delete_bookmark(id).expect("delete");
    assert_eq!(store.list_bookmarks().expect("list").len(), 1);
    assert!(matches!(
        store.delete_bookmark(id),
        Err(StoreError::BookmarkNotFound(_))
    ));
    store.delete_bookmark(bare_id).expect("delete");
}

fn recording_row(stem: &str, samples: u64) -> RecordingRow {
    RecordingRow {
        stem: stem.to_string(),
        created_at: "2026-08-09T12:00:00Z".to_string(),
        device_label: "Signal Generator (virtual)".to_string(),
        center_hz: 100_000_000.0,
        sample_rate: 2_048_000.0,
        samples,
        bytes: samples * 8,
    }
}

#[test]
fn recording_index_upsert_list_prune_roundtrip() {
    let store = Store::open(None).expect("open");
    let dir = Path::new("/tmp/recs");
    assert!(store.list_recordings(dir).expect("list").is_empty());

    store
        .upsert_recording(&recording_row("rec_1_a", 2_048_000))
        .expect("upsert");
    store
        .upsert_recording(&recording_row("rec_1_b", 1_024_000))
        .expect("upsert");
    let listed = store.list_recordings(dir).expect("list");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].file, "rec_1_a");
    assert_eq!(
        listed[0].device_id,
        format!("virtual:file:{}", dir.join("rec_1_a").display())
    );
    assert_eq!(listed[0].duration_s, 1.0);
    assert_eq!(listed[0].bytes, 2_048_000 * 8);
    let id = listed[0].id;
    assert_eq!(store.recording_stem(id).expect("stem"), "rec_1_a");

    store
        .upsert_recording(&recording_row("rec_1_a", 4_096_000))
        .expect("upsert");
    let listed = store.list_recordings(dir).expect("list");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].samples, 4_096_000);

    store
        .prune_recordings(&["rec_1_a".to_string()])
        .expect("prune");
    let listed = store.list_recordings(dir).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].file, "rec_1_a");

    store.delete_recording(id).expect("delete");
    assert!(matches!(
        store.delete_recording(id),
        Err(StoreError::RecordingNotFound(_))
    ));
    assert!(matches!(
        store.recording_stem(id),
        Err(StoreError::RecordingNotFound(_))
    ));

    store
        .upsert_recording(&recording_row("rec_1_c", 1))
        .expect("upsert");
    store.prune_recordings(&[]).expect("prune all");
    assert!(store.list_recordings(dir).expect("list").is_empty());
}

fn adsb(icao: &str, callsign: &str) -> DecoderEvent {
    DecoderEvent::Adsb(AdsbMessage {
        icao: icao.to_string(),
        df: 17,
        callsign: Some(callsign.to_string()),
        raw: "8D3C6444".to_string(),
        ..AdsbMessage::default()
    })
}

fn aprs(source: &str, tnc2: &str) -> DecoderEvent {
    DecoderEvent::Aprs(AprsPacket {
        source: source.to_string(),
        destination: "APRS".to_string(),
        tnc2: tnc2.to_string(),
        ..AprsPacket::default()
    })
}

fn record(at: &str, device_set: u32, event: DecoderEvent) -> DecodedRecord {
    DecodedRecord {
        device_set,
        channel: 0,
        at: at.to_string(),
        freq_hz: 1_090_000_000.0,
        event,
    }
}

fn bound(workspace: i64, nodes: &HashMap<(u32, u32), String>) -> LogOrigin<'_> {
    LogOrigin {
        workspace: Some(workspace),
        nodes,
    }
}

fn active(store: &Store) -> i64 {
    store
        .active_workspace_id()
        .expect("read the active workspace")
        .expect("open seeds and activates one")
}

fn seed(store: &Store) {
    store
        .insert_decoder_events(
            &[
                record("2026-08-09T12:00:00Z", 0, adsb("3C6444", "DLH123")),
                record(
                    "2026-08-09T12:00:01Z",
                    1,
                    aprs("DL1ABC-9", "DL1ABC-9>APRS:hi"),
                ),
                record("2026-08-09T12:00:02Z", 0, adsb("4CA2D4", "RYR9AB")),
            ],
            &LogOrigin::unattributed(),
        )
        .expect("insert");
}

fn query(store: &Store, filter: DecoderLogQuery) -> (Vec<DecoderLogEntry>, u64) {
    store.query_decoder_log(&filter).expect("query")
}

#[test]
fn decoder_log_insert_and_query_newest_first() {
    let store = Store::open(None).expect("open");
    assert_eq!(
        store
            .insert_decoder_events(&[], &LogOrigin::unattributed())
            .expect("empty"),
        0
    );
    seed(&store);

    let (entries, total) = query(&store, DecoderLogQuery::default());
    assert_eq!(total, 3);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].summary, "4CA2D4 · RYR9AB");
    assert_eq!(entries[0].kind, "adsb");
    assert_eq!(entries[0].station.as_deref(), Some("4CA2D4"));
    assert_eq!(entries[0].freq_hz, 1_090_000_000.0);
    assert_eq!(entries[0].device_set, 0);
    assert_eq!(entries[0].event, adsb("4CA2D4", "RYR9AB"));
    assert_eq!(entries[1].kind, "aprs");
    assert_eq!(entries[2].station.as_deref(), Some("3C6444"));
    assert_eq!(entries[2].at, "2026-08-09T12:00:00.000000000Z");
}

#[test]
fn decoder_log_filters_compose() {
    let store = Store::open(None).expect("open");
    seed(&store);

    let by_kind = query(
        &store,
        DecoderLogQuery {
            kind: Some("aprs".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(by_kind.1, 1);
    assert_eq!(by_kind.0[0].station.as_deref(), Some("DL1ABC-9"));

    let by_set = query(
        &store,
        DecoderLogQuery {
            device_set: Some(1),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(by_set.1, 1);
    assert_eq!(by_set.0[0].kind, "aprs");

    let since = query(
        &store,
        DecoderLogQuery {
            since: Some("2026-08-09T12:00:01Z".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(since.1, 2);

    let until = query(
        &store,
        DecoderLogQuery {
            until: Some("2026-08-09T12:00:00Z".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(until.1, 1);
    assert_eq!(until.0[0].station.as_deref(), Some("3C6444"));

    let offset = query(
        &store,
        DecoderLogQuery {
            since: Some("2026-08-09T14:00:01+02:00".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(offset.1, 2);

    let by_station = query(
        &store,
        DecoderLogQuery {
            q: Some("dl1abc".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(by_station.1, 1);
    let by_summary = query(
        &store,
        DecoderLogQuery {
            q: Some("ryr9".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(by_summary.1, 1);

    let literal = query(
        &store,
        DecoderLogQuery {
            q: Some("%".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(literal.1, 0);

    let combined = query(
        &store,
        DecoderLogQuery {
            kind: Some("adsb".to_string()),
            device_set: Some(0),
            since: Some("2026-08-09T12:00:01Z".to_string()),
            q: Some("ryr".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(combined.1, 1);
    assert_eq!(combined.0[0].station.as_deref(), Some("4CA2D4"));

    let contradictory = query(
        &store,
        DecoderLogQuery {
            kind: Some("adsb".to_string()),
            device_set: Some(1),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(contradictory.1, 0);
    assert!(contradictory.0.is_empty());
}

#[test]
fn a_kind_list_narrows_the_log_to_those_kinds() {
    let store = Store::open(None).expect("open");
    seed(&store);

    let none = query(
        &store,
        DecoderLogQuery {
            kinds: Some(String::new()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(none.1, 3, "an empty list places no restriction");

    let one = query(
        &store,
        DecoderLogQuery {
            kinds: Some("aprs".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(one.1, 1);
    assert_eq!(one.0[0].kind, "aprs");

    let both = query(
        &store,
        DecoderLogQuery {
            kinds: Some("aprs,adsb".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(both.1, 3);

    let absent = query(
        &store,
        DecoderLogQuery {
            kinds: Some("call".to_string()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(absent.1, 0, "nothing stored is a call");
}

#[test]
fn decoder_log_sources_filter_names_channels_not_device_sets() {
    let store = Store::open(None).expect("open");
    let now = now_rfc3339();
    let on = |device_set: u32, channel: u32, icao: &str| DecodedRecord {
        channel,
        ..record(&now, device_set, adsb(icao, "FLIGHT"))
    };
    store
        .insert_decoder_events(
            &[on(0, 1, "AAAAAA"), on(0, 2, "BBBBBB"), on(1, 1, "CCCCCC")],
            &LogOrigin::unattributed(),
        )
        .expect("insert");
    let stations = |sources: &str| {
        query(
            &store,
            DecoderLogQuery {
                sources: Some(sources.to_owned()),
                ..DecoderLogQuery::default()
            },
        )
        .0
        .into_iter()
        .filter_map(|entry| entry.station)
        .collect::<Vec<_>>()
    };
    assert_eq!(stations("0:1"), ["AAAAAA"]);
    assert_eq!(stations("1:1"), ["CCCCCC"]);
    assert_eq!(stations("0:2,1:1"), ["CCCCCC", "BBBBBB"]);

    assert!(stations("").is_empty());
    let empty = DecoderLogQuery {
        sources: Some(String::new()),
        ..DecoderLogQuery::default()
    };
    assert_eq!(store.delete_decoder_log(&empty).expect("clear"), 0);
    assert_eq!(query(&store, DecoderLogQuery::default()).1, 3);

    let malformed = DecoderLogQuery {
        sources: Some("0:1,nonsense".to_owned()),
        ..DecoderLogQuery::default()
    };
    assert!(matches!(
        store.query_decoder_log(&malformed),
        Err(StoreError::Sources(_))
    ));
    assert!(matches!(
        store.delete_decoder_log(&malformed),
        Err(StoreError::Sources(_))
    ));
}

#[test]
fn a_trimmed_fraction_never_outsorts_a_later_timestamp() {
    let trimmed = jiff::Timestamp::from_nanosecond(1_000_000_000_981_200_000).expect("ts");
    let later = jiff::Timestamp::from_nanosecond(1_000_000_000_981_250_000).expect("ts");
    assert!(trimmed < later);
    assert!(rfc3339(trimmed) < rfc3339(later));
    assert_eq!(
        normalize_timestamp(&rfc3339(trimmed)).expect("normalize"),
        rfc3339(trimmed),
        "a stored timestamp and the run start it is compared against must agree"
    );
    assert_eq!(now_rfc3339().len(), rfc3339(trimmed).len());
}

#[test]
fn decoder_log_scope_prefers_the_node_over_the_reused_channel_id() {
    let store = Store::open(None).expect("open");
    let workspace = active(&store);
    let now = now_rfc3339();
    let on = |channel: u32, icao: &str| DecodedRecord {
        channel,
        ..record(&now, 0, adsb(icao, "FLIGHT"))
    };
    store
        .insert_decoder_events(
            &[on(1, "AAAAAA")],
            &bound(
                workspace,
                &HashMap::from([((0, 1), "channel:old".to_owned())]),
            ),
        )
        .expect("insert");
    store
        .insert_decoder_events(
            &[on(1, "BBBBBB")],
            &bound(
                workspace,
                &HashMap::from([((0, 1), "channel:new".to_owned())]),
            ),
        )
        .expect("insert");
    store
        .insert_decoder_events(&[on(1, "LEGACY")], &LogOrigin::unattributed())
        .expect("insert");

    let stations = |nodes: &str, sources: &str| {
        query(
            &store,
            DecoderLogQuery {
                nodes: Some(nodes.to_owned()),
                sources: Some(sources.to_owned()),
                ..DecoderLogQuery::default()
            },
        )
        .0
        .into_iter()
        .filter_map(|entry| entry.station)
        .collect::<Vec<_>>()
    };

    assert_eq!(stations("channel:new", "0:1"), ["LEGACY", "BBBBBB"]);
    assert_eq!(stations("channel:old", "0:1"), ["LEGACY", "AAAAAA"]);
    assert_eq!(stations("channel:new", ""), ["BBBBBB"]);
    assert_eq!(stations("", "0:1"), ["LEGACY"]);
    assert!(stations("", "").is_empty());

    let entries = query(&store, DecoderLogQuery::default()).0;
    assert_eq!(entries[0].node, None, "the legacy row carries no node");
    assert_eq!(entries[2].node.as_deref(), Some("channel:old"));
}

#[test]
fn decoder_log_scope_fallback_stops_at_the_start_of_this_run() {
    let store = Store::open(None).expect("open");
    let on = |at: &str, icao: &str| DecodedRecord {
        channel: 1,
        ..record(at, 0, adsb(icao, "FLIGHT"))
    };
    store
        .insert_decoder_events(
            &[
                on("2026-08-09T12:00:00Z", "LASTRUN"),
                on(&now_rfc3339(), "THISRUN"),
            ],
            &LogOrigin::unattributed(),
        )
        .expect("insert");

    let scoped = query(
        &store,
        DecoderLogQuery {
            sources: Some("0:1".to_owned()),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(
        scoped
            .0
            .into_iter()
            .filter_map(|entry| entry.station)
            .collect::<Vec<_>>(),
        ["THISRUN"]
    );
    assert_eq!(query(&store, DecoderLogQuery::default()).1, 2);
}

#[test]
fn decoder_log_scope_does_not_cross_workspaces_sharing_a_node_id() {
    let store = Store::open(None).expect("open");
    let first = active(&store);
    let second = store
        .create_workspace("second", &WorkspaceSnapshot::starter())
        .expect("create");
    let now = now_rfc3339();
    let nodes = HashMap::from([((0, 1), "ch0".to_owned())]);
    let on = |icao: &str| DecodedRecord {
        channel: 1,
        ..record(&now, 0, adsb(icao, "FLIGHT"))
    };
    store
        .insert_decoder_events(&[on("FIRSTWS")], &bound(first, &nodes))
        .expect("insert");
    store
        .insert_decoder_events(&[on("SECONDWS")], &bound(second, &nodes))
        .expect("insert");

    let stations = || {
        query(
            &store,
            DecoderLogQuery {
                nodes: Some("ch0".to_owned()),
                ..DecoderLogQuery::default()
            },
        )
        .0
        .into_iter()
        .filter_map(|entry| entry.station)
        .collect::<Vec<_>>()
    };
    assert_eq!(stations(), ["FIRSTWS"]);
    store.activate_workspace(second).expect("activate");
    assert_eq!(stations(), ["SECONDWS"]);
}

#[test]
fn decoder_log_limit_bounds_the_page_but_not_the_total() {
    let store = Store::open(None).expect("open");
    seed(&store);

    let (entries, total) = query(
        &store,
        DecoderLogQuery {
            limit: Some(1),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(total, 3);
    assert_eq!(entries[0].station.as_deref(), Some("4CA2D4"));

    let (entries, _) = query(
        &store,
        DecoderLogQuery {
            limit: Some(u32::MAX),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(entries.len(), 3);
}

#[test]
fn decoder_log_serves_the_largest_page_the_panel_offers() {
    const PANEL_MAX: u32 = 2_000;
    let store = Store::open(None).expect("open");
    let records: Vec<DecodedRecord> = (0..=PANEL_MAX)
        .map(|i| {
            record(
                "2026-08-09T12:00:00Z",
                0,
                adsb("3C6444", &format!("DLH{i:04}")),
            )
        })
        .collect();
    store
        .insert_decoder_events(&records, &LogOrigin::unattributed())
        .expect("insert");

    let (entries, total) = query(
        &store,
        DecoderLogQuery {
            limit: Some(PANEL_MAX),
            ..DecoderLogQuery::default()
        },
    );
    assert_eq!(entries.len(), PANEL_MAX as usize);
    assert_eq!(total, u64::from(PANEL_MAX) + 1);
}

#[test]
fn decoder_log_export_ignores_limit_and_caps() {
    let store = Store::open(None).expect("open");
    seed(&store);
    let exported = store
        .export_decoder_log(&DecoderLogQuery {
            limit: Some(1),
            ..DecoderLogQuery::default()
        })
        .expect("export");
    assert_eq!(exported.len(), 3);
    const { assert!(DECODER_LOG_EXPORT_MAX > DECODER_LOG_LIMIT_MAX) };
}

#[test]
fn decoder_log_delete_applies_the_filter() {
    let store = Store::open(None).expect("open");
    seed(&store);

    let deleted = store
        .delete_decoder_log(&DecoderLogQuery {
            kind: Some("adsb".to_string()),
            ..DecoderLogQuery::default()
        })
        .expect("delete");
    assert_eq!(deleted, 2);
    let (entries, total) = query(&store, DecoderLogQuery::default());
    assert_eq!(total, 1);
    assert_eq!(entries[0].kind, "aprs");

    assert_eq!(
        store
            .delete_decoder_log(&DecoderLogQuery::default())
            .expect("clear"),
        1
    );
    assert_eq!(query(&store, DecoderLogQuery::default()).1, 0);
}

#[test]
fn decoder_log_prune_keeps_the_newest_rows() {
    let store = Store::open(None).expect("open");
    let records: Vec<DecodedRecord> = (0..10)
        .map(|i| {
            record(
                &format!("2026-08-09T12:00:{i:02}Z"),
                0,
                adsb(&format!("00000{i}"), "X"),
            )
        })
        .collect();
    assert_eq!(
        store
            .insert_decoder_events(&records, &LogOrigin::unattributed())
            .expect("insert"),
        10
    );

    assert_eq!(store.prune_decoder_log(10).expect("prune"), 0);
    assert_eq!(store.prune_decoder_log(4).expect("prune"), 6);
    let (entries, total) = query(&store, DecoderLogQuery::default());
    assert_eq!(total, 4);
    assert_eq!(entries[0].station.as_deref(), Some("000009"));
    assert_eq!(entries[3].station.as_deref(), Some("000006"));

    assert_eq!(store.prune_decoder_log(0).expect("prune"), 4);
    assert_eq!(query(&store, DecoderLogQuery::default()).1, 0);
}

#[test]
fn decoder_log_rejects_a_malformed_time_bound() {
    let store = Store::open(None).expect("open");
    seed(&store);
    for filter in [
        DecoderLogQuery {
            since: Some("yesterday".to_string()),
            ..DecoderLogQuery::default()
        },
        DecoderLogQuery {
            until: Some("2026-13-40".to_string()),
            ..DecoderLogQuery::default()
        },
    ] {
        assert!(matches!(
            store.query_decoder_log(&filter),
            Err(StoreError::Timestamp(_))
        ));
        assert!(matches!(
            store.delete_decoder_log(&filter),
            Err(StoreError::Timestamp(_))
        ));
    }
}

#[test]
fn a_fresh_database_is_seeded_with_one_active_workspace() {
    let store = Store::open(None).expect("open");
    let listed = store.list_workspaces().expect("list");
    assert_eq!(listed.workspaces.len(), 1);
    assert_eq!(listed.workspaces[0].name, "Workspace");
    assert_eq!(listed.workspaces[0].revision, 1);
    assert_eq!(listed.workspaces[0].nodes, 3);
    assert_eq!(listed.active, Some(listed.workspaces[0].id));

    let active = store.active_workspace().expect("active").expect("seeded");
    assert_eq!(active.snapshot, WorkspaceSnapshot::starter());

    drop(store);
}

#[test]
fn adding_the_origin_columns_keeps_the_rows_already_logged() {
    let file = tempfile::NamedTempFile::new().expect("temp db");
    {
        let conn = Connection::open(file.path()).expect("open");
        let created = MIGRATIONS
            .iter()
            .position(|migration| migration.contains("CREATE TABLE decoder_log"))
            .expect("the log has a migration");
        for (i, migration) in MIGRATIONS.iter().take(created + 1).enumerate() {
            conn.execute_batch(&format!(
                "BEGIN;\n{migration}\nPRAGMA user_version = {};\nCOMMIT;",
                i + 1
            ))
            .expect("migrate");
        }
        conn.execute(
            "INSERT INTO decoder_log (at, device_set, channel, kind, freq_hz, station, \
                 summary, event) VALUES ('2026-08-09T12:00:00.000000000Z', 0, 1, 'adsb', \
                 1090000000.0, 'LEGACY', 'LEGACY', '{\"kind\":\"adsb\",\"data\":{\"icao\":\
                 \"LEGACY\",\"df\":17,\"raw\":\"8d\"}}')",
            [],
        )
        .expect("a row from before the columns");
    }

    let store = Store::open(Some(file.path())).expect("reopen");
    let (entries, total) = query(&store, DecoderLogQuery::default());
    assert_eq!(total, 1, "the upgrade kept the row");
    assert_eq!(entries[0].node, None);
    assert_eq!(
        store
            .export_decoder_log(&DecoderLogQuery::default())
            .expect("export")
            .len(),
        1,
        "and the export still reaches it"
    );

    let scoped = |nodes: &str, sources: &str| {
        query(
            &store,
            DecoderLogQuery {
                nodes: Some(nodes.to_owned()),
                sources: Some(sources.to_owned()),
                ..DecoderLogQuery::default()
            },
        )
        .1
    };
    assert_eq!(scoped("channel:whatever", "0:1"), 0);
    assert_eq!(scoped("channel:whatever", ""), 0);
}

#[test]
fn the_canvas_migration_clears_m6_workspaces_and_re_seeds() {
    let file = tempfile::NamedTempFile::new().expect("temp db");
    {
        let conn = Connection::open(file.path()).expect("open");
        for (i, migration) in MIGRATIONS.iter().take(4).enumerate() {
            conn.execute_batch(&format!(
                "BEGIN;\n{migration}\nPRAGMA user_version = {};\nCOMMIT;",
                i + 1
            ))
            .expect("migrate");
        }
        conn.execute(
            "INSERT INTO workspaces (name, created_at, updated_at, revision, tabs, snapshot) \
                 VALUES ('Old', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 7, 2, \
                 '{\"version\":1,\"tabs\":[]}')",
            [],
        )
        .expect("an M6 row");
        conn.execute("UPDATE active_workspace SET workspace_id = 1", [])
            .expect("active");
    }

    let store = Store::open(Some(file.path())).expect("reopen");
    let listed = store.list_workspaces().expect("list");
    assert_eq!(listed.workspaces.len(), 1, "the M6 row is gone");
    assert_eq!(listed.workspaces[0].name, "Workspace");
    assert_eq!(listed.active, Some(listed.workspaces[0].id));
    assert_eq!(
        store
            .active_workspace()
            .expect("active")
            .expect("seeded")
            .snapshot,
        WorkspaceSnapshot::starter()
    );
}

#[test]
fn a_stored_call_buffer_is_folded_into_its_dmr_system() {
    let mut value = serde_json::to_value(WorkspaceSnapshot::starter()).expect("snapshot");
    let graph = value
        .get_mut("graph")
        .and_then(serde_json::Value::as_object_mut)
        .expect("graph");
    let nodes = graph
        .get_mut("nodes")
        .and_then(serde_json::Value::as_array_mut)
        .expect("nodes");
    nodes.extend([
        serde_json::json!({
            "id": "carrier",
            "position": { "x": 0.0, "y": 0.0 },
            "kind": "channel",
            "data": { "channel_type": "dmr" }
        }),
        serde_json::json!({
            "id": "system",
            "position": { "x": 100.0, "y": 0.0 },
            "kind": "dmr_trunk",
            "data": { "protocol": "auto" }
        }),
        serde_json::json!({
            "id": "buffer",
            "position": { "x": 200.0, "y": 0.0 },
            "kind": "call_buffer",
            "data": { "record_audio": false, "retention_seconds": 900 }
        }),
    ]);
    let edges = graph
        .get_mut("edges")
        .and_then(serde_json::Value::as_array_mut)
        .expect("edges");
    edges.extend([
        serde_json::json!({
            "from": { "node": "carrier", "port": "events" },
            "to": { "node": "system", "port": "carriers" }
        }),
        serde_json::json!({
            "from": { "node": "system", "port": "trunk_events" },
            "to": { "node": "buffer", "port": "trunk_events" }
        }),
        serde_json::json!({
            "from": { "node": "system", "port": "trunk_audio" },
            "to": { "node": "buffer", "port": "trunk_audio" }
        }),
    ]);

    let migrated = parse_workspace_snapshot(&value.to_string()).expect("migrated");
    migrated.validate().expect("valid");
    assert!(migrated.graph.node("buffer").is_none());
    let system = migrated.graph.node("system").expect("system");
    let sdrmm_wire::NodeBody::DmrTrunk(settings) = &system.body else {
        panic!("DMR system");
    };
    assert!(settings.record_calls);
    assert!(
        !migrated
            .graph
            .edges
            .iter()
            .any(|edge| edge.to.node == "system"),
        "a wire into a system that decodes for itself survived"
    );
}

#[test]
fn workspace_crud_roundtrip() {
    let store = Store::open(None).expect("open");
    let seeded = store.list_workspaces().expect("list").workspaces[0].id;

    let snapshot = WorkspaceSnapshot::starter();
    let id = store.create_workspace("Bench", &snapshot).expect("create");
    let listed = store.list_workspaces().expect("list");
    assert_eq!(listed.workspaces.len(), 2);
    assert_eq!(listed.active, Some(seeded), "creating does not activate");

    store.activate_workspace(id).expect("activate");
    assert_eq!(store.list_workspaces().expect("list").active, Some(id));

    let detail = store.workspace(id).expect("read");
    assert_eq!(detail.snapshot, snapshot);
    assert_eq!(detail.info.revision, 1);
    assert_eq!(detail.info.created_at, detail.info.updated_at);

    let mut edited = snapshot.clone();
    edited.graph.nodes.retain(|node| node.id != "speaker");
    let info = store
        .update_workspace(
            id,
            &UpdateWorkspaceRequest {
                revision: 1,
                name: Some("Bench 2".to_string()),
                snapshot: Some(edited.clone()),
            },
        )
        .expect("update");
    assert_eq!(info.revision, 2);
    assert_eq!(info.name, "Bench 2");
    assert_eq!(info.nodes, 2);
    assert_eq!(store.workspace(id).expect("read").snapshot, edited);

    assert_eq!(store.delete_workspace(id).expect("delete"), Some(seeded));
    assert!(matches!(
        store.delete_workspace(id),
        Err(StoreError::WorkspaceNotFound(_))
    ));
    assert!(matches!(
        store.workspace(id),
        Err(StoreError::WorkspaceNotFound(_))
    ));
    assert!(matches!(
        store.activate_workspace(id),
        Err(StoreError::WorkspaceNotFound(_))
    ));

    assert_eq!(store.delete_workspace(seeded).expect("delete"), None);
    assert_eq!(store.list_workspaces().expect("list").active, None);
    assert!(store.active_workspace().expect("active").is_none());
}

fn without(node: &str) -> WorkspaceSnapshot {
    let mut snapshot = WorkspaceSnapshot::starter();
    snapshot.graph.nodes.retain(|held| held.id != node);
    snapshot
        .graph
        .edges
        .retain(|edge| edge.from.node != node && edge.to.node != node);
    snapshot
}

fn write(store: &Store, id: i64, snapshot: &WorkspaceSnapshot) -> u64 {
    let revision = store.workspace(id).expect("read").info.revision;
    store
        .update_workspace(
            id,
            &UpdateWorkspaceRequest {
                revision,
                name: None,
                snapshot: Some(snapshot.clone()),
            },
        )
        .expect("update")
        .revision
}

#[test]
fn workspace_history_walks_back_out_of_its_own_edits() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    let starter = WorkspaceSnapshot::starter();

    let fresh = store.workspace(id).expect("read");
    assert_eq!(fresh.history, WorkspaceHistory::default());
    assert!(matches!(
        store.undo_workspace(id),
        Err(StoreError::WorkspaceHistoryEnd { step: "undo", .. })
    ));

    write(&store, id, &without("speaker"));
    write(&store, id, &without("scope"));

    let back = store.undo_workspace(id).expect("undo").detail;
    assert_eq!(back.snapshot, without("speaker"));
    assert!(back.history.can_undo && back.history.can_redo);
    assert!(back.info.revision > 3);
    assert_eq!(back.info.nodes, 2);

    let base = store.undo_workspace(id).expect("undo to the start").detail;
    assert_eq!(base.snapshot, starter, "the state the first edit left");
    assert!(!base.history.can_undo && base.history.can_redo);
    assert!(matches!(
        store.undo_workspace(id),
        Err(StoreError::WorkspaceHistoryEnd { step: "undo", .. })
    ));

    assert_eq!(
        store.redo_workspace(id).expect("redo").detail.snapshot,
        without("speaker")
    );
    let forward = store.redo_workspace(id).expect("redo").detail;
    assert_eq!(forward.snapshot, without("scope"));
    assert!(!forward.history.can_redo);
    assert!(matches!(
        store.redo_workspace(id),
        Err(StoreError::WorkspaceHistoryEnd { step: "redo", .. })
    ));
}

#[test]
fn an_edit_after_an_undo_drops_what_redo_would_have_reached() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    write(&store, id, &without("speaker"));
    write(&store, id, &without("scope"));
    store.undo_workspace(id).expect("undo");

    write(&store, id, &without("device"));
    let now = store.workspace(id).expect("read");
    assert_eq!(now.snapshot, without("device"));
    assert!(now.history.can_undo && !now.history.can_redo);
    assert_eq!(
        store.undo_workspace(id).expect("undo").detail.snapshot,
        without("speaker"),
        "the branch it was made from"
    );
}

#[test]
fn a_write_that_changes_nothing_is_not_a_step() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    write(&store, id, &WorkspaceSnapshot::starter());
    assert!(!store.workspace(id).expect("read").history.can_undo);
    write(&store, id, &without("speaker"));
    write(&store, id, &without("speaker"));
    assert_eq!(
        store.undo_workspace(id).expect("undo").detail.snapshot,
        WorkspaceSnapshot::starter()
    );
}

#[test]
fn the_history_forgets_its_oldest_arrangements() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    let labelled = |at: usize| {
        let mut snapshot = WorkspaceSnapshot::starter();
        snapshot.graph.nodes[1].label = Some(format!("scope {at}"));
        snapshot
    };
    let writes = usize::try_from(WORKSPACE_HISTORY_DEPTH).expect("depth fits") + 20;
    for at in 0..writes {
        write(&store, id, &labelled(at));
    }
    let entries: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM workspace_history WHERE workspace_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(entries, WORKSPACE_HISTORY_DEPTH);

    for at in (writes - usize::try_from(WORKSPACE_HISTORY_DEPTH).expect("depth fits")..writes)
        .rev()
        .skip(1)
    {
        assert_eq!(
            store.undo_workspace(id).expect("undo").detail.snapshot,
            labelled(at)
        );
    }
    assert!(matches!(
        store.undo_workspace(id),
        Err(StoreError::WorkspaceHistoryEnd { .. })
    ));
}

fn tuned(node: &str, center_hz: f64) -> WorkspaceState {
    let mut state = WorkspaceState::new();
    state.merge(vec![sdrmm_wire::WorkspaceDevice {
        node: node.to_string(),
        settings: DeviceSettings {
            center_hz: Some(center_hz),
            ..DeviceSettings::default()
        },
        channels: Vec::new(),
    }]);
    state
}

fn dial(store: &Store, id: i64, node: &str, from: f64, to: f64) -> bool {
    store
        .record_settings(
            id,
            &SettingsStep {
                node,
                before: &tuned(node, from),
                after: &tuned(node, to),
            },
        )
        .expect("record")
}

fn center_of(state: &Option<WorkspaceState>, node: &str) -> Option<f64> {
    state.as_ref()?.device(node)?.settings.center_hz
}

#[test]
fn the_history_walks_back_out_of_a_dial_move() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;

    assert!(dial(&store, id, "device", 100e6, 101e6));
    assert!(store.workspace(id).expect("read").history.can_undo);

    let back = store.undo_workspace(id).expect("undo");
    assert_eq!(center_of(&back.settings, "device"), Some(100e6));
    assert_eq!(
        store
            .workspace_state(id)
            .expect("state")
            .device("device")
            .and_then(|device| device.settings.center_hz),
        Some(100e6),
        "the settings the step reached are the ones a restart would come back to"
    );

    let forward = store.redo_workspace(id).expect("redo");
    assert_eq!(center_of(&forward.settings, "device"), Some(101e6));
}

#[test]
fn an_arrangement_step_between_two_dial_moves_leaves_the_dial_alone() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;

    assert!(dial(&store, id, "device", 100e6, 101e6));
    write(&store, id, &without("speaker"));
    assert!(dial(&store, id, "device", 101e6, 102e6));

    let to_layout = store.undo_workspace(id).expect("undo");
    assert_eq!(center_of(&to_layout.settings, "device"), Some(101e6));

    let to_first_dial = store.undo_workspace(id).expect("undo");
    assert_eq!(to_first_dial.detail.snapshot, WorkspaceSnapshot::starter());
    assert!(
        to_first_dial.settings.is_none(),
        "an arrangement step moved a radio that had not been touched"
    );

    let to_start = store.undo_workspace(id).expect("undo");
    assert_eq!(center_of(&to_start.settings, "device"), Some(100e6));
}

#[test]
fn a_drag_of_one_dial_is_one_step_and_a_second_dial_is_another() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;

    assert!(dial(&store, id, "device", 100e6, 100.1e6));
    assert!(
        !dial(&store, id, "device", 100.1e6, 100.2e6),
        "a drag lands a patch a frame and none of them is a step of its own"
    );
    assert!(!dial(&store, id, "device", 100.2e6, 100.3e6));
    assert!(dial(&store, id, "scope", 1e6, 2e6));

    assert_eq!(
        center_of(&store.undo_workspace(id).expect("undo").settings, "device"),
        Some(100.3e6)
    );
    assert_eq!(
        center_of(&store.undo_workspace(id).expect("undo").settings, "device"),
        Some(100e6),
        "the whole drag walks back at once"
    );
}

#[test]
fn walking_back_past_the_first_dial_move_leaves_the_radios_alone() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    write(&store, id, &without("speaker"));
    write(&store, id, &without("scope"));
    assert!(dial(&store, id, "device", 100e6, 101e6));

    let off_the_dial = store.undo_workspace(id).expect("undo");
    assert_eq!(center_of(&off_the_dial.settings, "device"), Some(100e6));
    for _ in 0..2 {
        assert!(
            store.undo_workspace(id).expect("undo").settings.is_none(),
            "an arrangement older than the first dial move moved a radio"
        );
    }
}

#[test]
fn a_dial_move_that_lands_where_it_started_is_not_a_step() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    assert!(!dial(&store, id, "device", 100e6, 100e6));
    assert!(!store.workspace(id).expect("read").history.can_undo);
}

#[test]
fn a_burst_coalesces_only_while_it_is_still_the_same_gesture() {
    let start = "2026-08-18T10:00:00.000000000Z";
    assert!(within_coalesce(start, "2026-08-18T10:00:00.016000000Z"));
    assert!(within_coalesce(start, "2026-08-18T10:00:01.000000000Z"));
    assert!(!within_coalesce(start, "2026-08-18T10:00:01.500000000Z"));
    assert!(!within_coalesce(start, "2026-08-18T09:59:59.000000000Z"));
    assert!(!within_coalesce("not a time", start));
}

#[test]
fn the_history_keeps_a_deleted_nodes_settings_reachable() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    assert!(store.history_nodes(id).expect("nodes").is_empty());

    write(&store, id, &without("speaker"));
    let nodes = store.history_nodes(id).expect("nodes");
    assert!(nodes.contains("speaker"), "the deleted node is recoverable");
    assert!(nodes.contains("scope"));

    store.delete_workspace(id).expect("delete");
    assert!(store.history_nodes(id).expect("nodes").is_empty());
    let rows: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM workspace_history", [], |row| {
            row.get(0)
        })
        .expect("count");
    assert_eq!(rows, 0, "deleting a workspace takes its history with it");
}

#[test]
fn workspace_update_refuses_a_stale_revision() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    let update = |revision| UpdateWorkspaceRequest {
        revision,
        name: None,
        snapshot: Some(WorkspaceSnapshot::starter()),
    };
    store.update_workspace(id, &update(1)).expect("first write");
    assert!(matches!(
        store.update_workspace(id, &update(1)),
        Err(StoreError::WorkspaceConflict {
            sent: 1,
            current: 2,
            ..
        })
    ));
    store.update_workspace(id, &update(2)).expect("fresh write");
}

#[test]
fn workspace_writes_reject_a_bad_layout_and_a_taken_name() {
    let store = Store::open(None).expect("open");
    let id = store.list_workspaces().expect("list").workspaces[0].id;
    let mut dangling = WorkspaceSnapshot::starter();
    dangling.graph.edges.push(sdrmm_wire::PatchEdge {
        from: sdrmm_wire::PortRef {
            node: "device".to_string(),
            port: "iq".to_string(),
        },
        to: sdrmm_wire::PortRef {
            node: "ghost".to_string(),
            port: "iq".to_string(),
        },
    });
    assert!(matches!(
        store.create_workspace("Broken", &dangling),
        Err(StoreError::WorkspaceLayout(WorkspaceError::Patch(_)))
    ));
    assert!(matches!(
        store.update_workspace(
            id,
            &UpdateWorkspaceRequest {
                revision: 1,
                name: None,
                snapshot: Some(dangling),
            }
        ),
        Err(StoreError::WorkspaceLayout(WorkspaceError::Patch(_)))
    ));
    assert_eq!(store.workspace(id).expect("read").info.revision, 1);

    assert!(matches!(
        store.create_workspace("Workspace", &WorkspaceSnapshot::starter()),
        Err(StoreError::WorkspaceNameTaken(_))
    ));
    for blank in [
        "",
        "   ",
        &"x".repeat(sdrmm_wire::workspace::MAX_NAME_LEN + 1),
    ] {
        assert!(matches!(
            store.create_workspace(blank, &WorkspaceSnapshot::starter()),
            Err(StoreError::WorkspaceLayout(WorkspaceError::Name))
        ));
    }
    let other = store
        .create_workspace("Bench", &WorkspaceSnapshot::starter())
        .expect("create");
    assert!(matches!(
        store.update_workspace(
            other,
            &UpdateWorkspaceRequest {
                revision: 1,
                name: Some("Workspace".to_string()),
                snapshot: None,
            }
        ),
        Err(StoreError::WorkspaceNameTaken(_))
    ));
}

#[test]
fn decoder_log_surfaces_an_unparseable_event_blob() {
    let store = Store::open(None).expect("open");
    seed(&store);
    store
        .lock()
        .execute("UPDATE decoder_log SET event = '{\"kind\":\"zzz\"}'", [])
        .expect("corrupt");
    assert!(matches!(
        store.query_decoder_log(&DecoderLogQuery::default()),
        Err(StoreError::Corrupt(_))
    ));
}

#[test]
fn a_stored_discord_output_reopens_as_a_webhook_in_the_discord_format() {
    let mut value = serde_json::to_value(WorkspaceSnapshot::starter()).expect("snapshot");
    let nodes = value
        .get_mut("graph")
        .and_then(|graph| graph.get_mut("nodes"))
        .and_then(serde_json::Value::as_array_mut)
        .expect("nodes");
    nodes.extend([
        serde_json::json!({
            "id": "discord",
            "position": { "x": 0.0, "y": 0.0 },
            "kind": "chat_output",
            "data": { "target": {
                "service": "discord",
                "webhook_url": "https://discord.com/api/webhooks/1/token"
            }}
        }),
        serde_json::json!({
            "id": "matrix",
            "position": { "x": 100.0, "y": 0.0 },
            "kind": "chat_output",
            "data": { "target": {
                "service": "matrix",
                "homeserver_url": "https://matrix.example",
                "room_id": "!radio:matrix.example",
                "access_token": "matrix-secret"
            }}
        }),
    ]);

    let migrated = parse_workspace_snapshot(&value.to_string()).expect("migrated");
    migrated.validate().expect("valid");

    let sdrmm_wire::NodeBody::EventOutput(discord) =
        &migrated.graph.node("discord").expect("discord").body
    else {
        panic!("event output");
    };
    assert_eq!(
        discord.target,
        sdrmm_wire::EventOutputTarget::Webhook {
            url: "https://discord.com/api/webhooks/1/token".to_owned(),
            format: sdrmm_wire::WebhookFormat::Discord,
        }
    );
    let sdrmm_wire::NodeBody::EventOutput(matrix) =
        &migrated.graph.node("matrix").expect("matrix").body
    else {
        panic!("event output");
    };
    assert!(matrix.target.configured(), "a Matrix room keeps posting");
}
