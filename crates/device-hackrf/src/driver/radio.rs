use nusb::MaybeFuture;
use sdrmm_usb_stream::{NusbBulkIn, NusbBulkOut, RxStream, StreamConfig};
use tracing::{debug, info};

use super::{
    commands::TransceiverMode,
    config::{self, Config, FilterWidth},
    control::{
        Control, VendorControlRequest, decode_c_string, decode_part_id_serial,
        validate_gain_response,
    },
    discovery::{self, DeviceDescriptor, Select},
    error::{Error, Result},
    sweep::{SweepPlan, TRANSFER_BYTES as SWEEP_TRANSFER_SIZE},
    tx::{BurstQueue, TX_TRANSFER_SIZE},
    types::{BoardId, DeviceInfo},
};

const RX_ENDPOINT: u8 = 0x81;
const TX_ENDPOINT: u8 = 0x02;
const USB_CONFIGURATION: u8 = 1;
const USB_INTERFACE: u8 = 0;

pub(crate) const RX_TRANSFER_SIZE: usize = 262_144;
const RX_CHANNEL_DEPTH: usize = 8;
const DEFAULT_FLUSH_SIZE: usize = 32 * 1024;

const BUFFER_SIZE_USB_API: u16 = 0x0112;
const SWEEP_INIT_USB_API: u16 = 0x0102;
const SWEEP_MODE_USB_API: u16 = 0x0104;

pub(crate) struct HackRf {
    control: Control,
    config: Config,
    flush_size: usize,
    usb_api_version: u16,
}

impl HackRf {
    pub(crate) fn list() -> Result<Vec<DeviceDescriptor>> {
        discovery::list_devices()
    }

    pub(crate) fn open_serial(serial: u128) -> Result<Self> {
        Self::open_inner(&Select::Serial(serial))
    }

    pub(crate) fn open_at(bus: String, address: u8) -> Result<Self> {
        Self::open_inner(&Select::Location { bus, address })
    }

