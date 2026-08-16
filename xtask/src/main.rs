#![allow(clippy::expect_used)]

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use notify::{Config, Event, EventKind, PollWatcher, RecursiveMode, Watcher};
use num_complex::Complex;

mod bandplan;
mod ber;
mod homebrew;
mod icons;
mod licenses;
mod linkage;
mod updater;

#[derive(Parser)]
#[command(name = "xtask", about = "sdr-- workspace tasks")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Codegen,
    Licenses,
    Dev,
    Check,
    Test,
    Audit,
    Smoke,
    Screenshots,
    Fixtures,
    Bandplan {
        #[arg(long)]
        offline: bool,
    },
    Ber {
        entry: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        full: bool,
    },
    Icons,
    Dist {
        #[arg(long)]
        target: Option<String>,
    },
    Desktop {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        bundles: Option<String>,
    },
    SoapyBundleCheck {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    LinkCheck {
        path: PathBuf,
        #[arg(long = "external")]
        external: Vec<String>,
    },
    SetVersion {
        version: String,
    },
    UpdaterManifest {
        #[arg(long)]
        version: String,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        base_url: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    HomebrewTap {
        #[arg(long)]
        version: String,
        #[arg(long)]
        sums: PathBuf,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Codegen => codegen(&root()),
        Cmd::Licenses => licenses::run(&root(), PNPM),
        Cmd::Dev => dev(&root()),
        Cmd::Check => check(&root()),
        Cmd::Test => test(&root()),
        Cmd::Audit => audit(&root()),
        Cmd::Smoke => smoke(&root()),
        Cmd::Screenshots => screenshots(&root()),
        Cmd::Fixtures => fixtures(&root()),
        Cmd::Bandplan { offline } => bandplan::run(&root(), offline),
        Cmd::Ber { entry, out, full } => ber::run(&root(), &entry, out.as_deref(), full),
        Cmd::Icons => icons::icons(&root()),
        Cmd::Dist { target } => dist(&root(), target.as_deref()),
        Cmd::Desktop { target, bundles } => desktop(&root(), target.as_deref(), bundles.as_deref()),
        Cmd::SoapyBundleCheck { dir } => soapy_bundle_check(
            dir.unwrap_or_else(|| root().join("apps/desktop/resources/soapy"))
                .as_path(),
        ),
        Cmd::LinkCheck { path, external } => linkage::check(&path, &external),
        Cmd::SetVersion { version } => set_version(&root(), &version),
        Cmd::UpdaterManifest {
            version,
            dir,
            base_url,
            out,
        } => updater::manifest(&dir, &version, &base_url, out.as_deref()),
        Cmd::HomebrewTap {
            version,
            sums,
            repo,
            out,
        } => homebrew::tap(&sums, &version, &repo, &out),
    }
}

#[cfg(windows)]
const PNPM: &str = "pnpm.cmd";
#[cfg(not(windows))]
const PNPM: &str = "pnpm";

const MACOS_LIBRARY_PREFIXES: &[&str] = &["/opt/homebrew/lib", "/usr/local/lib"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent")
        .to_path_buf()
}

fn codegen(root: &Path) -> Result<()> {
    let spec = sdrmm_server::openapi()
        .to_pretty_json()
        .context("serialize OpenAPI")?;
    let openapi_path = root.join("openapi.json");
    std::fs::write(&openapi_path, format!("{spec}\n")).context("write openapi.json")?;
    println!("wrote {}", openapi_path.display());

    let out = root.join("web/src/generated/schema.d.ts");
    std::fs::create_dir_all(out.parent().expect("schema has a parent"))
        .context("create generated dir")?;

    run(
        PNPM,
        &[
            "--dir",
            "web",
            "exec",
            "openapi-typescript",
            openapi_path.to_str().expect("utf8 path"),
            "-o",
            out.to_str().expect("utf8 path"),
            "--alphabetize",
        ],
        root,
    )?;
    println!("wrote {}", out.display());
    Ok(())
}

fn dev(root: &Path) -> Result<()> {
    ensure_web_deps(root)?;
    #[cfg(unix)]
    let _interrupt_handler = InterruptHandler::install()?;

    let mut vite = Command::new(PNPM);
    vite.args(["--dir", "web", "dev"]).current_dir(root);
    vite.stdin(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut vite, 0);
    let mut vite = vite
        .spawn()
        .context("spawn vite dev server (is pnpm installed?)")?;

    let result = watch_rust_server(root, &mut vite);

    kill_process_tree(&mut vite);
    result
}

const DEV_POLL_INTERVAL: Duration = Duration::from_millis(100);
const DEV_FILE_SCAN_INTERVAL: Duration = Duration::from_millis(500);
const DEV_RESTART_DEBOUNCE: Duration = Duration::from_millis(250);

fn watch_rust_server(root: &Path, vite: &mut Child) -> Result<()> {
    let (changes_tx, changes_rx) = mpsc::channel();
    let mut watcher = PollWatcher::new(
        changes_tx,
        Config::default().with_poll_interval(DEV_FILE_SCAN_INTERVAL),
    )
    .context("start backend watcher")?;
    watcher
        .watch(root, RecursiveMode::NonRecursive)
        .context("watch workspace manifests")?;
    for path in [root.join("crates"), root.join("apps/sdrmm")] {
        watcher
            .watch(&path, RecursiveMode::Recursive)
            .with_context(|| format!("watch {}", path.display()))?;
    }

    let mut server = Some(spawn_rust_server(root)?);
    let result = (|| -> Result<()> {
        let mut last_change = None;
        loop {
            if dev_interrupted() {
                return Ok(());
            }
            if let Some(status) = vite.try_wait().context("poll Vite dev server")? {
                bail!("Vite dev server exited with {status}");
            }
            if let Some(child) = server.as_mut()
                && let Some(status) = child.try_wait().context("poll Rust server")?
            {
                eprintln!("Rust server exited with {status}; waiting for a backend change");
                server = None;
            }

            match changes_rx.recv_timeout(DEV_POLL_INTERVAL) {
                Ok(Ok(event)) if is_backend_change(root, &event) => {
                    last_change = Some(Instant::now());
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => return Err(error).context("watch backend inputs"),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => bail!("backend watcher stopped"),
            }

            if last_change.is_some_and(|changed| changed.elapsed() >= DEV_RESTART_DEBOUNCE) {
                if let Some(mut child) = server.take() {
                    kill_process_tree(&mut child);
                }
                println!("backend changed; restarting Rust server");
                server = Some(spawn_rust_server(root)?);
                last_change = None;
            }
        }
    })();

    if let Some(mut child) = server {
        kill_process_tree(&mut child);
    }
    result
}

fn is_backend_change(root: &Path, event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    let crates = root.join("crates");
    let server = root.join("apps/sdrmm");
    let manifest = root.join("Cargo.toml");
    let lockfile = root.join("Cargo.lock");
    event.paths.iter().any(|path| {
        path == &manifest
            || path == &lockfile
            || path.starts_with(&crates)
            || path.starts_with(&server)
    })
}

fn rust_server_command(root: &Path) -> Command {
    let mut server = Command::new("cargo");
    server
        .args(["run", "-p", "sdrmm", "--", "--dev-cors"])
        .current_dir(root);
    server
}

fn spawn_rust_server(root: &Path) -> Result<Child> {
    let mut server = rust_server_command(root);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut server, 0);
    server.spawn().context("spawn Rust server")
}

#[cfg(unix)]
struct InterruptHandler(libc::sigaction);

#[cfg(unix)]
static DEV_INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
impl InterruptHandler {
    fn install() -> Result<Self> {
        DEV_INTERRUPTED.store(false, Ordering::Relaxed);
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        action.sa_sigaction = preserve_dev_supervisor as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        unsafe { libc::sigemptyset(&mut action.sa_mask) };

        let mut previous = unsafe { std::mem::zeroed::<libc::sigaction>() };
        if unsafe { libc::sigaction(libc::SIGINT, &action, &mut previous) } != 0 {
            return Err(std::io::Error::last_os_error()).context("install Ctrl-C handler");
        }
        Ok(Self(previous))
    }
}

#[cfg(unix)]
impl Drop for InterruptHandler {
    fn drop(&mut self) {
        unsafe { libc::sigaction(libc::SIGINT, &self.0, std::ptr::null_mut()) };
    }
}

#[cfg(unix)]
extern "C" fn preserve_dev_supervisor(_: libc::c_int) {
    DEV_INTERRUPTED.store(true, Ordering::Relaxed);
}

#[cfg(unix)]
fn dev_interrupted() -> bool {
    DEV_INTERRUPTED.load(Ordering::Relaxed)
}

#[cfg(not(unix))]
fn dev_interrupted() -> bool {
    false
}

#[cfg(unix)]
fn kill_process_tree(child: &mut Child) {
    let group = -(child.id() as i32);
    unsafe { libc::kill(group, libc::SIGTERM) };
    for _ in 0..50 {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    unsafe { libc::kill(group, libc::SIGKILL) };
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_process_tree(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod dev_tests {
    use super::*;

    const HELPER_ENV: &str = "SDRMM_XTASK_INTERRUPT_HELPER";

    #[test]
    fn interrupt_handler_keeps_supervisor_alive() {
        let status = Command::new(std::env::current_exe().expect("locate test binary"))
            .args([
                "--ignored",
                "--exact",
                "dev_tests::interrupt_handler_helper",
            ])
            .env(HELPER_ENV, "1")
            .status()
            .expect("run interrupt helper");
        assert!(status.success(), "interrupt helper exited with {status}");
    }

    #[test]
    #[ignore]
    fn interrupt_handler_helper() {
        if std::env::var_os(HELPER_ENV).is_none() {
            return;
        }
        let _handler = InterruptHandler::install().expect("install Ctrl-C handler");
        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        assert!(dev_interrupted());
    }
}

#[cfg(test)]
mod dev_command_tests {
    use super::*;

    #[test]
    fn rust_server_command_enables_development_cors() {
        let root = Path::new("workspace");
        let server = rust_server_command(root);

        assert_eq!(server.get_program(), "cargo");
        assert_eq!(server.get_current_dir(), Some(root));
        assert_eq!(
            server.get_args().collect::<Vec<_>>(),
            ["run", "-p", "sdrmm", "--", "--dev-cors"]
        );
    }

    #[test]
    fn backend_change_filter_accepts_only_watched_build_inputs() {
        let root = Path::new("/workspace");
        for path in [
            "/workspace/Cargo.toml",
            "/workspace/Cargo.lock",
            "/workspace/crates/server/src/lib.rs",
            "/workspace/apps/sdrmm/src/main.rs",
        ] {
            let event = Event::new(EventKind::Any).add_path(path.into());
            assert!(is_backend_change(root, &event), "ignored {path}");
        }

        for path in [
            "/workspace/README.md",
            "/workspace/web/src/App.tsx",
            "/workspace/xtask/src/main.rs",
        ] {
            let event = Event::new(EventKind::Any).add_path(path.into());
            assert!(!is_backend_change(root, &event), "accepted {path}");
        }
    }

    #[test]
    fn backend_change_filter_ignores_file_access() {
        use notify::event::AccessKind;

        let event = Event::new(EventKind::Access(AccessKind::Any))
            .add_path("/workspace/crates/server/src/lib.rs".into());
        assert!(!is_backend_change(Path::new("/workspace"), &event));
    }
}

fn check(root: &Path) -> Result<()> {
    check_toolchain_pins(root)?;
    check_windows_rs_alignment(root)?;
    check_baked_in_fixtures(root)?;
    run("cargo", &["fmt", "--all", "--", "--check"], root)?;

    ensure_web_deps(root)?;
    run(PNPM, &["--dir", "web", "exec", "biome", "ci", "."], root)?;
    run(
        PNPM,
        &["--dir", "web", "exec", "oxlint", "--type-aware"],
        root,
    )?;
    run(PNPM, &["--dir", "web", "exec", "tsgo", "--noEmit"], root)?;

    run(
        "cargo",
        &["clippy", "--all-targets", "--", "-D", "warnings"],
        root,
    )?;
    run(
        "cargo",
        &["check", "-p", "sdrmm", "--no-default-features"],
        root,
    )?;
    run(
        "cargo",
        &[
            "check",
            "-p",
            "sdrmm",
            &release_features()[0],
            &release_features()[1],
            &release_features()[2],
        ],
        root,
    )?;

    web_build(root)?;

    codegen(root)?;
    run(
        "git",
        &[
            "diff",
            "--exit-code",
            "--",
            "openapi.json",
            "web/src/generated",
        ],
        root,
    )
    .context("codegen drift: regenerate with `cargo xtask codegen` and commit")?;

    licenses::run(root, PNPM)?;
    run(
        "git",
        &[
            "diff",
            "--exit-code",
            "--",
            licenses::NOTICES_JSON,
            licenses::NOTICES_MARKDOWN,
        ],
        root,
    )
    .context("notices drift: regenerate with `cargo xtask licenses` and commit")?;
    println!("check: all gates green");
    Ok(())
}

fn check_toolchain_pins(root: &Path) -> Result<()> {
    let file = |rel: &str| -> Result<String> {
        std::fs::read_to_string(root.join(rel)).with_context(|| format!("read {rel}"))
    };
    let pin = |text: &str, open: &str, close: &str, rel: &str, what: &str| -> Result<String> {
        slice_between(text, open, close)
            .map(str::to_string)
            .with_context(|| format!("{rel} declares no {what} (looked for `{open}`)"))
    };

    let dockerfile = file("Dockerfile")?;
    let package_json = file("web/package.json")?;

    let mut pnpm = vec![
        (
            "web/package.json".to_string(),
            pin(
                &package_json,
                "\"packageManager\": \"pnpm@",
                "\"",
                "web/package.json",
                "packageManager pin",
            )?,
        ),
        (
            "Dockerfile".to_string(),
            pin(&dockerfile, "pnpm@", "\n", "Dockerfile", "pnpm pin")?,
        ),
    ];
    let mut node = vec![(
        "Dockerfile".to_string(),
        pin(
            &dockerfile,
            "FROM node:",
            "-slim",
            "Dockerfile",
            "node base image",
        )?,
    )];

    for name in ["ci.yml", "release.yml"] {
        let rel = format!(".github/workflows/{name}");
        let text = file(&rel)?;
        pnpm.push((
            rel.clone(),
            pin(&text, "PNPM_VERSION: ", "\n", &rel, "PNPM_VERSION")?,
        ));
        node.push((
            rel.clone(),
            pin(&text, "NODE_VERSION: ", "\n", &rel, "NODE_VERSION")?,
        ));
    }

    agree("pnpm", &pnpm)?;
    agree("the Node major", &node)
}

fn check_windows_rs_alignment(root: &Path) -> Result<()> {
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).context("read Cargo.lock")?;
    let (Some(hal), Some(allocator)) = (
        locked_dependency(&lock, "wgpu-hal", "windows"),
        locked_dependency(&lock, "gpu-allocator", "windows"),
    ) else {
        return Ok(());
    };
    ensure!(
        hal == allocator,
        "Cargo.lock resolves gpu-allocator against windows {allocator} and wgpu-hal against \
         windows {hal}: wgpu-hal's dx12 backend cannot compile against the pair. Point \
         gpu-allocator's `dependencies` entry in Cargo.lock at `windows {hal}`."
    );
    Ok(())
}

fn locked_dependency(lock: &str, package: &str, dependency: &str) -> Option<String> {
    let entry = locked_package(lock, package)?
        .lines()
        .map(|line| line.trim().trim_end_matches(',').trim_matches('"'))
        .find(|entry| *entry == dependency || entry.starts_with(&format!("{dependency} ")))?;
    match entry.split_once(' ') {
        Some((_, version)) => Some(version.to_string()),
        None => locked_package(lock, dependency)
            .and_then(|block| slice_between(block, "\nversion = \"", "\""))
            .map(str::to_string),
    }
}

fn locked_package<'a>(lock: &'a str, package: &str) -> Option<&'a str> {
    lock.split("[[package]]")
        .find(|block| block.starts_with(&format!("\nname = \"{package}\"\n")))
}

fn check_baked_in_fixtures(root: &Path) -> Result<()> {
    let mut sources = Vec::new();
    for dir in ["crates", "apps", "xtask"] {
        sources.extend(rust_sources(&root.join(dir))?);
    }
    for source in sources {
        let text = std::fs::read_to_string(&source)
            .with_context(|| format!("read {}", source.display()))?;
        for stem in text
            .split("include_bytes!(\"")
            .skip(1)
            .filter_map(|rest| rest.split_once('"').map(|(path, _)| path))
            .filter_map(|path| path.rsplit_once("fixtures/").map(|(_, stem)| stem))
        {
            let rel = format!("fixtures/{stem}");
            let tracked = Command::new("git")
                .args(["ls-files", "--error-unmatch", "--", &rel])
                .current_dir(root)
                .output()
                .context("git ls-files")?
                .status
                .success();
            ensure!(
                tracked,
                "{} bakes in {rel}, which git does not track: the build only works where that \
                 file was generated. Commit it with `git add -f {rel}` (and say why in \
                 fixtures/README.md), or point the test at a fixture it generates itself.",
                source.display()
            );
        }
    }
    Ok(())
}

fn rust_sources(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry
            .with_context(|| format!("read {}", dir.display()))?
            .path();
        if path.is_dir() {
            out.extend(rust_sources(&path)?);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(out)
}

fn slice_between<'a>(haystack: &'a str, open: &str, close: &str) -> Option<&'a str> {
    Some(haystack.split_once(open)?.1.split_once(close)?.0.trim())
}

