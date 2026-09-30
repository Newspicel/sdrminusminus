use rusqlite::params;
use sdrmm_wire::{
    DroppedNode, NodeBody, WORKSPACE_SNAPSHOT_VERSION, WorkspaceNoticeKind, WorkspaceSnapshot,
};
use serde_json::{Value, json};

use super::*;
use crate::store::parse_workspace_snapshot;

fn at(x: f64, y: f64) -> Value {
    json!({ "x": x, "y": y })
}

fn kraken_df() -> Value {
    json!({
        "version": 3,
        "graph": {
            "nodes": [
                { "id": "device", "kind": "device", "data": {}, "position": at(0.0, 0.0) },
                {
                    "id": "arr", "kind": "array", "position": at(300.0, 0.0),
                    "data": { "members": 1, "coherence": "phase_coherent", "shared_tuning": true }
                },
                {
                    "id": "df1", "kind": "df", "label": "Roof DF", "position": at(600.0, 0.0),
                    "data": { "settings": { "algorithm": "music" } }
                },
                { "id": "radar", "kind": "passive_radar", "data": {}, "position": at(600.0, 400.0) },
                { "id": "scope", "kind": "scope", "position": at(300.0, 400.0) },
                { "id": "log", "kind": "decoder_log", "position": at(900.0, 0.0) }
            ],
            "edges": [
                { "from": { "node": "device", "port": "iq" }, "to": { "node": "arr", "port": "iq0" } },
                { "from": { "node": "arr", "port": "iq0" }, "to": { "node": "df1", "port": "iq0" } },
                { "from": { "node": "df1", "port": "events" }, "to": { "node": "log", "port": "events" } },
                { "from": { "node": "device", "port": "iq" }, "to": { "node": "scope", "port": "iq" } }
            ]
        },
        "rack": {
            "slots": [
                { "node": "df1", "x": 0, "y": 0, "w": 2, "h": 2 },
                { "node": "scope", "x": 2, "y": 0, "w": 2, "h": 2 }
            ]
        },
        "settings": {}
    })
}

fn with_gps_and_triangulation(mut snapshot: Value) -> Value {
    let Some(nodes) = graph_nodes(&mut snapshot) else {
        panic!("the fixture has nodes");
    };
    nodes.extend([
        json!({
            "id": "gps1", "kind": "gps", "position": at(0.0, 800.0),
            "data": { "source": { "type": "device" } }
        }),
        json!({
            "id": "gps2", "kind": "gps", "position": at(300.0, 800.0),
            "data": { "source": { "type": "fixed", "lat": 48.1, "lon": 11.5 } }
        }),
        json!({ "id": "tri", "kind": "triangulation", "position": at(600.0, 800.0) }),
    ]);
    snapshot
}

