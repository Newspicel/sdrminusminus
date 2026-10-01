use std::time::{Duration, Instant};

use sdrmm_device::DeviceError;

use crate::{
    caps::{Profile, Remote},
    link::{Link, Port, Stop, Wait},
    proto::{
        Bits, DataHeader, Features, Identity, Limits, crc32, legacy_range, parse_gain_max,
        parse_range, rate_index,
    },
};

pub(crate) const BAUDS: [u32; 3] = [2_000_000, 1_000_000, 921_600];
const BOOT_WINDOW: Duration = Duration::from_secs(6);
const PROBE_REPLY: Duration = Duration::from_millis(250);
const REPLY: Duration = Duration::from_secs(2);
const TRANSFER_SLACK: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) enum BurstError {
    Damaged(String),
    Stopped,
    Fatal(DeviceError),
}

impl From<Wait> for BurstError {
    fn from(wait: Wait) -> Self {
        match wait {
            Wait::TimedOut => Self::Damaged("transfer timed out".to_string()),
            Wait::Stopped => Self::Stopped,
            Wait::Failed(error) => Self::Fatal(error),
        }
    }
}

pub(crate) struct Session {
    port: Port,
    baud: u32,
    nonce: u64,
    applied: Option<Remote>,
    payload: Vec<u8>,
}

impl Session {
    pub(crate) fn connect(link: Box<dyn Link>, stop: Stop) -> Result<(Self, Profile), DeviceError> {
        let mut port = Port::new(link, stop);
        let mut nonce = 0;
        let baud = find_baud(&mut port, &mut nonce)?;
        let mut session = Self {
            port,
            baud,
            nonce,
            applied: None,
            payload: Vec::new(),
        };
        let profile = session.query()?;
        session.payload = vec![0; Bits::Ten.payload_bytes(profile.identity.max_samples)];
        Ok((session, profile))
    }

    pub(crate) const fn baud(&self) -> u32 {
        self.baud
    }

    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn query(&mut self) -> Result<Profile, DeviceError> {
        let identity = Identity::parse(&self.ask("INFO")?)?;
        let features = Features::parse(&self.ask("CAPS")?)?;
        let limits = if features.has("RXLIMITS") {
            Limits::parse(&self.ask("LIMITS?")?)?
        } else {
            Limits::legacy(parse_gain_max(&self.ask("GAIN?")?)?)
        };
        let range_mhz = if features.has("TUNEEXT") {
            parse_range(&self.ask("RANGE?")?)?
        } else {
            legacy_range()
        };
        Ok(Profile {
            identity,
            features,
            limits,
            range_mhz,
        })
    }

    fn ask(&mut self, command: &str) -> Result<String, DeviceError> {
        self.port.send(command)?;
        let reply = self
            .port
            .line(Instant::now() + REPLY)
            .map_err(|wait| waited(command, wait))?;
        refused(command, &reply)?;
        Ok(reply)
    }

    pub(crate) fn sync(&mut self, profile: &Profile, desired: &Remote) -> Result<(), DeviceError> {
        for command in profile.commands(self.applied.as_ref(), desired) {
            self.applied = None;
            let reply = self.ask(&command)?;
            if reply != "OK" {
                return Err(DeviceError::Io(format!("{command}: {reply}")));
            }
        }
        self.applied = Some(*desired);
        Ok(())
    }

    pub(crate) fn burst(&mut self, remote: &Remote) -> Result<DataHeader, BurstError> {
        let index = rate_index(remote.rate).ok_or_else(|| {
            BurstError::Fatal(DeviceError::Unsupported(format!(
                "sample_rate {}",
                remote.rate
            )))
        })?;
        let command = format!("{} {} {index}", remote.bits.command(), remote.burst);
        self.port.send(&command).map_err(BurstError::Fatal)?;
        let line = self.port.line(Instant::now() + REPLY)?;
        if line.starts_with("ERR") {
            refused(&command, &line).map_err(BurstError::Fatal)?;
            return Err(BurstError::Damaged(line));
        }
        let header = DataHeader::parse(&line).map_err(BurstError::Damaged)?;
        if header.samples != remote.burst {
            return Err(BurstError::Damaged(format!(
                "asked for {} samples, got {}",
                remote.burst, header.samples
            )));
        }
        let bytes = remote.bits.payload_bytes(header.samples);
        let deadline = Instant::now() + self.transfer_time(bytes);
        let payload = &mut self.payload[..bytes];
        self.port.exact(payload, deadline)?;
        if crc32(payload) != header.crc {
            return Err(BurstError::Damaged("payload CRC mismatch".to_string()));
        }
        Ok(header)
    }

