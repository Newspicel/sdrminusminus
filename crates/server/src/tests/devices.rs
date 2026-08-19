use super::*;

#[tokio::test]
async fn get_state_returns_empty_snapshot() {
    let app = test_router();
    let snap = get_state(&app).await;
    assert!(snap.device_sets.is_empty());
}

#[tokio::test]
async fn nmea_device_catalog_is_available_over_http() {
    let (status, body) = request(test_router(), "GET", "/api/position/nmea-devices", None).await;
    assert_eq!(status, StatusCode::OK);
    let response: NmeaDevicesResponse = serde_json::from_slice(&body).expect("NMEA devices");
    assert!(
        response
            .devices
            .iter()
            .all(|device| !device.path.is_empty())
    );
}

#[tokio::test]
async fn create_and_delete_device_set_over_http() {
    let app = test_router();
    create_virtual_set(&app).await;

    let (status, _) = request(
        app,
        "POST",
        "/api/devicesets",
        Some(r#"{"device_id":"virtual:nope"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn channeltypes_lists_every_demod_exactly_once() {
    let (status, body) = request(test_router(), "GET", "/api/channeltypes", None).await;
    assert_eq!(status, StatusCode::OK);
    let types: ChannelTypesResponse = serde_json::from_slice(&body).expect("json");
    for id in ["nfm", "selcall", "am", "ssb", "wfm", "freedv"] {
        assert!(
            types.types.iter().any(|t| t.type_id == id),
            "missing type {id}"
        );
    }
    let unique: std::collections::HashSet<&str> =
        types.types.iter().map(|t| t.type_id.as_str()).collect();
    assert_eq!(unique.len(), types.types.len());
}

#[tokio::test]
async fn channel_create_patch_and_error_mapping_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;

    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"offset_hz":100000.0,"params":{"type":"nfm","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ch = serde_json::from_slice::<CreatedId>(&body).expect("json").id;

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{ch}"),
        Some(r#"{"offset_hz":-200000.0,"params":{"type":"am","settings":{"agc":false}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let snap = get_state(&app).await;
    let channel = &snap.device_sets[0].channels[0];
    assert_eq!(channel.settings.offset_hz, -200_000.0);
    assert_eq!(channel.settings.params.type_id(), "am");

    let (status, body) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{ch}"),
        Some(r#"{"params":{"type":"zzz","settings":{}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{ch}"),
        Some(r#"{"offset_hz":5000000.0,"params":{"type":"nfm","settings":{}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let valid = r#"{"params":{"type":"nfm","settings":{}}}"#;
    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/999"),
        Some(valid),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = request(app, "PATCH", "/api/devicesets/999/channels/1", Some(valid)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn opening_a_radio_that_is_already_open_conflicts() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/devicesets",
        Some(r#"{"device_id":"virtual:siggen"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
    assert!(err.error.contains("already open"), "{err:?}");

    let (status, _) = request(app, "DELETE", &format!("/api/devicesets/{ds}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn extractor_rejections_return_api_error_body() {
    let app = test_router();

    let (status, body) = request(app.clone(), "POST", "/api/devicesets", Some("{not json")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
    assert_eq!(err.error, "invalid request body");
    assert!(err.detail.is_some());

    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/devicesets",
        Some(r#"{"nope":1}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
    assert_eq!(err.error, "invalid request body");

    let (status, body) = request(app, "DELETE", "/api/devicesets/abc", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
    assert_eq!(err.error, "invalid path parameter");
}
