use std::{net::UdpSocket, path::Path, time::Instant};

use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sdrmm_wire::{
    AdsbMessage, ApiError, AprsPacket, Bookmark, CapturedImagesResponse, ChannelParams,
    ChannelSettings, ChannelTypesResponse, CreatedId, CreatedRowId, DecodedRecord, DecoderEvent,
    DecoderLogEntry, DecoderLogResponse, DeletedCount, DeviceSettings, NetworkExportStatus,
    NfmParams, NmeaDevicesResponse, PresetInfo, PresetSnapshot, RecordingStatus,
    RecordingsResponse, StateSnapshot, TimeMachineStatus, VoiceCallsResponse,
};
use tower::ServiceExt;

use super::*;

mod auth_mcp;
mod calls;
mod catalog;
mod channel_capture;
mod coherent;
mod cps;
mod decoderlog;
mod devices;
mod openapi;
mod presets;
mod recordings;
mod scanning;
mod templates;
mod workspaces;

struct NanoVnaStub;

impl sdrmm_tools::Tool for NanoVnaStub {
    fn descriptor(&self) -> sdrmm_wire::ToolDescriptor {
        sdrmm_wire::ToolDescriptor {
            id: sdrmm_wire::NANOVNA_TOOL_ID.to_owned(),
            name: "NanoVNA".to_owned(),
            summary: "Sweep S11 and S21 from a NanoVNA over USB serial".to_owned(),
            category: sdrmm_wire::ToolCategory::Instrument,
            needs_hardware: true,
        }
    }

    fn run(
        &self,
        request: sdrmm_wire::ToolRequest,
    ) -> Result<sdrmm_wire::ToolResponse, sdrmm_tools::ToolError> {
        match request {
            sdrmm_wire::ToolRequest::NanoVna(sdrmm_wire::NanoVnaRequest::ListDevices) => Ok(
                sdrmm_wire::ToolResponse::NanoVna(Box::new(sdrmm_wire::NanoVnaResult::Devices {
                    devices: vec![sdrmm_wire::NanoVnaDevice {
                        port: "fixture-port".to_owned(),
                        label: "Fixture NanoVNA".to_owned(),
                        match_kind: sdrmm_wire::NanoVnaMatch::Confirmed,
                        model: Some("NanoVNA-H4".to_owned()),
                        manufacturer: Some("nanovna.com".to_owned()),
                        product: Some("NanoVNA_H4".to_owned()),
                        serial_number: Some("fixture-serial".to_owned()),
                        usb_vid: Some(0x0483),
                        usb_pid: Some(0x5740),
                    }],
                    ignored_ports: vec!["fixture-gnss".to_owned()],
                })),
            ),
            request => Err(sdrmm_tools::ToolError::WrongTool {
                tool: sdrmm_wire::NANOVNA_TOOL_ID,
                got: request.tool_id().to_owned(),
            }),
        }
    }
}

fn test_router() -> Router {
    test_router_with_store().0
}

fn test_router_with_store() -> (Router, Arc<Store>) {
    let (router, state) = test_router_with_state();
    (router, state.store.clone())
}

fn test_router_with_state() -> (Router, AppState) {
    let store = Arc::new(Store::open(None).expect("in-memory store"));
    let state = state_over(store);
    let (router, background) = router_with_state(state.clone(), &ServerOptions::default());
    background.detach();
    (router, state)
}

fn state_over(store: Arc<Store>) -> AppState {
    let arrays = sdrmm_engine::ArrayCatalog::new();
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let mut state = AppState::new(Engine::with_arrays(registry, None, arrays), store);
    let mut tools = sdrmm_tools::ToolRegistry::default();
    tools
        .register(Box::new(sdrmm_tools::AntennaTool))
        .expect("register antenna fixture");
    tools
        .register(Box::new(NanoVnaStub))
        .expect("register NanoVNA fixture");
    state.tools = Arc::new(tools);
    state
}

fn recording_router(dir: &Path) -> Router {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(
        1,
        Box::new(sdrmm_device_virtual::VirtualDriver::with_recordings(
            dir.to_path_buf(),
        )),
    );
    let state = AppState::new(
        Engine::with_registry(registry, Some(dir.to_path_buf())),
        Arc::new(Store::open(None).expect("in-memory store")),
    );
    let (router, background) = router_with_state(state, &ServerOptions::default());
    background.detach();
    router
}

async fn request(app: Router, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Bytes) {
    let (status, _, bytes) = request_parts(app, method, uri, body, &[]).await;
    (status, bytes)
}

async fn request_parts(
    app: Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Bytes) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_owned())
        }
        None => Body::empty(),
    };
    let response = app
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, headers, bytes)
}

async fn create_virtual_set(app: &Router) -> u32 {
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/devicesets",
        Some(r#"{"device_id":"virtual:siggen"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice::<CreatedId>(&body).expect("json").id
}

async fn get_state(app: &Router) -> StateSnapshot {
    let (status, body) = request(app.clone(), "GET", "/api/state", None).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).expect("json")
}

