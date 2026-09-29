use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sdrmm_device::{
    Capture, CaptureConfig, DeviceDriver, DeviceError, Direction, DuplexState, RxSink, SdrDevice,
    TxStream, fan_out, lock,
    net::{Adopted, Endpoint},
};
use sdrmm_wire::{AgcGain, Capabilities, DeviceInfo, DeviceSettings};

use crate::{
    board::{Board, Start},
    control::Control,
    discovery::Identity,
    link::{DEFAULT_PORT, Ports},
    rx::{IqConverter, RxRadio},
    tx::AntsdrTx,
};

mod ad9361;
mod apply;
mod board;
mod caps;
mod chdr;
mod control;
mod discovery;
#[cfg(test)]
mod end_to_end;
mod link;
mod radio;
mod rate;
mod regs;
mod rx;
#[cfg(test)]
mod sim;
mod spi;
mod tx;

const DRIVER_ID: &str = "antsdr";
const SINK_BLOCK_SAMPLES: usize = 32_768;
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
const OPEN_ASK_TIMEOUT: Duration = Duration::from_millis(600);
const DEFAULT_RATE: f64 = 2_048_000.0;
const DEFAULT_CENTER_HZ: f64 = 100e6;
const DEFAULT_GAIN_DB: f64 = 40.0;

#[derive(Debug)]
pub struct AntsdrDriver {
    adopted: Adopted,
    known: Mutex<BTreeMap<Endpoint, Identity>>,
    well_known: Vec<String>,
    swept: Mutex<Option<Instant>>,
}

impl Default for AntsdrDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl AntsdrDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::searching(discovery::WELL_KNOWN.iter().map(ToString::to_string))
    }

    #[must_use]
    pub fn searching(hosts: impl IntoIterator<Item = String>) -> Self {
        Self {
            adopted: Adopted::default(),
            known: Mutex::new(BTreeMap::new()),
            well_known: hosts.into_iter().collect(),
            swept: Mutex::new(None),
        }
    }

    fn info(&self, endpoint: &Endpoint) -> DeviceInfo {
        let identity = lock(&self.known).get(endpoint).cloned();
        DeviceInfo {
            driver: DRIVER_ID.to_string(),
            key: endpoint.to_string(),
            label: match &identity {
                Some(identity) => format!("{} {}", identity.label(), endpoint.host()),
                None => format!("AntSDR UHD {endpoint}"),
            },
            serial: identity.and_then(|identity| identity.serial),
            profile: None,
        }
    }

    fn due_for_a_sweep(&self) -> bool {
        let mut swept = lock(&self.swept);
        if swept.is_some_and(|at| at.elapsed() < SWEEP_INTERVAL) {
            return false;
        }
        *swept = Some(Instant::now());
        true
    }

    fn remember(&self, identity: Identity, port: u16) {
        let key = format!("{}:{port}", identity.address.ip());
        let Ok(endpoint) = Endpoint::parse(&key, DEFAULT_PORT) else {
            return;
        };
        if self.adopted.adopt(endpoint.clone()) {
            lock(&self.known).insert(endpoint, identity);
        }
    }
}

impl DeviceDriver for AntsdrDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let mut serials: Vec<String> = Vec::new();
        self.adopted
            .list()
            .iter()
            .map(|endpoint| self.info(endpoint))
            .filter(|info| match &info.serial {
                Some(serial) if serials.contains(serial) => false,
                Some(serial) => {
                    serials.push(serial.clone());
                    true
                }
                None => true,
            })
            .collect()
    }

    fn probe_deep(&self) -> Vec<DeviceInfo> {
        if self.due_for_a_sweep() {
            let ports = Ports {
                control: DEFAULT_PORT,
            };
            for identity in discovery::ask(&self.well_known, ports, discovery::REPLY_TIMEOUT) {
                tracing::info!(at = %identity.address.ip(), "found an AntSDR running UHD firmware");
                self.remember(identity, DEFAULT_PORT);
            }
        }
        self.probe()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let endpoint = Endpoint::parse(&info.key, DEFAULT_PORT)?;
        let device = AntsdrDevice::open(&endpoint)?;
        if let Some(identity) = &device.identity {
            lock(&self.known).insert(endpoint, identity.clone());
        }
        Ok(Box::new(device))
    }

    fn resolve(&self, key: &str) -> Option<DeviceInfo> {
        let endpoint = Endpoint::parse(key, DEFAULT_PORT)
            .inspect_err(|e| tracing::warn!("antsdr endpoint: {e}"))
            .ok()?;
        if !self.adopted.adopt(endpoint.clone()) {
            tracing::warn!(%endpoint, "too many antsdr endpoints; refusing to adopt another");
            return None;
        }
        Some(self.info(&endpoint))
    }
}

