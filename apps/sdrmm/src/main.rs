use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Context;
use clap::Parser;
use sdrmm_engine::Engine;
use sdrmm_server::{Config, ServerOptions, routing::RoutingOptions, serve, tls::Tls};
use sdrmm_wire::RoutingBackend;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum RoutingBackendArg {
    OpenRouteService,
    GraphHopper,
}

impl From<RoutingBackendArg> for RoutingBackend {
    fn from(value: RoutingBackendArg) -> Self {
        match value {
            RoutingBackendArg::OpenRouteService => Self::OpenRouteService,
            RoutingBackendArg::GraphHopper => Self::GraphHopper,
        }
    }
}

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
    #[arg(long, requires = "tls_key", conflicts_with = "tls_self_signed")]
    tls_cert: Option<PathBuf>,
    #[arg(long, requires = "tls_cert", conflicts_with = "tls_self_signed")]
    tls_key: Option<PathBuf>,
    #[arg(long)]
    tls_self_signed: bool,
    #[arg(
        long = "tls-name",
        env = "SDRMM_TLS_NAMES",
        value_delimiter = ',',
        requires = "tls_self_signed"
    )]
    tls_names: Vec<String>,
    #[arg(
        long,
        env = "SDRMM_ROUTING_BACKEND",
        default_value = "open-route-service"
    )]
    routing_backend: RoutingBackendArg,
    #[arg(long, env = "SDRMM_ROUTING_URL")]
    routing_url: Option<String>,
    #[arg(long, env = "SDRMM_ROUTING_KEY", hide_env_values = true)]
    routing_key: Option<String>,
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

fn resolve_tls(args: &Args, db_path: &Path) -> anyhow::Result<Option<Tls>> {
    match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => Ok(Some(Tls::Files {
            cert: cert.clone(),
            key: key.clone(),
        })),
        (None, None) if args.tls_self_signed => {
            let dir = db_path
                .parent()
                .context("no directory to keep the self-signed certificate in")?;
            Ok(Some(Tls::SelfSigned {
                dir: dir.to_path_buf(),
                names: args.tls_names.clone(),
            }))
        }
        (None, None) => Ok(None),
        _ => anyhow::bail!("--tls-cert and --tls-key must be given together"),
    }
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
    #[cfg(feature = "soapy")]
    sdrmm_device_soapy::enable_isolated_probes();

    sdrmm_server::diagnostics::install_tracing()?;

    let mut args = Args::parse();
    let db_path = resolve_db_path(args.db.take())?;
    let recordings_dir = resolve_recordings_dir(args.recordings_dir.take())?;
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
    let tls = resolve_tls(&args, &db_path)?;
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
        tls,
        options: ServerOptions {
            dev_cors: args.dev_cors,
            token: args.token,
            routing: RoutingOptions {
                backend: args.routing_backend.into(),
                base_url: args.routing_url,
                key: args.routing_key,
            },
        },
    };

    let handle = serve(config, engine.clone())
        .await
        .context("failed to start server")?;
    tracing::info!(url = %format!("{}://{}", handle.scheme, handle.local_addr), "SDR-- ready");

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
    fn tls_is_off_unless_asked_for() {
        let args = Args::parse_from(["sdrmm"]);
        assert_eq!(
            resolve_tls(&args, Path::new("/data/sdrmm.db")).expect("resolve"),
            None
        );
    }

    #[test]
    fn a_certificate_pair_is_taken_as_given() {
        let args = Args::parse_from(["sdrmm", "--tls-cert", "a.pem", "--tls-key", "a.key"]);
        assert_eq!(
            resolve_tls(&args, Path::new("/data/sdrmm.db")).expect("resolve"),
            Some(Tls::Files {
                cert: PathBuf::from("a.pem"),
                key: PathBuf::from("a.key"),
            })
        );
    }

    #[test]
    fn self_signed_material_is_kept_beside_the_database() {
        let args = Args::parse_from(["sdrmm", "--tls-self-signed"]);
        assert_eq!(
            resolve_tls(&args, Path::new("/data/sdrmm.db")).expect("resolve"),
            Some(Tls::SelfSigned {
                dir: PathBuf::from("/data"),
                names: Vec::new(),
            })
        );
    }

    #[test]
    fn the_names_a_self_signed_certificate_must_cover_are_carried_through() {
        let args = Args::parse_from([
            "sdrmm",
            "--tls-self-signed",
            "--tls-name",
            "radio.example,192.0.2.10",
            "--tls-name",
            "nas.local",
        ]);
        assert_eq!(
            resolve_tls(&args, Path::new("/data/sdrmm.db")).expect("resolve"),
            Some(Tls::SelfSigned {
                dir: PathBuf::from("/data"),
                names: vec![
                    "radio.example".to_owned(),
                    "192.0.2.10".to_owned(),
                    "nas.local".to_owned(),
                ],
            })
        );
    }

    #[test]
    fn a_name_without_a_self_signed_certificate_is_rejected() {
        assert!(
            Args::try_parse_from(["sdrmm", "--tls-name", "radio.example"]).is_err(),
            "a name was accepted with nothing to put it on"
        );
    }

    #[test]
    fn half_a_certificate_pair_is_rejected() {
        for half in [
            vec!["sdrmm", "--tls-cert", "a.pem"],
            vec!["sdrmm", "--tls-key", "a.key"],
        ] {
            assert!(Args::try_parse_from(&half).is_err(), "accepted {half:?}");
        }
    }

    #[test]
    fn a_certificate_and_a_self_signed_one_cannot_both_be_asked_for() {
        assert!(
            Args::try_parse_from([
                "sdrmm",
                "--tls-self-signed",
                "--tls-cert",
                "a.pem",
                "--tls-key",
                "a.key",
            ])
            .is_err()
        );
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
