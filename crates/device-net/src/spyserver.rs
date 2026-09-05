use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use sdrmm_device::{
    Capture, CaptureConfig, CaptureRadio, DeviceDriver, DeviceError, RxSink, SdrDevice, lock,
    single_rx_sink,
};
use sdrmm_wire::{Capabilities, DeviceInfo as WireDeviceInfo, DeviceSettings};

use crate::{
    adopted::Adopted,
    endpoint::{CONNECT_TIMEOUT, Endpoint},
    socket::{BlockPool, Connection, Read},
    spyserver::{
        caps::Remote,
        proto::{
            ClientSync, DeviceInfo, HEADER_LEN, IqFormat, MSG_CLIENT_SYNC, MSG_DEVICE_INFO,
            MessageHeader, Setting, hello, setting,
        },
        stream::{Coding, SpyConverter, SpyStream},
    },
};

mod caps;
mod proto;
mod stream;

pub(crate) const DRIVER_ID: &str = "spyserver";

const CLIENT_NAME: &str = "sdr--";

#[derive(Debug, Default)]
pub struct SpyServerDriver {
    adopted: Adopted,
}

impl SpyServerDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn device_info(endpoint: &Endpoint) -> WireDeviceInfo {
    WireDeviceInfo {
        driver: DRIVER_ID.to_string(),
        key: endpoint.to_string(),
        label: format!("SpyServer {endpoint}"),
        serial: None,
        profile: None,
    }
}

impl DeviceDriver for SpyServerDriver {
    fn id(&self) -> &'static str {
        DRIVER_ID
    }

    fn probe(&self) -> Vec<WireDeviceInfo> {
        self.adopted.list().iter().map(device_info).collect()
    }

    fn open(&self, info: &WireDeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
        let endpoint = Endpoint::parse(&info.key, proto::DEFAULT_PORT)?;
        Ok(Box::new(SpyServerDevice::open(endpoint)?))
    }

    fn resolve(&self, key: &str) -> Option<WireDeviceInfo> {
        let endpoint = Endpoint::parse(key, proto::DEFAULT_PORT)
            .inspect_err(|e| tracing::warn!("spyserver endpoint: {e}"))
            .ok()?;
        if !self.adopted.adopt(endpoint.clone()) {
            tracing::warn!(%endpoint, "too many SpyServer endpoints; refusing to adopt another");
            return None;
        }
        Some(device_info(&endpoint))
    }
}

fn read_exact(
    connection: &Connection,
    buf: &mut [u8],
    deadline: Instant,
    what: &str,
) -> Result<(), DeviceError> {
    let mut got = 0;
    while got < buf.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(DeviceError::Io(format!("timed out waiting for {what}")));
        }
        match connection.read(&mut buf[got..], left) {
            Read::Got(n) => got += n,
            Read::Idle => {}
            Read::Ended => {
                return Err(DeviceError::Io(format!(
                    "{what}: {}",
                    connection.failure().reason
                )));
            }
        }
    }
    Ok(())
}

fn handshake(connection: &Connection) -> Result<(DeviceInfo, ClientSync), DeviceError> {
    connection.send(&hello(CLIENT_NAME))?;
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut info = None;
    let mut sync = None;
    let mut body = Vec::new();
    while info.is_none() || sync.is_none() {
        let mut bytes = [0u8; HEADER_LEN];
        read_exact(connection, &mut bytes, deadline, "the SpyServer handshake")?;
        let header = MessageHeader::parse(&bytes)?;
        body.clear();
        body.resize(header.body_size as usize, 0);
        read_exact(connection, &mut body, deadline, "a SpyServer message")?;
        match header.kind {
            MSG_DEVICE_INFO => info = Some(DeviceInfo::parse(&body)?),
            MSG_CLIENT_SYNC => sync = Some(ClientSync::parse(&body)?),
            _ => {}
        }
    }
    match (info, sync) {
        (Some(info), Some(sync)) => Ok((info, sync)),
        _ => Err(DeviceError::Io(
            "the SpyServer handshake ended without a device info and client sync".to_string(),
        )),
    }
}

fn connect(endpoint: &Endpoint) -> Result<(Connection, DeviceInfo, ClientSync), DeviceError> {
    let connection = Connection::new(endpoint.connect()?);
    let (info, sync) = handshake(&connection)?;
    Ok((connection, info, sync))
}

#[derive(Debug)]
struct SpyRadio {
    endpoint: Endpoint,
    info: DeviceInfo,
    connection: Mutex<Option<Arc<Connection>>>,
    remote: Mutex<Remote>,
    coding: Coding,
    pool: BlockPool,
}

