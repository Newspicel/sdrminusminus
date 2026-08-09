//! The opened radio: control-plane setters plus RX stream lifecycle.

use nusb::MaybeFuture;
use sdrmm_usb_stream::{NusbBulkIn, RxStream, StreamConfig};
use tracing::{debug, info};

use super::{
    commands::TransceiverMode,
    config::{self, Config},
    control::{
        Control, VendorControlRequest, decode_c_string, decode_part_id_serial,
        validate_gain_response,
    },
    discovery::{self, DeviceDescriptor},
    error::{Error, Result},
    tx::{NusbBulkOut, TxQueue},
    types::{BoardId, DeviceInfo},
};

/// Bulk-IN endpoint the IQ samples arrive on.
const RX_ENDPOINT: u8 = 0x81;
/// Bulk-OUT endpoint the transmit samples leave on.
const TX_ENDPOINT: u8 = 0x02;
/// The USB configuration and interface HackRF firmware exposes in normal mode.
const USB_CONFIGURATION: u8 = 1;
const USB_INTERFACE: u8 = 0;

/// Bytes per USB transfer — libhackrf's size, and the one the 20 Msps field session ran at.
pub(crate) const RX_TRANSFER_SIZE: usize = 262_144;
/// Transfers the consumer may fall behind by. Deliberately shallower than the RTL path's: each
/// buffer here is 256 KiB, so 8 is already 2 MiB of slack and 105 ms at 20 Msps.
const RX_CHANNEL_DEPTH: usize = 8;
/// Firmware older than USB API 1.18 cannot be asked for its buffer size; libhackrf assumes this.
const DEFAULT_FLUSH_SIZE: usize = 32 * 1024;

/// Which way the radio is pointed. It is half duplex: the LPC's transceiver mode selects one
/// data path, so a direction change means stopping the other first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Rx,
    Tx,
}

impl Direction {
    const fn name(self) -> &'static str {
        match self {
            Self::Rx => "receiving",
            Self::Tx => "transmitting",
        }
    }

    const fn mode(self) -> TransceiverMode {
        match self {
            Self::Rx => TransceiverMode::Receive,
            Self::Tx => TransceiverMode::Transmit,
        }
    }
}

/// An opened HackRF.
///
/// Every setter writes through to the radio and only then updates [`HackRf::config`], so the
/// reported configuration is what the hardware holds — including a gain the MAX2837's step grid
/// moved, and, when a batch fails halfway, only the prefix that landed.
pub(crate) struct HackRf {
    control: Control,
    config: Config,
    /// The direction currently selected, if any. `None` is transceiver mode off.
    active: Option<Direction>,
    /// Length of the zero-filled transfer that marks the end of a transmit burst, as the
    /// firmware reports it.
    flush_size: usize,
}

impl HackRf {
    /// Every HackRF currently attached, without claiming any of them.
    pub(crate) fn list() -> Result<Vec<DeviceDescriptor>> {
        discovery::list_devices()
    }

    /// Open the first visible HackRF.
    pub(crate) fn open() -> Result<Self> {
        Self::open_inner(None)
    }

    /// Open the HackRF with this exact 128-bit serial.
    pub(crate) fn open_serial(serial: u128) -> Result<Self> {
        Self::open_inner(Some(serial))
    }

