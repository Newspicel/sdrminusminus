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
mod basemap;
mod calls;
pub(crate) mod coherent;
mod decoderlog;
pub(crate) mod df_fusion;
pub mod doctor;
mod event_output;
mod events;
mod gps;
mod images;
mod ionosonde;
mod mcp;
pub mod notices;
mod rest;
pub mod routing;
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
    pub routing: routing::RoutingOptions,
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
    pub(crate) coherent: Arc<coherent::CoherentHub>,
    pub(crate) fusion: df_fusion::SharedFusion,
    pub(crate) routing: Arc<routing::RoutingOptions>,
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
            coherent: Arc::new(coherent::CoherentHub::default()),
            fusion: Arc::new(df_fusion::FusionHub::default()),
            routing: Arc::new(routing::RoutingOptions::default()),
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
    state.routing = Arc::new(options.routing.clone());
    let (router, background) = router_with_state(state, options);
    background.detach();
    router
}

fn router_with_state(state: AppState, options: &ServerOptions) -> (Router, Background) {
    let background = start_background(&state);
    ws::start_decoded_encoder(&state);
    workspace::spawn_autosave(&state);
    state.gps.reconcile(&state);
    rest::reload_arrays(&state);
    let (api_router, api) = rest::openapi_router().split_for_parts();

    let mut app = Router::new()
        .merge(api_router)
        .route("/api/ws", axum::routing::get(ws::handler))
        .route(
            "/api/basemap.pmtiles",
            axum::routing::get(basemap::handler).head(basemap::handler),
        )
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
    let event_output = {
        let engine = Arc::downgrade(&state.engine);
        let store = state.store.clone();
        let calls = state.calls.clone();
        spawn_task("sdrmm-event-output", move || {
            event_output::run(engine, store, calls)
        })
    };
    Background {
        tasks: vec![log, patch, calls, images, event_output],
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
mod tests;
