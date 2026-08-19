use super::*;

#[tokio::test]
async fn decoder_log_lists_newest_first_and_filters() {
    let (app, store) = test_router_with_store();
    seed_decoder_log(&store);

    let (status, body) = request(app.clone(), "GET", "/api/decoderlog", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: DecoderLogResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(listed.total, 3);
    assert_eq!(listed.dropped, 0);
    assert_eq!(listed.entries.len(), 3);
    assert_eq!(listed.entries[0].station.as_deref(), Some("4CA2D4"));
    assert_eq!(listed.entries[2].station.as_deref(), Some("3C6444"));

    let (status, body) = request(
        app.clone(),
        "GET",
        "/api/decoderlog?kind=aprs&device_set=1&limit=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let filtered: DecoderLogResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.entries[0].kind, "aprs");

    let (status, body) = request(app.clone(), "GET", "/api/decoderlog?since=yesterday", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

    let (status, body) = request(app, "GET", "/api/decoderlog?limit=lots", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
    assert_eq!(err.error, "invalid query parameter");
}

#[tokio::test]
async fn decoder_log_clear_removes_only_the_filtered_rows() {
    let (app, store) = test_router_with_store();
    seed_decoder_log(&store);

    let (status, body) = request(app.clone(), "DELETE", "/api/decoderlog?kind=adsb", None).await;
    assert_eq!(status, StatusCode::OK);
    let deleted: DeletedCount = serde_json::from_slice(&body).expect("json");
    assert_eq!(deleted.deleted, 2);

    let (_, body) = request(app, "GET", "/api/decoderlog", None).await;
    let listed: DecoderLogResponse = serde_json::from_slice(&body).expect("json");
    assert_eq!(listed.total, 1);
    assert_eq!(listed.entries[0].kind, "aprs");
}

#[tokio::test]
async fn decoder_log_clear_emits_the_decoder_log_scope() {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let engine = Engine::with_registry(registry, None);
    let store = Arc::new(Store::open(None).expect("in-memory store"));
    seed_decoder_log(&store);
    let mut events = engine.subscribe_events();
    let (app, background) =
        router_with_state(AppState::new(engine, store), &ServerOptions::default());
    background.detach();

    let (status, _) = request(app, "DELETE", "/api/decoderlog?kind=adsb", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(matches!(
        events.try_recv().expect("scope emitted"),
        sdrmm_wire::ServerEvent::StateChanged {
            scope: sdrmm_wire::StateScope::DecoderLog
        }
    ));
}

#[tokio::test]
async fn decoder_log_exports_csv_and_json() {
    let (app, store) = test_router_with_store();
    seed_decoder_log(&store);

    let (status, body) = request(
        app.clone(),
        "GET",
        "/api/decoderlog/export/csv?limit=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let csv = String::from_utf8(body.to_vec()).expect("utf-8");
    let mut lines = csv.split_terminator("\r\n");
    assert_eq!(
        lines.next(),
        Some("at,device_set,channel,kind,freq_hz,station,summary,event")
    );
    assert_eq!(csv.split_terminator("\r\n").count(), 4);
    assert!(
        csv.contains(r#""DL1ABC-9>APRS:hello, ""world""""#),
        "unquoted CSV field: {csv}"
    );

    let (status, body) = request(app, "GET", "/api/decoderlog/export/json", None).await;
    assert_eq!(status, StatusCode::OK);
    let exported: Vec<DecoderLogEntry> = serde_json::from_slice(&body).expect("json");
    assert_eq!(exported.len(), 3);
    assert_eq!(exported[0].station.as_deref(), Some("4CA2D4"));
    assert_eq!(
        exported[1].event,
        awkward_record("2026-08-09T12:00:01Z").event
    );
}

#[tokio::test]
async fn decoder_log_export_sets_download_headers() {
    let (app, store) = test_router_with_store();
    seed_decoder_log(&store);
    for (format, content_type) in [
        ("csv", "text/csv; charset=utf-8"),
        ("json", "application/json"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/decoderlog/export/{format}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(
            headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .expect("content-type"),
            content_type
        );
        let disposition = headers
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .expect("content-disposition");
        assert!(
            disposition.starts_with("attachment; filename=\"decoderlog-")
                && disposition.ends_with(&format!(".{format}\"")),
            "unusable download name: {disposition}"
        );
    }

    let (status, _) = request(app, "GET", "/api/decoderlog/export/xml", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