fn agree(what: &str, pins: &[(String, String)]) -> Result<()> {
    let (canonical_at, canonical) = &pins[0];
    for (at, value) in pins {
        ensure!(
            value == canonical,
            "{what} is pinned to {canonical} in {canonical_at} but {value} in {at}. \
             These are updated by different tools and must be changed together."
        );
    }
    Ok(())
}

fn release_features() -> [String; 3] {
    [
        "--no-default-features".to_string(),
        "--features".to_string(),
        "soapy,sdrplay,rtlsdr,hackrf,net-client,gpu-fft".to_string(),
    ]
}

fn web_build(root: &Path) -> Result<()> {
    ensure_web_deps(root)?;
    run(PNPM, &["--dir", "web", "build"], root)
}

fn assert_web_dist(root: &Path) -> Result<()> {
    let index = root.join("web/dist/index.html");
    ensure!(
        index.exists(),
        "{} is missing after the web build: the artifact would embed an empty UI",
        index.display()
    );
    Ok(())
}

fn dist(root: &Path, target: Option<&str>) -> Result<()> {
    web_build(root)?;
    assert_web_dist(root)?;
    ensure_target(root, target)?;

    let features = release_features();
    let mut args = vec![
        "build",
        "--release",
        "--locked",
        "-p",
        "sdrmm",
        &features[0],
        &features[1],
        &features[2],
    ];
    if let Some(triple) = target {
        args.push("--target");
        args.push(triple);
    }
    run("cargo", &args, root)?;

    let triple = match target {
        Some(triple) => triple.to_string(),
        None => host_triple()?,
    };
    let windows = triple.contains("windows");
    let exe = if windows { "sdrmm.exe" } else { "sdrmm" };
    let built = match target {
        Some(triple) => root.join("target").join(triple).join("release").join(exe),
        None => root.join("target").join("release").join(exe),
    };

    let out = root.join("dist");
    let name = format!("sdrmm-{}-{triple}", env!("CARGO_PKG_VERSION"));
    let staged = out.join(&name);
    if staged.exists() {
        std::fs::remove_dir_all(&staged)
            .with_context(|| format!("cannot clear {}", staged.display()))?;
    }
    std::fs::create_dir_all(&staged)
        .with_context(|| format!("cannot create {}", staged.display()))?;

    std::fs::copy(&built, staged.join(exe))
        .with_context(|| format!("cannot stage {}", built.display()))?;
    for doc in ["README.md", "LICENSE", "THIRD_PARTY_NOTICES.md"] {
        std::fs::copy(root.join(doc), staged.join(doc))
            .with_context(|| format!("cannot stage {doc}"))?;
    }

    if triple.contains("linux") {
        run(
            "strip",
            &[staged.join(exe).to_str().expect("utf8 path")],
            root,
        )?;
    }
    if triple.contains("apple") {
        add_loader_paths(root, &staged.join(exe))?;
    }

    let archive = archive(root, &out, &name, windows)?;
    println!("dist: {}", archive.display());
    Ok(())
}

