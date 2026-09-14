use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use nusb::{
    Interface, MaybeFuture,
    transfer::{Bulk, ControlOut, ControlType, Direction as EpDirection, In, Out, Recipient},
};
use sdrmm_device::{DeviceError, StopHandle, StreamFailure, lock, net::Read};

use crate::iio::link::{Stopper, Transport};

/// The interface string every libiio USB gadget names itself with, and the only way to find the
/// right interface on a radio that also presents storage, serial and a network gadget.
pub(crate) const INTERFACE_NAME: &str = "IIO";

const CTRL_TIMEOUT: Duration = Duration::from_secs(1);

const CMD_RESET_PIPES: u8 = 0;
const CMD_OPEN_PIPE: u8 = 1;
const CMD_CLOSE_PIPE: u8 = 2;

/// Endpoint couples the control conversation and one buffer each need. A radio that offers fewer
/// cannot stream while it is being tuned, which is not a mode this driver has.
pub(crate) const MIN_COUPLES: usize = 2;

/// The claimed IIO interface and its endpoint couples, shared by every link on one radio.
pub(crate) struct UsbBus {
    interface: Interface,
    number: u16,
    couples: Vec<Couple>,
    taken: Mutex<Vec<bool>>,
}

#[derive(Clone, Copy, Debug)]
struct Couple {
    address_in: u8,
    address_out: u8,
}

impl std::fmt::Debug for UsbBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbBus")
            .field("interface", &self.number)
            .field("couples", &self.couples.len())
            .finish()
    }
}

impl UsbBus {
    pub(crate) fn open(info: &nusb::DeviceInfo) -> Result<Arc<Self>, DeviceError> {
        let number = iio_interface(info)?;
        let device = info.open().wait().map_err(|e| open_error(&e))?;
        let interface = device
            .detach_and_claim_interface(number)
            .wait()
            .map_err(|e| open_error(&e))?;
        let couples = couples(&interface)?;
        let bus = Self {
            taken: Mutex::new(vec![false; couples.len()]),
            couples,
            number: u16::from(number),
            interface,
        };
        bus.control(CMD_RESET_PIPES, 0)?;
        Ok(Arc::new(bus))
    }

    pub(crate) fn couples(&self) -> usize {
        self.couples.len()
    }

    /// Reserves one endpoint couple and opens its pipe. The couple is released when the transport
    /// built on it is dropped, so a stopped stream gives its pipe back to the next one.
    pub(crate) fn claim(self: &Arc<Self>) -> Result<UsbTransport, DeviceError> {
        let pipe = {
            let mut taken = lock(&self.taken);
            let free = taken.iter().position(|held| !*held).ok_or_else(|| {
                DeviceError::InUse(format!(
                    "this radio offers {} usb pipes and all of them are in use",
                    self.couples.len()
                ))
            })?;
            taken[free] = true;
            free
        };
        match self.open_pipe(pipe) {
            Ok(transport) => Ok(transport),
            Err(e) => {
                lock(&self.taken)[pipe] = false;
                Err(e)
            }
        }
    }

    fn open_pipe(self: &Arc<Self>, pipe: usize) -> Result<UsbTransport, DeviceError> {
        let couple = self.couples[pipe];
        self.control(CMD_OPEN_PIPE, pipe as u16)?;
        let endpoint_in = self
            .interface
            .endpoint::<Bulk, In>(couple.address_in)
            .map_err(|e| DeviceError::Io(format!("usb pipe {pipe} in: {e}")))?;
        let endpoint_out = self
            .interface
            .endpoint::<Bulk, Out>(couple.address_out)
            .map_err(|e| DeviceError::Io(format!("usb pipe {pipe} out: {e}")))?;
        Ok(UsbTransport {
            bus: self.clone(),
            pipe,
            packet: endpoint_in.max_packet_size().max(1),
            endpoint_in: Mutex::new(endpoint_in),
            endpoint_out: Mutex::new(endpoint_out),
            stopper: Stopper::flag(),
            failure: Mutex::new(None),
        })
    }