    fn open_inner(select: &Select) -> Result<Self> {
        let usb_info = discovery::select_device(select)?;
        let usb_api_version = usb_info.device_version();
        let device = usb_info
            .open()
            .wait()
            .map_err(|e| Error::usb("opening HackRF USB device", e))?;

        match device.set_configuration(USB_CONFIGURATION).wait() {
            Ok(()) => {}
            Err(e) if e.kind() == nusb::ErrorKind::Unsupported => {}
            Err(e) => return Err(Error::usb("selecting USB configuration 1", e)),
        }
        let interface = device
            .detach_and_claim_interface(USB_INTERFACE)
            .wait()
            .map_err(|e| Error::usb("claiming HackRF USB interface 0", e))?;

        let control = Control::new(device, interface);
        control.control_out(&VendorControlRequest::transceiver_mode(
            TransceiverMode::Off,
        ))?;
        debug!("hackrf transceiver mode off on open");

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
            flush_size,
            usb_api_version,
        };
        let defaults = Config::default();
        opened.set_sample_rate_hz(defaults.sample_rate_hz)?;
        opened.set_frequency_hz(defaults.frequency_hz)?;
        opened.set_lna_gain_db(defaults.lna_gain_db)?;
        opened.set_vga_gain_db(defaults.vga_gain_db)?;
        opened.set_amp_enable(defaults.amp_enabled)?;
        opened.set_bias_tee(defaults.bias_tee_enabled)?;
        opened.set_tx_vga_gain_db(defaults.tx_vga_gain_db)?;
        Ok(opened)
    }

    #[must_use]
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) fn set_frequency_hz(&mut self, frequency_hz: u64) -> Result<()> {
        config::validate_frequency(frequency_hz)?;
        self.control
            .control_out(&VendorControlRequest::set_frequency(frequency_hz))?;
        self.config.frequency_hz = frequency_hz;
        Ok(())
    }

    pub(crate) fn set_sample_rate_hz(&mut self, sample_rate_hz: u32) -> Result<()> {
        config::validate_sample_rate(sample_rate_hz)?;
        self.control
            .control_out(&VendorControlRequest::set_sample_rate(sample_rate_hz))?;
        self.config.sample_rate_hz = sample_rate_hz;
        self.write_filter(self.config.filter)
    }

    pub(crate) fn set_filter_width_hz(&mut self, width_hz: u32) -> Result<()> {
        config::validate_filter_width(width_hz)?;
        self.write_filter(FilterWidth::Hz(width_hz))
    }

    pub(crate) fn set_filter_to_match_rate(&mut self) -> Result<()> {
        self.write_filter(FilterWidth::MatchRate)
    }

    fn write_filter(&mut self, filter: FilterWidth) -> Result<()> {
        let width_hz = filter.resolve(self.config.sample_rate_hz);
        self.control
            .control_out(&VendorControlRequest::set_baseband_bandwidth(width_hz))?;
        self.config.filter = filter;
        Ok(())
    }

    pub(crate) fn set_lna_gain_db(&mut self, gain_db: u8) -> Result<()> {
        config::validate_lna_gain(gain_db)?;
        let response = self
            .control
            .control_in_exact(&VendorControlRequest::set_lna_gain(gain_db), 1)?;
        validate_gain_response(&response, "set LNA gain")?;
        self.config.lna_gain_db = gain_db;
        Ok(())
    }

    pub(crate) fn set_vga_gain_db(&mut self, gain_db: u8) -> Result<()> {
        config::validate_vga_gain(gain_db)?;
        let response = self
            .control
            .control_in_exact(&VendorControlRequest::set_vga_gain(gain_db), 1)?;
        validate_gain_response(&response, "set VGA gain")?;
        self.config.vga_gain_db = gain_db;
        Ok(())
    }

    pub(crate) fn set_tx_vga_gain_db(&mut self, gain_db: u8) -> Result<()> {
        config::validate_tx_vga_gain(gain_db)?;
        let response = self
            .control
            .control_in_exact(&VendorControlRequest::set_tx_vga_gain(gain_db), 1)?;
        validate_gain_response(&response, "set TX VGA gain")?;
        self.config.tx_vga_gain_db = gain_db;
        Ok(())
    }

    pub(crate) fn set_amp_enable(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_amp(enabled))?;
        self.config.amp_enabled = enabled;
        Ok(())
    }

    pub(crate) fn set_bias_tee(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_bias_tee(enabled))?;
        self.config.bias_tee_enabled = enabled;
        Ok(())
    }

    pub(crate) fn start_rx(&mut self) -> Result<RxStream> {
        let endpoint = NusbBulkIn::open(self.control.interface(), RX_ENDPOINT)?;
        let mut config = StreamConfig::new(RX_TRANSFER_SIZE, "sdrmm-hackrf-usb");
        config.channel_depth = RX_CHANNEL_DEPTH;
        config.on_thread_start = Some(|| {
            sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
        });
        let stream = sdrmm_usb_stream::start(endpoint, config)?;
        self.select(TransceiverMode::Receive)?;
        Ok(stream)
    }

    pub(crate) fn start_rx_sweep(&mut self, plan: &SweepPlan) -> Result<RxStream> {
        if self.usb_api_version < SWEEP_INIT_USB_API {
            return Err(Error::invalid_config(
                "sweep",
                "the firmware is older than USB API 1.02 and has no sweep request",
            ));
        }
        if self.usb_api_version < SWEEP_MODE_USB_API {
            return Err(Error::invalid_config(
                "sweep",
                "the firmware is older than USB API 1.04 and has no sweep transceiver mode",
            ));
        }
        let (bytes_per_tuning, payload) = plan.encode()?;
        self.set_mode_off()?;
        self.control
            .control_out(&VendorControlRequest::init_sweep(bytes_per_tuning, payload))?;
        let endpoint = NusbBulkIn::open(self.control.interface(), RX_ENDPOINT)?;
        let mut config = StreamConfig::new(SWEEP_TRANSFER_SIZE, "sdrmm-hackrf-sweep");
        config.channel_depth = RX_CHANNEL_DEPTH;
        config.on_thread_start = Some(|| {
            sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
        });
        let stream = sdrmm_usb_stream::start(endpoint, config)?;
        self.select(TransceiverMode::RxSweep)?;
        Ok(stream)
    }

    pub(crate) fn start_tx(&mut self) -> Result<BurstQueue<NusbBulkOut>> {
        let endpoint = NusbBulkOut::open(self.control.interface(), TX_ENDPOINT)?;
        let queue = BurstQueue::start(endpoint, self.flush_size)?;
        self.select(TransceiverMode::Transmit)?;
        Ok(queue)
    }

    pub(crate) fn set_mode_off(&self) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::transceiver_mode(
                TransceiverMode::Off,
            ))
    }

    fn select(&mut self, mode: TransceiverMode) -> Result<()> {
        debug!(?mode, "hackrf transceiver mode");
        self.control
            .control_out(&VendorControlRequest::transceiver_mode(mode))
    }
}

impl Drop for HackRf {
    fn drop(&mut self) {
        if let Err(e) = self.set_mode_off() {
            debug!("hackrf shutdown failed: {e}");
        }
    }
}

fn read_flush_size(control: &Control, usb_api_version: u16) -> Result<usize> {
    if usb_api_version < BUFFER_SIZE_USB_API {
        return Ok(DEFAULT_FLUSH_SIZE);
    }
    let bytes = control.control_in_exact(&VendorControlRequest::get_buffer_size(), 4)?;
    let size = bytes
        .first_chunk::<4>()
        .map(|word| u32::from_le_bytes(*word) as usize)
        .ok_or_else(|| Error::protocol("read TX flush size", "response was not four bytes"))?;
    if size == 0 || size > TX_TRANSFER_SIZE {
        // Every burst end submits a marker of this many zero bytes, so an implausible answer
        // would be an allocation the firmware gets to choose.
        return Err(Error::protocol(
            "read TX flush size",
            "firmware returned an unusable buffer size",
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