impl SpyRadio {
    fn send(&self, batch: &[(Setting, u32)]) -> Result<(), DeviceError> {
        let Some(connection) = lock(&self.connection).clone() else {
            return Ok(());
        };
        for (target, value) in batch {
            connection.send(&setting(*target, *value))?;
        }
        Ok(())
    }
}

impl CaptureRadio for SpyRadio {
    type Stream = SpyStream;

    fn arm(&self) -> Result<SpyStream, DeviceError> {
        let (connection, _, sync) = connect(&self.endpoint)?;
        let connection = Arc::new(connection);
        let remote = lock(&self.remote);
        for (target, value) in remote.replay(self.info) {
            connection.send(&setting(target, value))?;
        }
        *lock(&self.connection) = Some(connection.clone());
        drop(remote);
        tracing::debug!(
            endpoint = %self.endpoint,
            can_control = sync.can_control,
            "SpyServer stream armed"
        );
        Ok(SpyStream::new(
            connection,
            self.pool.clone(),
            self.coding.clone(),
        ))
    }

    fn disarm(&self) {
        if let Some(connection) = lock(&self.connection).take() {
            let _ = connection.send(&setting(Setting::StreamingEnabled, 0));
            connection.close();
        }
    }
}

pub struct SpyServerDevice {
    radio: Arc<SpyRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    capture: Capture<SpyRadio>,
}

impl SpyServerDevice {
    fn open(endpoint: Endpoint) -> Result<Self, DeviceError> {
        let (connection, info, sync) = connect(&endpoint)?;
        connection.close();
        let formats = match IqFormat::forced(info.forced_iq_format)? {
            Some(forced) => vec![forced],
            None => vec![IqFormat::Uint8, IqFormat::Int16, IqFormat::Float32],
        };
        let format = formats.first().copied().unwrap_or_default();
        tracing::info!(
            %endpoint,
            model = info.model(),
            serial = format_args!("{:08x}", info.serial),
            can_control = sync.can_control,
            max_sample_rate = info.max_sample_rate,
            "opened SpyServer device"
        );
        let capabilities = caps::capabilities(info, sync, &formats);
        let remote = Remote::new(
            info,
            sync,
            if formats.len() > 1 {
                IqFormat::default()
            } else {
                format
            },
        );
        Ok(Self {
            settings: remote.wire(info, &capabilities),
            capabilities,
            radio: Arc::new(SpyRadio {
                endpoint,
                info,
                connection: Mutex::new(None),
                remote: Mutex::new(remote),
                coding: Coding::default(),
                pool: BlockPool::default(),
            }),
            capture: Capture::new(),
        })
    }
}

impl SdrDevice for SpyServerDevice {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn settings(&self) -> &DeviceSettings {
        &self.settings
    }

    fn apply(&mut self, settings: &DeviceSettings) -> Result<(), DeviceError> {
        let mut remote = lock(&self.radio.remote);
        let (next, batch) = caps::validate(settings, &self.capabilities, self.radio.info, *remote)?;
        *remote = next;
        self.settings = next.wire(self.radio.info, &self.capabilities);
        self.radio.send(&batch)?;
        Ok(())
    }

    fn rx_start(&mut self, sinks: Vec<RxSink>) -> Result<(), DeviceError> {
        self.capture.start(
            self.radio.clone(),
            SpyConverter::new(self.radio.coding.clone()),
            single_rx_sink(sinks)?,
            CaptureConfig::new("sdrmm-spyserver-rx", DRIVER_ID)
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

    #[test]
    fn the_driver_probes_nothing_until_it_is_told_about_an_endpoint() {
        let driver = SpyServerDriver::new();
        assert_eq!(driver.id(), DRIVER_ID);
        assert!(driver.probe().is_empty());

        let info = driver.resolve("spy.local").expect("addressable");
        assert_eq!(
            info.id(),
            "spyserver:spy.local:5555",
            "SpyServer's default port, not rtl_tcp's"
        );
        assert_eq!(info.label, "SpyServer spy.local:5555");
        assert!(info.serial.is_none());
        assert_eq!(driver.probe(), vec![info]);
    }

    #[test]
    fn a_key_that_is_not_an_endpoint_is_not_adopted() {
        let driver = SpyServerDriver::new();
        assert!(driver.resolve("spy.local:port").is_none());
        assert!(driver.probe().is_empty());
    }
}
