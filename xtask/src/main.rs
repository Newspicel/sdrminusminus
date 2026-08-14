//! `cargo xtask` — the only entry points (CLAUDE.md). Keeps local gates and CI in lockstep:
//! every check CI runs is runnable here first.
#![allow(clippy::expect_used)]

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use num_complex::Complex;

mod bandplan;
mod ber;
mod catalog;
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
    /// Regenerate OpenAPI + the TypeScript client (run after changing `crates/wire`).
    Codegen,
    /// Re-harvest the third-party notices from the lockfiles (run after changing a dependency).
    /// Writes `crates/server/data/notices.json` and `THIRD_PARTY_NOTICES.md`; `check` verifies
    /// both are current.
    Licenses,
    /// Server + Vite dev server with HMR.
    Dev,
    /// Full local gate = fmt + clippy + biome + oxlint + tsgo + web build + codegen-drift.
    Check,
    /// Full test suite (uses `device-virtual`; no hardware).
    Test,
    /// Dependency advisories (`deny.toml`). Separate from `check`: it needs `cargo-deny` and
    /// fetches the RustSec database, so it can go red on a day the tree did not change.
    Audit,
    /// The Playwright smoke flow against the real server on `device-virtual`.
    /// Separate from `test` because it needs a browser binary; CI installs one.
    Smoke,
    /// Regenerate the synthesized SigMF fixtures in `fixtures/` (see fixtures/README.md).
    Fixtures,
    Bandplan {
        #[arg(long)]
        offline: bool,
    },
    Ber {
        /// Harness entry to sweep, e.g. `bpsk-ideal`. An unknown name lists the known ones.
        entry: String,
        /// Output directory (default `target/ber/`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// The nightly grid (0–10 dB in 1 dB steps) instead of the fast smoke subset.
        #[arg(long)]
        full: bool,
    },
    /// Re-render every icon from `assets/icon.svg` (run after changing the mark, commit the
    /// output). Not part of `check`: the renders are committed precisely so no build needs a
    /// rasteriser, and re-rendering them to compare would defeat that.
    Icons,
    Dist {
        /// Cross-compile for this target triple instead of the host.
        #[arg(long)]
        target: Option<String>,
    },
    /// Build the Tauri desktop shell. Without `--bundles` this is the compile gate, which is
    /// what CI runs on every pull request; with it, the installers for this host.
    Desktop {
        /// Cross-compile for this target triple instead of the host.
        #[arg(long)]
        target: Option<String>,
        /// Comma-separated Tauri bundle targets (`dmg`, `deb`, `appimage`, `msi`, `nsis`).
        #[arg(long)]
        bundles: Option<String>,
    },
    /// Verify a staged private Soapy runtime contains the core, baseline modules, and notices.
    SoapyBundleCheck {
        /// Staged `soapy` directory (defaults to the desktop resource directory).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Resolve every library a built macOS artifact loads, the way the loader will on a machine
    /// that did not build it. Reads `.app` bundles, staged directories and loose binaries.
    LinkCheck {
        /// Artifact to walk.
        path: PathBuf,
        /// Leaf-name fragment of a library the artifact is not meant to carry (repeatable) —
        /// the headless archive links the pinned SoapySDR and leaves it to the host.
        #[arg(long = "external")]
        external: Vec<String>,
    },
    SetVersion {
        /// Semver, with or without a leading `v`.
        version: String,
    },
    UpdaterManifest {
        /// Release version the manifest offers, with or without a leading `v`.
        #[arg(long)]
        version: String,
        /// Directory holding the collected bundles and their `.sig` files.
        #[arg(long)]
        dir: PathBuf,
        /// Prefix the download URL of each artifact is built from.
        #[arg(long)]
        base_url: String,
        /// Output path (default `<dir>/latest.json`).
        #[arg(long)]
        out: Option<PathBuf>,
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
    }
}

/// How to spawn pnpm. `CreateProcess` applies no PATHEXT search, so on Windows the bare name
/// resolves to pnpm's extensionless shell script — not an executable image — and every web step
/// dies with "program not found". Naming the shim is what finds it; std applies the batch-file
/// quoting rules once the extension is explicit (CVE-2024-24576).
#[cfg(windows)]
const PNPM: &str = "pnpm.cmd";
#[cfg(not(windows))]
const PNPM: &str = "pnpm";

