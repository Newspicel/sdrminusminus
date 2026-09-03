use super::*;

#[tokio::test]
async fn applying_a_workspace_opens_its_radio_and_adds_its_channels_once() {
    let app = test_router();
    let snapshot = virtual_snapshot("siggen", &[("nfm", "nfm", "iq"), ("am", "am", "iq")]);
    let workspace = put_active_workspace(&app, &snapshot).await;

    let first = apply(&app, workspace).await;
    assert_eq!(first.opened, 1);
    assert_eq!(first.created, 2);
    assert_eq!(first.bound.len(), 1);
    assert_eq!(first.bound[0].node, "device");
    assert!(first.absent.is_empty());
    assert!(first.refused.is_empty(), "{:?}", first.refused);

    let second = apply(&app, workspace).await;
    assert_eq!(second.opened, 0, "apply is idempotent");
    assert_eq!(second.created, 0);
    assert_eq!(second.bound, first.bound);

    let state = get_state(&app).await;
    assert_eq!(state.device_sets.len(), 1);
    let types: Vec<&str> = state.device_sets[0]
        .channels
        .iter()
        .map(|c| c.settings.params.type_id())
        .collect();
    assert_eq!(types, vec!["nfm", "am"]);
}

#[tokio::test]
async fn applying_a_workspace_reports_an_absent_radio() {
    let app = test_router();
    let mut snapshot = sdrmm_wire::WorkspaceSnapshot::starter();
    let sdrmm_wire::NodeBody::Device(node) = &mut snapshot.graph.nodes[0].body else {
        panic!("the default workspace opens with a receiver")
    };
    node.device = Some(sdrmm_wire::DeviceRef {
        backend: "hackrf".to_string(),
        serial: Some("deadbeef".to_string()),
        key: None,
    });
    let workspace = put_active_workspace(&app, &snapshot).await;

    let report = apply(&app, workspace).await;
    assert_eq!(report.absent, vec!["device".to_string()]);
    assert_eq!(report.opened, 0);
    assert!(report.bound.is_empty());
    assert!(get_state(&app).await.device_sets.is_empty());
}

