use super::*;

#[test]
fn openapi_registers_paths_and_ws_schemas() {
    let spec = openapi().to_pretty_json().expect("serialize");
    for path in [
        "/api/state",
        "/api/devices",
        "/api/channeltypes",
        "/api/devicesets",
        "/api/devicesets/{ds}/device",
        "/api/devicesets/{ds}/channels/{ch}",
        "/api/presets",
        "/api/presets/{id}",
        "/api/presets/{id}/apply",
        "/api/bookmarks",
        "/api/bookmarks/{id}",
        "/api/devicesets/{ds}/channels/{ch}/network-export",
        "/api/devicesets/{ds}/time-machine",
        "/api/audiorecordings",
        "/api/audiorecordings/{file}",
        "/api/audiorecordings/{file}/download",
        "/api/devicesets/{ds}/network-export",
        "/api/devicesets/{ds}/playback",
        "/api/recordings",
        "/api/recordings/{id}",
        "/api/recordings/{id}/download",
        "/api/recordings/{id}/annotation",
        "/api/decoderlog",
        "/api/decoderlog/export/{format}",
        "/api/calls",
        "/api/calls/{id}/audio",
        "/api/workspaces/import",
        "/api/workspaces/{id}/export",
        "/api/workspaces/{id}/apply",
        "/api/workspaces/{id}/undo",
        "/api/workspaces/{id}/redo",
        "/api/workspaces/{id}/notices/{notice}",
        "/api/patch/catalog",
        "/api/diagnostics",
        "/api/tools",
        "/api/tools/run",
        "/api/images",
        "/api/images/{id}/png",
    ] {
        assert!(spec.contains(path), "missing path {path}");
    }
    assert!(spec.contains("ServerEvent"), "ServerEvent schema missing");
    assert!(
        spec.contains("ClientCommand"),
        "ClientCommand schema missing"
    );
    for schema in [
        "DvbtParams",
        "DvbtBandwidth",
        "BroadcastData",
        "ChannelParams",
        "ChannelSettings",
        "PresetSnapshot",
        "RecordingStatus",
        "AudioRecordingStatus",
        "AudioRecordingInfo",
        "RecordingInfo",
        "RecordingAnnotation",
        "NetworkExportStatus",
        "ChannelNetworkExportRequest",
        "TimeMachineStatus",
        "TimeMachineRequest",
        "RecordingInfo",
        "DecoderLogEntry",
        "DecoderLogResponse",
        "DiagnosticsReport",
        "LogLine",
        "LogLevel",
        "ErrorCode",
        "VoiceCall",
        "VoiceCallsResponse",
        "CapturedImage",
        "CapturedImagesResponse",
        "SstvParams",
        "SstvPicture",
        "DecoderEvent",
        "FlexMessage",
        "ErmesMessage",
        "CwSkimmerSpot",
        "SelcallSequence",
        "FreeDvParams",
        "DeletedCount",
        "PatchGraph",
        "EventOutputNode",
        "EventOutputTarget",
        "WebhookFormat",
        "RackLayout",
        "DeviceRef",
        "PatchCatalog",
        "PatchApplyReport",
        "PlacementCoverage",
        "WorkspaceExport",
        "WorkspaceState",
        "ToolDescriptor",
        "ToolRequest",
        "ToolResponse",
        "AntennaDesign",
        "AntennaReport",
        "NanoVnaRequest",
        "NanoVnaSweep",
        "NanoVnaDeviceReport",
        "NanoVnaCalibration",
        "NanoVnaCalStep",
    ] {
        assert!(
            spec.contains(&format!("\"{schema}\"")),
            "{schema} schema missing"
        );
    }
    let spec: serde_json::Value = serde_json::from_str(&spec).expect("spec is JSON");
    for params in ["VorParams", "IlsParams"] {
        let report_ms = &spec["components"]["schemas"][params]["properties"]["report_ms"];
        assert_eq!(
            report_ms["minimum"],
            serde_json::json!(sdrmm_wire::MIN_NAVAID_REPORT_MS),
            "{params} report_ms minimum"
        );
        assert_eq!(
            report_ms["maximum"],
            serde_json::json!(sdrmm_wire::MAX_NAVAID_REPORT_MS),
            "{params} report_ms maximum"
        );
    }
}

