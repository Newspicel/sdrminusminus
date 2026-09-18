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
    let was_frequency = set.channels[0].settings.frequency_hz;

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
        Some(&format!(
            r#"{{"frequency_hz":{},"params":{{"type":"nfm","settings":{{}}}}}}"#,
            was_center + 1_025_000.0
        )),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let moved = get_state(&app).await;
    assert_eq!(
        moved.device_sets[0].channels[0].settings.frequency_hz,
        was_center + 1_025_000.0
    );
    assert!(
        workspace_detail(&app, workspace).await.history.can_undo,
        "a dial move is a step the history knows about"
    );

    let undone = step(&app, workspace, "undo").await;
    assert!(undone.history.can_redo);
    let back = get_state(&app).await;
    assert_eq!(
        back.device_sets[0].channels[0].settings.frequency_hz, was_frequency,
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
        Some(r#"{"frequency_hz":145512500.0,"squelch":{"mode":"manual","level_db":-42.0},"params":{"type":"nfm","settings":{}}}"#),
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
    assert_eq!(set.channels[0].settings.frequency_hz, 145_512_500.0);
    assert_eq!(
        set.channels[0].settings.squelch,
        sdrmm_wire::Squelch::Manual { level_db: -42.0 }
    );
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
        channels: Vec::new(),
        devices: vec![sdrmm_wire::WorkspaceDevice {
            node: "device".to_string(),
            settings: DeviceSettings {
                center_hz: Some(145_500_000.0),
                sample_rate: Some(999.0),
                bias_tee: Some(true),
                ..DeviceSettings::default()
            },
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
    assert_eq!(kept.settings.bias_tee, Some(true));
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
    let frequency_for = |stream: u32| {
        if stream == 0 {
            100_011_000.0
        } else {
            100_033_000.0
        }
    };
    for channel in &set.channels {
        let (status, body) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{}/channels/{}", set.id, channel.id),
            Some(&format!(
                r#"{{"frequency_hz":{},"params":{{"type":"nfm","settings":{{}}}}}}"#,
                frequency_for(channel.stream)
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
            channel.settings.frequency_hz,
            frequency_for(channel.stream),
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
        channels: Vec::new(),
        devices: vec![sdrmm_wire::WorkspaceDevice {
            node: "device".to_string(),
            settings: DeviceSettings {
                center_hz: Some(center_hz),
                ..DeviceSettings::default()
            },
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

#[tokio::test]
async fn a_radio_nobody_tuned_opens_over_the_decoder_wired_into_it() {
    let app = test_router();
    let snapshot = virtual_snapshot("siggen", &[("planes", "adsb", "iq")]);
    let workspace = put_active_workspace(&app, &snapshot).await;

    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let set = &get_state(&app).await.device_sets[0];
    let center_hz = set.settings.center_hz.expect("a tuned radio");
    assert!(
        (center_hz - 1_090_000_000.0).abs() < 2_000_000.0,
        "the radio came up at {center_hz} Hz, where its decoder cannot be heard"
    );
    assert_ne!(
        center_hz, 1_090_000_000.0,
        "the decoder was left on the DC spike"
    );
    assert!(!set.channels[0].out_of_band);
}

#[tokio::test]
async fn a_decoder_keeps_its_own_frequency_and_the_radio_comes_to_it() {
    let app = test_router();
    let snapshot = virtual_snapshot("siggen", &[("voice", "nfm", "iq")]);
    let workspace = put_active_workspace(&app, &snapshot).await;
    apply(&app, workspace).await;

    let set = &get_state(&app).await.device_sets[0];
    let default_hz = sdrmm_wire::ChannelSettings::default_for("nfm")
        .expect("nfm")
        .frequency_hz;
    assert_eq!(
        set.channels[0].settings.frequency_hz, default_hz,
        "the radio moved the decoder instead of following it"
    );
    assert!(!set.channels[0].out_of_band);
}

#[tokio::test]
async fn a_channel_node_holds_its_settings_before_any_radio_carries_it() {
    let app = test_router();
    let snapshot = virtual_snapshot("siggen", &[("voice", "nfm", "iq")]);
    let workspace = put_active_workspace(&app, &snapshot).await;

    let mut settings = sdrmm_wire::ChannelSettings::default_for("nfm").expect("nfm is built in");
    settings.frequency_hz = 100_012_500.0;
    settings.squelch = sdrmm_wire::Squelch::Manual { level_db: -70.0 };
    let (status, _) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{workspace}/channels/voice"),
        Some(&serde_json::to_string(&settings).expect("settings serialize")),
    )
    .await;
    assert_eq!(status, 204);

    let held = workspace_detail(&app, workspace)
        .await
        .state
        .channel("voice")
        .expect("the node holds what it was set to")
        .settings
        .clone();
    assert_eq!(held.frequency_hz, 100_012_500.0);
    assert_eq!(
        held.squelch,
        sdrmm_wire::Squelch::Manual { level_db: -70.0 }
    );

    apply(&app, workspace).await;
    let live = get_state(&app).await;
    let channel = live.device_sets[0]
        .channels
        .iter()
        .find(|channel| channel.settings.params.type_id() == "nfm")
        .expect("the channel opened with the radio");
    assert_eq!(
        channel.settings.frequency_hz, 100_012_500.0,
        "settings held while there was no radio are what the channel starts on"
    );
    assert_eq!(
        channel.settings.squelch,
        sdrmm_wire::Squelch::Manual { level_db: -70.0 }
    );
}

#[tokio::test]
async fn settings_of_another_channel_type_are_refused() {
    let app = test_router();
    let snapshot = virtual_snapshot("siggen", &[("voice", "nfm", "iq")]);
    let workspace = put_active_workspace(&app, &snapshot).await;

    let settings = sdrmm_wire::ChannelSettings::default_for("am").expect("am is built in");
    let (status, _) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{workspace}/channels/voice"),
        Some(&serde_json::to_string(&settings).expect("settings serialize")),
    )
    .await;
    assert_eq!(status, 400);

    let (status, _) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{workspace}/channels/device"),
        Some(&serde_json::to_string(&settings).expect("settings serialize")),
    )
    .await;
    assert_eq!(status, 400, "a device node is not a channel");
}

#[tokio::test]
async fn every_channel_type_offers_the_settings_a_node_starts_on() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/channeltypes", None).await;
    assert_eq!(status, 200);
    let types: sdrmm_wire::ChannelTypesResponse =
        serde_json::from_slice(&body).expect("channel types");
    assert!(!types.types.is_empty());
    for descriptor in &types.types {
        let defaults = descriptor
            .defaults
            .as_ref()
            .unwrap_or_else(|| panic!("{} offers no defaults", descriptor.type_id));
        assert_eq!(defaults.params.type_id(), descriptor.type_id);
    }
}

#[tokio::test]
async fn an_open_radio_holds_its_frequency_when_a_decoder_is_wired_in() {
    let app = test_router();
    let workspace =
        put_active_workspace(&app, &virtual_snapshot("siggen", &[("voice", "nfm", "iq")])).await;
    apply(&app, workspace).await;
    let opened = get_state(&app).await.device_sets[0].settings.center_hz;

    put_workspace_revision(
        &app,
        &virtual_snapshot("siggen", &[("voice", "nfm", "iq"), ("air", "adsb", "iq")]),
        2,
    )
    .await;
    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{report:?}");
    assert_eq!(
        get_state(&app).await.device_sets[0].settings.center_hz,
        opened,
        "a decoder wired into a running radio may not drag it off frequency"
    );
}

#[tokio::test]
async fn cutting_a_decoders_wire_closes_it_and_hands_the_window_to_the_rest() {
    let app = test_router();
    let mut snapshot = virtual_snapshot(
        "halfduplex",
        &[("planes", "adsb", "iq"), ("voice", "aprs", "iq")],
    );
    let workspace = put_active_workspace(&app, &snapshot).await;
    let opened = apply(&app, workspace).await;
    assert!(opened.refused.is_empty(), "{:?}", opened.refused);

    let set = &get_state(&app).await.device_sets[0];
    assert_eq!(set.channels.len(), 2);
    assert_eq!(
        set.channels.iter().filter(|c| c.out_of_band).count(),
        1,
        "one window cannot hold both of these decoders at once"
    );

    snapshot.graph.edges.retain(|edge| edge.to.node != "planes");
    snapshot.graph.nodes.retain(|node| node.id != "planes");
    put_workspace_revision(&app, &snapshot, 2).await;
    let report = apply(&app, workspace).await;
    assert_eq!(report.closed, 1, "the unwired decoder went on running");

    let set = &get_state(&app).await.device_sets[0];
    assert_eq!(set.channels.len(), 1);
    assert!(
        !set.channels[0].out_of_band,
        "the radio stayed on the decoder that was cut away, at {:?}",
        set.settings.center_hz
    );
}

async fn retune(app: &Router, set: u32, channel: u32, frequency_hz: f64) {
    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{set}/channels/{channel}"),
        Some(&format!(
            r#"{{"frequency_hz":{frequency_hz},"params":{{"type":"nfm","settings":{{}}}}}}"#
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

#[tokio::test]
async fn cutting_one_of_two_alike_decoders_leaves_the_other_where_it_was_set() {
    let app = test_router();
    let mut snapshot =
        virtual_snapshot("siggen", &[("first", "nfm", "iq"), ("second", "nfm", "iq")]);
    let workspace = put_active_workspace(&app, &snapshot).await;
    let opened = apply(&app, workspace).await;
    assert!(opened.refused.is_empty(), "{:?}", opened.refused);

    let set = get_state(&app).await.device_sets[0].clone();
    let by_node = |node: &str| {
        set.channels
            .iter()
            .find(|channel| channel.node.as_deref() == Some(node))
            .unwrap_or_else(|| panic!("no decoder was opened for {node}"))
            .id
    };
    let center_hz = set.settings.center_hz.expect("an open radio is tuned");
    retune(&app, set.id, by_node("first"), center_hz + 100_000.0).await;
    retune(&app, set.id, by_node("second"), center_hz + 200_000.0).await;
    let settled_hz = get_state(&app).await.device_sets[0].settings.center_hz;

    snapshot.graph.edges.retain(|edge| edge.to.node != "first");
    snapshot.graph.nodes.retain(|node| node.id != "first");
    put_workspace_revision(&app, &snapshot, 2).await;
    let report = apply(&app, workspace).await;
    assert_eq!(report.closed, 1, "{report:?}");

    let set = &get_state(&app).await.device_sets[0];
    assert_eq!(set.channels.len(), 1);
    assert_eq!(set.channels[0].node.as_deref(), Some("second"));
    assert_eq!(
        set.channels[0].settings.frequency_hz,
        center_hz + 200_000.0,
        "the surviving decoder was handed the cut one's frequency"
    );
    assert_eq!(
        set.settings.center_hz, settled_hz,
        "the radio moved for nothing"
    );
}

fn two_radio_snapshot(taps: &[(&str, &str)]) -> sdrmm_wire::WorkspaceSnapshot {
    let mut snapshot = virtual_snapshot("siggen", &[]);
    snapshot.graph.nodes.push(sdrmm_wire::PatchNode {
        id: "device2".to_string(),
        body: sdrmm_wire::NodeBody::Device(sdrmm_wire::DeviceNode {
            device: Some(sdrmm_wire::DeviceRef {
                backend: "virtual".to_string(),
                serial: None,
                key: Some("halfduplex".to_string()),
            }),
            locked_streams: Vec::new(),
        }),
        position: sdrmm_wire::Position { x: 0.0, y: 300.0 },
        size: None,
        label: None,
    });
    for (id, channel_type) in taps {
        snapshot.graph.nodes.push(sdrmm_wire::PatchNode {
            id: (*id).to_string(),
            body: sdrmm_wire::NodeBody::Channel(sdrmm_wire::ChannelNode {
                channel_type: (*channel_type).to_string(),
                record_calls: false,
                tuning_locked: false,
            }),
            position: sdrmm_wire::Position { x: 400.0, y: 300.0 },
            size: None,
            label: None,
        });
        for device in ["device", "device2"] {
            snapshot.graph.edges.push(sdrmm_wire::PatchEdge {
                from: sdrmm_wire::PortRef {
                    node: device.to_string(),
                    port: "iq".to_string(),
                },
                to: sdrmm_wire::PortRef {
                    node: (*id).to_string(),
                    port: "iq".to_string(),
                },
            });
        }
    }
    snapshot
}

fn carrier_of(state: &StateSnapshot, node: &str) -> Option<(u32, sdrmm_wire::ChannelInfo)> {
    state.device_sets.iter().find_map(|set| {
        set.channels
            .iter()
            .find(|channel| channel.node.as_deref() == Some(node))
            .map(|channel| (set.id, channel.clone()))
    })
}

async fn hold_by_hand(app: &Router, set: u32, center_hz: f64) {
    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{set}/device"),
        Some(&format!(r#"{{"center_hz":{center_hz},"tuning":"manual"}}"#)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

async fn hold_channel(app: &Router, workspace: i64, node: &str, frequency_hz: f64) {
    let mut settings = sdrmm_wire::ChannelSettings::default_for("nfm").expect("nfm is built in");
    settings.frequency_hz = frequency_hz;
    let (status, body) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{workspace}/channels/{node}"),
        Some(&serde_json::to_string(&settings).expect("settings serialize")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn a_decoder_wired_to_two_radios_opens_on_one_of_them() {
    let app = test_router();
    let workspace = put_active_workspace(&app, &two_radio_snapshot(&[("voice", "nfm")])).await;

    let report = apply(&app, workspace).await;
    assert_eq!(report.opened, 2);
    assert_eq!(report.created, 1, "{report:?}");
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let state = get_state(&app).await;
    let opened: usize = state.device_sets.iter().map(|set| set.channels.len()).sum();
    assert_eq!(opened, 1);
    let (_, channel) = carrier_of(&state, "voice").expect("the decoder is on a radio");
    assert!(!channel.out_of_band);

    let again = apply(&app, workspace).await;
    assert_eq!(again.created, 0, "{again:?}");
    assert_eq!(again.closed, 0, "{again:?}");
}

#[tokio::test]
async fn a_decoder_moves_to_the_radio_that_can_still_hear_it() {
    let app = test_router();
    let workspace = put_active_workspace(&app, &two_radio_snapshot(&[("voice", "nfm")])).await;
    apply(&app, workspace).await;

    let state = get_state(&app).await;
    let (first, channel) = carrier_of(&state, "voice").expect("the decoder is on a radio");
    hold_by_hand(&app, first, channel.settings.frequency_hz + 300e6).await;

    let state = get_state(&app).await;
    let (second, moved) = carrier_of(&state, "voice").expect("the decoder is still on a radio");
    assert_ne!(
        second, first,
        "the decoder stayed on a radio that cannot hear it"
    );
    assert!(!moved.out_of_band);
    assert_eq!(moved.settings.frequency_hz, channel.settings.frequency_hz);
    let opened: usize = state.device_sets.iter().map(|set| set.channels.len()).sum();
    assert_eq!(opened, 1, "the decoder was copied, not moved");
}

#[tokio::test]
async fn a_retuned_decoder_finds_the_radio_that_hears_its_new_frequency() {
    let app = test_router();
    let workspace = put_active_workspace(&app, &two_radio_snapshot(&[("voice", "nfm")])).await;
    apply(&app, workspace).await;

    let state = get_state(&app).await;
    let (first, channel) = carrier_of(&state, "voice").expect("the decoder is on a radio");
    let other = state
        .device_sets
        .iter()
        .find(|set| set.id != first)
        .expect("a second radio")
        .id;
    hold_by_hand(&app, first, channel.settings.frequency_hz).await;
    hold_by_hand(&app, other, 400e6).await;

    retune(&app, first, channel.id, 400.1e6).await;

    let state = get_state(&app).await;
    let (now, moved) = carrier_of(&state, "voice").expect("the decoder is still on a radio");
    assert_eq!(now, other);
    assert!(!moved.out_of_band);
    assert_eq!(moved.settings.frequency_hz, 400.1e6);
}

#[tokio::test]
async fn two_self_tuning_radios_share_a_crowd_one_window_cannot_hold() {
    let app = test_router();
    let taps = [("low", "nfm"), ("high", "nfm"), ("low2", "nfm")];
    let workspace = put_active_workspace(&app, &two_radio_snapshot(&taps)).await;
    hold_channel(&app, workspace, "low", 100e6).await;
    hold_channel(&app, workspace, "low2", 100.5e6).await;
    hold_channel(&app, workspace, "high", 400e6).await;

    let report = apply(&app, workspace).await;
    assert_eq!(report.created, 3, "{report:?}");
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let state = get_state(&app).await;
    for node in ["low", "high", "low2"] {
        let (_, channel) = carrier_of(&state, node).expect("every decoder is on a radio");
        assert!(!channel.out_of_band, "{node} is out of band");
    }
    let (low, _) = carrier_of(&state, "low").expect("low");
    let (low2, _) = carrier_of(&state, "low2").expect("low2");
    let (high, _) = carrier_of(&state, "high").expect("high");
    assert_eq!(low, low2);
    assert_ne!(low, high);
}

#[tokio::test]
async fn a_decoder_wired_to_one_radio_keeps_it_while_the_flexible_ones_move_aside() {
    let app = test_router();
    let mut snapshot = two_radio_snapshot(&[("flex", "nfm"), ("fixed", "nfm")]);
    snapshot
        .graph
        .edges
        .retain(|edge| !(edge.to.node == "fixed" && edge.from.node == "device2"));
    let workspace = put_active_workspace(&app, &snapshot).await;
    hold_channel(&app, workspace, "fixed", 400e6).await;
    hold_channel(&app, workspace, "flex", 100e6).await;

    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{:?}", report.refused);

    let state = get_state(&app).await;
    let (fixed_on, fixed) = carrier_of(&state, "fixed").expect("fixed");
    let (flex_on, flex) = carrier_of(&state, "flex").expect("flex");
    let siggen = state
        .device_sets
        .iter()
        .find(|set| set.device.key == "siggen")
        .expect("the siggen radio")
        .id;
    assert_eq!(fixed_on, siggen);
    assert_ne!(flex_on, siggen);
    assert!(!fixed.out_of_band && !flex.out_of_band);
}

#[tokio::test]
async fn applying_adopts_an_unnamed_channel_without_copying_it() {
    let (app, state) = test_router_with_state();
    let workspace = put_active_workspace(&app, &two_radio_snapshot(&[("voice", "nfm")])).await;
    let set = state.engine.create_device_set("virtual:siggen").unwrap();
    let settings = ChannelSettings::default_for("nfm").unwrap();
    let id = state.engine.add_channel(set, 0, settings).unwrap();
    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{report:?}");
    assert_eq!(report.created, 0);
    let snapshot = state.engine.snapshot();
    let (owner, channel) = carrier_of(&snapshot, "voice").unwrap();
    assert_eq!((owner, channel.id), (set, id));
    assert_eq!(
        snapshot
            .device_sets
            .iter()
            .map(|set| set.channels.len())
            .sum::<usize>(),
        1
    );
}

#[tokio::test]
async fn a_second_stream_on_the_same_radio_can_carry_a_decoder() {
    let (app, state) = test_router_with_state();
    let mut snapshot = virtual_snapshot("transceiver", &[("voice", "nfm", "iq")]);
    snapshot.graph.edges.push(sdrmm_wire::PatchEdge {
        from: sdrmm_wire::PortRef {
            node: "device".to_owned(),
            port: "iq2".to_owned(),
        },
        to: sdrmm_wire::PortRef {
            node: "voice".to_owned(),
            port: "iq".to_owned(),
        },
    });
    let workspace = put_active_workspace(&app, &snapshot).await;
    hold_channel(&app, workspace, "voice", 400e6).await;
    let set = state
        .engine
        .create_device_set("virtual:transceiver")
        .unwrap();
    state
        .engine
        .patch_device(
            set,
            DeviceSettings {
                center_hz: Some(100e6),
                tuning: Some(sdrmm_wire::Tuning::Manual),
                streams: vec![sdrmm_wire::StreamSettings {
                    stream: 1,
                    center_hz: Some(400e6),
                    tuning: Some(sdrmm_wire::Tuning::Manual),
                    gains: Vec::new(),
                    antenna: None,
                }],
                ..Default::default()
            },
        )
        .unwrap();
    let report = apply(&app, workspace).await;
    assert!(report.refused.is_empty(), "{report:?}");
    let snapshot = state.engine.snapshot();
    assert_eq!(
        snapshot
            .device_sets
            .iter()
            .map(|set| set.channels.len())
            .sum::<usize>(),
        1
    );
    assert_eq!(carrier_of(&snapshot, "voice").unwrap().1.stream, 1);
    assert_eq!(apply(&app, workspace).await.created, 0);
}
