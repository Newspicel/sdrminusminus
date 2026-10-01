use std::path::Path;

use anyhow::Result;
use clap::{Args, Subcommand};

mod apps;
mod decoders;
mod dsp;
mod report;

#[derive(Subcommand)]
pub enum Compare {
    Dsp(Scope),
    Decoders(Scope),
    Apps(apps::Apps),
}

#[derive(Args)]
pub struct Scope {
    #[arg(long)]
    pub ours: bool,
}

pub fn run(root: &Path, suite: &Compare) -> Result<()> {
    match suite {
        Compare::Dsp(scope) => dsp::run(root, scope.ours),
        Compare::Decoders(scope) => decoders::run(root, scope.ours),
        Compare::Apps(args) => apps::run(root, args),
    }
}
