use std::{
    collections::HashSet,
    net::SocketAddr,
    path::{Path, PathBuf},
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

pub const HOTPLUG_INTERVAL: Duration = Duration::from_secs(5);
pub const LEVEL_INTERVAL: Duration = Duration::from_millis(100);

const DECODED_TEXT_CAP: usize = 1024;

mod array;
mod assets;
mod audio_fx;
mod auth;
mod bandplan;
mod calibration;
mod calls;
pub(crate) mod cps;
mod decoded;
mod decoderlog;
pub mod denoise_models;
pub(crate) mod df_fusion;
pub mod diagnostics;
pub mod doctor;
mod event_output;
mod events;
mod fusion_routes;
mod gps;
mod health;
mod images;
mod ionosonde;
mod json;
mod mcp;
mod missions;
mod monitor;
mod net;
pub mod notices;
mod packed;
pub mod phones;
mod placement;
mod radar;
mod reconcile;
mod recorders;
mod remote;
mod rest;
mod satellites;
mod store;
mod surfaces;
mod survey;
mod templates;
pub mod tls;
mod tracks;
mod trunking;
mod workspace;
mod ws;

pub use remote::device_name;
pub use store::{RemotePairing, Store, StoreError};

pub trait NativeShell: Send + Sync + std::fmt::Debug {
    fn reveal(&self, path: &Path) -> std::io::Result<()>;
}

#[derive(Clone, Debug, Default)]
pub struct ServerOptions {
    pub dev_cors: bool,
    pub token: Option<String>,
    pub shell: Option<Arc<dyn NativeShell>>,
    pub remote_app: Option<url::Url>,
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
    pub(crate) decoded: decoded::Feed,
    pub(crate) tracks: Arc<tracks::Tracks>,
    pub(crate) calls: Arc<calls::Calls>,
    pub(crate) images: Arc<images::Images>,
    pub(crate) ionosonde: Arc<ionosonde::Ionosonde>,
    pub clients: Arc<std::sync::atomic::AtomicU32>,
    pub(crate) tools: Arc<sdrmm_tools::ToolRegistry>,
    pub(crate) unrestored: Arc<std::sync::Mutex<Vec<String>>>,
    pub(crate) restored: Arc<std::sync::Mutex<HashSet<(i64, String, u32)>>>,
    pub(crate) gps: Arc<gps::GpsHub>,
    pub(crate) satellites: Arc<satellites::SatelliteHub>,
    pub(crate) cps: Arc<cps::CpsHub>,
    pub(crate) fusion: df_fusion::SharedFusion,
    pub(crate) shell: Option<Arc<dyn NativeShell>>,
    pub(crate) arrays: Arc<array::ArrayHub>,
    pub(crate) radar: Arc<radar::RadarHub>,
    pub(crate) surfaces: Arc<surfaces::SurfaceHub>,
    pub(crate) survey: Arc<survey::SurveyHub>,
    pub(crate) phones: Arc<phones::Phones>,
    pub(crate) gate: Arc<phones::gate::PhoneGate>,
    pub(crate) server_id: Arc<str>,
    pub(crate) server_name: Arc<str>,
    pub(crate) dev_cors: bool,
    pub(crate) data_dir: Option<PathBuf>,
    pub(crate) remote: Arc<remote::RemoteHub>,
    pub(crate) denoise: Arc<denoise_models::Downloads>,
    started: std::time::Instant,
}

impl AppState {
    fn new(engine: Arc<Engine>, store: Arc<Store>) -> Self {
        let server_id = store.server_id();
        let phones = Arc::new(phones::Phones::new(store.clone()));
        Self {
            engine,
            remote: Arc::new(remote::RemoteHub::new(None, store.clone(), health::idle())),
            store,
            auth: auth::Auth::default(),
            db_path: None,
            recordings_gate: Arc::new(std::sync::Mutex::new(())),
            apply_gate: Arc::new(std::sync::Mutex::new(())),
            decoder_log_dropped: Arc::new(AtomicU64::new(0)),
            decoded_text: tokio::sync::broadcast::channel(DECODED_TEXT_CAP).0,
            decoded: decoded::feed(),
            tracks: Arc::new(tracks::Tracks::default()),
            calls: Arc::new(calls::Calls::default()),
            images: Arc::new(images::Images::default()),
            ionosonde: Arc::new(ionosonde::Ionosonde::default()),
            clients: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            tools: Arc::new(sdrmm_tools::ToolRegistry::with_builtins()),
            unrestored: Arc::new(std::sync::Mutex::new(Vec::new())),
            restored: Arc::new(std::sync::Mutex::new(HashSet::new())),
            gps: Arc::new(gps::GpsHub::default()),
            satellites: Arc::new(satellites::SatelliteHub::default()),
            cps: Arc::new(cps::CpsHub::default()),
            fusion: Arc::new(df_fusion::FusionHub::default()),
            shell: None,
            arrays: Arc::default(),
            radar: Arc::default(),
            surfaces: Arc::default(),
            survey: Arc::default(),
            phones,
            gate: Arc::default(),
            server_id,
            server_name: net::host_label().into(),
            dev_cors: false,
            data_dir: None,
            denoise: Arc::default(),
            started: std::time::Instant::now(),
        }
    }

    pub(crate) fn uptime(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    pub fn decoder_log_dropped(&self) -> u64 {
        self.decoder_log_dropped.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub db_path: Option<PathBuf>,
    pub tls: Option<tls::Tls>,
    pub options: ServerOptions,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            db_path: None,
            tls: None,
            options: ServerOptions::default(),
        }
    }
}

#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    let (_router, api) = rest::openapi_router().split_for_parts();
    api
}