    fn release(&self, pipe: usize) {
        if let Err(e) = self.control(CMD_CLOSE_PIPE, pipe as u16) {
            tracing::debug!("closing usb pipe {pipe}: {e}");
        }
        lock(&self.taken)[pipe] = false;
    }

    fn control(&self, request: u8, value: u16) -> Result<(), DeviceError> {
        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Interface,
                    request,
                    value,
                    index: self.number,
                    data: &[],
                },
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(|e| DeviceError::Io(format!("usb control request {request}: {e}")))
    }
}

impl Drop for UsbBus {
    fn drop(&mut self) {
        if let Err(e) = self.control(CMD_RESET_PIPES, 0) {
            tracing::debug!("resetting usb pipes: {e}");
        }
    }
}

/// One endpoint couple, spoken to as an ordered request and answer stream.
pub(crate) struct UsbTransport {
    bus: Arc<UsbBus>,
    pipe: usize,
    packet: usize,
    endpoint_in: Mutex<nusb::Endpoint<Bulk, In>>,
    endpoint_out: Mutex<nusb::Endpoint<Bulk, Out>>,
    stopper: Stopper,
    failure: Mutex<Option<StreamFailure>>,
}

impl std::fmt::Debug for UsbTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbTransport")
            .field("pipe", &self.pipe)
            .finish_non_exhaustive()
    }
}

impl UsbTransport {
    fn record(&self, reason: String, gone: bool) {
        let mut failure = lock(&self.failure);
        if failure.is_none() {
            *failure = Some(StreamFailure { reason, gone });
        }
    }
}

impl Transport for UsbTransport {
    fn send(&self, bytes: &[u8]) -> Result<(), DeviceError> {
        let mut endpoint = lock(&self.endpoint_out);
        let mut buffer = endpoint.allocate(bytes.len());
        buffer.extend_from_slice(bytes);
        let completion = endpoint.transfer_blocking(buffer, CTRL_TIMEOUT);
        match completion.status {
            Ok(()) if completion.actual_len == bytes.len() => Ok(()),
            Ok(()) => Err(DeviceError::Io(format!(
                "usb pipe {} took {} of {} bytes",
                self.pipe,
                completion.actual_len,
                bytes.len()
            ))),
            Err(e) => {
                let gone = is_gone(e);
                let reason = format!("usb pipe {} send: {e}", self.pipe);
                self.record(reason.clone(), gone);
                Err(if gone {
                    DeviceError::Disconnected(reason)
                } else {
                    DeviceError::Io(reason)
                })
            }
        }
    }

    fn read(&self, buf: &mut [u8], timeout: Duration) -> Read {
        if self.stopper.is_stopped() {
            self.record(format!("usb pipe {} was stopped", self.pipe), false);
            return Read::Ended;
        }
        // A bulk IN transfer is refused unless its length is a whole number of packets, and it
        // ends early on the first short one, so asking for more than is coming costs nothing.
        let want = (buf.len() / self.packet) * self.packet;
        if want == 0 {
            return Read::Idle;
        }
        let mut endpoint = lock(&self.endpoint_in);
        let buffer = endpoint.allocate(want);
        let completion = endpoint.transfer_blocking(buffer, timeout);
        match completion.status {
            Ok(()) => {
                let got = completion.actual_len.min(want);
                buf[..got].copy_from_slice(&completion.buffer[..got]);
                if got == 0 { Read::Idle } else { Read::Got(got) }
            }
            Err(nusb::transfer::TransferError::Cancelled) => Read::Idle,
            Err(e) => {
                let gone = is_gone(e);
                self.record(format!("usb pipe {} read: {e}", self.pipe), gone);
                Read::Ended
            }
        }
    }

    fn fail(&self, reason: String) {
        self.record(reason, false);
    }

    fn failure(&self) -> StreamFailure {
        lock(&self.failure).clone().unwrap_or(StreamFailure {
            reason: "the usb pipe went quiet".to_string(),
            gone: false,
        })
    }

