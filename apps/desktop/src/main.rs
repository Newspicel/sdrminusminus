//! `sdrmm-desktop` — Tauri v2 shell (PLAN §10). Embeds `crates/server` in-process on an
//! ephemeral loopback port and points the WebView at it, so the desktop app and a remote
//! browser run the exact same frontend over the same origin model. The UI talks to the server
//! purely over HTTP/WebSocket, so no Tauri IPC (and no capability grant) is required.

use sdrmm_engine::Engine;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("info,sdrmm=debug")
        .init();

    tauri::Builder::default()
        .setup(|app| {
            // Bind synchronously so the port is known before we build the window.
            let listener =
                tauri::async_runtime::block_on(tokio::net::TcpListener::bind(("127.0.0.1", 0u16)))?;
            let port = listener.local_addr()?.port();

            let engine = Engine::new();
            // The desktop shell bypasses `serve()` (it needs the pre-bound listener), so it
            // must start the hotplug prober itself to match the headless binary.
            engine.start_hotplug_prober(sdrmm_server::HOTPLUG_INTERVAL)?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = sdrmm_server::Store::open(Some(&data_dir.join("sdrmm.db")))?;
            let router = sdrmm_server::router(engine, store, false);
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running sdr-- desktop");
}
