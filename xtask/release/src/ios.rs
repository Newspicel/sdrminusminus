use std::{path::Path, process::Command};

use anyhow::{Context, Result, ensure};

pub fn changed(root: &Path, base: Option<&str>) -> Result<()> {
    let Some(base) = base else {
        println!("true");
        return Ok(());
    };
    let output = Command::new("git")
        .args(["diff", "--name-only", "-z", base, "HEAD", "--"])
        .current_dir(root)
        .output()
        .context("compare iOS release sources")?;
    ensure!(output.status.success(), "could not compare release {base}");
    let paths = String::from_utf8(output.stdout).context("release paths are not UTF-8")?;
    println!("{}", paths.split('\0').any(affects_ios));
    Ok(())
}

fn affects_ios(path: &str) -> bool {
    [
        "apps/ios/",
        "crates/mobile-core/",
        "crates/wire/",
        "xtask/src/ios/",
        "xtask/src/mobile/",
        ".cargo/",
        ".github/actions/rust/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        || matches!(
            path,
            "Cargo.toml"
                | "Cargo.lock"
                | "rust-toolchain.toml"
                | "xtask/src/ios.rs"
                | "xtask/src/mobile.rs"
                | "xtask/release/src/ios.rs"
                | ".github/workflows/release.yml"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_core_wire_and_build_changes_release_ios() {
        for path in [
            "apps/ios/App/Sources/App/SdrmmApp.swift",
            "crates/mobile-core/src/lib.rs",
            "crates/wire/src/lib.rs",
            "Cargo.lock",
            "Cargo.toml",
            "rust-toolchain.toml",
            "xtask/src/ios/privacy.rs",
            "xtask/src/mobile.rs",
            ".github/workflows/release.yml",
        ] {
            assert!(affects_ios(path), "{path}");
        }
    }

    #[test]
    fn unrelated_changes_skip_ios() {
        for path in [
            "web/src/App.tsx",
            "apps/android/app/build.gradle.kts",
            "crates/engine/src/lib.rs",
            "docs/src/user-guide/phones.md",
            "crates/wireless/src/lib.rs",
            "",
        ] {
            assert!(!affects_ios(path), "{path}");
        }
    }
}
