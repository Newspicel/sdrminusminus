use std::{path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, ensure};

use super::{accessibility, disclaim, running::Running, sdrmm, signal::Signal};

pub const TOOL: &str = "SDR-- app";

const IDENTIFIER: &str = "dev.newspicel.sdrmm";

pub fn build(root: &Path) -> Result<()> {
    crate::web_build(root)?;
    let status = Command::new("cargo")
        .args(["build", "-p", "sdrmm-desktop", "--release"])
        .current_dir(root)
        .status()
        .context("run cargo build")?;
    ensure!(
        status.success(),
        "cargo build -p sdrmm-desktop --release failed"
    );
    Ok(())
}

pub fn launch(root: &Path, signal: &Signal, receivers: usize, work: &Path) -> Result<Running> {
    let home = work.join("home");
    link_recordings(signal, &home)?;
    let process = disclaim::spawn(
        &root.join("target/release/sdrmm-desktop"),
        &[],
        &[("HOME", home.as_os_str())],
        &work.join("desktop.log"),
    )?;
    let mut running = Running::disclaimed(process, TOOL);
    let pid = running.pid();
    let mut port = None;
    running.wait_for("listen", Duration::from_secs(60), || {
        port = listening(pid).ok();
        port.is_some()
    })?;
    let url = format!("http://127.0.0.1:{}", port.context("no port")?);
    sdrmm::bring_up(&mut running, &url, receivers)?;
    accessibility::listen(&mut running, receivers)?;
    Ok(running)
}

fn link_recordings(signal: &Signal, home: &Path) -> Result<()> {
    let recordings = home
        .join("Library/Application Support")
        .join(IDENTIFIER)
        .join("recordings");
    std::fs::create_dir_all(&recordings)
        .with_context(|| format!("create {}", recordings.display()))?;
    for entry in std::fs::read_dir(&signal.dir)? {
        let source = entry?.path();
        let name = source.file_name().context("recording without a name")?;
        std::fs::hard_link(&source, recordings.join(name))
            .with_context(|| format!("link {}", source.display()))?;
    }
    Ok(())
}

fn listening(pid: u32) -> Result<u16> {
    let out = Command::new("lsof")
        .args([
            "-a",
            "-p",
            &pid.to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-P",
            "-n",
            "-Fn",
        ])
        .output()
        .context("run lsof")?;
    port(&String::from_utf8(out.stdout)?)
}

fn port(text: &str) -> Result<u16> {
    text.lines()
        .find_map(|line| line.strip_prefix('n'))
        .and_then(|address| address.rsplit_once(':'))
        .context("not listening yet")?
        .1
        .parse()
        .context("lsof printed no port")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_comes_from_the_name_field() {
        assert_eq!(port("p42\nf12\nn127.0.0.1:54321\n").unwrap(), 54321);
        assert!(port("p42\n").is_err());
    }
}