    fn open_inner(serial: Option<u128>) -> Result<Self> {
        let usb_info = discovery::select_device(serial)?;
        let usb_api_version = usb_info.device_version();
        let device = usb_info
            .open()
            .wait()
            .map_err(|e| Error::usb("opening HackRF USB device", e))?;

        match device.set_configuration(USB_CONFIGURATION).wait() {
            Ok(()) => {}
            // macOS and Windows select the configuration themselves and refuse the request.
            Err(e) if e.kind() == nusb::ErrorKind::Unsupported => {}
            Err(e) => return Err(Error::usb("selecting USB configuration 1", e)),
        }
        let interface = device
            .detach_and_claim_interface(USB_INTERFACE)
            .wait()
            .map_err(|e| Error::usb("claiming HackRF USB interface 0", e))?;

        let control = Control::new(device, interface);
        // Whatever the radio was doing before this process, it is not doing it now.
        control.control_out(&VendorControlRequest::transceiver_mode(
            TransceiverMode::Off,
        ))?;

        // Read and logged rather than stored: the backend labels devices from the USB
        // descriptor, so this is the one place the firmware's own account of itself is useful.
        let info = read_device_info(&control, usb_api_version)?;
        info!(
            board = info.board_name(),
            firmware = %info.firmware_version,
            serial = ?info.serial,
            "opened hackrf device"
        );

        let flush_size = read_flush_size(&control, usb_api_version)?;
        let mut opened = Self {
            control,
            config: Config::default(),
            active: None,
            flush_size,
        };
        // The radio powers up with no usable tuning, so the defaults are written through and the
        // reported configuration is true from the first read.
        let defaults = Config::default();
        opened.set_sample_rate_hz(defaults.sample_rate_hz)?;
        opened.set_frequency_hz(defaults.frequency_hz)?;
        opened.set_lna_gain_db(defaults.lna_gain_db)?;
        opened.set_vga_gain_db(defaults.vga_gain_db)?;
        opened.set_amp_enable(defaults.amp_enabled)?;
        opened.set_bias_tee(defaults.bias_tee_enabled)?;
        // Deliberately last and deliberately zero: the transmit driver comes up silent, so a
        // device opened for receive cannot be made to radiate by a later mode change alone.
        opened.set_tx_vga_gain_db(defaults.tx_vga_gain_db)?;
        Ok(opened)
    }

    /// What the radio currently holds.
    #[must_use]
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Retune. Safe while streaming, though there is no sample-accurate boundary.
    pub(crate) fn set_frequency_hz(&mut self, frequency_hz: u64) -> Result<()> {
        config::validate_frequency(frequency_hz)?;
        self.control
            .control_out(&VendorControlRequest::set_frequency(frequency_hz))?;
        self.config.frequency_hz = frequency_hz;
        Ok(())
    }

    /// Set the complex sample rate, and the baseband filter to match it.
    ///
    /// The two are one operation on purpose: a filter left at the old width either folds
    /// out-of-band energy into a wider passband or clips a narrower one.
    pub(crate) fn set_sample_rate_hz(&mut self, sample_rate_hz: u32) -> Result<()> {
        config::validate_sample_rate(sample_rate_hz)?;
        self.control
            .control_out(&VendorControlRequest::set_sample_rate(sample_rate_hz))?;
        self.control
            .control_out(&VendorControlRequest::set_baseband_bandwidth(
                sample_rate_hz,
            ))?;
        self.config.sample_rate_hz = sample_rate_hz;
        Ok(())
    }

    /// Set the MAX2837 IF/LNA gain, in 8 dB steps up to 40.
    pub(crate) fn set_lna_gain_db(&mut self, gain_db: u8) -> Result<()> {
        config::validate_lna_gain(gain_db)?;
        let response = self
            .control
            .control_in_exact(&VendorControlRequest::set_lna_gain(gain_db), 1)?;
        validate_gain_response(&response, "set LNA gain")?;
        self.config.lna_gain_db = gain_db;
        Ok(())
    }

    /// Set the MAX2837 baseband VGA gain, in 2 dB steps up to 62.
    pub(crate) fn set_vga_gain_db(&mut self, gain_db: u8) -> Result<()> {
        config::validate_vga_gain(gain_db)?;
        let response = self
            .control
            .control_in_exact(&VendorControlRequest::set_vga_gain(gain_db), 1)?;
        validate_gain_response(&response, "set VGA gain")?;
        self.config.vga_gain_db = gain_db;
        Ok(())
    }

    /// Set the transmit VGA gain, 0–47 dB. Zero is silence.
    pub(crate) fn set_tx_vga_gain_db(&mut self, gain_db: u8) -> Result<()> {
        config::validate_tx_vga_gain(gain_db)?;
        let response = self
            .control
            .control_in_exact(&VendorControlRequest::set_tx_vga_gain(gain_db), 1)?;
        validate_gain_response(&response, "set TX VGA gain")?;
        self.config.tx_vga_gain_db = gain_db;
        Ok(())
    }