fn openapi_route(api: &utoipa::openapi::OpenApi) -> axum::routing::MethodRouter<AppState> {
    let spec = serde_json::to_vec(api)
        .map(axum::body::Bytes::from)
        .map_err(|error| error.to_string());
    axum::routing::get(move || {
        let spec = spec.clone();
        async move {
            use axum::response::IntoResponse;
            match spec {
                Ok(body) => (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    body,
                )
                    .into_response(),
                Err(error) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(sdrmm_wire::ApiError {
                        error: "OpenAPI document failed to serialize".to_string(),
                        detail: Some(error),
                        code: None,
                    }),
                )
                    .into_response(),
            }
        }
    })
}

fn configure(state: &mut AppState, options: &ServerOptions) -> health::Reporter {
    state.auth = auth::Auth::new(options.token.as_deref());
    state.shell = options.shell.clone();
    state.dev_cors = options.dev_cors;
    let health = health::Reporter::new(state);
    health.refresh();
    state.remote = Arc::new(remote::RemoteHub::new(
        options.remote_app.as_ref(),
        state.store.clone(),
        health.subscribe(),
    ));
    if let Some(token) = &options.token {
        diagnostics::hide_secret(token);
    }
    health
}

#[cfg(test)]
fn router_with_state(state: AppState, options: &ServerOptions) -> (Router, Background) {
    main_router(state, options, false)
}

#[cfg(test)]
fn main_router(mut state: AppState, options: &ServerOptions, tls: bool) -> (Router, Background) {
    let health = configure(&mut state, options);
    let background = start_runtime(&state, health);
    let app = app(&state, auth::ListenerRole::Main, tls);
    state.remote.attach(app.clone());
    (app, background)
}

