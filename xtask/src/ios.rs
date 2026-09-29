use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

mod e2e;
mod privacy;
mod scan;

pub(crate) const XCODEGEN_VERSION: &str = "2.46.0";
pub(crate) const XCODEGEN_URL: &str =
    "https://github.com/yonaskolb/XcodeGen/releases/download/2.46.0/xcodegen.zip";
pub(crate) const XCODEGEN_SHA256: &str =
    "4d9e34b62172d645eed6457cac13fc222569974098ef4ee9c3368bedf0196806";
const APP_DIR: &str = "apps/ios";
const PROJECT: &str = "apps/ios/SDRmm.xcodeproj";
const SCHEME: &str = "SDRmm";
const XCFRAMEWORK_PLIST: &str = "Core/Artifacts/SdrmmMobileFFI.xcframework/Info.plist";
const DEVICE_LIBRARY: &str =
    "Core/Artifacts/SdrmmMobileFFI.xcframework/ios-arm64/libsdrmm_mobile_core.a";
const SWIFT_BINDINGS: &str = "Core/Generated/SdrmmCore.swift";
const PRIVACY_MANIFEST: &str = "Support/PrivacyInfo.xcprivacy";
const VERSION_XCCONFIG: &str = "Config/Version.xcconfig";
const SIMULATOR_ENV: &str = "SDRMM_IOS_SIMULATOR";
const NEEDS_MAC: &str = "iOS builds need macOS with Xcode 27";
const UNIT_TESTS: &str = "SDRmmTests";
const UI_TESTS: &str = "SDRmmUITests";

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Device {
    Primary,
    Floor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Simulator {
    name: String,
    device_type: &'static str,
    runtime: &'static str,
}

impl Simulator {
    fn of(device: Device, custom: Option<&str>) -> Self {
        match device {
            Device::Primary => Self {
                name: custom.unwrap_or("iPhone 17").to_owned(),
                device_type: "com.apple.CoreSimulator.SimDeviceType.iPhone-17",
                runtime: "com.apple.CoreSimulator.SimRuntime.iOS-27-0",
            },
            Device::Floor => Self {
                name: custom.map_or_else(|| "iPhone 16".to_owned(), |name| format!("{name} 18")),
                device_type: "com.apple.CoreSimulator.SimDeviceType.iPhone-16",
                runtime: "com.apple.CoreSimulator.SimRuntime.iOS-18-1",
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TestRun {
    device: Device,
    label: &'static str,
    selection: Vec<String>,
}

pub(crate) fn run(root: &Path, action: &IosAction) -> Result<()> {
    match action {
        IosAction::Lint => lint(root),
        IosAction::E2e => on_mac(|| e2e(root)),
        IosAction::Generate => on_mac(|| generate(root)),
        IosAction::Build => on_mac(|| build(root)),
        IosAction::Test { ui, floor, only } => on_mac(|| test(root, &test_runs(*ui, *floor, only))),
        IosAction::Archive => on_mac(|| archive(root)),
    }
}

pub(crate) fn check(root: &Path) -> Result<()> {
    report(&scan::scan_tree(root)?)?;
    println!("ios scanner: no comments, em dashes or long files");
    Ok(())
}

fn report(findings: &[scan::Finding]) -> Result<()> {
    if findings.is_empty() {
        return Ok(());
    }
    for found in findings {
        eprintln!("{found}");
    }
    bail!("{} Swift rule findings in apps/ios", findings.len())
}

fn on_mac(work: impl FnOnce() -> Result<()>) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!(NEEDS_MAC);
    }
    work()
}

fn lint(root: &Path) -> Result<()> {
    check(root)?;
    on_mac(|| {
        let app = root.join(APP_DIR);
        crate::run(
            "xcrun",
            &[
                "swift-format",
                "lint",
                "--strict",
                "--recursive",
                "--configuration",
                &display(&app.join(".swift-format")),
                &display(&app.join("App")),
                &display(&app.join("AppTests")),
                &display(&app.join("AppUITests")),
            ],
            root,
        )
    })
}

fn target_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => root.join(dir),
        None => root.join("target"),
    }
}

fn derived_data(root: &Path) -> PathBuf {
    target_dir(root).join("ios/DerivedData")
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn generate(root: &Path) -> Result<()> {
    core_artifacts(root)?;
    write_version(root)?;
    let xcodegen = xcodegen(root)?;
    crate::run(
        &display(&xcodegen),
        &["generate", "--spec", "project.yml", "--project", "."],
        &root.join(APP_DIR),
    )
}

fn core_artifacts(root: &Path) -> Result<()> {
    let app = root.join(APP_DIR);
    crate::mobile::ios_artifacts(root, &app.join("Core"))?;
    for produced in [XCFRAMEWORK_PLIST, SWIFT_BINDINGS] {
        ensure!(
            app.join(produced).is_file(),
            "{produced} missing after the core build"
        );
    }
    Ok(())
}

fn write_version(root: &Path) -> Result<()> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).context("read Cargo.toml")?;
    let version = workspace_version(&manifest)
        .context("Cargo.toml names no version under [workspace.package]")?;
    let path = root.join(APP_DIR).join(VERSION_XCCONFIG);
    let content = format!("MARKETING_VERSION = {version}\n");
    if std::fs::read_to_string(&path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))
}

