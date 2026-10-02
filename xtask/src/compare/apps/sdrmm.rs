use std::{net::TcpListener, path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, ensure};
use sdrmm_wire::{
    CreateWorkspaceRequest, CreatedRowId, DeviceSetStatus, PatchApplyReport, StateSnapshot,
};
use serde::{Serialize, de::DeserializeOwned};

use super::{running::Running, signal::Signal, workspace};

pub const HEADLESS: &str = "SDR-- headless";
pub const APP: &str = "SDR-- app";

pub fn build(root: &Path) -> Result<()> {
    crate::web_build(root)?;
    let status = Command::new("cargo")
        .args(["build", "-p", "sdrmm", "--release"])
        .current_dir(root)
        .status()
        .context("run cargo build")?;
    ensure!(status.success(), "cargo build -p sdrmm --release failed");
    Ok(())
}

pub struct Server {
    pub running: Running,
    pub url: String,
}

pub fn launch(root: &Path, signal: &Signal, receivers: usize, work: &Path) -> Result<Server> {
    let port = free_port()?;
    let url = format!("http://127.0.0.1:{port}");
    let mut command = Command::new(root.join("target/release/sdrmm"));
    command
        .arg("--bind")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--db")
        .arg(work.join("sdrmm.db"))
        .arg("--recordings-dir")
        .arg(&signal.dir);
    let mut running = Running::spawn(command, "sdrmm", &work.join("sdrmm.log"))?;
    running.wait_for("answer", Duration::from_secs(60), || {
        get::<serde_json::Value>(&url, "/api/about").is_ok()
    })?;
    open_workspace(&url, receivers)?;
    running.wait_for("start playing", Duration::from_secs(30), || {
        playing(&url, receivers)
    })?;
    Ok(Server { running, url })
}

fn open_workspace(base: &str, receivers: usize) -> Result<()> {
    let request = CreateWorkspaceRequest {
        name: "Compare".to_owned(),
        snapshot: Some(workspace::snapshot(receivers)),
    };
    let created: CreatedRowId = parse(&send(base, "POST", "/api/workspaces", &request)?)?;
    let path = format!("/api/workspaces/{}", created.id);
    for index in 0..receivers {
        let node = workspace::channel(index);
        send(
            base,
            "PUT",
            &format!("{path}/channels/{node}"),
            &workspace::settings(index),
        )?;
    }
    send(
        base,
        "POST",
        &format!("{path}/activate"),
        &serde_json::json!({}),
    )?;
    let report: PatchApplyReport = parse(&send(
        base,
        "POST",
        &format!("{path}/apply"),
        &serde_json::json!({}),
    )?)?;
    ensure!(
        report.refused.is_empty() && report.absent.is_empty(),
        "the workspace did not come up: {report:?}"
    );
    Ok(())
}

fn playing(base: &str, receivers: usize) -> bool {
    get::<StateSnapshot>(base, "/api/state").is_ok_and(|state| {
        state.device_sets.iter().any(|device| {
            device.status == DeviceSetStatus::Running && device.channels.len() == receivers
        })
    })
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("find a free port")?;
    Ok(listener.local_addr()?.port())
}

fn get<T: DeserializeOwned>(base: &str, path: &str) -> Result<T> {
    parse(&curl(&["-sf", &format!("{base}{path}")])?)
}

fn send(base: &str, method: &str, path: &str, body: &impl Serialize) -> Result<Vec<u8>> {
    let body = serde_json::to_string(body)?;
    curl(&[
        "-sS",
        "--fail-with-body",
        "-X",
        method,
        "-H",
        "content-type: application/json",
        "-d",
        &body,
        &format!("{base}{path}"),
    ])
}

fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).context("read the server answer")
}

fn curl(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("curl")
        .args(args)
        .output()
        .context("run curl")?;
    ensure!(
        out.status.success(),
        "curl {} failed: {}{}",
        args.last().copied().unwrap_or_default(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(out.stdout)
}
