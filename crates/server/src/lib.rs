use std::{
    collections::HashSet,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::Router;
use sdrmm_engine::Engine;
use tower_http::{
    compression::predicate::{NotForContentType, Predicate},
    cors::CorsLayer,
};
use utoipa_swagger_ui::SwaggerUi;

pub const HOTPLUG_INTERVAL: Duration = Duration::from_secs(5);
pub const LEVEL_INTERVAL: Duration = Duration::from_millis(100);

const DECODED_TEXT_CAP: usize = 1024;

mod assets;
mod auth;
mod bandplan;
mod calls;
mod chat_output;
mod decoderlog;
pub mod doctor;
mod events;
mod gps;
mod images;
mod ionosonde;
mod mcp;
pub mod notices;
mod rest;
mod store;
mod templates;
mod tracks;
mod trunking;
mod workspace;
mod ws;

pub use store::{Store, StoreError};

#[derive(Clone, Debug, Default)]
pub struct ServerOptions {
    pub dev_cors: bool,
    pub token: Option<String>,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub engine: Arc<Engine>,
    pub store: Arc<Store>,
    pub auth: auth::Auth,
    pub db_path: Option<PathBuf>,
    pub recordings_gate: Arc<std::sync::Mutex<()>>,
    pub apply_gate: Arc<std::sync::Mutex<()>>,
    decoder_log_dropped: Arc<AtomicU64>,
    pub decoded_text: tokio::sync::broadcast::Sender<axum::extract::ws::Utf8Bytes>,
    pub(crate) tracks: Arc<tracks::Tracks>,
    pub(crate) calls: Arc<calls::Calls>,
    pub(crate) images: Arc<images::Images>,
    pub(crate) ionosonde: Arc<ionosonde::Ionosonde>,
    pub clients: Arc<std::sync::atomic::AtomicU32>,
    pub(crate) tools: Arc<sdrmm_tools::ToolRegistry>,
    pub(crate) unrestored: Arc<std::sync::Mutex<Vec<String>>>,
    pub(crate) restored: Arc<std::sync::Mutex<HashSet<(i64, String, u32)>>>,
    pub(crate) gps: Arc<gps::GpsHub>,
}

impl AppState {
    fn new(engine: Arc<Engine>, store: Arc<Store>) -> Self {
        Self {
            engine,
            store,
            auth: auth::Auth::default(),
            db_path: None,
            recordings_gate: Arc::new(std::sync::Mutex::new(())),
            apply_gate: Arc::new(std::sync::Mutex::new(())),
            decoder_log_dropped: Arc::new(AtomicU64::new(0)),
            decoded_text: tokio::sync::broadcast::channel(DECODED_TEXT_CAP).0,
            tracks: Arc::new(tracks::Tracks::default()),
            calls: Arc::new(calls::Calls::default()),
            images: Arc::new(images::Images::default()),
            ionosonde: Arc::new(ionosonde::Ionosonde::default()),
            clients: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            tools: Arc::new(sdrmm_tools::ToolRegistry::with_builtins()),
            unrestored: Arc::new(std::sync::Mutex::new(Vec::new())),
            restored: Arc::new(std::sync::Mutex::new(HashSet::new())),
            gps: Arc::new(gps::GpsHub::default()),
        }
    }

    pub fn decoder_log_dropped(&self) -> u64 {
        self.decoder_log_dropped.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub db_path: Option<PathBuf>,
    pub options: ServerOptions,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            db_path: None,
            options: ServerOptions::default(),
        }
    }
}

#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    let (_router, api) = rest::openapi_router().split_for_parts();
    api
}

pub fn router(engine: Arc<Engine>, store: Store, options: &ServerOptions) -> Router {
    let mut state = AppState::new(engine, Arc::new(store));
    state.auth = auth::Auth::new(options.token.as_deref());
    let (router, background) = router_with_state(state, options);
    background.detach();
    router
}

fn router_with_state(state: AppState, options: &ServerOptions) -> (Router, Background) {
    let background = start_background(&state);
    ws::start_decoded_encoder(&state);
    workspace::spawn_autosave(&state);
    state.gps.reconcile(&state);
    let (api_router, api) = rest::openapi_router().split_for_parts();

    let mut app = Router::new()
        .merge(api_router)
        .route("/api/ws", axum::routing::get(ws::handler))
        .merge(mcp::router(
            state.engine.clone(),
            state.store.clone(),
            state.tools.clone(),
            state.recordings_gate.clone(),
        ))
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", api))
        .route_layer(axum::middleware::from_fn_with_state(
            state.auth.clone(),
            auth::require_token,
        ))
        .fallback(assets::static_handler)
        .with_state(state)
        .layer(
            tower_http::compression::CompressionLayer::new().compress_when(
                tower_http::compression::predicate::DefaultPredicate::new()
                    .and(NotForContentType::const_new("application/x-tar"))
                    .and(NotForContentType::const_new("audio/wav")),
            ),
        );

    if options.dev_cors {
        app = app.layer(CorsLayer::very_permissive());
    }
    (app, background)
}

struct Background {
    tasks: Vec<BackgroundTask>,
    detached: bool,
}

enum BackgroundTask {
    Task(tokio::task::JoinHandle<()>),
    Owned,
}

impl Background {
    fn detach(mut self) {
        self.detached = true;
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        if self.detached {
            return;
        }
        for task in &self.tasks {
            if let BackgroundTask::Task(task) = task {
                task.abort();
            }
        }
    }
}

fn start_background(state: &AppState) -> Background {
    let (recording_tx, recording_rx) = tokio::sync::watch::channel(trunking::Recording::default());
    let log = {
        let engine = state.engine.clone();
        let store = state.store.clone();
        let dropped = state.decoder_log_dropped.clone();
        spawn_task("sdrmm-decoderlog", move || {
            decoderlog::run(engine, store, dropped)
        })
    };
    let patch = {
        let engine = Arc::downgrade(&state.engine);
        let store = state.store.clone();
        spawn_task("sdrmm-trunking", move || {
            trunking::watch_patch(engine, store, recording_tx)
        })
    };
    let calls = {
        let engine = Arc::downgrade(&state.engine);
        let calls = state.calls.clone();
        spawn_task("sdrmm-calls", move || {
            calls::run(engine, calls, recording_rx)
        })
    };
    let images = {
        let engine = Arc::downgrade(&state.engine);
        let images = state.images.clone();
        spawn_task("sdrmm-images", move || images::run(engine, images))
    };
    let chat_output = {
        let engine = Arc::downgrade(&state.engine);
        let store = state.store.clone();
        let calls = state.calls.clone();
        spawn_task("sdrmm-chat-output", move || {
            chat_output::run(engine, store, calls)
        })
    };
    Background {
        tasks: vec![log, patch, calls, images, chat_output],
        detached: false,
    }
}

fn spawn_task<F, Fut>(name: &'static str, make: F) -> BackgroundTask
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let _guard = handle.enter();
        return BackgroundTask::Task(tokio::spawn(make()));
    }
    let spawned = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(make()),
                Err(err) => tracing::error!(error = %err, name, "no runtime for a background task"),
            }
        });
    if let Err(err) = spawned {
        tracing::error!(error = %err, name, "failed to start a background task");
    }
    BackgroundTask::Owned
}

pub struct ServerHandle {
    pub local_addr: SocketAddr,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    _background: Background,
}

impl ServerHandle {
    pub async fn join(self) -> std::io::Result<()> {
        match self.task.await {
            Ok(res) => res,
            Err(join_err) => Err(std::io::Error::other(join_err)),
        }
    }
}

pub async fn serve(config: Config, engine: Arc<Engine>) -> std::io::Result<ServerHandle> {
    engine.start_hotplug_prober(HOTPLUG_INTERVAL)?;
    engine.start_level_meter(LEVEL_INTERVAL)?;
    engine.start_occupancy_collector(HOTPLUG_INTERVAL)?;
    match &config.db_path {
        Some(path) => tracing::info!(db = %path.display(), "opening database"),
        None => tracing::info!("using in-memory database (nothing will persist)"),
    }
    match engine.recordings_dir() {
        Some(dir) => tracing::info!(dir = %dir.display(), "recordings directory"),
        None => tracing::info!("recording disabled (engine has no recordings directory)"),
    }
    match &config.options.token {
        Some(_) => tracing::info!("shared-token auth enabled"),
        None => tracing::info!("no token configured: LAN-trusted, unauthenticated ()"),
    }
    let store = Store::open(config.db_path.as_deref()).map_err(std::io::Error::other)?;
    workspace::adopt_named_devices(&engine, &store);
    let mut state = AppState::new(engine, Arc::new(store));
    state.auth = auth::Auth::new(config.options.token.as_deref());
    state.db_path = config.db_path.clone();
    let (app, background) = router_with_state(state, &config.options);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(%local_addr, "sdr-- server listening");
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(ServerHandle {
        local_addr,
        task,
        _background: background,
    })
}

#[cfg(test)]
mod tests {
    use std::{net::UdpSocket, path::Path, time::Instant};

