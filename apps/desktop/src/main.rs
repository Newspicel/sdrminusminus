#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use anyhow::Context;
use sdrmm_engine::Engine;
use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

mod browser;
mod graphics;
mod reveal;
mod update;

fn main() -> anyhow::Result<()> {
    #[cfg(feature = "soapy")]
    sdrmm_device_soapy::enable_isolated_probes();

    sdrmm_server::diagnostics::install_tracing()?;

    unsafe { graphics::configure() };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let engine = Engine::new(Some(data_dir.join("recordings")));
            app.manage(engine.clone());
            let config = sdrmm_server::Config {
                bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
                db_path: Some(data_dir.join("sdrmm.db")),
                tls: None,
                options: sdrmm_server::ServerOptions {
                    shell: Some(Arc::new(reveal::Shell)),
                    ..sdrmm_server::ServerOptions::default()
                },
            };
            let handle = tauri::async_runtime::block_on(sdrmm_server::serve(config, engine))?;
            let url: tauri::Url = format!("http://{}", handle.local_addr).parse()?;
            let dialogs = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = handle.join().await {
                    tracing::error!(%error, "embedded server exited");
                    dialogs
                        .dialog()
                        .message(format!("Server stopped: {error}"))
                        .show(|_| {});
                }
            });

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("SDR--")
                .inner_size(1280.0, 800.0)
                .on_new_window(browser::open_in_browser)
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