    fn transfer_time(&self, bytes: usize) -> Duration {
        let wire = bytes as f64 * 10.0 / f64::from(self.baud.max(1));
        TRANSFER_SLACK + Duration::from_secs_f64(wire)
    }

    pub(crate) fn resync(&mut self) -> Result<(), DeviceError> {
        self.applied = None;
        self.nonce += 1;
        let marker = format!("SYNC {}", self.nonce);
        self.port.send("")?;
        self.port.send(&marker)?;
        let deadline = Instant::now() + self.transfer_time(self.payload.len()) + REPLY;
        loop {
            match self.port.line(deadline) {
                Ok(line) if line.ends_with(&marker) => return Ok(()),
                Ok(_) => {}
                Err(wait) => return Err(waited("SYNC", wait)),
            }
        }
    }

    pub(crate) fn stop(&self) -> &Stop {
        self.port.stop()
    }
}

fn waited(command: &str, wait: Wait) -> DeviceError {
    match wait {
        Wait::TimedOut => DeviceError::Io(format!("{command}: no reply within {REPLY:?}")),
        Wait::Stopped => DeviceError::Io(format!("{command}: stopped")),
        Wait::Failed(error) => error,
    }
}

fn refused(command: &str, reply: &str) -> Result<(), DeviceError> {
    match reply.strip_prefix("ERR ") {
        Some("busy") => Err(DeviceError::InUse(
            "another client holds the ESP-SDR".to_string(),
        )),
        Some(reason) => Err(DeviceError::Io(format!("{command}: {reason}"))),
        None => Ok(()),
    }
}

fn find_baud(port: &mut Port, nonce: &mut u64) -> Result<u32, DeviceError> {
    let deadline = Instant::now() + BOOT_WINDOW;
    let mut usable = BAUDS.to_vec();
    while Instant::now() < deadline && !usable.is_empty() {
        let mut index = 0;
        while index < usable.len() {
            let baud = usable[index];
            if port.set_baud(baud).is_err() {
                usable.remove(index);
                continue;
            }
            *nonce += 1;
            if answers(port, *nonce)? {
                return Ok(baud);
            }
            index += 1;
        }
    }
    Err(DeviceError::NotFound(
        "no ESP-SDR firmware answered at 2 MBd, 1 MBd or 921.6 kBd".to_string(),
    ))
}

