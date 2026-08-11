//! `cargo xtask` — the only entry points (CLAUDE.md). Keeps local gates and CI in lockstep:
//! every check CI runs is runnable here first.
//!
//! Dev tooling: `expect` on infallible workspace-path invariants is fine here (startup code).
#![allow(clippy::expect_used)]

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use num_complex::Complex;

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
    /// Server + Vite dev server with HMR.
    Dev,
    /// Full local gate = fmt + clippy + biome + oxlint + tsgo + web build + codegen-drift.
    Check,
    /// Full test suite (uses `device-virtual`; no hardware).
    Test,
    /// Dependency advisories (`deny.toml`). Separate from `check`: it needs `cargo-deny` and
    /// fetches the RustSec database, so it can go red on a day the tree did not change.
    Audit,
    /// The Playwright smoke flow against the real server on `device-virtual` (PLAN §14).
    /// Separate from `test` because it needs a browser binary; CI installs one.
    Smoke,
    /// Regenerate the synthesized SigMF fixtures in `fixtures/` (see fixtures/README.md).
    Fixtures,
    /// Build the self-contained release archive for this host (PLAN §15).
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
    /// Stamp a release version across the workspace (PLAN §15). CI runs this from the tag.
    SetVersion {
        /// Semver, with or without a leading `v`.
        version: String,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Codegen => codegen(&root()),
        Cmd::Dev => dev(&root()),
        Cmd::Check => check(&root()),
        Cmd::Test => test(&root()),
        Cmd::Audit => audit(&root()),
        Cmd::Smoke => smoke(&root()),
        Cmd::Fixtures => fixtures(&root()),
        Cmd::Dist { target } => dist(&root(), target.as_deref()),
        Cmd::Desktop { target, bundles } => desktop(&root(), target.as_deref(), bundles.as_deref()),
        Cmd::SetVersion { version } => set_version(&root(), &version),
    }
}

/// Workspace root = xtask's parent directory.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent")
        .to_path_buf()
}

/// Emit `openapi.json` from the Rust types (no server needed, PLAN §4 step 1) and regenerate the
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
        "pnpm",
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
    // Vite (HMR) proxies /api to the server; the server serves the API + WS on :8080.
    let mut vite = Command::new("pnpm");
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
    run("cargo", &["fmt", "--all", "--", "--check"], root)?;

    // Web gate.
    ensure_web_deps(root)?;
    run("pnpm", &["--dir", "web", "exec", "biome", "ci", "."], root)?;
    run(
        "pnpm",
        &["--dir", "web", "exec", "oxlint", "--type-aware"],
        root,
    )?;
    run("pnpm", &["--dir", "web", "exec", "tsgo", "--noEmit"], root)?;

    // Rust gate (default members; the Tauri app is `xtask desktop`, see workspace manifest).
    run(
        "cargo",
        &["clippy", "--all-targets", "--", "-D", "warnings"],
        root,
    )?;
    // Every backend must stay optional (PLAN §3: minimal Pi images).
    run(
        "cargo",
        &["check", "-p", "sdrmm", "--no-default-features"],
        root,
    )?;
    // …and the shape a release artifact actually ships must build: native backends in, Soapy
    // out, so a missing libSoapySDR costs exotic-device support and not startup (PLAN §15).
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

    // Codegen must be reproducible: regenerate and fail on any diff (PLAN §4 step 5).
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
    println!("check: all gates green");
    Ok(())
}

/// The cargo flags a release artifact is built with: native RTL-SDR and HackRF compiled in
/// (pure Rust, no C library to install), SoapySDR left out (`soapysdr-sys` dynamically links
/// libSoapySDR, which PLAN §15 forbids as a launch dependency of a release artifact).
fn release_features() -> [String; 3] {
    [
        "--no-default-features".to_string(),
        "--features".to_string(),
        "rtl-native,hackrf-native".to_string(),
    ]
}

/// `pnpm --dir web build` — typechecks and emits `web/dist`, which `crates/server` embeds.
/// Shared by `check` and `dist` so they can never build the UI differently.
fn web_build(root: &Path) -> Result<()> {
    ensure_web_deps(root)?;
    run("pnpm", &["--dir", "web", "build"], root)
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

/// Build the self-contained headless artifact (PLAN §15: "release artifacts just run") and pack
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
    // A stale member from an earlier run would be packed into the archive as if it belonged.
    if staged.exists() {
        std::fs::remove_dir_all(&staged)
            .with_context(|| format!("cannot clear {}", staged.display()))?;
    }
    std::fs::create_dir_all(&staged)
        .with_context(|| format!("cannot create {}", staged.display()))?;

    // README.md and LICENSE are release contents, not optional: a missing one fails here.
    std::fs::copy(&built, staged.join(exe))
        .with_context(|| format!("cannot stage {}", built.display()))?;
    for doc in ["README.md", "LICENSE"] {
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
        // COPYFILE_DISABLE stops macOS bsdtar from emitting ._ AppleDouble members for
        // extended attributes; ignored by GNU tar.
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

/// Build the Tauri shell (PLAN §10). The workspace's `default-members` deliberately skips this
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
    run("cargo", &args, &root.join("apps/desktop"))
}

/// Stamp `version` across everything a release artifact carries it in.
///
/// The workspace manifest is the only place it is written: `apps/desktop/tauri.conf.json`
/// deliberately omits `version` so Tauri falls back to the crate's, and the archive names come
/// from xtask's own `CARGO_PKG_VERSION` — which is this same field, so a stamped tree cannot
/// name an artifact one version and have it report another.
fn set_version(root: &Path, version: &str) -> Result<()> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let (core, _pre) = version.split_once(['-', '+']).unwrap_or((version, ""));
    let numeric: Vec<&str> = core.split('.').collect();
    ensure!(
        numeric.len() == 3
            && numeric
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
        "`{version}` is not a semver version: need major.minor.patch, e.g. 0.2.0"
    );

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
    run("cargo", &["test", "--all-targets"], root)?;
    ensure_web_deps(root)?;
    run("pnpm", &["--dir", "web", "test"], root)?;
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
    run(
        "pnpm",
        &["--dir", "web", "exec", "playwright", "test"],
        root,
    )?;
    Ok(())
}

/// Synthesize the fixture SigMF pairs in `fixtures/` (PLAN §14). Deterministic renders — the
/// siggen for the record/replay fixture, and `channels::testgen`'s reference modulators for
/// one fixture per wave-1 decoder — so the pairs are regenerated on demand and never
/// committed (fixtures/README.md).
///
/// The decoder fixtures are the same encoders the decoder unit tests and the engine
/// end-to-end run use, so a fixture can never drift from what the decoders are tested
/// against. They exist to be *played*: open one as a `virtual:file:` device, add the matching
/// channel at the stated offset, and the decoder log fills up with the documented message.
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

/// The APRS fixture's burst, keyed by the modulator that pairs with the decoder it is meant to
/// feed (PLAN §20) — nothing here reaches an antenna; it is written to a file.
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
        // 2 Msps is the *lowest* rate ADS-B runs at — one sample per half-chip. The decoder
        // reads whatever the radio gives it up to 4 Msps (PLAN §18), so this fixture is the
        // floor of that range rather than the only point in it.
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
        "pnpm",
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
