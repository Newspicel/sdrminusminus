use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "xtask-release", about = "SDR-- release tasks")]
struct Cli {
    #[command(subcommand)]
    cmd: xtask_release::Cmd,
}

fn main() -> Result<()> {
    xtask_release::run(&xtask_release::root()?, &Cli::parse().cmd)
}