fn node_ids(snapshot: &Value) -> Vec<&str> {
    snapshot["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| node["id"].as_str())
        .collect()
}

fn plant(store: &Store, id: i64, snapshot: &Value) {
    store
        .lock()
        .execute(
            "UPDATE workspaces SET snapshot = ?2, nodes = ?3 WHERE id = ?1",
            params![id, snapshot.to_string(), node_ids(snapshot).len() as i64],
        )
        .expect("plant the old layout");
}

fn plant_state(store: &Store, id: i64, state: &Value) {
    store
        .lock()
        .execute(
            "INSERT INTO workspace_state (workspace_id, updated_at, state) VALUES (?1, 't', ?2)",
            params![id, state.to_string()],
        )
        .expect("plant the old settings");
}

fn old_state() -> Value {
    json!({
        "version": 2,
        "devices": [
            { "node": "device", "settings": { "center_hz": 433_920_000.0 } },
            { "node": "arr", "settings": { "center_hz": 433_920_000.0 } }
        ],
        "channels": [
            {
                "node": "df1",
                "settings": { "frequency_hz": 433_920_000.0, "params": { "type": "df", "settings": {} } }
            }
        ]
    })
}

fn saved_before_the_rework() -> (tempfile::NamedTempFile, i64) {
    let file = tempfile::NamedTempFile::new().expect("temp db");
    let store = Store::open(Some(file.path())).expect("open");
    let id = store
        .active_workspace_id()
        .expect("active")
        .expect("seeded");
    plant(&store, id, &with_gps_and_triangulation(kraken_df()));
    plant_state(&store, id, &old_state());
    (file, id)
}

fn dropped(id: &str, kind: &str, label: Option<&str>) -> DroppedNode {
    DroppedNode {
        id: id.to_owned(),
        kind: kind.to_owned(),
        label: label.map(str::to_owned),
    }
}

#[test]
fn upgrade_drops_old_coherent_nodes_with_their_edges_and_slots() {
    let mut snapshot = kraken_df();

    let broken = upgrade_snapshot(&mut snapshot);

    assert_eq!(
        broken.dropped,
        vec![
            dropped("arr", "array", None),
            dropped("df1", "df", Some("Roof DF")),
            dropped("radar", "passive_radar", None),
        ]
    );
    assert!(broken.cleared_gps.is_empty());
    assert_eq!(
        broken.notices(),
        vec![WorkspaceNoticeKind::DroppedNodes {
            nodes: broken.dropped.clone()
        }]
    );
    assert_eq!(node_ids(&snapshot), vec!["device", "scope", "log"]);
    assert_eq!(
        snapshot["graph"]["edges"],
        json!([{ "from": { "node": "device", "port": "iq" }, "to": { "node": "scope", "port": "iq" } }])
    );
    assert_eq!(
        snapshot["rack"]["slots"],
        json!([{ "node": "scope", "x": 2, "y": 0, "w": 2, "h": 2 }])
    );
    assert_eq!(snapshot["version"], json!(WORKSPACE_SNAPSHOT_VERSION));
    parse_workspace_snapshot(&snapshot.to_string())
        .expect("the upgraded layout reads")
        .validate()
        .expect("and is valid");
}

#[test]
fn every_retired_kind_is_dropped() {
    for kind in RETIRED_KINDS {
        let mut snapshot = json!({
            "version": 3,
            "graph": { "nodes": [{ "id": "old", "kind": kind, "position": at(0.0, 0.0) }] }
        });
        let broken = upgrade_snapshot(&mut snapshot);
        assert_eq!(broken.dropped, vec![dropped("old", kind, None)], "{kind}");
        assert!(node_ids(&snapshot).is_empty(), "{kind}");
    }
}

#[test]
fn upgrade_clears_device_gps_sources() {
    let mut snapshot = with_gps_and_triangulation(kraken_df());

    let broken = upgrade_snapshot(&mut snapshot);

    assert_eq!(broken.cleared_gps, vec!["gps1".to_owned()]);
    assert_eq!(
        broken.notices()[1],
        WorkspaceNoticeKind::ClearedGps {
            nodes: vec!["gps1".to_owned()]
        }
    );
    let parsed = parse_workspace_snapshot(&snapshot.to_string()).expect("reads");
    let source = |id: &str| match &parsed.graph.node(id).expect("kept").body {
        NodeBody::Gps(gps) => gps.source.clone(),
        other => panic!("not a GPS node: {other:?}"),
    };
    assert_eq!(source("gps1"), None);
    assert_eq!(
        source("gps2"),
        Some(sdrmm_wire::PositionSource::Fixed {
            lat: 48.1,
            lon: 11.5,
            altitude_m: None
        })
    );
}

#[test]
fn upgrade_gives_old_triangulations_data() {
    let mut snapshot = json!({
        "version": 3,
        "graph": {
            "nodes": [
                { "id": "tri", "kind": "triangulation", "position": at(0.0, 0.0) },
                { "id": "nul", "kind": "triangulation", "data": null, "position": at(0.0, 400.0) }
            ]
        }
    });
    assert!(
        crate::json::from_value::<WorkspaceSnapshot>(&snapshot).is_err(),
        "a triangulation without data no longer reads"
    );

    let broken = upgrade_snapshot(&mut snapshot);

    assert_eq!(broken, Broken::default());
    assert!(broken.notices().is_empty());
    assert_eq!(snapshot["graph"]["nodes"][0]["data"], json!({}));
    assert_eq!(snapshot["graph"]["nodes"][1]["data"], json!({}));
    let parsed = parse_workspace_snapshot(&snapshot.to_string()).expect("reads");
    assert!(matches!(
        parsed.graph.node("tri").expect("kept").body,
        NodeBody::Triangulation(_)
    ));
}

#[test]
fn upgrade_is_idempotent_on_v4() {
    let mut once = with_gps_and_triangulation(kraken_df());
    assert_ne!(upgrade_snapshot(&mut once), Broken::default());

    let mut twice = once.clone();
    assert_eq!(upgrade_snapshot(&mut twice), Broken::default());
    assert_eq!(twice, once);

    let mut newer = once.clone();
    if let Some(nodes) = graph_nodes(&mut newer) {
        nodes.push(json!({ "id": "df2", "kind": "df", "data": {}, "position": at(0.0, 0.0) }));
    }
    let before = newer.clone();
    assert_eq!(upgrade_snapshot(&mut newer), Broken::default());
    assert_eq!(
        newer, before,
        "a node of a reused kind in a v4 layout stays"
    );

    let (file, id) = saved_before_the_rework();
    let first = Store::open(Some(file.path()))
        .expect("open")
        .workspace(id)
        .expect("read");
    let second = Store::open(Some(file.path()))
        .expect("reopen")
        .workspace(id)
        .expect("read");
    assert_eq!(second.info.revision, first.info.revision);
    assert_eq!(second.notices, first.notices);
    assert_eq!(second.snapshot, first.snapshot);
}

#[test]
fn a_version_3_workspace_opens_with_a_notice() {
    let (file, id) = saved_before_the_rework();

    let store = Store::open(Some(file.path())).expect("reopen");
    let detail = store.workspace(id).expect("the upgraded workspace opens");

    assert_eq!(detail.snapshot.version, WORKSPACE_SNAPSHOT_VERSION);
    assert_eq!(detail.info.revision, 2);
    assert_eq!(
        detail.info.nodes as usize,
        detail.snapshot.graph.nodes.len()
    );
    let notices: Vec<&WorkspaceNoticeKind> =
        detail.notices.iter().map(|notice| &notice.notice).collect();
    assert_eq!(
        notices,
        vec![
            &WorkspaceNoticeKind::DroppedNodes {
                nodes: vec![
                    dropped("arr", "array", None),
                    dropped("df1", "df", Some("Roof DF")),
                    dropped("radar", "passive_radar", None),
                ]
            },
            &WorkspaceNoticeKind::ClearedGps {
                nodes: vec!["gps1".to_owned()]
            },
        ]
    );
    assert!(detail.notices[0].id < detail.notices[1].id, "oldest first");
    let state = store.workspace_state(id).expect("settings read");
    let devices: Vec<&str> = state.devices.iter().map(|d| d.node.as_str()).collect();
    assert_eq!(devices, vec!["device"]);
    assert!(state.channels.is_empty());
}

#[test]
fn history_rows_of_an_upgraded_workspace_are_dropped() {
    let (file, id) = saved_before_the_rework();
    {
        let store = Store::open(Some(file.path())).expect("open");
        let old = kraken_df().to_string();
        let conn = store.lock();
        for seq in [1, 2] {
            conn.execute(
                "INSERT INTO workspace_history (workspace_id, seq, created_at, snapshot) \
                 VALUES (?1, ?2, 't', ?3)",
                params![id, seq, old],
            )
            .expect("an old history row");
        }
        conn.execute(
            "UPDATE workspaces SET history_at = 2, snapshot = ?2 WHERE id = ?1",
            params![id, with_gps_and_triangulation(kraken_df()).to_string()],
        )
        .expect("back to v3");
    }

    let store = Store::open(Some(file.path())).expect("reopen");

    let rows: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM workspace_history WHERE workspace_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(rows, 0);
    let detail = store.workspace(id).expect("read");
    assert!(!detail.history.can_undo && !detail.history.can_redo);
    assert!(matches!(
        store.undo_workspace(id),
        Err(StoreError::WorkspaceHistoryEnd { .. })
    ));
}

#[test]
fn a_raw_v3_snapshot_is_refused_not_dropped() {
    assert!(parse_workspace_snapshot(&kraken_df().to_string()).is_err());
    let mut plain = serde_json::to_value(WorkspaceSnapshot::starter()).expect("snapshot");
    plain["version"] = json!(3);
    let err = parse_workspace_snapshot(&plain.to_string()).expect_err("v3 skipped the upgrade");
    assert!(err.to_string().contains("version 3"), "{err}");

    let store = Store::open(None).expect("open");
    let id = store
        .active_workspace_id()
        .expect("active")
        .expect("seeded");
    plant(&store, id, &kraken_df());

    assert!(matches!(store.workspace(id), Err(StoreError::Corrupt(_))));
    let stored: String = store
        .lock()
        .query_row(
            "SELECT snapshot FROM workspaces WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .expect("row");
    assert!(stored.contains("\"df1\""), "the row is left as it was");
}

#[test]
fn an_old_export_loses_the_settings_of_dropped_nodes() {
    let mut export = json!({
        "version": 1,
        "name": "Kraken DF",
        "snapshot": with_gps_and_triangulation(kraken_df()),
        "state": old_state()
    });

    let broken = upgrade_export(&mut export);

    assert_eq!(broken.dropped.len(), 3);
    assert_eq!(broken.cleared_gps, vec!["gps1".to_owned()]);
    assert_eq!(
        export["state"]["devices"],
        json!([{ "node": "device", "settings": { "center_hz": 433_920_000.0 } }])
    );
    assert_eq!(export["state"]["channels"], json!([]));
    let read: sdrmm_wire::WorkspaceExport =
        crate::json::from_value(&export).expect("the upgraded export reads");
    read.validate().expect("and is valid");
}

#[test]
fn an_unreadable_layout_is_left_for_its_open_to_report() {
    let file = tempfile::NamedTempFile::new().expect("temp db");
    let (text, blob) = {
        let store = Store::open(Some(file.path())).expect("open");
        let text = store
            .create_workspace("Text", &WorkspaceSnapshot::starter())
            .expect("create");
        let blob = store
            .create_workspace("Blob", &WorkspaceSnapshot::starter())
            .expect("create");
        let conn = store.lock();
        conn.execute(
            "UPDATE workspaces SET snapshot = 'not json' WHERE id = ?1",
            params![text],
        )
        .expect("plant text");
        conn.execute(
            "UPDATE workspaces SET snapshot = ?2 WHERE id = ?1",
            params![blob, kraken_df().to_string().into_bytes()],
        )
        .expect("plant blob");
        (text, blob)
    };

    let store = Store::open(Some(file.path())).expect("the store still opens");

    assert!(store.workspace(text).is_err());
    assert!(store.workspace(blob).is_err());
    let untouched: String = store
        .lock()
        .query_row(
            "SELECT snapshot FROM workspaces WHERE id = ?1",
            params![text],
            |row| row.get(0),
        )
        .expect("row");
    assert_eq!(untouched, "not json");
}

#[test]
fn a_dismissed_notice_stays_gone_after_a_restart() {
    let (file, id) = saved_before_the_rework();
    {
        let store = Store::open(Some(file.path())).expect("open");
        let notices = store.workspace(id).expect("read").notices;
        assert_eq!(notices.len(), 2);
        for notice in notices {
            store.dismiss_notice(id, notice.id).expect("dismiss");
        }
    }

    let store = Store::open(Some(file.path())).expect("restart");

    assert!(store.workspace(id).expect("read").notices.is_empty());
}

#[test]
fn dismissing_an_unknown_notice_says_so() {
    let store = Store::open(None).expect("open");
    let id = store
        .active_workspace_id()
        .expect("active")
        .expect("seeded");
    assert!(matches!(
        store.dismiss_notice(id, 42),
        Err(StoreError::NoticeNotFound(42))
    ));
}
