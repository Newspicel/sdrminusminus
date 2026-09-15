use nusb::MaybeFuture;
use sdrmm_usb_stream::{NusbBulkIn, RxStream, StreamConfig};
use tracing::{debug, info};

use super::{
    commands::ReceiverMode,
    config::{self, Config},
    control::{Control, VendorControlRequest, decode_c_string, decode_sample_rates, decode_serial},
    discovery::{self, DeviceDescriptor, Select},
    error::{Error, Result},
};

const RX_ENDPOINT: u8 = 0x81;
const USB_CONFIGURATION: u8 = 1;
const USB_INTERFACE: u8 = 0;
/// Alternate setting 1 is the one whose bulk endpoint is wide enough for the sample stream; the
/// default setting exists so the radio can enumerate without reserving that bandwidth.
const USB_ALT_SETTING: u8 = 1;

pub(crate) const RX_TRANSFER_SIZE: usize = 262_144;
const RX_CHANNEL_DEPTH: usize = 8;
const MAX_SAMPLE_RATES: u16 = 32;

pub(crate) struct Airspy {
    control: Control,
    config: Config,
    sample_rates: Vec<u32>,
    version: String,
    serial: Option<u64>,
}

impl Airspy {
    pub(crate) fn list() -> Result<Vec<DeviceDescriptor>> {
        discovery::list_devices()
    }

    pub(crate) fn open_serial(serial: u64) -> Result<Self> {
        Self::open_inner(&Select::Serial(serial))
    }

    pub(crate) fn open_at(bus: String, address: u8) -> Result<Self> {
        Self::open_inner(&Select::Location { bus, address })
    }

    fn open_inner(select: &Select) -> Result<Self> {
        let usb_info = discovery::select_device(select)?;
        let device = usb_info
            .open()
            .wait()
            .map_err(|e| Error::usb("opening Airspy USB device", e))?;

        match device.set_configuration(USB_CONFIGURATION).wait() {
            Ok(()) => {}
            Err(e) if e.kind() == nusb::ErrorKind::Unsupported => {}
            Err(e) => return Err(Error::usb("selecting USB configuration 1", e)),
        }
        let interface = device
            .detach_and_claim_interface(USB_INTERFACE)
            .wait()
            .map_err(|e| Error::usb("claiming Airspy USB interface 0", e))?;
        interface
            .set_alt_setting(USB_ALT_SETTING)
            .wait()
            .map_err(|e| Error::usb("selecting the Airspy streaming interface", e))?;

        let control = Control::new(device, interface);
        control.control_out(&VendorControlRequest::receiver_mode(ReceiverMode::Off))?;

        let version =
            decode_c_string(&control.control_in(&VendorControlRequest::version_string_read())?);
        let serial =
            decode_serial(&control.control_in(&VendorControlRequest::part_id_serial_read())?);
        let sample_rates = read_sample_rates(&control)?;
        info!(
            firmware = %version,
            serial = ?serial.map(|serial| format!("{serial:016x}")),
            rates = ?sample_rates,
            "opened airspy device"
        );

        let mut opened = Self {
            control,
            config: Config::default(),
            sample_rates,
            version,
            serial,
        };
        // Packing halves the bytes each sample costs on the bus. It is left off because the
        // unpacked codes are what this build reads, and a firmware that silently kept packing on
        // would hand back sixteen bits of three different samples.
        opened.control.control_in_accepted(
            &VendorControlRequest::set_packing(false),
            "disable sample packing",
        )?;

        let defaults = Config::default();
        let rate = opened
            .sample_rates
            .first()
            .copied()
            .ok_or_else(|| Error::protocol("read sample rates", "the radio published none"))?;
        opened.set_sample_rate_hz(rate)?;
        opened.set_frequency_hz(defaults.frequency_hz)?;
        opened.set_lna_gain(defaults.lna_gain)?;
        opened.set_mixer_gain(defaults.mixer_gain)?;
        opened.set_vga_gain(defaults.vga_gain)?;
        opened.set_lna_agc(defaults.lna_agc)?;
        opened.set_mixer_agc(defaults.mixer_agc)?;
        opened.set_bias_tee(defaults.bias_tee)?;
        Ok(opened)
    }

    #[must_use]
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    #[must_use]
    pub(crate) fn sample_rates(&self) -> &[u32] {
        &self.sample_rates
    }

    #[must_use]
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub(crate) fn serial(&self) -> Option<u64> {
        self.serial
    }

