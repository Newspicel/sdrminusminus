use std::{future::IntoFuture, time::Duration};

use futures::StreamExt;
use sdrmm_engine::ArrayEvent;
use sdrmm_wire::{
    ArrayCalRecord, ArrayCalSource, ArrayGain, ArrayNode, ArrayOrientation, ArrayStatus, Attitude,
    BeamformerNode, CalSourceKind, ChannelNode, DeviceNode, DeviceRef, DfNode, FusionGridOwned,
    GpsNode, HeadingSource, LaneKey, LaneSolution, NodeBody, PassiveRadarNode, PatchApplyReport,
    PatchEdge, PatchGraph, PatchNode, PatchRefusal, PortRef, Position, PositionFix, PositionSource,
    ProcessorReading, RackLayout, RadarTrackEvent, RadarUpdate, ServerEvent, SurfaceFrame,
    TrackChange, WorkspaceNoticeKind, WorkspaceSnapshot,
};
use tokio::sync::broadcast::{self, error::RecvError};

use super::*;
use crate::array::{self, ArrayWiring};

const WAIT: Duration = Duration::from_secs(10);
const RADIO: &str = "radio";
const ARRAY: &str = "arr";

fn node(id: &str, body: NodeBody) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

fn wire(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
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

fn radio(id: &str, key: &str) -> PatchNode {
    node(
        id,
        NodeBody::Device(DeviceNode {
            device: Some(DeviceRef {
                backend: "virtual".to_owned(),
                serial: None,
                key: Some(key.to_owned()),
            }),
            locked_streams: Vec::new(),
        }),
    )
}

fn lane_wire(device: &str, stream: u32, array: &str, lane: u32) -> PatchEdge {
    wire(
        (device, &sdrmm_wire::patch::stream_port("iq", stream)),
        (array, &sdrmm_wire::patch::stream_port("lane", lane)),
    )
}

fn kraken_graph(settings: ArrayNode) -> PatchGraph {
    PatchGraph {
        nodes: vec![
            radio(RADIO, "kraken5"),
            node(ARRAY, NodeBody::Array(settings)),
        ],
        edges: (0..5)
            .map(|lane| lane_wire(RADIO, lane, ARRAY, lane))
            .collect(),
    }
}

fn snapshot(graph: PatchGraph) -> WorkspaceSnapshot {
    WorkspaceSnapshot::new(graph, RackLayout::default())
}

fn with_gps(mut graph: PatchGraph, source: PositionSource, wired: bool) -> PatchGraph {
    graph.nodes.push(node(
        "gps",
        NodeBody::Gps(GpsNode {
            source: Some(source),
        }),
    ));
    if wired {
        graph
            .edges
            .push(wire(("gps", "position"), (ARRAY, "position")));
    }
    graph
}

fn fixed(lat: f64, lon: f64) -> PositionSource {
    PositionSource::Fixed {
        lat,
        lon,
        altitude_m: None,
    }
}

fn heading_array() -> ArrayNode {
    ArrayNode {
        orientation: ArrayOrientation::Heading {
            mount_offset_deg: 10.0,
        },
        ..ArrayNode::default()
    }
}

async fn put_and_apply(app: &Router, graph: PatchGraph, revision: u64) -> PatchApplyReport {
    let workspace = put_workspace_revision(app, &snapshot(graph), revision).await;
    apply(app, workspace).await
}

fn status_of(state: &AppState, node: &str) -> Option<ArrayStatus> {
    state
        .engine
        .array_statuses()
        .into_iter()
        .find(|status| status.node == node)
}

async fn wait_for<T>(what: &str, mut found: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(found) = found() {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn next_error(
    events: &mut broadcast::Receiver<ServerEvent>,
    wanted: &str,
    within: Duration,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, events.recv()).await {
            Ok(Ok(ServerEvent::Error { message })) if message.contains(wanted) => {
                return Some(message);
            }
            Ok(Ok(_) | Err(RecvError::Lagged(_))) => {}
            Ok(Err(RecvError::Closed)) | Err(_) => return None,
        }
    }
}

fn phone_fix(lat: f64, heading_deg: f64) -> PositionFix {
    PositionFix {
        latitude: lat,
        longitude: 11.5,
        altitude_m: Some(520.0),
        accuracy_m: Some(3.0),
        speed_mps: Some(0.0),
        track_deg: None,
        time: crate::store::rfc3339_now(),
        attitude: Attitude {
            heading_deg: Some(heading_deg),
            heading_accuracy_deg: Some(4.0),
            heading_source: Some(HeadingSource::Compass),
            ..Attitude::default()
        },
    }
}

#[test]
fn wired_array_reads_lanes_in_port_order() {
    let mut graph = PatchGraph {
        nodes: vec![
            radio(RADIO, "kraken5"),
            node(ARRAY, NodeBody::Array(ArrayNode::default())),
        ],
        edges: vec![
            lane_wire(RADIO, 4, ARRAY, 2),
            lane_wire(RADIO, 0, ARRAY, 1),
            lane_wire(RADIO, 2, ARRAY, 0),
        ],
    };
    assert_eq!(
        array::wired_array(&graph, ARRAY),
        ArrayWiring::Lanes(vec![
            (RADIO.to_owned(), 2),
            (RADIO.to_owned(), 0),
            (RADIO.to_owned(), 4),
        ])
    );

    graph.edges.push(lane_wire(RADIO, 1, ARRAY, 4));
    assert_eq!(
        array::wired_array(&graph, ARRAY),
        ArrayWiring::Gap { lane: 3 }
    );

    graph.edges.push(lane_wire(RADIO, 2, ARRAY, 3));
    assert_eq!(
        array::wired_array(&graph, ARRAY),
        ArrayWiring::Duplicate { lane: 3 }
    );

    graph.edges.clear();
    assert_eq!(array::wired_array(&graph, ARRAY), ArrayWiring::Unwired);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gap_in_the_lanes_is_refused_visibly() {
    let (app, state) = test_router_with_state();
    let mut graph = kraken_graph(ArrayNode::default());
    graph.edges.retain(|edge| edge.to.port != "lane2");

    let report = put_and_apply(&app, graph.clone(), 1).await;

    assert_eq!(
        report.refused,
        vec![PatchRefusal {
            node: ARRAY.to_owned(),
            reason: "Lane 2 unwired".to_owned(),
        }]
    );
    assert!(state.engine.array_statuses().is_empty());
    let (status, body) = request(app.clone(), "POST", "/api/arrays/arr/calibrate", None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let error: ApiError = serde_json::from_slice(&body).expect("ApiError");
    assert_eq!(error.error, "Lane 2 unwired");

    graph.edges.push(lane_wire(RADIO, 1, ARRAY, 1));
    let report = put_and_apply(&app, graph, 2).await;
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    assert!(status_of(&state, ARRAY).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn processors_bind_to_the_array_they_are_wired_from() {
    let (app, state) = test_router_with_state();
    let mut graph = kraken_graph(ArrayNode::default());
    graph.nodes.extend([
        radio("quad", "array4"),
        node("second", NodeBody::Array(ArrayNode::default())),
        node("df1", NodeBody::Df(DfNode::default())),
        node("df2", NodeBody::Df(DfNode::default())),
    ]);
    graph
        .edges
        .extend((0..4).map(|lane| lane_wire("quad", lane, "second", lane)));
    graph.edges.push(wire((ARRAY, "array"), ("df1", "array")));
    graph
        .edges
        .push(wire(("second", "array"), ("df2", "array")));

    let report = put_and_apply(&app, graph.clone(), 1).await;
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let processors = |array: &str| -> Vec<String> {
        status_of(&state, array)
            .map(|status| status.processors.into_iter().map(|p| p.node).collect())
            .unwrap_or_default()
    };
    assert_eq!(processors(ARRAY), vec!["df1"]);
    assert_eq!(processors("second"), vec!["df2"]);
    assert_eq!(
        array::processor_array(&state, "df1").as_deref(),
        Some(ARRAY)
    );
    assert_eq!(
        array::processor_array(&state, "df2").as_deref(),
        Some("second")
    );

    graph
        .edges
        .retain(|edge| !(edge.to.node == "df1" && edge.to.port == "array"));
    graph
        .edges
        .push(wire(("second", "array"), ("df1", "array")));
    put_and_apply(&app, graph.clone(), 2).await;
    assert!(processors(ARRAY).is_empty());
    let mut moved = processors("second");
    moved.sort();
    assert_eq!(moved, vec!["df1", "df2"]);
    assert_eq!(
        array::processor_array(&state, "df1").as_deref(),
        Some("second")
    );

    graph.nodes.retain(|node| node.id != "df2");
    graph.edges.retain(|edge| edge.to.node != "df2");
    put_and_apply(&app, graph, 3).await;
    assert_eq!(processors("second"), vec!["df1"]);
    assert_eq!(array::processor_array(&state, "df2"), None);
}

fn beam_graph(listened: bool) -> PatchGraph {
    let mut graph = kraken_graph(ArrayNode::default());
    graph.nodes.extend([
        node("bf", NodeBody::Beamformer(BeamformerNode::default())),
        node(
            "voice",
            NodeBody::Channel(ChannelNode {
                channel_type: "nfm".to_owned(),
                record_calls: false,
                tuning_locked: false,
            }),
        ),
    ]);
    graph.edges.push(wire((ARRAY, "array"), ("bf", "array")));
    if listened {
        graph.edges.push(wire(("bf", "beam"), ("voice", "iq")));
    }
    graph
}

#[tokio::test(flavor = "multi_thread")]
async fn channels_on_a_lane_port_bind_to_that_processors_virtual_lane() {
    let (app, state) = test_router_with_state();

    let first = put_and_apply(&app, beam_graph(true), 1).await;
    assert!(first.refused.is_empty(), "{:?}", first.refused);
    assert_eq!(first.created, 1);

    let live = get_state(&app).await;
    let set = &live.device_sets[0];
    let [lane] = set.virtual_lanes.as_slice() else {
        panic!("one lane output: {:?}", set.virtual_lanes);
    };
    assert_eq!((lane.node.as_str(), lane.port.as_str()), ("bf", "beam"));
    let voice: Vec<&sdrmm_wire::ChannelInfo> = set
        .channels
        .iter()
        .filter(|channel| channel.node.as_deref() == Some("voice"))
        .collect();
    assert_eq!(voice.len(), 1);
    assert_eq!(voice[0].stream, lane.stream);
    assert_eq!(voice[0].settings.frequency_hz, lane.center_hz);
    assert_eq!(
        crate::workspace::virtual_bound(&beam_graph(true), set),
        vec![("voice".to_owned(), voice[0].id)]
    );

    let again = put_and_apply(&app, beam_graph(true), 2).await;
    assert_eq!(again.created, 0, "the lane channel is kept, not reopened");
    assert_eq!(again.closed, 0);
    let live = get_state(&app).await;
    assert_eq!(
        live.device_sets[0]
            .channels
            .iter()
            .filter(|channel| channel.node.as_deref() == Some("voice"))
            .count(),
        1
    );

    put_and_apply(&app, beam_graph(false), 3).await;
    let live = get_state(&app).await;
    assert!(live.device_sets[0].virtual_lanes.is_empty());
    assert!(
        live.device_sets[0]
            .channels
            .iter()
            .all(|channel| channel.node.as_deref() != Some("voice"))
    );
    assert!(
        status_of(&state, ARRAY)
            .expect("the array stays")
            .processors
            .iter()
            .any(|processor| processor.node == "bf")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unwired_position_sends_no_pose() {
    let (app, state) = test_router_with_state();
    let unwired = with_gps(kraken_graph(ArrayNode::default()), fixed(48.1, 11.5), false);
    put_and_apply(&app, unwired.clone(), 1).await;
    wait_for("the GPS fix", || state.gps.fix("gps")).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        status_of(&state, ARRAY).expect("running").position,
        None,
        "a GPS on the canvas is not the array's GPS"
    );
    assert!(array::array_pose(&state, ARRAY).is_none());

    let wired = with_gps(kraken_graph(ArrayNode::default()), fixed(48.1, 11.5), true);
    put_and_apply(&app, wired, 2).await;
    wait_for("the wired fix", || status_of(&state, ARRAY)?.position).await;

    put_and_apply(&app, unwired, 3).await;
    assert_eq!(status_of(&state, ARRAY).expect("running").position, None);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(status_of(&state, ARRAY).expect("running").position, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gps_wired_into_an_array_moves_its_pose() {
    let (app, state) = test_router_with_state();
    state.gps.set_pose_interval(Duration::ZERO);
    let phone = crate::phones::tests::pair_one(&state.phones).phone.id;
    let graph = with_gps(
        kraken_graph(heading_array()),
        PositionSource::Phone {
            phone: phone.clone(),
        },
        true,
    );
    put_and_apply(&app, graph, 1).await;

    let publish = |fix: PositionFix| {
        state
            .gps
            .publish_pose(&state, &phone, Some(fix), None)
            .expect("the pose is taken")
    };
    assert_eq!(publish(phone_fix(48.1, 80.0)), 1);
    let placed = wait_for("the first pose", || {
        let status = status_of(&state, ARRAY)?;
        (status.azimuth_deg == Some(90.0)).then_some(status)
    })
    .await;
    let position = placed.position.expect("a position");
    assert_eq!((position.lat, position.lon), (48.1, 11.5));
    assert_eq!(placed.heading_source, Some(HeadingSource::Compass));

    let pose = array::array_pose(&state, ARRAY).expect("a pose");
    assert_eq!(
        (pose.lat, pose.lon, pose.altitude_m),
        (48.1, 11.5, Some(520.0))
    );
    assert_eq!(pose.heading_deg, Some(90.0));
    assert_eq!(pose.heading_sigma_deg, Some(4.0));
    assert_eq!(pose.heading_source, Some(HeadingSource::Compass));

    publish(phone_fix(48.2, 355.0));
    let moved = wait_for("the moved pose", || {
        let status = status_of(&state, ARRAY)?;
        (status.azimuth_deg == Some(5.0)).then_some(status)
    })
    .await;
    assert_eq!(moved.position.map(|at| at.lat), Some(48.2));
    assert_eq!(
        array::array_pose(&state, ARRAY).and_then(|pose| pose.heading_deg),
        Some(5.0)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_pose_is_reported() {
    let (app, state) = test_router_with_state();
    state.gps.set_pose_interval(Duration::ZERO);
    let phone = crate::phones::tests::pair_one(&state.phones).phone.id;
    let graph = with_gps(
        kraken_graph(heading_array()),
        PositionSource::Phone {
            phone: phone.clone(),
        },
        true,
    );
    put_and_apply(&app, graph, 1).await;
    state
        .engine
        .remove_array(ARRAY)
        .expect("the runtime goes away under the binding");
    let mut events = state.engine.subscribe_events();

    let publish = |lat: f64| {
        state
            .gps
            .publish_pose(&state, &phone, Some(phone_fix(lat, 90.0)), None)
            .expect("the pose is taken")
    };
    publish(48.1);
    assert_eq!(
        next_error(&mut events, "array pose lost", WAIT)
            .await
            .as_deref(),
        Some("array pose lost: arr")
    );
    publish(48.2);
    assert_eq!(
        next_error(&mut events, "array pose lost", Duration::from_millis(400)).await,
        None,
        "at most once a second"
    );
    tokio::time::sleep(Duration::from_millis(700)).await;
    publish(48.3);
    assert!(
        next_error(&mut events, "array pose lost", WAIT)
            .await
            .is_some()
    );
}

fn v3_kraken() -> serde_json::Value {
    let at = serde_json::json!({ "x": 0.0, "y": 0.0 });
    serde_json::json!({
        "version": 3,
        "graph": {
            "nodes": [
                { "id": "device", "kind": "device", "data": {}, "position": at },
                { "id": "arr", "kind": "array", "data": { "members": 1 }, "position": at },
                { "id": "df1", "kind": "df", "label": "Roof DF", "data": {}, "position": at },
                { "id": "beam", "kind": "combiner", "data": {}, "position": at },
                { "id": "scope", "kind": "scope", "position": at }
            ],
            "edges": [
                { "from": { "node": "device", "port": "iq" }, "to": { "node": "arr", "port": "iq0" } },
                { "from": { "node": "arr", "port": "iq0" }, "to": { "node": "df1", "port": "iq0" } },
                { "from": { "node": "device", "port": "iq" }, "to": { "node": "scope", "port": "iq" } }
            ]
        },
        "rack": { "slots": [{ "node": "df1", "x": 0, "y": 0, "w": 2, "h": 2 }] }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_3_workspace_drops_old_array_nodes_with_a_notice() {
    let file = tempfile::NamedTempFile::new().expect("temp db");
    let id = {
        let store = Store::open(Some(file.path())).expect("open");
        store
            .active_workspace_id()
            .expect("active")
            .expect("seeded")
    };
    rusqlite::Connection::open(file.path())
        .expect("raw db")
        .execute(
            "UPDATE workspaces SET snapshot = ?2 WHERE id = ?1",
            rusqlite::params![id, v3_kraken().to_string()],
        )
        .expect("an old layout");

    let store = Arc::new(Store::open(Some(file.path())).expect("reopen"));
    let (app, background) = router_with_state(state_over(store), &ServerOptions::default());
    background.detach();
    let detail = workspace_detail(&app, id).await;

    assert_eq!(
        detail.snapshot.version,
        sdrmm_wire::WORKSPACE_SNAPSHOT_VERSION
    );
    let kept: Vec<&str> = detail
        .snapshot
        .graph
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect();
    assert_eq!(kept, vec!["device", "scope"]);
    assert_eq!(detail.snapshot.graph.edges.len(), 1);
    assert!(detail.snapshot.rack.slots.is_empty());
    let [notice] = detail.notices.as_slice() else {
        panic!("one notice: {:?}", detail.notices);
    };
    let WorkspaceNoticeKind::DroppedNodes { nodes } = &notice.notice else {
        panic!("a dropped node notice: {notice:?}");
    };
    let dropped: Vec<(&str, &str)> = nodes
        .iter()
        .map(|node| (node.id.as_str(), node.kind.as_str()))
        .collect();
    assert_eq!(
        dropped,
        vec![("arr", "array"), ("df1", "df"), ("beam", "combiner")]
    );
}

fn solution(lanes: &[LaneKey], center_hz: f64, sample_rate: f64) -> ArrayCalRecord {
    ArrayCalRecord {
        lanes: lanes.to_vec(),
        center_hz,
        sample_rate,
        gain_db: Some(30.0),
        source: CalSourceKind::Noise,
        keeps_phase: true,
        solved_at: "2026-09-29T10:00:00.000Z".to_owned(),
        solution: lanes
            .iter()
            .map(|_| LaneSolution {
                delay_samples: 0.0,
                phase_deg: 12.0,
                gain_db: 0.0,
                coherence: 0.98,
                equaliser: Vec::new(),
            })
            .collect(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_solved_calibration_is_stored_and_offered_as_warm_start() {
    let (app, state) = test_router_with_state();
    put_and_apply(&app, kraken_graph(ArrayNode::default()), 1).await;
    let binding = state.arrays.binding(ARRAY).expect("bound");
    assert_eq!(
        binding.spec.as_ref().and_then(|spec| spec.warm.clone()),
        None
    );

    let live = get_state(&app).await;
    let set = &live.device_sets[0];
    let keys: Vec<LaneKey> = (0..5)
        .map(|stream| LaneKey {
            device: set.device.id(),
            stream,
        })
        .collect();
    let rate = set.settings.sample_rate.expect("the radio names its rate");
    let center_hz = status_of(&state, ARRAY).expect("running").center_hz;
    let record = solution(&keys, center_hz, rate);

    let flow = array::handle(
        &state,
        Ok(ArrayEvent::Solved {
            array: ARRAY.to_owned(),
            record: record.clone(),
        }),
    );
    assert!(flow.is_continue());
    let stored = wait_for("the stored solution", || {
        state
            .store
            .array_calibration(&keys, rate, center_hz)
            .expect("read")
    })
    .await;
    assert_eq!(stored, record);

    let workspace = workspaces(&app).await.active.expect("active");
    apply(&app, workspace).await;
    assert_eq!(
        state
            .arrays
            .binding(ARRAY)
            .and_then(|binding| binding.spec?.warm),
        None,
        "a running array is not restarted for a new record"
    );

    let restarted = state_over(state.store.clone());
    let (again, background) = router_with_state(restarted.clone(), &ServerOptions::default());
    background.detach();
    apply(&again, workspace).await;
    assert_eq!(
        restarted
            .arrays
            .binding(ARRAY)
            .and_then(|binding| binding.spec?.warm),
        Some(record),
        "the next start begins from the stored solution"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn array_status_reaches_clients_as_array_update() {
    let (app, _state) = test_router_with_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app.clone()).into_future());
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws"))
        .await
        .expect("connect");

    put_and_apply(&app, kraken_graph(ArrayNode::default()), 1).await;

    let status = tokio::time::timeout(WAIT, async {
        loop {
            let Some(Ok(message)) = socket.next().await else {
                panic!("the socket closed");
            };
            let tokio_tungstenite::tungstenite::Message::Text(text) = message else {
                continue;
            };
            if let Ok(ServerEvent::ArrayUpdate { status }) = serde_json::from_str(text.as_str()) {
                return status;
            }
        }
    })
    .await
    .expect("an ArrayUpdate arrives");
    assert_eq!(status.node, ARRAY);
    assert_eq!(status.lanes.len(), 5);
}

async fn tune(app: &Router, node: &str, body: &str) -> (StatusCode, String) {
    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/arrays/{node}/tune"),
        Some(body),
    )
    .await;
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn calibrate(app: &Router, node: &str) -> (StatusCode, String) {
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/arrays/{node}/calibrate"),
        None,
    )
    .await;
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn calibrate_and_tune_endpoints_reach_the_engine() {
    let (app, state) = test_router_with_state();
    let workspace =
        put_workspace_revision(&app, &snapshot(kraken_graph(ArrayNode::default())), 1).await;
    apply(&app, workspace).await;

    let (status, body) = tune(
        &app,
        ARRAY,
        r#"{"center_hz":433920000,"gain":{"kind":"manual","db":20}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = request(app.clone(), "GET", "/api/arrays", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<ArrayStatus> = serde_json::from_slice(&body).expect("statuses");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].center_hz, 433.92e6);
    assert_eq!(listed[0].gain, ArrayGain::Manual { db: 20.0 });

    let (status, body) = tune(&app, ARRAY, r#"{"gain":{"kind":"manual","db":500}}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Gain out of range"), "{body}");
    assert_eq!(tune(&app, "ghost", "{}").await.0, StatusCode::NOT_FOUND);

    assert_eq!(calibrate(&app, ARRAY).await.0, StatusCode::ACCEPTED);
    assert_eq!(calibrate(&app, "ghost").await.0, StatusCode::NOT_FOUND);

    let graph = snapshot(kraken_graph(ArrayNode::default())).graph;
    let summary = wait_for("a settled calibration", || {
        array::summary(&state, &graph, ARRAY).filter(|summary| summary.problem.is_none())
    })
    .await;
    assert_eq!(
        summary.device_sets,
        vec![get_state(&app).await.device_sets[0].id]
    );
    assert_eq!(summary.center_hz, Some(433.92e6));
    assert!(summary.can_calibrate);
    assert_eq!(summary.problem, None);

    let mut settings = ArrayNode::default();
    settings.cal.source = ArrayCalSource::Off;
    put_and_apply(&app, kraken_graph(settings.clone()), 2).await;
    let (status, body) = calibrate(&app, ARRAY).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("Cal is off"), "{body}");
    let (status, body) = tune(&app, ARRAY, r#"{"gain":{"kind":"auto"}}"#).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("Auto gain needs cal"), "{body}");
    let off = array::summary(&state, &kraken_graph(settings), ARRAY).expect("an array");
    assert!(!off.can_calibrate);
    assert_eq!(off.center_hz, Some(433.92e6), "the tune survives the edit");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_array_records_every_lane_until_stopped() {
    let dir = tempfile::tempdir().expect("recordings dir");
    let state = recording_state(dir.path());
    let (app, background) = router_with_state(state, &ServerOptions::default());
    background.detach();
    put_and_apply(&app, kraken_graph(ArrayNode::default()), 1).await;

    let record = |method: &'static str| {
        let app = app.clone();
        async move {
            let body = (method == "POST").then_some(r#"{"name":"roof"}"#);
            request(app, method, "/api/arrays/arr/recording", body).await
        }
    };
    let (status, body) = record("POST").await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let started: sdrmm_wire::ArrayRecordingStarted =
        serde_json::from_slice(&body).expect("started");
    assert_eq!(started.stem, "roof");
    assert_eq!(record("POST").await.0, StatusCode::CONFLICT);
    assert_eq!(record("DELETE").await.0, StatusCode::NO_CONTENT);
    assert_eq!(record("DELETE").await.0, StatusCode::NOT_FOUND);
}

fn radar_graph() -> PatchGraph {
    let mut graph = kraken_graph(ArrayNode::default());
    graph.nodes.push(node(
        "radar",
        NodeBody::PassiveRadar(PassiveRadarNode::default()),
    ));
    graph.edges.push(wire((ARRAY, "array"), ("radar", "array")));
    graph
}

fn grid(seq: u32) -> Arc<SurfaceFrame> {
    Arc::new(SurfaceFrame::FusionGrid(FusionGridOwned {
        stream_id: 0,
        seq,
        timestamp: 1,
        south: 48.0,
        west: 11.0,
        north: 48.1,
        east: 11.1,
        cols: 1,
        rows: 1,
        cells: vec![200],
    }))
}

fn draw(state: &AppState, seq: u32, surface: Arc<SurfaceFrame>) -> bool {
    array::handle(
        state,
        Ok(ArrayEvent::Surface {
            processor: "radar".to_owned(),
            seq,
            surface,
        }),
    )
    .is_continue()
}

#[tokio::test(flavor = "multi_thread")]
async fn processor_reports_and_surfaces_are_forwarded() {
    let (app, state) = test_router_with_state();
    put_and_apply(&app, radar_graph(), 1).await;
    let mut events = state.engine.subscribe_events();
    let (_, mut surface) = state.surfaces.subscribe("radar").expect("an open surface");
    let update = RadarUpdate {
        seq: 3,
        events: vec![RadarTrackEvent {
            track_id: 1,
            change: TrackChange::Confirmed,
            range_km: 12.0,
            range_rate_mps: -80.0,
            doppler_hz: 40.0,
            snr_db: 18.0,
            bearing_deg: None,
            lat: None,
            lon: None,
            icao: None,
        }],
        ..RadarUpdate::default()
    };
    let reading = ProcessorReading::PassiveRadar(update);

    let reported = array::handle(
        &state,
        Ok(ArrayEvent::Report {
            processor: "radar".to_owned(),
            reading: Arc::new(reading.clone()),
        }),
    );
    let frame = grid(9);
    assert!(reported.is_continue() && draw(&state, 9, frame.clone()));

    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(ServerEvent::ProcessorUpdate {
                node,
                reading: sent,
            }) = events.recv().await
                && node == "radar"
                && matches!(
                    &*sent,
                    ProcessorReading::PassiveRadar(done) if done.seq == 3 && done.events.is_empty()
                )
            {
                return;
            }
        }
    })
    .await
    .expect("the report goes out as a ProcessorUpdate through the radar hub");
    tokio::time::timeout(WAIT, async {
        loop {
            match surface.recv().await {
                Ok((9, sent)) if Arc::ptr_eq(&sent, &frame) => return,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the surface closed"),
            }
        }
    })
    .await
    .expect("the frame reaches the surface");

    put_and_apply(&app, kraken_graph(ArrayNode::default()), 2).await;
    assert!(state.surfaces.subscribe("radar").is_none());
    assert!(draw(&state, 10, grid(10)));
    assert!(
        state.surfaces.subscribe("radar").is_none(),
        "a late frame does not reopen a removed surface"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn lagged_array_events_are_reported() {
    let (_app, state) = test_router_with_state();
    let mut events = state.engine.subscribe_events();

    let flow = array::handle(&state, Err(RecvError::Lagged(7)));

    assert!(flow.is_continue());
    assert_eq!(
        next_error(&mut events, "array updates lost", WAIT).await,
        Some("array updates lost: 7".to_owned())
    );
    assert!(array::handle(&state, Err(RecvError::Closed)).is_break());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_array_comes_back_tuned_and_keeps_its_tune_through_edits() {
    let (app, state) = test_router_with_state();
    let workspace =
        put_workspace_revision(&app, &snapshot(kraken_graph(ArrayNode::default())), 1).await;
    apply(&app, workspace).await;
    let (status, body) = tune(&app, ARRAY, r#"{"center_hz":868000000}"#).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    crate::workspace::save_active(&state).expect("saved");
    let saved = state.store.workspace_state(workspace).expect("state");
    assert_eq!(
        saved.array(ARRAY).map(|held| held.tune.center_hz),
        Some(868e6)
    );

    let restarted = state_over(state.store.clone());
    let (again, background) = router_with_state(restarted.clone(), &ServerOptions::default());
    background.detach();
    apply(&again, workspace).await;
    let center = |state: &AppState| status_of(state, ARRAY).map(|status| status.center_hz);
    assert_eq!(center(&restarted), Some(868e6), "the saved tune comes back");

    let (status, body) = tune(&again, ARRAY, r#"{"center_hz":433920000}"#).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let mut edited = ArrayNode::default();
    edited.cal.check_s = 0;
    put_and_apply(&again, kraken_graph(edited), 2).await;
    assert_eq!(
        center(&restarted),
        Some(433.92e6),
        "an edit keeps the live tune, not the saved one"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rewired_array_keeps_its_live_tune() {
    let (app, state) = test_router_with_state();
    put_and_apply(&app, kraken_graph(ArrayNode::default()), 1).await;
    let (status, body) = tune(
        &app,
        ARRAY,
        r#"{"center_hz":868000000,"gain":{"kind":"auto"}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let mut fewer = kraken_graph(ArrayNode::default());
    fewer
        .edges
        .retain(|edge| edge.to.port != sdrmm_wire::patch::stream_port("lane", 4));
    let report = put_and_apply(&app, fewer, 2).await;

    assert!(report.refused.is_empty(), "{:?}", report.refused);
    let status = status_of(&state, ARRAY).expect("running");
    assert_eq!(status.lanes.len(), 4);
    assert_eq!((status.center_hz, status.gain), (868e6, ArrayGain::Auto));
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_workspaces_stops_the_old_arrays() {
    let (app, state) = test_router_with_state();
    let mut graph = radar_graph();
    graph
        .nodes
        .push(node("df1", NodeBody::Df(DfNode::default())));
    graph.edges.push(wire((ARRAY, "array"), ("df1", "array")));
    put_and_apply(&app, graph, 1).await;
    assert!(status_of(&state, ARRAY).is_some());
    assert!(state.surfaces.subscribe("radar").is_some());

    let other = store_second_workspace(&app, "field", "nfm").await;
    activate(&app, other).await;

    assert!(state.engine.array_statuses().is_empty());
    assert!(state.arrays.binding(ARRAY).is_none());
    assert_eq!(array::processor_array(&state, "df1"), None);
    assert!(state.surfaces.subscribe("radar").is_none());
}
