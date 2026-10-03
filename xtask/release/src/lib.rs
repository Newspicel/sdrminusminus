mod aur;
pub mod changeset;
mod sums;
mod updater;
mod version;

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Subcommand;

const HOMEPAGE: &str = "https://sdrmm.com";
const DOWNLOADS: &str = "https://downloads.sdrmm.com/releases";

#[derive(Subcommand)]
pub enum Cmd {
    SetVersion {
        version: String,
    },
    Changeset {
        bump: changeset::Bump,
        summary: String,
    },
    Release {
        #[arg(long)]
        dry_run: bool,
    },
    ReleaseNotes {
        version: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    UpdaterManifest {
        #[arg(long)]
        version: String,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        base_url: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Aur {
        #[arg(long)]
        version: String,
        #[arg(long)]
        sums: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}

pub fn run(root: &Path, cmd: &Cmd) -> Result<()> {
    match cmd {
        Cmd::SetVersion { version } => version::set(root, version),
        Cmd::Changeset { bump, summary } => changeset::add(root, *bump, summary),
        Cmd::Release { dry_run } => changeset::release(root, *dry_run),
        Cmd::ReleaseNotes { version, out } => changeset::notes(root, version, out.as_deref()),
        Cmd::UpdaterManifest {
            version,
            dir,
            base_url,
            out,
        } => updater::manifest(dir, version, base_url, out.as_deref()),
        Cmd::Aur { version, sums, out } => aur::packages(sums, version, out),
    }
}

pub fn root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("xtask-release sits two levels below the workspace root"))
}