    fn close(&self) {
        self.stopper.stop();
    }

    fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }
}

impl Drop for UsbTransport {
    fn drop(&mut self) {
        lock(&self.endpoint_in).cancel_all();
        lock(&self.endpoint_out).cancel_all();
        self.bus.release(self.pipe);
    }
}

const fn is_gone(error: nusb::transfer::TransferError) -> bool {
    matches!(error, nusb::transfer::TransferError::Disconnected)
}

fn open_error(error: &nusb::Error) -> DeviceError {
    let text = error.to_string();
    match error.kind() {
        nusb::ErrorKind::NotFound | nusb::ErrorKind::Disconnected => {
            DeviceError::Disconnected(text)
        }
        nusb::ErrorKind::Busy | nusb::ErrorKind::PermissionDenied => DeviceError::InUse(text),
        _ => DeviceError::Io(text),
    }
}

/// The interface number of this device's IIO gadget, as the operating system already describes it.
pub(crate) fn iio_interface(info: &nusb::DeviceInfo) -> Result<u8, DeviceError> {
    info.interfaces()
        .find(|interface| interface.interface_string() == Some(INTERFACE_NAME))
        .map(|interface| interface.interface_number())
        .ok_or_else(|| {
            DeviceError::NotFound(format!(
                "usb device {:04x}:{:04x} exposes no {INTERFACE_NAME} interface; the radio is \
                 running firmware that does not serve iiod over usb",
                info.vendor_id(),
                info.product_id()
            ))
        })
}

fn couples(interface: &Interface) -> Result<Vec<Couple>, DeviceError> {
    let descriptor = interface
        .descriptor()
        .ok_or_else(|| DeviceError::Io("the IIO interface has no active descriptor".to_string()))?;
    let addresses: Vec<u8> = descriptor
        .endpoints()
        .map(|endpoint| endpoint.address())
        .collect();
    let couples = pair_up(&addresses);
    if couples.len() < MIN_COUPLES {
        return Err(DeviceError::Unsupported(format!(
            "the IIO interface offers {} usable endpoint couples; {MIN_COUPLES} are needed to \
             tune a radio while it streams",
            couples.len()
        )));
    }
    Ok(couples)
}

/// Endpoints are declared in and/out pairs, which is how a pipe is addressed. Anything that does
/// not pair up is not part of a pipe and is left alone.
fn pair_up(addresses: &[u8]) -> Vec<Couple> {
    addresses
        .as_chunks::<2>()
        .0
        .iter()
        .filter(|pair| {
            direction(pair[0]) == EpDirection::In && direction(pair[1]) == EpDirection::Out
        })
        .map(|pair| Couple {
            address_in: pair[0],
            address_out: pair[1],
        })
        .collect()
}

const fn direction(address: u8) -> EpDirection {
    if address & 0x80 == 0 {
        EpDirection::Out
    } else {
        EpDirection::In
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_pair_into_pipes_in_the_order_they_are_declared() {
        let couples = pair_up(&[0x81, 0x01, 0x82, 0x02, 0x83, 0x03]);
        assert_eq!(couples.len(), 3);
        assert_eq!(couples[0].address_in, 0x81);
        assert_eq!(couples[0].address_out, 0x01);
        assert_eq!(couples[2].address_in, 0x83);
    }

    #[test]
    fn an_interface_whose_endpoints_do_not_interleave_yields_no_pipes() {
        assert!(pair_up(&[0x01, 0x81]).is_empty());
        assert!(pair_up(&[0x81]).is_empty());
        assert!(pair_up(&[]).is_empty());
    }

    #[test]
    fn a_trailing_endpoint_without_a_partner_is_left_out() {
        let couples = pair_up(&[0x81, 0x01, 0x82]);
        assert_eq!(couples.len(), 1);
    }

    #[test]
    fn an_address_reads_as_the_direction_its_top_bit_says() {
        assert_eq!(direction(0x81), EpDirection::In);
        assert_eq!(direction(0x01), EpDirection::Out);
    }
}
