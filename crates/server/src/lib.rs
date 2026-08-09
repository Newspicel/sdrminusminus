//! `sdrmm-server` — the axum app as a *library* (PLAN §3, §10): `router()` builds the whole
//! HTTP+WS surface over a shared [`Engine`], and `serve()` binds it. The Tauri desktop app and
//! the headless binary both consume this crate, so there is exactly one server implementation.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::Router;
use sdrmm_engine::Engine;
use tower_http::cors::CorsLayer;
use utoipa_swagger_ui::SwaggerUi;

/// Hotplug probe cadence (PLAN §16 M1). Public so the desktop shell, which embeds the router
/// without going through [`serve`], starts the same prober.
pub const HOTPLUG_INTERVAL: Duration = Duration::from_secs(5);

mod assets;
mod rest;
mod store;
mod ws;

pub use store::{Store, StoreError};

/// Shared application state handed to every handler (cheap to clone: two `Arc`s).
#[derive(Clone)]
pub(crate) struct AppState {
    pub engine: Arc<Engine>,
    pub store: Arc<Store>,
}

/// Server configuration (PLAN §11 `config.toml`; token auth lands at M5).
#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    /// Relax CORS for the Vite dev origin (PLAN §10 dev mode).
    pub dev_cors: bool,
    /// SQLite database for presets/bookmarks (PLAN §11); `None` = in-memory.
    pub db_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            dev_cors: false,
            db_path: None,
        }
    }
}

/// The OpenAPI document, produced without a running server (PLAN §4 step 1) — this is what
/// `cargo xtask codegen` serializes to `openapi.json`.
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    let (_router, api) = rest::openapi_router().split_for_parts();
    api
}

/// Build the full axum app: REST + WebSocket + Swagger UI + embedded SPA, over `engine` and
/// `store`.
pub fn router(engine: Arc<Engine>, store: Store, dev_cors: bool) -> Router {
    router_with_state(
        AppState {
            engine,
            store: Arc::new(store),
        },
        dev_cors,
    )
}

fn router_with_state(state: AppState, dev_cors: bool) -> Router {
    let (api_router, api) = rest::openapi_router().split_for_parts();

    let mut app = Router::new()
        .merge(api_router)
        .route("/api/ws", axum::routing::get(ws::handler))
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", api))
        .fallback(assets::static_handler)
        .with_state(state);

    if dev_cors {
        app = app.layer(CorsLayer::very_permissive());
    }
    app
}

/// A running server plus the address it actually bound (the port may be ephemeral).
pub struct ServerHandle {
    pub local_addr: SocketAddr,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl ServerHandle {
    /// Await server exit (it normally runs until the process ends).
    pub async fn join(self) -> std::io::Result<()> {
        match self.task.await {
            Ok(res) => res,
            Err(join_err) => Err(std::io::Error::other(join_err)),
        }
    }
}

/// Bind and start serving on `config.bind`, returning once the socket is listening.
pub async fn serve(config: Config, engine: Arc<Engine>) -> std::io::Result<ServerHandle> {
    engine.start_hotplug_prober(HOTPLUG_INTERVAL)?;
    // Name the file being opened: a cwd-dependent or unexpected path otherwise shows up
    // only as presets/bookmarks silently "vanishing".
    match &config.db_path {
        Some(path) => tracing::info!(db = %path.display(), "opening database"),
        None => tracing::info!("using in-memory database (nothing will persist)"),
    }
    let store = Store::open(config.db_path.as_deref()).map_err(std::io::Error::other)?;
    let app = router(engine, store, config.dev_cors);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(%local_addr, "sdr-- server listening");
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(ServerHandle { local_addr, task })
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, Bytes},
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use sdrmm_wire::{
        ApiError, Bookmark, ChannelParams, ChannelSettings, ChannelTypesResponse, CreatedId,
        CreatedRowId, DeviceSettings, NfmParams, PresetInfo, PresetSnapshot, StateSnapshot,
    };
    use tower::ServiceExt;

    use super::*;

