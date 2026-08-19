use sdrmm_wire::{
    ArrayGeometry, DfFusionState, DfNode, DfParams, GuidanceMode, NavTargetKind, NodeBody,
    PatchEdge, PatchNode, PortRef, Position, WorkspaceSnapshot, stream_port,
};

use super::*;

const ARRAY_LANES: u32 = 4;

fn array_snapshot() -> WorkspaceSnapshot {
    let mut snapshot = virtual_snapshot("array4", &[]);
    snapshot.graph.nodes.push(PatchNode {
        id: "df".to_owned(),
        body: NodeBody::Df(DfNode {
            settings: DfParams {
                geometry: ArrayGeometry::Uca {
                    radius_m: 0.35,
                    count: ARRAY_LANES,
                },
                report_ms: 100,
                offset_hz: 25_000.0,
                bandwidth_hz: 20_000.0,
                ..DfParams::default()
            },
        }),
        position: Position { x: 700.0, y: 300.0 },
        size: None,
        label: None,
    });
    for lane in 0..ARRAY_LANES {
        snapshot.graph.edges.push(PatchEdge {
            from: PortRef {
                node: "device".to_owned(),
                port: stream_port("iq", lane),
            },
            to: PortRef {
                node: "df".to_owned(),
                port: stream_port("iq", lane),
            },
        });
    }
    snapshot
}

async fn staged_array(app: &Router) -> i64 {
    let snapshot = array_snapshot();
    let workspace = put_active_workspace(app, &snapshot).await;
    let report = apply(app, workspace).await;
    assert!(report.refused.is_empty(), "{report:?}");
    workspace
}

#[tokio::test]
async fn applying_a_patch_puts_the_direction_finder_on_the_array() {
    let (app, state) = test_router_with_state();
    staged_array(&app).await;
    let binding = state
        .coherent
        .binding("df")
        .expect("the direction finder is bound");
    assert_eq!(binding.kind, "df");
    assert_eq!(
        state.engine.coherent_nodes(binding.device_set),
        vec![binding.id]
    );
}

#[tokio::test]
async fn a_direction_finder_the_patch_no_longer_draws_is_taken_down() {
    let (app, state) = test_router_with_state();
    staged_array(&app).await;
    let binding = state.coherent.binding("df").expect("bound");

    let bare = virtual_snapshot("array4", &[]);
    let workspace = put_workspace_revision(&app, &bare, 2).await;
    apply(&app, workspace).await;
    assert!(state.coherent.binding("df").is_none());
    assert!(state.engine.coherent_nodes(binding.device_set).is_empty());
}

#[tokio::test]
async fn an_array_wired_to_only_some_of_its_lanes_is_refused_by_name() {
    let (app, _state) = test_router_with_state();
    let mut snapshot = array_snapshot();
    snapshot.graph.edges.retain(|edge| edge.to.port != "iq4");
    let workspace = put_active_workspace(&app, &snapshot).await;
    let report = apply(&app, workspace).await;
    let refusal = report
        .refused
        .iter()
        .find(|refusal| refusal.node == "df")
        .expect("the half-wired array is refused");
    assert!(refusal.reason.contains("lane"), "{refusal:?}");
}