fn app(state: &AppState, role: auth::ListenerRole, tls: bool) -> Router {
    let (api_router, api) = rest::openapi_router().split_for_parts();
    let mut routes = Router::new()
        .merge(api_router)
        .route("/api/ws", axum::routing::get(ws::handler));
    if role == auth::ListenerRole::Main {
        routes = routes
            .merge(mcp::router(
                state.engine.clone(),
                state.store.clone(),
                state.tools.clone(),
            ))
            .route("/api/openapi.json", openapi_route(&api))
            .route("/api/docs", axum::routing::get(assets::api_docs))
            .route("/api/docs/", axum::routing::get(assets::api_docs));
    }
    routes = routes.route_layer(axum::middleware::from_fn_with_state(
        auth::AuthGate::new(state, role, tls),
        auth::authenticate,
    ));
    if role == auth::ListenerRole::Main {
        routes = routes.fallback(assets::static_handler);
    }
    let mut app = routes.with_state(state.clone()).layer(
        tower_http::compression::CompressionLayer::new().compress_when(
            tower_http::compression::predicate::DefaultPredicate::new()
                .and(NotForContentType::const_new("application/x-tar"))
                .and(NotForContentType::const_new("audio/wav")),
        ),
    );
    if state.dev_cors && role == auth::ListenerRole::Main {
        app = app.layer(CorsLayer::very_permissive());
    }
    app
}

fn start_runtime(state: &AppState, health: health::Reporter) -> Background {
    let background = start_background(state, health);
    ws::start_decoded_encoder(state);
    workspace::spawn_autosave(state);
    placement::spawn_settling(state);
    state.gps.reconcile(state);
    state.satellites.reconcile(state);
    array::start_pump(state);
    radar::start(state);
    df_fusion::start(state);
    fusion_routes::start(state);
    survey::start(state);
    missions::watch::spawn(state);
    state.gps.spawn_watchdog(state);
    phones::spawn_flusher(state);
    background
}

struct Background {
    tasks: Vec<BackgroundTask>,
    remote: Arc<remote::RemoteHub>,
    detached: bool,
}

enum BackgroundTask {
    Task(tokio::task::JoinHandle<()>),
    Owned,
}

#[cfg(test)]
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
        self.remote.shutdown();
        for task in &self.tasks {
            if let BackgroundTask::Task(task) = task {
                task.abort();
            }
        }
    }
}

