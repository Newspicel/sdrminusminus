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
    /// Regenerate the synthesized SigMF fixtures in `fixtures/` (see fixtures/README.md).
    Fixtures,
    /// Release artifacts (stub until M5 packaging).
    Dist,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Codegen => codegen(&root()),
        Cmd::Dev => dev(&root()),
        Cmd::Check => check(&root()),
        Cmd::Test => test(&root()),
        Cmd::Fixtures => fixtures(&root()),
        Cmd::Dist => {
            println!("dist: release packaging lands at M5 (PLAN §16).");
            Ok(())
        }
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
    // Rust gate (default members; the Tauri app is built separately, see workspace manifest).
    run("cargo", &["fmt", "--all", "--", "--check"], root)?;
    run(
        "cargo",
        &["clippy", "--all-targets", "--", "-D", "warnings"],
        root,
    )?;
    // The Soapy-free build must stay buildable (PLAN §3: minimal Pi images).
    run(
        "cargo",
        &["check", "-p", "sdrmm", "--no-default-features"],
        root,
    )?;

    // Web gate.
    ensure_web_deps(root)?;
    run("pnpm", &["--dir", "web", "exec", "biome", "ci", "."], root)?;
    run(
        "pnpm",
        &["--dir", "web", "exec", "oxlint", "--type-aware"],
        root,
    )?;
    run("pnpm", &["--dir", "web", "exec", "tsgo", "--noEmit"], root)?;
    run("pnpm", &["--dir", "web", "build"], root)?;

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

fn test(root: &Path) -> Result<()> {
    run("cargo", &["test", "--all-targets"], root)?;
    ensure_web_deps(root)?;
    run("pnpm", &["--dir", "web", "test"], root)?;
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
        iq: at(
            testgen::aprs::afsk1200(
                &testgen::aprs::ui_frame(
                    "DL1ABC-9",
                    "APRS",
                    &["WIDE1-1"],
                    "!5230.00N/01324.00E>sdr-- fixture",
                ),
                NARROW,
            ),
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
        // The device must run at exactly 2 Msps: ADS-B fills its channel, so a resampling
        // DDC cannot carry it (PLAN §18).
        note: "adsb channel at 0 Hz, device at exactly 2 Msps -> 3C6444/DLH123 at FL380"
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
    println!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("failed to spawn `{program}` (is it installed?)"))?;
    if !status.success() {
        bail!("`{program} {}` failed with {status}", args.join(" "));
    }
    Ok(())
}
