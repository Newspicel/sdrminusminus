use std::{path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, ensure};

use super::running::Running;

const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const LISTEN: &str = "scripts/compare-listen.mjs";

pub fn install() -> Result<()> {
    ensure!(
        Path::new(CHROME).exists(),
        "SDR-- is measured with its UI in Google Chrome, install it to {CHROME}"
    );
    Ok(())
}

pub fn open(root: &Path, url: &str, receivers: usize, work: &Path) -> Result<Running> {
    let profile = work.join("chrome");
    let mut command = Command::new(CHROME);
    command.args(flags(&profile, url));
    let mut running = Running::spawn(command, "Chrome", &work.join("chrome.log"))?;
    let port_file = profile.join("DevToolsActivePort");
    let mut port = None;
    running.wait_for("open DevTools", Duration::from_secs(30), || {
        port = std::fs::read_to_string(&port_file)
            .ok()
            .and_then(|text| debug_port(&text).ok());
        port.is_some()
    })?;
    listen(root, port.context("no DevTools port")?, receivers)?;
    running.alive()?;
    Ok(running)
}

fn flags(profile: &Path, url: &str) -> Vec<String> {
    vec![
        format!("--user-data-dir={}", profile.display()),
        "--remote-debugging-port=0".to_owned(),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        "--autoplay-policy=no-user-gesture-required".to_owned(),
        "--disable-backgrounding-occluded-windows".to_owned(),
        "--disable-renderer-backgrounding".to_owned(),
        "--disable-background-timer-throttling".to_owned(),
        "--window-size=1280,800".to_owned(),
        format!("--app={url}"),
    ]
}

fn debug_port(text: &str) -> Result<u16> {
    text.lines()
        .next()
        .context("DevToolsActivePort is empty")?
        .trim()
        .parse()
        .context("DevToolsActivePort has no port")
}

fn listen(root: &Path, port: u16, receivers: usize) -> Result<()> {
    let status = Command::new("node")
        .arg(LISTEN)
        .arg(format!("http://127.0.0.1:{port}"))
        .arg(receivers.to_string())
        .current_dir(root.join("web"))
        .status()
        .context("run node")?;
    ensure!(status.success(), "could not start audio in the SDR-- UI");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_is_the_first_line() {
        assert_eq!(debug_port("53211\n/devtools/browser/abc\n").unwrap(), 53211);
        assert!(debug_port("").is_err());
    }

    #[test]
    fn chrome_plays_without_a_click_and_never_throttles() {
        let flags = flags(Path::new("/w/chrome"), "http://127.0.0.1:1");
        assert!(flags.contains(&"--autoplay-policy=no-user-gesture-required".to_owned()));
        assert!(flags.contains(&"--disable-backgrounding-occluded-windows".to_owned()));
        assert_eq!(flags.last().unwrap(), "--app=http://127.0.0.1:1");
    }
}
