use super::*;

#[tokio::test]
async fn scanner_start_stop_and_error_mapping_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/scanner"),
        Some(r#"{"action":"start"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/scanner"),
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let start = r#"{"action":"start","settings":{"ranges":[{"start_hz":99000000.0,"stop_hz":101000000.0,"step_hz":100000.0}],"threshold_db":100.0,"dwell_ms":40}}"#;
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/scanner"),
        Some(start),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let status_body: sdrmm_wire::ScannerStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(status_body.targets, 21);
    assert!(get_state(&app).await.device_sets[0].scanner.is_some());

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":88000000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/scanner"),
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(get_state(&app).await.device_sets[0].scanner.is_none());

    let (status, _) = request(
        app,
        "POST",
        "/api/devicesets/999/scanner",
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_scan_session_spans_two_sets_over_http() {
    let app = test_router();
    let a = create_virtual_set(&app).await;

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/scanner",
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let start = format!(
        r#"{{"action":"start","device_sets":[{a}],"settings":{{"ranges":[{{"start_hz":99000000.0,"stop_hz":101000000.0,"step_hz":100000.0}}],"threshold_db":100.0,"dwell_ms":40,"hardware_sweep":false}}}}"#
    );
    let (status, body) = request(app.clone(), "POST", "/api/scanner", Some(&start)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let session: sdrmm_wire::ScanSessionStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(session.members.len(), 1);
    assert_eq!(session.members[0].device_set, a);
    let state = get_state(&app).await;
    assert_eq!(
        state.scan_session.expect("listed").device_sets,
        vec![a],
        "the ganged scan must be listed on the state"
    );

    let (status, body) = request(app.clone(), "POST", "/api/scanner", Some(&start)).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a second scan must be refused"
    );
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = request(
        app.clone(),
        "POST",
        "/api/scanner",
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(get_state(&app).await.scan_session.is_none());
}

#[tokio::test]
async fn hunt_start_stop_and_error_mapping_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/hunt"),
        Some(r#"{"action":"start"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/hunt"),
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/hunt"),
        Some(r#"{"action":"start","settings":{"freq_hz":100000000.0,"bw_hz":25000.0}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let hunt: sdrmm_wire::HuntStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(hunt.settings.freq_hz, 100_000_000.0);
    assert!(get_state(&app).await.device_sets[0].hunt.is_some());

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":88000000.0}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a hunt owns the dial");

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/hunt"),
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(get_state(&app).await.device_sets[0].hunt.is_none());

    let (status, _) = request(
        app,
        "POST",
        "/api/devicesets/999/hunt",
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