    use axum::{
        body::{Body, Bytes},
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use sdrmm_wire::{
        AdsbMessage, ApiError, AprsPacket, Bookmark, CapturedImagesResponse, ChannelParams,
        ChannelSettings, ChannelTypesResponse, CreatedId, CreatedRowId, DecodedRecord,
        DecoderEvent, DecoderLogEntry, DecoderLogResponse, DeletedCount, DeviceSettings,
        NetworkExportStatus, NfmParams, NmeaDevicesResponse, PresetInfo, PresetSnapshot,
        RecordingStatus, RecordingsResponse, StateSnapshot, TimeMachineStatus, VoiceCallsResponse,
    };
    use tower::ServiceExt;

    use super::*;

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
                sdrmm_wire::ToolRequest::NanoVna(sdrmm_wire::NanoVnaRequest::ListDevices) => {
                    Ok(sdrmm_wire::ToolResponse::NanoVna(Box::new(
                        sdrmm_wire::NanoVnaResult::Devices {
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
                        },
                    )))
                }
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
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let mut state = AppState::new(Engine::with_registry(registry, None), store);
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

    async fn request(
        app: Router,
        method: &str,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, Bytes) {
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
            "ChatOutputNode",
            "ChatOutputTarget",
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

    #[tokio::test]
    async fn get_state_returns_empty_snapshot() {
        let app = test_router();
        let snap = get_state(&app).await;
        assert!(snap.device_sets.is_empty());
    }

    #[tokio::test]
    async fn nmea_device_catalog_is_available_over_http() {
        let (status, body) =
            request(test_router(), "GET", "/api/position/nmea-devices", None).await;
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
    async fn call_endpoints_list_completed_calls_and_reject_missing_audio() {
        let app = test_router();
        let (status, body) = request(app.clone(), "GET", "/api/calls", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: VoiceCallsResponse = serde_json::from_slice(&body).expect("json");
        assert!(listed.calls.is_empty());

        let (status, body) = request(app, "GET", "/api/calls/99/audio", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");
    }

    #[tokio::test]
    async fn a_plain_dmr_channel_records_every_call_without_any_trunk_system() {
        const RATE: f64 = 240_000.0;
        let dir = tempfile::TempDir::new().expect("temp dir");
        let stem = dir.path().join("dmr");
        let spoken = sdrmm_channels::testgen::dv::dmr::Call::default();
        let one = sdrmm_channels::testgen::dv::dmr::transmission(&spoken, RATE);
        let mut iq = Vec::new();
        for _ in 0..6 {
            iq.extend_from_slice(&one);
        }
        let floor = RATE as usize * 2;
        if iq.len() < floor {
            iq.extend(sdrmm_channels::testgen::silence(floor - iq.len()));
        }
        let mut writer = sdrmm_recorder::SigmfWriter::create(
            &stem,
            RATE,
            145_000_000.0,
            "conventional dmr fixture",
        )
        .expect("create fixture");
        writer.write_block(&iq).expect("write fixture");
        writer.finalize().expect("finalize fixture");

        let app = recording_router(dir.path());
        let mut snapshot =
            virtual_snapshot(&format!("file:{}", stem.display()), &[("dmr", "dmr", "iq")]);
        for node in &mut snapshot.graph.nodes {
            if let sdrmm_wire::NodeBody::Channel(channel) = &mut node.body {
                channel.record_calls = true;
            }
        }
        let workspace = put_active_workspace(&app, &snapshot).await;
        assert_eq!(apply(&app, workspace).await.created, 1);

        let recorded = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let (status, body) = request(app.clone(), "GET", "/api/calls", None).await;
                assert_eq!(status, StatusCode::OK);
                let listed: VoiceCallsResponse = serde_json::from_slice(&body).expect("json");
                if let Some(call) = listed.calls.into_iter().next() {
                    return call;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("a conventional DMR call reaches the call store");

        assert_eq!(recorded.node, "dmr", "the channel node owns the call");
        assert_eq!(recorded.source, Some(spoken.source));
        assert_eq!(recorded.destination, Some(spoken.destination));
        assert_eq!(recorded.group_call, Some(true));
        assert!(!recorded.encrypted);
        let audio = recorded.audio.expect("the call kept its audio");
        let (status, wav) = request(app, "GET", &audio.url, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[tokio::test]
    async fn image_endpoints_list_captures_and_reject_a_missing_picture() {
        let app = test_router();
        let (status, body) = request(app.clone(), "GET", "/api/images", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: CapturedImagesResponse = serde_json::from_slice(&body).expect("json");
        assert!(listed.images.is_empty());

        let (status, body) = request(app, "GET", "/api/images/99/png", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");
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
        let (status, _) =
            request(app, "PATCH", "/api/devicesets/999/channels/1", Some(valid)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn preset_capture_apply_delete_roundtrip() {
        let app = test_router();
        let workspace = store_siggen_workspace(&app).await;
        apply(&app, workspace).await;
        let ds = get_state(&app).await.device_sets[0].id;

        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(r#"{"center_hz":145500000.0,"sample_rate":2400000.0}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let channel = get_state(&app).await.device_sets[0].channels[0].id;
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/channels/{channel}"),
            Some(
                r#"{"offset_hz":25000.0,"squelch_db":-70.0,"params":{"type":"nfm","settings":{}}}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/presets",
            Some(r#"{"name":"2m"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let preset = serde_json::from_slice::<CreatedRowId>(&body)
            .expect("json")
            .id;

        let (status, body) = request(app.clone(), "GET", "/api/presets", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, preset);
        assert_eq!(listed[0].name, "2m");
        assert_eq!(listed[0].devices, 1);

        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(r#"{"center_hz":100000000.0,"sample_rate":2048000.0}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/presets/{preset}/apply"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );

        let set = &get_state(&app).await.device_sets[0];
        assert_eq!(set.settings.center_hz, Some(145_500_000.0));
        assert_eq!(set.settings.sample_rate, Some(2_400_000.0));
        assert_eq!(set.channels.len(), 1);
        assert_eq!(set.channels[0].settings.offset_hz, 25_000.0);
        assert_eq!(set.channels[0].settings.squelch_db, Some(-70.0));

        let (status, _) = request(app.clone(), "POST", "/api/presets/999/apply", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = request(
            app.clone(),
            "DELETE",
            &format!("/api/presets/{preset}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = request(
            app.clone(),
            "DELETE",
            &format!("/api/presets/{preset}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, body) = request(app, "GET", "/api/presets", None).await;
        let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn a_preset_carries_every_radio_the_workspace_draws() {
        let app = test_router();
        let mut snapshot = virtual_snapshot("siggen", &[]);
        snapshot.graph.nodes.push(sdrmm_wire::PatchNode {
            id: "second".to_string(),
            body: sdrmm_wire::NodeBody::Device(sdrmm_wire::DeviceNode {
                device: Some(sdrmm_wire::DeviceRef {
                    backend: "virtual".to_string(),
                    serial: None,
                    key: Some("array4".to_string()),
                }),
            }),
            position: sdrmm_wire::Position { x: 0.0, y: 600.0 },
            size: None,
            label: None,
        });
        let workspace = put_active_workspace(&app, &snapshot).await;
        assert_eq!(apply(&app, workspace).await.opened, 2);

        let tune = async |app: Router, ds: u32, hz: f64| {
            let (status, _) = request(
                app,
                "PATCH",
                &format!("/api/devicesets/{ds}/device"),
                Some(&format!(r#"{{"center_hz":{hz}}}"#)),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        };
        let sets: Vec<u32> = get_state(&app)
            .await
            .device_sets
            .iter()
            .map(|s| s.id)
            .collect();
        tune(app.clone(), sets[0], 145_500_000.0).await;
        tune(app.clone(), sets[1], 433_000_000.0).await;

        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/presets",
            Some(r#"{"name":"the bench"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let preset = serde_json::from_slice::<CreatedRowId>(&body)
            .expect("json")
            .id;
        let (_, body) = request(app.clone(), "GET", "/api/presets", None).await;
        let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
        assert_eq!(listed[0].devices, 2);

        tune(app.clone(), sets[0], 100_000_000.0).await;
        tune(app.clone(), sets[1], 100_000_000.0).await;
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/presets/{preset}/apply"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );

        let state = get_state(&app).await;
        let center = |ds: u32| {
            state
                .device_sets
                .iter()
                .find(|set| set.id == ds)
                .and_then(|set| set.settings.center_hz)
        };
        assert_eq!(center(sets[0]), Some(145_500_000.0));
        assert_eq!(center(sets[1]), Some(433_000_000.0));
    }

    #[tokio::test]
    async fn saving_a_preset_with_no_radio_open_is_refused() {
        let app = test_router();
        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/presets",
            Some(r#"{"name":"empty bench"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
        assert!(err.error.contains("nothing to save"), "{err:?}");

        let (_, body) = request(app, "GET", "/api/presets", None).await;
        let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
        assert!(listed.is_empty());
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

        let (status, body) =
            request(app.clone(), "POST", "/api/devicesets", Some("{not json")).await;
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

    #[tokio::test]
    async fn apply_preset_replaces_channels_that_do_not_fit_the_preset_rate() {
        let (app, store) = test_router_with_store();
        let workspace = put_active_workspace(&app, &virtual_snapshot("siggen", &[])).await;
        apply(&app, workspace).await;
        let ds = get_state(&app).await.device_sets[0].id;
        let (status, _) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{ds}/channels"),
            Some(r#"{"settings":{"offset_hz":900000.0,"params":{"type":"nfm","settings":{}}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let preset = store
            .create_preset("lowrate", &preset_250k(vec![nfm_at(0.0)]))
            .expect("preset");
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/presets/{preset}/apply"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "apply failed: {}",
            String::from_utf8_lossy(&body)
        );

        let snap = get_state(&app).await;
        let set = snap
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("device set");
        assert_eq!(set.settings.sample_rate, Some(250_000.0));
        assert_eq!(set.channels.len(), 1);
        assert_eq!(set.channels[0].settings.offset_hz, 0.0);
    }

    #[tokio::test]
    async fn apply_preset_rejected_up_front_leaves_the_set_untouched() {
        let (app, store) = test_router_with_store();
        let workspace = put_active_workspace(&app, &virtual_snapshot("siggen", &[])).await;
        apply(&app, workspace).await;
        let ds = get_state(&app).await.device_sets[0].id;
        let (status, _) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{ds}/channels"),
            Some(r#"{"settings":{"offset_hz":100000.0,"params":{"type":"nfm","settings":{}}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let preset = store
            .create_preset("broken", &preset_250k(vec![nfm_at(900_000.0)]))
            .expect("preset");
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/presets/{preset}/apply"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
        assert!(
            err.error.contains("exceeds"),
            "the rejection must name the problem: {err:?}"
        );
        assert_eq!(
            err.detail.as_deref(),
            Some("0 of 1 radios in the preset were configured"),
            "nothing was applied to this radio, and the report says which radios were: {err:?}"
        );

        let snap = get_state(&app).await;
        let set = snap
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("device set");
        assert_eq!(set.settings.sample_rate, Some(2_048_000.0));
        assert_eq!(set.channels.len(), 1);
        assert_eq!(set.channels[0].settings.offset_hz, 100_000.0);
    }

    #[tokio::test]
    async fn bookmark_crud_over_http() {
        let app = test_router();
        let (status, body) = request(app.clone(), "GET", "/api/bookmarks", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: Vec<Bookmark> = serde_json::from_slice(&body).expect("json");
        assert!(listed.is_empty());

        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/bookmarks",
            Some(r#"{"label":"tower","freq_hz":118700000.0,"mode":"am","group":"airband"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let id = serde_json::from_slice::<CreatedRowId>(&body)
            .expect("json")
            .id;

        let (_, body) = request(app.clone(), "GET", "/api/bookmarks", None).await;
        let listed: Vec<Bookmark> = serde_json::from_slice(&body).expect("json");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].label, "tower");
        assert_eq!(listed[0].freq_hz, 118_700_000.0);
        assert_eq!(listed[0].mode.as_deref(), Some("am"));
        assert_eq!(listed[0].group.as_deref(), Some("airband"));

        let (status, _) =
            request(app.clone(), "DELETE", &format!("/api/bookmarks/{id}"), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = request(app, "DELETE", &format!("/api/bookmarks/{id}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
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

    #[tokio::test]
    async fn templates_report_the_radios_that_can_run_them() {
        let app = test_router();
        let (status, body) = request(app.clone(), "GET", "/api/templates", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");

        assert!(!listed.templates.is_empty());
        for template in &listed.templates {
            assert!(
                template
                    .supported_devices
                    .contains(&"virtual:siggen".to_string()),
                "{} does not offer the signal generator: {:?}",
                template.id,
                template.supported_devices
            );
        }
    }

    #[tokio::test]
    async fn a_template_the_radio_cannot_run_is_refused_before_anything_is_torn_down() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let ds = create_virtual_set(&app).await;
        record(&app, ds, "start").await;
        wait_for_recorded_samples(&app, ds, 1).await;
        record(&app, ds, "stop").await;

        let rec = list_recordings(&app).await.remove(0);
        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/devicesets",
            Some(&format!(r#"{{"device_id":"{}"}}"#, rec.device_id)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let playback = serde_json::from_slice::<CreatedId>(&body).expect("json").id;

        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/templates/adsb/apply",
            Some(&format!(r#"{{"device_set":{playback}}}"#)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{}",
            String::from_utf8_lossy(&body)
        );

        let set = get_state(&app)
            .await
            .device_sets
            .into_iter()
            .find(|set| set.id == playback)
            .expect("the playback set survived the refusal");
        assert!(set.channels.is_empty());
        assert_eq!(set.settings.center_hz, Some(100_000_000.0));
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

    #[tokio::test]
    async fn playback_transport_pauses_seeks_and_stops() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let rec = recorded(&app).await;
        let ds = playback_set(&app, &rec).await;

        let reported = |app: &Router| {
            let app = app.clone();
            async move {
                get_state(&app)
                    .await
                    .device_sets
                    .into_iter()
                    .find(|set| set.id == ds)
                    .expect("the playback set is listed")
                    .playback
                    .expect("a replaying set reports a transport")
            }
        };

        let initial = reported(&app).await;
        assert!(!initial.paused);
        assert_eq!(initial.total_samples, rec.samples);

        let (status, body) = playback(&app, ds, r#"{"action":"pause"}"#).await;
        assert_eq!(status, StatusCode::OK);
        let paused: sdrmm_wire::PlaybackStatus = serde_json::from_slice(&body).expect("json");
        assert!(paused.paused);
        assert_eq!(reported(&app).await, paused);

        let (status, body) = playback(
            &app,
            ds,
            &format!(
                r#"{{"action":"seek","position_samples":{}}}"#,
                rec.samples / 2
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let sought: sdrmm_wire::PlaybackStatus = serde_json::from_slice(&body).expect("json");
        assert_eq!(sought.position_samples, rec.samples / 2);
        assert_eq!(reported(&app).await, sought);

        let (status, body) = playback(&app, ds, r#"{"action":"stop"}"#).await;
        assert_eq!(status, StatusCode::OK);
        let stopped: sdrmm_wire::PlaybackStatus = serde_json::from_slice(&body).expect("json");
        assert!(stopped.paused);
        assert_eq!(stopped.position_samples, 0);

        let (status, _) = playback(&app, ds, r#"{"action":"play"}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!reported(&app).await.paused);
    }

    #[tokio::test]
    async fn a_radio_has_no_transport_to_drive() {
        let app = test_router();
        let ds = create_virtual_set(&app).await;

        let set = get_state(&app)
            .await
            .device_sets
            .into_iter()
            .find(|set| set.id == ds)
            .expect("set listed");
        assert_eq!(set.playback, None);

        let (status, body) = playback(&app, ds, r#"{"action":"pause"}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            String::from_utf8_lossy(&body).contains("not a recording"),
            "{}",
            String::from_utf8_lossy(&body)
        );

        let (status, _) = playback(&app, 9_999, r#"{"action":"pause"}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
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

    #[tokio::test]
    async fn download_serves_the_pair_as_a_sigmf_archive() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let rec = recorded(&app).await;

        let (status, headers, body) = request_parts(
            app,
            "GET",
            &format!("/api/recordings/{}/download", rec.id),
            None,
            &[],
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(header_value(&headers, "content-type"), "application/x-tar");
        assert_eq!(
            header_value(&headers, "content-disposition"),
            format!("attachment; filename=\"{}.sigmf\"", rec.file)
        );
        assert_eq!(
            header_value(&headers, "content-length"),
            body.len().to_string()
        );
        assert!(
            body.starts_with(format!("{}/", rec.file).as_bytes()),
            "first tar header names the recording's directory"
        );
        assert_eq!(&body[257..263], b"ustar\0");
    }

    #[tokio::test]
    async fn download_serves_iq_as_a_float_wav() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let rec = recorded(&app).await;

        let (status, headers, body) = request_parts(
            app,
            "GET",
            &format!("/api/recordings/{}/download?format=wav", rec.id),
            None,
            &[],
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(header_value(&headers, "content-type"), "audio/wav");
        assert_eq!(
            header_value(&headers, "content-disposition"),
            format!("attachment; filename=\"{}.wav\"", rec.file)
        );
        assert_eq!(
            header_value(&headers, "content-length"),
            body.len().to_string()
        );
        assert_eq!(&body[..4], b"RIFF");
        assert_eq!(&body[8..12], b"WAVE");
        assert_eq!(&body[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes([body[20], body[21]]), 3);
        assert_eq!(u16::from_le_bytes([body[22], body[23]]), 2);
        assert_eq!(
            u32::from_le_bytes([body[24], body[25], body[26], body[27]]),
            2_048_000
        );
        assert_eq!(
            body.len() as u64,
            230 + rec.samples * sdrmm_recorder::BYTES_PER_SAMPLE,
            "header plus every recorded sample"
        );
    }

    #[tokio::test]
    async fn downloads_are_never_compressed() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let rec = recorded(&app).await;

        for format in ["sigmf", "wav"] {
            let (status, headers, body) = request_parts(
                app.clone(),
                "GET",
                &format!("/api/recordings/{}/download?format={format}", rec.id),
                None,
                &[("accept-encoding", "gzip, deflate, br")],
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(header_value(&headers, "content-encoding"), "", "{format}");
            assert_eq!(
                header_value(&headers, "content-length"),
                body.len().to_string(),
                "{format}"
            );
        }

        let (_, headers, _) = request_parts(
            app,
            "GET",
            "/api/state",
            None,
            &[("accept-encoding", "gzip")],
        )
        .await;
        assert_eq!(header_value(&headers, "content-encoding"), "gzip");
    }

    #[tokio::test]
    async fn downloading_an_unknown_recording_or_format_fails_cleanly() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let rec = recorded(&app).await;

        let (status, _) = request(app.clone(), "GET", "/api/recordings/9999/download", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = request(
            app.clone(),
            "GET",
            &format!("/api/recordings/{}/download?format=flac", rec.id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        std::fs::remove_file(sdrmm_recorder::data_path(&dir.path().join(&rec.file)))
            .expect("remove data");
        let (status, _) = request(
            app,
            "GET",
            &format!("/api/recordings/{}/download", rec.id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn record_start_stop_index_and_delete_roundtrip_over_http() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let ds = create_virtual_set(&app).await;

        let (status, body) = record(&app, ds, "start").await;
        assert_eq!(status, StatusCode::OK);
        let live: RecordingStatus = serde_json::from_slice(&body).expect("json");
        assert!(!live.file.is_empty());
        assert_eq!(live.error, None);
        live.started_at.parse::<jiff::Timestamp>().expect("rfc3339");

        wait_for_recorded_samples(&app, ds, 1).await;

        let (status, body) = record(&app, ds, "stop").await;
        assert_eq!(status, StatusCode::OK);
        let done: RecordingStatus = serde_json::from_slice(&body).expect("json");
        assert_eq!(done.file, live.file);
        assert!(done.samples > 0);
        assert_eq!(done.bytes, done.samples * sdrmm_recorder::BYTES_PER_SAMPLE);
        assert_eq!(done.error, None);

        let listed = list_recordings(&app).await;
        assert_eq!(listed.len(), 1);
        let rec = &listed[0];
        assert_eq!(rec.file, done.file);
        assert_eq!(rec.samples, done.samples);
        assert_eq!(rec.sample_rate, 2_048_000.0);
        assert_eq!(rec.center_hz, 100_000_000.0);
        assert_eq!(rec.device_label, "Signal Generator (virtual)");
        assert!(rec.duration_s > 0.0);
        assert_eq!(
            rec.device_id,
            format!("virtual:file:{}", dir.path().join(&rec.file).display())
        );

        let (status, _) = request(
            app.clone(),
            "POST",
            "/api/devicesets",
            Some(&format!(r#"{{"device_id":"{}"}}"#, rec.device_id)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = request(
            app.clone(),
            "DELETE",
            &format!("/api/recordings/{}", rec.id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let stem = dir.path().join(&rec.file);
        assert!(!sdrmm_recorder::meta_path(&stem).exists());
        assert!(!sdrmm_recorder::data_path(&stem).exists());
        assert!(list_recordings(&app).await.is_empty());
        let (status, _) =
            request(app, "DELETE", &format!("/api/recordings/{}", rec.id), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
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

    #[tokio::test]
    async fn channel_audio_record_list_download_and_delete_roundtrip_over_http() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let ds = create_virtual_set(&app).await;
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{ds}/channels"),
            Some(r#"{"settings":{"params":{"type":"nfm","settings":{}}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ch = serde_json::from_slice::<CreatedId>(&body).expect("json").id;

        let (status, body) = record_channel(&app, ds, ch, "start").await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let live: sdrmm_wire::AudioRecordingStatus = serde_json::from_slice(&body).expect("json");
        assert!(live.file.ends_with(".wav"));
        assert_eq!(live.channels, 1);
        live.started_at.parse::<jiff::Timestamp>().expect("rfc3339");

        wait_for_recorded_frames(&app, ds, ch, 4_800).await;

        let (status, body) = record_channel(&app, ds, ch, "stop").await;
        assert_eq!(status, StatusCode::OK);
        let done: sdrmm_wire::AudioRecordingStatus = serde_json::from_slice(&body).expect("json");
        assert_eq!(done.file, live.file);
        assert!(done.frames > 0);
        assert_eq!(done.bytes, done.frames * 2);
        assert_eq!(done.error, None);

        let listed = list_audio_recordings(&app).await;
        assert_eq!(listed.len(), 1);
        let rec = &listed[0];
        assert_eq!(rec.file, done.file);
        assert_eq!((rec.channels, rec.sample_rate), (1, 48_000));
        assert_eq!(rec.frames, done.frames);
        assert!(rec.duration_s > 0.0);
        rec.created_at
            .parse::<jiff::Timestamp>()
            .expect("rfc3339 created_at");

        let (status, wav) = request(
            app.clone(),
            "GET",
            &format!("/api/audiorecordings/{}/download", rec.file),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len() as u64, 44 + rec.bytes);

        let (status, _) = request(
            app.clone(),
            "DELETE",
            &format!("/api/audiorecordings/{}", rec.file),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(list_audio_recordings(&app).await.is_empty());
        let (status, _) = request(
            app.clone(),
            "DELETE",
            &format!("/api/audiorecordings/{}", rec.file),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_audio_recording_name_cannot_reach_outside_its_directory() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("secret.wav"), b"not yours").expect("plant");
        let app = recording_router(dir.path());
        for name in [
            "..%2Fsecret.wav",
            "..",
            "nothing.wav",
            "notes.txt",
            "%2Fetc%2Fpasswd",
        ] {
            let (status, _) = request(
                app.clone(),
                "GET",
                &format!("/api/audiorecordings/{name}/download"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{name} was served");
        }
    }

    #[tokio::test]
    async fn recording_a_channel_that_makes_no_audio_is_refused() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let ds = create_virtual_set(&app).await;
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{ds}/channels"),
            Some(r#"{"settings":{"params":{"type":"adsb","settings":{}}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ch = serde_json::from_slice::<CreatedId>(&body).expect("json").id;

        let (status, body) = record_channel(&app, ds, ch, "start").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(String::from_utf8_lossy(&body).contains("no audio"));

        let (status, _) = record_channel(&app, ds, 9_999, "start").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn network_export_start_stream_and_stop_roundtrip_over_http() {
        let app = test_router();
        let ds = create_virtual_set(&app).await;
        let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
        receiver
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        let address = receiver.local_addr().expect("address");
        let start = serde_json::json!({
            "action": "start",
            "node": "network:test",
            "stream": 0,
            "settings": {
                "transport": "udp",
                "format": "cu8",
                "address": address.to_string(),
            },
        });
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{ds}/network-export"),
            Some(&start.to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let live: NetworkExportStatus = serde_json::from_slice(&body).expect("status");
        assert_eq!(live.node, "network:test");
        assert_eq!(live.settings.address, address.to_string());

        let mut datagram = [0u8; 1_500];
        let received = receiver.recv(&mut datagram).expect("IQ datagram");
        assert!(received > 0);
        assert_eq!(received % 2, 0);

        let stop = serde_json::json!({ "action": "stop", "node": "network:test" });
        let (status, body) = request(
            app,
            "POST",
            &format!("/api/devicesets/{ds}/network-export"),
            Some(&stop.to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let done: NetworkExportStatus = serde_json::from_slice(&body).expect("status");
        assert!(done.samples > 0);
        assert_eq!(done.bytes, done.samples * 2);
        assert!(done.packets > 0);
        assert_eq!(done.error, None);
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

    #[tokio::test]
    async fn channel_baseband_record_roundtrip_lands_in_the_recording_library() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let ds = create_virtual_set(&app).await;
        let ch = create_nfm_channel(&app, ds).await;

        let (status, body) = record_baseband(&app, ds, ch, "start").await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let live: RecordingStatus = serde_json::from_slice(&body).expect("json");
        assert!(live.file.starts_with(&format!("bb_{ds}_{ch}_")));
        live.started_at.parse::<jiff::Timestamp>().expect("rfc3339");

        let (status, body) = record_baseband(&app, ds, ch, "start").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(String::from_utf8_lossy(&body).contains("already recording"));

        wait_for_baseband_samples(&app, ds, ch, 4_800).await;
        let (status, body) = record_baseband(&app, ds, ch, "stop").await;
        assert_eq!(status, StatusCode::OK);
        let done: RecordingStatus = serde_json::from_slice(&body).expect("json");
        assert_eq!(done.file, live.file);
        assert!(done.samples >= 4_800);
        assert_eq!(done.error, None);

        let listed = list_recordings(&app).await;
        let entry = listed
            .iter()
            .find(|entry| entry.file == done.file)
            .expect("the finished baseband pair is in the library");
        assert_eq!(entry.sample_rate, 48_000.0);
        assert_eq!(entry.center_hz, 100_000_000.0);
        assert_eq!(entry.samples, done.samples);

        let (status, _) = record_baseband(&app, ds, ch, "stop").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = record_baseband(&app, ds, 9_999, "start").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn channel_network_export_streams_that_channels_baseband_over_http() {
        let app = test_router();
        let ds = create_virtual_set(&app).await;
        let ch = create_nfm_channel(&app, ds).await;
        let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
        receiver
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        let address = receiver.local_addr().expect("address").to_string();
        let start = serde_json::json!({
            "action": "start",
            "node": "baseband:test",
            "settings": { "transport": "udp", "format": "cf32_le", "address": address },
        });
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{ds}/channels/{ch}/network-export"),
            Some(&start.to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let live: NetworkExportStatus = serde_json::from_slice(&body).expect("status");
        assert_eq!(live.sample_rate, 48_000);
        assert_eq!(live.node, "baseband:test");

        let mut datagram = [0u8; 1_500];
        let received = receiver.recv(&mut datagram).expect("baseband datagram");
        assert!(received > 0 && received.is_multiple_of(8));

        let state = get_state(&app).await;
        let channel = &state.device_sets[0].channels[0];
        assert_eq!(
            channel
                .network_export
                .as_ref()
                .map(|export| export.node.as_str()),
            Some("baseband:test")
        );

        let stop = serde_json::json!({ "action": "stop", "node": "baseband:test" });
        let (status, body) = request(
            app,
            "POST",
            &format!("/api/devicesets/{ds}/channels/{ch}/network-export"),
            Some(&stop.to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let done: NetworkExportStatus = serde_json::from_slice(&body).expect("status");
        assert!(done.samples > 0);
        assert_eq!(done.error, None);
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

    #[tokio::test]
    async fn the_time_machine_arms_captures_and_files_its_window_over_http() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let ds = create_virtual_set(&app).await;
        let node = "time_machine:test";

        let (status, body) = time_machine(
            &app,
            ds,
            serde_json::json!({ "action": "arm", "node": node, "settings": { "history_seconds": 1 } }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let armed: TimeMachineStatus = serde_json::from_slice(&body).expect("status");
        assert_eq!(armed.history_seconds, 1);
        assert_eq!(armed.capacity_samples, 2_048_000);
        assert!(armed.capture.is_none());

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let held = get_state(&app)
                .await
                .device_sets
                .iter()
                .find(|set| set.id == ds)
                .expect("set listed")
                .time_machine
                .clone()
                .expect("armed")
                .held_samples;
            if held >= 1_024_000 {
                break;
            }
            assert!(Instant::now() < deadline, "the window never filled");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let (status, body) = time_machine(
            &app,
            ds,
            serde_json::json!({ "action": "capture", "node": node }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let capturing: TimeMachineStatus = serde_json::from_slice(&body).expect("status");
        let file = capturing.capture.expect("a capture is running").file;

        let (status, body) = time_machine(
            &app,
            ds,
            serde_json::json!({ "action": "stop", "node": node }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let stopped: TimeMachineStatus = serde_json::from_slice(&body).expect("status");
        let written = stopped.capture.expect("the stop reports what it wrote");
        assert!(
            written.samples >= 1_024_000,
            "the past never reached the file"
        );

        let listed = list_recordings(&app).await;
        let entry = listed
            .iter()
            .find(|entry| entry.file == file)
            .expect("the captured window is in the library");
        assert_eq!(entry.sample_rate, 2_048_000.0);

        let (status, _) = time_machine(
            &app,
            ds,
            serde_json::json!({ "action": "disarm", "node": node }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(get_state(&app).await.device_sets[0].time_machine.is_none());

        let (status, body) = time_machine(
            &app,
            ds,
            serde_json::json!({ "action": "capture", "node": node }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(String::from_utf8_lossy(&body).contains("no time machine"));
    }

    #[tokio::test]
    async fn recordings_list_reconciles_planted_files_and_prunes_vanished_ones() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());

        let stem = dir.path().join("planted");
        let block: Vec<num_complex::Complex<f32>> =
            vec![num_complex::Complex::new(0.5, -0.5); 4_800];
        let mut writer =
            sdrmm_recorder::SigmfWriter::create(&stem, 48_000.0, 7_100_000.0, "Foreign HW")
                .expect("writer");
        writer.write_block(&block).expect("write");
        writer.finalize().expect("finalize");
        drop(
            sdrmm_recorder::SigmfWriter::create(&dir.path().join("crashed"), 48_000.0, 1e6, "hw")
                .expect("writer"),
        );

        let listed = list_recordings(&app).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, "planted");
        assert_eq!(listed[0].samples, 4_800);
        assert_eq!(listed[0].duration_s, 0.1);
        assert_eq!(listed[0].device_label, "Foreign HW");
        listed[0]
            .created_at
            .parse::<jiff::Timestamp>()
            .expect("rfc3339");

        std::fs::remove_file(sdrmm_recorder::meta_path(&stem)).expect("remove meta");
        std::fs::remove_file(sdrmm_recorder::data_path(&stem)).expect("remove data");
        assert!(list_recordings(&app).await.is_empty());
    }

    #[tokio::test]
    async fn record_error_mapping_over_http() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());

        let (status, _) = record(&app, 999, "start").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let ds = create_virtual_set(&app).await;
        let (status, body) = record(&app, ds, "stop").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

        let (status, _) = record(&app, ds, "start").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = record(&app, ds, "start").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, body) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(r#"{"sample_rate":2400000.0}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

        let (status, _) = record(&app, ds, "stop").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn recording_endpoints_without_a_recordings_dir() {
        let app = test_router();
        let ds = create_virtual_set(&app).await;

        let (status, body) = record(&app, ds, "start").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

        let (status, _) = record(&app, 999, "start").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        assert!(list_recordings(&app).await.is_empty());
        let (status, _) = request(app, "DELETE", "/api/recordings/1", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
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

        let (status, body) =
            request(app.clone(), "GET", "/api/decoderlog?since=yesterday", None).await;
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

        let (status, body) =
            request(app.clone(), "DELETE", "/api/decoderlog?kind=adsb", None).await;
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

    #[tokio::test]
    async fn occupancy_is_served_and_filtered_by_how_well_observed_it_is() {
        let (app, state) = test_router_with_state();

        let (status, body) = request(app.clone(), "GET", "/api/occupancy", None).await;
        assert_eq!(status, StatusCode::OK);
        let report: sdrmm_wire::OccupancyReport = serde_json::from_slice(&body).expect("json");
        assert!(report.buckets.is_empty());
        assert_eq!(report.bucket_hz, sdrmm_engine::occupancy::BUCKET_HZ);

        {
            let mut occupancy = state
                .engine
                .occupancy()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut db = vec![-100.0f32; 128];
            for round in 0..40 {
                db[64] = -40.0;
                db[96] = if round < 4 { -40.0 } else { -100.0 };
                occupancy.observe(&db, 100e6, 1.6e6, None, 0);
            }
        }

        let (status, body) = request(app.clone(), "GET", "/api/occupancy", None).await;
        assert_eq!(status, StatusCode::OK);
        let report: sdrmm_wire::OccupancyReport = serde_json::from_slice(&body).expect("json");
        assert!(
            report.buckets.len() >= 2,
            "nothing survived the sample floor"
        );
        assert!(
            report.buckets[0].duty >= report.buckets[1].duty,
            "the report is not ordered busiest first"
        );
        assert_eq!(report.buckets[0].by_hour.len(), 24);
        assert!(!report.since.is_empty());

        let (status, body) =
            request(app.clone(), "GET", "/api/occupancy?min_samples=1000", None).await;
        assert_eq!(status, StatusCode::OK);
        let report: sdrmm_wire::OccupancyReport = serde_json::from_slice(&body).expect("json");
        assert!(report.buckets.is_empty());
    }

    #[tokio::test]
    async fn the_band_plan_is_served_per_region() {
        let app = test_router();
        let (status, body) = request(app.clone(), "GET", "/api/bandplan/regions", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: sdrmm_wire::BandRegionsResponse = serde_json::from_slice(&body).expect("json");
        assert!(listed.regions.iter().any(|region| region.id == "de"));
        assert!(
            listed
                .regions
                .iter()
                .any(|region| region.id == listed.default_region)
        );

        let (status, body) = request(app.clone(), "GET", "/api/bandplan/regions/de", None).await;
        assert_eq!(status, StatusCode::OK);
        let plan: sdrmm_wire::BandPlan = serde_json::from_slice(&body).expect("json");
        assert_eq!(plan.region.id, "de");
        assert!(
            plan.layers
                .iter()
                .any(|layer| layer.authority == "Bundesnetzagentur")
        );
        let allocation = &plan.lanes[0];
        assert!(!allocation.overlay);
        let block = allocation
            .blocks
            .iter()
            .find(|block| block.start_hz <= 121_500_000.0 && block.stop_hz > 121_500_000.0)
            .expect("118–137 MHz is allocated");
        let winner = &plan.allocations[block.of as usize];
        assert_eq!(winner.service, sdrmm_wire::BandService::Aeronautical);
        assert_eq!(
            winner.suggested.as_ref().map(ChannelParams::type_id),
            Some("am"),
            "the airband suggests AM, which is what one-click tuning applies"
        );

        let (status, _) = request(app, "GET", "/api/bandplan/regions/atlantis", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn locating_a_region_validates_its_coordinate() {
        let app = test_router();
        let (status, body) = request(
            app.clone(),
            "GET",
            "/api/bandplan/locate?lat=52.52&lon=13.40",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let found: sdrmm_wire::BandRegionMatch = serde_json::from_slice(&body).expect("json");
        assert_eq!(found.region, "de");
        assert!(!found.approximate);

        let (status, _) = request(
            app.clone(),
            "GET",
            "/api/bandplan/locate?lat=91&lon=0",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = request(app, "GET", "/api/bandplan/locate?lat=52.52", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn templates_list_and_apply_over_http() {
        let app = test_router();
        let (status, body) = request(app.clone(), "GET", "/api/templates", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");
        assert!(listed.templates.iter().any(|t| t.id == "fm-radio"));

        let ds = create_virtual_set(&app).await;
        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/templates/fm-radio/apply",
            Some(&format!(r#"{{"device_set":{ds}}}"#)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );
        let set = &get_state(&app).await.device_sets[0];
        assert_eq!(set.settings.center_hz, Some(98_000_000.0));
        assert_eq!(set.channels.len(), 1);
        assert_eq!(set.channels[0].settings.params.type_id(), "wfm");

        let (status, _) = request(
            app.clone(),
            "POST",
            "/api/templates/nope/apply",
            Some(&format!(r#"{{"device_set":{ds}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = request(
            app,
            "POST",
            "/api/templates/fm-radio/apply",
            Some(r#"{"device_set":999}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn every_template_runs_on_the_signal_generator() {
        let app = test_router();
        let ds = create_virtual_set(&app).await;
        let (_, body) = request(app.clone(), "GET", "/api/templates", None).await;
        let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");

        for template in &listed.templates {
            let (status, body) = request(
                app.clone(),
                "POST",
                &format!("/api/templates/{}/apply", template.id),
                Some(&format!(r#"{{"device_set":{ds}}}"#)),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "{}: {}",
                template.id,
                String::from_utf8_lossy(&body)
            );
            let set = &get_state(&app).await.device_sets[0];
            assert_eq!(set.settings.center_hz, Some(template.center_hz));
            assert_eq!(
                set.channels.len(),
                template.channels.len(),
                "{}",
                template.id
            );
        }
    }

    #[tokio::test]
    async fn applying_a_template_merges_its_patch_into_the_active_workspace() {
        let app = test_router();
        let ds = create_virtual_set(&app).await;
        let before = workspaces(&app).await;
        let active = before.active.expect("seeded workspace");
        let nodes_before = before.workspaces[0].nodes;

        for _ in 0..2 {
            let (status, body) = request(
                app.clone(),
                "POST",
                "/api/templates/fm-radio/apply",
                Some(&format!(r#"{{"device_set":{ds}}}"#)),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "{}",
                String::from_utf8_lossy(&body)
            );
        }

        let (status, body) = request(
            app.clone(),
            "GET",
            &format!("/api/workspaces/{active}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let detail: sdrmm_wire::WorkspaceDetail = serde_json::from_slice(&body).expect("json");
        let (_, body) = request(app.clone(), "GET", "/api/templates", None).await;
        let listed: sdrmm_wire::TemplatesResponse = serde_json::from_slice(&body).expect("json");
        let template = listed
            .templates
            .iter()
            .find(|t| t.id == "fm-radio")
            .expect("template");
        let patch = template.patch.as_ref().expect("templates carry a patch");

        let added = u32::try_from(patch.nodes.len()).unwrap();
        assert_eq!(
            u32::try_from(detail.snapshot.graph.nodes.len()).unwrap(),
            nodes_before + added
        );
        let device = detail
            .snapshot
            .graph
            .node("template:fm-radio:dev")
            .expect("the template's receiver");
        let sdrmm_wire::NodeBody::Device(bound) = &device.body else {
            panic!("a receiver node")
        };
        assert_eq!(
            bound.device.as_ref().map(|d| d.backend.as_str()),
            Some("virtual"),
            "the patch names the radio the template was applied to"
        );
        assert_eq!(
            detail
                .snapshot
                .graph
                .channels_of("template:fm-radio:dev")
                .count(),
            1
        );
        detail.snapshot.validate().expect("a valid workspace");
    }

    #[tokio::test]
    async fn applying_a_workspace_opens_its_radio_and_adds_its_channels_once() {
        let app = test_router();
        let snapshot = virtual_snapshot("siggen", &[("nfm", "nfm", "iq"), ("am", "am", "iq")]);
        let workspace = put_active_workspace(&app, &snapshot).await;

        let first = apply(&app, workspace).await;
        assert_eq!(first.opened, 1);
        assert_eq!(first.created, 2);
        assert_eq!(first.bound.len(), 1);
        assert_eq!(first.bound[0].node, "device");
        assert!(first.absent.is_empty());
        assert!(first.refused.is_empty(), "{:?}", first.refused);

        let second = apply(&app, workspace).await;
        assert_eq!(second.opened, 0, "apply is idempotent");
        assert_eq!(second.created, 0);
        assert_eq!(second.bound, first.bound);

        let state = get_state(&app).await;
        assert_eq!(state.device_sets.len(), 1);
        let types: Vec<&str> = state.device_sets[0]
            .channels
            .iter()
            .map(|c| c.settings.params.type_id())
            .collect();
        assert_eq!(types, vec!["nfm", "am"]);
    }

    #[tokio::test]
    async fn applying_a_workspace_reports_an_absent_radio() {
        let app = test_router();
        let mut snapshot = sdrmm_wire::WorkspaceSnapshot::starter();
        let sdrmm_wire::NodeBody::Device(node) = &mut snapshot.graph.nodes[0].body else {
            panic!("the default workspace opens with a receiver")
        };
        node.device = Some(sdrmm_wire::DeviceRef {
            backend: "hackrf".to_string(),
            serial: Some("deadbeef".to_string()),
            key: None,
        });
        let workspace = put_active_workspace(&app, &snapshot).await;

        let report = apply(&app, workspace).await;
        assert_eq!(report.absent, vec!["device".to_string()]);
        assert_eq!(report.opened, 0);
        assert!(report.bound.is_empty());
        assert!(get_state(&app).await.device_sets.is_empty());
    }

    #[tokio::test]
    async fn undoing_a_workspace_takes_the_engine_back_with_it() {
        let app = test_router();
        let workspace =
            put_active_workspace(&app, &virtual_snapshot("siggen", &[("nfm", "nfm", "iq")])).await;
        apply(&app, workspace).await;

        let two = virtual_snapshot("siggen", &[("nfm", "nfm", "iq"), ("am", "am", "iq")]);
        let revision = workspace_detail(&app, workspace).await.info.revision;
        let (status, body) = request(
            app.clone(),
            "PUT",
            &format!("/api/workspaces/{workspace}"),
            Some(&format!(
                r#"{{"revision":{revision},"snapshot":{}}}"#,
                serde_json::to_string(&two).unwrap()
            )),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        apply(&app, workspace).await;
        assert_eq!(channel_types(&app).await, vec!["nfm", "am"]);

        let undone = step(&app, workspace, "undo").await;
        assert_eq!(undone.snapshot.graph.nodes.len(), two.graph.nodes.len() - 1);
        assert!(undone.history.can_undo && undone.history.can_redo);
        assert_eq!(
            channel_types(&app).await,
            vec!["nfm"],
            "the channel the undone step created is closed, not left running"
        );
        assert_eq!(
            workspace_detail(&app, workspace).await.snapshot,
            undone.snapshot
        );

        let redone = step(&app, workspace, "redo").await;
        assert_eq!(redone.snapshot, two);
        assert!(!redone.history.can_redo);
        assert_eq!(channel_types(&app).await, vec!["nfm", "am"]);

        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/workspaces/{workspace}/redo"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");
    }

    async fn workspace_detail(app: &Router, id: i64) -> sdrmm_wire::WorkspaceDetail {
        let (status, body) =
            request(app.clone(), "GET", &format!("/api/workspaces/{id}"), None).await;
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

    #[tokio::test]
    async fn the_patch_catalog_describes_the_node_palette() {
        let app = test_router();
        let (status, body) = request(app, "GET", "/api/patch/catalog", None).await;
        assert_eq!(status, StatusCode::OK);
        let catalog: sdrmm_wire::PatchCatalog = serde_json::from_slice(&body).expect("json");
        assert_eq!(catalog, sdrmm_wire::PatchCatalog::build());
        let device = catalog
            .nodes
            .iter()
            .find(|n| n.kind == "device")
            .expect("a device in the palette");
        assert_eq!(device.category, sdrmm_wire::NodeCategory::Source);
        let port = |name: &str| {
            device
                .ports
                .iter()
                .find(|port| port.name == name)
                .unwrap_or_else(|| panic!("the device node has a {name} port"))
        };
        assert!(port("iq").multi, "one radio feeds many nodes");
        assert!(!port("control").multi, "one sweep owns a radio");
        assert!(port("tx").note.is_some(), "the reserved port says why");
    }

    #[tokio::test]
    async fn workspace_crud_over_http() {
        let app = test_router();
        let seeded = workspaces(&app).await;
        let workspace = seeded.workspaces[0].id;
        assert_eq!(seeded.active, Some(workspace));

        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/workspaces",
            Some(r#"{"name":"Bench"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let created: sdrmm_wire::CreatedRowId = serde_json::from_slice(&body).expect("json");

        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/workspaces",
            Some(r#"{"name":"Bench"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

        let (status, _) = request(
            app.clone(),
            "POST",
            &format!("/api/workspaces/{}/activate", created.id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(workspaces(&app).await.active, Some(created.id));

        let (status, body) = request(
            app.clone(),
            "PUT",
            &format!("/api/workspaces/{}", created.id),
            Some(r#"{"revision":1,"snapshot":{"version":1,"graph":{"nodes":[]}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

        let (status, body) = request(
            app.clone(),
            "PUT",
            &format!("/api/workspaces/{}", created.id),
            Some(
                r#"{"revision":1,"snapshot":{"version":2,"graph":{"nodes":[],"edges":[
                   {"from":{"node":"a","port":"iq"},"to":{"node":"b","port":"iq"}}]}}}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

        let snapshot =
            serde_json::to_string(&sdrmm_wire::WorkspaceSnapshot::starter()).expect("snapshot");
        let (status, body) = request(
            app.clone(),
            "PUT",
            &format!("/api/workspaces/{}", created.id),
            Some(&format!(r#"{{"revision":1,"snapshot":{snapshot}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let info: sdrmm_wire::WorkspaceInfo = serde_json::from_slice(&body).expect("json");
        assert_eq!(info.revision, 2);

        let (status, _) = request(
            app.clone(),
            "PUT",
            &format!("/api/workspaces/{}", created.id),
            Some(&format!(r#"{{"revision":1,"snapshot":{snapshot}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, _) = request(
            app.clone(),
            "DELETE",
            &format!("/api/workspaces/{}", created.id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let after = workspaces(&app).await;
        assert_eq!(after.workspaces.len(), 1);
        assert_eq!(
            after.active,
            Some(workspace),
            "deleting the active one promotes"
        );

        let (status, _) = request(
            app.clone(),
            "GET",
            &format!("/api/workspaces/{}", created.id),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
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
        let workspace = workspaces(app).await.active.expect("seeded workspace");
        let (status, body) = request(
            app.clone(),
            "PUT",
            &format!("/api/workspaces/{workspace}"),
            Some(&format!(
                r#"{{"revision":1,"snapshot":{}}}"#,
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

    #[tokio::test]
    async fn a_workspace_comes_back_tuned_the_way_it_was_left() {
        let (app, state) = test_router_with_state();
        let workspace = store_siggen_workspace(&app).await;
        assert_eq!(apply(&app, workspace).await.created, 1);

        let ds = get_state(&app).await.device_sets[0].id;
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(r#"{"center_hz":145500000.0}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let channel = get_state(&app).await.device_sets[0].channels[0].id;
        let (status, body) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/channels/{channel}"),
            Some(
                r#"{"offset_hz":12500.0,"squelch_db":-42.0,"params":{"type":"nfm","settings":{}}}"#,
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );

        workspace::save_active(&state).expect("capture the workspace");

        let restarted = state_over(state.store.clone());
        let (app, background) = router_with_state(restarted, &ServerOptions::default());
        background.detach();
        assert!(get_state(&app).await.device_sets.is_empty());

        let report = apply(&app, workspace).await;
        assert_eq!(report.opened, 1);
        assert_eq!(report.created, 1);
        assert!(report.refused.is_empty(), "{:?}", report.refused);

        let set = &get_state(&app).await.device_sets[0];
        assert_eq!(set.settings.center_hz, Some(145_500_000.0));
        assert_eq!(set.channels.len(), 1, "no duplicate channel on restore");
        assert_eq!(set.channels[0].settings.offset_hz, 12_500.0);
        assert_eq!(set.channels[0].settings.squelch_db, Some(-42.0));
    }

    #[tokio::test]
    async fn a_hand_picked_radio_comes_up_with_the_nodes_stored_settings() {
        let (app, state) = test_router_with_state();
        let workspace = store_siggen_workspace(&app).await;
        apply(&app, workspace).await;
        let ds = get_state(&app).await.device_sets[0].id;
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(r#"{"center_hz":145500000.0}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        workspace::save_active(&state).expect("capture the workspace");

        let restarted = state_over(state.store.clone());
        let (app, background) = router_with_state(restarted, &ServerOptions::default());
        background.detach();
        create_virtual_set(&app).await;
        apply(&app, workspace).await;
        assert_eq!(
            get_state(&app).await.device_sets[0].settings.center_hz,
            Some(145_500_000.0)
        );
    }

    #[tokio::test]
    async fn a_refused_restore_does_not_let_the_autosave_overwrite_the_stored_settings() {
        let (app, state) = test_router_with_state();
        let workspace = store_siggen_workspace(&app).await;
        let planted = sdrmm_wire::WorkspaceState {
            trunks: Vec::new(),
            version: sdrmm_wire::WORKSPACE_STATE_VERSION,
            devices: vec![sdrmm_wire::WorkspaceDevice {
                node: "device".to_string(),
                settings: DeviceSettings {
                    center_hz: Some(145_500_000.0),
                    sample_rate: Some(999.0),
                    ..DeviceSettings::default()
                },
                channels: Vec::new(),
            }],
        };
        state
            .store
            .put_workspace_state(workspace, &planted)
            .expect("plant the stored settings");

        let report = apply(&app, workspace).await;
        assert_eq!(report.refused.len(), 1, "{report:?}");
        assert_eq!(report.refused[0].node, "device");

        workspace::save_active(&state).expect("capture the workspace");
        let stored = state
            .store
            .workspace_state(workspace)
            .expect("read the stored settings");
        assert_eq!(
            stored.device("device").expect("kept").settings.sample_rate,
            Some(999.0),
            "the workspace keeps what it had until a restore succeeds"
        );
    }

    #[tokio::test]
    async fn applying_a_workspace_does_not_retune_an_open_radio() {
        let (app, state) = test_router_with_state();
        let workspace = store_siggen_workspace(&app).await;
        apply(&app, workspace).await;

        let ds = get_state(&app).await.device_sets[0].id;
        let tune = async |hz: f64| {
            let (status, _) = request(
                app.clone(),
                "PATCH",
                &format!("/api/devicesets/{ds}/device"),
                Some(&format!(r#"{{"center_hz":{hz}}}"#)),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        };

        tune(145_500_000.0).await;
        workspace::save_active(&state).expect("capture the workspace");
        tune(433_800_000.0).await;

        apply(&app, workspace).await;
        assert_eq!(
            get_state(&app).await.device_sets[0].settings.center_hz,
            Some(433_800_000.0)
        );
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

    #[tokio::test]
    async fn switching_workspaces_closes_the_radios_the_new_one_does_not_name() {
        let (app, _state) = test_router_with_state();
        let workspace = store_siggen_workspace(&app).await;
        apply(&app, workspace).await;
        assert_eq!(get_state(&app).await.device_sets.len(), 1);

        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/workspaces",
            Some(r#"{"name":"Empty"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let created: sdrmm_wire::CreatedRowId = serde_json::from_slice(&body).expect("json");
        activate(&app, created.id).await;

        assert!(
            get_state(&app).await.device_sets.is_empty(),
            "the radio the previous workspace opened is still running"
        );
    }

    #[tokio::test]
    async fn switching_between_workspaces_sharing_a_radio_restores_each_ones_settings() {
        let (app, _state) = test_router_with_state();
        let first = store_siggen_workspace(&app).await;
        apply(&app, first).await;
        let ds = get_state(&app).await.device_sets[0].id;

        let tune = async |hz: f64| {
            let (status, _) = request(
                app.clone(),
                "PATCH",
                &format!("/api/devicesets/{ds}/device"),
                Some(&format!(r#"{{"center_hz":{hz}}}"#)),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        };
        tune(145_500_000.0).await;

        let second = store_second_workspace(&app, "Marine", "am").await;
        activate(&app, second).await;

        let sets = get_state(&app).await.device_sets;
        assert_eq!(sets.len(), 1, "the shared radio was closed and reopened");
        assert_eq!(sets[0].id, ds, "the shared radio was closed and reopened");
        assert!(
            sets[0].channels.is_empty(),
            "the previous workspace's channel is still running"
        );

        apply(&app, second).await;
        tune(162_000_000.0).await;
        let sets = get_state(&app).await.device_sets;
        assert_eq!(sets[0].channels.len(), 1);
        assert_eq!(sets[0].channels[0].settings.params.type_id(), "am");

        activate(&app, first).await;
        let sets = get_state(&app).await.device_sets;
        assert_eq!(
            sets[0].settings.center_hz,
            Some(145_500_000.0),
            "the first workspace came back on the second one's frequency"
        );
        assert!(
            sets[0].channels.is_empty(),
            "the second workspace's channel is still running"
        );

        apply(&app, first).await;
        let sets = get_state(&app).await.device_sets;
        assert_eq!(sets[0].channels.len(), 1);
        assert_eq!(sets[0].channels[0].settings.params.type_id(), "nfm");
        assert_eq!(sets[0].settings.center_hz, Some(145_500_000.0));
    }

    #[tokio::test]
    async fn a_capture_without_the_radio_keeps_its_stored_settings() {
        let (app, state) = test_router_with_state();
        let workspace = store_siggen_workspace(&app).await;
        apply(&app, workspace).await;
        let ds = get_state(&app).await.device_sets[0].id;
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(r#"{"center_hz":145500000.0}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        workspace::save_active(&state).expect("capture the workspace");

        let (status, _) = request(
            app.clone(),
            "DELETE",
            &format!("/api/devicesets/{ds}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        workspace::save_active(&state).expect("capture the empty workspace");

        let stored = state
            .store
            .workspace_state(workspace)
            .expect("workspace state");
        assert_eq!(
            stored
                .device("device")
                .expect("device entry")
                .settings
                .center_hz,
            Some(145_500_000.0)
        );
    }

    #[tokio::test]
    async fn a_workspace_remembers_per_stream_overrides() {
        let (app, state) = test_router_with_state();
        let workspace = put_active_workspace(&app, &virtual_snapshot("transceiver", &[])).await;
        apply(&app, workspace).await;
        let ds = get_state(&app).await.device_sets[0].id;
        let (status, body) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/device"),
            Some(r#"{"streams":[{"stream":1,"center_hz":433920000.0}]}"#),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{}",
            String::from_utf8_lossy(&body)
        );
        workspace::save_active(&state).expect("capture the workspace");

        let restarted = state_over(state.store.clone());
        let (app, background) = router_with_state(restarted, &ServerOptions::default());
        background.detach();
        let report = apply(&app, workspace).await;
        assert!(report.refused.is_empty(), "{:?}", report.refused);

        let set = &get_state(&app).await.device_sets[0];
        assert_eq!(set.settings.streams.len(), 1, "{:?}", set.settings.streams);
        assert_eq!(set.settings.streams[0].stream, 1);
        assert_eq!(set.settings.streams[0].center_hz, Some(433_920_000.0));
    }

    #[tokio::test]
    async fn applying_a_workspace_lands_each_channel_on_the_stream_its_wire_names() {
        let app = test_router();
        let taps = [("low", "nfm", "iq"), ("high", "nfm", "iq4")];
        let workspace = put_active_workspace(&app, &virtual_snapshot("array4", &taps)).await;

        let report = apply(&app, workspace).await;
        assert_eq!(report.created, 2);
        assert!(report.refused.is_empty(), "{:?}", report.refused);
        let streams: Vec<u32> = get_state(&app).await.device_sets[0]
            .channels
            .iter()
            .map(|channel| channel.stream)
            .collect();
        assert_eq!(streams, vec![0, 3], "the iq4 wire must land on stream 3");

        let second = apply(&app, workspace).await;
        assert_eq!(
            second.created, 0,
            "apply duplicated a channel across streams"
        );
    }

    #[tokio::test]
    async fn a_wire_to_a_stream_the_radio_does_not_have_is_refused_not_moved() {
        let app = test_router();
        let taps = [("voice", "nfm", "iq3")];
        let workspace = put_active_workspace(&app, &virtual_snapshot("siggen", &taps)).await;

        let report = apply(&app, workspace).await;
        assert_eq!(report.opened, 1, "the radio itself is fine and must open");
        assert_eq!(report.created, 0);
        assert_eq!(report.refused.len(), 1, "{:?}", report.refused);
        assert_eq!(report.refused[0].node, "voice");
        assert!(
            report.refused[0].reason.contains("1 rx streams"),
            "the refusal must name the count: {}",
            report.refused[0].reason
        );
        assert!(
            get_state(&app).await.device_sets[0].channels.is_empty(),
            "the channel must not come up on another stream"
        );
    }

    #[tokio::test]
    async fn capture_and_restore_pair_same_type_channels_by_stream() {
        let (app, state) = test_router_with_state();
        let taps = [("low", "nfm", "iq"), ("high", "nfm", "iq4")];
        let workspace = put_active_workspace(&app, &virtual_snapshot("array4", &taps)).await;
        apply(&app, workspace).await;

        let set = &get_state(&app).await.device_sets[0];
        let offset_for = |stream: u32| if stream == 0 { 11_000.0 } else { 33_000.0 };
        for channel in &set.channels {
            let (status, body) = request(
                app.clone(),
                "PATCH",
                &format!("/api/devicesets/{}/channels/{}", set.id, channel.id),
                Some(&format!(
                    r#"{{"offset_hz":{},"params":{{"type":"nfm","settings":{{}}}}}}"#,
                    offset_for(channel.stream)
                )),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "{}",
                String::from_utf8_lossy(&body)
            );
        }
        workspace::save_active(&state).expect("capture the workspace");

        let restarted = state_over(state.store.clone());
        let (app, background) = router_with_state(restarted, &ServerOptions::default());
        background.detach();
        let report = apply(&app, workspace).await;
        assert!(report.refused.is_empty(), "{:?}", report.refused);

        let set = &get_state(&app).await.device_sets[0];
        let streams: Vec<u32> = set.channels.iter().map(|channel| channel.stream).collect();
        assert_eq!(streams, vec![0, 3]);
        for channel in &set.channels {
            assert_eq!(
                channel.settings.offset_hz,
                offset_for(channel.stream),
                "stream {} came back with the other lane's settings",
                channel.stream
            );
        }
    }

    #[tokio::test]
    async fn scanner_start_stop_and_error_mapping_over_http() {
        let app = test_router();
        let ds = create_virtual_set(&app).await;

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

        let start = r#"{"action":"start","settings":{"ranges":[{"start_hz":99000000.0,"stop_hz":101000000.0,"step_hz":100000.0}],"threshold_db":100.0,"dwell_ms":40}}"#;
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{ds}/scanner"),
            Some(start),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let status_body: sdrmm_wire::ScannerStatus = serde_json::from_slice(&body).expect("json");
        assert_eq!(status_body.targets, 21);
        assert!(get_state(&app).await.device_sets[0].scanner.is_some());

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
    async fn tools_lists_what_this_build_offers() {
        let (status, body) = request(test_router(), "GET", "/api/tools", None).await;
        assert_eq!(status, StatusCode::OK);
        let tools: sdrmm_wire::ToolsResponse = serde_json::from_slice(&body).expect("json");
        let antenna = tools
            .tools
            .iter()
            .find(|tool| tool.id == sdrmm_wire::ANTENNA_TOOL_ID)
            .expect("the antenna calculator is a builtin");
        assert!(!antenna.needs_hardware);
        assert!(!antenna.summary.is_empty());
        let nanovna = tools
            .tools
            .iter()
            .find(|tool| tool.id == sdrmm_wire::NANOVNA_TOOL_ID)
            .expect("the NanoVNA instrument is a builtin");
        assert!(nanovna.needs_hardware);
        assert_eq!(nanovna.category, sdrmm_wire::ToolCategory::Instrument);
    }

    #[tokio::test]
    async fn nanovna_device_discovery_uses_the_tool_handler() {
        let (status, body) = request(
            test_router(),
            "POST",
            "/api/tools/run",
            Some(r#"{"tool":"nanovna","request":{"action":"list_devices"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let response: sdrmm_wire::ToolResponse = serde_json::from_slice(&body).expect("json");
        let sdrmm_wire::ToolResponse::NanoVna(result) = response else {
            panic!("a NanoVNA call must answer under the NanoVNA tag");
        };
        let sdrmm_wire::NanoVnaResult::Devices {
            devices,
            ignored_ports,
        } = *result
        else {
            panic!("NanoVNA discovery must return the device result");
        };
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].port, "fixture-port");
        assert_eq!(devices[0].match_kind, sdrmm_wire::NanoVnaMatch::Confirmed);
        assert_eq!(ignored_ports, vec!["fixture-gnss".to_owned()]);
    }

    #[tokio::test]
    async fn a_tool_call_answers_under_the_tag_it_was_asked_with() {
        let (status, body) = request(
            test_router(),
            "POST",
            "/api/tools/run",
            Some(
                r#"{"tool":"antenna","request":{"frequency_hz":145500000.0,
                    "design":{"type":"yagi","settings":{"directors":3,
                    "spacing_wavelengths":0.2}}}}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let response: sdrmm_wire::ToolResponse = serde_json::from_slice(&body).expect("json");
        assert_eq!(response.tool_id(), sdrmm_wire::ANTENNA_TOOL_ID);
        let sdrmm_wire::ToolResponse::Antenna(report) = response else {
            panic!("an antenna request is answered by the antenna tool");
        };
        assert_eq!(report.frequency_hz, 145_500_000.0);
        assert!(
            report
                .parts
                .iter()
                .any(|part| part.name == "Director 3" && part.position_m.is_some())
        );
    }

    #[tokio::test]
    async fn a_tool_refusal_is_a_typed_bad_request() {
        let (status, body) = request(
            test_router(),
            "POST",
            "/api/tools/run",
            Some(r#"{"tool":"antenna","request":{"frequency_hz":0.0,"design":{"type":"dipole"}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let error: ApiError = serde_json::from_slice(&body).expect("json");
        assert!(error.error.contains("frequency_hz"), "{}", error.error);
    }

    #[tokio::test]
    async fn an_unknown_tool_tag_is_refused_in_the_error_shape() {
        let (status, body) = request(
            test_router(),
            "POST",
            "/api/tools/run",
            Some(r#"{"tool":"nanovna","request":{}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let error: ApiError = serde_json::from_slice(&body).expect("json");
        assert_eq!(error.error, "invalid request body");
    }

    #[tokio::test]
    async fn about_serves_the_notices_and_their_texts() {
        let app = test_router();
        let (status, body) = request(app.clone(), "GET", "/api/about", None).await;
        assert_eq!(status, StatusCode::OK);
        let about: sdrmm_wire::AboutResponse = serde_json::from_slice(&body).expect("json");
        assert_eq!(about.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(about.license, "GPL-3.0-or-later");

        let component = about
            .components
            .iter()
            .find(|component| !component.texts.is_empty())
            .expect("some component ships a license text");
        let id = &component.texts[0];
        let (status, body) = request(
            app.clone(),
            "GET",
            &format!("/api/about/licenses/{id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text: sdrmm_wire::LicenseTextResponse = serde_json::from_slice(&body).expect("json");
        assert_eq!(&text.id, id);
        assert!(!text.text.is_empty());

        let (status, _) = request(app, "GET", "/api/about/licenses/nosuchtext", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn doctor_reports_the_running_configuration() {
        let (status, body) = request(test_router(), "GET", "/api/doctor", None).await;
        assert_eq!(status, StatusCode::OK);
        let report: sdrmm_wire::DoctorReport = serde_json::from_slice(&body).expect("json");
        assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
        let backends = report
            .checks
            .iter()
            .find(|c| c.id == "backends")
            .expect("backends check");
        assert_eq!(backends.status, sdrmm_wire::CheckStatus::Warn);
        assert!(backends.detail.contains("virtual"));
        assert!(report.checks.iter().any(|c| c.id == "storage.db"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn delete_recording_never_404s_against_concurrent_reconciles() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let app = recording_router(dir.path());
        let block: Vec<num_complex::Complex<f32>> = vec![num_complex::Complex::new(0.5, -0.5); 64];
        for i in 0..10 {
            let file = format!("planted_{i}");
            let mut writer =
                sdrmm_recorder::SigmfWriter::create(&dir.path().join(&file), 48_000.0, 1e6, "hw")
                    .expect("writer");
            writer.write_block(&block).expect("write");
            writer.finalize().expect("finalize");

            let listed = list_recordings(&app).await;
            let id = listed.iter().find(|r| r.file == file).expect("indexed").id;
            let delete = {
                let app = app.clone();
                tokio::spawn(async move {
                    request(app, "DELETE", &format!("/api/recordings/{id}"), None).await
                })
            };
            let lists: Vec<_> = (0..3)
                .map(|_| {
                    let app = app.clone();
                    tokio::spawn(async move {
                        request(app, "GET", "/api/recordings", None).await;
                    })
                })
                .collect();
            let (status, body) = delete.await.expect("join");
            assert_eq!(
                status,
                StatusCode::NO_CONTENT,
                "iteration {i}: {}",
                String::from_utf8_lossy(&body)
            );
            for list in lists {
                list.await.expect("join");
            }
            assert!(
                !list_recordings(&app).await.iter().any(|r| r.file == file),
                "iteration {i}: deleted recording resurfaced"
            );
        }
    }
}
