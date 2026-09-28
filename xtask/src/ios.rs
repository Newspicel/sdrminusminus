use std::path::Path;

use anyhow::{Result, bail};

#[derive(Clone, Debug, clap::Subcommand)]
pub(crate) enum IosAction {
    Generate,
    Build,
    Test {
        #[arg(long)]
        ui: bool,
        #[arg(long)]
        floor: bool,
        #[arg(long)]
        only: Vec<String>,
    },
    Lint,
    E2e,
    Archive,
}

pub(crate) fn run(_root: &Path, action: &IosAction) -> Result<()> {
    bail!("xtask ios {}: not built yet", name(action))
}

pub(crate) fn check(_root: &Path) -> Result<()> {
    println!("ios scanner: not built yet");
    Ok(())
}

fn name(action: &IosAction) -> &'static str {
    match action {
        IosAction::Generate => "generate",
        IosAction::Build => "build",
        IosAction::Test { .. } => "test",
        IosAction::Lint => "lint",
        IosAction::E2e => "e2e",
        IosAction::Archive => "archive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ios_command_says_it_is_not_built_yet() {
        for action in [
            IosAction::Generate,
            IosAction::Build,
            IosAction::Test {
                ui: false,
                floor: false,
                only: Vec::new(),
            },
            IosAction::Lint,
            IosAction::E2e,
            IosAction::Archive,
        ] {
            let error = run(Path::new("."), &action).expect_err("placeholder must refuse");
            assert!(error.to_string().ends_with("not built yet"), "{error}");
        }
    }
}
