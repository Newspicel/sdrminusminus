use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};

pub(crate) const CRATE: &str = "sdrmm-mobile-core";
pub(crate) const LIB: &str = "sdrmm_mobile_core";
pub(crate) const PROFILE: &str = "mobile";
pub(crate) const IOS_TARGETS: [&str; 2] = ["aarch64-apple-ios", "aarch64-apple-ios-sim"];
pub(crate) const IOS_DEPLOYMENT_TARGET: &str = "18.0";
pub(crate) const ANDROID_API: u32 = 29;
pub(crate) const MIN_NDK_MAJOR: u32 = 28;
pub(crate) const NDK_PACKAGE: &str = "ndk;30.0.16248370";
const PAGE_SIZE_FLAGS: &str = "-C link-arg=-Wl,-z,max-page-size=16384";
const SWIFT_MODULE: &str = "SdrmmCore";
const FFI_MODULE: &str = "SdrmmMobileFFI";
const XCFRAMEWORK: &str = "SdrmmMobileFFI.xcframework";
const KOTLIN_PACKAGE_DIR: &str = "dev/newspicel/sdrmm/ffi";
const LINKED_FRAMEWORKS: &[&str] = &["Security"];

pub(crate) struct AndroidAbi {
    pub(crate) abi: &'static str,
    pub(crate) triple: &'static str,
}

pub(crate) const ANDROID_ABIS: [AndroidAbi; 2] = [
    AndroidAbi {
        abi: "arm64-v8a",
        triple: "aarch64-linux-android",
    },
    AndroidAbi {
        abi: "x86_64",
        triple: "x86_64-linux-android",
    },
];

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

pub(crate) fn run(root: &Path, args: &Mobile) -> Result<()> {
    let layout = Layout::find(root)?;
    match &args.what {
        MobileWhat::Bindings => execute(root, &bindings_plan(&layout)),
        MobileWhat::Ios => {
            require_xcode()?;
            let ios = layout.mobile().join("ios");
            execute(
                root,
                &ios_plan(
                    &layout,
                    &ios.join(XCFRAMEWORK),
                    &layout.mobile().join("bindings/swift"),
                ),
            )
        }
        MobileWhat::Android { out } => {
            let ndk = find_ndk(&|name| std::env::var(name).ok(), &home()?)?;
            println!("NDK r{} at {}", ndk.major, ndk.root.display());
            let out = out
                .clone()
                .unwrap_or_else(|| layout.mobile().join("android"));
            execute(root, &android_plan(&layout, &ndk, &out))
        }
        MobileWhat::Check { ios, android } => {
            let both = !ios && !android;
            if *ios || both {
                require_xcode()?;
                execute(root, &ios_check_plan())?;
            }
            if *android || both {
                let ndk = find_ndk(&|name| std::env::var(name).ok(), &home()?)?;
                execute(root, &android_check_plan(&ndk))?;
            }
            Ok(())
        }
    }
}

#[expect(dead_code)]
pub(crate) fn ios_artifacts(root: &Path, out_dir: &Path) -> Result<()> {
    require_xcode()?;
    execute(root, &ios_artifacts_plan(&Layout::find(root)?, out_dir))
}

pub(crate) fn check(root: &Path) -> Result<()> {
    let expected = crate::licenses::mobile_notices(root)?;
    let path = root.join(crate::licenses::MOBILE_NOTICES_JSON);
    let committed =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    ensure!(
        committed == expected,
        "mobile notices drift: regenerate with `cargo xtask licenses` and commit"
    );
    println!("mobile gate: phone notices current");
    Ok(())
}

pub(crate) fn phone_targets() -> impl Iterator<Item = &'static str> {
    IOS_TARGETS
        .into_iter()
        .chain(ANDROID_ABIS.iter().map(|abi| abi.triple))
}

