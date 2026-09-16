use super::*;

async fn nfm_decoder(app: &Router, ds: u32) -> u32 {
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"frequency_hz":100100000.0,"params":{"type":"nfm","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice::<CreatedId>(&body).expect("json").id
}

#[tokio::test]
async fn scanner_start_stop_and_error_mapping_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;
    let ch = nfm_decoder(&app, ds).await;

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

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/scanner"),
        Some(r#"{"action":"skip"}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "nothing to skip before a scan"
    );

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/scanner"),
        Some(r#"{"action":"start","settings":{"channel":99,"frequencies":[100000000.0]}}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let start = format!(
        r#"{{"action":"start","settings":{{"channel":{ch},"ranges":[{{"start_hz":99000000.0,"stop_hz":101000000.0,"step_hz":100000.0}}],"threshold_db":100.0,"dwell_ms":40}}}}"#
    );
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/scanner"),
        Some(&start),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let status_body: sdrmm_wire::ScannerStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(status_body.targets, 21);
    assert_eq!(status_body.settings.channel, ch);
    assert!(
        status_body.settings.measure_bw_hz.is_some(),
        "the decoder's bandwidth is reported as the one measured"
    );
    assert!(get_state(&app).await.device_sets[0].scanner.is_some());

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/scanner"),
        Some(r#"{"action":"skip"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "nothing held to skip");

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
async fn hunt_start_stop_and_error_mapping_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;
    let ch = nfm_decoder(&app, ds).await;

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

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/hunt"),
        Some(r#"{"action":"start","settings":{"channel":99}}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a hunt needs a decoder that exists"
    );

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/hunt"),
        Some(&format!(
            r#"{{"action":"start","settings":{{"channel":{ch}}}}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let hunt: sdrmm_wire::HuntStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(hunt.settings.channel, ch);
    assert_eq!(hunt.freq_hz, 100_100_000.0, "the hunt reads the decoder");
    assert!(get_state(&app).await.device_sets[0].hunt.is_some());

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"gains":[]}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "a hunt leaves the radio to its operator"
    );

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
