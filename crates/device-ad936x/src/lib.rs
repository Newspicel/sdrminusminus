use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sdrmm_device::{
    Capture, CaptureConfig, DeviceDriver, DeviceError, Direction, DuplexState, RxSink, SdrDevice,
    TxStream, lock,
    net::{Adopted, Endpoint},
};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings, Duplex};

use crate::{
    caps::Front,
    convert::IqConverter,
    discovery::USB_PREFIX,
    iio::{Client, DEFAULT_PORT, UsbBus},
    layout::Layout,
    rx::{RxRadio, fan_out},
    source::Source,
    tx::Ad936xTx,
};

mod apply;
mod caps;
mod convert;
mod discovery;
mod iio;
mod layout;
mod rx;
mod source;
mod tx;

const DRIVER_ID: &str = "ad936x";

/// Samples of one lane pushed to a sink at a time. A buffer holds several of these, so a decoder
/// sees the signal while the rest of the buffer is still being taken apart.
const SINK_BLOCK_SAMPLES: usize = 32_768;

/// How long the well-known addresses are left alone after a search tried them. A search is what
/// every open goes through, and a radio being reconnected to must not dial the whole network on
/// each attempt.
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);

/// Every AD936x board that serves libiio, over its own ethernet or usb: AntSDR, PlutoSDR,
/// and anything else built around the same transceiver.
#[derive(Debug)]
pub struct Ad936xDriver {
    adopted: Adopted,
    serials: Mutex<BTreeMap<Endpoint, String>>,
    well_known: Vec<Endpoint>,
    swept: Mutex<Option<Instant>>,
}

impl Default for Ad936xDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Ad936xDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::searching(discovery::WELL_KNOWN.iter().map(|host| host.to_string()))
    }

    /// A driver whose search tries these addresses instead of the ones the boards ship on.
    #[must_use]
    pub fn searching(hosts: impl IntoIterator<Item = String>) -> Self {
        Self {
            adopted: Adopted::default(),
            serials: Mutex::new(BTreeMap::new()),
            well_known: discovery::endpoints(hosts),
            swept: Mutex::new(None),
        }
    }

    /// The adopted addresses, less any that turned out to be a radio already listed: a board
    /// reachable by name, by address and over usb is one radio.
    fn endpoints(&self, listed: &[DeviceInfo]) -> Vec<DeviceInfo> {
        let serials = lock(&self.serials);
        let mut known: Vec<String> = listed
            .iter()
            .filter_map(|info| info.serial.clone())
            .collect();
        self.adopted
            .list()
            .into_iter()
            .filter_map(|endpoint| {
                let serial = serials.get(&endpoint).cloned();
                if let Some(serial) = &serial {
                    if known.contains(serial) {
                        return None;
                    }
                    known.push(serial.clone());
                }
                Some(net_info(&endpoint, serial))
            })
            .collect()
    }

    fn remember(&self, endpoint: Endpoint, serial: Option<&str>) {
        if let Some(serial) = serial {
            lock(&self.serials).insert(endpoint, serial.to_string());
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
}

fn net_info(endpoint: &Endpoint, serial: Option<String>) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: endpoint.to_string(),
        label: format!("AD936x {endpoint}"),
        serial,
        profile: None,
    }
}

fn usb_info(radio: &discovery::UsbRadio) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: radio.key.clone(),
        label: radio.label.clone(),
        serial: radio.serial.clone(),
        profile: None,
    }
}

impl DeviceDriver for Ad936xDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        let mut found: Vec<DeviceInfo> = discovery::usb_radios().iter().map(usb_info).collect();
        found.extend(self.endpoints(&found));
        found
    }

    /// Tries the addresses these radios ship on as well, so one straight out of its box is found
    /// without the operator having to know where it lives.
    fn probe_deep(&self) -> Vec<DeviceInfo> {
        if self.due_for_a_sweep() {
            let known = self.adopted.list();
            let untried: Vec<Endpoint> = self
                .well_known
                .iter()
                .filter(|endpoint| !known.contains(endpoint))
                .cloned()
                .collect();
            for found in discovery::sweep(untried) {
                tracing::info!(endpoint = %found.endpoint, "found an AD936x radio at a well-known address");
                self.adopted.adopt(found.endpoint.clone());
                self.remember(found.endpoint, found.serial.as_deref());
            }
        }
        self.probe()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let device = Ad936xDevice::open(source(&info.key)?)?;
        if let Source::Net(endpoint) = &device.source {
            self.remember(endpoint.clone(), device.serial.as_deref());
        }
        Ok(Box::new(device))
    }

    fn resolve(&self, key: &str) -> Option<DeviceInfo> {
        if key.starts_with(USB_PREFIX) {
            return discovery::attached(key).map(|radio| usb_info(&radio));
        }
        let endpoint = Endpoint::parse(key, DEFAULT_PORT)
            .inspect_err(|e| tracing::warn!("ad936x endpoint: {e}"))
            .ok()?;
        if !self.adopted.adopt(endpoint.clone()) {
            tracing::warn!(%endpoint, "too many ad936x endpoints; refusing to adopt another");
            return None;
        }
        let serial = lock(&self.serials).get(&endpoint).cloned();
        Some(net_info(&endpoint, serial))
    }
}

