use std::path::Path;

use anyhow::{Context, Result, ensure};

pub fn set(root: &Path, version: &str) -> Result<()> {
    let version = version.strip_prefix('v').unwrap_or(version);
    validate(version)?;
    let manifest = root.join("Cargo.toml");
    let lock = root.join("Cargo.lock");
    let members = members(&read(&lock)?);
    write(&manifest, &stamp_manifest(&read(&manifest)?, version)?)?;
    write(&lock, &stamp_lock(&read(&lock)?, &members, version))?;
    println!("version: {version}");
    Ok(())
}

fn validate(version: &str) -> Result<()> {
    let parts: Vec<&str> = version.split('.').collect();
    let numeric: Vec<u64> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    ensure!(
        parts.len() == 3 && numeric.len() == 3,
        "`{version}` is not a plain major.minor.patch version, e.g. 0.2.0. \
         Suffixes are not usable: the Windows MSI bundler cannot express one."
    );
    for (value, limit, field) in [
        (numeric[0], 255, "major"),
        (numeric[1], 255, "minor"),
        (numeric[2], 65_535, "patch"),
    ] {
        ensure!(
            value <= limit,
            "`{version}` has a {field} of {value}: the Windows MSI bundler caps it at {limit}"
        );
    }
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

fn stamp_manifest(manifest: &str, version: &str) -> Result<String> {
    let mut section = "";
    let mut hits = 0;
    let mut out = String::with_capacity(manifest.len());
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed;
        }
        if section == "[workspace.package]" && trimmed.starts_with("version") {
            out.push_str(&format!("version = \"{version}\"\n"));
            hits += 1;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    ensure!(
        hits == 1,
        "expected exactly one `version` under [workspace.package] in Cargo.toml, found {hits}"
    );
    Ok(out)
}

fn members(lock: &str) -> Vec<String> {
    lock.split("[[package]]")
        .skip(1)
        .filter(|block| !block.contains("\nsource = "))
        .filter_map(|block| block.lines().find_map(|line| line.strip_prefix("name = ")))
        .map(str::to_owned)
        .collect()
}

fn stamp_lock(lock: &str, members: &[String], version: &str) -> String {
    let mut out = String::with_capacity(lock.len());
    let mut local = false;
    for line in lock.lines() {
        if let Some(name) = line.strip_prefix("name = ") {
            local = members.iter().any(|member| member == name);
        }
        if local && line.starts_with("version = ") {
            out.push_str(&format!("version = \"{version}\"\n"));
            local = false;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = "version = 4\n\n[[package]]\nname = \"anyhow\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"sdrmm\"\nversion = \"0.9.0\"\ndependencies = [\n \"anyhow\",\n]\n";

    #[test]
    fn stamps_only_local_packages_in_the_lock() {
        let stamped = stamp_lock(LOCK, &members(LOCK), "2.0.0");
        assert_eq!(stamped, LOCK.replace("0.9.0", "2.0.0"));
    }

    #[test]
    fn stamps_the_workspace_package_version() {
        let manifest =
            "[package]\nversion = \"1.0.0\"\n\n[workspace.package]\nversion = \"0.9.0\"\n";
        let stamped = stamp_manifest(manifest, "2.0.0").unwrap();
        assert_eq!(stamped, manifest.replace("0.9.0", "2.0.0"));
    }

    #[test]
    fn rejects_suffixes_and_msi_overflow() {
        assert!(validate("2.0.0").is_ok());
        assert!(validate("2.0.0-rc.1").is_err());
        assert!(validate("256.0.0").is_err());
    }
}
