use std::{
    num::NonZeroU8,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use nusb::{
    Interface, MaybeFuture,
    transfer::{
        Buffer, Bulk, Completion, ControlOut, ControlType, Direction as EpDirection,
        EndpointDirection, In, Out, Recipient, TransferError,
    },
};
use sdrmm_device::{DeviceError, StopHandle, StreamFailure, lock, net::Read};
use sdrmm_usb_stream::is_disconnect;

use crate::iio::link::{Stopper, Transport, remaining};

/// The interface string every libiio USB gadget names itself with, and the only way to find the
/// right interface on a radio that also presents storage, serial and a network gadget.
pub(crate) const INTERFACE_NAME: &str = "IIO";

const CTRL_TIMEOUT: Duration = Duration::from_secs(1);

/// How often a parked read looks for a stop, so that a radio that has gone quiet does not hold
/// the thread for the whole of its read timeout.
const STOP_POLL: Duration = Duration::from_millis(50);

const ENGLISH_US: u16 = 0x0409;

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
        let device = info.open().wait().map_err(|e| open_error(&e))?;
        let number = iio_interface(info, &device)?;
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
            inbound: Mutex::new(Pipe::new(endpoint_in)),
            outbound: Mutex::new(Pipe::new(endpoint_out)),
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
            .map_err(|e| {
                let reason = format!("usb control request {request}: {e}");
                if is_disconnect(&e) {
                    DeviceError::Disconnected(reason)
                } else {
                    DeviceError::Io(reason)
                }
            })
    }
}

impl Drop for UsbBus {
    fn drop(&mut self) {
        if let Err(e) = self.control(CMD_RESET_PIPES, 0) {
            tracing::debug!("resetting usb pipes: {e}");
        }
    }
}

/// One endpoint and the buffer its last transfer came back in, kept so the next transfer does
/// not map a fresh one.
struct Pipe<Dir: EndpointDirection> {
    endpoint: nusb::Endpoint<Bulk, Dir>,
    spare: Option<Buffer>,
}

impl<Dir: EndpointDirection> Pipe<Dir> {
    const fn new(endpoint: nusb::Endpoint<Bulk, Dir>) -> Self {
        Self {
            endpoint,
            spare: None,
        }
    }

    fn buffer(&mut self, len: usize) -> Buffer {
        match self.spare.take() {
            Some(mut buffer) if buffer.capacity() >= len => {
                buffer.clear();
                buffer.set_requested_len(len);
                buffer
            }
            _ => self.endpoint.allocate(len),
        }
    }

    fn keep(&mut self, buffer: Buffer) {
        self.spare = Some(buffer);
    }
}

/// One endpoint couple, spoken to as an ordered request and answer stream.
pub(crate) struct UsbTransport {
    bus: Arc<UsbBus>,
    pipe: usize,
    packet: usize,
    inbound: Mutex<Pipe<In>>,
    outbound: Mutex<Pipe<Out>>,
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

    /// Waits for the transfer in flight, looking for a stop while it does, and cancels it once
    /// either the stop or the deadline arrives. What the device sent before that is kept.
    fn complete(&self, pipe: &mut Pipe<In>, timeout: Duration) -> Completion {
        let deadline = Instant::now() + timeout;
        loop {
            let slice = remaining(deadline).min(STOP_POLL);
            if let Some(completion) = pipe.endpoint.wait_next_complete(slice) {
                return completion;
            }
            if self.stopper.is_stopped() || Instant::now() >= deadline {
                pipe.endpoint.cancel_all();
                loop {
                    if let Some(completion) = pipe.endpoint.wait_next_complete(CTRL_TIMEOUT) {
                        return completion;
                    }
                    tracing::warn!(
                        pipe = self.pipe,
                        "a cancelled usb transfer has not come back"
                    );
                }
            }
        }
    }
}

impl Transport for UsbTransport {
    fn send(&self, bytes: &[u8]) -> Result<(), DeviceError> {
        let mut pipe = lock(&self.outbound);
        let mut buffer = pipe.buffer(bytes.len());
        buffer.extend_from_slice(bytes);
        let completion = pipe.endpoint.transfer_blocking(buffer, CTRL_TIMEOUT);
        let outcome = match completion.status {
            Ok(()) if completion.actual_len == bytes.len() => Ok(()),
            Ok(()) => Err(DeviceError::Io(format!(
                "usb pipe {} took {} of {} bytes",
                self.pipe,
                completion.actual_len,
                bytes.len()
            ))),
            Err(e) => {
                let gone = is_disconnect(&e);
                let reason = format!("usb pipe {} send: {e}", self.pipe);
                self.record(reason.clone(), gone);
                Err(if gone {
                    DeviceError::Disconnected(reason)
                } else {
                    DeviceError::Io(reason)
                })
            }
        };
        pipe.keep(completion.buffer);
        outcome
    }

