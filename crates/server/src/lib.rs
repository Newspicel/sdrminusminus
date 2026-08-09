//! `sdrmm-server` — the axum app as a *library* (PLAN §3, §10): `router()` builds the whole
//! HTTP+WS surface over a shared [`Engine`], and `serve()` binds it. The Tauri desktop app and
//! the headless binary both consume this crate, so there is exactly one server implementation.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::Router;
use sdrmm_engine::Engine;
use tower_http::cors::CorsLayer;
use utoipa_swagger_ui::SwaggerUi;

/// Hotplug probe cadence (PLAN §16 M1). Public so the desktop shell, which embeds the router
/// without going through [`serve`], starts the same prober.
pub const HOTPLUG_INTERVAL: Duration = Duration::from_secs(5);

mod assets;
mod rest;
mod ws;

/// Shared application state handed to every handler (cheap to clone: one `Arc`).
#[derive(Clone)]
pub(crate) struct AppState {
    pub engine: Arc<Engine>,
}

/// Server configuration (PLAN §11 `config.toml`; token auth lands at M5).
#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    /// Relax CORS for the Vite dev origin (PLAN §10 dev mode).
    pub dev_cors: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            dev_cors: false,
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

/// Build the full axum app: REST + WebSocket + Swagger UI + embedded SPA, over `engine`.
pub fn router(engine: Arc<Engine>, dev_cors: bool) -> Router {
    let state = AppState { engine };
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
    let app = router(engine, config.dev_cors);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(%local_addr, "sdr-- server listening");
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(ServerHandle { local_addr, task })
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use sdrmm_wire::StateSnapshot;
    use tower::ServiceExt;

    use super::*;

    fn test_router() -> Router {
        router(Engine::new(), false)
    }

    /// OpenAPI snapshot (PLAN §14): the REST paths must be present, and the WS-only enums must
    /// be force-registered as schema components (PLAN §4) or the generated TS client loses them.
    #[test]
    fn openapi_registers_paths_and_ws_schemas() {
        let spec = openapi().to_pretty_json().expect("serialize");
        for path in [
            "/api/state",
            "/api/devices",
            "/api/devicesets",
            "/api/devicesets/{ds}/device",
            "/api/devicesets/{ds}/channels/{ch}",
        ] {
            assert!(spec.contains(path), "missing path {path}");
        }
        // Force-registered WS message enums.
        assert!(spec.contains("ServerEvent"), "ServerEvent schema missing");
        assert!(
            spec.contains("ClientCommand"),
            "ClientCommand schema missing"
        );
    }

    #[tokio::test]
    async fn get_state_returns_empty_snapshot() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/state")
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let snap: StateSnapshot = serde_json::from_slice(&bytes).expect("json");
        assert!(snap.device_sets.is_empty());
    }

    #[tokio::test]
    async fn create_and_delete_device_set_over_http() {
        let app = test_router();
        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/devicesets")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"device_id":"virtual:siggen"}"#))
                    .expect("req"),
            )
            .await
            .expect("response");
        assert_eq!(create.status(), StatusCode::OK);

        let unknown = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/devicesets")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"device_id":"virtual:nope"}"#))
                    .expect("req"),
            )
            .await
            .expect("response");
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }
}
