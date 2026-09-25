mod api;
mod binding;
mod bus;
mod format;
mod host;
mod params;
mod socket;
mod starter;
mod store;
mod theme;
mod ui;
mod workspace;

use std::path::PathBuf;

use clap::Parser;
use zgui::prelude::*;

#[derive(Parser, Debug)]
#[command(
    name = "sdrmm-native",
    about = "A native sdr-- front end drawn with zgui"
)]
struct Args {
    #[arg(long, env = "SDRMM_NATIVE_SERVER")]
    server: Option<String>,
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

fn data_dir(cli: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match cli {
        Some(path) => Ok(path),
        None => Ok(dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("no platform data directory; pass --data-dir"))?
            .join("sdrmm-native")),
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,sdrmm_native=info".into()),
        )
        .init();

    let args = Args::parse();
    let tokio = zgui_tokio::install()?;

    let host = tokio.handle().block_on(async {
        match args.server {
            Some(base) => Ok(host::Host::remote(base)),
            None => host::Host::embedded(data_dir(args.data_dir)?).await,
        }
    })?;
    tracing::info!(base = %host.base, "sdr-- native ready");

    let api = api::Api::new(host.base.clone())?;
    let websocket = host.websocket_url();
    let runtime = tokio.handle().clone();

    let outcome = app()
        .with_application_id("dev.sdrmm.Native")
        .with_title("sdr-- native")
        .with_size(1440.0, 900.0)
        .with_min_size(880.0, 560.0)
        .with_stylesheet(theme::SHEET)
        .run(move || {
            let store = store::Store::new(api.clone());
            let (socket, incoming) = socket::connect(&runtime, websocket.clone());
            store.start(socket, incoming);
            ui::app(store)
        });

    host.shutdown();
    outcome.map_err(anyhow::Error::from)
}
