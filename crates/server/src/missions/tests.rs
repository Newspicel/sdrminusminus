use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, Bytes},
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use sdrmm_wire::{
    ApiError, ArrayNode, Attitude, BearingSource, ChannelNode, DecodedRecord, DecoderEvent,
    DeviceNode, DeviceRef, DfNode, GpsNode, HeadingSource, HuntMission, HuntNode,
    MissionActionResponse, NodeBody, PassiveRadarNode, PatchEdge, PatchNode, PortRef, Position,
    PositionFix, PositionLink, PositionSource, RackLayout, ServerEvent, SignalMapNode, SweepState,
    TriangulationNode, WorkspaceSnapshot, patch::stream_port,
};
use tokio::sync::broadcast::{self, error::RecvError};
use tower::ServiceExt;

use super::*;
use crate::{
    ServerOptions, Store, main_router,
    phones::{scope::PHONE_ROUTES, tests::pair_one},
    tests::state_over,
};

const WAIT: Duration = Duration::from_secs(10);
const HUNT: &str = "hunt";
const VOICE: &str = "voice";
const STRANGER: &str = "p0123456789abcdef";

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

fn channel(id: &str) -> PatchNode {
    node(
        id,
        NodeBody::Channel(ChannelNode {
            channel_type: "nfm".to_owned(),
            record_calls: false,
            tuning_locked: false,
        }),
    )
}

fn gps(id: &str, source: PositionSource) -> PatchNode {
    node(
        id,
        NodeBody::Gps(GpsNode {
            source: Some(source),
        }),
    )
}

fn fixed() -> PositionSource {
    PositionSource::Fixed {
        lat: 48.1,
        lon: 11.5,
        altitude_m: None,
    }
}

fn phone(id: &str) -> PositionSource {
    PositionSource::Phone {
        phone: id.to_owned(),
    }
}

fn hunt_graph(position: Option<PositionSource>) -> PatchGraph {
    let mut graph = PatchGraph {
        nodes: vec![
            radio("radio", "siggen"),
            channel(VOICE),
            node(HUNT, NodeBody::Hunt(HuntNode::default())),
        ],
        edges: vec![
            wire(("radio", "iq"), (VOICE, "iq")),
            wire((HUNT, "control"), (VOICE, "control")),
        ],
    };
    if let Some(source) = position {
        graph.nodes.push(gps("gps", source));
        graph
            .edges
            .push(wire(("gps", "position"), (HUNT, "position")));
    }
    graph
}

fn fresh() -> AppState {
    state_over(Arc::new(Store::open(None).expect("in-memory store")))
}

fn store_graph(state: &AppState, name: &str, graph: PatchGraph) -> i64 {
    state
        .store
        .create_workspace(name, &WorkspaceSnapshot::new(graph, RackLayout::default()))
        .expect("workspace stored")
}

async fn bench_on(state: AppState, graph: PatchGraph, tls: bool) -> (Router, AppState, i64) {
    let id = store_graph(&state, "missions", graph);
    state.store.activate_workspace(id).expect("activate");
    let (app, background) = main_router(state.clone(), &ServerOptions::default(), tls);
    background.detach();
    let (status, body) = call(&app, "POST", &format!("/api/workspaces/{id}/apply"), None).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    (app, state, id)
}

async fn bench(graph: PatchGraph) -> (Router, AppState, i64) {
    bench_on(fresh(), graph, false).await
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
    bearer: Option<&str>,
) -> (StatusCode, Bytes) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_owned())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, bytes)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Bytes) {
    send(app, method, uri, body, None).await
}

async fn listing(app: &Router) -> MissionsResponse {
    let (status, body) = call(app, "GET", "/api/missions", None).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("missions")
}

fn find<'a>(listing: &'a MissionsResponse, node: &str) -> &'a Mission {
    listing
        .missions
        .iter()
        .find(|mission| mission.node == node)
        .unwrap_or_else(|| panic!("no mission {node} in {listing:?}"))
}