#[test]
fn router_builds_outside_a_tokio_runtime() {
    let _router = test_router_with_options(&ServerOptions::default());
}

#[test]
fn openapi_matches_the_committed_snapshot() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../../openapi.json")).expect("snapshot");
    let actual: serde_json::Value =
        serde_json::from_str(&openapi().to_pretty_json().expect("OpenAPI")).expect("OpenAPI");
    assert_eq!(
        actual, expected,
        "regenerate the wire schema with cargo xtask codegen"
    );
}

const NEW_ROUTES: [(&str, &str, Option<&str>); 22] = [
    ("GET", "/api/arrays", None),
    ("POST", "/api/arrays/{node}/calibrate", None),
    (
        "PATCH",
        "/api/arrays/{node}/tune",
        Some(r#"{"center_hz":433920000}"#),
    ),
    ("POST", "/api/arrays/{node}/recording", Some("{}")),
    ("DELETE", "/api/arrays/{node}/recording", None),
    ("GET", "/api/radar/{node}", None),
    ("DELETE", "/api/radar/{node}/tracks", None),
    ("GET", "/api/phones", None),
    (
        "PUT",
        "/api/phones/access",
        Some(r#"{"enabled":false,"port":8443}"#),
    ),
    ("POST", "/api/phones/offers", Some("{}")),
    ("DELETE", "/api/phones/offers", None),
    (
        "POST",
        "/api/phones/pair",
        Some(r#"{"code":"12345678","name":"Pixel","platform":"android","protocol":1}"#),
    ),
    ("GET", "/api/phones/self", None),
    ("DELETE", "/api/phones/self", None),
    ("PATCH", "/api/phones/{id}", Some(r#"{"name":"Pixel"}"#)),
    ("DELETE", "/api/phones/{id}", None),
    ("GET", "/api/missions", None),
    (
        "POST",
        "/api/missions/{node}/actions",
        Some(r#"{"action":"mark"}"#),
    ),
    (
        "POST",
        "/api/missions/workspace",
        Some(r#"{"workspace":1}"#),
    ),
    ("GET", "/api/survey/{node}", None),
    ("POST", "/api/survey/{node}", Some(r#"{"action":"start"}"#)),
    ("GET", "/api/fusion/{node}", None),
];

#[test]
fn every_new_route_is_in_the_contract() {
    let spec = serde_json::to_value(openapi()).expect("OpenAPI");
    for (method, path, _) in NEW_ROUTES {
        assert!(
            spec["paths"][path][method.to_lowercase()].is_object(),
            "{method} {path} missing from the contract"
        );
    }
    assert!(spec["paths"]["/api/fusion/{node}"]["delete"].is_object());
    let schemas = &spec["components"]["schemas"];
    for schema in [
        "ArrayStatus",
        "ArrayTuneRequest",
        "RadarUpdate",
        "PhonesResponse",
        "PairingOffer",
        "MissionsResponse",
        "SurveyGrid",
        "ProcessorReading",
        "SurfaceFit",
        "NodeTypeInfo",
    ] {
        assert!(schemas[schema].is_object(), "{schema} schema missing");
    }
}

#[tokio::test]
async fn a_route_whose_owner_has_not_landed_says_so() {
    for (method, path, body) in NEW_ROUTES {
        if path == "/api/fusion/{node}"
            || path.starts_with("/api/arrays")
            || path.starts_with("/api/phones")
            || path.starts_with("/api/survey/")
        {
            continue;
        }
        let uri = path.replace("{node}", "arr").replace("{id}", "ab12");
        let (status, answer) = request(test_router(), method, &uri, body).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {uri}: {}",
            String::from_utf8_lossy(&answer)
        );
        let error: ApiError = serde_json::from_slice(&answer).expect("error body");
        assert_eq!(error.error, "Not built yet", "{method} {uri}");
    }
}
