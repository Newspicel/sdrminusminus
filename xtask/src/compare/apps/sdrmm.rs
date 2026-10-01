use std::{net::TcpListener, path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, ensure};
use sdrmm_wire::{
    ChannelParams, ChannelSettings, CreateChannelRequest, CreateDeviceSetRequest, CreatedId,
    DeviceSetStatus, NfmParams, NoiseBlankerSettings, RECORDING_DRIVER_ID, Squelch, StateSnapshot,
};
use serde::{Serialize, de::DeserializeOwned};

use super::{
    running::Running,
    signal::{self, CENTER_HZ, Signal},
};

pub fn build(root: &Path) -> Result<()> {
    let status = Command::new("cargo")
        .args(["build", "-p", "sdrmm", "--release"])
        .current_dir(root)
        .status()
        .context("run cargo build")?;
    ensure!(status.success(), "cargo build -p sdrmm --release failed");
    Ok(())
}

pub fn launch(root: &Path, signal: &Signal, receivers: usize, work: &Path) -> Result<Running> {
    let port = free_port()?;
    let base = format!("http://127.0.0.1:{port}");
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
        get::<serde_json::Value>(&base, "/api/about").is_ok()
    })?;
    let device_id = format!("{RECORDING_DRIVER_ID}:{}", signal::STEM);
    let set: CreatedId = post(
        &base,
        "/api/devicesets",
        &CreateDeviceSetRequest { device_id },
    )?;
    for index in 0..receivers {
        let request = CreateChannelRequest {
            stream: 0,
            settings: ChannelSettings {
                frequency_hz: CENTER_HZ + signal::offset_hz(index),
                squelch: Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                blanker: NoiseBlankerSettings::default(),
            },
        };
        let _: CreatedId = post(
            &base,
            &format!("/api/devicesets/{}/channels", set.id),
            &request,
        )?;
    }
    running.wait_for("start playing", Duration::from_secs(30), || {
        playing(&base, set.id, receivers)
    })?;
    Ok(running)
}

fn playing(base: &str, set: u32, receivers: usize) -> bool {
    get::<StateSnapshot>(base, "/api/state").is_ok_and(|state| {
        state.device_sets.iter().any(|device| {
            device.id == set
                && device.status == DeviceSetStatus::Running
                && device.channels.len() == receivers
        })
    })
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("find a free port")?;
    Ok(listener.local_addr()?.port())
}

fn get<T: DeserializeOwned>(base: &str, path: &str) -> Result<T> {
    curl(&["-sf", &format!("{base}{path}")])
}

fn post<T: DeserializeOwned>(base: &str, path: &str, body: &impl Serialize) -> Result<T> {
    let body = serde_json::to_string(body)?;
    curl(&[
        "-sS",
        "--fail-with-body",
        "-X",
        "POST",
        "-H",
        "content-type: application/json",
        "-d",
        &body,
        &format!("{base}{path}"),
    ])
}

fn curl<T: DeserializeOwned>(args: &[&str]) -> Result<T> {
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
    serde_json::from_slice(&out.stdout).context("read the server answer")
}