fn hunt_of(mission: &Mission) -> &HuntMission {
    match &mission.body {
        MissionBody::Hunt(hunt) => hunt,
        other => panic!("not a hunt: {other:?}"),
    }
}

async fn try_act(
    app: &Router,
    node: &str,
    action: &str,
    bearer: Option<&str>,
) -> (StatusCode, Bytes) {
    send(
        app,
        "POST",
        &format!("/api/missions/{node}/actions"),
        Some(action),
        bearer,
    )
    .await
}

async fn act(app: &Router, node: &str, action: &str) -> Mission {
    let (status, body) = try_act(app, node, action, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{action}: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice::<MissionActionResponse>(&body)
        .expect("mission")
        .mission
}

fn refusal(status: StatusCode, body: &Bytes) -> (StatusCode, String) {
    let error: ApiError = serde_json::from_slice(body).expect("ApiError");
    (status, error.error)
}

fn voice(state: &AppState) -> (u32, sdrmm_wire::ChannelInfo) {
    state
        .engine
        .snapshot()
        .device_sets
        .into_iter()
        .find_map(|set| {
            let channel = set
                .channels
                .into_iter()
                .find(|channel| channel.node.as_deref() == Some(VOICE))?;
            Some((set.id, channel))
        })
        .expect("the voice decoder is open")
}

fn hunt_status(state: &AppState) -> Option<sdrmm_wire::HuntStatus> {
    state
        .engine
        .snapshot()
        .device_sets
        .into_iter()
        .flat_map(|set| set.hunts)
        .next()
}

async fn put_graph(app: &Router, state: &AppState, id: i64, graph: PatchGraph) {
    let revision = state.store.workspace(id).expect("workspace").info.revision;
    let body = serde_json::json!({
        "revision": revision,
        "snapshot": WorkspaceSnapshot::new(graph, RackLayout::default()),
    });
    let (status, answer) = call(
        app,
        "PUT",
        &format!("/api/workspaces/{id}"),
        Some(&body.to_string()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&answer)
    );
}

async fn next_event(
    events: &mut broadcast::Receiver<ServerEvent>,
    wanted: impl Fn(&ServerEvent) -> bool,
) -> Option<ServerEvent> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(event)) if wanted(&event) => return Some(event),
            Ok(Ok(_) | Err(RecvError::Lagged(_))) => {}
            Ok(Err(RecvError::Closed)) | Err(_) => return None,
        }
    }
}

async fn missions_scopes(events: &mut broadcast::Receiver<ServerEvent>, within: Duration) -> usize {
    let deadline = tokio::time::Instant::now() + within;
    let mut count = 0;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(ServerEvent::StateChanged {
                scope: sdrmm_wire::StateScope::Missions,
            })) => count += 1,
            Ok(Ok(_) | Err(RecvError::Lagged(_))) => {}
            Ok(Err(RecvError::Closed)) | Err(_) => return count,
        }
    }
}