fn preset_250k(channels: Vec<ChannelSettings>) -> PresetSnapshot {
    PresetSnapshot {
        version: sdrmm_wire::PRESET_SNAPSHOT_VERSION,
        devices: vec![sdrmm_wire::PresetDevice {
            node: "device".to_string(),
            device_id: "virtual:siggen".to_string(),
            settings: DeviceSettings {
                center_hz: Some(100_000_000.0),
                sample_rate: Some(250_000.0),
                ..DeviceSettings::default()
            },
            channels,
        }],
    }
}

fn nfm_at(offset_hz: f64) -> ChannelSettings {
    ChannelSettings {
        offset_hz,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Nfm(NfmParams::default()),
        audio: Default::default(),
    }
}

async fn record(app: &Router, ds: u32, action: &str) -> (StatusCode, Bytes) {
    request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/record"),
        Some(&format!(r#"{{"action":"{action}"}}"#)),
    )
    .await
}

async fn list_recordings(app: &Router) -> Vec<sdrmm_wire::RecordingInfo> {
    let (status, body) = request(app.clone(), "GET", "/api/recordings", None).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice::<RecordingsResponse>(&body)
        .expect("json")
        .recordings
}

async fn annotate(app: &Router, id: i64, body: &str) -> (StatusCode, Bytes) {
    request(
        app.clone(),
        "PUT",
        &format!("/api/recordings/{id}/annotation"),
        Some(body),
    )
    .await
}

async fn wait_for_recorded_samples(app: &Router, ds: u32, min: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snap = get_state(app).await;
        let recording = snap
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("set listed")
            .recording
            .clone();
        if recording.is_some_and(|r| r.samples >= min) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "recording never reached {min} samples"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn playback_set(app: &Router, rec: &sdrmm_wire::RecordingInfo) -> u32 {
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/devicesets",
        Some(&format!(r#"{{"device_id":"{}"}}"#, rec.device_id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice::<CreatedId>(&body).expect("json").id
}

async fn playback(app: &Router, ds: u32, body: &str) -> (StatusCode, Bytes) {
    request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/playback"),
        Some(body),
    )
    .await
}

async fn recorded(app: &Router) -> sdrmm_wire::RecordingInfo {
    let ds = create_virtual_set(app).await;
    record(app, ds, "start").await;
    wait_for_recorded_samples(app, ds, 1_024).await;
    record(app, ds, "stop").await;
    list_recordings(app).await.remove(0)
}

fn header_value(headers: &axum::http::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .map(|value| value.to_str().expect("ascii header").to_string())
        .unwrap_or_default()
}

async fn record_channel(app: &Router, ds: u32, ch: u32, action: &str) -> (StatusCode, Bytes) {
    request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/record"),
        Some(&format!(r#"{{"action":"{action}"}}"#)),
    )
    .await
}

async fn list_audio_recordings(app: &Router) -> Vec<sdrmm_wire::AudioRecordingInfo> {
    let (status, body) = request(app.clone(), "GET", "/api/audiorecordings", None).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice::<sdrmm_wire::AudioRecordingsResponse>(&body)
        .expect("json")
        .recordings
}

async fn wait_for_recorded_frames(app: &Router, ds: u32, ch: u32, min: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let recording = get_state(app)
            .await
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("set listed")
            .channels
            .iter()
            .find(|c| c.id == ch)
            .expect("channel listed")
            .audio_recording
            .clone();
        if recording.is_some_and(|r| r.frames >= min) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the audio recording never reached {min} frames"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn create_nfm_channel(app: &Router, ds: u32) -> u32 {
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels"),
        Some(r#"{"settings":{"params":{"type":"nfm","settings":{}}}}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice::<CreatedId>(&body).expect("json").id
}

async fn record_baseband(app: &Router, ds: u32, ch: u32, action: &str) -> (StatusCode, Bytes) {
    request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/channels/{ch}/baseband"),
        Some(&format!(r#"{{"action":"{action}"}}"#)),
    )
    .await
}

async fn wait_for_baseband_samples(app: &Router, ds: u32, ch: u32, min: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let recording = get_state(app)
            .await
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("set listed")
            .channels
            .iter()
            .find(|c| c.id == ch)
            .expect("channel listed")
            .baseband_recording
            .clone();
        if recording.is_some_and(|r| r.samples >= min) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the baseband recording never reached {min} samples"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn time_machine(app: &Router, ds: u32, body: serde_json::Value) -> (StatusCode, Bytes) {
    request(
        app.clone(),
        "POST",
        &format!("/api/devicesets/{ds}/time-machine"),
        Some(&body.to_string()),
    )
    .await
}

fn adsb_record(at: &str, device_set: u32, icao: &str, callsign: &str) -> DecodedRecord {
    DecodedRecord {
        device_set,
        channel: 0,
        at: at.to_string(),
        freq_hz: 1_090_000_000.0,
        event: DecoderEvent::Adsb(AdsbMessage {
            icao: icao.to_string(),
            df: 17,
            callsign: Some(callsign.to_string()),
            raw: "8D3C6444".to_string(),
            ..AdsbMessage::default()
        }),
    }
}

fn awkward_record(at: &str) -> DecodedRecord {
    DecodedRecord {
        device_set: 1,
        channel: 2,
        at: at.to_string(),
        freq_hz: 144_800_000.0,
        event: DecoderEvent::Aprs(AprsPacket {
            source: "DL1ABC-9".to_string(),
            destination: "APRS".to_string(),
            tnc2: "DL1ABC-9>APRS:hello, \"world\"".to_string(),
            ..AprsPacket::default()
        }),
    }
}

fn recent(seconds_ago: i64) -> String {
    (jiff::Timestamp::now() - jiff::SignedDuration::from_secs(seconds_ago)).to_string()
}

fn seed_decoder_log(store: &Store) {
    store
        .insert_decoder_events(
            &[
                adsb_record(&recent(180), 0, "3C6444", "DLH123"),
                awkward_record(&recent(120)),
                adsb_record(&recent(60), 0, "4CA2D4", "RYR9AB"),
            ],
            &crate::store::LogOrigin::unattributed(),
        )
        .expect("insert");
}

async fn mcp_call(app: &Router, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("host", "sdrmm.local:8080")
                .body(Body::from(call.to_string()))
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
    serde_json::from_slice(&bytes).expect("json-rpc body")
}

async fn workspace_detail(app: &Router, id: i64) -> sdrmm_wire::WorkspaceDetail {
    let (status, body) = request(app.clone(), "GET", &format!("/api/workspaces/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("json")
}

async fn step(app: &Router, id: i64, step: &str) -> sdrmm_wire::WorkspaceDetail {
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/workspaces/{id}/{step}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("json")
}

async fn channel_types(app: &Router) -> Vec<String> {
    get_state(app)
        .await
        .device_sets
        .iter()
        .flat_map(|set| &set.channels)
        .map(|channel| channel.settings.params.type_id().to_string())
        .collect()
}

async fn workspaces(app: &Router) -> sdrmm_wire::WorkspacesResponse {
    let (status, body) = request(app.clone(), "GET", "/api/workspaces", None).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).expect("json")
}

fn virtual_snapshot(key: &str, taps: &[(&str, &str, &str)]) -> sdrmm_wire::WorkspaceSnapshot {
    let mut snapshot = sdrmm_wire::WorkspaceSnapshot::starter();
    let sdrmm_wire::NodeBody::Device(node) = &mut snapshot.graph.nodes[0].body else {
        panic!("the default workspace opens with a receiver")
    };
    node.device = Some(sdrmm_wire::DeviceRef {
        backend: "virtual".to_string(),
        serial: None,
        key: Some(key.to_string()),
    });
    for (id, channel_type, port) in taps {
        snapshot.graph.nodes.push(sdrmm_wire::PatchNode {
            id: (*id).to_string(),
            body: sdrmm_wire::NodeBody::Channel(sdrmm_wire::ChannelNode {
                channel_type: (*channel_type).to_string(),
                record_calls: false,
            }),
            position: sdrmm_wire::Position { x: 400.0, y: 300.0 },
            size: None,
            label: None,
        });
        snapshot.graph.edges.push(sdrmm_wire::PatchEdge {
            from: sdrmm_wire::PortRef {
                node: "device".to_string(),
                port: (*port).to_string(),
            },
            to: sdrmm_wire::PortRef {
                node: (*id).to_string(),
                port: "iq".to_string(),
            },
        });
    }
    snapshot
}

async fn put_active_workspace(app: &Router, snapshot: &sdrmm_wire::WorkspaceSnapshot) -> i64 {
    put_workspace_revision(app, snapshot, 1).await
}

async fn put_workspace_revision(
    app: &Router,
    snapshot: &sdrmm_wire::WorkspaceSnapshot,
    revision: u64,
) -> i64 {
    let workspace = workspaces(app).await.active.expect("seeded workspace");
    let (status, body) = request(
        app.clone(),
        "PUT",
        &format!("/api/workspaces/{workspace}"),
        Some(&format!(
            r#"{{"revision":{revision},"snapshot":{}}}"#,
            serde_json::to_string(snapshot).unwrap()
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    workspace
}

async fn store_siggen_workspace(app: &Router) -> i64 {
    put_active_workspace(app, &virtual_snapshot("siggen", &[("voice", "nfm", "iq")])).await
}

async fn apply(app: &Router, workspace: i64) -> sdrmm_wire::PatchApplyReport {
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/workspaces/{workspace}/apply"),
        Some("{}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).expect("json")
}

async fn store_second_workspace(app: &Router, name: &str, channel_type: &str) -> i64 {
    let snapshot = virtual_snapshot("siggen", &[("other", channel_type, "iq")]);
    let (status, body) = request(
        app.clone(),
        "POST",
        "/api/workspaces",
        Some(&format!(
            r#"{{"name":"{name}","snapshot":{}}}"#,
            serde_json::to_string(&snapshot).unwrap()
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let created: sdrmm_wire::CreatedRowId = serde_json::from_slice(&body).expect("json");
    created.id
}

async fn activate(app: &Router, workspace: i64) {
    let (status, body) = request(
        app.clone(),
        "POST",
        &format!("/api/workspaces/{workspace}/activate"),
        Some("{}"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&body)
    );
}