pub struct AntsdrDevice {
    host: String,
    ports: Ports,
    identity: Option<Identity>,
    control: Arc<Control>,
    board: Arc<Mutex<Board>>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    duplex: Arc<Mutex<DuplexState>>,
    capture: Capture<RxRadio>,
}

impl AntsdrDevice {
    fn open(endpoint: &Endpoint) -> Result<Self, DeviceError> {
        let host = endpoint.host().to_string();
        let ports = Ports {
            control: endpoint.port(),
        };
        let identity = discovery::ask(std::slice::from_ref(&host), ports, OPEN_ASK_TIMEOUT)
            .into_iter()
            .next();
        if identity.is_none() {
            tracing::debug!(host, "no discovery answer; trying the control port anyway");
        }
        let control = Arc::new(Control::connect(&host, ports.control)?);
        let mut board = Board::open(
            control.clone(),
            Start {
                rate: DEFAULT_RATE,
                lanes: 1,
                center_hz: DEFAULT_CENTER_HZ,
                bandwidth: DEFAULT_RATE,
            },
        )?;
        for lane in 0..board.radios() {
            board.set_rx_gain(lane, DEFAULT_GAIN_DB)?;
        }
        let capabilities = caps::capabilities(board.radios(), 1);
        let settings = apply::initial(&board, &capabilities, DEFAULT_RATE, DEFAULT_GAIN_DB);
        tracing::info!(
            at = %endpoint,
            board = identity.as_ref().map_or("unknown", |identity| identity.board.as_str()),
            radios = board.radios(),
            "opened an AntSDR running UHD firmware"
        );
        Ok(Self {
            host,
            ports,
            identity,
            control,
            board: Arc::new(Mutex::new(board)),
            duplex: Arc::new(Mutex::new(DuplexState::new(capabilities.duplex))),
            capabilities,
            settings,
            capture: Capture::new(),
        })
    }

    fn radios(&self) -> usize {
        lock(&self.board).radios()
    }
}

impl SdrDevice for AntsdrDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, delta: &DeviceSettings) -> Result<(), DeviceError> {
        let streaming = self.capture.is_running();
        let mut board = lock(&self.board);
        let result = apply::apply(
            &mut board,
            &mut self.capabilities,
            &mut self.settings,
            delta,
            streaming,
        )
        .and_then(|()| board.settle());
        if result.is_err() {
            self.settings.center_hz = Some(board.frequency());
        }
        result
    }

    fn agc_gains(&self) -> Result<Vec<AgcGain>, DeviceError> {
        let mut board = lock(&self.board);
        let mut gains = Vec::new();
        for lane in 0..self.capabilities.rx_streams {
            let resolved = self
                .settings
                .for_stream(lane, &self.capabilities.per_stream);
            if resolved.agc.as_ref().is_some_and(|agc| agc.on) {
                gains.push(AgcGain {
                    stream: lane,
                    value_db: board.gain(lane as usize)?,
                });
            }
        }
        Ok(gains)
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let lanes = sinks.len();
        if lanes == 0 || lanes > self.capabilities.rx_streams as usize {
            return Err(DeviceError::Unsupported(format!(
                "this radio streams {} lanes, got {lanes}",
                self.capabilities.rx_streams
            )));
        }
        lock(&self.duplex).claim(Direction::Rx)?;
        let started = self.start_capture(sinks, lanes);
        if started.is_err() {
            if let Err(e) = lock(&self.board).set_streaming(0) {
                tracing::debug!("antsdr receive release: {e}");
            }
            lock(&self.duplex).release(Direction::Rx);
        }
        started
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
        if let Err(e) = lock(&self.board).set_streaming(0) {
            tracing::warn!("antsdr receive stop: {e}");
        }
        lock(&self.duplex).release(Direction::Rx);
    }

    fn tx_start_channels(&mut self, channels: &[u32]) -> Result<Box<dyn TxStream>, DeviceError> {
        let lanes = channels.len();
        if lanes == 0 || lanes > self.radios() || channels.iter().copied().ne(0..lanes as u32) {
            return Err(DeviceError::Unsupported(format!(
                "this radio transmits on its lanes in order, got channels {channels:?}"
            )));
        }
        lock(&self.duplex).claim(Direction::Tx)?;
        match AntsdrTx::open(
            &self.host,
            self.ports,
            lanes,
            self.board.clone(),
            self.duplex.clone(),
        ) {
            Ok(stream) => Ok(Box::new(stream)),
            Err(e) => {
                lock(&self.duplex).release(Direction::Tx);
                Err(e)
            }
        }
    }
}

