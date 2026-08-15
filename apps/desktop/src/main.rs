//! `sdrmm-desktop` — Tauri v2 shell. Embeds `crates/server` in-process on an
//! ephemeral loopback port and points the WebView at it, so the desktop app and a remote
//! browser run the exact same frontend over the same origin model. The UI talks to the server
//! purely over HTTP/WebSocket, so no Tauri IPC (and no capability grant) is required.

use std::sync::Arc;

use anyhow::Context;
use sdrmm_engine::Engine;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

mod update;

#[cfg(feature = "soapy")]
fn configure_soapy_runtime() -> anyhow::Result<()> {
    let executable = std::env::current_exe().context("cannot locate desktop executable")?;
    let executable_dir = executable
        .parent()
        .context("desktop executable has no parent directory")?;
    #[cfg(target_os = "macos")]
    let resources = executable_dir.join("../Resources");
    #[cfg(target_os = "linux")]
    let resources = executable_dir.join("../lib/sdr--");
    #[cfg(target_os = "windows")]
    let resources = executable_dir.to_path_buf();
    let root = resources.join("soapy");
    let modules = root.join("lib").join("SoapySDR").join("modules0.8");
    if modules.is_dir() {
        unsafe { sdrmm_device_soapy::configure_bundled_runtime(&root, &modules) }
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    #[cfg(feature = "soapy")]
    configure_soapy_runtime()?;

    tracing_subscriber::fmt()
        .with_env_filter("info,sdrmm=debug")
        .init();

    tauri::Builder::default()
        // Both are driven from Rust only (see `update`), so neither needs a capability grant.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Bind synchronously so the port is known before we build the window.
            let listener =
                tauri::async_runtime::block_on(tokio::net::TcpListener::bind(("127.0.0.1", 0u16)))?;
            let port = listener.local_addr()?.port();

            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let engine = Engine::new(Some(data_dir.join("recordings")));
            engine.start_hotplug_prober(sdrmm_server::HOTPLUG_INTERVAL)?;
            engine.start_level_meter(sdrmm_server::LEVEL_INTERVAL)?;
            engine.start_occupancy_collector(sdrmm_server::HOTPLUG_INTERVAL)?;
            // Managed so the exit hook below can reach the engine for teardown.
            app.manage(engine.clone());
            let store = sdrmm_server::Store::open(Some(&data_dir.join("sdrmm.db")))?;
            let router = {
                let _entered = tauri::async_runtime::handle().inner().enter();
                sdrmm_server::router(
                    engine,
                    store,
                    // Loopback-only and single-user: a token would gate the app against itself.
                    // Tokens matter for the *remote* connections the app saves, which are the
                    // other server's configuration, not this one's.
                    &sdrmm_server::ServerOptions::default(),
                )
            };
            tauri::async_runtime::spawn(async move {
                if let Err(e) = axum::serve(listener, router).await {
                    tracing::error!("embedded server exited: {e}");
                }
            });

            let url: tauri::Url = format!("http://127.0.0.1:{port}").parse()?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("sdr--")
                .inner_size(1280.0, 800.0)
                .build()?;

            update::spawn(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
        .context("failed to start sdr-- desktop")?
        .run(|app, event| {
            // Tauri exits the process without unwinding `main`, so `Engine`'s drop never
            // runs on its own: tear down here or a live recording dies as an unlisted
            // breadcrumb instead of a finalized pair.
            if matches!(event, RunEvent::Exit)
                && let Some(engine) = app.try_state::<Arc<Engine>>()
            {
                engine.shutdown();
            }
        });
    Ok(())
}