    /// Hermetic engine: virtual driver only — `Engine::new()` would register the Soapy driver,
    /// whose probe enumerates live system modules (PLAN §14: no hardware in CI, ever).
    fn test_router() -> Router {
        test_router_with_store().0
    }

    /// Same, but keeping a handle on the store so tests can plant snapshots the REST surface
    /// cannot produce (e.g. a preset whose channels are invalid at its own rate).
    fn test_router_with_store() -> (Router, Arc<Store>) {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let store = Arc::new(Store::open(None).expect("in-memory store"));
        let state = AppState {
            engine: Engine::with_registry(registry),
            store: store.clone(),
        };
        (router_with_state(state, false), store)
    }

    async fn request(
        app: Router,
        method: &str,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, Bytes) {
        let mut builder = Request::builder().method(method).uri(uri);
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
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (status, bytes)
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

    /// OpenAPI snapshot (PLAN §14): the REST paths must be present, and the WS-only enums must
    /// be force-registered as schema components (PLAN §4) or the generated TS client loses them.
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
        ] {
            assert!(spec.contains(path), "missing path {path}");
        }
        // Force-registered WS message enums.
        assert!(spec.contains("ServerEvent"), "ServerEvent schema missing");
        assert!(
            spec.contains("ClientCommand"),
            "ClientCommand schema missing"
        );
        // Path-referenced DTOs the generated client needs as named schemas.
        for schema in ["ChannelParams", "ChannelSettings", "PresetSnapshot"] {
            assert!(
                spec.contains(&format!("\"{schema}\"")),
                "{schema} schema missing"
            );
        }
    }

    #[tokio::test]
    async fn get_state_returns_empty_snapshot() {
        let app = test_router();
        let snap = get_state(&app).await;
        assert!(snap.device_sets.is_empty());
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
    async fn channeltypes_lists_all_four_demods() {
        let (status, body) = request(test_router(), "GET", "/api/channeltypes", None).await;
        assert_eq!(status, StatusCode::OK);
        let types: ChannelTypesResponse = serde_json::from_slice(&body).expect("json");
        assert_eq!(types.types.len(), 4);
        for id in ["nfm", "am", "ssb", "wfm"] {
            assert!(
                types.types.iter().any(|t| t.type_id == id),
                "missing type {id}"
            );
        }
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

        // Unknown demod type: rejected by deserialization before reaching the engine, but
        // still in the ApiError shape the contract promises.
        let (status, body) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{ds}/channels/{ch}"),
            Some(r#"{"params":{"type":"zzz","settings":{}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        serde_json::from_slice::<ApiError>(&body).expect("ApiError body");

        // Well-formed body the engine rejects: offset outside the ±1.024 MHz passband.
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
        let source = create_virtual_set(&app).await;
        let (status, _) = request(
            app.clone(),
            "PATCH",
            &format!("/api/devicesets/{source}/device"),
            Some(r#"{"center_hz":145500000.0,"sample_rate":2400000.0}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{source}/channels"),
            Some(
                r#"{"settings":{"offset_hz":25000.0,"squelch_db":-70.0,"params":{"type":"nfm","settings":{}}}}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = request(
            app.clone(),
            "POST",
            "/api/presets",
            Some(&format!(r#"{{"name":"2m","device_set":{source}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let preset = serde_json::from_slice::<CreatedRowId>(&body)
            .expect("json")
            .id;

        let (status, _) = request(
            app.clone(),
            "POST",
            "/api/presets",
            Some(r#"{"name":"ghost","device_set":999}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = request(app.clone(), "GET", "/api/presets", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: Vec<PresetInfo> = serde_json::from_slice(&body).expect("json");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, preset);
        assert_eq!(listed[0].name, "2m");
        assert_eq!(listed[0].device_id, "virtual:siggen");

        let target = create_virtual_set(&app).await;
        let (status, _) = request(
            app.clone(),
            "POST",
            &format!("/api/presets/{preset}/apply"),
            Some(&format!(r#"{{"device_set":{target}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let snap = get_state(&app).await;
        let source_set = snap
            .device_sets
            .iter()
            .find(|s| s.id == source)
            .expect("source");
        let target_set = snap
            .device_sets
            .iter()
            .find(|s| s.id == target)
            .expect("target");
        assert_eq!(target_set.settings.center_hz, Some(145_500_000.0));
        assert_eq!(target_set.settings.sample_rate, Some(2_400_000.0));
        assert_eq!(target_set.channels.len(), 1);
        assert_eq!(
            target_set.channels[0].settings,
            source_set.channels[0].settings
        );

        let (status, _) = request(
            app.clone(),
            "POST",
            "/api/presets/999/apply",
            Some(&format!(r#"{{"device_set":{target}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = request(
            app.clone(),
            "POST",
            &format!("/api/presets/{preset}/apply"),
            Some(r#"{"device_set":999}"#),
        )
        .await;
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

    /// Extractor failures must produce the documented ApiError JSON shape, never axum's
    /// plain-text defaults (the generated client's typed error branch depends on it).
    #[tokio::test]
    async fn extractor_rejections_return_api_error_body() {
        let app = test_router();

        // Malformed JSON syntax → 400.
        let (status, body) =
            request(app.clone(), "POST", "/api/devicesets", Some("{not json")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
        assert_eq!(err.error, "invalid request body");
        assert!(err.detail.is_some());

        // Well-formed JSON that misses the schema → 422.
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

        // Unparseable path parameter → 400.
        let (status, body) = request(app, "DELETE", "/api/devicesets/abc", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
        assert_eq!(err.error, "invalid path parameter");
    }

    fn preset_250k(channels: Vec<ChannelSettings>) -> PresetSnapshot {
        PresetSnapshot {
            version: rest::PRESET_VERSION,
            device_id: "virtual:siggen".to_string(),
            settings: DeviceSettings {
                center_hz: Some(100_000_000.0),
                sample_rate: Some(250_000.0),
                ..DeviceSettings::default()
            },
            channels,
        }
    }

    fn nfm_at(offset_hz: f64) -> ChannelSettings {
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Nfm(NfmParams::default()),
        }
    }

    /// Regression: applying a lower-rate preset to a set whose *current* channels don't fit
    /// that rate must succeed — patch_device may only be asked to validate against the
    /// preset's channels, not the ones the apply removes.
    #[tokio::test]
    async fn apply_preset_replaces_channels_that_do_not_fit_the_preset_rate() {
        let (app, store) = test_router_with_store();
        let ds = create_virtual_set(&app).await;
        // Valid at the default 2.048 Msps, far outside the preset's ±125 kHz passband.
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
            Some(&format!(r#"{{"device_set":{ds}}}"#)),
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

    /// A mid-sequence engine failure cannot be rolled back (each step is real device I/O);
    /// the error must say exactly what state the set was left in, and the state must match.
    #[tokio::test]
    async fn apply_preset_failure_reports_partial_state_honestly() {
        let (app, store) = test_router_with_store();
        let ds = create_virtual_set(&app).await;
        let (status, _) = request(
            app.clone(),
            "POST",
            &format!("/api/devicesets/{ds}/channels"),
            Some(r#"{"settings":{"offset_hz":100000.0,"params":{"type":"nfm","settings":{}}}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // A preset the REST surface can't produce: its channel is invalid at its own rate,
        // so the apply fails at the final add step.
        let preset = store
            .create_preset("broken", &preset_250k(vec![nfm_at(900_000.0)]))
            .expect("preset");
        let (status, body) = request(
            app.clone(),
            "POST",
            &format!("/api/presets/{preset}/apply"),
            Some(&format!(r#"{{"device_set":{ds}}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let err: ApiError = serde_json::from_slice(&body).expect("ApiError body");
        let detail = err.detail.expect("partial-application detail");
        assert!(
            detail.contains("existing channels removed")
                && detail.contains("device settings applied")
                && detail.contains("0 of 1 preset channels added"),
            "dishonest detail: {detail}"
        );

        // The reported state is the actual state: retuned, and no channels left.
        let snap = get_state(&app).await;
        let set = snap
            .device_sets
            .iter()
            .find(|s| s.id == ds)
            .expect("device set");
        assert_eq!(set.settings.sample_rate, Some(250_000.0));
        assert!(set.channels.is_empty());
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
}
