use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};

use super::{feeder::Feeder, running::Running, signal};

pub const TOOL: &str = "SDR++";

pub fn app(root: &Path) -> PathBuf {
    root.join("target/compare/apps/SDR++.app/Contents")
}

pub fn install(root: &Path) -> Result<()> {
    if app(root).exists() {
        return Ok(());
    }
    let dir = root.join("target/compare/apps");
    std::fs::create_dir_all(&dir)?;
    let fetched = Command::new("gh")
        .args([
            "release",
            "download",
            "nightly",
            "-R",
            "AlexandreRouma/SDRPlusPlus",
            "-p",
            "sdrpp_macos_arm.zip",
            "--clobber",
        ])
        .current_dir(&dir)
        .status()
        .context("run gh")?;
    ensure!(fetched.success(), "download SDR++ failed");
    let unpacked = Command::new("unzip")
        .args(["-oq", "sdrpp_macos_arm.zip"])
        .current_dir(&dir)
        .status()
        .context("run unzip")?;
    ensure!(unpacked.success(), "unzip SDR++ failed");
    Ok(())
}

pub fn version(root: &Path) -> Result<String> {
    let out = Command::new(app(root).join("MacOS/sdrpp"))
        .arg("--help")
        .output()
        .context("run sdrpp --help")?;
    let text = String::from_utf8_lossy(&out.stdout);
    parse_version(&text).context("no version in sdrpp --help")
}

fn parse_version(text: &str) -> Option<String> {
    let (_, rest) = text.split_once("SDR++ v")?;
    rest.split_whitespace().next().map(str::to_owned)
}

pub fn launch(root: &Path, feeder: &Feeder, receivers: usize, work: &Path) -> Result<Running> {
    let config = work.join("sdrpp");
    std::fs::create_dir_all(&config)?;
    let radios: Vec<String> = (1..=receivers)
        .map(|index| format!("Radio {index}"))
        .collect();
    write(&config, "config.json", &core(&app(root), &radios))?;
    write(&config, "radio_config.json", &radio(&radios))?;
    write(
        &config,
        "network_source_config.json",
        &network(feeder.port()),
    )?;
    let mut command = Command::new(app(root).join("MacOS/sdrpp"));
    command.arg("--root").arg(&config).arg("--autostart");
    let mut running = Running::spawn(command, TOOL, &work.join("sdrpp.log"))?;
    running.wait_for("connect to the IQ feed", Duration::from_secs(60), || {
        feeder.connected()
    })?;
    Ok(running)
}

fn core(contents: &Path, radios: &[String]) -> Value {
    let mut instances = Map::new();
    instances.insert(
        "Network Source".into(),
        json!({ "enabled": true, "module": "network_source" }),
    );
    instances.insert(
        "Audio Sink".into(),
        json!({ "enabled": true, "module": "audio_sink" }),
    );
    let mut streams = Map::new();
    let mut offsets = Map::new();
    for (index, name) in radios.iter().enumerate() {
        instances.insert(name.clone(), json!({ "enabled": true, "module": "radio" }));
        streams.insert(
            name.clone(),
            json!({ "muted": false, "sink": "Audio", "volume": 1.0 }),
        );
        offsets.insert(name.clone(), json!(signal::offset_hz(index)));
    }
    json!({
        "source": "Network",
        "frequency": signal::CENTER_HZ,
        "modulesDirectory": contents.join("Plugins"),
        "resourcesDirectory": contents.join("Resources"),
        "moduleInstances": instances,
        "streams": streams,
        "vfoOffsets": offsets,
        "fftRate": 1,
        "fftSize": 1024,
        "showWaterfall": false,
    })
}

fn radio(radios: &[String]) -> Value {
    let mut config = Map::new();
    for name in radios {
        config.insert(
            name.clone(),
            json!({
                "selectedDemodId": 0,
                "NFM": { "bandwidth": 12_500.0, "squelchEnabled": false },
            }),
        );
    }
    Value::Object(config)
}

fn network(port: u16) -> Value {
    json!({
        "Network Source": {
            "host": "127.0.0.1",
            "port": port,
            "protocol": "TCP (Client)",
            "sampleType": "Float32",
            "samplerate": signal::RATE as u64,
        }
    })
}

fn write(dir: &Path, name: &str, value: &Value) -> Result<()> {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_comes_from_the_banner() {
        let banner = "\u{1b}[0m[01/10/2026] [INFO] SDR++ v1.3.0\n-a --addr";
        assert_eq!(parse_version(banner).as_deref(), Some("1.3.0"));
        assert_eq!(parse_version("nothing"), None);
    }

    #[test]
    fn every_radio_gets_a_module_a_stream_and_its_own_offset() {
        let radios = vec!["Radio 1".to_owned(), "Radio 2".to_owned()];
        let config = core(Path::new("/app"), &radios);
        assert_eq!(config["source"], "Network");
        assert_eq!(config["moduleInstances"]["Radio 2"]["module"], "radio");
        assert_eq!(config["streams"]["Radio 1"]["sink"], "Audio");
        assert_eq!(config["vfoOffsets"]["Radio 2"], json!(signal::offset_hz(1)));
        assert_eq!(radio(&radios)["Radio 1"]["selectedDemodId"], 0);
    }

    #[test]
    fn the_feed_is_float_iq_over_tcp() {
        let source = &network(9)["Network Source"];
        assert_eq!(source["protocol"], "TCP (Client)");
        assert_eq!(source["sampleType"], "Float32");
        assert_eq!(source["samplerate"], 10_000_000);
    }
}
