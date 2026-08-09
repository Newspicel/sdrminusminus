//! `cargo xtask` — the only entry points (CLAUDE.md). Keeps local gates and CI in lockstep:
//! every check CI runs is runnable here first.
//!
//! Dev tooling: `expect` on infallible workspace-path invariants is fine here (startup code).
#![allow(clippy::expect_used)]

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

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
    /// Release artifacts (stub until M5 packaging).
    Dist,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Codegen => codegen(&root()),
        Cmd::Dev => dev(&root()),
        Cmd::Check => check(&root()),
        Cmd::Test => test(&root()),
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
    let mut vite = Command::new("pnpm")
        .args(["--dir", "web", "dev"])
        .current_dir(root)
        .spawn()
        .context("spawn vite dev server (is pnpm installed?)")?;

    let status = Command::new("cargo")
        .args(["run", "-p", "sdrmm", "--", "--dev-cors"])
        .current_dir(root)
        .status()
        .context("run server")?;

    let _ = vite.kill();
    if !status.success() {
        bail!("server exited with {status}");
    }
    Ok(())
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
