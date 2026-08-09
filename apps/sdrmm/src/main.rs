//! `sdrmm` — the headless server binary (PLAN §3), a thin wrapper over `crates/server`. This is
//! the Raspberry Pi target: one binary, embedded UI, browse to `http://host:8080`.

use std::net::SocketAddr;

use anyhow::Context;
use clap::Parser;
use sdrmm_engine::Engine;
use sdrmm_server::{Config, serve};

/// sdr-- headless SDR server.
#[derive(Parser, Debug)]
#[command(name = "sdrmm", version, about)]
struct Args {
    /// Address to bind (LAN-trusted by default, PLAN §12).
    #[arg(long, default_value = "0.0.0.0:8080")]
    bind: SocketAddr,
    /// Relax CORS for a separate dev origin (PLAN §10).
    #[arg(long)]
    dev_cors: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sdrmm=debug".into()),
        )
        .init();

    let args = Args::parse();
    let engine = Engine::new();
    let config = Config {
        bind: args.bind,
        dev_cors: args.dev_cors,
    };

    let handle = serve(config, engine)
        .await
        .context("failed to bind server")?;
    tracing::info!(addr = %handle.local_addr, "sdr-- ready");

    tokio::select! {
        res = handle.join() => res.context("server task failed")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("shutting down"),
    }
    Ok(())
}