fn source(key: &str) -> Result<Source, DeviceError> {
    if key.starts_with(USB_PREFIX) {
        let info = discovery::find_usb(key)?;
        return Ok(Source::Usb(UsbBus::open(&info)?));
    }
    Ok(Source::Net(Endpoint::parse(key, DEFAULT_PORT)?))
}

pub struct Ad936xDevice {
    client: Arc<Client>,
    source: Source,
    serial: Option<String>,
    layout: Layout,
    front: Front,
    capabilities: Capabilities,
    settings: DeviceSettings,
    duplex: Arc<Mutex<DuplexState>>,
    capture: Capture<RxRadio>,
}

impl Ad936xDevice {
    fn open(source: Source) -> Result<Self, DeviceError> {
        let client = Client::new(source.open()?);
        match client.version() {
            Ok(version) => tracing::debug!(%version, "iiod answered"),
            Err(e) => tracing::debug!("iiod version: {e}"),
        }
        let context = client.context()?;
        let layout = Layout::read(&context)?;
        let front = Front::read(&client, &context, &layout)?;
        let mut capabilities = caps::capabilities(&front, &layout);
        // A transmit buffer needs a conversation of its own. Over usb those are the endpoint
        // couples the board built, and a board with only two cannot hold both directions open.
        if capabilities.duplex == Duplex::Full && !source.full_duplex() {
            capabilities.duplex = Duplex::Half;
        }
        let settings = apply::read_settings(&client, &capabilities, &front, &layout);
        tracing::info!(
            radio = context
                .attribute("hw_model")
                .unwrap_or(context.description.as_str()),
            at = %source,
            rx = capabilities.rx_streams,
            tx = capabilities.tx_streams,
            "opened an AD936x radio"
        );
        Ok(Self {
            duplex: Arc::new(Mutex::new(DuplexState::new(capabilities.duplex))),
            client: Arc::new(client),
            source,
            serial: context.attribute("hw_serial").map(str::to_string),
            layout,
            front,
            capabilities,
            settings,
            capture: Capture::new(),
        })
    }

    fn lanes(&self, output: bool, wanted: usize) -> Result<usize, DeviceError> {
        let have = if output {
            self.layout.tx_streams()
        } else {
            self.layout.rx_streams()
        };
        if wanted == 0 || wanted > have {
            return Err(DeviceError::Unsupported(format!(
                "this radio has {have} {} streams, got {wanted}",
                if output { "tx" } else { "rx" }
            )));
        }
        Ok(wanted)
    }
}

