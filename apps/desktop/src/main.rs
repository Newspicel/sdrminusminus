#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use anyhow::Context;
use sdrmm_engine::Engine;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

mod graphics;
mod update;

fn main() -> anyhow::Result<()> {
    #[cfg(feature = "soapy")]
    sdrmm_device_soapy::enable_isolated_probes();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sdrmm=debug".into()),
        )
        .init();

    unsafe { graphics::configure() };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let listener =
                tauri::async_runtime::block_on(tokio::net::TcpListener::bind(("127.0.0.1", 0u16)))?;
            let port = listener.local_addr()?.port();

            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let engine = Engine::new(Some(data_dir.join("recordings")));
            engine.start_hotplug_prober(sdrmm_server::HOTPLUG_INTERVAL)?;
            engine.start_level_meter(sdrmm_server::LEVEL_INTERVAL)?;
            engine.start_occupancy_collector(sdrmm_server::HOTPLUG_INTERVAL)?;
            app.manage(engine.clone());
            let store = sdrmm_server::Store::open(Some(&data_dir.join("sdrmm.db")))?;
            let router = {
                let _entered = tauri::async_runtime::handle().inner().enter();
                sdrmm_server::router(engine, store, &sdrmm_server::ServerOptions::default())
            };
            tauri::async_runtime::spawn(async move {
                if let Err(e) = axum::serve(listener, router).await {
                    tracing::error!("embedded server exited: {e}");
                }
            });

            let url: tauri::Url = format!("http://127.0.0.1:{port}").parse()?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("SDR--")
                .inner_size(1280.0, 800.0)
                .build()?;

            update::spawn(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .context("failed to start SDR-- desktop")?
        .run(|app, event| {
            if matches!(event, RunEvent::Exit)
                && let Some(engine) = app.try_state::<Arc<Engine>>()
            {
                engine.shutdown();
            }
        });
    Ok(())
}
