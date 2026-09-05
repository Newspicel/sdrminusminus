use std::{path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

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
            {"name": "sdrmm-modem-test-support", "dependencies": []}
        ]})
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
