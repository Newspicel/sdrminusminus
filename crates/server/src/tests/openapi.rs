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

const NEW_ROUTES: [(&str, &str); 22] = [
    ("GET", "/api/arrays"),
    ("POST", "/api/arrays/{node}/calibrate"),
    ("PATCH", "/api/arrays/{node}/tune"),
    ("POST", "/api/arrays/{node}/recording"),
    ("DELETE", "/api/arrays/{node}/recording"),
    ("GET", "/api/radar/{node}"),
    ("DELETE", "/api/radar/{node}/tracks"),
    ("GET", "/api/phones"),
    ("PUT", "/api/phones/access"),
    ("POST", "/api/phones/offers"),
    ("DELETE", "/api/phones/offers"),
    ("POST", "/api/phones/pair"),
    ("GET", "/api/phones/self"),
    ("DELETE", "/api/phones/self"),
    ("PATCH", "/api/phones/{id}"),
    ("DELETE", "/api/phones/{id}"),
    ("GET", "/api/missions"),
    ("POST", "/api/missions/{node}/actions"),
    ("POST", "/api/missions/workspace"),
    ("GET", "/api/survey/{node}"),
    ("POST", "/api/survey/{node}"),
    ("GET", "/api/fusion/{node}"),
];

#[test]
fn every_new_route_is_in_the_contract() {
    let spec = serde_json::to_value(openapi()).expect("OpenAPI");
    for (method, path) in NEW_ROUTES {
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