pub(crate) fn phone_tree(root: &Path, target: &str) -> Result<String> {
    let output = Command::new("cargo")
        .args([
            "tree",
            "-p",
            CRATE,
            "--target",
            target,
            "-e",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .current_dir(root)
        .output()
        .context("failed to spawn `cargo tree`")?;
    ensure!(
        output.status.success(),
        "`cargo tree --target {target}` failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).context("`cargo tree` printed non-utf8")
}

pub(crate) fn phone_packages(root: &Path) -> Result<BTreeSet<(String, String)>> {
    let mut packages = BTreeSet::new();
    for target in phone_targets() {
        packages.extend(tree_packages(&phone_tree(root, target)?));
    }
    Ok(packages)
}

fn tree_packages(tree: &str) -> impl Iterator<Item = (String, String)> + '_ {
    tree.lines().filter_map(|line| {
        let mut words = line.split_whitespace();
        let name = words.next()?;
        let version = words.next()?.strip_prefix('v')?;
        Some((name.to_owned(), version.to_owned()))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Target(&'static str),
    Cargo {
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    Tool {
        program: &'static str,
        args: Vec<String>,
    },
    Clean(PathBuf),
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
}

#[derive(Debug, Default)]
pub(crate) struct Plan {
    steps: Vec<Step>,
    outputs: Vec<PathBuf>,
}

fn execute(root: &Path, plan: &Plan) -> Result<()> {
    for step in &plan.steps {
        match step {
            Step::Target(triple) => crate::ensure_target(root, Some(triple))?,
            Step::Cargo { args, env } => {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let env: Vec<(&str, &str)> = env
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect();
                crate::run_with_env("cargo", &args, root, &env)?;
            }
            Step::Tool { program, args } => {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                crate::run(program, &args, root)?;
            }
            Step::Clean(path) => {
                if path.exists() {
                    std::fs::remove_dir_all(path)
                        .with_context(|| format!("remove {}", path.display()))?;
                }
            }
            Step::Copy { from, to } => {
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("create {}", parent.display()))?;
                }
                std::fs::copy(from, to)
                    .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
            }
        }
    }
    for output in &plan.outputs {
        ensure!(output.exists(), "{} was not produced", output.display());
        println!("wrote {}", output.display());
    }
    Ok(())
}

pub(crate) struct Layout {
    target_dir: PathBuf,
}

impl Layout {
    fn find(root: &Path) -> Result<Self> {
        let output = Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .current_dir(root)
            .output()
            .context("failed to spawn `cargo metadata`")?;
        ensure!(
            output.status.success(),
            "`cargo metadata` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let metadata: serde_json::Value =
            serde_json::from_slice(&output.stdout).context("parse `cargo metadata` output")?;
        let target_dir = metadata["target_directory"]
            .as_str()
            .context("`cargo metadata` names no target directory")?;
        Ok(Self {
            target_dir: PathBuf::from(target_dir),
        })
    }

    fn mobile(&self) -> PathBuf {
        self.target_dir.join("mobile")
    }

    fn host_library(&self) -> PathBuf {
        self.target_dir.join("debug").join(format!(
            "{}{LIB}{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        ))
    }

    fn built(&self, triple: &str, file: &str) -> PathBuf {
        self.target_dir.join(triple).join(PROFILE).join(file)
    }

    fn headers(&self) -> PathBuf {
        self.mobile().join("ios/include")
    }
}

fn strings<const N: usize>(items: [&str; N]) -> Vec<String> {
    items.into_iter().map(str::to_owned).collect()
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn host_library_step() -> Step {
    Step::Cargo {
        args: strings(["rustc", "-p", CRATE, "--lib", "--crate-type", "cdylib"]),
        env: Vec::new(),
    }
}

fn bindgen(bin: &str, args: Vec<String>) -> Step {
    let mut all = strings(["run", "--quiet", "-p", "xtask", "--bin"]);
    all.push(bin.to_owned());
    all.push("--".to_owned());
    all.extend(args);
    Step::Cargo {
        args: all,
        env: Vec::new(),
    }
}

fn swift_sources_step(layout: &Layout, out_dir: &Path) -> Step {
    bindgen(
        "uniffi-bindgen-swift",
        vec![
            display(&layout.host_library()),
            display(out_dir),
            "--swift-sources".to_owned(),
        ],
    )
}

fn swift_headers_step(layout: &Layout) -> Step {
    let mut args = vec![
        display(&layout.host_library()),
        display(&layout.headers()),
        "--headers".to_owned(),
        "--modulemap".to_owned(),
        "--module-name".to_owned(),
        FFI_MODULE.to_owned(),
        "--modulemap-filename".to_owned(),
        "module.modulemap".to_owned(),
    ];
    for framework in LINKED_FRAMEWORKS {
        args.push("--link-frameworks".to_owned());
        args.push((*framework).to_owned());
    }
    bindgen("uniffi-bindgen-swift", args)
}

fn kotlin_step(layout: &Layout, out_dir: &Path) -> Step {
    bindgen(
        "uniffi-bindgen",
        vec![
            "generate".to_owned(),
            "--language".to_owned(),
            "kotlin".to_owned(),
            "--out-dir".to_owned(),
            display(out_dir),
            "--no-format".to_owned(),
            display(&layout.host_library()),
        ],
    )
}

fn swift_output(dir: &Path) -> PathBuf {
    dir.join(format!("{SWIFT_MODULE}.swift"))
}

fn kotlin_output(dir: &Path) -> PathBuf {
    dir.join(KOTLIN_PACKAGE_DIR).join(format!("{LIB}.kt"))
}

pub(crate) fn bindings_plan(layout: &Layout) -> Plan {
    let swift = layout.mobile().join("bindings/swift");
    let kotlin = layout.mobile().join("bindings/kotlin");
    Plan {
        steps: vec![
            host_library_step(),
            swift_sources_step(layout, &swift),
            kotlin_step(layout, &kotlin),
        ],
        outputs: vec![swift_output(&swift), kotlin_output(&kotlin)],
    }
}

pub(crate) fn ios_artifacts_plan(layout: &Layout, out_dir: &Path) -> Plan {
    ios_plan(
        layout,
        &out_dir.join("Artifacts").join(XCFRAMEWORK),
        &out_dir.join("Generated"),
    )
}

fn ios_plan(layout: &Layout, xcframework: &Path, swift_dir: &Path) -> Plan {
    let mut steps = vec![
        host_library_step(),
        swift_sources_step(layout, swift_dir),
        Step::Clean(layout.headers()),
        swift_headers_step(layout),
    ];
    let mut libraries = Vec::new();
    for triple in IOS_TARGETS {
        steps.push(Step::Target(triple));
        steps.push(Step::Cargo {
            args: strings([
                "rustc",
                "-p",
                CRATE,
                "--lib",
                "--crate-type",
                "staticlib",
                "--profile",
                PROFILE,
                "--target",
                triple,
            ]),
            env: ios_env(),
        });
        libraries.push(layout.built(triple, &format!("lib{LIB}.a")));
    }
    steps.push(Step::Clean(xcframework.to_path_buf()));
    steps.push(Step::Tool {
        program: "xcodebuild",
        args: xcframework_args(&libraries[0], &libraries[1], &layout.headers(), xcframework),
    });
    Plan {
        steps,
        outputs: vec![xcframework.to_path_buf(), swift_output(swift_dir)],
    }
}

fn ios_check_plan() -> Plan {
    let mut steps = Vec::new();
    for triple in IOS_TARGETS {
        steps.push(Step::Target(triple));
        steps.push(clippy_step(triple, ios_env()));
    }
    Plan {
        steps,
        outputs: Vec::new(),
    }
}

fn android_plan(layout: &Layout, ndk: &Ndk, out_dir: &Path) -> Plan {
    let kotlin = out_dir.join("kotlin");
    let mut steps = vec![host_library_step(), kotlin_step(layout, &kotlin)];
    let mut outputs = vec![kotlin_output(&kotlin)];
    let library = format!("lib{LIB}.so");
    for abi in &ANDROID_ABIS {
        steps.push(Step::Target(abi.triple));
        steps.push(Step::Cargo {
            args: strings([
                "rustc",
                "-p",
                CRATE,
                "--lib",
                "--crate-type",
                "cdylib",
                "--profile",
                PROFILE,
                "--target",
                abi.triple,
            ]),
            env: ndk_env(ndk, abi),
        });
        let to = out_dir.join("jniLibs").join(abi.abi).join(&library);
        steps.push(Step::Copy {
            from: layout.built(abi.triple, &library),
            to: to.clone(),
        });
        outputs.push(to);
    }
    Plan { steps, outputs }
}

fn android_check_plan(ndk: &Ndk) -> Plan {
    let mut steps = Vec::new();
    for abi in &ANDROID_ABIS {
        steps.push(Step::Target(abi.triple));
        steps.push(clippy_step(abi.triple, ndk_env(ndk, abi)));
    }
    Plan {
        steps,
        outputs: Vec::new(),
    }
}

fn clippy_step(triple: &str, env: Vec<(String, String)>) -> Step {
    Step::Cargo {
        args: strings([
            "clippy", "-p", CRATE, "--lib", "--target", triple, "--", "-D", "warnings",
        ]),
        env,
    }
}

fn ios_env() -> Vec<(String, String)> {
    vec![(
        "IPHONEOS_DEPLOYMENT_TARGET".to_owned(),
        IOS_DEPLOYMENT_TARGET.to_owned(),
    )]
}

pub(crate) fn xcframework_args(
    device: &Path,
    simulator: &Path,
    headers: &Path,
    out: &Path,
) -> Vec<String> {
    vec![
        "-create-xcframework".to_owned(),
        "-library".to_owned(),
        display(device),
        "-headers".to_owned(),
        display(headers),
        "-library".to_owned(),
        display(simulator),
        "-headers".to_owned(),
        display(headers),
        "-output".to_owned(),
        display(out),
    ]
}

fn require_xcode() -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("iOS builds need macOS and Xcode");
    }
    let found = Command::new("xcrun")
        .args(["--sdk", "iphoneos", "--show-sdk-path"])
        .output()
        .is_ok_and(|output| output.status.success());
    ensure!(
        found,
        "iOS builds need Xcode with the iOS SDK (xcrun --sdk iphoneos)"
    );
    Ok(())
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

#[derive(Debug)]
pub(crate) struct Ndk {
    pub(crate) root: PathBuf,
    pub(crate) major: u32,
    pub(crate) host_tag: &'static str,
}

pub(crate) fn find_ndk(var: &dyn Fn(&str) -> Option<String>, home: &Path) -> Result<Ndk> {
    let root = match ["ANDROID_NDK_HOME", "NDK_HOME"].into_iter().find_map(var) {
        Some(root) => PathBuf::from(root),
        None => sdk_ndk_dirs(var, home)
            .iter()
            .find_map(|dir| newest_version_dir(dir))
            .with_context(|| {
                format!(
                    "no Android NDK: set ANDROID_NDK_HOME or ANDROID_HOME, or install one with \
                     `sdkmanager \"{NDK_PACKAGE}\"`"
                )
            })?,
    };
    let properties = root.join("source.properties");
    let text = std::fs::read_to_string(&properties).with_context(|| {
        format!(
            "{} is not an NDK: install one with `sdkmanager \"{NDK_PACKAGE}\"`",
            root.display()
        )
    })?;
    let major = ndk_major(&text)
        .with_context(|| format!("{} names no Pkg.Revision", properties.display()))?;
    ensure!(
        major >= MIN_NDK_MAJOR,
        "NDK r{major} is too old: install r30 with `sdkmanager \"{NDK_PACKAGE}\"`"
    );
    Ok(Ndk {
        root,
        major,
        host_tag: host_tag()?,
    })
}

fn sdk_ndk_dirs(var: &dyn Fn(&str) -> Option<String>, home: &Path) -> Vec<PathBuf> {
    ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .into_iter()
        .filter_map(var)
        .map(|sdk| Path::new(&sdk).join("ndk"))
        .chain([
            home.join("Library/Android/sdk/ndk"),
            home.join("Android/Sdk/ndk"),
        ])
        .collect()
}

fn newest_version_dir(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let version = version_key(&entry.file_name().to_string_lossy())?;
            Some((version, entry.path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

fn version_key(name: &str) -> Option<Vec<u64>> {
    name.split('.').map(|part| part.parse().ok()).collect()
}

pub(crate) fn ndk_major(source_properties: &str) -> Option<u32> {
    source_properties.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "Pkg.Revision" {
            return None;
        }
        value.trim().split('.').next()?.parse().ok()
    })
}

fn host_tag() -> Result<&'static str> {
    match std::env::consts::OS {
        "macos" => Ok("darwin-x86_64"),
        "linux" => Ok("linux-x86_64"),
        os => bail!("Android builds need a macOS or Linux host, not {os}"),
    }
}

pub(crate) fn ndk_env(ndk: &Ndk, abi: &AndroidAbi) -> Vec<(String, String)> {
    let bin = ndk
        .root
        .join("toolchains/llvm/prebuilt")
        .join(ndk.host_tag)
        .join("bin");
    let upper = abi.triple.to_uppercase().replace('-', "_");
    let lower = abi.triple.replace('-', "_");
    let clang = display(&bin.join(format!("{}{ANDROID_API}-clang", abi.triple)));
    vec![
        (format!("CARGO_TARGET_{upper}_LINKER"), clang.clone()),
        (format!("CC_{lower}"), clang),
        (format!("AR_{lower}"), display(&bin.join("llvm-ar"))),
        (
            format!("CARGO_TARGET_{upper}_RUSTFLAGS"),
            PAGE_SIZE_FLAGS.to_owned(),
        ),
        ("ANDROID_NDK_HOME".to_owned(), display(&ndk.root)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout {
            target_dir: PathBuf::from("/repo/target"),
        }
    }

    fn ndk() -> Ndk {
        Ndk {
            root: PathBuf::from("/sdk/ndk/30.0.16248370"),
            major: 30,
            host_tag: "darwin-x86_64",
        }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("sdrmm-xtask-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }
    }

    impl std::ops::Deref for Scratch {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fake_ndk(dir: &Path, revision: &str) {
        std::fs::create_dir_all(dir).expect("ndk dir");
        std::fs::write(
            dir.join("source.properties"),
            format!("Pkg.Desc = Android NDK\nPkg.Revision = {revision}\n"),
        )
        .expect("properties");
    }

    fn step_args(plan: &Plan, first: &str) -> Vec<Vec<String>> {
        plan.steps
            .iter()
            .filter_map(|step| match step {
                Step::Cargo { args, .. } | Step::Tool { args, .. }
                    if args.iter().any(|arg| arg == first) =>
                {
                    Some(args.clone())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ndk_major_parses_source_properties() {
        let text = "Pkg.Desc = Android NDK\nPkg.Revision = 30.0.16248370\nPkg.ReleaseName = r30\n";
        assert_eq!(ndk_major(text), Some(30));
        assert_eq!(ndk_major("Pkg.Revision=28.1.13356709"), Some(28));
        assert_eq!(ndk_major("Pkg.Desc = Android NDK"), None);
        assert_eq!(ndk_major("Pkg.Revision = r30"), None);
    }

    #[test]
    fn ndk_env_names_the_linker() {
        let env = ndk_env(&ndk(), &ANDROID_ABIS[0]);
        let bin = "/sdk/ndk/30.0.16248370/toolchains/llvm/prebuilt/darwin-x86_64/bin";
        let expected = [
            (
                "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER",
                format!("{bin}/aarch64-linux-android29-clang"),
            ),
            (
                "CC_aarch64_linux_android",
                format!("{bin}/aarch64-linux-android29-clang"),
            ),
            ("AR_aarch64_linux_android", format!("{bin}/llvm-ar")),
            (
                "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS",
                "-C link-arg=-Wl,-z,max-page-size=16384".to_owned(),
            ),
            ("ANDROID_NDK_HOME", "/sdk/ndk/30.0.16248370".to_owned()),
        ];
        assert_eq!(
            env,
            expected
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect::<Vec<_>>()
        );
        let x86 = ndk_env(&ndk(), &ANDROID_ABIS[1]);
        assert_eq!(x86[0].0, "CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER");
        assert!(x86[0].1.ends_with("/x86_64-linux-android29-clang"));
    }

    #[test]
    fn an_explicit_ndk_wins_and_an_old_one_is_refused() {
        let dir = Scratch::new("explicit");
        fake_ndk(&dir.join("r30"), "30.0.16248370");
        fake_ndk(&dir.join("r27"), "27.2.12479018");
        let r30 = display(&dir.join("r30"));
        let found = find_ndk(
            &|name| (name == "ANDROID_NDK_HOME").then(|| r30.clone()),
            &dir,
        )
        .expect("found");
        assert_eq!((found.root, found.major), (dir.join("r30"), 30));
        let r27 = display(&dir.join("r27"));
        let error =
            find_ndk(&|name| (name == "NDK_HOME").then(|| r27.clone()), &dir).expect_err("too old");
        assert!(error.to_string().contains("NDK r27 is too old"), "{error}");
        assert!(error.to_string().contains(NDK_PACKAGE), "{error}");
    }

    #[test]
    fn the_newest_ndk_dir_under_the_sdk_wins() {
        let dir = Scratch::new("newest");
        let sdk = dir.join("sdk");
        fake_ndk(&sdk.join("ndk/28.2.13676358"), "28.2.13676358");
        fake_ndk(&sdk.join("ndk/30.0.16248370"), "30.0.16248370");
        fake_ndk(&sdk.join("ndk/9.9.9"), "9.9.9");
        std::fs::create_dir_all(sdk.join("ndk/notes")).expect("stray dir");
        let sdk_text = display(&sdk);
        let found = find_ndk(
            &|name| (name == "ANDROID_HOME").then(|| sdk_text.clone()),
            &dir,
        )
        .expect("found");
        assert_eq!(found.root, sdk.join("ndk/30.0.16248370"));
        let home = Scratch::new("home");
        fake_ndk(&home.join("Library/Android/sdk/ndk/30.0.1"), "30.0.1");
        assert_eq!(
            find_ndk(&|_| None, &home).expect("found").root,
            home.join("Library/Android/sdk/ndk/30.0.1")
        );
    }

    #[test]
    fn a_missing_ndk_is_an_error_with_instructions() {
        let dir = Scratch::new("missing");
        let error = find_ndk(&|_| None, &dir).expect_err("missing");
        let text = error.to_string();
        assert!(text.contains("ANDROID_NDK_HOME"), "{text}");
        assert!(text.contains(NDK_PACKAGE), "{text}");
    }

    #[test]
    fn xcframework_args_order() {
        let args = xcframework_args(
            Path::new("/t/ios.a"),
            Path::new("/t/sim.a"),
            Path::new("/t/include"),
            Path::new("/o/X.xcframework"),
        );
        assert_eq!(
            args,
            [
                "-create-xcframework",
                "-library",
                "/t/ios.a",
                "-headers",
                "/t/include",
                "-library",
                "/t/sim.a",
                "-headers",
                "/t/include",
                "-output",
                "/o/X.xcframework"
            ]
        );
    }

    #[test]
    fn ios_artifacts_writes_both_paths() {
        let plan = ios_artifacts_plan(&layout(), Path::new("/repo/apps/ios/Core"));
        let xcframework = PathBuf::from("/repo/apps/ios/Core/Artifacts/SdrmmMobileFFI.xcframework");
        let swift = PathBuf::from("/repo/apps/ios/Core/Generated/SdrmmCore.swift");
        assert_eq!(plan.outputs, [xcframework.clone(), swift]);
        let xcodebuild = step_args(&plan, "-create-xcframework");
        assert_eq!(
            xcodebuild,
            [xcframework_args(
                Path::new("/repo/target/aarch64-apple-ios/mobile/libsdrmm_mobile_core.a"),
                Path::new("/repo/target/aarch64-apple-ios-sim/mobile/libsdrmm_mobile_core.a"),
                Path::new("/repo/target/mobile/ios/include"),
                &xcframework,
            )]
        );
        let sources = step_args(&plan, "--swift-sources");
        assert_eq!(sources.len(), 1);
        assert!(sources[0].contains(&"/repo/apps/ios/Core/Generated".to_owned()));
        assert!(plan.steps.contains(&Step::Clean(xcframework)));
        let builds = step_args(&plan, "staticlib");
        assert_eq!(builds.len(), 2);
        assert!(builds.iter().all(|args| args.contains(&PROFILE.to_owned())));
        let headers = step_args(&plan, "--headers");
        assert!(headers[0].contains(&FFI_MODULE.to_owned()));
        assert!(headers[0].contains(&"Security".to_owned()));
    }

    #[test]
    fn bindings_come_from_one_host_library() {
        let plan = bindings_plan(&layout());
        let host = display(&layout().host_library());
        assert_eq!(plan.steps[0], host_library_step());
        assert!(plan.steps[1..].iter().all(|step| matches!(
            step,
            Step::Cargo { args, .. } if args.contains(&host)
        )));
        assert_eq!(
            plan.outputs,
            [
                PathBuf::from("/repo/target/mobile/bindings/swift/SdrmmCore.swift"),
                PathBuf::from(
                    "/repo/target/mobile/bindings/kotlin/dev/newspicel/sdrmm/ffi/sdrmm_mobile_core.kt"
                ),
            ]
        );
    }

    #[test]
    fn android_builds_both_abis_into_the_out_dir() {
        let plan = android_plan(&layout(), &ndk(), Path::new("/out"));
        assert_eq!(
            plan.outputs,
            [
                PathBuf::from("/out/kotlin/dev/newspicel/sdrmm/ffi/sdrmm_mobile_core.kt"),
                PathBuf::from("/out/jniLibs/arm64-v8a/libsdrmm_mobile_core.so"),
                PathBuf::from("/out/jniLibs/x86_64/libsdrmm_mobile_core.so"),
            ]
        );
        assert!(plan.steps.contains(&Step::Copy {
            from: PathBuf::from("/repo/target/x86_64-linux-android/mobile/libsdrmm_mobile_core.so"),
            to: PathBuf::from("/out/jniLibs/x86_64/libsdrmm_mobile_core.so"),
        }));
        let builds: Vec<&Step> = plan
            .steps
            .iter()
            .filter(|step| matches!(step, Step::Cargo { args, .. } if args.contains(&"--target".to_owned())))
            .collect();
        assert_eq!(builds.len(), 2);
        for (build, abi) in builds.into_iter().zip(&ANDROID_ABIS) {
            assert!(matches!(build, Step::Cargo { env, .. } if *env == ndk_env(&ndk(), abi)));
        }
    }

    #[test]
    fn checks_lint_the_library_for_every_phone_target() {
        let linted: Vec<Vec<String>> = ios_check_plan()
            .steps
            .into_iter()
            .chain(android_check_plan(&ndk()).steps)
            .filter_map(|step| match step {
                Step::Cargo { args, .. } => Some(args),
                _ => None,
            })
            .collect();
        assert_eq!(linted.len(), 4);
        for (args, triple) in linted.iter().zip(phone_targets()) {
            assert_eq!(
                args,
                &strings([
                    "clippy", "-p", CRATE, "--lib", "--target", triple, "--", "-D", "warnings"
                ])
            );
        }
    }

    #[test]
    fn tree_lines_name_package_and_version() {
        let tree = "sdrmm-mobile-core v0.1.0 (/repo/crates/mobile-core)\nring v0.17.14\nring v0.17.14 (*)\nserde_derive v1.0.228 (proc-macro)\n\n";
        assert_eq!(
            tree_packages(tree).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                ("ring".to_owned(), "0.17.14".to_owned()),
                ("sdrmm-mobile-core".to_owned(), "0.1.0".to_owned()),
                ("serde_derive".to_owned(), "1.0.228".to_owned()),
            ])
        );
    }
}
