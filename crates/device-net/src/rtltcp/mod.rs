//! The rtl_tcp client backend: an RTL-SDR on someone else's machine, driven over the protocol
//! osmocom's `rtl_tcp` speaks (PLAN §6).
//!
//! Layers, in dependency order:
//!
//! - [`proto`] — the wire format: the greeting, the command frames, the order they go in.
//! - [`caps`] — the pure translation to the wire capability model, and what `apply` will accept.
//! - [`stream`] — the byte stream and the RTL2832U's coding.
//! - this module — `DeviceDriver`/`SdrDevice` over `sdrmm-device`'s shared capture machinery.
//!
//! Two things make this backend different from one that owns its radio, and both are the
//! protocol's doing.
//!
//! **The connection is only held while capturing.** An rtl_tcp server streams from the moment the
//! socket opens and buffers for a client that is not draining it, until it gives up and closes
//! (osmocom's `llbuf_num` cap). A connection kept open between an operator opening the device and
//! starting a channel would be dropped underneath them, so the device opens, reads the greeting,
//! and disconnects; the connection lives exactly as long as the capture does.
//!
//! **Nothing is acknowledged.** No command has a reply and no setting can be read back, so this
//! backend's [`caps::Remote`] is not a cache of the radio's state — it *is* the state, the only
//! account anything has of it. That is also what makes a reconnect work: a re-dialled server hands
//! back a dongle at its power-on defaults, and replaying [`caps::Remote`] into it is what puts the
//! operator's tuning back before the first sample is pushed.

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

/// Driver for RTL-SDR dongles reached over the rtl_tcp protocol.
///
/// It probes nothing on its own: rtl_tcp has no discovery, so the endpoints it reports are the
/// ones it has been told about ([`DeviceDriver::resolve`]).
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
        // Never the remote dongle's: what identifies this radio is the endpoint it answers on. A
        // serial would also merge it with the same dongle seen locally, which is a different
        // radio as far as everything above here is concerned — one of them is on a different
        // antenna, in a different room, and may be a different dongle entirely.
        serial: None,
        // The tuner, and with it the frequency range and the gain table, is only known once the
        // server has greeted us — which is what an absent profile says.
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
            // Refusing here rather than opening anyway: a device this driver will not probe is one
            // the engine faults seconds later as "disappeared", which is a worse way to say no.
            tracing::warn!(%endpoint, "too many rtl_tcp endpoints; refusing to adopt another");
            return None;
        }
        Some(device_info(&endpoint))
    }
}

/// Dial the endpoint and read the greeting.
fn connect(endpoint: &Endpoint) -> Result<(Connection, Greeting), DeviceError> {
    let connection = Connection::new(endpoint.connect()?);
    let mut bytes = [0u8; GREETING_LEN];
    let mut got = 0;
    // The greeting is twelve bytes and arrives at once in practice, but a TCP read is entitled to
    // split it — and a server that says nothing at all must time out here rather than leave the
    // control thread parked.
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

/// The radio as the shared capture supervisor sees it: an endpoint, the settings it is to be put
/// into, and whatever connection is currently carrying them.
#[derive(Debug)]
struct RtlRadio {
    endpoint: Endpoint,
    /// Present exactly while a capture is running. The control thread writes commands through it;
    /// its absence is what makes `apply` a recording rather than a send.
    connection: Mutex<Option<Arc<Connection>>>,
    remote: Mutex<Remote>,
    pool: BlockPool,
}

impl RtlRadio {
    /// Send a batch on the live connection. A device that is not capturing has none, and the batch
    /// is already recorded for the next `arm` to replay.
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
        // Before the first sample: a re-dialled server hands back a dongle at its power-on
        // defaults, and anything pushed downstream before the replay lands would be samples from a
        // frequency nobody asked for.
        for (command, param) in lock(&self.remote).replay() {
            connection.send(&frame(command, param))?;
        }
        tracing::debug!(
            endpoint = %self.endpoint,
            tuner = greeting.tuner.name(),
            "rtl_tcp stream armed"
        );
        *lock(&self.connection) = Some(connection.clone());
        Ok(RtlTcpStream::new(connection, self.pool.clone()))
    }

    /// Closing the connection is the only way to stop an rtl_tcp server producing — there is no
    /// command for it — and it is what unblocks a capture thread parked in a read.
    fn disarm(&self) {
        if let Some(connection) = lock(&self.connection).take() {
            connection.close();
        }
    }
}

/// An RTL-SDR reached over rtl_tcp.
pub struct RtlTcpDevice {
    radio: Arc<RtlRadio>,
    capabilities: Capabilities,
    settings: DeviceSettings,
    /// The tuner's gain steps in tenths of a dB, empty when the server's step count did not
    /// confirm the table this backend holds for its tuner.
    gain_table: &'static [i32],
    capture: Capture<RtlRadio>,
}

impl RtlTcpDevice {
    /// Dial `endpoint`, read what the server says about its dongle, and hang up.
    ///
    /// # Errors
    /// [`DeviceError::NotFound`] when the host does not resolve, [`DeviceError::Io`] when nothing
    /// answers or what answers is not an rtl_tcp server.
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
        let (next, batch) = {
            let remote = *lock(&self.radio.remote);
            caps::validate(settings, &self.capabilities, remote, self.gain_table)?
        };
        // Recorded before the send, and kept even when the send fails: a write that fails means
        // the connection is dying, and the reconnect that follows replays this state into the
        // fresh one. The error is still returned, because until that happens the radio is not
        // where the caller was told to expect it.
        *lock(&self.radio.remote) = next;
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

    /// The canonical key is the one everything above binds by, so a second spelling of one
    /// endpoint must not become a second device.
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

    // Opening and streaming are exercised against a fake server in `tests/rtltcp.rs`: they are
    // the parts that touch a socket, and everything that does not is a pure function in `proto`
    // or `caps`.
}