    /// Switch the 14 dB front-end RF amplifier.
    pub(crate) fn set_amp_enable(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_amp(enabled))?;
        self.config.amp_enabled = enabled;
        Ok(())
    }

    /// Switch phantom power on the antenna port.
    pub(crate) fn set_bias_tee(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_bias_tee(enabled))?;
        self.config.bias_tee_enabled = enabled;
        Ok(())
    }

    /// Start receiving.
    ///
    /// The transfer queue is filled before the radio is switched into receive mode, so the
    /// first samples the front end produces already have a transfer waiting for them. Safe to
    /// call again after [`HackRf::stop`], which is what an in-place restart does.
    pub(crate) fn start_rx(&mut self) -> Result<RxStream> {
        self.claim(Direction::Rx)?;
        let endpoint = NusbBulkIn::open(self.control.interface(), RX_ENDPOINT)?;
        let mut config = StreamConfig::new(RX_TRANSFER_SIZE, "sdrmm-hackrf-usb");
        config.channel_depth = RX_CHANNEL_DEPTH;
        let stream = sdrmm_usb_stream::start(endpoint, config)?;
        self.select(Direction::Rx)?;
        Ok(stream)
    }

    /// Start transmitting.
    ///
    /// The mirror of [`HackRf::start_rx`], with the order reversed: the radio enters transmit
    /// mode only once the queue exists, because a transmit pipe with nothing in it radiates
    /// nothing, whereas a receive pipe that is not ready loses the first samples the front end
    /// produced.
    ///
    /// Returns [`Error::AlreadyStreaming`] while the other direction is live — the radio is half
    /// duplex, and switching under a running stream would strand its endpoint.
    pub(crate) fn start_tx(&mut self) -> Result<TxQueue<NusbBulkOut>> {
        self.claim(Direction::Tx)?;
        let endpoint = NusbBulkOut::open(self.control.interface(), TX_ENDPOINT)?;
        let queue = TxQueue::start(endpoint, self.flush_size)?;
        self.select(Direction::Tx)?;
        Ok(queue)
    }

    /// Switch the radio off, whichever way it was pointed.
    ///
    /// Call it before dropping an [`RxStream`], so the front end stops filling a queue that is
    /// about to go away — and *after* draining a [`TxQueue`], so the burst it was asked to send
    /// actually leaves.
    pub(crate) fn stop(&mut self) -> Result<()> {
        self.active = None;
        self.control
            .control_out(&VendorControlRequest::transceiver_mode(
                TransceiverMode::Off,
            ))
    }

    /// Refuse a second direction while one is live.
    fn claim(&self, direction: Direction) -> Result<()> {
        match self.active {
            Some(active) => Err(Error::AlreadyStreaming(active.name())),
            None => {
                debug!(?direction, "hackrf direction claimed");
                Ok(())
            }
        }
    }

    fn select(&mut self, direction: Direction) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::transceiver_mode(direction.mode()))?;
        self.active = Some(direction);
        Ok(())
    }
}

impl Drop for HackRf {
    fn drop(&mut self) {
        // Best effort, and not optional for transmit: a radio left in transmit mode keeps
        // radiating until it is physically unplugged.
        if let Err(e) = self.stop() {
            debug!("hackrf shutdown failed: {e}");
        }
    }
}

/// The firmware's own transmit buffer size, which is the length of a burst's end marker.
/// `GetBufferSize` only exists from USB API 1.18; libhackrf falls back to 32 KiB below that.
fn read_flush_size(control: &Control, usb_api_version: u16) -> Result<usize> {
    if usb_api_version < 0x0112 {
        return Ok(DEFAULT_FLUSH_SIZE);
    }
    let bytes = control.control_in_exact(&VendorControlRequest::get_buffer_size(), 4)?;
    let size = bytes
        .first_chunk::<4>()
        .map(|word| u32::from_le_bytes(*word) as usize)
        .ok_or_else(|| Error::protocol("read TX flush size", "response was not four bytes"))?;
    if size == 0 {
        return Err(Error::protocol(
            "read TX flush size",
            "firmware returned a zero buffer size",
        ));
    }
    Ok(size)
}

fn read_device_info(control: &Control, usb_api_version: u16) -> Result<DeviceInfo> {
    let board = control.control_in_exact(&VendorControlRequest::board_id_read(), 1)?;
    let board_id = board
        .first()
        .copied()
        .map(BoardId::from_raw)
        .ok_or_else(|| Error::protocol("read board ID", "empty response"))?;
    let firmware = control.control_in(&VendorControlRequest::version_string_read())?;
    let serial = control.control_in_exact(&VendorControlRequest::part_id_serial_read(), 24)?;
    Ok(DeviceInfo {
        board_id,
        firmware_version: decode_c_string(&firmware),
        usb_api_version,
        serial: decode_part_id_serial(&serial)?.serial_u128(),
    })
}
