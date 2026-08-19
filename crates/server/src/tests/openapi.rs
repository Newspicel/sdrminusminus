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
        "/api/devicesets/{ds}/record",
        "/api/devicesets/{ds}/channels/{ch}/record",
        "/api/devicesets/{ds}/channels/{ch}/baseband",
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
        "/api/decoderlog",
        "/api/decoderlog/export/{format}",
        "/api/calls",
        "/api/calls/{id}/audio",
        "/api/workspaces/{id}/apply",
        "/api/workspaces/{id}/undo",
        "/api/workspaces/{id}/redo",
        "/api/patch/catalog",
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
        "ChannelParams",
        "ChannelSettings",
        "PresetSnapshot",
        "RecordingStatus",
        "AudioRecordingStatus",
        "AudioRecordingInfo",
        "NetworkExportStatus",
        "ChannelNetworkExportRequest",
        "TimeMachineStatus",
        "TimeMachineRequest",
        "RecordingInfo",
        "DecoderLogEntry",
        "DecoderLogResponse",
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
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let store = Store::open(None).expect("in-memory store");
    let _router = router(
        Engine::with_registry(registry, None),
        store,
        &ServerOptions::default(),
    );
}