/// Workspace root = xtask's parent directory.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent")
        .to_path_buf()
}

/// Emit `openapi.json` from the Rust types (no server needed) and regenerate the
/// typed TS client from it.
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
    let mut vite = Command::new(PNPM);
    vite.args(["--dir", "web", "dev"]).current_dir(root);
    // Detach Vite from the terminal: it must not read the TTY it shares with the server
    // and the shell (a non-foreground reader is stopped by SIGTTIN, and an orphaned one
    // dies with EIO), and it must die as a whole pnpm → node tree — killing only the pnpm
    // parent orphans the actual Vite process, which keeps writing over the shell prompt.
    vite.stdin(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut vite, 0);
    let mut vite = vite
        .spawn()
        .context("spawn vite dev server (is pnpm installed?)")?;

    let status = Command::new("cargo")
        .args(["run", "-p", "sdrmm", "--", "--dev-cors"])
        .current_dir(root)
        .status()
        .context("run server");

    kill_process_tree(&mut vite);
    let status = status?;
    if !status.success() {
        bail!("server exited with {status}");
    }
    Ok(())
}

/// Terminate `child` and everything in its process group (see the spawn site), escalating to
/// SIGKILL if the group ignores SIGTERM.
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

fn check(root: &Path) -> Result<()> {
    // Ordered cheapest-first, and that ordering is load-bearing: formatting and the web lints
    // answer in seconds, clippy takes minutes from a cold cache, and CI pays for the whole job
    // either way. A misformatted pull request should not cost a full workspace build to say so.
    check_toolchain_pins(root)?;
    check_baked_in_fixtures(root)?;
    catalog::check(root)?;
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
    // Every backend must stay optional for deliberate virtual-only builds.
    run(
        "cargo",
        &["check", "-p", "sdrmm", "--no-default-features"],
        root,
    )?;
    // …and the shape a release artifact actually ships must build: Soapy is the canonical
    // local-hardware backend, with network receivers alongside it.
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

    // Codegen must be reproducible: regenerate and fail on any diff.
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

    // Same contract for the notices: a dependency bump that does not update them ships a
    // release whose attribution describes the previous one.
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

/// Node and pnpm are pinned in four files, and nothing updates them together: `web/package.json`
/// (`packageManager`), the Dockerfile's `FROM node:` and `pnpm@`, and each workflow's
/// `NODE_VERSION`/`PNPM_VERSION`. Dependabot's docker ecosystem bumps the Dockerfile on its own,
/// which would leave the container building the UI on one Node while CI and every release
/// artifact built it on another — divergence that produces no error, just two different bundles.
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

/// `fixtures/` is gitignored wholesale (generated pairs must never land in a commit), so an
/// `include_bytes!` of one compiles on the machine that generated it and fails to compile
/// anywhere else — including every CI job, which cannot regenerate what an older generator
/// wrote. A fixture baked into a binary is therefore only legitimate if it is force-added, and
/// this asserts exactly that before clippy spends minutes discovering it the hard way.
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

/// The text between the first `open` and the next `close` after it.
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
        "soapy,net-client,gpu-fft".to_string(),
    ]
}

/// `pnpm --dir web build` — typechecks and emits `web/dist`, which `crates/server` embeds.
/// Shared by `check` and `dist` so they can never build the UI differently.
fn web_build(root: &Path) -> Result<()> {
    ensure_web_deps(root)?;
    run(PNPM, &["--dir", "web", "build"], root)
}

/// `web/dist` exists *and holds a UI*. `crates/server/build.rs` creates the directory when it
/// is missing so the crate compiles on a fresh clone, which means a release built without the
/// UI succeeds and silently ships the "not built" placeholder page. Every path that produces a
/// shippable artifact asserts this instead.
fn assert_web_dist(root: &Path) -> Result<()> {
    let index = root.join("web/dist/index.html");
    ensure!(
        index.exists(),
        "{} is missing after the web build: the artifact would embed an empty UI",
        index.display()
    );
    Ok(())
}

