use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context;
use clap::Parser;
use sdrmm_engine::Engine;
use sdrmm_server::{Config, ServerOptions, serve};

#[derive(Parser, Debug)]
#[command(name = "sdrmm", version, about)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:8080")]
    bind: SocketAddr,
    #[arg(long)]
    dev_cors: bool,
    #[arg(long)]
    db: Option<PathBuf>,
    #[arg(long)]
    recordings_dir: Option<PathBuf>,
    #[arg(long, hide = true, default_value_t = 1.0, value_parser = parse_playback_speed)]
    playback_speed: f64,
    #[arg(long, env = "SDRMM_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[arg(long)]
    doctor: bool,
    #[arg(long)]
    doctor_rates: bool,
}

fn resolve_db_path(cli: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let path = match cli {
        Some(path) => path,
        None => dirs::data_dir()
            .context("no platform data directory; pass --db")?
            .join("sdrmm")
            .join("sdrmm.db"),
    };
    std::path::absolute(&path).with_context(|| format!("cannot resolve {}", path.display()))
}

fn parse_playback_speed(raw: &str) -> Result<f64, String> {
    let speed: f64 = raw
        .parse()
        .map_err(|_| format!("`{raw}` is not a number"))?;
    if !speed.is_finite() || speed < 1.0 {
        return Err("playback speed must be finite and at least real time (1.0)".to_string());
    }
    Ok(speed)
}

fn resolve_recordings_dir(cli: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let path = match cli {
        Some(path) => path,
        None => dirs::data_dir()
            .context("no platform data directory; pass --recordings-dir")?
            .join("sdrmm")
            .join("recordings"),
    };
    std::path::absolute(&path).with_context(|| format!("cannot resolve {}", path.display()))
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
    let db_path = resolve_db_path(args.db)?;
    let recordings_dir = resolve_recordings_dir(args.recordings_dir)?;
    if args.doctor {
        print!(
            "{}",
            sdrmm_server::doctor::render(&sdrmm_server::doctor::collect(
                Some(&db_path),
                Some(&recordings_dir),
            ))
        );
        return Ok(());
    }
    if args.doctor_rates {
        let registry = sdrmm_engine::builtin_registry(None);
        print!(
            "{}",
            sdrmm_server::doctor::render(&sdrmm_server::doctor::rate_report(&registry))
        );
        return Ok(());
    }
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }

    let engine = Engine::with_registry(
        sdrmm_engine::builtin_registry_accelerated(
            Some(recordings_dir.clone()),
            args.playback_speed,
        ),
        Some(recordings_dir),
    );
    let config = Config {
        bind: args.bind,
        db_path: Some(db_path),
        options: ServerOptions {
            dev_cors: args.dev_cors,
            token: args.token,
        },
    };

    let handle = serve(config, engine.clone())
        .await
        .context("failed to start server")?;
    tracing::info!(addr = %handle.local_addr, "sdr-- ready");

    tokio::select! {
        res = handle.join() => res.context("server task failed")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("shutting down"),
    }
    engine.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_db_path_is_absolute_and_in_the_data_dir() {
        let path = resolve_db_path(None).expect("resolve");
        assert!(path.is_absolute(), "{}", path.display());
        assert!(
            path.ends_with("sdrmm/sdrmm.db"),
            "unexpected default {}",
            path.display()
        );
    }

    #[test]
    fn db_flag_overrides_and_is_made_absolute() {
        let path = resolve_db_path(Some(PathBuf::from("custom.db"))).expect("resolve");
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.ends_with("custom.db"), "{}", path.display());

        let explicit = std::env::temp_dir().join("elsewhere").join("x.db");
        let path = resolve_db_path(Some(explicit.clone())).expect("resolve");
        assert_eq!(path, explicit);
    }

    #[test]
    fn default_recordings_dir_is_absolute_and_in_the_data_dir() {
        let path = resolve_recordings_dir(None).expect("resolve");
        assert!(path.is_absolute(), "{}", path.display());
        assert!(
            path.ends_with("sdrmm/recordings"),
            "unexpected default {}",
            path.display()
        );
    }

    #[test]
    fn playback_speed_accepts_real_time_and_faster() {
        assert_eq!(parse_playback_speed("1").expect("parse"), 1.0);
        assert_eq!(parse_playback_speed("20.5").expect("parse"), 20.5);
    }

    #[test]
    fn playback_speed_rejects_slower_than_real_time_and_nonsense() {
        for raw in ["0.5", "0", "-2", "inf", "nan", "fast"] {
            assert!(parse_playback_speed(raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn playback_speed_defaults_to_real_time() {
        let args = Args::parse_from(["sdrmm"]);
        assert_eq!(args.playback_speed, 1.0);
    }

    #[test]
    fn recordings_dir_flag_overrides_and_is_made_absolute() {
        let path = resolve_recordings_dir(Some(PathBuf::from("recs"))).expect("resolve");
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.ends_with("recs"), "{}", path.display());

        let explicit = std::env::temp_dir().join("elsewhere").join("recs");
        let path = resolve_recordings_dir(Some(explicit.clone())).expect("resolve");
        assert_eq!(path, explicit);
    }
}