fn add_loader_paths(root: &Path, binary: &Path) -> Result<()> {
    let path = binary.to_str().context("non-utf8 binary path")?;
    for prefix in MACOS_LIBRARY_PREFIXES {
        run("install_name_tool", &["-add_rpath", prefix, path], root)?;
    }
    run("codesign", &["--sign", "-", "--force", path], root)?;

    let present = linkage::rpaths(binary)?;
    for prefix in MACOS_LIBRARY_PREFIXES {
        ensure!(
            present.iter().any(|rpath| rpath == prefix),
            "{path} carries no {prefix} search path: it links @rpath/libSoapySDR and would fail \
             to launch on a host that installed SoapySDR where every installer puts it"
        );
    }
    Ok(())
}

fn archive(root: &Path, out: &Path, name: &str, windows: bool) -> Result<PathBuf> {
    let ext = if windows { "zip" } else { "tar.gz" };
    let path = out.join(format!("{name}.{ext}"));
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("replace {}", path.display())),
    }

    if windows {
        run(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "Add-Type -AssemblyName System.IO.Compression.FileSystem; \
                     [System.IO.Compression.ZipFile]::CreateFromDirectory('{}', '{}', \
                     [System.IO.Compression.CompressionLevel]::Optimal, $true)",
                    out.join(name).display(),
                    path.display()
                ),
            ],
            root,
        )?;
    } else {
        run_with_env(
            "tar",
            &[
                "-C",
                out.to_str().expect("utf8 path"),
                "-czf",
                path.to_str().expect("utf8 path"),
                name,
            ],
            root,
            &[("COPYFILE_DISABLE", "1")],
        )?;
    }
    Ok(path)
}