#[test]
fn every_phone_route_exists() {
    let spec = serde_json::to_value(crate::openapi()).expect("OpenAPI");
    for (method, path) in PHONE_ROUTES {
        if *path == "/api/ws" {
            continue;
        }
        assert!(
            spec["paths"][path][method.as_str().to_lowercase()].is_object(),
            "{method} {path} is open to phones but no route"
        );
    }
    for (method, path) in [
        (Method::GET, "/api/missions"),
        (Method::POST, "/api/missions/{node}/actions"),
        (Method::POST, "/api/missions/workspace"),
    ] {
        assert!(
            PHONE_ROUTES
                .iter()
                .any(|(open, route)| *open == method && *route == path),
            "{method} {path} is closed to phones"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hunt_mission_resolves_its_decoder() {
    let (app, state, id) = bench(hunt_graph(None)).await;
    let (device_set, decoder) = voice(&state);
    let listed = listing(&app).await;
    assert_eq!(listed.workspace.as_ref().map(|active| active.id), Some(id));
    assert!(listed.workspaces.iter().any(|workspace| workspace.id == id));
    let mission = find(&listed, HUNT);
    assert_eq!(mission.label, "Signal hunt");
    assert!(mission.ready, "{mission:?}");
    assert_eq!(
        mission.controls,
        vec![MissionControl::Tune, MissionControl::StartHunt]
    );
    let hunt = hunt_of(mission);
    let target = hunt.target.as_ref().expect("a target");
    assert_eq!(target.device_set, device_set);
    assert_eq!(target.channel, decoder.id);
    assert_eq!(target.channel_node, VOICE);
    assert_eq!(target.channel_type, "nfm");
    assert_eq!(target.frequency_hz, decoder.settings.frequency_hz);
    assert!(target.bandwidth_hz > 0.0);
    assert!(hunt.clicks);
    assert!(hunt.status.is_none());
    assert!(hunt.position.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unwired_hunt_says_so() {
    let mut graph = PatchGraph {
        nodes: vec![node(HUNT, NodeBody::Hunt(HuntNode::default()))],
        edges: Vec::new(),
    };
    graph.nodes.push(channel("orphan"));
    graph
        .nodes
        .push(node("idle", NodeBody::Hunt(HuntNode::default())));
    graph
        .edges
        .push(wire(("idle", "control"), ("orphan", "control")));
    let (app, _, _) = bench(graph).await;
    let listed = listing(&app).await;

    let unwired = find(&listed, HUNT);
    assert!(!unwired.ready);
    assert_eq!(
        unwired.problems,
        vec![MissionProblem::Unwired {
            port: "control".to_owned()
        }]
    );
    assert!(unwired.controls.is_empty());
    assert!(hunt_of(unwired).target.is_none());

    let idle = find(&listed, "idle");
    assert!(!idle.ready);
    assert_eq!(idle.problems, vec![MissionProblem::NotRunning]);
    assert!(idle.controls.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn hunts_start_and_stop_through_the_mission() {
    let (app, state, _) = bench(hunt_graph(None)).await;
    let started = act(&app, HUNT, r#"{"action":"start_hunt"}"#).await;
    assert!(hunt_of(&started).status.is_some());
    assert_eq!(
        started.controls,
        vec![MissionControl::Tune, MissionControl::StopHunt]
    );
    let running = hunt_status(&state).expect("a hunt runs");
    assert_eq!(running.settings.node.as_deref(), Some(HUNT));

    let stopped = act(&app, HUNT, r#"{"action":"stop_hunt"}"#).await;
    assert!(hunt_of(&stopped).status.is_none());
    assert!(stopped.controls.contains(&MissionControl::StartHunt));
    assert!(hunt_status(&state).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scanning_decoder_offers_no_hunt() {
    let (app, state, _) = bench(hunt_graph(Some(fixed()))).await;
    let (device_set, decoder) = voice(&state);
    let here = decoder.settings.frequency_hz;
    let scan = serde_json::json!({
        "action": "start",
        "settings": { "channel": decoder.id, "frequencies": [here, here + 25_000.0] },
    });
    let (status, body) = call(
        &app,
        "POST",
        &format!(
            "/api/devicesets/{device_set}/channels/{}/scanner",
            decoder.id
        ),
        Some(&scan.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let listed = listing(&app).await;
    let mission = find(&listed, HUNT);
    assert_eq!(mission.problems, vec![MissionProblem::Scanning]);
    assert!(!mission.ready);
    assert_eq!(mission.controls, vec![MissionControl::Tune]);
    let (status, body) = try_act(&app, HUNT, r#"{"action":"start_sweep"}"#, None).await;
    assert_eq!(refusal(status, &body).0, StatusCode::BAD_REQUEST);
    assert!(hunt_status(&state).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn tuning_a_hunt_moves_its_decoder_and_records_history() {
    let (app, state, id) = bench(hunt_graph(None)).await;
    let (_, before) = voice(&state);
    let wanted = before.settings.frequency_hz + 25_000.0;
    let tuned = act(
        &app,
        HUNT,
        &format!(r#"{{"action":"tune","frequency_hz":{wanted}}}"#),
    )
    .await;
    let target = hunt_of(&tuned).target.clone().expect("a target");
    assert_eq!(target.frequency_hz, wanted);
    assert_eq!(voice(&state).1.settings.frequency_hz, wanted);
    let detail = state.store.workspace(id).expect("workspace");
    assert!(detail.history.can_undo, "the tune is in the history");

    let (status, body) = try_act(&app, HUNT, r#"{"action":"tune","frequency_hz":-5}"#, None).await;
    assert_eq!(
        refusal(status, &body),
        (
            StatusCode::BAD_REQUEST,
            "Frequency must be positive".to_owned()
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_control_the_mission_lacks_is_refused() {
    let (app, state, _) = bench(hunt_graph(None)).await;
    for action in [r#"{"action":"stop_hunt"}"#, r#"{"action":"calibrate"}"#] {
        let (status, body) = try_act(&app, HUNT, action, None).await;
        assert_eq!(
            refusal(status, &body),
            (
                StatusCode::BAD_REQUEST,
                "Not a control of this mission".to_owned()
            ),
            "{action}"
        );
    }
    assert!(hunt_status(&state).is_none());
    let (status, body) = try_act(&app, "ghost", r#"{"action":"start_hunt"}"#, None).await;
    assert_eq!(
        refusal(status, &body),
        (StatusCode::NOT_FOUND, "No mission ghost".to_owned())
    );
    let (status, body) =
        try_act(&app, "radio", r#"{"action":"tune","frequency_hz":1}"#, None).await;
    assert_eq!(refusal(status, &body).0, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn triangulation_lists_sources_and_clears() {
    let graph = PatchGraph {
        nodes: vec![
            node("h1", NodeBody::Hunt(HuntNode::default())),
            node("h2", NodeBody::Hunt(HuntNode::default())),
            node(
                "filter",
                NodeBody::default_for("event_filter").expect("an event filter"),
            ),
            node("tri", NodeBody::Triangulation(TriangulationNode::default())),
            gps("car", fixed()),
        ],
        edges: vec![
            wire(("h1", "events"), ("filter", "events")),
            wire(("filter", "events"), ("tri", "events")),
            wire(("h2", "events"), ("tri", "events")),
            wire(("car", "position"), ("tri", "position")),
        ],
    };
    let (app, state, _) = bench(graph).await;
    let listed = listing(&app).await;
    let tri = find(&listed, "tri");
    assert!(tri.ready, "{tri:?}");
    assert_eq!(tri.controls, vec![MissionControl::ClearFusion]);
    let MissionBody::Triangulation(body) = &tri.body else {
        panic!("not a triangulation: {tri:?}");
    };
    let mut sources = body.sources.clone();
    sources.sort();
    assert_eq!(sources, ["h1", "h2"]);
    assert_eq!(
        body.position,
        Some(PositionLink {
            node: "car".to_owned(),
            phone: None
        })
    );
    assert!(body.state.is_some());
    for hunt in ["h1", "h2"] {
        assert_eq!(hunt_of(find(&listed, hunt)).triangulations, ["tri"]);
    }

    let mut events = state.engine.subscribe_events();
    let cleared = act(&app, "tri", r#"{"action":"clear_fusion"}"#).await;
    assert_eq!(cleared.node, "tri");
    let update = next_event(
        &mut events,
        |event| matches!(event, ServerEvent::DfFusionUpdate { node, .. } if node == "tri"),
    )
    .await;
    assert!(update.is_some(), "clearing tells the clients");
}

fn surveyed(positioned: bool) -> PatchGraph {
    let mut graph = PatchGraph {
        nodes: vec![
            radio("radio", "siggen"),
            node("map", NodeBody::SignalMap(SignalMapNode::default())),
            gps("gps", fixed()),
        ],
        edges: vec![wire(("radio", "iq"), ("map", "iq"))],
    };
    if positioned {
        graph
            .edges
            .push(wire(("gps", "position"), ("map", "position")));
    }
    graph
}

fn survey_of(mission: &Mission) -> &sdrmm_wire::SurveyMission {
    match &mission.body {
        MissionBody::Survey(survey) => survey,
        other => panic!("not a survey: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn survey_records_through_the_mission() {
    let (app, state, _) = bench(surveyed(true)).await;
    let listed = listing(&app).await;
    let idle = find(&listed, "map");
    assert!(idle.ready, "{idle:?}");
    assert_eq!(idle.controls, vec![MissionControl::StartSurvey]);
    let body = survey_of(idle);
    let set = state.engine.snapshot().device_sets[0].clone();
    assert_eq!(body.device_set, Some(set.id));
    assert_eq!(body.stream, 0);
    assert_eq!(
        body.frequency_hz,
        set.settings
            .center_hz
            .map(|center| center + body.offset_hz as f64)
    );
    assert!(!body.recording);

    let mut events = state.engine.subscribe_events();
    let started = act(&app, "map", r#"{"action":"start_survey"}"#).await;
    assert!(survey_of(&started).recording);
    assert_eq!(started.controls, vec![MissionControl::StopSurvey]);
    let cell = next_event(&mut events, |event| {
        matches!(event, ServerEvent::SurveyUpdate { node, update } if node == "map" && update.cell.is_some())
    })
    .await;
    assert!(cell.is_some(), "a cell is surveyed");

    let stopped = act(&app, "map", r#"{"action":"stop_survey"}"#).await;
    assert!(!survey_of(&stopped).recording);
    assert!(survey_of(&stopped).cells > 0);
    assert_eq!(
        stopped.controls,
        vec![MissionControl::StartSurvey, MissionControl::ClearSurvey]
    );
    let cleared = act(&app, "map", r#"{"action":"clear_survey"}"#).await;
    assert_eq!(survey_of(&cleared).cells, 0);
    assert_eq!(cleared.controls, vec![MissionControl::StartSurvey]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_survey_without_a_position_cannot_start() {
    let (app, _, _) = bench(surveyed(false)).await;
    let listed = listing(&app).await;
    let map = find(&listed, "map");
    assert_eq!(map.problems, vec![MissionProblem::NoPosition]);
    assert!(map.controls.is_empty());
    let (status, body) = try_act(&app, "map", r#"{"action":"start_survey"}"#, None).await;
    assert_eq!(refusal(status, &body).0, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_position_reports_offline() {
    let state = fresh();
    let paired = pair_one(&state.phones);
    let phone_id = paired.phone.id.clone();
    let mut graph = hunt_graph(Some(phone(&phone_id)));
    graph
        .nodes
        .push(node("other", NodeBody::Hunt(HuntNode::default())));
    graph.nodes.push(gps("borrowed", phone(STRANGER)));
    graph
        .edges
        .push(wire(("borrowed", "position"), ("other", "position")));
    let (app, state, _) = bench_on(state, graph, false).await;

    let listed = listing(&app).await;
    let mission = find(&listed, HUNT);
    assert_eq!(
        mission.problems,
        vec![MissionProblem::PhoneOffline {
            phone: phone_id.clone()
        }]
    );
    assert!(!mission.ready);
    assert_eq!(
        hunt_of(mission).position,
        Some(PositionLink {
            node: "gps".to_owned(),
            phone: Some(phone_id.clone())
        })
    );
    assert!(
        find(&listed, "other")
            .problems
            .contains(&MissionProblem::PhoneNotPaired {
                phone: STRANGER.to_owned()
            })
    );

    let (_session, _) = state.phones.join(&phone_id);
    let online = listing(&app).await;
    assert!(find(&online, HUNT).ready, "{online:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_revision_ignores_live_state() {
    let (app, state, id) = bench(hunt_graph(None)).await;
    act(&app, HUNT, r#"{"action":"start_hunt"}"#).await;
    let first = listing(&app).await;
    let readings = |listed: &MissionsResponse| {
        hunt_of(find(listed, HUNT))
            .status
            .as_ref()
            .map_or(0, |status| status.readings)
    };
    let mut events = state.engine.subscribe_events();
    let later = readings(&first) + 2;
    let moved = next_event(
        &mut events,
        |event| matches!(event, ServerEvent::HuntUpdate { status, .. } if status.readings >= later),
    )
    .await;
    assert!(moved.is_some(), "the hunt keeps reading");
    let second = listing(&app).await;
    assert!(readings(&second) > readings(&first));
    assert_eq!(second.revision, first.revision);

    let mut rewired = hunt_graph(None);
    rewired.edges.retain(|edge| edge.from.node != HUNT);
    put_graph(&app, &state, id, rewired).await;
    let third = listing(&app).await;
    assert_ne!(third.revision, second.revision);

    let mut renamed = third.clone();
    renamed.revision = 7;
    assert_eq!(revision(&renamed).expect("revision"), third.revision);
    if let Some(mission) = renamed.missions.first_mut() {
        mission.label = "Fox".to_owned();
    }
    assert_ne!(revision(&renamed).expect("revision"), third.revision);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_missions_scope_follows_the_graph() {
    let (app, state, id) = bench(hunt_graph(None)).await;
    let mut events = state.engine.subscribe_events();
    let mut grown = hunt_graph(None);
    grown
        .nodes
        .push(node("second", NodeBody::Hunt(HuntNode::default())));
    put_graph(&app, &state, id, grown.clone()).await;
    assert_eq!(
        missions_scopes(&mut events, watch::DEBOUNCE * 6).await,
        1,
        "one real change, one scope"
    );

    if let Some(moved) = grown.nodes.iter_mut().find(|patch| patch.id == "second") {
        moved.position = Position { x: 240.0, y: 80.0 };
    }
    put_graph(&app, &state, id, grown).await;
    assert_eq!(
        missions_scopes(&mut events, watch::DEBOUNCE * 4).await,
        0,
        "moving a node changes no mission"
    );
}

#[test]
fn the_listing_is_capped_and_says_so() {
    let state = fresh();
    let graph = PatchGraph {
        nodes: (0..70)
            .map(|index| node(&format!("hunt{index}"), NodeBody::Hunt(HuntNode::default())))
            .collect(),
        edges: Vec::new(),
    };
    let id = store_graph(&state, "crowd", graph);
    state.store.activate_workspace(id).expect("activate");
    let listed = missions(&state).expect("listing");
    assert_eq!(listed.missions.len(), MAX_MISSIONS);
    assert_eq!(listed.truncated, 6);
    assert_eq!(listed.missions[0].node, "hunt0");
    assert_eq!(listed.missions[MAX_MISSIONS - 1].node, "hunt63");
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_workspace_activates_and_applies() {
    let (app, state, first) = bench(hunt_graph(None)).await;
    let second = store_graph(
        &state,
        "quad",
        PatchGraph {
            nodes: vec![
                radio("quad", "quad"),
                node("fox", NodeBody::Hunt(HuntNode::default())),
            ],
            edges: Vec::new(),
        },
    );
    let (status, body) = call(
        &app,
        "POST",
        "/api/missions/workspace",
        Some(&format!(r#"{{"workspace":{second}}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let switched: MissionsResponse = serde_json::from_slice(&body).expect("missions");
    assert_eq!(
        switched.workspace.as_ref().map(|active| active.id),
        Some(second)
    );
    for id in [first, second] {
        assert!(switched.workspaces.iter().any(|listed| listed.id == id));
    }
    assert!(
        switched
            .missions
            .iter()
            .any(|mission| mission.node == "fox")
    );
    assert!(!switched.missions.iter().any(|mission| mission.node == HUNT));
    assert_eq!(
        state.store.active_workspace_id().expect("active"),
        Some(second)
    );
    let radios: Vec<String> = state
        .engine
        .snapshot()
        .device_sets
        .iter()
        .map(|set| set.device.id())
        .collect();
    assert_eq!(
        radios,
        ["virtual:quad"],
        "the old radio closed, the new one opened"
    );

    let (status, _) = call(
        &app,
        "POST",
        "/api/missions/workspace",
        Some(r#"{"workspace":9999}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn array_graph() -> PatchGraph {
    let mut graph = PatchGraph {
        nodes: vec![
            radio("kraken", "kraken5"),
            node("arr", NodeBody::Array(ArrayNode::default())),
            node("df", NodeBody::Df(DfNode::default())),
            node("lonely", NodeBody::Df(DfNode::default())),
            node("tri", NodeBody::Triangulation(TriangulationNode::default())),
            node("radar", NodeBody::PassiveRadar(PassiveRadarNode::default())),
            gps("roof", fixed()),
            gps("mast", fixed()),
        ],
        edges: vec![
            wire(("arr", "array"), ("df", "array")),
            wire(("arr", "array"), ("radar", "array")),
            wire(("df", "events"), ("tri", "events")),
            wire(("roof", "position"), ("arr", "position")),
            wire(("mast", "position"), ("radar", "tx")),
        ],
    };
    for lane in 0..5 {
        graph.edges.push(wire(
            ("kraken", &stream_port("iq", lane)),
            ("arr", &stream_port("lane", lane)),
        ));
    }
    graph
}

#[tokio::test(flavor = "multi_thread")]
async fn df_and_radar_missions_use_the_array() {
    let (app, state, _) = bench(array_graph()).await;
    let listed = listing(&app).await;
    let device_set = state.engine.snapshot().device_sets[0].id;
    let roof = Some(PositionLink {
        node: "roof".to_owned(),
        phone: None,
    });

    let df = find(&listed, "df");
    assert!(
        df.problems
            .iter()
            .all(|problem| matches!(problem, MissionProblem::Refused { .. })),
        "{df:?}"
    );
    assert_eq!(
        df.controls,
        vec![MissionControl::Tune, MissionControl::Calibrate]
    );
    let MissionBody::Df(body) = &df.body else {
        panic!("not a df mission: {df:?}");
    };
    assert_eq!(body.array.as_deref(), Some("arr"));
    assert_eq!(body.device_sets, [device_set]);
    assert!(body.center_hz.is_some());
    assert_eq!(body.position, roof);
    assert_eq!(body.triangulations, ["tri"]);

    let radar = find(&listed, "radar");
    let MissionBody::Radar(body) = &radar.body else {
        panic!("not a radar mission: {radar:?}");
    };
    assert_eq!(body.array.as_deref(), Some("arr"));
    assert_eq!(body.device_sets, [device_set]);
    assert_eq!(body.position, roof);
    assert_eq!(
        body.transmitter,
        Some(PositionLink {
            node: "mast".to_owned(),
            phone: None
        })
    );
    assert!(body.surface);

    let lonely = find(&listed, "lonely");
    assert_eq!(
        lonely.problems,
        vec![MissionProblem::Unwired {
            port: "array".to_owned()
        }]
    );
    assert!(lonely.controls.is_empty());

    let tuned = act(&app, "df", r#"{"action":"tune","frequency_hz":433920000}"#).await;
    let MissionBody::Df(body) = &tuned.body else {
        panic!("not a df mission: {tuned:?}");
    };
    assert_eq!(body.center_hz, Some(433.92e6));
    let calibrating = act(&app, "radar", r#"{"action":"calibrate"}"#).await;
    assert_eq!(calibrating.node, "radar");
    let (status, body) = try_act(&app, "df", r#"{"action":"tune","frequency_hz":0}"#, None).await;
    assert_eq!(refusal(status, &body).0, StatusCode::BAD_REQUEST);
}

fn pose(heading_deg: f64) -> PositionFix {
    PositionFix {
        latitude: 48.1,
        longitude: 11.5,
        altitude_m: None,
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

#[tokio::test(flavor = "multi_thread")]
async fn hunt_with_a_position_wire_offers_sweep_and_mark() {
    let (app, state, _) = bench(hunt_graph(Some(fixed()))).await;
    let listed = listing(&app).await;
    let mission = find(&listed, HUNT);
    assert_eq!(
        mission.controls,
        vec![
            MissionControl::Tune,
            MissionControl::StartHunt,
            MissionControl::StartSweep
        ]
    );
    assert_eq!(
        hunt_of(mission).position,
        Some(PositionLink {
            node: "gps".to_owned(),
            phone: None
        })
    );

    let started = act(&app, HUNT, r#"{"action":"start_hunt"}"#).await;
    assert_eq!(
        started.controls,
        vec![
            MissionControl::Tune,
            MissionControl::StopHunt,
            MissionControl::StartSweep,
            MissionControl::Mark
        ]
    );
    let (device_set, decoder) = voice(&state);
    let received_ms = u64::try_from(jiff::Timestamp::now().as_millisecond()).expect("after 1970");
    state
        .engine
        .hunt_pose(device_set, decoder.id, pose(135.0), received_ms)
        .expect("the hunt takes poses");
    let mut records = state.engine.subscribe_decoded();
    let marked = act(&app, HUNT, r#"{"action":"mark"}"#).await;
    assert!(hunt_of(&marked).status.is_some());
    let bearing = tokio::time::timeout(WAIT, async {
        loop {
            match records.recv().await {
                Ok(DecodedRecord {
                    event: DecoderEvent::Df(bearing),
                    ..
                }) => return bearing,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the decoded feed closed"),
            }
        }
    })
    .await
    .expect("the mark files a bearing");
    assert_eq!(bearing.node, HUNT);
    assert_eq!(bearing.source, BearingSource::Mark);
    assert!(
        (bearing.bearing_deg - 135.0).abs() < 0.5,
        "{}",
        bearing.bearing_deg
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_can_be_turned_off_from_a_phone() {
    let state = fresh();
    let paired = pair_one(&state.phones);
    let graph = hunt_graph(Some(phone(&paired.phone.id)));
    let (app, state, _) = bench_on(state, graph, true).await;
    let token = Some(paired.token.as_str());

    let (status, body) = try_act(&app, HUNT, r#"{"action":"start_sweep"}"#, token).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let sweeping: MissionActionResponse = serde_json::from_slice(&body).expect("mission");
    assert!(
        sweeping
            .mission
            .controls
            .contains(&MissionControl::StopSweep),
        "{sweeping:?}"
    );
    let sweep = hunt_status(&state)
        .and_then(|status| status.sweep)
        .expect("a sweep");
    assert_ne!(sweep.state, SweepState::Off);

    let (status, body) = try_act(&app, HUNT, r#"{"action":"stop_sweep"}"#, token).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let stopped: MissionActionResponse = serde_json::from_slice(&body).expect("mission");
    let controls = &stopped.mission.controls;
    assert!(
        controls.contains(&MissionControl::StartSweep),
        "{controls:?}"
    );
    assert!(controls.contains(&MissionControl::StopHunt), "{controls:?}");
    let running = hunt_status(&state).expect("the hunt keeps running");
    assert_eq!(
        running.sweep.map(|sweep| sweep.state),
        Some(SweepState::Off)
    );
}