fn workspace_version(manifest: &str) -> Option<&str> {
    let mut section = "";
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed;
            continue;
        }
        if section != "[workspace.package]" {
            continue;
        }
        let Some(value) = trimmed.strip_prefix("version") else {
            continue;
        };
        let value = value.trim_start().strip_prefix('=')?.trim();
        return value.strip_prefix('"')?.strip_suffix('"');
    }
    None
}

fn xcodegen(root: &Path) -> Result<PathBuf> {
    let cache = target_dir(root)
        .join("tools")
        .join(format!("xcodegen-{XCODEGEN_VERSION}"));
    let binary = cache.join("xcodegen/bin/xcodegen");
    if reports_version(&binary) {
        return Ok(binary);
    }
    std::fs::create_dir_all(&cache).with_context(|| format!("create {}", cache.display()))?;
    let zip = cache.join("xcodegen.zip");
    crate::run("curl", &["-fsSL", "-o", &display(&zip), XCODEGEN_URL], root)?;
    let bytes = std::fs::read(&zip).with_context(|| format!("read {}", zip.display()))?;
    if let Err(error) = verify_sha256(&bytes, XCODEGEN_SHA256) {
        std::fs::remove_file(&zip).with_context(|| format!("remove {}", zip.display()))?;
        return Err(error);
    }
    crate::run(
        "ditto",
        &["-x", "-k", &display(&zip), &display(&cache)],
        root,
    )?;
    ensure!(
        reports_version(&binary),
        "{} does not report version {XCODEGEN_VERSION}",
        binary.display()
    );
    Ok(binary)
}

fn reports_version(binary: &Path) -> bool {
    Command::new(binary)
        .arg("--version")
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(XCODEGEN_VERSION)
        })
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    ensure!(
        actual == expected.to_ascii_lowercase(),
        "xcodegen.zip sha256 is {actual}, expected {expected}"
    );
    Ok(())
}

