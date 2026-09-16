use std::sync::{Arc, Mutex};

use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, RxSink, SdrDevice, lock,
    net::{Adopted, WebSocket},
    single_rx_sink,
};
use sdrmm_wire::{Capabilities, DeviceInfo as WireDeviceInfo, DeviceSettings};

use crate::{
    caps::Remote,
    proto::{Address, Command, Property},
    stream::{IqConverter, SdrConnectStream},
};

mod caps;
mod proto;
mod session;
mod stream;

const DRIVER_ID: &str = "sdrconnect";

#[derive(Debug, Default)]
pub struct SdrConnectDriver {
    adopted: Adopted<Address>,
}

impl SdrConnectDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn device_info(address: &Address) -> WireDeviceInfo {
    WireDeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: address.to_string(),
        label: format!("SDRconnect {address}"),
        serial: None,
        profile: None,
    }
}

impl DeviceDriver for SdrConnectDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<WireDeviceInfo> {
        self.adopted.list().iter().map(device_info).collect()
    }

    fn open(&self, info: &WireDeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(SdrConnectDevice::open(Address::parse(
            &info.key,
        )?)?))
    }

    fn resolve(&self, key: &str) -> Option<WireDeviceInfo> {
        let address = Address::parse(key)
            .inspect_err(|e| tracing::warn!("sdrconnect endpoint: {e}"))
            .ok()?;
        if !self.adopted.adopt(address.clone()) {
            tracing::warn!(%address, "too many SDRconnect endpoints; refusing to adopt another");
            return None;
        }
        Some(device_info(&address))
    }
}

#[derive(Debug)]
struct SdrConnectRadio {
    address: Address,
    socket: Mutex<Option<Arc<WebSocket>>>,
    remote: Mutex<Remote>,
}

impl SdrConnectRadio {
    fn send(&self, batch: &[Command]) -> Result<(), DeviceError> {
        let socket = lock(&self.socket).clone();
        let Some(socket) = socket else {
            return Ok(());
        };
        session::send(&socket, batch, self.address.tuner)
    }
}

impl CaptureRadio for SdrConnectRadio {
    type Stream = SdrConnectStream;

    fn arm(&self) -> Result<SdrConnectStream, DeviceError> {
        let tuner = self.address.tuner;
        let socket = Arc::new(session::connect(&self.address)?);
        session::send(&socket, &session::focus(tuner), tuner)?;
        let start = lock(&self.remote).start();
        session::send(&socket, &start, tuner)?;
        *lock(&self.socket) = Some(socket.clone());
        tracing::debug!(address = %self.address, "SDRconnect stream armed");
        Ok(SdrConnectStream::new(socket, tuner))
    }

    fn disarm(&self) {
        let socket = lock(&self.socket).take();
        if let Some(socket) = socket {
            let stop = lock(&self.remote).stop();
            let _ = session::send(&socket, &stop, self.address.tuner);
            socket.close();
        }
    }
}

pub struct SdrConnectDevice {
    radio: Arc<SdrConnectRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    capture: Capture<SdrConnectRadio>,
}

impl SdrConnectDevice {
    fn open(address: Address) -> Result<Self, DeviceError> {
        let socket = session::connect(&address)?;
        let snapshot = session::interrogate(&socket, address.tuner)?;
        socket.close();
        let capabilities = caps::capabilities(&snapshot);
        let remote = Remote::new(&snapshot);
        tracing::info!(
            %address,
            api = snapshot.text(Property::ApiVersion).unwrap_or("unknown"),
            receiver = snapshot.text(Property::ActiveDevice).unwrap_or("unnamed"),
            steerable = snapshot.boolean(Property::CanControl).unwrap_or(true),
            "opened an SDRconnect receiver"
        );
        Ok(Self {
            settings: remote.wire(&capabilities),
            capabilities,
            radio: Arc::new(SdrConnectRadio {
                address,
                socket: Mutex::new(None),
                remote: Mutex::new(remote),
            }),
            capture: Capture::new(),
        })
    }
}

impl SdrDevice for SdrConnectDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let mut remote = lock(&self.radio.remote);
        let (next, batch) = caps::validate(settings, &self.capabilities, &remote)?;
        *remote = next;
        self.settings = remote.wire(&self.capabilities);
        drop(remote);
        self.radio.send(&batch)
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.capture.start(
            self.radio.clone(),
            IqConverter::default(),
            single_rx_sink(sinks)?,
            CaptureConfig::new("sdrmm-sdrconnect-rx", DRIVER_ID)
                .with_sample_rate(self.settings.sample_rate),
        )
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Tuner;

    #[test]
    fn the_driver_probes_nothing_until_it_is_told_about_an_endpoint() {
        let driver = SdrConnectDriver::new();
        assert_eq!(driver.id(), DRIVER_ID);
        assert!(driver.probe().is_empty());

        let info = driver.resolve("rsp.local").expect("addressable");
        assert_eq!(
            info.id(),
            "sdrconnect:rsp.local:5454",
            "SDRconnect's own port, not SpyServer's"
        );
        assert_eq!(info.label, "SDRconnect rsp.local:5454");
        assert!(info.serial.is_none());
        assert_eq!(driver.probe(), vec![info]);
    }

    #[test]
    fn the_second_tuner_of_a_dual_tuner_receiver_is_a_device_of_its_own() {
        let driver = SdrConnectDriver::new();
        let primary = driver.resolve("rsp.local:5454").expect("addressable");
        let secondary = driver
            .resolve("rsp.local:5454/secondary")
            .expect("addressable");
        assert_ne!(primary.key, secondary.key);
        assert_eq!(secondary.label, "SDRconnect rsp.local:5454/secondary");
        assert_eq!(driver.probe().len(), 2);
    }

    #[test]
    fn a_key_that_is_not_an_endpoint_is_not_adopted() {
        let driver = SdrConnectDriver::new();
        assert!(driver.resolve("rsp.local:port").is_none());
        assert!(driver.resolve("rsp.local/third").is_none());
        assert!(driver.probe().is_empty());
    }

    #[test]
    fn the_capture_thread_reads_the_tuner_the_key_names() {
        assert_eq!(
            Address::parse("rsp.local").expect("parses").tuner,
            Tuner::Primary
        );
        assert_eq!(
            Address::parse("rsp.local/secondary").expect("parses").tuner,
            Tuner::Secondary
        );
    }
}
