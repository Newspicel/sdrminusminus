use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::http::StatusCode;
use sdrmm_engine::SpectrumSnapshot;
use sdrmm_wire::{
    ApiError, DeviceRef, GpsNode, NodeBody, PatchEdge, PatchNode, PortRef, Position, PositionFix,
    PositionSource, SignalMapNode, SurveyCell, SurveyGrid, SurveyStop, WorkspaceSnapshot,
    survey::MAX_SURVEY_CELLS,
};

use super::{
    measure::{Merged, cell_key, measure_dbfs, merge},
    *,
};
use crate::{
    ServerOptions, Store, router_with_state,
    tests::{request, state_over},
};

const CENTER_HZ: f64 = 100_000_000.0;
const OFFSET_HZ: i64 = 25_000;

fn spectrum(center_hz: f64) -> SpectrumSnapshot {
    SpectrumSnapshot {
        seq: 0,
        timestamp: 0,
        center_hz,
        span_hz: 1_000_000.0,
        db: Arc::from(vec![-90.0f32; 1_000]),
    }
}

fn fix(time: &str, latitude: f64) -> PositionFix {
    PositionFix {
        latitude,
        longitude: 13.405,
        altitude_m: None,
        accuracy_m: Some(3.0),
        speed_mps: None,
        track_deg: None,
        time: time.to_owned(),
        attitude: sdrmm_wire::Attitude::default(),
    }
}

fn recording() -> SurveySession {
    SurveySession {
        wiring: Wiring {
            settings: SignalMapNode {
                offset_hz: OFFSET_HZ,
                bandwidth_hz: 12_500,
            },
            iq_wired: true,
            radio: Some(Radio {
                device_set: 1,
                stream: 0,
            }),
            position_node: Some("gps".to_owned()),
        },
        recording: true,
        ..SurveySession::default()
    }
}

fn cell(latitude: f64, frequency_hz: f64, level_dbfs: f32) -> SurveyCell {
    SurveyCell {
        latitude,
        longitude: 13.405,
        frequency_hz,
        level_dbfs,
        measured_at: "2026-08-15T10:00:00Z".to_owned(),
        observations: 1,
        accuracy_m: None,
    }
}

#[test]
fn the_peak_in_the_slice_is_measured() {
    let db = [-120.0, -100.0, -80.0, -60.0, -40.0];
    assert_eq!(
        measure_dbfs(CENTER_HZ, 1e6, &db, CENTER_HZ, 200_000.0),
        Some(-80.0)
    );
    assert_eq!(
        measure_dbfs(CENTER_HZ, 1e6, &db, 100_300_000.0, 400_000.0),
        Some(-40.0)
    );
    assert_eq!(
        measure_dbfs(CENTER_HZ, 1e6, &db, 101_000_000.0, 12_500.0),
        None
    );
    assert_eq!(measure_dbfs(CENTER_HZ, 1e6, &[], CENTER_HZ, 12_500.0), None);
    assert_eq!(measure_dbfs(CENTER_HZ, 0.0, &db, CENTER_HZ, 12_500.0), None);
    assert_eq!(measure_dbfs(CENTER_HZ, 1e6, &db, CENTER_HZ, 0.0), None);
    assert_eq!(measure_dbfs(CENTER_HZ, 1e6, &db, f64::NAN, 12_500.0), None);
}

#[test]
fn cells_merge_in_the_power_domain() {
    let mut cells = Vec::new();
    assert_eq!(
        merge(&mut cells, cell(52.52, 145_500_000.0, -60.0)),
        Merged::Added(0)
    );
    let again = SurveyCell {
        latitude: 52.520_001,
        accuracy_m: Some(3.0),
        measured_at: "2026-08-15T10:00:01Z".to_owned(),
        ..cell(52.52, 145_500_000.0, -50.0)
    };
    assert_eq!(merge(&mut cells, again), Merged::Updated(0));
    assert_eq!(cells.len(), 1);
    assert!((cells[0].level_dbfs - -52.596).abs() < 0.01, "{cells:?}");
    assert_eq!(cells[0].observations, 2);
    assert_eq!(cells[0].accuracy_m, Some(3.0));
    assert_eq!(cells[0].measured_at, "2026-08-15T10:00:01Z");

    assert_eq!(
        merge(&mut cells, cell(52.52, 145_525_000.0, -41.0)),
        Merged::Added(1)
    );
    assert_ne!(
        cell_key(52.52, 13.405, 145_500_000.0),
        cell_key(52.52, 13.405, 145_525_000.0)
    );
}