fn xcodebuild(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<()> {
    let project = display(&root.join(PROJECT));
    let derived = display(&derived_data(root));
    let mut all = vec![
        "-project",
        &project,
        "-scheme",
        SCHEME,
        "-derivedDataPath",
        &derived,
        "-quiet",
    ];
    all.extend_from_slice(args);
    crate::run_with_env("xcodebuild", &all, root, env)
}

fn build(root: &Path) -> Result<()> {
    generate(root)?;
    xcodebuild(
        root,
        &[
            "-configuration",
            "Debug",
            "-destination",
            "generic/platform=iOS Simulator",
            "build-for-testing",
        ],
        &[],
    )
}

fn test_runs(ui: bool, floor: bool, only: &[String]) -> Vec<TestRun> {
    let run = |device, label, selection: &[String]| TestRun {
        device,
        label,
        selection: selection.to_vec(),
    };
    if !only.is_empty() {
        let mut runs = vec![run(Device::Primary, "only", only)];
        if floor {
            runs.push(run(Device::Floor, "floor", only));
        }
        return runs;
    }
    let unit = [UNIT_TESTS.to_owned()];
    let mut runs = vec![run(Device::Primary, "unit", &unit)];
    if ui {
        runs.push(run(Device::Primary, "ui", &[UI_TESTS.to_owned()]));
    }
    if floor {
        runs.push(run(Device::Floor, "floor", &unit));
    }
    runs
}

fn test(root: &Path, runs: &[TestRun]) -> Result<()> {
    build(root)?;
    let custom = std::env::var(SIMULATOR_ENV).ok();
    for run in runs {
        let udid = ensure_simulator(&Simulator::of(run.device, custom.as_deref()))?;
        run_tests(root, &udid, run.label, &run.selection, &[])?.require_tests(&run.selection)?;
    }
    Ok(())
}

fn e2e(root: &Path) -> Result<()> {
    build(root)?;
    let custom = std::env::var(SIMULATOR_ENV).ok();
    let udid = ensure_simulator(&Simulator::of(Device::Primary, custom.as_deref()))?;
    let server = e2e::Server::start(root, &target_dir(root))?;
    let link = server.pairing_link(root)?;
    println!("pairing link {link}");
    let selection = [e2e::SELECTION.to_owned()];
    let counts = run_tests(root, &udid, "e2e", &selection, &[(e2e::LINK_ENV, &link)])?;
    drop(server);
    counts.require_passed(&selection)
}

fn run_tests(
    root: &Path,
    udid: &str,
    label: &str,
    selection: &[String],
    env: &[(&str, &str)],
) -> Result<Counts> {
    let results = target_dir(root).join("ios/results");
    std::fs::create_dir_all(&results).with_context(|| format!("create {}", results.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("clock before 1970")?
        .as_secs();
    let destination = format!("platform=iOS Simulator,id={udid}");
    let bundle = display(&results.join(format!("{label}-{stamp}.xcresult")));
    let only: Vec<String> = selection
        .iter()
        .map(|selected| format!("-only-testing:{selected}"))
        .collect();
    let mut args = vec![
        "-destination",
        &destination,
        "-resultBundlePath",
        &bundle,
        "-parallel-testing-enabled",
        "NO",
        "-collect-test-diagnostics",
        "never",
    ];
    args.extend(only.iter().map(String::as_str));
    args.push("test-without-building");
    let ran = xcodebuild(root, &args, env);
    let counts = results_summary(&bundle);
    if let Ok(counts) = &counts {
        println!("{label}: {counts}");
    }
    ran?;
    counts
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Counts {
    passed: u64,
    failed: u64,
    skipped: u64,
}

impl Counts {
    fn from_summary(summary: &serde_json::Value) -> Result<Self> {
        let count = |key: &str| {
            summary[key]
                .as_u64()
                .with_context(|| format!("xcresulttool summary has no `{key}`"))
        };
        Ok(Self {
            passed: count("passedTests")?,
            failed: count("failedTests")?,
            skipped: count("skippedTests")?,
        })
    }

    fn require_tests(self, selection: &[String]) -> Result<()> {
        ensure!(
            self.passed + self.failed + self.skipped > 0,
            "no tests ran for {}",
            selection.join(", ")
        );
        Ok(())
    }

    fn require_passed(self, selection: &[String]) -> Result<()> {
        ensure!(
            self.passed > 0 && self.failed == 0 && self.skipped == 0,
            "{} did not pass: {self}",
            selection.join(", ")
        );
        Ok(())
    }
}

impl std::fmt::Display for Counts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} passed, {} failed, {} skipped",
            self.passed, self.failed, self.skipped
        )
    }
}