fn answers(port: &mut Port, nonce: u64) -> Result<bool, DeviceError> {
    let marker = format!("SYNC {nonce}");
    port.send("")?;
    port.send(&marker)?;
    let deadline = Instant::now() + PROBE_REPLY;
    loop {
        match port.line(deadline) {
            Ok(line) if line.ends_with(&marker) => return Ok(true),
            Ok(_) => {}
            Err(Wait::TimedOut) => return Ok(false),
            Err(wait) => return Err(waited("SYNC", wait)),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::link::fake::FakeLink;

    pub(crate) const LIMITS: &str = r#"LIMITS {"gain":[0,72,1],"bandwidth":[12,67,1,0],"rates":[80000000,40000000,16000000],"bits":[8,10]}"#;

    pub(crate) fn firmware(baud: u32) -> impl FnMut(&str, u32) -> Vec<u8> + Send + 'static {
        move |line, at| {
            if at != baud {
                return vec![0x80, 0x00, 0x80];
            }
            let words: Vec<&str> = line.split(' ').collect();
            let reply = match words.as_slice() {
                ["INFO"] => "ESP32SDR 6 burst 16380".to_string(),
                ["CAPS"] => "CAPS RXLIMITS TUNEEXT GAIN HWAGC".to_string(),
                ["LIMITS?"] => LIMITS.to_string(),
                ["RANGE?"] => "RANGE 100 6000 1".to_string(),
                ["SYNC", nonce] => format!("SYNC {nonce}"),
                ["FREQ" | "BANDWIDTH", _] | ["GAIN", "HARDWARE"] | ["GAIN", "MANUAL", _] => {
                    "OK".to_string()
                }
                ["CAP16", n, _] => return capture(n.parse().unwrap_or(0)),
                _ => "ERR command".to_string(),
            };
            format!("{reply}\n").into_bytes()
        }
    }

    pub(crate) fn capture(samples: u32) -> Vec<u8> {
        let payload: Vec<u8> = (0..samples * 2).map(|n| n as u8).collect();
        let mut reply = format!("DATA {samples} {:08x} 205\n", crc32(&payload)).into_bytes();
        reply.extend(payload);
        reply
    }

    fn connect(link: &FakeLink) -> Result<(Session, Profile), DeviceError> {
        Session::connect(Box::new(link.clone()), Stop::default())
    }

    #[test]
    fn the_handshake_finds_the_baud_the_firmware_was_built_for() {
        let link = FakeLink::new(firmware(921_600));
        let (session, profile) = connect(&link).expect("connects");
        assert_eq!(session.baud(), 921_600);
        assert_eq!(profile.identity.max_samples, 16380);
        assert_eq!(profile.range_mhz, (100, 6000));
        assert_eq!(profile.limits.gain_max, 72);
        let sent = link.sent();
        assert_eq!(
            &sent[sent.len() - 4..],
            ["INFO", "CAPS", "LIMITS?", "RANGE?"]
        );
    }

    #[test]
    fn other_firmware_is_named_in_the_error() {
        let link = FakeLink::new(|line, _| {
            if line.starts_with("SYNC") {
                format!("{line}\n").into_bytes()
            } else {
                b"ch> \n".to_vec()
            }
        });
        let error = connect(&link).err().expect("refused");
        assert!(error.to_string().contains("not ESP-SDR"), "{error}");
    }

    #[test]
    fn settings_are_sent_once_and_only_when_they_move() {
        let link = FakeLink::new(firmware(2_000_000));
        let (mut session, profile) = connect(&link).expect("connects");
        let remote = profile.defaults();
        session.sync(&profile, &remote).expect("synced");
        session.sync(&profile, &remote).expect("synced");
        let tuned = Remote {
            center_mhz: 2462,
            ..remote
        };
        session.sync(&profile, &tuned).expect("synced");
        let sent = link.sent();
        assert_eq!(
            &sent[sent.len() - 4..],
            ["FREQ 2437", "BANDWIDTH 0", "GAIN HARDWARE", "FREQ 2462"]
        );
    }

    #[test]
    fn a_burst_arrives_checked() {
        let link = FakeLink::new(firmware(2_000_000));
        let (mut session, profile) = connect(&link).expect("connects");
        let remote = Remote {
            burst: 300,
            ..profile.defaults()
        };
        let header = session.burst(&remote).expect("burst");
        assert_eq!(header.samples, 300);
        assert_eq!(session.payload()[..4], [0, 1, 2, 3]);
        assert_eq!(link.sent().last().map(String::as_str), Some("CAP16 300 0"));
    }

    #[test]
    fn a_corrupt_burst_is_reported_and_resync_restores_framing() {
        let link = FakeLink::new(firmware(2_000_000));
        let (mut session, profile) = connect(&link).expect("connects");
        let remote = Remote {
            burst: 256,
            ..profile.defaults()
        };
        link.inject(b"DATA 256 00000000 205\n");
        link.inject(&[7; 512]);
        assert!(matches!(
            session.burst(&remote),
            Err(BurstError::Damaged(reason)) if reason.contains("CRC")
        ));
        session.resync().expect("resynced");
        assert!(session.burst(&remote).is_ok());
    }

    #[test]
    fn a_busy_radio_says_so() {
        let link = FakeLink::new(firmware(2_000_000));
        let (mut session, profile) = connect(&link).expect("connects");
        link.inject(b"ERR busy\n");
        assert!(matches!(
            session.burst(&profile.defaults()),
            Err(BurstError::Fatal(DeviceError::InUse(_)))
        ));
    }
}
