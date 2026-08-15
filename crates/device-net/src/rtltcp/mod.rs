use std::sync::{Arc, Mutex};

use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, RxSink, SdrDevice, lock,
    single_rx_sink,
};
use sdrmm_wire::{Capabilities, DeviceInfo, DeviceSettings};

use crate::{
    adopted::Adopted,
    endpoint::Endpoint,
    rtltcp::{
        caps::Remote,
        proto::{Command, GREETING_LEN, Greeting, frame},
        stream::RtlTcpStream,
    },
    socket::{BlockPool, Connection},
};

mod caps;
mod proto;
mod stream;

pub(crate) const DRIVER_ID: &str = "rtltcp";

#[derive(Debug, Default)]
pub struct RtlTcpDriver {
    adopted: Adopted,
}

impl RtlTcpDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn device_info(endpoint: &Endpoint) -> DeviceInfo {
    DeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: endpoint.to_string(),
        label: format!("rtl_tcp {endpoint}"),
        serial: None,
        profile: None,
    }
}

impl DeviceDriver for RtlTcpDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<DeviceInfo> {
        self.adopted.list().iter().map(device_info).collect()
    }

    fn open(&self, info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let endpoint = Endpoint::parse(&info.key, proto::DEFAULT_PORT)?;
        Ok(Box::new(RtlTcpDevice::open(endpoint)?))
    }

    fn resolve(&self, key: &str) -> Option<DeviceInfo> {
        let endpoint = Endpoint::parse(key, proto::DEFAULT_PORT)
            .inspect_err(|e| tracing::warn!("rtl_tcp endpoint: {e}"))
            .ok()?;
        if !self.adopted.adopt(endpoint.clone()) {
            tracing::warn!(%endpoint, "too many rtl_tcp endpoints; refusing to adopt another");
            return None;
        }
        Some(device_info(&endpoint))
    }
}

fn connect(endpoint: &Endpoint) -> Result<(Connection, Greeting), DeviceError> {
    let connection = Connection::new(endpoint.connect()?);
    let mut bytes = [0u8; GREETING_LEN];
    let mut got = 0;
    while got < GREETING_LEN {
        match connection.read(&mut bytes[got..], crate::endpoint::CONNECT_TIMEOUT) {
            crate::socket::Read::Got(n) => got += n,
            crate::socket::Read::Idle => {
                return Err(DeviceError::Io(format!(
                    "{endpoint}: no rtl_tcp greeting within {:?}",
                    crate::endpoint::CONNECT_TIMEOUT
                )));
            }
            crate::socket::Read::Ended => {
                return Err(DeviceError::Io(format!(
                    "{endpoint}: {}",
                    connection.failure().reason
                )));
            }
        }
    }
    let greeting = Greeting::parse(&bytes)?;
    Ok((connection, greeting))
}

#[derive(Debug)]
struct RtlRadio {
    endpoint: Endpoint,
    connection: Mutex<Option<Arc<Connection>>>,
    remote: Mutex<Remote>,
    pool: BlockPool,
}

impl RtlRadio {
    fn send(&self, batch: &[(Command, u32)]) -> Result<(), DeviceError> {
        let Some(connection) = lock(&self.connection).clone() else {
            return Ok(());
        };
        for (command, param) in batch {
            connection.send(&frame(*command, *param))?;
        }
        Ok(())
    }
}

impl CaptureRadio for RtlRadio {
    type Stream = RtlTcpStream;

    fn arm(&self) -> Result<RtlTcpStream, DeviceError> {
        let (connection, greeting) = connect(&self.endpoint)?;
        let connection = Arc::new(connection);
        let remote = lock(&self.remote);
        for (command, param) in remote.replay() {
            connection.send(&frame(command, param))?;
        }
        *lock(&self.connection) = Some(connection.clone());
        drop(remote);
        tracing::debug!(
            endpoint = %self.endpoint,
            tuner = greeting.tuner.name(),
            "rtl_tcp stream armed"
        );
        Ok(RtlTcpStream::new(connection, self.pool.clone()))
    }

    fn disarm(&self) {
        if let Some(connection) = lock(&self.connection).take() {
            connection.close();
        }
    }
}

pub struct RtlTcpDevice {
    radio: Arc<RtlRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    gain_table: &'static [i32],
    capture: Capture<RtlRadio>,
}

impl RtlTcpDevice {
    fn open(endpoint: Endpoint) -> Result<Self, DeviceError> {
        let (connection, greeting) = connect(&endpoint)?;
        connection.close();
        let gain_table = caps::gain_table(greeting.tuner, greeting.gain_steps);
        tracing::info!(
            %endpoint,
            tuner = greeting.tuner.name(),
            gain_steps = greeting.gain_steps,
            table = gain_table.len(),
            "opened rtl_tcp device"
        );
        let remote = Remote::new(gain_table);
        Ok(Self {
            capabilities: caps::capabilities(greeting.tuner, gain_table),
            settings: remote.wire(),
            gain_table,
            radio: Arc::new(RtlRadio {
                endpoint,
                connection: Mutex::new(None),
                remote: Mutex::new(remote),
                pool: BlockPool::default(),
            }),
            capture: Capture::new(),
        })
    }
}

impl SdrDevice for RtlTcpDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let mut remote = lock(&self.radio.remote);
        let (next, batch) = caps::validate(settings, &self.capabilities, *remote, self.gain_table)?;
        *remote = next;
        self.settings = next.wire();
        self.radio.send(&batch)?;
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.capture.start(
            self.radio.clone(),
            stream::converter(),
            single_rx_sink(sinks)?,
            CaptureConfig::new("sdrmm-rtltcp-rx", DRIVER_ID),
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
    fn the_driver_probes_nothing_until_it_is_told_about_an_endpoint() {
        let driver = RtlTcpDriver::new();
        assert_eq!(driver.id(), DRIVER_ID);
        assert!(driver.probe().is_empty());

        let info = driver.resolve("radio.local").expect("addressable");
        assert_eq!(
            info.id(),
            "rtltcp:radio.local:1234",
            "the default port is filled in"
        );
        assert_eq!(info.label, "rtl_tcp radio.local:1234");
        assert!(info.serial.is_none());
        assert_eq!(driver.probe(), vec![info]);
    }

    #[test]
    fn two_spellings_of_one_endpoint_are_one_device() {
        let driver = RtlTcpDriver::new();
        let first = driver.resolve("Radio.local:1234").expect("addressable");
        let second = driver.resolve("radio.local").expect("addressable");
        assert_eq!(first, second);
        assert_eq!(driver.probe().len(), 1);
    }

    #[test]
    fn a_key_that_is_not_an_endpoint_is_not_adopted() {
        let driver = RtlTcpDriver::new();
        assert!(driver.resolve("radio.local:not-a-port").is_none());
        assert!(driver.probe().is_empty());
    }
}