fn start_background(state: &AppState, health: health::Reporter) -> Background {
    let (recording_tx, recording_rx) = tokio::sync::watch::channel(trunking::Recording::default());
    let decoded = {
        let engine = Arc::downgrade(&state.engine);
        let store = state.store.clone();
        let feed = state.decoded.clone();
        spawn_task("sdrmm-decoded", move || decoded::run(engine, store, feed))
    };
    let log = {
        let engine = Arc::downgrade(&state.engine);
        let store = state.store.clone();
        let dropped = state.decoder_log_dropped.clone();
        let records = state.decoded.subscribe();
        spawn_task("sdrmm-decoderlog", move || {
            decoderlog::run(records, engine, store, dropped)
        })
    };
    let patch = {
        let engine = Arc::downgrade(&state.engine);
        let store = state.store.clone();
        spawn_task("sdrmm-trunking", move || {
            trunking::watch_patch(engine, store, recording_tx)
        })
    };
    let monitor = {
        let engine = Arc::downgrade(&state.engine);
        let store = state.store.clone();
        let calls = state.calls.clone();
        spawn_task("sdrmm-monitors", move || monitor::run(engine, store, calls))
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
    let event_output = {
        let engine = Arc::downgrade(&state.engine);
        let store = state.store.clone();
        let calls = state.calls.clone();
        let records = state.decoded.subscribe();
        spawn_task("sdrmm-event-output", move || {
            event_output::run(records, engine, store, calls)
        })
    };
    let audio_fx = {
        let engine = Arc::downgrade(&state.engine);
        let hooks = Arc::new(recorders::Hooks {
            store: state.store.clone(),
            gate: state.recordings_gate.clone(),
            gps: state.gps.clone(),
        });
        spawn_task("sdrmm-recorders", move || audio_fx::run(engine, hooks))
    };
    let health = spawn_task("sdrmm-health", move || health.run());
    Background {
        tasks: vec![
            decoded,
            log,
            patch,
            calls,
            images,
            event_output,
            monitor,
            audio_fx,
            health,
        ],
        remote: state.remote.clone(),
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
    pub scheme: &'static str,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    _gate: phones::gate::GateGuard,
    _background: Background,
}

impl ServerHandle {
    pub async fn join(mut self) -> std::io::Result<()> {
        match (&mut self.task).await {
            Ok(res) => res,
            Err(join_err) => Err(std::io::Error::other(join_err)),
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn serve(config: Config, engine: Arc<Engine>) -> std::io::Result<ServerHandle> {
    refuse_phone_token(&config.options)?;
    engine.start_hotplug_prober(HOTPLUG_INTERVAL)?;
    engine.start_level_meter(LEVEL_INTERVAL)?;
    engine.start_occupancy_collector(HOTPLUG_INTERVAL)?;
    log_start(&config, &engine);
    let store = Store::open(config.db_path.as_deref()).map_err(std::io::Error::other)?;
    workspace::adopt_named_devices(&engine, &store);
    let mut state = AppState::new(engine, Arc::new(store));
    let health = configure(&mut state, &config.options);
    state.db_path = config.db_path.clone();
    state.data_dir = config
        .db_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    if let Some(dir) = &state.data_dir {
        state
            .engine
            .denoise_models()
            .set_dir(dir.join("denoise-models"));
    }
    let served = config
        .tls
        .as_ref()
        .map(tls::load)
        .transpose()
        .map_err(std::io::Error::other)?;
    let listener = std::net::TcpListener::bind(config.bind)?;
    listener.set_nonblocking(true)?;
    let local_addr = listener.local_addr()?;
    state.gate.set_main(main_listener(
        local_addr,
        config.tls.as_ref(),
        served.as_ref(),
    ));
    let background = start_runtime(&state, health);
    let app = app(&state, auth::ListenerRole::Main, served.is_some());
    state.remote.attach(app.clone());
    let scheme = if served.is_some() { "https" } else { "http" };
    tracing::info!(%local_addr, scheme, "SDR-- server listening");
    let task = match served {
        Some(served) => {
            let server = axum_server::from_tcp_rustls(
                listener,
                axum_server::tls_rustls::RustlsConfig::from_config(served.config),
            )?;
            tokio::spawn(async move { server.serve(app.into_make_service()).await })
        }
        None => {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            tokio::spawn(async move { axum::serve(listener, app).await })
        }
    };
    let gate = phones::gate::GateGuard::new(state.gate.clone());
    state.gate.restore(&state).await;
    Ok(ServerHandle {
        local_addr,
        scheme,
        task,
        _gate: gate,
        _background: background,
    })
}

fn refuse_phone_token(options: &ServerOptions) -> std::io::Result<()> {
    if options
        .token
        .as_deref()
        .is_some_and(|token| token.starts_with(sdrmm_wire::phone::PHONE_TOKEN_PREFIX))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the token must not start with sdrmm-phone.",
        ));
    }
    Ok(())
}

fn log_start(config: &Config, engine: &Engine) {
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
        None => tracing::info!("no token: LAN-trusted, unauthenticated"),
    }
}

fn main_listener(
    local_addr: SocketAddr,
    asked: Option<&tls::Tls>,
    served: Option<&tls::Served>,
) -> phones::gate::MainListener {
    let own_key = matches!(asked, Some(tls::Tls::SelfSigned { .. }));
    let named = matches!(asked, Some(tls::Tls::SelfSigned { names, .. }) if !names.is_empty());
    phones::gate::MainListener {
        record: phones::gate::ListenerRecord {
            role: auth::ListenerRole::Main,
            port: local_addr.port(),
            bound: local_addr.ip(),
            pin: served.map(|served| served.pin.clone()),
            stable_key: own_key && served.is_some(),
            names: served
                .filter(|_| named)
                .map(|served| served.names.clone())
                .unwrap_or_default(),
        },
        own_key: served
            .filter(|_| own_key)
            .map(|served| served.config.clone()),
    }
}

#[cfg(test)]
mod tests;