#[test]
fn the_oldest_cell_is_dropped_and_counted() {
    let mut session = recording();
    session.frequency_hz = Some(CENTER_HZ + OFFSET_HZ as f64);
    let started = Instant::now();
    for index in 0..=MAX_SURVEY_CELLS {
        let latitude = 52.0 + index as f64 * 0.000_2;
        let update = session
            .step(
                &spectrum(CENTER_HZ),
                Some(&fix(&format!("t{index}"), latitude)),
                started,
            )
            .expect("every new fix makes a cell");
        assert!(update.cell.is_some());
    }
    assert_eq!(session.cells.len(), MAX_SURVEY_CELLS);
    assert_eq!(session.dropped, 1);
    assert!((session.cells[0].latitude - 52.000_2).abs() < 1e-9);
    let grid = session.grid("map");
    assert_eq!(grid.dropped, 1);
    assert_eq!(grid.offset_hz, OFFSET_HZ);
}

#[test]
fn a_retune_stops_the_recording() {
    let mut session = recording();
    let now = Instant::now();
    let first = session
        .step(&spectrum(CENTER_HZ), Some(&fix("t0", 52.0)), now)
        .expect("a cell");
    assert_eq!(first.target_hz, Some(CENTER_HZ + OFFSET_HZ as f64));
    assert_eq!(session.frequency_hz, Some(CENTER_HZ + OFFSET_HZ as f64));
    let moved = session
        .step(
            &spectrum(CENTER_HZ + 100_000.0),
            Some(&fix("t1", 52.0)),
            now,
        )
        .expect("a stop");
    assert_eq!(moved.stopped, Some(SurveyStop::Retuned));
    assert!(!moved.recording);
    assert!(!session.recording);
    assert_eq!(session.cells.len(), 1);
}

#[test]
fn one_cell_per_new_fix() {
    let mut session = recording();
    let now = Instant::now();
    let here = fix("t0", 52.0);
    let first = session
        .step(&spectrum(CENTER_HZ), Some(&here), now)
        .expect("first cell");
    assert_eq!(first.cell.map(|cell| cell.observations), Some(1));
    assert!(
        session
            .step(&spectrum(CENTER_HZ), Some(&here), now)
            .is_none(),
        "the same fix again is paced as a level"
    );
    let level = session
        .step(&spectrum(CENTER_HZ), Some(&here), now + LEVEL_INTERVAL)
        .expect("a level");
    assert!(level.cell.is_none());
    assert_eq!(level.level_dbfs, Some(-90.0));
    let next = session
        .step(
            &spectrum(CENTER_HZ),
            Some(&fix("t1", 52.0)),
            now + LEVEL_INTERVAL,
        )
        .expect("a second observation");
    assert_eq!(next.cell.map(|cell| cell.observations), Some(2));
    assert_eq!(session.cells.len(), 1);
}

#[test]
fn a_stopped_survey_only_reports_levels() {
    let mut session = SurveySession {
        recording: false,
        ..recording()
    };
    let update = session
        .step(&spectrum(CENTER_HZ), Some(&fix("t0", 52.0)), Instant::now())
        .expect("a level");
    assert!(update.cell.is_none());
    assert!(session.cells.is_empty());
}

fn node(id: &str, body: NodeBody) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
    PatchEdge {
        from: PortRef {
            node: from.0.to_owned(),
            port: from.1.to_owned(),
        },
        to: PortRef {
            node: to.0.to_owned(),
            port: to.1.to_owned(),
        },
    }
}

fn surveyed(with_position: bool) -> WorkspaceSnapshot {
    let mut snapshot = WorkspaceSnapshot::starter();
    for patch in &mut snapshot.graph.nodes {
        if let NodeBody::Device(device) = &mut patch.body {
            device.device = Some(DeviceRef {
                backend: "virtual".to_owned(),
                serial: None,
                key: Some("siggen".to_owned()),
            });
        }
    }
    snapshot.graph.nodes.push(node(
        "gps",
        NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Fixed {
                lat: 52.52,
                lon: 13.405,
                altitude_m: None,
            }),
        }),
    ));
    snapshot
        .graph
        .nodes
        .push(node("map", NodeBody::SignalMap(SignalMapNode::default())));
    snapshot
        .graph
        .edges
        .push(edge(("device", "iq"), ("map", "iq")));
    if with_position {
        snapshot
            .graph
            .edges
            .push(edge(("gps", "position"), ("map", "position")));
    }
    snapshot
}

