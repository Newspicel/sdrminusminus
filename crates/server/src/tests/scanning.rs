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
        &format!("/api/devicesets/{ds}/channels/{ch}/scanner"),
        Some(r#"{"action":"start"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/scanner"),
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/scanner"),
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
        &format!("/api/devicesets/{ds}/channels/99/scanner"),
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
        r#"{{"action":"start","settings":{{"channel":{ch},"ranges":[{{"start_hz":99000000.0,"stop_hz":101000000.0,"step_hz":100000.0}}],"threshold_db":100.0,"dwell_ms":40,"hardware_sweep":false}}}}"#
    );
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/scanner"),
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
    let listed = get_state(&app).await.device_sets[0].scanners.clone();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].settings.channel, ch);

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/scanner"),
        Some(r#"{"action":"skip"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "nothing held to skip");

    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/device"),
        Some(r#"{"center_hz":88000000.0}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "a scan never owns the dial: {}",
        String::from_utf8_lossy(&body)
    );

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/scanner"),
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(get_state(&app).await.device_sets[0].scanners.is_empty());

    let (status, _) = request(
        app,
        "POST",
        &format!("/api/devicesets/999/channels/{ch}/scanner"),
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
        &format!("/api/devicesets/{ds}/channels/{ch}/hunt"),
        Some(r#"{"action":"start"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/hunt"),
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/99/hunt"),
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
        &format!("/api/devicesets/{ds}/channels/{ch}/hunt"),
        Some(&format!(
            r#"{{"action":"start","settings":{{"channel":{ch}}}}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let hunt: sdrmm_wire::HuntStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(hunt.settings.channel, ch);
    assert_eq!(hunt.freq_hz, 100_100_000.0, "the hunt reads the decoder");
    assert_eq!(get_state(&app).await.device_sets[0].hunts.len(), 1);

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
        &format!("/api/devicesets/{ds}/channels/{ch}/hunt"),
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(get_state(&app).await.device_sets[0].hunts.is_empty());

    let (status, _) = request(
        app,
        "POST",
        "/api/devicesets/999/hunt",
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

async fn hunt_error(app: &Router, uri: &str, body: &str) -> String {
    let (status, body) = request(app.clone(), "POST", uri, Some(body)).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice::<ApiError>(&body)
        .expect("ApiError body")
        .error
}

fn phone_pose(heading_deg: Option<f64>) -> sdrmm_wire::PositionFix {
    sdrmm_wire::PositionFix {
        latitude: 48.137,
        longitude: 11.575,
        altitude_m: None,
        accuracy_m: Some(5.0),
        speed_mps: None,
        track_deg: None,
        time: jiff::Timestamp::now().to_string(),
        attitude: sdrmm_wire::Attitude {
            heading_deg,
            ..sdrmm_wire::Attitude::default()
        },
    }
}

fn now_ms() -> u64 {
    u64::try_from(jiff::Timestamp::now().as_millisecond()).expect("after 1970")
}

#[tokio::test]
async fn hunt_mark_without_heading_is_409() {
    let (app, state) = test_router_with_state();
    let ds = create_virtual_set(&app).await;
    let ch = nfm_decoder(&app, ds).await;
    let hunt = format!("/api/devicesets/{ds}/channels/{ch}/hunt");
    let mark = r#"{"action":"mark"}"#;

    assert_eq!(hunt_error(&app, &hunt, mark).await, "Not running");
    assert_eq!(
        hunt_error(&app, &hunt, r#"{"action":"sweep"}"#).await,
        "No hunt node"
    );

    let plain = format!(r#"{{"action":"start","settings":{{"channel":{ch}}}}}"#);
    let (status, body) = request(app.clone(), "POST", &hunt, Some(&plain)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(hunt_error(&app, &hunt, mark).await, "No hunt node");

    let (status, body) = request(
        app.clone(),
        "POST",
        &hunt,
        Some(&format!(
            r#"{{"action":"sweep","settings":{{"channel":{ch},"node":"hunt-1","sweep":{{"mount_offset_deg":-90}}}}}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let sweeping: sdrmm_wire::HuntStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        sweeping.sweep.map(|sweep| sweep.state),
        Some(sdrmm_wire::SweepState::NoHeading),
        "a running hunt takes the sweep on"
    );
    assert_eq!(sweeping.settings.node.as_deref(), Some("hunt-1"));
    assert_eq!(sweeping.settings.sweep.mount_offset_deg, -90.0);
    assert_eq!(hunt_error(&app, &hunt, mark).await, "No position");

    state
        .engine
        .hunt_pose(ds, ch, phone_pose(None), now_ms())
        .expect("pose");
    assert_eq!(hunt_error(&app, &hunt, mark).await, "No heading");

    let mut decoded = state.engine.subscribe_decoded();
    state
        .engine
        .hunt_pose(ds, ch, phone_pose(Some(100.0)), now_ms())
        .expect("pose");
    let (status, body) = request(app.clone(), "POST", &hunt, Some(mark)).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let bearing = loop {
        let record = tokio::time::timeout(std::time::Duration::from_secs(5), decoded.recv())
            .await
            .expect("mark event")
            .expect("decoded stream");
        if let DecoderEvent::Df(bearing) = record.event {
            assert_eq!(
                record.origin.map(|origin| origin.node).as_deref(),
                Some("hunt-1")
            );
            break bearing;
        }
    };
    assert_eq!(bearing.bearing_deg, 10.0);
    assert_eq!(bearing.source, sdrmm_wire::BearingSource::Mark);

    let (status, body) = request(app.clone(), "POST", &hunt, Some(&plain)).await;
    assert_eq!(status, StatusCode::OK);
    let started: sdrmm_wire::HuntStatus = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        started.sweep.map(|sweep| sweep.state),
        Some(sdrmm_wire::SweepState::Off)
    );
    let (status, _) = request(app, "POST", &hunt, Some(r#"{"action":"stop"}"#)).await;
    assert_eq!(status, StatusCode::OK);
}
