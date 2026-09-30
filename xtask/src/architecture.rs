use std::{path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

pub(crate) const FORBIDDEN_ON_PHONES: &[&str] = &[
    "sdrmm-server",
    "sdrmm-engine",
    "sdrmm-channels",
    "sdrmm-dsp",
    "sdrmm-modem",
    "sdrmm-device",
    "sdrmm-usb-stream",
    "sdrmm-recorder",
    "codec2",
    "ffmpeg-the-third",
    "ffmpeg-sys-the-third",
    "opus",
    "nusb",
    "libloading",
    "rusqlite",
    "aws-lc-rs",
    "aws-lc-sys",
    "openssl",
    "openssl-sys",
    "wgpu",
];

pub(crate) fn check(root: &Path) -> Result<()> {
    let metadata = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .context("read workspace dependency metadata")?;
    ensure!(
        metadata.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    validate(&serde_json::from_slice(&metadata.stdout)?)?;
    let tree = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "sdrmm",
            "--no-default-features",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}|{f}",
        ])
        .current_dir(root)
        .output()
        .context("inspect production dependencies")?;
    ensure!(
        tree.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let tree = String::from_utf8(tree.stdout)?;
    validate_production_tree(&tree)?;
    check_mobile_trees(root)
}

fn check_mobile_trees(root: &Path) -> Result<()> {
    for target in crate::mobile::phone_targets() {
        validate_phone_tree(target, &crate::mobile::phone_tree(root, target)?)?;
    }
    Ok(())
}

fn validate_phone_tree(target: &str, tree: &str) -> Result<()> {
    let found: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| FORBIDDEN_ON_PHONES.contains(name))
        .collect();
    ensure!(
        found.is_empty(),
        "sdrmm-mobile-core pulls {} into {target}",
        found.join(", ")
    );
    Ok(())
}

fn validate_production_tree(tree: &str) -> Result<()> {
    for name in ["sdrmm-test-support", "sdrmm-modem-test-support"] {
        ensure!(
            !tree
                .lines()
                .any(|line| line.split_whitespace().next() == Some(name)),
            "{name} leaked into the application dependency graph"
        );
    }
    Ok(())
}

fn validate(metadata: &Value) -> Result<()> {
    let packages = metadata["packages"]
        .as_array()
        .context("metadata has no packages")?;
    for (name, allowed) in [
        ("sdrmm-dsp", &[][..]),
        ("sdrmm-modem", &["sdrmm-dsp"][..]),
        (
            "sdrmm-modem-test-support",
            &["sdrmm-dsp", "sdrmm-modem", "sdrmm-test-support"][..],
        ),
        (
            "sdrmm-channels",
            &["sdrmm-dsp", "sdrmm-modem", "sdrmm-wire"][..],
        ),
        ("sdrmm-mobile-core", &["sdrmm-wire"][..]),
    ] {
        let package = packages
            .iter()
            .find(|package| package["name"] == name)
            .with_context(|| format!("missing {name}"))?;
        for dependency in package["dependencies"]
            .as_array()
            .context("package has no dependencies")?
        {
            if !dependency["kind"].is_null() {
                continue;
            }
            let dependency_name = dependency["name"]
                .as_str()
                .context("dependency has no name")?;
            ensure!(
                !dependency_name.starts_with("sdrmm-") || allowed.contains(&dependency_name),
                "{name} must not depend on {dependency_name}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn metadata(dependency: Value) -> Value {
        json!({"packages": [
            {"name": "sdrmm-dsp", "dependencies": [dependency]},
            {"name": "sdrmm-modem", "dependencies": []},
            {"name": "sdrmm-channels", "dependencies": []},
            {"name": "sdrmm-modem-test-support", "dependencies": []},
            {"name": "sdrmm-mobile-core", "dependencies": []}
        ]})
    }

    #[test]
    fn mobile_core_may_only_depend_on_wire() {
        let mut data = metadata(json!({"name": "num-complex", "kind": null}));
        data["packages"][4]["dependencies"] = json!([
            {"name": "sdrmm-wire", "kind": null},
            {"name": "sdrmm-server", "kind": "dev"},
            {"name": "tokio", "kind": null}
        ]);
        assert!(validate(&data).is_ok());
        data["packages"][4]["dependencies"] = json!([{"name": "sdrmm-engine", "kind": null}]);
        assert!(validate(&data).is_err());
    }

    #[test]
    fn forbidden_crates_are_listed() {
        for name in [
            "sdrmm-server",
            "sdrmm-channels",
            "codec2",
            "aws-lc-rs",
            "rusqlite",
            "nusb",
        ] {
            assert!(FORBIDDEN_ON_PHONES.contains(&name), "{name}");
        }
        let clean = "sdrmm-mobile-core v0.1.0 (/repo)\nsdrmm-wire v0.1.0 (/repo)\nring v0.17.14\n";
        assert!(validate_phone_tree("aarch64-apple-ios", clean).is_ok());
        let leaked = format!("{clean}sdrmm-server v0.1.0 (/repo)\naws-lc-rs v1.15.0\n");
        let error = validate_phone_tree("aarch64-linux-android", &leaked).expect_err("refused");
        assert_eq!(
            error.to_string(),
            "sdrmm-mobile-core pulls sdrmm-server, aws-lc-rs into aarch64-linux-android"
        );
    }

    #[test]
    fn core_cannot_depend_on_engine_but_test_dependencies_are_allowed() {
        assert!(validate(&metadata(json!({"name": "sdrmm-engine", "kind": null}))).is_err());
        assert!(
            validate(&metadata(
                json!({"name": "sdrmm-test-support", "kind": "dev"})
            ))
            .is_ok()
        );
    }

    #[test]
    fn application_cannot_include_either_test_support_crate() {
        for name in ["sdrmm-test-support", "sdrmm-modem-test-support"] {
            assert!(validate_production_tree(&format!("sdrmm v0.0.0|\n{name} v0.0.0|")).is_err());
            let mut data = metadata(json!({"name": "num-complex", "kind": null}));
            data["packages"][1]["dependencies"] = json!([{"name": name, "kind": null}]);
            assert!(validate(&data).is_err());
        }
        assert!(validate_production_tree("sdrmm v0.0.0|\nsdrmm-modem v0.0.0|").is_ok());
    }

    #[test]
    fn the_real_workspace_respects_the_boundaries() {
        check(&crate::root()).expect("crate boundaries and production features");
    }
}