#[tokio::test]
async fn undoing_a_workspace_takes_the_engine_back_with_it() {
    let app = test_router();
    let workspace =
        put_active_workspace(&app, &virtual_snapshot("siggen", &[("nfm", "nfm", "iq")])).await;
    apply(&app, workspace).await;

    let two = virtual_snapshot("siggen", &[("nfm", "nfm", "iq"), ("am", "am", "iq")]);
    let revision = workspace_detail(&app, workspace).await.info.revision;
    let (status, body) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{workspace}"),
        Some(&format!(
            r#"{{"revision":{revision},"snapshot":{}}}"#,
            serde_json::to_string(&two).unwrap()
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    apply(&app, workspace).await;
    assert_eq!(channel_types(&app).await, vec!["nfm", "am"]);

    let undone = step(&app, workspace, "undo").await;
    assert_eq!(undone.snapshot.graph.nodes.len(), two.graph.nodes.len() - 1);
    assert!(undone.history.can_undo && undone.history.can_redo);
    assert_eq!(
        channel_types(&app).await,
        vec!["nfm"],
        "the channel the undone step created is closed, not left running"
    );
    assert_eq!(
        workspace_detail(&app, workspace).await.snapshot,
        undone.snapshot
    );

    let redone = step(&app, workspace, "redo").await;
    assert_eq!(redone.snapshot, two);
    assert!(!redone.history.can_redo);
    assert_eq!(channel_types(&app).await, vec!["nfm", "am"]);

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/workspaces/{workspace}/redo"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");
}

#[tokio::test]
async fn undoing_a_dial_move_puts_the_frequency_back() {
    let app = test_router();
    let workspace =
        put_active_workspace(&app, &virtual_snapshot("siggen", &[("nfm", "nfm", "iq")])).await;
    apply(&app, workspace).await;

    let opened = get_state(&app).await;
    let set = &opened.device_sets[0];
    let (ds, ch) = (set.id, set.channels[0].id);
    let was_center = set
        .settings
        .center_hz
        .expect("the radio is tuned somewhere");
    let was_offset = set.channels[0].settings.offset_hz;

    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(&format!(r#"{{"center_hz":{}}}"#, was_center + 1_000_000.0)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{ch}"),
        Some(r#"{"offset_hz":25000.0,"params":{"type":"nfm","settings":{}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let moved = get_state(&app).await;
    assert_eq!(
        moved.device_sets[0].channels[0].settings.offset_hz,
        25_000.0
    );
    assert!(
        workspace_detail(&app, workspace).await.history.can_undo,
        "a dial move is a step the history knows about"
    );

    let undone = step(&app, workspace, "undo").await;
    assert!(undone.history.can_redo);
    let back = get_state(&app).await;
    assert_eq!(
        back.device_sets[0].channels[0].settings.offset_hz, was_offset,
        "the channel was left where the undone step put it"
    );
    assert_eq!(
        back.device_sets[0].settings.center_hz,
        Some(was_center + 1_000_000.0),
        "undo walked back one step, not both"
    );

    step(&app, workspace, "undo").await;
    assert_eq!(
        get_state(&app).await.device_sets[0].settings.center_hz,
        Some(was_center)
    );

    step(&app, workspace, "redo").await;
    assert_eq!(
        get_state(&app).await.device_sets[0].settings.center_hz,
        Some(was_center + 1_000_000.0),
        "redo puts the radio back where the undo took it from"
    );
}

#[tokio::test]
async fn a_radio_the_canvas_does_not_draw_records_no_history() {
    let app = test_router();
    let workspace = workspaces(&app).await.active.expect("seeded workspace");
    let ds = create_virtual_set(&app).await;

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":123000000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!workspace_detail(&app, workspace).await.history.can_undo);
}

#[tokio::test]
async fn workspace_crud_over_http() {
    let app = test_router();
    let seeded = workspaces(&app).await;
    let workspace = seeded.workspaces[0].id;
    assert_eq!(seeded.active, Some(workspace));

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/workspaces",
        Some(r#"{"name":"Bench"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let created: sdrmm_wire::CreatedRowId = serde_json::from_slice(&body).expect("json");

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/workspaces",
        Some(r#"{"name":"Bench"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/workspaces/{}/activate", created.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(workspaces(&app).await.active, Some(created.id));

    let (status, body) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{}", created.id),
        Some(r#"{"revision":1,"snapshot":{"version":1,"graph":{"nodes":[]}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, body) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{}", created.id),
        Some(
            r#"{"revision":1,"snapshot":{"version":2,"graph":{"nodes":[],"edges":[
                   {"from":{"node":"a","port":"iq"},"to":{"node":"b","port":"iq"}}]}}}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let snapshot =
        serde_json::to_string(&sdrmm_wire::WorkspaceSnapshot::starter()).expect("snapshot");
    let (status, body) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{}", created.id),
        Some(&format!(r#"{{"revision":1,"snapshot":{snapshot}}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let info: sdrmm_wire::WorkspaceInfo = serde_json::from_slice(&body).expect("json");
    assert_eq!(info.revision, 2);

    let (status, _) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{}", created.id),
        Some(&format!(r#"{{"revision":1,"snapshot":{snapshot}}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = request(
        app.clone(),
        "DELETE",
        &format!("/api/workspaces/{}", created.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let after = workspaces(&app).await;
    assert_eq!(after.workspaces.len(), 1);
    assert_eq!(
        after.active,
        Some(workspace),
        "deleting the active one promotes"
    );

    let (status, _) = request(
        app.clone(),
        "GET",
        &format!("/api/workspaces/{}", created.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_workspace_comes_back_tuned_the_way_it_was_left() {
    let (app, state) = test_router_with_state();
    let workspace = store_siggen_workspace(&app).await;
    assert_eq!(apply(&app, workspace).await.created, 1);

    let ds = get_state(&app).await.device_sets[0].id;
    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":145500000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let channel = get_state(&app).await.device_sets[0].channels[0].id;
    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{channel}"),
        Some(r#"{"offset_hz":12500.0,"squelch_db":-42.0,"params":{"type":"nfm","settings":{}}}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );

    workspace::save_active(&state).expect("capture the workspace");

    let restarted = state_over(state.store.clone());
    let (app, background) = router_with_state(restarted, &ServerOptions::default());
    background.detach();
    assert!(get_state(&app).await.device_sets.is_empty());

    let report = apply(&app, workspace).await;
    assert_eq!(report.opened, 1);
    assert_eq!(report.created, 1);
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let set = &get_state(&app).await.device_sets[0];
    assert_eq!(set.settings.center_hz, Some(145_500_000.0));
    assert_eq!(set.channels.len(), 1, "no duplicate channel on restore");
    assert_eq!(set.channels[0].settings.offset_hz, 12_500.0);
    assert_eq!(set.channels[0].settings.squelch_db, Some(-42.0));
}

#[tokio::test]
async fn a_hand_picked_radio_comes_up_with_the_nodes_stored_settings() {
    let (app, state) = test_router_with_state();
    let workspace = store_siggen_workspace(&app).await;
    apply(&app, workspace).await;
    let ds = get_state(&app).await.device_sets[0].id;
    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":145500000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    workspace::save_active(&state).expect("capture the workspace");

    let restarted = state_over(state.store.clone());
    let (app, background) = router_with_state(restarted, &ServerOptions::default());
    background.detach();
    create_virtual_set(&app).await;
    apply(&app, workspace).await;
    assert_eq!(
        get_state(&app).await.device_sets[0].settings.center_hz,
        Some(145_500_000.0)
    );
}

#[tokio::test]
async fn a_partial_restore_lands_what_fits_and_keeps_remembering_the_rest() {
    let (app, state) = test_router_with_state();
    let workspace = store_siggen_workspace(&app).await;
    let planted = sdrmm_wire::WorkspaceState {
        trunks: Vec::new(),
        version: sdrmm_wire::WORKSPACE_STATE_VERSION,
        devices: vec![sdrmm_wire::WorkspaceDevice {
            node: "device".to_string(),
            settings: DeviceSettings {
                center_hz: Some(145_500_000.0),
                sample_rate: Some(999.0),
                extra: vec![sdrmm_wire::ExtraValue {
                    name: "bias_tee".to_string(),
                    value: true.into(),
                }],
                ..DeviceSettings::default()
            },
            channels: Vec::new(),
        }],
    };
    state
        .store
        .put_workspace_state(workspace, &planted)
        .expect("plant the stored settings");

    let report = apply(&app, workspace).await;
    assert!(
        report.refused.is_empty(),
        "a radio without the other one's bias tee is not a refusal: {report:?}"
    );
    assert_eq!(
        get_state(&app).await.device_sets[0].settings.center_hz,
        Some(145_500_000.0),
        "the frequency this radio can reach still landed"
    );

    workspace::save_active(&state).expect("capture the workspace");
    let stored = state
        .store
        .workspace_state(workspace)
        .expect("read the stored settings");
    let kept = stored.device("device").expect("kept");
    assert_eq!(
        kept.settings.sample_rate,
        Some(999.0),
        "the node goes on remembering what this radio could not take"
    );
    assert_eq!(kept.settings.extra.len(), 1);
}

#[tokio::test]
async fn applying_a_workspace_does_not_retune_an_open_radio() {
    let (app, state) = test_router_with_state();
    let workspace = store_siggen_workspace(&app).await;
    apply(&app, workspace).await;

    let ds = get_state(&app).await.device_sets[0].id;
    let tune = async |hz: f64| {
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(&format!(r#"{{"center_hz":{hz}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    };

    tune(145_500_000.0).await;
    workspace::save_active(&state).expect("capture the workspace");
    tune(433_800_000.0).await;

    apply(&app, workspace).await;
    assert_eq!(
        get_state(&app).await.device_sets[0].settings.center_hz,
        Some(433_800_000.0)
    );
}

#[tokio::test]
async fn switching_workspaces_closes_the_radios_the_new_one_does_not_name() {
    let (app, _state) = test_router_with_state();
    let workspace = store_siggen_workspace(&app).await;
    apply(&app, workspace).await;
    assert_eq!(get_state(&app).await.device_sets.len(), 1);

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/workspaces",
        Some(r#"{"name":"Empty"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let created: sdrmm_wire::CreatedRowId = serde_json::from_slice(&body).expect("json");
    activate(&app, created.id).await;

    assert!(
        get_state(&app).await.device_sets.is_empty(),
        "the radio the previous workspace opened is still running"
    );
}

#[tokio::test]
async fn switching_between_workspaces_sharing_a_radio_restores_each_ones_settings() {
    let (app, _state) = test_router_with_state();
    let first = store_siggen_workspace(&app).await;
    apply(&app, first).await;
    let ds = get_state(&app).await.device_sets[0].id;

    let tune = async |hz: f64| {
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(&format!(r#"{{"center_hz":{hz}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    };
    tune(145_500_000.0).await;

    let second = store_second_workspace(&app, "Marine", "am").await;
    activate(&app, second).await;

    let sets = get_state(&app).await.device_sets;
    assert_eq!(sets.len(), 1, "the shared radio was closed and reopened");
    assert_eq!(sets[0].id, ds, "the shared radio was closed and reopened");
    assert!(
        sets[0].channels.is_empty(),
        "the previous workspace's channel is still running"
    );

    apply(&app, second).await;
    tune(162_000_000.0).await;
    let sets = get_state(&app).await.device_sets;
    assert_eq!(sets[0].channels.len(), 1);
    assert_eq!(sets[0].channels[0].settings.params.type_id(), "am");

    activate(&app, first).await;
    let sets = get_state(&app).await.device_sets;
    assert_eq!(
        sets[0].settings.center_hz,
        Some(145_500_000.0),
        "the first workspace came back on the second one's frequency"
    );
    assert!(
        sets[0].channels.is_empty(),
        "the second workspace's channel is still running"
    );

    apply(&app, first).await;
    let sets = get_state(&app).await.device_sets;
    assert_eq!(sets[0].channels.len(), 1);
    assert_eq!(sets[0].channels[0].settings.params.type_id(), "nfm");
    assert_eq!(sets[0].settings.center_hz, Some(145_500_000.0));
}

#[tokio::test]
async fn a_capture_without_the_radio_keeps_its_stored_settings() {
    let (app, state) = test_router_with_state();
    let workspace = store_siggen_workspace(&app).await;
    apply(&app, workspace).await;
    let ds = get_state(&app).await.device_sets[0].id;
    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":145500000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    workspace::save_active(&state).expect("capture the workspace");

    let (status, _) = request(
        app.clone(),
        "DELETE",
        &format!("/api/devicesets/{ds}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    workspace::save_active(&state).expect("capture the empty workspace");

    let stored = state
        .store
        .workspace_state(workspace)
        .expect("workspace state");
    assert_eq!(
        stored
            .device("device")
            .expect("device entry")
            .settings
            .center_hz,
        Some(145_500_000.0)
    );
}

#[tokio::test]
async fn a_workspace_remembers_per_stream_overrides() {
    let (app, state) = test_router_with_state();
    let workspace = put_active_workspace(&app, &virtual_snapshot("transceiver", &[])).await;
    apply(&app, workspace).await;
    let ds = get_state(&app).await.device_sets[0].id;
    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"streams":[{"stream":1,"center_hz":433920000.0}]}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    workspace::save_active(&state).expect("capture the workspace");

    let restarted = state_over(state.store.clone());
    let (app, background) = router_with_state(restarted, &ServerOptions::default());
    background.detach();
    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let set = &get_state(&app).await.device_sets[0];
    assert_eq!(set.settings.streams.len(), 1, "{:?}", set.settings.streams);
    assert_eq!(set.settings.streams[0].stream, 1);
    assert_eq!(set.settings.streams[0].center_hz, Some(433_920_000.0));
}

#[tokio::test]
async fn applying_a_workspace_lands_each_channel_on_the_stream_its_wire_names() {
    let app = test_router();
    let taps = [("low", "nfm", "iq"), ("high", "nfm", "iq4")];
    let workspace = put_active_workspace(&app, &virtual_snapshot("array4", &taps)).await;

    let report = apply(&app, workspace).await;
    assert_eq!(report.created, 2);
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    let streams: Vec<u32> = get_state(&app).await.device_sets[0]
        .channels
        .iter()
        .map(|channel| channel.stream)
        .collect();
    assert_eq!(streams, vec![0, 3], "the iq4 wire must land on stream 3");

    let second = apply(&app, workspace).await;
    assert_eq!(
        second.created, 0,
        "apply duplicated a channel across streams"
    );
}

#[tokio::test]
async fn a_wire_to_a_stream_the_radio_does_not_have_is_refused_not_moved() {
    let app = test_router();
    let taps = [("voice", "nfm", "iq3")];
    let workspace = put_active_workspace(&app, &virtual_snapshot("siggen", &taps)).await;

    let report = apply(&app, workspace).await;
    assert_eq!(report.opened, 1, "the radio itself is fine and must open");
    assert_eq!(report.created, 0);
    assert_eq!(report.refused.len(), 1, "{:?}", report.refused);
    assert_eq!(report.refused[0].node, "voice");
    assert!(
        report.refused[0].reason.contains("1 rx streams"),
        "the refusal must name the count: {}",
        report.refused[0].reason
    );
    assert!(
        get_state(&app).await.device_sets[0].channels.is_empty(),
        "the channel must not come up on another stream"
    );
}

#[tokio::test]
async fn capture_and_restore_pair_same_type_channels_by_stream() {
    let (app, state) = test_router_with_state();
    let taps = [("low", "nfm", "iq"), ("high", "nfm", "iq4")];
    let workspace = put_active_workspace(&app, &virtual_snapshot("array4", &taps)).await;
    apply(&app, workspace).await;

    let set = &get_state(&app).await.device_sets[0];
    let offset_for = |stream: u32| if stream == 0 { 11_000.0 } else { 33_000.0 };
    for channel in &set.channels {
        let (status, body) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{}/channels/{}", set.id, channel.id),
            Some(&format!(
                r#"{{"offset_hz":{},"params":{{"type":"nfm","settings":{{}}}}}}"#,
                offset_for(channel.stream)
            )),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );
    }
    workspace::save_active(&state).expect("capture the workspace");

    let restarted = state_over(state.store.clone());
    let (app, background) = router_with_state(restarted, &ServerOptions::default());
    background.detach();
    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let set = &get_state(&app).await.device_sets[0];
    let streams: Vec<u32> = set.channels.iter().map(|channel| channel.stream).collect();
    assert_eq!(streams, vec![0, 3]);
    for channel in &set.channels {
        assert_eq!(
            channel.settings.offset_hz,
            offset_for(channel.stream),
            "stream {} came back with the other lane's settings",
            channel.stream
        );
    }
}

async fn create_named_workspace(
    app: &Router,
    name: &str,
    snapshot: &sdrmm_wire::WorkspaceSnapshot,
) -> i64 {
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/workspaces",
        Some(&format!(
            r#"{{"name":{},"snapshot":{}}}"#,
            serde_json::to_string(name).unwrap(),
            serde_json::to_string(snapshot).unwrap()
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice::<sdrmm_wire::CreatedRowId>(&body)
        .expect("json")
        .id
}

async fn export_document(
    app: &Router,
    id: i64,
) -> (axum::http::HeaderMap, sdrmm_wire::WorkspaceExport) {
    let (status, headers, body) = request_parts(
        app.clone(),
        "GET",
        &format!("/api/workspaces/{id}/export"),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    (headers, serde_json::from_slice(&body).expect("json"))
}

async fn import_document(app: &Router, document: &str) -> (StatusCode, Bytes) {
    request(
        app.clone(),
        "POST",
        "/api/workspaces/import",
        Some(document),
    )
    .await
}

fn tuned_state(center_hz: f64) -> sdrmm_wire::WorkspaceState {
    sdrmm_wire::WorkspaceState {
        version: sdrmm_wire::WORKSPACE_STATE_VERSION,
        trunks: Vec::new(),
        devices: vec![sdrmm_wire::WorkspaceDevice {
            node: "device".to_string(),
            settings: DeviceSettings {
                center_hz: Some(center_hz),
                ..DeviceSettings::default()
            },
            channels: Vec::new(),
        }],
    }
}

#[tokio::test]
async fn an_exported_workspace_carries_its_layout_its_tuning_and_a_download_name() {
    let (app, state) = test_router_with_state();
    let snapshot = virtual_snapshot("siggen", &[("nfm", "nfm", "iq")]);
    let workspace = create_named_workspace(&app, "Airband Watch", &snapshot).await;
    state
        .store
        .put_workspace_state(workspace, &tuned_state(145_500_000.0))
        .expect("plant the stored settings");

    let (headers, export) = export_document(&app, workspace).await;

    assert_eq!(export.version, sdrmm_wire::WORKSPACE_EXPORT_VERSION);
    assert_eq!(export.name, "Airband Watch");
    assert_eq!(export.snapshot, snapshot);
    assert_eq!(
        export.state.device("device").map(|d| d.settings.center_hz),
        Some(Some(145_500_000.0)),
        "an export without the tuning would import as an untuned workspace"
    );
    let disposition = headers
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert_eq!(
        disposition, "attachment; filename=\"workspace-airband-watch.json\"",
        "unusable download name: {disposition}"
    );
}

#[tokio::test]
async fn a_workspace_named_in_another_script_still_downloads_under_a_usable_name() {
    let app = test_router();
    let workspace =
        create_named_workspace(&app, "航空無線", &sdrmm_wire::WorkspaceSnapshot::starter()).await;

    let (headers, _) = export_document(&app, workspace).await;

    assert_eq!(
        headers
            .get("content-disposition")
            .and_then(|value| value.to_str().ok()),
        Some(format!("attachment; filename=\"workspace-{workspace}.json\"").as_str())
    );
}

#[tokio::test]
async fn exporting_a_workspace_that_is_not_there_says_so() {
    let app = test_router();
    let (status, _) = request(app, "GET", "/api/workspaces/9999/export", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_imported_workspace_lands_next_to_the_one_it_came_from_and_opens_its_radio() {
    let (app, state) = test_router_with_state();
    let snapshot = virtual_snapshot("siggen", &[("nfm", "nfm", "iq")]);
    let workspace = create_named_workspace(&app, "Airband Watch", &snapshot).await;
    state
        .store
        .put_workspace_state(workspace, &tuned_state(145_500_000.0))
        .expect("plant the stored settings");
    let (_, export) = export_document(&app, workspace).await;
    let active_before = workspaces(&app).await.active;

    let (status, body) = import_document(&app, &serde_json::to_string(&export).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let imported = serde_json::from_slice::<sdrmm_wire::CreatedRowId>(&body)
        .expect("json")
        .id;

    assert_ne!(imported, workspace, "an import never overwrites its source");
    let listed = workspaces(&app).await;
    assert_eq!(
        listed.active, active_before,
        "an import does not switch away"
    );
    let names: Vec<&str> = listed
        .workspaces
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert!(names.contains(&"Airband Watch"));
    assert!(
        names.contains(&"Airband Watch (2)"),
        "a taken name should gain a copy number: {names:?}"
    );

    let detail = workspace_detail(&app, imported).await;
    assert_eq!(detail.snapshot, snapshot);
    assert_eq!(
        state
            .store
            .workspace_state(imported)
            .expect("stored settings")
            .device("device")
            .map(|d| d.settings.center_hz),
        Some(Some(145_500_000.0))
    );

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/workspaces/{imported}/activate"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let report = apply(&app, imported).await;
    assert_eq!(report.opened, 1);
    assert_eq!(report.created, 1);
    assert_eq!(
        get_state(&app).await.device_sets[0].settings.center_hz,
        Some(145_500_000.0),
        "the imported workspace came up on the frequency it was exported from"
    );
}

#[tokio::test]
async fn importing_refuses_a_document_this_build_cannot_read() {
    let app = test_router();
    let export = sdrmm_wire::WorkspaceExport::new(
        "Airband Watch".to_string(),
        sdrmm_wire::WorkspaceSnapshot::starter(),
        sdrmm_wire::WorkspaceState::new(),
    );

    let mut newer = export.clone();
    newer.version = sdrmm_wire::WORKSPACE_EXPORT_VERSION + 1;
    let (status, _) = import_document(&app, &serde_json::to_string(&newer).unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut newer_tuning = export.clone();
    newer_tuning.state.version = sdrmm_wire::WORKSPACE_STATE_VERSION + 1;
    let (status, body) =
        import_document(&app, &serde_json::to_string(&newer_tuning).unwrap()).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "tuning this build cannot read must be refused, not dropped in silence"
    );
    assert!(String::from_utf8_lossy(&body).contains("state version"));

    let mut broken = export.clone();
    broken.snapshot.graph.edges.push(sdrmm_wire::PatchEdge {
        from: sdrmm_wire::PortRef {
            node: "device".to_string(),
            port: "iq".to_string(),
        },
        to: sdrmm_wire::PortRef {
            node: "ghost".to_string(),
            port: "iq".to_string(),
        },
    });
    let (status, _) = import_document(&app, &serde_json::to_string(&broken).unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = import_document(&app, r#"{"name":"nope"}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    assert_eq!(
        workspaces(&app).await.workspaces.len(),
        1,
        "a refused import left a row behind"
    );
}

#[tokio::test]
async fn an_import_forgets_tuning_for_nodes_the_document_never_draws() {
    let (app, state) = test_router_with_state();
    let mut export = sdrmm_wire::WorkspaceExport::new(
        "Trimmed".to_string(),
        sdrmm_wire::WorkspaceSnapshot::starter(),
        tuned_state(145_500_000.0),
    );
    export.state.merge(vec![sdrmm_wire::WorkspaceDevice {
        node: "gone".to_string(),
        settings: DeviceSettings::default(),
        channels: Vec::new(),
    }]);

    let (status, body) = import_document(&app, &serde_json::to_string(&export).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let imported = serde_json::from_slice::<sdrmm_wire::CreatedRowId>(&body)
        .expect("json")
        .id;

    let stored = state
        .store
        .workspace_state(imported)
        .expect("stored settings");
    let nodes: Vec<&str> = stored
        .devices
        .iter()
        .map(|device| device.node.as_str())
        .collect();
    assert_eq!(nodes, vec!["device"]);
}
