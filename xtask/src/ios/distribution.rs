use std::path::Path;

use anyhow::{Result, ensure};

pub(super) fn version_config(version: &str, build: Option<&str>) -> Result<String> {
    let mut content = format!("MARKETING_VERSION = {version}\n");
    if let Some(build) = build {
        let parts: Vec<&str> = build.split('.').collect();
        ensure!(
            (1..=3).contains(&parts.len())
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                && parts[0].parse::<u64>().is_ok_and(|value| value > 0),
            "SDRMM_IOS_BUILD_NUMBER must contain one to three numeric components"
        );
        content.push_str(&format!("CURRENT_PROJECT_VERSION = {build}\n"));
    }
    Ok(content)
}

pub(super) fn authentication() -> Result<Vec<String>> {
    let names = [
        ("APP_STORE_CONNECT_KEY_PATH", "-authenticationKeyPath"),
        ("APP_STORE_CONNECT_KEY_ID", "-authenticationKeyID"),
        ("APP_STORE_CONNECT_ISSUER_ID", "-authenticationKeyIssuerID"),
    ];
    let values: Vec<_> = names
        .iter()
        .map(|(name, flag)| (std::env::var(name).ok().filter(|v| !v.is_empty()), *flag))
        .collect();
    let count = values.iter().filter(|(value, _)| value.is_some()).count();
    ensure!(
        count == 0 || count == 3,
        "provide all three App Store Connect key settings"
    );
    Ok(values
        .into_iter()
        .filter_map(|(value, flag)| value.map(|value| [flag.to_owned(), value]))
        .flatten()
        .collect())
}

pub(super) fn upload(root: &Path) -> Result<()> {
    let archive = super::target_dir(root).join("ios/SDRmm.xcarchive");
    ensure!(archive.is_dir(), "run `cargo xtask ios archive` first");
    let output = super::target_dir(root).join("ios/upload");
    let options = root.join("apps/ios/Support/ExportOptions.plist");
    let mut args = vec![
        "-exportArchive".to_owned(),
        "-archivePath".to_owned(),
        super::display(&archive),
        "-exportPath".to_owned(),
        super::display(&output),
        "-exportOptionsPlist".to_owned(),
        super::display(&options),
        "-allowProvisioningUpdates".to_owned(),
    ];
    args.extend(authentication()?);
    crate::run(
        "xcodebuild",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        root,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_version_and_build_are_independent() {
        assert_eq!(
            version_config("2.1.0", Some("42.2")).unwrap(),
            "MARKETING_VERSION = 2.1.0\nCURRENT_PROJECT_VERSION = 42.2\n"
        );
        assert_eq!(
            version_config("2.1.0", None).unwrap(),
            "MARKETING_VERSION = 2.1.0\n"
        );
    }

    #[test]
    fn invalid_build_numbers_cannot_inject_settings() {
        for build in [
            "",
            "0",
            "1.2.3.4",
            "1..2",
            "1beta",
            "1\nCODE_SIGNING_ALLOWED = NO",
        ] {
            assert!(version_config("2.1.0", Some(build)).is_err(), "{build}");
        }
    }
}
