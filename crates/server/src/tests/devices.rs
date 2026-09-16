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
        Some(r#"{"settings":{"frequency_hz":100100000.0,"params":{"type":"nfm","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ch = serde_json::from_slice::<CreatedId>(&body).expect("json").id;

    let (status, _) = request(
        app.clone(),
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{ch}"),
        Some(r#"{"frequency_hz":99800000.0,"params":{"type":"am","settings":{"agc":false}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let snap = get_state(&app).await;
    let channel = &snap.device_sets[0].channels[0];
    assert_eq!(channel.settings.frequency_hz, 99_800_000.0);
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
        Some(r#"{"frequency_hz":900000000.0,"params":{"type":"nfm","settings":{}}}"#),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "a decoder may sit where the radio is not listening"
    );
    let snap = get_state(&app).await;
    assert!(snap.device_sets[0].channels[0].out_of_band);

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

#[tokio::test]
async fn dab_transmission_modes_round_trip_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"frequency_hz":100000000.0,"params":{"type":"dab","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ch = serde_json::from_slice::<CreatedId>(&body)
        .expect("created channel")
        .id;
    for (name, expected) in [
        ("i", sdrmm_wire::DabTransmissionMode::I),
        ("ii", sdrmm_wire::DabTransmissionMode::Ii),
        ("iii", sdrmm_wire::DabTransmissionMode::Iii),
        ("iv", sdrmm_wire::DabTransmissionMode::Iv),
    ] {
        let body = serde_json::json!({"params": {"type": "dab", "settings": {"transmission_mode": name, "service_id": 49569}}}).to_string();
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/channels/{ch}"),
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let snapshot = get_state(&app).await;
        let sdrmm_wire::ChannelParams::Dab(params) =
            snapshot.device_sets[0].channels[0].settings.params
        else {
            panic!("DAB params");
        };
        assert_eq!(params.transmission_mode, expected);
        assert_eq!(params.service_id, Some(49569));
    }
    let (status, _) = request(
        app,
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{ch}"),
        Some(r#"{"params":{"type":"dab","settings":{"transmission_mode":"v"}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn terrestrial_bandwidth_priority_and_service_round_trip_over_http() {
    let dir = tempfile::TempDir::new().expect("recording directory");
    let stem = dir.path().join("terrestrial");
    let mut writer = sdrmm_recorder::SigmfWriter::create(
        &stem,
        12_000_000.0,
        100_000_000.0,
        "DVB-T settings test",
    )
    .expect("writer");
    writer
        .write_block(&vec![num_complex::Complex::new(0.0, 0.0); 65536])
        .expect("IQ");
    writer.finalize().expect("recording");
    let app = recording_router(dir.path());
    let body = serde_json::json!({
        "device_id": format!("recording:{}", stem.file_name().expect("stem").display()),
    })
    .to_string();
    let (status, body) = request(app.clone(), "POST", "/api/devicesets", Some(&body)).await;
    assert_eq!(status, StatusCode::OK);
    let ds = serde_json::from_slice::<CreatedId>(&body)
        .expect("device")
        .id;
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"frequency_hz":100000000.0,"params":{"type":"dvbt","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let ch = serde_json::from_slice::<CreatedId>(&body)
        .expect("channel")
        .id;
    for bandwidth in ["mhz6", "mhz7", "mhz8"] {
        let params = serde_json::json!({"type":"dvbt","settings":{"bandwidth":bandwidth,"low_priority":true,"program":42}});
        let body = serde_json::json!({"params":params}).to_string();
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/channels/{ch}"),
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let snapshot = get_state(&app).await;
        assert_eq!(
            serde_json::to_value(&snapshot.device_sets[0].channels[0].settings.params)
                .expect("params"),
            params
        );
    }
    let (status, _) = request(
        app,
        "PATCH",
        &format!("/api/devicesets/{ds}/channels/{ch}"),
        Some(r#"{"params":{"type":"dvbt","settings":{"bandwidth":"mhz5"}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn satellite_superframes_and_stream_selection_round_trip_over_http() {
    let app = test_router();
    let ds = create_virtual_set(&app).await;
    let (status, body) = request(app.clone(), "POST", &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"frequency_hz":100000000.0,"params":{"type":"datv","settings":{"standard":"dvb_s2"}}}}"#)).await;
    assert_eq!(status, StatusCode::OK);
    let ch = serde_json::from_slice::<CreatedId>(&body)
        .expect("created channel")
        .id;
    for enabled in [true, false] {
        let body = serde_json::json!({"params":{"type":"datv","settings":{"standard":"dvb_s2","superframes":enabled,"program":42,"input_stream":7}}}).to_string();
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/channels/{ch}"),
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let snapshot = get_state(&app).await;
        let sdrmm_wire::ChannelParams::Datv(params) =
            snapshot.device_sets[0].channels[0].settings.params
        else {
            panic!("DATV params");
        };
        assert_eq!(params.superframes, enabled);
        assert_eq!(params.input_stream, Some(7));
        assert_eq!(params.program, Some(42));
    }
}
