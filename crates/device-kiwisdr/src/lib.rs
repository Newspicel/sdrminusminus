use std::sync::{Arc, Mutex};

use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, RxSink, SdrDevice, lock,
    net::{Adopted, WebSocket},
    single_rx_sink,
};
use sdrmm_wire::{Capabilities, DeviceInfo as WireDeviceInfo, DeviceSettings};

use crate::{
    address::Address,
    caps::Tuning,
    proto::Command,
    session::Station,
    stream::{IqConverter, KiwiStream},
};

mod address;
mod caps;
mod proto;
mod session;
mod stream;

const DRIVER_ID: &str = "kiwisdr";

#[derive(Debug, Default)]
pub struct KiwiSdrDriver {
    adopted: Adopted<Address>,
}

impl KiwiSdrDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn device_info(address: &Address) -> WireDeviceInfo {
    WireDeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: address.to_string(),
        label: format!("KiwiSDR {}", address.endpoint),
        serial: None,
        profile: None,
    }
}

impl DeviceDriver for KiwiSdrDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<WireDeviceInfo> {
        self.adopted.list().iter().map(device_info).collect()
    }

    fn open(&self, info: &WireDeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        Ok(Box::new(KiwiSdrDevice::open(Address::parse(&info.key)?)?))
    }

    fn resolve(&self, key: &str) -> Option<WireDeviceInfo> {
        let address = Address::parse(key)
            .inspect_err(|e| tracing::warn!("kiwisdr endpoint: {e}"))
            .ok()?;
        if !self.adopted.adopt(address.clone()) {
            tracing::warn!(endpoint = %address.endpoint, "too many KiwiSDR endpoints; refusing to adopt another");
            return None;
        }
        Some(device_info(&address))
    }
}

#[derive(Debug)]
struct KiwiSdrRadio {
    address: Address,
    station: Station,
    socket: Mutex<Option<Arc<WebSocket>>>,
    spare: Mutex<Option<WebSocket>>,
    tuning: Mutex<Tuning>,
}

impl KiwiSdrRadio {
    fn send(&self, batch: &[Command]) -> Result<(), DeviceError> {
        let socket = lock(&self.socket).clone();
        match socket {
            Some(socket) if !batch.is_empty() => session::send(&socket, batch),
            _ => Ok(()),
        }
    }

    fn login(&self) -> Result<WebSocket, DeviceError> {
        let socket = session::connect(&self.address.endpoint)?;
        let station =
            session::login(&socket, &self.address.password).inspect_err(|_| socket.close())?;
        if !self.station.same_as(&station) {
            socket.close();
            return Err(DeviceError::Disconnected(
                "the KiwiSDR changed its sample rate or band; open it again".to_string(),
            ));
        }
        Ok(socket)
    }

    fn start(&self, socket: WebSocket) -> Result<KiwiStream, DeviceError> {
        let socket = Arc::new(socket);
        let mut held = lock(&self.socket);
        let mut batch = vec![Command::Unsquelch, Command::Ident];
        batch.extend(lock(&self.tuning).replay(&self.station));
        batch.push(Command::Keepalive);
        session::send(&socket, &batch).inspect_err(|_| socket.close())?;
        *held = Some(socket.clone());
        drop(held);
        tracing::debug!(endpoint = %self.address.endpoint, "KiwiSDR stream armed");
        Ok(KiwiStream::new(socket))
    }
}

impl CaptureRadio for KiwiSdrRadio {
    type Stream = KiwiStream;

    fn arm(&self) -> Result<KiwiStream, DeviceError> {
        let spare = lock(&self.spare).take();
        if let Some(stream) = spare.and_then(|socket| self.start(socket).ok()) {
            return Ok(stream);
        }
        self.start(self.login()?)
    }

    fn disarm(&self) {
        if let Some(socket) = lock(&self.socket).take() {
            socket.close();
        }
    }
}

pub struct KiwiSdrDevice {
    radio: Arc<KiwiSdrRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    capture: Capture<KiwiSdrRadio>,
}

impl KiwiSdrDevice {
    fn open(address: Address) -> Result<Self, DeviceError> {
        let socket = session::connect(&address.endpoint)?;
        let station = session::login(&socket, &address.password).inspect_err(|_| socket.close())?;
        tracing::info!(
            endpoint = %address.endpoint,
            sample_rate = station.sample_rate,
            channels = ?station.channels,
            "opened a KiwiSDR"
        );
        let tuning = Tuning::new(&station);
        Ok(Self {
            capabilities: caps::capabilities(&station),
            settings: tuning.wire(&station),
            radio: Arc::new(KiwiSdrRadio {
                address,
                station,
                socket: Mutex::new(None),
                spare: Mutex::new(Some(socket)),
                tuning: Mutex::new(tuning),
            }),
            capture: Capture::new(),
        })
    }
}

impl SdrDevice for KiwiSdrDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let station = &self.radio.station;
        let mut tuning = lock(&self.radio.tuning);
        let (next, batch) = caps::validate(station, &self.capabilities, &tuning, settings)?;
        *tuning = next;
        self.settings = tuning.wire(station);
        drop(tuning);
        self.radio.send(&batch)
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.capture.start(
            self.radio.clone(),
            IqConverter::default(),
            single_rx_sink(sinks)?,
            CaptureConfig::new("sdrmm-kiwisdr-rx", DRIVER_ID)
                .with_sample_rate(Some(self.radio.station.sample_rate)),
        )
    }

    fn rx_stop(&mut self) {
        self.capture.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_without_a_port_gets_the_kiwi_port() {
        let driver = KiwiSdrDriver::new();
        assert!(driver.probe().is_empty());
        let info = driver.resolve("kiwi.local").expect("addressable");
        assert_eq!(info.id(), "kiwisdr:kiwi.local:8073");
        assert_eq!(info.label, "KiwiSDR kiwi.local:8073");
        assert_eq!(driver.probe(), vec![info]);
        assert!(driver.resolve("kiwi.local:port").is_none());
        let private = driver.resolve("pw@kiwi.local").expect("addressable");
        assert_eq!(private.id(), "kiwisdr:pw@kiwi.local:8073");
        assert_eq!(private.label, "KiwiSDR kiwi.local:8073");
    }
}