    pub(crate) fn set_frequency_hz(&mut self, frequency_hz: u32) -> Result<()> {
        config::validate_frequency(frequency_hz)?;
        self.control
            .control_out(&VendorControlRequest::set_frequency(frequency_hz))?;
        self.config.frequency_hz = frequency_hz;
        Ok(())
    }

    pub(crate) fn set_sample_rate_hz(&mut self, sample_rate_hz: u32) -> Result<()> {
        let index = self
            .sample_rates
            .iter()
            .position(|rate| *rate == sample_rate_hz)
            .ok_or_else(|| {
                Error::invalid_config("sample rate", "this radio does not publish that rate")
            })?;
        let index = u16::try_from(index)
            .map_err(|_| Error::protocol("set sample rate", "rate index beyond the request"))?;
        self.control.control_in_accepted(
            &VendorControlRequest::set_sample_rate_index(index),
            "set sample rate",
        )?;
        self.config.sample_rate_hz = sample_rate_hz;
        Ok(())
    }

    pub(crate) fn set_lna_gain(&mut self, gain: u8) -> Result<()> {
        config::validate_gain("LNA", gain, config::MAX_LNA_GAIN)?;
        self.control
            .control_in_accepted(&VendorControlRequest::set_lna_gain(gain), "set LNA gain")?;
        self.config.lna_gain = gain;
        Ok(())
    }

    pub(crate) fn set_mixer_gain(&mut self, gain: u8) -> Result<()> {
        config::validate_gain("MIX", gain, config::MAX_MIXER_GAIN)?;
        self.control.control_in_accepted(
            &VendorControlRequest::set_mixer_gain(gain),
            "set mixer gain",
        )?;
        self.config.mixer_gain = gain;
        Ok(())
    }

    pub(crate) fn set_vga_gain(&mut self, gain: u8) -> Result<()> {
        config::validate_gain("VGA", gain, config::MAX_VGA_GAIN)?;
        self.control
            .control_in_accepted(&VendorControlRequest::set_vga_gain(gain), "set VGA gain")?;
        self.config.vga_gain = gain;
        Ok(())
    }

    pub(crate) fn set_lna_agc(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_in_accepted(&VendorControlRequest::set_lna_agc(enabled), "set LNA AGC")?;
        self.config.lna_agc = enabled;
        Ok(())
    }

    pub(crate) fn set_mixer_agc(&mut self, enabled: bool) -> Result<()> {
        self.control.control_in_accepted(
            &VendorControlRequest::set_mixer_agc(enabled),
            "set mixer AGC",
        )?;
        self.config.mixer_agc = enabled;
        Ok(())
    }

    pub(crate) fn set_bias_tee(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_bias_tee(enabled))?;
        self.config.bias_tee = enabled;
        Ok(())
    }

    pub(crate) fn start_rx(&mut self) -> Result<RxStream> {
        let endpoint = NusbBulkIn::open(self.control.interface(), RX_ENDPOINT)?;
        let mut config = StreamConfig::new(RX_TRANSFER_SIZE, "sdrmm-airspy-usb");
        config.channel_depth = RX_CHANNEL_DEPTH;
        config.on_thread_start = Some(|| {
            sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
        });
        let stream = sdrmm_usb_stream::start(endpoint, config)?;
        self.set_mode(ReceiverMode::Rx)?;
        Ok(stream)
    }

    pub(crate) fn set_mode_off(&self) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::receiver_mode(ReceiverMode::Off))
    }

    fn set_mode(&mut self, mode: ReceiverMode) -> Result<()> {
        debug!(?mode, "airspy receiver mode");
        self.control
            .control_out(&VendorControlRequest::receiver_mode(mode))
    }
}

fn read_sample_rates(control: &Control) -> Result<Vec<u32>> {
    let counted = control.control_in_exact(&VendorControlRequest::sample_rate_count(), 4)?;
    let count = u32::from_le_bytes(
        counted
            .as_slice()
            .try_into()
            .map_err(|_| Error::protocol("read sample rate count", "short reply"))?,
    );
    let count = u16::try_from(count.min(u32::from(MAX_SAMPLE_RATES)))
        .map_err(|_| Error::protocol("read sample rate count", "implausible count"))?;
    if count == 0 {
        return Err(Error::protocol(
            "read sample rates",
            "the radio published none",
        ));
    }
    let rates =
        decode_sample_rates(&control.control_in(&VendorControlRequest::sample_rates(count))?);
    if rates.is_empty() {
        return Err(Error::protocol(
            "read sample rates",
            "the radio answered with no usable rate",
        ));
    }
    Ok(rates)
}
