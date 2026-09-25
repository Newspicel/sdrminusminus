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

pub(crate) const RX_TRANSFER_SIZE: usize = 65_536;
const RX_CHANNEL_DEPTH: usize = 8;
const MAX_SAMPLE_RATES: u32 = 32;

pub(crate) struct AirspyHf {
    control: Control,
    config: Config,
    sample_rates: Vec<u32>,
    low_if: Vec<bool>,
    version: String,
    serial: Option<u64>,
}

impl AirspyHf {
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
            .map_err(|e| Error::usb("opening Airspy HF+ USB device", e))?;

        match device.set_configuration(USB_CONFIGURATION).wait() {
            Ok(()) => {}
            Err(e) if e.kind() == nusb::ErrorKind::Unsupported => {}
            Err(e) => return Err(Error::usb("selecting USB configuration 1", e)),
        }
        let interface = device
            .detach_and_claim_interface(USB_INTERFACE)
            .wait()
            .map_err(|e| Error::usb("claiming Airspy HF+ USB interface 0", e))?;

       interface
           .set_alt_setting(1)
           .wait()
           .map_err(|e| Error::usb("selecting Airspy HF+ interface alternate setting 1", e))?;

        let control = Control::new(device, interface);
        control.control_out(&VendorControlRequest::receiver_mode(ReceiverMode::Off))?;

        let version =
            decode_c_string(&control.control_in(&VendorControlRequest::version_string_read())?);
        let serial =
            decode_serial(&control.control_in(&VendorControlRequest::part_id_serial_read())?);
        let sample_rates = read_sample_rates(&control)?;
        let low_if = read_architectures(&control, sample_rates.len());
        info!(
            firmware = %version,
            serial = ?serial.map(|serial| format!("{serial:016x}")),
            rates = ?sample_rates,
            "opened airspy hf+ device"
        );

        let mut opened = Self {
            control,
            config: Config::default(),
            sample_rates,
            low_if,
            version,
            serial,
        };
        let defaults = Config::default();
        let rate = opened
            .sample_rates
            .first()
            .copied()
            .ok_or_else(|| Error::protocol("read sample rates", "the radio published none"))?;
        opened.set_sample_rate_hz(rate)?;
        opened.set_frequency_hz(defaults.frequency_hz)?;
        opened.set_attenuation_step(defaults.attenuation_step)?;
        opened.set_lna(defaults.lna)?;
        opened.set_agc(defaults.agc)?;
        opened.set_agc_high_threshold(defaults.agc_high_threshold)?;
// Airspy HF+ and HF+ Discovery have no bias tee
//        opened.set_bias_tee(defaults.bias_tee)?;
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

    /// Whether the rate now selected puts the signal at an intermediate frequency rather than
    /// straight on DC, which decides whether the centre of the band carries the converter's own
    /// leakage.
    #[must_use]
    pub(crate) fn is_low_if(&self) -> bool {
        self.sample_rates
            .iter()
            .position(|rate| *rate == self.config.sample_rate_hz)
            .and_then(|index| self.low_if.get(index).copied())
            .unwrap_or(false)
    }

    pub(crate) fn set_frequency_hz(&mut self, frequency_hz: u32) -> Result<()> {
        config::validate_frequency(frequency_hz)?;
        // The radio tunes in kilohertz, so the frequency it reports back is the one it can
        // actually reach rather than the one that was asked for.
        let khz = ((f64::from(frequency_hz) / 1000.0).round() as u32).max(1);
        self.control
            .control_out(&VendorControlRequest::set_frequency(khz))?;
        self.config.frequency_hz = khz.saturating_mul(1_000);
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
        self.control
            .control_out(&VendorControlRequest::set_sample_rate_index(index))?;
        self.config.sample_rate_hz = sample_rate_hz;
        Ok(())
    }

    pub(crate) fn set_attenuation_step(&mut self, step: u8) -> Result<()> {
        config::validate_attenuation(step)?;
        self.control
            .control_out(&VendorControlRequest::set_attenuation(step))?;
        self.config.attenuation_step = step;
        Ok(())
    }

    pub(crate) fn set_lna(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_lna(enabled))?;
        self.config.lna = enabled;
        Ok(())
    }

    pub(crate) fn set_agc(&mut self, enabled: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_agc(enabled))?;
        self.config.agc = enabled;
        Ok(())
    }

    pub(crate) fn set_agc_high_threshold(&mut self, high: bool) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::set_agc_threshold(high))?;
        self.config.agc_high_threshold = high;
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
        let mut config = StreamConfig::new(RX_TRANSFER_SIZE, "sdrmm-airspyhf-usb");
        config.channel_depth = RX_CHANNEL_DEPTH;
        config.on_thread_start = Some(|| {
            sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
        });
        let stream = sdrmm_usb_stream::start(endpoint, config)?;
        self.set_mode(ReceiverMode::On)?;
        Ok(stream)
    }

    pub(crate) fn set_mode_off(&self) -> Result<()> {
        self.control
            .control_out(&VendorControlRequest::receiver_mode(ReceiverMode::Off))
    }

    fn set_mode(&mut self, mode: ReceiverMode) -> Result<()> {
        debug!(?mode, "airspy hf+ receiver mode");
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
    let count = u16::try_from(count.min(MAX_SAMPLE_RATES))
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

/// A firmware old enough not to answer this simply leaves every rate reported as zero-IF, which
/// is the more cautious of the two: it keeps the DC term treated as an artifact to remove.
fn read_architectures(control: &Control, rates: usize) -> Vec<bool> {
    let Ok(count) = u16::try_from(rates) else {
        return vec![false; rates];
    };
    match control.control_in(&VendorControlRequest::sample_rate_architectures(count)) {
        Ok(answer) if answer.len() == rates => answer.iter().map(|flag| *flag != 0).collect(),
        Ok(_) | Err(_) => vec![false; rates],
    }
}
