use super::*;

#[tokio::test]
async fn preset_capture_apply_delete_roundtrip() {
    let app = test_router();
    let workspace = store_siggen_workspace(&app).await;
    apply(&app, workspace).await;
    let ds = get_state(&app).await.device_sets[0].id;

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":145500000.0,"sample_rate":2400000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let channel = get_state(&app).await.device_sets[0].channels[0].id;
    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{channel}"),
        Some(r#"{"offset_hz":25000.0,"squelch_db":-70.0,"params":{"type":"nfm","settings":{}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/presets",
        Some(r#"{"name":"2m"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let preset = serde_json::from_slice::<CreatedRowId>(&body)
        .expect("json")
        .id;

    let (status, body) = request(app.clone(), "GET", "/api/presets", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, preset);
    assert_eq!(listed[0].name, "2m");
    assert_eq!(listed[0].devices, 1);

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":100000000.0,"sample_rate":2048000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/presets/{preset}/apply"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let set = &get_state(&app).await.device_sets[0];
    assert_eq!(set.settings.center_hz, Some(145_500_000.0));
    assert_eq!(set.settings.sample_rate, Some(2_400_000.0));
    assert_eq!(set.channels.len(), 1);
    assert_eq!(set.channels[0].settings.offset_hz, 25_000.0);
    assert_eq!(set.channels[0].settings.squelch_db, Some(-70.0));

    let (status, _) = request(app.clone(), "POST", "/api/presets/999/apply", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = request(
        app.clone(),
        "DELETE",
        &format!("/api/presets/{preset}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(
        app.clone(),
        "DELETE",
        &format!("/api/presets/{preset}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = request(app, "GET", "/api/presets", None).await;
    let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
    assert!(listed.is_empty());
}

#[tokio::test]
async fn a_preset_carries_every_radio_the_workspace_draws() {
    let app = test_router();
    let mut snapshot = virtual_snapshot("siggen", &[]);
    snapshot.graph.nodes.push(sdrmm_wire::PatchNode {
        id: "second".to_string(),
        body: sdrmm_wire::NodeBody::Device(sdrmm_wire::DeviceNode {
            device: Some(sdrmm_wire::DeviceRef {
                backend: "virtual".to_string(),
                serial: None,
                key: Some("array4".to_string()),
            }),
            tuning_locked: false,
        }),
        position: sdrmm_wire::Position { x: 0.0, y: 600.0 },
        size: None,
        label: None,
    });
    let workspace = put_active_workspace(&app, &snapshot).await;
    assert_eq!(apply(&app, workspace).await.opened, 2);

    let tune = async |app: Router, ds: u32, hz: f64| {
        let (status, _) = request(
            app,
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(&format!(r#"{{"center_hz":{hz}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    };
    let sets: Vec<u32> = get_state(&app)
        .await
        .device_sets
        .iter()
        .map(|s| s.id)
        .collect();
    tune(app.clone(), sets[0], 145_500_000.0).await;
    tune(app.clone(), sets[1], 433_000_000.0).await;

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/presets",
        Some(r#"{"name":"the bench"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let preset = serde_json::from_slice::<CreatedRowId>(&body)
        .expect("json")
        .id;
    let (_, body) = request(app.clone(), "GET", "/api/presets", None).await;
    let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
    assert_eq!(listed[0].devices, 2);

    tune(app.clone(), sets[0], 100_000_000.0).await;
    tune(app.clone(), sets[1], 100_000_000.0).await;
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/presets/{preset}/apply"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let state = get_state(&app).await;
    let center = |ds: u32| {
        state
            .device_sets
            .iter()
            .find(|set| set.id == ds)
            .and_then(|set| set.settings.center_hz)
    };
    assert_eq!(center(sets[0]), Some(145_500_000.0));
    assert_eq!(center(sets[1]), Some(433_000_000.0));
}

#[tokio::test]
async fn saving_a_preset_with_no_radio_open_is_refused() {
    let app = test_router();
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/presets",
        Some(r#"{"name":"empty bench"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
    assert!(err.error.contains("nothing to save"), "{err:?}");

    let (_, body) = request(app, "GET", "/api/presets", None).await;
    let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
    assert!(listed.is_empty());
}

#[tokio::test]
async fn apply_preset_replaces_channels_that_do_not_fit_the_preset_rate() {
    let (app, store) = test_router_with_store();
    let workspace = put_active_workspace(&app, &virtual_snapshot("siggen", &[])).await;
    apply(&app, workspace).await;
    let ds = get_state(&app).await.device_sets[0].id;
    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"offset_hz":900000.0,"params":{"type":"nfm","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let preset = store
        .create_preset("lowrate", &preset_250k(vec![nfm_at(0.0)]))
        .expect("preset");
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/presets/{preset}/apply"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "apply failed: {}",
        String::from_utf8_lossy(&body)
    );

    let snap = get_state(&app).await;
    let set = snap
        .device_sets
        .iter()
        .find(|s| s.id == ds)
        .expect("device set");
    assert_eq!(set.settings.sample_rate, Some(250_000.0));
    assert_eq!(set.channels.len(), 1);
    assert_eq!(set.channels[0].settings.offset_hz, 0.0);
}

#[tokio::test]
async fn apply_preset_rejected_up_front_leaves_the_set_untouched() {
    let (app, store) = test_router_with_store();
    let workspace = put_active_workspace(&app, &virtual_snapshot("siggen", &[])).await;
    apply(&app, workspace).await;
    let ds = get_state(&app).await.device_sets[0].id;
    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"offset_hz":100000.0,"params":{"type":"nfm","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let preset = store
        .create_preset("broken", &preset_250k(vec![nfm_at(900_000.0)]))
        .expect("preset");
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/presets/{preset}/apply"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
    assert!(
        err.error.contains("exceeds"),
        "the rejection must name the problem: {err:?}"
    );
    assert_eq!(
        err.detail.as_deref(),
        Some("0 of 1 radios in the preset were configured"),
        "nothing was applied to this radio, and the report says which radios were: {err:?}"
    );

    let snap = get_state(&app).await;
    let set = snap
        .device_sets
        .iter()
        .find(|s| s.id == ds)
        .expect("device set");
    assert_eq!(set.settings.sample_rate, Some(2_048_000.0));
    assert_eq!(set.channels.len(), 1);
    assert_eq!(set.channels[0].settings.offset_hz, 100_000.0);
}

#[tokio::test]
async fn bookmark_crud_over_http() {
    let app = test_router();
    let (status, body) = request(app.clone(), "GET", "/api/bookmarks", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<Bookmark> = serde_json::from_slice(&body).expect("json");
    assert!(listed.is_empty());

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/bookmarks",
        Some(r#"{"label":"tower","freq_hz":118700000.0,"mode":"am","group":"airband"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id = serde_json::from_slice::<CreatedRowId>(&body)
        .expect("json")
        .id;

    let (_, body) = request(app.clone(), "GET", "/api/bookmarks", None).await;
    let listed: Vec<Bookmark> = serde_json::from_slice(&body).expect("json");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].label, "tower");
    assert_eq!(listed[0].freq_hz, 118_700_000.0);
    assert_eq!(listed[0].mode.as_deref(), Some("am"));
    assert_eq!(listed[0].group.as_deref(), Some("airband"));

    let (status, _) = request(app.clone(), "DELETE", &format!("/api/bookmarks/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(app, "DELETE", &format!("/api/bookmarks/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
