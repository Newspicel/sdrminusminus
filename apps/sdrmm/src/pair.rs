use std::path::PathBuf;

use anyhow::Context;
use sdrmm_server::{RemotePairing, Store};
use sdrmm_tunnel::{DeviceKey, pairing::Pairing};
use sdrmm_wire::DEFAULT_REMOTE_APP;

#[derive(clap::Args, Debug)]
pub(crate) struct PairArgs {
    #[arg(long)]
    pub(crate) db: Option<PathBuf>,
    #[arg(long, env = "SDRMM_REMOTE_APP")]
    pub(crate) remote_app: Option<url::Url>,
}

#[tokio::main]
pub(crate) async fn run(args: PairArgs) -> anyhow::Result<()> {
    let db_path = crate::resolve_db_path(args.db)?;
    let app = match args.remote_app {
        Some(app) => app,
        None => DEFAULT_REMOTE_APP
            .parse()
            .context("default remote app address")?,
    };
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let store = Store::open(Some(&db_path)).context("open the database")?;
    if let Some(old) = store.remote_pairing().context("read the pairing")? {
        println!("Replacing the pairing with {}.", old.device_id);
    }
    let (key, document) = DeviceKey::generate().context("create a device key")?;
    let pairing = Pairing::start(&app, &key, &sdrmm_server::device_name())
        .await
        .with_context(|| format!("start pairing with {app}"))?;
    let started = pairing.started();
    println!("Code: {}", started.user_code);
    println!("Open {}", started.verification_uri_complete);
    let paired = pairing.wait().await.context("pairing")?;
    store
        .save_remote_pairing(&RemotePairing::new(
            paired.device_id.clone(),
            paired.relay_url.to_string(),
            document,
        ))
        .context("keep the pairing")?;
    println!("Paired as {}. Restart sdrmm to connect.", paired.device_id);
    Ok(())
}
