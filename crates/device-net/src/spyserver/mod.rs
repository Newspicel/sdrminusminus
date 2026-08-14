//! The SpyServer client backend: an Airspy, Airspy HF+ or RTL-SDR published over Airspy's
//! SpyServer protocol.
//!
//! Layers, in dependency order:
//!
//! - [`proto`] — the wire format: the handshake, the settings, the message header.
//! - [`caps`] — the pure translation to the wire capability model, and what `apply` will accept.
//! - [`stream`] — message framing and the conversion from each quantisation.
//! - this module — `DeviceDriver`/`SdrDevice` over `sdrmm-device`'s shared capture machinery.
//!
//! Unlike rtl_tcp, this protocol describes the radio behind it, so the capability set is read
//! rather than assumed — including the one thing a SpyServer has that no local radio does: it may
//! refuse to be steered at all. A server with `CanControl` clear is somebody else's receiver, and
//! this client's whole frequency range is then the slice of spectrum it is allowed to slide
//! inside, which is what [`caps::capabilities`] reports so the dial cannot promise otherwise.
//!
//! The connection is held only while capturing, for the same reason it is on rtl_tcp: a SpyServer
//! counts connected clients and this backend should occupy a slot exactly while it is using one.

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

/// What a server operator sees this client listed as.
const CLIENT_NAME: &str = "sdr--";

/// Driver for receivers published over the SpyServer protocol.
///
/// Like rtl_tcp's, it probes nothing on its own: the protocol has no discovery, so what it reports
/// is what it has been told about ([`DeviceDriver::resolve`]).
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
        // Not the far side's `DeviceSerial`: what identifies this radio is the endpoint it answers
        // on, and a serial would also merge it with the same receiver seen locally — which is a
        // different radio to everything above here. Servers that report a serial of zero would
        // merge with each other, too.
        serial: None,
        // Everything about this receiver comes from a handshake, which is what an absent profile
        // says: not known until it is opened.
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

/// Fill `buf` completely or give up at `deadline`.
///
/// Only the handshake uses this. The capture thread must never block for a whole message — it has
/// a stop flag to look at — which is why [`SpyStream`] frames incrementally instead.
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

/// Say hello and read back what the server says about itself and about this client.
///
/// Both messages arrive unprompted once the handshake lands, in either order, possibly behind
/// others — so this reads messages until it has the two it needs.
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
        // The loop only exits with both, but saying so in the type is cheaper than a panic.
        _ => Err(DeviceError::Io(
            "the SpyServer handshake ended without a device info and client sync".to_string(),
        )),
    }
}

/// Dial the endpoint and complete the handshake.
fn connect(endpoint: &Endpoint) -> Result<(Connection, DeviceInfo, ClientSync), DeviceError> {
    let connection = Connection::new(endpoint.connect()?);
    let (info, sync) = handshake(&connection)?;
    Ok((connection, info, sync))
}

/// The radio as the shared capture supervisor sees it.
#[derive(Debug)]
struct SpyRadio {
    endpoint: Endpoint,
    info: DeviceInfo,
    /// Present exactly while a capture is running; its absence is what makes `apply` a recording
    /// rather than a send.
    connection: Mutex<Option<Arc<Connection>>>,
    remote: Mutex<Remote>,
    coding: Coding,
    pool: BlockPool,
}

impl SpyRadio {
    /// Send a batch on the live connection. A device that is not capturing has none, and the batch
    /// is already recorded for the next `arm` to replay.
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
        // A fresh connection is a server that has never heard of this client: every setting has to
        // be sent again, and streaming is only enabled at the end of it.
        //
        // The replay and the publication are one step, under the guard `apply` also holds. This
        // runs on the supervisor's restart thread while `apply` runs on the control thread, and a
        // setting recorded into `remote` after this replay read it would find no connection to go
        // out on — leaving the radio a setting behind what `settings()` reports, silently.
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

    /// Ask the server to stop before hanging up, so a shared receiver is not left producing for a
    /// client that has gone. Closing the connection is what actually enforces it, and what unblocks
    /// a capture thread parked in a read.
    fn disarm(&self) {
        if let Some(connection) = lock(&self.connection).take() {
            let _ = connection.send(&setting(Setting::StreamingEnabled, 0));
            connection.close();
        }
    }
}

/// A receiver reached over a SpyServer.
pub struct SpyServerDevice {
    radio: Arc<SpyRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    capture: Capture<SpyRadio>,
}

impl SpyServerDevice {
    /// Dial `endpoint`, complete the handshake, and hang up.
    ///
    /// # Errors
    /// [`DeviceError::NotFound`] when the host does not resolve, [`DeviceError::Io`] when nothing
    /// answers or what answers does not speak this protocol, [`DeviceError::Unsupported`] when the
    /// server forces a sample format this backend cannot decode.
    fn open(endpoint: Endpoint) -> Result<Self, DeviceError> {
        let (connection, info, sync) = connect(&endpoint)?;
        connection.close();
        // A forced format is the server's, not a preference: it is the only one it will send.
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
        // Held across the send, because `arm` replays this same state into a fresh connection
        // under it: that is what makes a batch either reach the live connection or be replayed by
        // the restart that is taking its place, and never neither.
        let mut remote = lock(&self.radio.remote);
        let (next, batch) = caps::validate(settings, &self.capabilities, self.radio.info, *remote)?;
        // Recorded before the send and kept even when it fails: a failed write means the connection
        // is dying, and the reconnect that follows replays this state into the fresh one. The error
        // is still returned, because until that happens the radio is not where the caller expects.
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
            CaptureConfig::new("sdrmm-spyserver-rx", DRIVER_ID),
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

    // Opening and streaming are exercised against a fake server in `tests/spyserver.rs`.
}
