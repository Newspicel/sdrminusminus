use std::sync::{Arc, Mutex};

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

/// Every AD936x board that serves libiio, over its own ethernet or usb: AntSDR, PlutoSDR,
/// and anything else built around the same transceiver.
#[derive(Debug, Default)]
pub struct Ad936xDriver {
    adopted: Adopted,
}

impl Ad936xDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn endpoints(&self) -> Vec<DeviceInfo> {
        self.adopted.list().iter().map(net_info).collect()
    }
}

fn net_info(endpoint: &Endpoint) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: endpoint.to_string(),
        label: format!("AD936x {endpoint}"),
        serial: None,
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
        found.extend(self.endpoints());
        found
    }

    /// Tries the addresses these radios ship on as well, so one straight out of its box is found
    /// without the operator having to know where it lives.
    fn probe_deep(&self) -> Vec<DeviceInfo> {
        let known = self.adopted.list();
        let untried: Vec<Endpoint> = discovery::well_known()
            .into_iter()
            .filter(|endpoint| !known.contains(endpoint))
            .collect();
        for endpoint in discovery::reachable(untried) {
            tracing::info!(%endpoint, "found an AD936x radio at a well-known address");
            self.adopted.adopt(endpoint);
        }
        self.probe()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(Ad936xDevice::open(source(&info.key)?)?))
    }

    fn resolve(&self, key: &str) -> Option<DeviceInfo> {
        if key.starts_with(USB_PREFIX) {
            let info = discovery::find_usb(key).ok()?;
            return discovery::has_iio_interface(&info).then(|| {
                usb_info(&discovery::UsbRadio {
                    key: key.to_string(),
                    label: key.to_string(),
                    serial: None,
                })
            });
        }
        let endpoint = Endpoint::parse(key, DEFAULT_PORT)
            .inspect_err(|e| tracing::warn!("ad936x endpoint: {e}"))
            .ok()?;
        if !self.adopted.adopt(endpoint.clone()) {
            tracing::warn!(%endpoint, "too many ad936x endpoints; refusing to adopt another");
            return None;
        }
        Some(net_info(&endpoint))
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

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let (next, writes) = apply::plan(
            settings,
            &self.capabilities,
            &self.front,
            &self.layout,
            &self.settings,
        )?;
        apply::execute(&self.client, &self.layout.phy, &writes)?;
        self.settings = next;
        Ok(())
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
            fan_out(sinks),
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