async fn survey_bench(with_position: bool) -> (axum::Router, AppState) {
    let store = Arc::new(Store::open(None).expect("store"));
    let id = store
        .create_workspace("survey", &surveyed(with_position))
        .expect("create");
    store.activate_workspace(id).expect("activate");
    let state = state_over(store);
    let (app, background) = router_with_state(state.clone(), &ServerOptions::default());
    background.detach();
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/workspaces/{id}/apply"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    (app, state)
}

async fn act(app: &axum::Router, action: &str) -> (StatusCode, axum::body::Bytes) {
    request(
        app.clone(),
        "POST",
        "/api/survey/map",
        Some(&format!(r#"{{"action":"{action}"}}"#)),
    )
    .await
}

#[tokio::test]
async fn survey_records_through_the_api() {
    let (app, state) = survey_bench(true).await;
    let mut events = state.engine.subscribe_events();
    let (status, body) = act(&app, "start").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let started: SurveyGrid = serde_json::from_slice(&body).expect("grid");
    assert!(started.recording);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let cell = loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("a surveyed cell in time")
            .expect("events");
        if let ServerEvent::SurveyUpdate { node, update } = event
            && node == "map"
            && let Some(cell) = update.cell
        {
            break cell;
        }
    };
    assert!((cell.latitude - 52.52).abs() < 1e-9);

    let (status, body) = request(app.clone(), "GET", "/api/survey/map", None).await;
    assert_eq!(status, StatusCode::OK);
    let grid: SurveyGrid = serde_json::from_slice(&body).expect("grid");
    assert_eq!(grid.cells.len(), 1);
    assert!(grid.frequency_hz.is_some());

    let (_, body) = act(&app, "stop").await;
    let stopped: SurveyGrid = serde_json::from_slice(&body).expect("grid");
    assert!(!stopped.recording);
    let (_, body) = act(&app, "clear").await;
    let cleared: SurveyGrid = serde_json::from_slice(&body).expect("grid");
    assert!(cleared.cells.is_empty());
    assert_eq!(cleared.frequency_hz, None);

    let (status, _) = request(app, "GET", "/api/survey/nowhere", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_survey_without_a_position_wire_refuses_to_record() {
    let (app, _) = survey_bench(false).await;
    let (status, body) = act(&app, "start").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let error: ApiError = serde_json::from_slice(&body).expect("error");
    assert_eq!(error.error, "Wire a position");
    let (status, _) = request(
        app,
        "POST",
        "/api/survey/nowhere",
        Some(r#"{"action":"start"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

async fn next_survey_update(
    events: &mut tokio::sync::broadcast::Receiver<ServerEvent>,
) -> sdrmm_wire::SurveyUpdate {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("a survey update in time")
            .expect("events");
        if let ServerEvent::SurveyUpdate { node, update } = event
            && node == "map"
        {
            return *update;
        }
    }
}

#[tokio::test]
async fn unwiring_keeps_the_cells_and_says_why_it_stopped() {
    let state = state_over(Arc::new(Store::open(None).expect("store")));
    let mut events = state.engine.subscribe_events();
    let mut session = recording();
    session.wiring.radio = None;
    session.cells.push(cell(52.52, 145_500_000.0, -60.0));
    state.survey.lock().insert("map".to_owned(), session);

    let unwired = Wiring {
        position_node: None,
        ..recording().wiring
    };
    state
        .survey
        .apply(&state, HashMap::from([("map".to_owned(), unwired)]));
    let update = next_survey_update(&mut events).await;
    assert_eq!(update.stopped, Some(SurveyStop::Unwired));
    assert!(!update.recording);
    assert_eq!(update.cells, 1);
    assert_eq!(state.survey.grid("map").expect("kept").cells.len(), 1);

    let radioless = Wiring {
        radio: None,
        ..recording().wiring
    };
    state
        .survey
        .lock()
        .get_mut("map")
        .expect("session")
        .recording = true;
    state
        .survey
        .apply(&state, HashMap::from([("map".to_owned(), radioless)]));
    let update = next_survey_update(&mut events).await;
    assert_eq!(update.stopped, Some(SurveyStop::RadioGone));

    state.survey.apply(&state, HashMap::new());
    assert!(state.survey.grid("map").is_none());
}