impl AntsdrDevice {
    fn start_capture(&mut self, sinks: Vec<RxSink>, lanes: usize) -> Result<(), DeviceError> {
        let timeline = {
            let mut board = lock(&self.board);
            board.set_streaming(lanes)?;
            board.timeline()
        };
        let rate = self.settings.sample_rate.unwrap_or(DEFAULT_RATE);
        let radio = Arc::new(RxRadio::new(
            self.control.clone(),
            self.host.clone(),
            self.ports,
            lanes,
            rate,
            timeline.clone(),
        ));
        let converter = IqConverter::new(timeline, rx::block_frames(rate) * lanes);
        self.capture.start(
            radio,
            converter,
            fan_out(sinks, SINK_BLOCK_SAMPLES),
            CaptureConfig {
                block_samples: SINK_BLOCK_SAMPLES * lanes,
                ..CaptureConfig::new("sdrmm-antsdr-rx", DRIVER_ID)
                    .with_sample_rate(Some(rate * lanes as f64))
            },
        )
    }
}

impl Drop for AntsdrDevice {
    fn drop(&mut self) {
        self.capture.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_listed_until_an_address_is_found_or_given() {
        let driver = AntsdrDriver::searching([]);
        assert_eq!(driver.id(), DRIVER_ID);
        assert!(driver.probe().is_empty());
    }

    #[test]
    fn an_address_is_adopted_under_the_control_port() {
        let driver = AntsdrDriver::searching([]);
        let info = driver.resolve("192.168.1.10").expect("addressable");
        assert_eq!(info.id(), "antsdr:192.168.1.10:49200");
        assert_eq!(info.label, "AntSDR UHD 192.168.1.10:49200");
        assert!(driver.probe().contains(&info));
        assert_eq!(driver.resolve("192.168.1.10:49200"), Some(info));
        assert!(driver.resolve("192.168.1.10:nope").is_none());
    }

    #[test]
    fn a_board_that_answered_is_listed_by_its_name_once() {
        let driver = AntsdrDriver::searching([]);
        for port in [DEFAULT_PORT, DEFAULT_PORT] {
            driver.remember(
                Identity {
                    address: ([192, 168, 1, 10], 49100).into(),
                    serial: Some("ABC123".to_string()),
                    board: "E310  v2".to_string(),
                },
                port,
            );
        }
        let listed = driver.probe();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].label, "AntSDR E310 ABC123 192.168.1.10");
        assert_eq!(listed[0].serial.as_deref(), Some("ABC123"));
    }

    #[test]
    fn a_search_leaves_the_network_alone_for_a_while() {
        let driver = AntsdrDriver::searching([]);
        assert!(driver.due_for_a_sweep());
        assert!(!driver.due_for_a_sweep());
    }
}