/// Build the portable headless artifact and pack
/// it the way its platform's users expect. The output contract — one archive per target at
/// `dist/sdrmm-<version>-<triple>.{tar.gz,zip}`, holding the binary plus README and LICENSE —
/// is what the release workflow uploads, so the build flags live here and only here.
fn dist(root: &Path, target: Option<&str>) -> Result<()> {
    web_build(root)?;
    assert_web_dist(root)?;
    ensure_target(root, target)?;

    // rust-embed only embeds bytes in non-debug builds, so a debug artifact serves nothing.
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

    // README.md and LICENSE are release contents, not optional: a missing one fails here.
    std::fs::copy(&built, staged.join(exe))
        .with_context(|| format!("cannot stage {}", built.display()))?;
    for doc in ["README.md", "LICENSE", "THIRD_PARTY_NOTICES.md"] {
        std::fs::copy(root.join(doc), staged.join(doc))
            .with_context(|| format!("cannot stage {doc}"))?;
    }

    // Linux only: on Apple targets the linker's ad-hoc code signature is required for the
    // binary to load at all, and `strip` invalidates it.
    if triple.contains("linux") {
        run(
            "strip",
            &[staged.join(exe).to_str().expect("utf8 path")],
            root,
        )?;
    }

    let archive = archive(root, &out, &name, windows)?;
    println!("dist: {}", archive.display());
    Ok(())
}

