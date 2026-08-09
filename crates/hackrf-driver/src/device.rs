//! The opened radio: control-plane setters plus RX stream lifecycle.

use nusb::MaybeFuture;
use sdrmm_usb_stream::{NusbBulkIn, RxStream, StreamConfig};
use tracing::{debug, info};

use crate::{
    commands::TransceiverMode,
    config::{self, Config},
    control::{
        Control, VendorControlRequest, decode_c_string, decode_part_id_serial,
        validate_gain_response,
    },
    discovery::{self, DeviceDescriptor},
    error::{Error, Result},
    types::{BoardId, DeviceInfo},
};

/// Bulk-IN endpoint the IQ samples arrive on.
const RX_ENDPOINT: u8 = 0x81;
/// The USB configuration and interface HackRF firmware exposes in normal mode.
const USB_CONFIGURATION: u8 = 1;
const USB_INTERFACE: u8 = 0;

/// Bytes per USB transfer — libhackrf's size, and the one the 20 Msps field session ran at.
pub const RX_TRANSFER_SIZE: usize = 262_144;
/// Transfers the consumer may fall behind by. Deliberately shallower than the RTL path's: each
/// buffer here is 256 KiB, so 8 is already 2 MiB of slack and 105 ms at 20 Msps.
const RX_CHANNEL_DEPTH: usize = 8;

/// An opened HackRF.
///
/// Every setter writes through to the radio and only then updates [`Device::config`], so the
/// reported configuration is what the hardware holds — including a gain the MAX2837's step grid
/// moved, and, when a batch fails halfway, only the prefix that landed.
pub struct Device {
    control: Control,
    info: DeviceInfo,
    config: Config,
    streaming: bool,
}

impl Device {
    /// Every HackRF currently attached, without claiming any of them.
    pub fn list() -> Result<Vec<DeviceDescriptor>> {
        discovery::list_devices()
    }

    /// Open the first visible HackRF.
    pub fn open() -> Result<Self> {
        Self::open_inner(None)
    }

    /// Open the HackRF with this exact 128-bit serial.
    pub fn open_serial(serial: u128) -> Result<Self> {
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

        let info = read_device_info(&control, usb_api_version)?;
        info!(
            board = info.board_name(),
            firmware = %info.firmware_version,
            serial = ?info.serial,
            "opened hackrf device"
        );

        let mut opened = Self {
            control,
            info,
            config: Config::default(),
            streaming: false,
        };
        // The radio powers up with no usable tuning, so the defaults are written through and the
        // reported configuration is true from the first read.
        let defaults = Config::default();
        opened.set_sample_rate_hz(defaults.sample_rate_hz())?;
        opened.set_frequency_hz(defaults.frequency_hz())?;
        opened.set_lna_gain_db(defaults.lna_gain_db())?;
        opened.set_vga_gain_db(defaults.vga_gain_db())?;
        opened.set_amp_enable(defaults.amp_enabled())?;
        opened.set_bias_tee(defaults.bias_tee_enabled())?;
        Ok(opened)
    }

    /// What the firmware reported while the device was opened.
    #[must_use]
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// What the radio currently holds.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Retune. Safe while streaming, though there is no sample-accurate boundary.
    pub fn set_frequency_hz(&mut self, frequency_hz: u64) -> Result<()> {
        config::validate_frequency(frequency_hz)?;
        self.control
            .control_out(&VendorControlRequest::set_frequency(frequency_hz))?;
        self.config.set_frequency_hz(frequency_hz);
        Ok(())
    }

    /// Set the complex sample rate, and the baseband filter to match it.
    ///
    /// The two are one operation on purpose: a filter left at the old width either folds
    /// out-of-band energy into a wider passband or clips a narrower one.
    pub fn set_sample_rate_hz(&mut self, sample_rate_hz: u32) -> Result<()> {
        config::validate_sample_rate(sample_rate_hz)?;
        self.control
            .control_out(&VendorControlRequest::set_sample_rate(sample_rate_hz))?;
        self.control
            .control_out(&VendorControlRequest::set_baseband_bandwidth(
                sample_rate_hz,
            ))?;
        self.config.set_sample_rate_hz(sample_rate_hz);
        Ok(())
    }

    /// Set the MAX2837 IF/LNA gain, in 8 dB steps up to 40.
    pub fn set_lna_gain_db(&mut self, gain_db: u8) -> Result<()> {
        config::validate_lna_gain(gain_db)?;
        let response = self
            .control
            .control_in_exact(&VendorControlRequest::set_lna_gain(gain_db), 1)?;
        validate_gain_response(&response, "set LNA gain")?;
        self.config.set_lna_gain_db(gain_db);
        Ok(())
    }

    /// Set the MAX2837 baseband VGA gain, in 2 dB steps up to 62.
    pub fn set_vga_gain_db(&mut self, gain_db: u8) -> Result<()> {
        config::validate_vga_gain(gain_db)?;
        let response = self
            .control
            .control_in_exact(&VendorControlRequest::set_vga_gain(gain_db), 1)?;
        validate_gain_response(&response, "set VGA gain")?;
        self.config.set_vga_gain_db(gain_db);
        Ok(())
    }

    /// Switch the 14 dB front-end RF amplifier.
    pub fn set_amp_enable(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_amp(enabled))?;
        self.config.set_amp_enabled(enabled);
        Ok(())
    }

    /// Switch phantom power on the antenna port.
    pub fn set_bias_tee(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_bias_tee(enabled))?;
        self.config.set_bias_tee_enabled(enabled);
        Ok(())
    }

    /// Start receiving.
    ///
    /// The transfer queue is filled before the radio is switched into receive mode, so the
    /// first samples the front end produces already have a transfer waiting for them. Safe to
    /// call again after [`Device::stop_rx`], which is what an in-place restart does.
    pub fn start_rx(&mut self) -> Result<RxStream> {
        if self.streaming {
            return Err(Error::AlreadyStreaming);
        }
        let endpoint = NusbBulkIn::open(self.control.interface(), RX_ENDPOINT)?;
        let mut config = StreamConfig::new(RX_TRANSFER_SIZE, "sdrmm-hackrf-usb");
        config.channel_depth = RX_CHANNEL_DEPTH;
        let stream = sdrmm_usb_stream::start(endpoint, config)?;
        self.control
            .control_out(&VendorControlRequest::transceiver_mode(
                TransceiverMode::Receive,
            ))?;
        self.streaming = true;
        Ok(stream)
    }

    /// Switch the radio off. Call before dropping the [`RxStream`], so the front end stops
    /// filling a queue that is about to go away.
    pub fn stop_rx(&mut self) -> Result<()> {
        self.streaming = false;
        self.control
            .control_out(&VendorControlRequest::transceiver_mode(
                TransceiverMode::Off,
            ))
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Best effort: a radio left in receive mode keeps its front end and amplifier powered
        // until it is physically unplugged.
        if let Err(e) = self.stop_rx() {
            debug!("hackrf shutdown failed: {e}");
        }
    }
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