#[tokio::test]
async fn calibration_can_be_asked_for_by_node_name() {
    let (app, _state) = test_router_with_state();
    staged_array(&app).await;
    let (status, _) = request(app.clone(), "POST", "/api/coherent/df/calibrate", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = request(app.clone(), "POST", "/api/coherent/ghost/calibrate", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let error: ApiError = serde_json::from_slice(&body).expect("json");
    assert!(error.error.contains("ghost"), "{error:?}");
}

#[tokio::test]
async fn a_finder_wired_to_a_triangulation_node_knows_where_to_cross_its_bearings() {
    let (app, state) = test_router_with_state();
    let mut snapshot = array_snapshot();
    snapshot.graph.nodes.push(PatchNode {
        id: "cross".to_owned(),
        body: NodeBody::Triangulation,
        position: Position { x: 900.0, y: 300.0 },
        size: None,
        label: None,
    });
    snapshot.graph.edges.push(PatchEdge {
        from: PortRef {
            node: "df".to_owned(),
            port: "events".to_owned(),
        },
        to: PortRef {
            node: "cross".to_owned(),
            port: "events".to_owned(),
        },
    });
    let workspace = put_active_workspace(&app, &snapshot).await;
    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{report:?}");
    let binding = state.coherent.binding("df").expect("bound");
    assert_eq!(binding.fusion_nodes, vec!["cross".to_owned()]);
    assert_eq!(
        binding.station("df"),
        "df",
        "an unnamed station is its node"
    );
}

#[tokio::test]
async fn two_finders_crossing_in_one_node_place_what_neither_could_alone() {
    let (app, state) = test_router_with_state();
    staged_array(&app).await;
    let target = crate::df_fusion::destination(51.5, 7.0, 45.0, 6_000.0);
    for (station, from) in [
        ("north", (51.5, 7.0)),
        (
            "east",
            crate::df_fusion::destination(51.5, 7.0, 135.0, 6_000.0),
        ),
    ] {
        let bearing = crate::df_fusion::bearing_between(from, target) as f32;
        for _ in 0..4 {
            state.fusion.observe(
                "cross",
                station,
                &sdrmm_wire::DfReading {
                    bearing_deg: bearing,
                    confidence: 0.95,
                    peak_to_floor_db: 20.0,
                    pseudospectrum: vec![0; 360],
                },
                Some(&sdrmm_wire::PositionFix {
                    latitude: from.0,
                    longitude: from.1,
                    altitude_m: None,
                    accuracy_m: None,
                    speed_mps: None,
                    track_deg: None,
                    time: "2026-01-01T00:00:00Z".to_owned(),
                }),
                "2026-01-01T00:00:00Z",
            );
        }
    }
    let (status, body) = request(app.clone(), "GET", "/api/coherent/cross/fusion", None).await;
    assert_eq!(status, StatusCode::OK);
    let fused: DfFusionState = serde_json::from_slice(&body).expect("json");
    let estimate = fused.estimate.expect("two stations give an estimate");
    let error = crate::df_fusion::distance_m((estimate.lat, estimate.lon), target);
    assert!(error < 800.0, "{error} m away: {estimate:?}");
    assert_eq!(fused.stations.len(), 2);

    let (status, _) = request(app.clone(), "DELETE", "/api/coherent/cross/fusion", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = request(app.clone(), "GET", "/api/coherent/cross/fusion", None).await;
    let cleared: DfFusionState = serde_json::from_slice(&body).expect("json");
    assert_eq!(cleared.samples, 0);
}

#[tokio::test]
async fn guidance_from_one_place_asks_the_operator_to_drive_across_the_bearing() {
    let (app, state) = test_router_with_state();
    staged_array(&app).await;
    let fix = sdrmm_wire::PositionFix {
        latitude: 51.5,
        longitude: 7.0,
        altitude_m: None,
        accuracy_m: None,
        speed_mps: None,
        track_deg: Some(0.0),
        time: "2026-01-01T00:00:00Z".to_owned(),
    };
    let reading = sdrmm_wire::DfReading {
        bearing_deg: 90.0,
        confidence: 0.9,
        peak_to_floor_db: 20.0,
        pseudospectrum: vec![0; 360],
    };
    let outcome = state
        .fusion
        .observe("cross", "north", &reading, Some(&fix), &fix.time)
        .expect("a fix and a bearing are enough");
    let guidance = outcome.state.guidance.expect("guidance");
    assert_eq!(guidance.mode, GuidanceMode::Cross);
    assert_eq!(guidance.nav_target.kind, NavTargetKind::Cross);
    assert!(
        (guidance.heading_deg - 0.0).abs() < 1e-6 || (guidance.heading_deg - 180.0).abs() < 1e-6,
        "{guidance:?}"
    );
}

fn array_node_snapshot(members: &[&str]) -> WorkspaceSnapshot {
    let mut snapshot = WorkspaceSnapshot::starter();
    snapshot.graph.nodes.retain(|node| node.id != "device");
    snapshot.graph.edges.clear();
    snapshot.graph.nodes.push(PatchNode {
        id: "bench".to_owned(),
        body: NodeBody::Array(sdrmm_wire::ArrayNode {
            members: members.iter().map(|member| (*member).to_owned()).collect(),
            coherence: sdrmm_wire::Coherence::TimeSync,
            shared_tuning: true,
        }),
        position: Position { x: 100.0, y: 300.0 },
        size: None,
        label: Some("Bench pair".to_owned()),
    });
    snapshot
}

#[tokio::test]
async fn an_array_drawn_on_the_canvas_opens_as_one_radio() {
    let (app, state) = test_router_with_state();
    let snapshot = array_node_snapshot(&["virtual:siggen", "virtual:halfduplex"]);
    let workspace = put_active_workspace(&app, &snapshot).await;
    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{report:?}");
    let bound = report
        .bound
        .iter()
        .find(|bound| bound.node == "bench")
        .expect("the array is bound like any other radio");
    let live = state.engine.snapshot();
    let set = live
        .device_sets
        .iter()
        .find(|set| set.id == bound.device_set)
        .expect("a device set for the array");
    assert_eq!(set.device.id(), "array:bench");
    assert_eq!(set.capabilities.rx_streams, 2, "one lane per member");
    assert_eq!(set.capabilities.coherence, sdrmm_wire::Coherence::TimeSync);
}

#[tokio::test]
async fn an_array_of_one_radio_opens_nothing() {
    let (app, state) = test_router_with_state();
    let snapshot = array_node_snapshot(&["virtual:siggen"]);
    let workspace = put_active_workspace(&app, &snapshot).await;
    apply(&app, workspace).await;
    assert!(state.engine.arrays().all().is_empty());
    assert!(state.engine.snapshot().device_sets.is_empty());
}