/// Pack `dist/<name>/` into the archive format that target's users expect.
fn archive(root: &Path, out: &Path, name: &str, windows: bool) -> Result<PathBuf> {
    let ext = if windows { "zip" } else { "tar.gz" };
    let path = out.join(format!("{name}.{ext}"));
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("replace {}", path.display())),
    }

    if windows {
        // PowerShell rather than the bundled bsdtar: `tar` on a Windows box resolves to Git's
        // GNU tar as often as to the system's libarchive one, and GNU tar cannot write zip.
        //
        // `ZipFile::CreateFromDirectory` rather than `Compress-Archive`, for its last argument:
        // `includeBaseDirectory = $true` puts the staged folder at the root of the archive, so a
        // zip unpacks to the same layout as the tar.gz. Compress-Archive's own behaviour there
        // depends on whether the path is given with a trailing wildcard.
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

/// Install the rust-std for a cross target on demand.
///
/// `rust-toolchain.toml` deliberately lists no `targets` (see the comment there): each one is
/// ~150 MB that `rustup show` would fetch in every job, including the ones that only build for
/// their own host. Adding it at the point of use keeps a cross build a single command locally
/// and in CI both.
fn ensure_target(root: &Path, target: Option<&str>) -> Result<()> {
    let Some(triple) = target else {
        return Ok(());
    };
    if triple == host_triple()? {
        return Ok(());
    }
    run("rustup", &["target", "add", triple], root)
}

/// The host target triple as rustc reports it — the name a `dist` archive carries when no
/// `--target` was asked for.
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

/// Build the Tauri shell. The workspace's `default-members` deliberately skips this
/// crate — it pulls the platform webview toolchain — so without this command nothing builds it
/// until release day.
///
/// `--bundles` shells out to the Tauri CLI, which is the local path. CI's release job drives
/// `tauri-action` instead, because signing a macOS bundle needs the certificate imported into a
/// temporary keychain first and that is the action's job, not this one's.
fn desktop(root: &Path, target: Option<&str>, bundles: Option<&str>) -> Result<()> {
    ensure_target(root, target)?;
    let features = release_features();

    let Some(bundles) = bundles else {
        // No web build on this path, deliberately: the shell's `frontendDist` is its own
        // placeholder page and `crates/server` embeds `web/dist` through a build script that
        // creates the directory when it is absent, so compiling needs no UI on disk. Bundling
        // below does, because that artifact is the one a user runs.
        //
        // Clippy rather than a plain build: the rest of the workspace is gated at
        // `-D warnings` and this crate has no reason to be held to less.
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
    // `createUpdaterArtifacts` plus a configured pubkey makes the CLI *require* a private key,
    // so a local bundle would otherwise fail outright for anyone who does not hold one. Unsigned
    // is the right outcome here — only the release workflow has to produce a signature, and it
    // asserts that it did — but it is said out loud rather than left to be inferred from a
    // missing `.sig`.
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
        // Before the `--`: the triple selects the bundle target, not just the cargo one.
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
    ensure!(
        has("rtlsdr"),
        "{} contains no SoapyRTLSDR module",
        dir.display()
    );
    ensure!(
        has("hackrf"),
        "{} contains no SoapyHackRF module",
        dir.display()
    );
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
    // A module is a wrapper around a driver library that ships as its own file, and the two are
    // staged by different rules — the modules by name, their libraries by walking what each one
    // records. 0.1.2 shipped every module on macOS and not one driver library, which no check
    // then in place could see and no user could either until they plugged a radio in.
    let outside_modules: Vec<&String> = files
        .iter()
        .zip(&names)
        .filter(|(path, name)| {
            let inside = |part: &str| path.components().any(|c| c.as_os_str() == part);
            // Not the notices: conda's own metadata is named after the packages it records, so
            // `librtlsdr-0.6.0-….json` would answer for the library it is only the receipt for.
            !inside("modules0.8")
                && !inside("licenses")
                && [".dylib", ".so", ".dll"]
                    .iter()
                    .any(|ext| name.contains(ext))
        })
        .map(|(_, name)| name)
        .collect();
    for driver in [
        "rtlsdr",
        "hackrf",
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
    for license in [
        "soapyhackrf-mit",
        "hackrf-gpl-2.0-or-later",
        "hackrf-bsd-3-clause",
    ] {
        ensure!(
            has(license),
            "{} contains no {license} license text",
            dir.display()
        );
    }
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

/// Stamp `version` across everything a release artifact carries it in.
///
/// The workspace manifest is the only place it is written: `apps/desktop/tauri.conf.json`
/// deliberately omits `version` so Tauri falls back to the crate's, and the archive names come
/// from xtask's own `CARGO_PKG_VERSION` — which is this same field, so a stamped tree cannot
/// name an artifact one version and have it report another.
fn set_version(root: &Path, version: &str) -> Result<()> {
    let version = version.strip_prefix('v').unwrap_or(version);
    // Rejected here rather than at the far end of the matrix. Every bound below is the Windows
    // MSI bundler's (`tauri-bundler`'s `validate_wix_version`), and MSI's ProductVersion has no
    // field a prerelease or build-metadata suffix could go in — `0.2.0-rc.1` does not fail at
    // the tag, it fails ~20 minutes later in the one job out of twelve that builds an installer.
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

    // Every workspace member inherits the field, so the lock's own entries are now stale and a
    // `--locked` release build would refuse to start. `--workspace --offline` rewrites exactly
    // those entries and cannot reach the network to drag anything else along with them.
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

/// Check the whole dependency graph — including the Tauri shell, which the default gate skips —
/// against the RustSec database. The policy and every standing exception live in `deny.toml`.
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

/// The browser smoke flow. It drives the built UI served by the real binary, which is how a
/// release artifact runs — the dev server would test a composition nobody gets. Playwright
/// starts and stops the server itself (`web/playwright.config.ts`), so the only thing to do
/// here is make sure the UI it serves is the one just built.
fn smoke(root: &Path) -> Result<()> {
    ensure_web_deps(root)?;
    web_build(root)?;
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
    /// What a listener should see once the fixture is playing — printed so the file is
    /// self-describing without an expected-output file beside it.
    note: String,
}

fn aprs_burst() -> Vec<Complex<f32>> {
    use sdrmm_channels::{AprsTx, ChannelCtx, ChannelTx, TxPayload, testgen};
    use sdrmm_wire::{AprsMode, AprsParams, ChannelParams, ChannelSettings};

    let settings = ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        params: ChannelParams::Aprs(AprsParams {
            mode: AprsMode::Afsk1200,
            ..AprsParams::default()
        }),
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

/// One playable fixture per wave-1 decoder. Each sits at a deliberate channel offset so
/// playing it also exercises the DDC, except ADS-B, which fills its whole channel.
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
        // The one fixture rendered by a real modulator rather than a testgen encoder: `AprsTx`
        // keys its own 48 kHz channel rate, so the burst is resampled up to the device's.
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

    out
}

/// Write one pair and read it straight back, so writer/reader drift can never ship a bad
/// fixture.
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
    // Fixtures are regenerable by definition, so a re-run replaces them. The recorder's stem
    // claim is deliberately atomic (it protects live recordings from each other), which would
    // otherwise make `xtask fixtures` fail on its second run.
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

/// Install web dependencies if they are missing.
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
