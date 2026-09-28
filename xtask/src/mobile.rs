use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

#[derive(clap::Args)]
pub(crate) struct Mobile {
    #[command(subcommand)]
    pub(crate) what: MobileWhat,
}

#[derive(Clone, Debug, clap::Subcommand)]
pub(crate) enum MobileWhat {
    Bindings,
    Ios,
    Android {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Check {
        #[arg(long)]
        ios: bool,
        #[arg(long)]
        android: bool,
    },
}

pub(crate) fn run(_root: &Path, args: &Mobile) -> Result<()> {
    bail!("xtask mobile {}: not built yet", name(&args.what))
}

pub(crate) fn check(_root: &Path) -> Result<()> {
    println!("mobile gate: not built yet");
    Ok(())
}

fn name(what: &MobileWhat) -> &'static str {
    match what {
        MobileWhat::Bindings => "bindings",
        MobileWhat::Ios => "ios",
        MobileWhat::Android { .. } => "android",
        MobileWhat::Check { .. } => "check",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mobile_command_says_it_is_not_built_yet() {
        for what in [
            MobileWhat::Bindings,
            MobileWhat::Ios,
            MobileWhat::Android { out: None },
            MobileWhat::Check {
                ios: true,
                android: false,
            },
        ] {
            let error = run(Path::new("."), &Mobile { what: what.clone() })
                .expect_err("placeholder must refuse");
            assert!(error.to_string().ends_with("not built yet"), "{error}");
        }
    }
}
