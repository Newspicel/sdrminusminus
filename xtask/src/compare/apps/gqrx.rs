use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::Path,
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, ensure};

use super::{
    running::Running,
    signal::{self, CENTER_HZ, RATE, Signal},
};

pub const TOOL: &str = "GQRX";

const BINARY: &str = "/Applications/Gqrx.app/Contents/MacOS/gqrx";
const PORT: u16 = 7356;

pub fn install() -> Result<()> {
    if Path::new(BINARY).exists() {
        return Ok(());
    }
    let status = Command::new("brew")
        .args(["install", "--cask", "gqrx"])
        .status()
        .context("run brew")?;
    ensure!(status.success(), "brew install --cask gqrx failed");
    Ok(())
}

pub fn version() -> Result<String> {
    let out = Command::new(BINARY)
        .arg("--help")
        .output()
        .context("run gqrx --help")?;
    parse_version(&String::from_utf8_lossy(&out.stdout)).context("no version in gqrx --help")
}

fn parse_version(text: &str) -> Option<String> {
    text.lines()
        .find(|line| line.starts_with("Gqrx "))
        .and_then(|line| line.split_whitespace().last())
        .map(str::to_owned)
}

pub fn launch(signal: &Signal, receivers: usize, work: &Path) -> Result<Running> {
    ensure!(receivers == 1, "{TOOL} has one receiver");
    let config = work.join("gqrx.conf");
    std::fs::write(&config, settings(signal))
        .with_context(|| format!("write {}", config.display()))?;
    let mut command = Command::new(BINARY);
    command.arg("--conf").arg(&config);
    let mut running = Running::spawn(command, TOOL, &work.join("gqrx.log"))?;
    running.wait_for("open remote control", Duration::from_secs(60), || {
        TcpStream::connect(("127.0.0.1", PORT)).is_ok()
    })?;
    let mut remote = Remote::connect()?;
    remote.expect("M FM", "RPRT 0")?;
    remote.expect("U DSP 1", "RPRT 0")?;
    remote.expect("u DSP", "1")?;
    Ok(running)
}

fn settings(signal: &Signal) -> String {
    let device = format!(
        "file={},rate={},freq={},repeat=true,throttle=true",
        signal.raw.display(),
        RATE as u64,
        CENTER_HZ as u64
    );
    format!(
        "[General]\nconfigversion=4\ncrashed=false\n\n\
         [input]\ndevice=\"{device}\"\nsample_rate={}\nfrequency={}\n\n\
         [receiver]\noffset={}\n\n\
         [fft]\nfft_rate={}\nfft_size={}\n\n\
         [remote_control]\nenabled=true\nport={PORT}\nallowed_hosts=127.0.0.1, ::1, ::ffff:127.0.0.1\n",
        RATE as u64,
        CENTER_HZ as u64,
        signal::offset_hz(0) as i64,
        signal::FFT_FPS,
        signal::FFT_BINS
    )
}

struct Remote {
    reader: BufReader<TcpStream>,
}

impl Remote {
    fn connect() -> Result<Self> {
        let stream = TcpStream::connect(("127.0.0.1", PORT)).context("connect to gqrx")?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    fn expect(&mut self, command: &str, answer: &str) -> Result<()> {
        self.reader
            .get_mut()
            .write_all(format!("{command}\n").as_bytes())?;
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .with_context(|| format!("gqrx did not answer `{command}`"))?;
        ensure!(
            line.trim() == answer,
            "gqrx answered `{command}` with `{}`",
            line.trim()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_version_is_the_last_word_of_the_title() {
        let help = "Usage: gqrx [options]\nGqrx software defined radio receiver 2.17.7\n";
        assert_eq!(parse_version(help).as_deref(), Some("2.17.7"));
    }

    #[test]
    fn the_file_input_repeats_in_real_time() {
        let signal = Signal {
            dir: PathBuf::from("/iq"),
            raw: PathBuf::from("/iq/x.sigmf-data"),
        };
        let text = settings(&signal);
        assert!(text.contains(
            "file=/iq/x.sigmf-data,rate=10000000,freq=100000000,repeat=true,throttle=true"
        ));
        assert!(text.contains("crashed=false"));
        assert!(text.contains("fft_rate=30\nfft_size=1024"));
    }
}
