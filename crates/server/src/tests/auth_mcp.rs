use super::*;

#[tokio::test]
async fn token_auth_gates_the_api_and_advertises_itself() {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let store = Store::open(None).expect("in-memory store");
    let app = router(
        Engine::with_registry(registry, None),
        store,
        &ServerOptions {
            dev_cors: false,
            token: Some("s3cret".to_string()),
            ..ServerOptions::default()
        },
    );

    let (status, body) = request(app.clone(), "GET", "/api/auth", None).await;
    assert_eq!(status, StatusCode::OK);
    let info: sdrmm_wire::AuthInfo = serde_json::from_slice(&body).expect("json");
    assert!(info.token_required);

    let (status, _) = request(app.clone(), "GET", "/api/state", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = request(app.clone(), "GET", "/api/state?token=s3cret", None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = request(app, "GET", "/", None).await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_reports_not_required_by_default() {
    let (status, body) = request(test_router(), "GET", "/api/auth", None).await;
    assert_eq!(status, StatusCode::OK);
    let info: sdrmm_wire::AuthInfo = serde_json::from_slice(&body).expect("json");
    assert!(!info.token_required);
}

#[tokio::test]
async fn mcp_is_mounted_and_shares_the_token_gate() {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let app = router(
        Engine::with_registry(registry, None),
        Store::open(None).expect("in-memory store"),
        &ServerOptions {
            dev_cors: false,
            token: Some("s3cret".to_string()),
            ..ServerOptions::default()
        },
    );
    let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let (status, _) = request(app.clone(), "POST", "/mcp", Some(call)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("host", "sdrmm.local:8080")
                .header("authorization", "Bearer s3cret")
                .body(Body::from(call))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc body");
    let tools = json["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tools in {json}"));
    assert!(
        tools.iter().any(|t| t["name"] == "get_state"),
        "get_state missing from the tool list"
    );
}

#[tokio::test]
async fn mcp_serves_the_tool_bench_beside_the_receiver() {
    let app = test_router();

    let listed = mcp_call(&app, "list_tools", serde_json::json!({})).await;
    let tools = listed["result"]["structuredContent"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tool bench in {listed}"));
    assert!(tools.iter().any(|tool| tool["id"] == "antenna"));
    assert!(tools.iter().any(|tool| tool["id"] == "nanovna"));

    let found = mcp_call(&app, "nanovna_list_devices", serde_json::json!({})).await;
    let result = &found["result"]["structuredContent"];
    assert_eq!(result["kind"], "devices", "{found}");
    assert_eq!(result["devices"][0]["port"], "fixture-port");
    assert_eq!(result["ignored_ports"][0], "fixture-gnss");

    let cut = mcp_call(
        &app,
        "design_antenna",
        serde_json::json!({
            "frequency_hz": 145_500_000.0,
            "design": "yagi",
            "directors": 3,
        }),
    )
    .await;
    let report = &cut["result"]["structuredContent"];
    assert_eq!(report["design"]["type"], "yagi", "{cut}");
    assert_eq!(report["design"]["settings"]["directors"], 3);
    let parts = report["parts"]
        .as_array()
        .unwrap_or_else(|| panic!("no parts in {cut}"));
    assert!(parts.iter().any(|part| part["name"] == "Director 3"));
}

#[tokio::test]
async fn mcp_tool_bench_refusals_name_what_was_wrong() {
    let app = test_router();

    let unknown = mcp_call(
        &app,
        "design_antenna",
        serde_json::json!({ "frequency_hz": 145_500_000.0, "design": "helix" }),
    )
    .await;
    assert!(
        unknown["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("helix")),
        "{unknown}"
    );

    let refused = mcp_call(
        &app,
        "design_antenna",
        serde_json::json!({ "frequency_hz": 0.0, "design": "dipole" }),
    )
    .await;
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("frequency_hz")),
        "{refused}"
    );

    let too_many = mcp_call(
        &app,
        "nanovna_sweep",
        serde_json::json!({
            "port": "fixture-port",
            "start_hz": 1_000_000,
            "stop_hz": 30_000_000,
            "points": 10_001,
        }),
    )
    .await;
    assert!(
        too_many["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("401 points")),
        "{too_many}"
    );

    let no_slot = mcp_call(
        &app,
        "nanovna_calibrate",
        serde_json::json!({ "port": "fixture-port", "step": "save" }),
    )
    .await;
    assert!(
        no_slot["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("slot")),
        "{no_slot}"
    );
}