    fn read(&self, buf: &mut [u8], wanted: usize, timeout: Duration) -> Read {
        if self.stopper.is_stopped() {
            self.record(format!("usb pipe {} was stopped", self.pipe), false);
            return Read::Ended;
        }
        let Some(want) = request_len(buf.len(), wanted, self.packet) else {
            return Read::Idle;
        };
        let mut pipe = lock(&self.inbound);
        let buffer = pipe.buffer(want);
        pipe.endpoint.submit(buffer);
        let completion = self.complete(&mut pipe, timeout);
        let got = completion.actual_len.min(want);
        buf[..got].copy_from_slice(&completion.buffer[..got]);
        let status = completion.status;
        pipe.keep(completion.buffer);
        match status {
            Ok(()) | Err(TransferError::Cancelled) if got > 0 => Read::Got(got),
            Ok(()) => Read::Idle,
            Err(TransferError::Cancelled) if self.stopper.is_stopped() => {
                self.record(format!("usb pipe {} was stopped", self.pipe), false);
                Read::Ended
            }
            Err(TransferError::Cancelled) => Read::Idle,
            Err(e) => {
                self.record(
                    format!("usb pipe {} read: {e}", self.pipe),
                    is_disconnect(&e),
                );
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
        lock(&self.inbound).endpoint.cancel_all();
        lock(&self.outbound).endpoint.cancel_all();
        self.bus.release(self.pipe);
    }
}

/// How much to ask the endpoint for. A bulk IN transfer must be whole packets and ends only at
/// a short one, so it asks for just enough packets to cover `wanted`: any more would leave the
/// transfer waiting on a device that has already sent everything it was going to.
fn request_len(room: usize, wanted: usize, packet: usize) -> Option<usize> {
    let whole = room / packet * packet;
    let asked = wanted.max(1).div_ceil(packet) * packet;
    let want = asked.min(whole);
    (want > 0).then_some(want)
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

/// The interface number of this device's IIO gadget: as the operating system already describes
/// it, or, where the operating system does not report interface names, as the device itself
/// does when asked for its string descriptors.
fn iio_interface(info: &nusb::DeviceInfo, device: &nusb::Device) -> Result<u8, DeviceError> {
    if let Some(number) = named_interface(info) {
        return Ok(number);
    }
    described_interface(device).ok_or_else(|| {
        DeviceError::NotFound(format!(
            "usb device {:04x}:{:04x} exposes no {INTERFACE_NAME} interface; the radio is \
             running firmware that does not serve iiod over usb",
            info.vendor_id(),
            info.product_id()
        ))
    })
}

pub(crate) fn named_interface(info: &nusb::DeviceInfo) -> Option<u8> {
    info.interfaces()
        .find(|interface| interface.interface_string() == Some(INTERFACE_NAME))
        .map(|interface| interface.interface_number())
}

fn described_interface(device: &nusb::Device) -> Option<u8> {
    let configuration = device.active_configuration().ok()?;
    configuration.interfaces().find_map(|interface| {
        let index: NonZeroU8 = interface.first_alt_setting().string_index()?;
        let name = device
            .get_string_descriptor(index, ENGLISH_US, CTRL_TIMEOUT)
            .wait()
            .ok()?;
        (name.trim() == INTERFACE_NAME).then(|| interface.interface_number())
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

    #[test]
    fn a_transfer_asks_for_just_the_packets_that_cover_what_is_wanted() {
        assert_eq!(
            request_len(65_536, 60_928, 512),
            Some(60_928),
            "a tail that is whole packets is asked for exactly, or it would never end"
        );
        assert_eq!(request_len(65_536, 60_930, 512), Some(61_440));
        assert_eq!(request_len(65_536, 1, 512), Some(512));
        assert_eq!(request_len(65_536, 100_000, 512), Some(65_536));
        assert_eq!(request_len(60_930, 60_930, 512), Some(60_928));
        assert_eq!(request_len(100, 5, 512), None, "no room for a packet");
    }
}