fn results_summary(bundle: &str) -> Result<Counts> {
    let output = Command::new("xcrun")
        .args([
            "xcresulttool",
            "get",
            "test-results",
            "summary",
            "--path",
            bundle,
        ])
        .output()
        .context("failed to spawn `xcrun xcresulttool`")?;
    ensure!(
        output.status.success(),
        "`xcresulttool` could not read {bundle}"
    );
    let summary: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parse `xcresulttool` output")?;
    Counts::from_summary(&summary)
}

fn ensure_simulator(simulator: &Simulator) -> Result<String> {
    let output = Command::new("xcrun")
        .args(["simctl", "list", "devices", "available", "-j"])
        .output()
        .context("failed to spawn `xcrun simctl list`")?;
    ensure!(output.status.success(), "`xcrun simctl list` failed");
    let listing: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parse `simctl list` output")?;
    if let Some(udid) = find_device(&listing, simulator) {
        return Ok(udid);
    }
    println!(
        "$ xcrun simctl create \"{}\" {} {}",
        simulator.name, simulator.device_type, simulator.runtime
    );
    let created = Command::new("xcrun")
        .args([
            "simctl",
            "create",
            &simulator.name,
            simulator.device_type,
            simulator.runtime,
        ])
        .output()
        .context("failed to spawn `xcrun simctl create`")?;
    ensure!(
        created.status.success(),
        "`xcrun simctl create` failed: {}",
        String::from_utf8_lossy(&created.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&created.stdout).trim().to_owned())
}

fn find_device(listing: &serde_json::Value, simulator: &Simulator) -> Option<String> {
    listing["devices"][simulator.runtime]
        .as_array()?
        .iter()
        .find(|device| device["name"].as_str() == Some(simulator.name.as_str()))
        .and_then(|device| device["udid"].as_str())
        .map(str::to_owned)
}

