use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use sdrmm_engine::Engine;
use sdrmm_server::{ServerOptions, Store};

pub struct Host {
    pub base: String,
    engine: Option<Arc<Engine>>,
}

impl Host {
    pub fn remote(base: String) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            engine: None,
        }
    }

    pub async fn embedded(data_dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("cannot create {}", data_dir.display()))?;

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0u16))
            .await
            .context("cannot bind the embedded server")?;
        let addr: SocketAddr = listener.local_addr().context("no local address")?;

        let engine = Engine::with_registry(
            sdrmm_engine::builtin_registry_accelerated(Some(data_dir.join("recordings")), 1.0),
            Some(data_dir.join("recordings")),
        );
        engine.start_hotplug_prober(sdrmm_server::HOTPLUG_INTERVAL)?;
        engine.start_level_meter(sdrmm_server::LEVEL_INTERVAL)?;
        engine.start_occupancy_collector(sdrmm_server::HOTPLUG_INTERVAL)?;

        let store = Store::open(Some(&data_dir.join("native.db"))).context("cannot open store")?;
        let router = sdrmm_server::router(engine.clone(), store, &ServerOptions::default());
        tokio::spawn(async move {
            if let Err(error) = axum_serve(listener, router).await {
                tracing::error!(%error, "embedded server exited");
            }
        });

        Ok(Self {
            base: format!("http://{addr}"),
            engine: Some(engine),
        })
    }

    pub fn websocket_url(&self) -> String {
        let scheme = if self.base.starts_with("https://") {
            "wss://"
        } else {
            "ws://"
        };
        let rest = self
            .base
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        format!("{scheme}{rest}/api/ws")
    }

    pub fn shutdown(&self) {
        if let Some(engine) = &self.engine {
            engine.shutdown();
        }
    }
}

async fn axum_serve(
    listener: tokio::net::TcpListener,
    router: axum::Router,
) -> std::io::Result<()> {
    axum::serve(listener, router).await
}