fn ensure_target(root: &Path, target: Option<&str>) -> Result<()> {
    let Some(triple) = target else {
        return Ok(());
    };
    if triple == host_triple()? {
        return Ok(());
    }
    run("rustup", &["target", "add", triple], root)
}

fn host_triple() -> Result<String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("failed to spawn `rustc`")?;
    ensure!(out.status.success(), "`rustc -vV` failed");
    let stdout = String::from_utf8(out.stdout).context("`rustc -vV` printed non-utf8")?;
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_string)
        .context("`rustc -vV` printed no host line")
}

fn desktop(root: &Path, target: Option<&str>, bundles: Option<&str>) -> Result<()> {
    ensure_target(root, target)?;
    let features = release_features();

    let Some(bundles) = bundles else {
        let mut args = vec![
            "clippy",
            "-p",
            "sdrmm-desktop",
            "--all-targets",
            &features[0],
            &features[1],
            &features[2],
        ];
        if let Some(triple) = target {
            args.push("--target");
            args.push(triple);
        }
        args.extend(["--", "-D", "warnings"]);
        return run("cargo", &args, root);
    };

    web_build(root)?;
    assert_web_dist(root)?;
    soapy_bundle_check(&root.join("apps/desktop/resources/soapy"))?;

    let installed = Command::new("cargo")
        .args(["tauri", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    ensure!(
        installed,
        "the Tauri CLI is missing: `cargo install --locked tauri-cli`"
    );
    let unsigned = std::env::var_os("TAURI_SIGNING_PRIVATE_KEY").is_none();
    if unsigned {
        println!(
            "note: TAURI_SIGNING_PRIVATE_KEY is unset — bundling unsigned, so these installers \
             cannot be served to the updater."
        );
    }
    let mut args = vec![
        "tauri",
        "build",
        "--bundles",
        bundles,
        "--",
        &features[0],
        &features[1],
        &features[2],
    ];
    if let Some(triple) = target {
        args.insert(2, "--target");
        args.insert(3, triple);
    }
    if unsigned {
        args.insert(2, "--no-sign");
    }
    run("cargo", &args, &root.join("apps/desktop"))
}

fn soapy_bundle_check(dir: &Path) -> Result<()> {
    ensure!(
        dir.is_dir(),
        "Soapy bundle directory is missing: {}",
        dir.display()
    );
    let files = files_under(dir)?;
    let names: Vec<String> = files
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .collect();
    let has = |needle: &str| names.iter().any(|name| name.contains(needle));
    ensure!(
        has("soapysdr"),
        "{} contains no SoapySDR core library",
        dir.display()
    );
    let staged_modules: Vec<&String> = files
        .iter()
        .zip(&names)
        .filter(|(path, _)| {
            path.components()
                .any(|part| part.as_os_str() == "modules0.8")
        })
        .map(|(_, name)| name)
        .collect();
    for native in ["rtlsdr", "hackrf"] {
        ensure!(
            !staged_modules.iter().any(|name| name.contains(native)),
            "{} carries a Soapy {native} module. This build drives {native} over its own USB \
             stack and hides it from Soapy, so the bundled module could never be reached.",
            dir.display()
        );
    }
    let curated = ["airspyhf", "bladerf", "lms7", "pluto", "remote"];
    for module in curated {
        ensure!(
            has(module),
            "{} contains no curated {module} module",
            dir.display()
        );
    }
    ensure!(
        names
            .iter()
            .any(|name| name.contains("airspy") && !name.contains("airspyhf")),
        "{} contains no curated Airspy module",
        dir.display()
    );
    let outside_modules: Vec<&String> = files
        .iter()
        .zip(&names)
        .filter(|(path, name)| {
            let inside = |part: &str| path.components().any(|c| c.as_os_str() == part);
            !inside("modules0.8")
                && !inside("licenses")
                && [".dylib", ".so", ".dll"]
                    .iter()
                    .any(|ext| name.contains(ext))
        })
        .map(|(_, name)| name)
        .collect();
    for driver in [
        "airspyhf",
        "airspy",
        "bladerf",
        "limesuite",
        "iio",
        "ad9361",
        "usb",
    ] {
        ensure!(
            outside_modules.iter().any(|name| name.contains(driver)),
            "{} carries a module for {driver} but not the library it loads. Staged beside the \
             modules: {}",
            dir.display(),
            outside_modules
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    ensure!(
        files
            .iter()
            .any(|path| path.components().any(|part| part.as_os_str() == "licenses")),
        "{} contains no dependency notices/licenses",
        dir.display()
    );
    println!("soapy bundle: {} files in {}", files.len(), dir.display());
    Ok(())
}

fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(files_under(&path)?);
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

fn set_version(root: &Path, version: &str) -> Result<()> {
    let version = version.strip_prefix('v').unwrap_or(version);
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

    let manifest_path = root.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).context("read Cargo.toml")?;
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
    std::fs::write(&manifest_path, out).context("write Cargo.toml")?;

    run("cargo", &["update", "--workspace", "--offline"], root)?;
    println!("version: {version}");
    Ok(())
}

fn test(root: &Path) -> Result<()> {
    let soapy_root = root.join("target/hermetic-soapy");
    let modules = soapy_root.join("lib/SoapySDR/modules0.8");
    std::fs::create_dir_all(&modules).context("create hermetic Soapy module directory")?;
    run_with_env(
        "cargo",
        &["test", "--all-targets"],
        root,
        &[
            (
                "SOAPY_SDR_ROOT",
                soapy_root
                    .to_str()
                    .context("non-utf8 hermetic Soapy path")?,
            ),
            (
                "SOAPY_SDR_PLUGIN_PATH",
                modules.to_str().context("non-utf8 hermetic Soapy path")?,
            ),
        ],
    )?;
    ensure_web_deps(root)?;
    run(PNPM, &["--dir", "web", "test"], root)?;
    Ok(())
}

fn audit(root: &Path) -> Result<()> {
    let installed = Command::new("cargo")
        .args(["deny", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    ensure!(
        installed,
        "cargo-deny is missing: `cargo install --locked cargo-deny`"
    );
    run("cargo", &["deny", "check", "advisories"], root)
}

fn smoke(root: &Path) -> Result<()> {
    ensure_web_deps(root)?;
    run_with_env(
        PNPM,
        &["--dir", "web", "build"],
        root,
        &[("VITE_ENABLE_SYNTHETIC_DEVICES", "true")],
    )?;
    let soapy_root = root.join("target/hermetic-soapy");
    let modules = soapy_root.join("lib/SoapySDR/modules0.8");
    std::fs::create_dir_all(&modules).context("create hermetic Soapy module directory")?;
    run_with_env(
        PNPM,
        &["--dir", "web", "exec", "playwright", "test"],
        root,
        &[
            (
                "SOAPY_SDR_ROOT",
                soapy_root
                    .to_str()
                    .context("non-utf8 hermetic Soapy path")?,
            ),
            (
                "SOAPY_SDR_PLUGIN_PATH",
                modules.to_str().context("non-utf8 hermetic Soapy path")?,
            ),
        ],
    )?;
    Ok(())
}

fn screenshots(root: &Path) -> Result<()> {
    ensure_web_deps(root)?;
    run_with_env(
        PNPM,
        &["--dir", "web", "build"],
        root,
        &[("VITE_ENABLE_SYNTHETIC_DEVICES", "true")],
    )?;
    let out = root.join("assets/screenshots");
    std::fs::create_dir_all(&out).context("create screenshot directory")?;
    let soapy_root = root.join("target/hermetic-soapy");
    let modules = soapy_root.join("lib/SoapySDR/modules0.8");
    std::fs::create_dir_all(&modules).context("create hermetic Soapy module directory")?;
    run_with_env(
        PNPM,
        &[
            "--dir",
            "web",
            "exec",
            "playwright",
            "test",
            "--config",
            "playwright.screenshots.config.ts",
        ],
        root,
        &[
            (
                "SOAPY_SDR_ROOT",
                soapy_root
                    .to_str()
                    .context("non-utf8 hermetic Soapy path")?,
            ),
            (
                "SOAPY_SDR_PLUGIN_PATH",
                modules.to_str().context("non-utf8 hermetic Soapy path")?,
            ),
        ],
    )?;
    println!("wrote {}", out.display());
    Ok(())
}

fn fixtures(root: &Path) -> Result<()> {
    const CENTER_HZ: f64 = 100_000_000.0;

    let dir = root.join("fixtures");
    std::fs::create_dir_all(&dir).context("create fixtures dir")?;

    const SIGGEN_RATE: f64 = 2_400_000.0;
    let samples = sdrmm_device_virtual::render(SIGGEN_RATE, SIGGEN_RATE as usize);
    write_fixture(
        &dir,
        "siggen_2m4_1s",
        &samples,
        SIGGEN_RATE,
        CENTER_HZ,
        "Signal Generator (virtual)",
        "1 s of the virtual siggen — the record/replay fixture",
    )?;

    for fixture in decoder_fixtures() {
        write_fixture(
            &dir,
            &fixture.stem,
            &fixture.iq,
            fixture.rate,
            CENTER_HZ,
            "sdr-- reference modulator",
            &fixture.note,
        )?;
    }
    Ok(())
}

struct Fixture {
    stem: String,
    iq: Vec<Complex<f32>>,
    rate: f64,
    note: String,
}

fn aprs_burst() -> Vec<Complex<f32>> {
    use sdrmm_channels::{AprsTx, ChannelCtx, ChannelTx, TxPayload, testgen};
    use sdrmm_wire::{AprsMode, AprsParams, ChannelParams, ChannelSettings};

    let settings = ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        squelch_auto_db: None,
        params: ChannelParams::Aprs(AprsParams {
            mode: AprsMode::Afsk1200,
            ..AprsParams::default()
        }),
        audio: Default::default(),
    };
    let mut tx = AprsTx::new(
        ChannelCtx {
            input_rate: AprsTx::descriptor().input_rate_hz,
        },
        settings,
    )
    .expect("aprs modulator at its own channel rate");
    tx.submit(TxPayload::Frame(AprsTx::ui_frame(
        "DL1ABC-9",
        "APRS",
        &["WIDE1-1"],
        "!5230.00N/01324.00E>sdr-- fixture",
    )))
    .expect("a ui frame is a payload the modulator carries");
    testgen::burst(&mut tx)
}

fn decoder_fixtures() -> Vec<Fixture> {
    use sdrmm_channels::testgen;

    const NARROW: f64 = 240_000.0;
    const AUDIO: f64 = 48_000.0;

    let at = |mut iq: Vec<Complex<f32>>, offset: f64, rate: f64| {
        testgen::shift(&mut iq, offset, rate);
        iq
    };

    let mut out = vec![Fixture {
        stem: "pocsag_1200_240k".to_string(),
        iq: at(
            testgen::pocsag::transmission(
                &[testgen::pocsag::Page {
                    address: 1_234_567,
                    function: 3,
                    text: "SDR-- FIXTURE".to_string(),
                    numeric: false,
                }],
                1_200,
                4_500.0,
                NARROW,
            ),
            50_000.0,
            NARROW,
        ),
        rate: NARROW,
        note: "pocsag channel at +50 kHz -> address 1234567 \"SDR-- FIXTURE\"".to_string(),
    }];

    out.push(Fixture {
        stem: "selcall_ccir1_48k".to_string(),
        iq: at(
            testgen::selcall::transmission(sdrmm_wire::SelcallSystem::Ccir1, "12234", AUDIO)
                .expect("CCIR-1 fixture code is valid"),
            5_000.0,
            AUDIO,
        ),
        rate: AUDIO,
        note: "selcall CCIR-1 channel at +5 kHz -> 12234 (repeat marker expanded)".to_string(),
    });

    out.push(Fixture {
        stem: "selcall_zvei1_48k".to_string(),
        iq: at(
            testgen::selcall::transmission(sdrmm_wire::SelcallSystem::Zvei1, "A11D0", AUDIO)
                .expect("ZVEI-1 fixture code is valid"),
            -5_000.0,
            AUDIO,
        ),
        rate: AUDIO,
        note: "selcall ZVEI-1 channel at -5 kHz -> A11D0 (group symbols and repeat marker)"
            .to_string(),
    });

    out.push(Fixture {
        stem: "ais_position_240k".to_string(),
        iq: at(
            testgen::ais::burst(
                &testgen::ais::position_payload(&testgen::ais::PositionReport {
                    mmsi: 211_234_560,
                    lat: 53.5413,
                    lon: 9.9846,
                    sog_kt: 12.3,
                    cog_deg: 178.4,
                    heading_deg: 179,
                    nav_status: 0,
                }),
                NARROW,
            ),
            25_000.0,
            NARROW,
        ),
        rate: NARROW,
        note: "ais channel at +25 kHz -> MMSI 211234560 at 53.5413, 9.9846".to_string(),
    });

    out.push(Fixture {
        stem: "aprs_afsk1200_240k".to_string(),
        iq: at(
            testgen::resample(&aprs_burst(), AUDIO, NARROW),
            -40_000.0,
            NARROW,
        ),
        rate: NARROW,
        note: "aprs channel at -40 kHz -> DL1ABC-9>APRS,WIDE1-1 at 52.5, 13.4".to_string(),
    });

    out.push(Fixture {
        stem: "rtty_45_170_48k".to_string(),
        iq: at(
            testgen::rtty::transmission("CQ CQ DE DL1ABC K\r\n", 45.45, 170.0, 1.5, AUDIO),
            5_000.0,
            AUDIO,
        ),
        rate: AUDIO,
        note: "rtty channel at +5 kHz (45.45 baud, 170 Hz) -> \"CQ CQ DE DL1ABC K\"".to_string(),
    });

    out.push(Fixture {
        stem: "morse_20wpm_48k".to_string(),
        iq: at(
            testgen::morse::transmission("CQ DE DL1ABC K", 20.0, 0.0, AUDIO),
            -5_000.0,
            AUDIO,
        ),
        rate: AUDIO,
        note: "morse channel at -5 kHz -> \"CQ DE DL1ABC K\" at 20 wpm".to_string(),
    });

    const ADSB_RATE: f64 = 2_000_000.0;
    let icao = 0x3C_6444;
    out.push(Fixture {
        stem: "adsb_squitters_2m".to_string(),
        iq: testgen::adsb::transmission(
            &[
                testgen::adsb::squitter(icao, testgen::adsb::me_identification("DLH123")),
                testgen::adsb::squitter(
                    icao,
                    testgen::adsb::me_airborne_position(38_000, 52.2572, 3.9190, false),
                ),
                testgen::adsb::squitter(
                    icao,
                    testgen::adsb::me_airborne_position(38_000, 52.2657, 3.9184, true),
                ),
                testgen::adsb::squitter(icao, testgen::adsb::me_velocity(450.0, 275.0, -1_024)),
            ],
            500.0,
            0.8,
            ADSB_RATE,
        ),
        rate: ADSB_RATE,
        note: "adsb channel at 0 Hz, device at 2 Msps -> 3C6444/DLH123 at FL380".to_string(),
    });

    out.push(Fixture {
        stem: "dcf77_2026_2k".to_string(),
        iq: testgen::radio_clock::dcf77_example(),
        rate: testgen::radio_clock::RATE,
        note: "radio_clock (DCF77) -> 2026-08-15 12:34 CET with valid parity".to_string(),
    });

    out.push(Fixture {
        stem: "gps_l1_ca_prn7_2m048".to_string(),
        iq: testgen::gnss::acquisition(7, 1_000.0, 317, 2),
        rate: testgen::gnss::RATE,
        note: "gnss channel -> GPS L1 C/A PRN 7, +1000 Hz Doppler, code phase 158.3 chips"
            .to_string(),
    });

    const RDS_RATE: f64 = 960_000.0;
    out.push(Fixture {
        stem: "rds_station_960k".to_string(),
        iq: at(
            testgen::rds::transmission(
                &testgen::rds::Station {
                    pi: 0xD3C2,
                    ps: "SDR-M4  ".to_string(),
                    radiotext: "sdr-- reference fixture".to_string(),
                    pty: 10,
                    tp: true,
                    ta: false,
                    music: true,
                    alt_freqs_hz: vec![89_800_000.0, 95_500_000.0],
                },
                8.0,
                Some(1_000.0),
                RDS_RATE,
            ),
            200_000.0,
            RDS_RATE,
        ),
        rate: RDS_RATE,
        note: "wfm channel at +200 kHz with rds on -> PI D3C2 \"SDR-M4\" + a 1 kHz tone"
            .to_string(),
    });

    out.push(Fixture {
        stem: "navtex_518_48k".to_string(),
        iq: at(
            testgen::navtex::transmission(
                "ZCZC DA07\r\nGALE WARNING\r\nGERMAN BIGHT\r\nNNNN",
                AUDIO,
            ),
            3_000.0,
            AUDIO,
        ),
        rate: AUDIO,
        note: "navtex channel at +3 kHz -> DA07 navigational warning, \"GALE WARNING\"".to_string(),
    });

    out.push(Fixture {
        stem: "acars_downlink_240k".to_string(),
        iq: at(
            testgen::acars::transmission(
                &testgen::acars::Block {
                    mode: '2',
                    registration: ".D-AIBC",
                    ack: '\x15',
                    label: "H1",
                    block_id: '3',
                    seq_no: Some("M01A"),
                    flight: Some("LH0400"),
                    text: "SDR-- FIXTURE",
                    more: false,
                },
                NARROW,
            ),
            -40_000.0,
            NARROW,
        ),
        rate: NARROW,
        note: "acars channel at -40 kHz -> D-AIBC / LH0400 [H1] \"SDR-- FIXTURE\"".to_string(),
    });

    const SUBGHZ_RATE: f64 = 500_000.0;
    out.push(Fixture {
        stem: "subghz_ev1527_500k".to_string(),
        iq: at(
            testgen::subghz::pwm(
                &testgen::subghz::Pwm {
                    bits: (0..24)
                        .map(|i| 0x0A_1B_23u32 >> (23 - i) & 1 == 1)
                        .collect(),
                    short_us: 320,
                    long_multiple: 3,
                    sync_gap_multiple: 31,
                    repeats: 6,
                },
                SUBGHZ_RATE,
            ),
            100_000.0,
            SUBGHZ_RATE,
        ),
        rate: SUBGHZ_RATE,
        note: "subghz channel at +100 kHz -> 24-bit PWM 0A1B23, address 0A1B2 button 3".to_string(),
    });

    const ATV_RATE: f64 = 2_400_000.0;
    let atv_params = sdrmm_wire::AtvParams::default();
    out.push(Fixture {
        stem: "atv_ccir625_2m4".to_string(),
        iq: at(
            testgen::atv::bars(&testgen::atv::AtvSource::new(&atv_params, ATV_RATE), 2),
            200_000.0,
            ATV_RATE,
        ),
        rate: ATV_RATE,
        note: "atv channel at +200 kHz -> 625/25 AM, five vertical bars black to white".to_string(),
    });

    const SSTV_RATE: f64 = 48_000.0;
    const SSTV_MODE: sdrmm_wire::SstvMode = sdrmm_wire::SstvMode::Robot36;
    let sstv = testgen::sstv::transmission(SSTV_MODE, &testgen::sstv::bars(SSTV_MODE), 16_000.0);
    out.push(Fixture {
        stem: "sstv_robot36_48k".to_string(),
        iq: at(
            testgen::resample(&sstv, 16_000.0, SSTV_RATE),
            4_000.0,
            SSTV_RATE,
        ),
        rate: SSTV_RATE,
        note: "sstv channel at +4 kHz -> Robot 36, eight colour bars white to black".to_string(),
    });

    out
}

fn write_fixture(
    dir: &Path,
    stem_name: &str,
    iq: &[Complex<f32>],
    rate: f64,
    center_hz: f64,
    hw: &str,
    note: &str,
) -> Result<()> {
    let stem = dir.join(stem_name);
    for path in [
        sdrmm_recorder::meta_path(&stem),
        sdrmm_recorder::data_path(&stem),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("replace {}", path.display())),
        }
    }
    let mut writer = sdrmm_recorder::SigmfWriter::create(&stem, rate, center_hz, hw)
        .with_context(|| format!("create fixture {stem_name}"))?;
    writer
        .write_block(iq)
        .with_context(|| format!("write fixture {stem_name}"))?;
    writer
        .finalize()
        .with_context(|| format!("finalize fixture {stem_name}"))?;

    let reader = sdrmm_recorder::SigmfReader::open(&stem)
        .with_context(|| format!("re-open fixture {stem_name}"))?;
    ensure!(
        reader.total_samples() == iq.len() as u64,
        "fixture {stem_name} readback: {} samples on disk, {} rendered",
        reader.total_samples(),
        iq.len()
    );
    println!(
        "{stem_name}: {} samples, {:.2} s @ {} Msps — {note}",
        iq.len(),
        iq.len() as f64 / rate,
        rate / 1e6,
    );
    Ok(())
}

fn ensure_web_deps(root: &Path) -> Result<()> {
    if root.join("web/node_modules").is_dir() {
        return Ok(());
    }
    run(
        PNPM,
        &["--dir", "web", "install", "--frozen-lockfile"],
        root,
    )
}

fn run(program: &str, args: &[&str], cwd: &Path) -> Result<()> {
    run_with_env(program, args, cwd, &[])
}

fn run_with_env(program: &str, args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Result<()> {
    println!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .envs(env.iter().copied())
        .current_dir(cwd)
        .status()
        .with_context(|| format!("failed to spawn `{program}` (is it installed?)"))?;
    if !status.success() {
        bail!("`{program} {}` failed with {status}", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    const LOCK: &str = r#"# This file is automatically @generated by Cargo.

[[package]]
name = "gpu-allocator"
version = "0.28.0"
dependencies = [
 "log",
 "windows 0.62.2",
]

[[package]]
name = "wgpu-hal"
version = "30.0.0"
dependencies = [
 "naga",
 "windows 0.62.2",
 "windows-core 0.62.2",
]

[[package]]
name = "tao"
version = "0.35.3"
dependencies = [
 "windows",
]

[[package]]
name = "windows"
version = "0.61.3"
"#;

    #[test]
    fn reads_the_version_a_duplicated_dependency_resolves_to() {
        assert_eq!(
            locked_dependency(LOCK, "wgpu-hal", "windows").as_deref(),
            Some("0.62.2")
        );
    }

    #[test]
    fn falls_back_to_the_package_entry_when_the_name_stands_alone() {
        assert_eq!(
            locked_dependency(LOCK, "tao", "windows").as_deref(),
            Some("0.61.3")
        );
    }

    #[test]
    fn a_prefix_of_another_crate_name_is_not_a_match() {
        assert_eq!(
            locked_dependency(LOCK, "wgpu-hal", "windows-core").as_deref(),
            Some("0.62.2")
        );
        assert_eq!(locked_dependency(LOCK, "gpu-allocator", "ash"), None);
        assert_eq!(locked_dependency(LOCK, "absent", "windows"), None);
    }

    #[test]
    fn the_workspace_lock_pairs_wgpu_hal_and_gpu_allocator_on_one_windows_rs() {
        check_windows_rs_alignment(&root()).expect("windows-rs alignment");
    }
}