fn archive(root: &Path) -> Result<()> {
    generate(root)?;
    let app = root.join(APP_DIR);
    let output = Command::new("nm")
        .arg("-u")
        .arg(app.join(DEVICE_LIBRARY))
        .output()
        .context("failed to spawn `nm`")?;
    ensure!(
        output.status.success(),
        "`nm -u` failed on the core library"
    );
    let required = privacy::required_categories(&String::from_utf8_lossy(&output.stdout));
    let manifest = std::fs::read_to_string(app.join(PRIVACY_MANIFEST))
        .with_context(|| format!("read {PRIVACY_MANIFEST}"))?;
    privacy::check_manifest(&required, &manifest)?;
    let archive = display(&target_dir(root).join("ios/SDRmm.xcarchive"));
    xcodebuild(
        root,
        &[
            "-configuration",
            "Release",
            "-destination",
            "generic/platform=iOS",
            "-archivePath",
            &archive,
            "-allowProvisioningUpdates",
            "archive",
        ],
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_mismatch_is_an_error() {
        let error = verify_sha256(b"xcodegen", XCODEGEN_SHA256).expect_err("wrong digest");
        assert!(error.to_string().contains(XCODEGEN_SHA256), "{error}");
    }

    #[test]
    fn sha256_match_passes() {
        verify_sha256(
            b"abc",
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
        )
        .expect("digest of abc");
    }

    #[test]
    fn workspace_version_is_read_from_its_section() {
        let manifest = "[package]\nversion = \"9.9.9\"\n[workspace.package]\nedition = \"2024\"\nversion = \"1.2.3\"\n";
        assert_eq!(workspace_version(manifest), Some("1.2.3"));
        assert_eq!(workspace_version("[workspace]\nversion = \"1\"\n"), None);
    }

    #[test]
    fn default_runs_are_unit_then_ui_then_floor() {
        let runs: Vec<_> = test_runs(true, true, &[])
            .into_iter()
            .map(|run| (run.device, run.label, run.selection))
            .collect();
        assert_eq!(
            runs,
            vec![
                (Device::Primary, "unit", vec![UNIT_TESTS.to_owned()]),
                (Device::Primary, "ui", vec![UI_TESTS.to_owned()]),
                (Device::Floor, "floor", vec![UNIT_TESTS.to_owned()]),
            ]
        );
        assert_eq!(test_runs(false, false, &[]).len(), 1);
    }

    #[test]
    fn only_replaces_the_default_selection() {
        let only = vec![
            "SDRmmTests/KeychainVaultTests".to_owned(),
            "SDRmmTests/AppLaunchTests".to_owned(),
        ];
        let runs = test_runs(true, false, &only);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].selection, only);
        assert_eq!(test_runs(false, true, &only)[1].device, Device::Floor);
    }

    #[test]
    fn a_custom_simulator_name_keeps_device_type_and_runtime() {
        let primary = Simulator::of(Device::Primary, Some("SDRmm-X"));
        assert_eq!(
            primary,
            Simulator {
                name: "SDRmm-X".to_owned(),
                ..Simulator::of(Device::Primary, None)
            }
        );
        assert_eq!(
            Simulator::of(Device::Floor, Some("SDRmm-X")).name,
            "SDRmm-X 18"
        );
        assert_eq!(Simulator::of(Device::Floor, None).name, "iPhone 16");
    }

    #[test]
    fn find_device_matches_name_within_runtime() {
        let simulator = Simulator::of(Device::Primary, None);
        let listing = serde_json::json!({
            "devices": {
                "com.apple.CoreSimulator.SimRuntime.iOS-18-1": [{"name": "iPhone 17", "udid": "OLD"}],
                "com.apple.CoreSimulator.SimRuntime.iOS-27-0": [
                    {"name": "iPhone 17 Pro", "udid": "PRO"},
                    {"name": "iPhone 17", "udid": "NEW"}
                ]
            }
        });
        assert_eq!(find_device(&listing, &simulator), Some("NEW".to_owned()));
        assert_eq!(find_device(&serde_json::json!({}), &simulator), None);
    }

    #[test]
    fn result_summary_counts_tests() {
        let summary = serde_json::json!({"passedTests": 62, "failedTests": 1, "skippedTests": 2});
        let counts = Counts::from_summary(&summary).expect("counts");
        assert_eq!(counts.to_string(), "62 passed, 1 failed, 2 skipped");
        let error =
            Counts::from_summary(&serde_json::json!({"passedTests": 1})).expect_err("missing keys");
        assert!(error.to_string().contains("failedTests"), "{error}");
    }

    #[test]
    fn a_selection_that_runs_nothing_fails() {
        let none = Counts {
            passed: 0,
            failed: 0,
            skipped: 0,
        };
        let error = none
            .require_tests(&["SDRmmTests/Typo".to_owned()])
            .expect_err("empty run");
        assert!(error.to_string().contains("SDRmmTests/Typo"), "{error}");
        Counts { skipped: 1, ..none }
            .require_tests(&[])
            .expect("a skipped test still ran");
    }

    #[test]
    fn e2e_needs_every_selected_test_to_pass() {
        let selection = [e2e::SELECTION.to_owned()];
        let passed = Counts {
            passed: 1,
            failed: 0,
            skipped: 0,
        };
        passed.require_passed(&selection).expect("passed");
        for counts in [
            Counts {
                skipped: 1,
                ..passed
            },
            Counts {
                failed: 1,
                ..passed
            },
            Counts {
                passed: 0,
                ..passed
            },
        ] {
            let error = counts.require_passed(&selection).expect_err("not passed");
            assert!(error.to_string().contains(e2e::SELECTION), "{error}");
        }
    }
}