impl SdrDevice for Ad936xDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    /// Writes reach the radio one at a time, so one it refuses part-way leaves the ones before
    /// it in place. What is reported afterwards is read back rather than assumed either way.
    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let (next, writes) = apply::plan(
            settings,
            &self.capabilities,
            &self.front,
            &self.layout,
            &self.settings,
        )?;
        match apply::execute(&self.client, &self.layout.phy, &writes) {
            Ok(()) => {
                self.settings = next;
                Ok(())
            }
            Err(e) => {
                let held = apply::read_settings(
                    &self.client,
                    &self.capabilities,
                    &self.front,
                    &self.layout,
                );
                self.settings.merge_from(&held);
                Err(e)
            }
        }
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        let lanes = self.lanes(false, sinks.len())?;
        let stream =
            self.layout.rx.clone().ok_or_else(|| {
                DeviceError::Unsupported("this radio does not receive".to_string())
            })?;
        let format = stream.format;
        lock(&self.duplex).claim(Direction::Rx)?;
        let radio = Arc::new(RxRadio::new(
            self.source.clone(),
            stream,
            lanes,
            self.settings.sample_rate,
        ));
        let converter = IqConverter::new(format, radio.buffer_samples() * lanes);
        let started = self.capture.start(
            radio,
            converter,
            fan_out(sinks, SINK_BLOCK_SAMPLES),
            CaptureConfig {
                // Lanes share one buffer, so the block the supervisor cuts must hold whole
                // frames of every lane and the rate it counts a gap in is the frame rate.
                block_samples: SINK_BLOCK_SAMPLES * lanes,
                ..CaptureConfig::new("sdrmm-ad936x-rx", DRIVER_ID)
                    .with_sample_rate(self.settings.sample_rate.map(|rate| rate * lanes as f64))
            },
        );
        if started.is_err() {
            lock(&self.duplex).release(Direction::Rx);
        }
        started
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
        lock(&self.duplex).release(Direction::Rx);
    }

    fn tx_start_channels(&mut self, channels: &[u32]) -> Result<Box<dyn TxStream>, DeviceError> {
        let lanes = self.lanes(true, channels.len())?;
        if channels.iter().copied().ne(0..lanes as u32) {
            return Err(DeviceError::Unsupported(format!(
                "this radio transmits on its lanes in order, got channels {channels:?}"
            )));
        }
        let stream =
            self.layout.tx.clone().ok_or_else(|| {
                DeviceError::Unsupported("this radio does not transmit".to_string())
            })?;
        let samples = rx::buffer_samples(
            self.settings.sample_rate.unwrap_or(0.0),
            stream.sample_bytes(lanes),
        );
        lock(&self.duplex).claim(Direction::Tx)?;
        match Ad936xTx::open(&self.source, &stream, lanes, samples, self.duplex.clone()) {
            Ok(stream) => Ok(Box::new(stream)),
            Err(e) => {
                lock(&self.duplex).release(Direction::Tx);
                Err(e)
            }
        }
    }
}

impl Drop for Ad936xDevice {
    fn drop(&mut self) {
        self.capture.stop();
        self.client.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_driver_offers_nothing_until_it_finds_or_is_told_about_a_radio() {
        let driver = Ad936xDriver::new();
        assert_eq!(driver.id(), DRIVER_ID);
        assert!(
            driver
                .probe()
                .iter()
                .all(|found| found.key.starts_with(USB_PREFIX)),
            "only attached radios appear before an address is given"
        );
    }

    #[test]
    fn an_address_is_adopted_and_then_listed_under_the_iiod_port() {
        let driver = Ad936xDriver::new();
        let info = driver.resolve("192.168.1.10").expect("addressable");
        assert_eq!(info.id(), "ad936x:192.168.1.10:30431");
        assert_eq!(info.label, "AD936x 192.168.1.10:30431");
        assert!(driver.probe().contains(&info));
        assert_eq!(
            driver.resolve("192.168.1.10:30431"),
            Some(info),
            "two spellings of one address are one radio"
        );
    }

    #[test]
    fn a_key_that_addresses_nothing_is_not_adopted() {
        let driver = Ad936xDriver::new();
        assert!(driver.resolve("192.168.1.10:not-a-port").is_none());
        assert!(driver.resolve("usb-nothing-is-plugged-in-here").is_none());
    }

    #[test]
    fn two_addresses_that_answered_with_one_serial_are_listed_as_one_radio() {
        let driver = Ad936xDriver::searching([]);
        let by_name = driver.resolve("pluto.local").expect("addressable");
        let by_address = driver.resolve("192.168.2.1").expect("addressable");
        driver.remember(
            Endpoint::parse("pluto.local", DEFAULT_PORT).expect("endpoint"),
            Some("1044734c960500111e002e0041984fc267"),
        );
        driver.remember(
            Endpoint::parse("192.168.2.1", DEFAULT_PORT).expect("endpoint"),
            Some("1044734c960500111e002e0041984fc267"),
        );
        let listed = driver.probe();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(
            listed[0].serial.as_deref(),
            Some("1044734c960500111e002e0041984fc267")
        );
        assert!(
            driver.resolve(&by_name.key).is_some() && driver.resolve(&by_address.key).is_some(),
            "either spelling still opens the radio"
        );
    }

    #[test]
    fn a_search_leaves_the_network_alone_for_a_while_after_trying_it() {
        let driver = Ad936xDriver::searching([]);
        assert!(driver.due_for_a_sweep());
        assert!(!driver.due_for_a_sweep(), "one search per interval");
    }

    #[test]
    fn a_key_says_which_way_the_radio_is_reached() {
        assert!(matches!(
            source("192.168.1.10").expect("an address"),
            Source::Net(_)
        ));
        assert!(
            source("usb-0000deadbeef").is_err(),
            "a usb key names a radio that has to be attached"
        );
    }
}
