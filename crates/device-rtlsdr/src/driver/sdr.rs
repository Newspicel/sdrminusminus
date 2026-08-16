use nusb::MaybeFuture;
use sdrmm_usb_stream::{NusbBulkIn, RxStream, StreamConfig};
use tracing::{debug, info, trace};

use super::{
    error::{Error, Result},
    regs::{self, Rtl2832u},
    tuner::{self, KNOWN_TUNERS, R82XX_CHECK_VAL, R82xx, TunerType},
};

pub(crate) const DEF_RTL_XTAL_FREQ: u32 = 28_800_000;

pub(crate) const DIRECT_SAMPLING_MAX_HZ: u32 = DEF_RTL_XTAL_FREQ / 2;

pub(crate) const RTL_USB_VID: u16 = 0x0bda;
pub(crate) const RTL_USB_PIDS: &[u16] = &[0x2832, 0x2838];

const EEPROM_BIAS_T_OFFSET: u8 = 7;
const BULK_ENDPOINT: u8 = 0x81;

pub(crate) const TRANSFER_BUF_SIZE: usize = 16_384;

pub(crate) const MAX_PPM: i32 = 488;

const DEFAULT_FIR: [i16; 16] = [
    -54, -36, -41, -40, -32, -14, 14, 53, 101, 156, 215, 273, 327, 372, 404, 421,
];

#[derive(Debug, Clone)]
pub(crate) struct DeviceDescriptor {
    pub(crate) index: usize,
    pub(crate) bus: String,
    pub(crate) address: u8,
    pub(crate) manufacturer: Option<String>,
    pub(crate) product: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) board_variant: BoardVariant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoardVariant {
    Generic,
    RtlSdrBlogV4,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DirectSampling {
    #[default]
    Off,
    IBranch,
    QBranch,
}

impl DirectSampling {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::IBranch => "i",
            Self::QBranch => "q",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        [Self::Off, Self::IBranch, Self::QBranch]
            .into_iter()
            .find(|mode| mode.as_str() == text)
    }

    pub(crate) const fn all() -> [Self; 3] {
        [Self::Off, Self::IBranch, Self::QBranch]
    }
}

struct EnumeratedDevice {
    usb: nusb::DeviceInfo,
    descriptor: DeviceDescriptor,
}

impl EnumeratedDevice {
    fn from_usb(index: usize, usb: nusb::DeviceInfo) -> Self {
        let descriptor = DeviceDescriptor {
            index,
            bus: usb.bus_id().to_string(),
            address: usb.device_address(),
            manufacturer: usb.manufacturer_string().map(str::to_owned),
            product: usb.product_string().map(str::to_owned),
            serial: usb.serial_number().map(str::to_owned),
            board_variant: classify_board_variant(usb.manufacturer_string(), usb.product_string()),
        };
        Self { usb, descriptor }
    }
}

fn is_known_rtl_device(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == RTL_USB_VID && RTL_USB_PIDS.contains(&product_id)
}

fn classify_board_variant(manufacturer: Option<&str>, product: Option<&str>) -> BoardVariant {
    match (manufacturer, product) {
        (Some(manufacturer), Some(product))
            if manufacturer.eq_ignore_ascii_case("RTLSDRBlog")
                && product.eq_ignore_ascii_case("Blog V4") =>
        {
            BoardVariant::RtlSdrBlogV4
        }
        _ => BoardVariant::Generic,
    }
}

pub(crate) struct DeviceDescriptors {
    devices: Vec<EnumeratedDevice>,
}

impl DeviceDescriptors {
    pub(crate) fn new() -> Result<Self> {
        let devices = nusb::list_devices()
            .wait()
            .map_err(Error::OpenFailed)?
            .filter(|device| is_known_rtl_device(device.vendor_id(), device.product_id()))
            .enumerate()
            .map(|(index, device)| EnumeratedDevice::from_usb(index, device))
            .collect();
        Ok(Self { devices })
    }

    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = &DeviceDescriptor> {
        self.devices.iter().map(|device| &device.descriptor)
    }

    pub(crate) fn open(&self, index: usize) -> Result<RtlSdr> {
        let enumerated = self.devices.get(index).ok_or(Error::DeviceNotFound)?;
        RtlSdr::open_enumerated(enumerated)
    }
}

pub(crate) struct RtlSdr {
    dev: Rtl2832u,
    _usb_device: nusb::Device,
    tuner: R82xx,
    center_freq: u32,
    sample_rate: u32,
    rtl_xtal_freq: u32,
    tuner_xtal_freq: u32,
    ppm: i32,
    force_bias_t: bool,
    board_variant: BoardVariant,
    direct_sampling: DirectSampling,
}

impl RtlSdr {
    pub(crate) fn open(index: usize) -> Result<Self> {
        info!(index, "opening RTL-SDR device");
        DeviceDescriptors::new()?.open(index)
    }

    fn open_enumerated(enumerated: &EnumeratedDevice) -> Result<Self> {
        let info = &enumerated.descriptor;
        info!(
            bus = %info.bus,
            address = info.address,
            manufacturer = ?info.manufacturer,
            product = ?info.product,
            variant = ?info.board_variant,
            "found RTL-SDR"
        );

        let usb_device = enumerated.usb.open().wait().map_err(Error::OpenFailed)?;
        #[cfg(target_os = "linux")]
        {
            let _ = usb_device.detach_kernel_driver(0);
        }
        let iface = usb_device
            .claim_interface(0)
            .wait()
            .map_err(Error::ClaimFailed)?;

        let mut sdr = Self {
            dev: Rtl2832u::new(iface),
            _usb_device: usb_device,
            tuner: R82xx::new(
                TunerType::R820T,
                tuner::R820T_I2C_ADDR,
                DEF_RTL_XTAL_FREQ,
                false,
            ),
            center_freq: 0,
            sample_rate: 0,
            rtl_xtal_freq: DEF_RTL_XTAL_FREQ,
            tuner_xtal_freq: DEF_RTL_XTAL_FREQ,
            ppm: 0,
            force_bias_t: false,
            board_variant: info.board_variant,
            direct_sampling: DirectSampling::Off,
        };
        sdr.init()?;
        Ok(sdr)
    }

    fn init(&mut self) -> Result<()> {
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_SYSCTL, 0x09, 1)?;
        self.init_baseband()?;

        self.dev.set_i2c_repeater(true)?;
        let (tuner_type, i2c_addr) = self.search_tuner()?;
        let is_blog_v4 = self.board_variant == BoardVariant::RtlSdrBlogV4;
        let tuner_xtal = if tuner_type == TunerType::R828D && !is_blog_v4 {
            tuner::XTAL_FREQ_16
        } else {
            DEF_RTL_XTAL_FREQ
        };
        self.tuner_xtal_freq = tuner_xtal;
        self.tuner = R82xx::new(tuner_type, i2c_addr, tuner_xtal, is_blog_v4);
        self.tuner.init(&self.dev)?;
        self.dev.set_i2c_repeater(false)?;

        self.dev.demod_write_reg(1, 0xb1, 0x1a, 1)?;
        self.dev.demod_write_reg(0, 0x08, 0x4d, 1)?;
        self.set_if_freq(self.tuner.if_freq())?;
        self.dev.demod_write_reg(1, 0x15, 0x01, 1)?;

        match self.dev.read_eeprom_byte(EEPROM_BIAS_T_OFFSET) {
            Ok(flags) => {
                self.force_bias_t = flags & 0x02 == 0;
                if self.force_bias_t {
                    debug!("EEPROM forces bias-T on");
                }
            }
            Err(e) => debug!("failed to read EEPROM: {e}, continuing"),
        }

        info!(
            tuner = ?tuner_type,
            variant = ?self.board_variant,
            xtal_hz = tuner_xtal,
            "RTL-SDR initialized"
        );
        Ok(())
    }

    fn init_baseband(&self) -> Result<()> {
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_SYSCTL, 0x09, 1)?;
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_EPA_MAXPKT, 0x0002, 2)?;
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_EPA_CTL, 0x1002, 2)?;

        self.dev
            .write_reg(regs::BLOCK_SYS, regs::DEMOD_CTL_1, 0x22, 1)?;
        self.dev
            .write_reg(regs::BLOCK_SYS, regs::DEMOD_CTL, 0xe8, 1)?;

        self.dev.demod_write_reg(1, 0x01, 0x14, 1)?;
        self.dev.demod_write_reg(1, 0x01, 0x10, 1)?;

        self.dev.demod_write_reg(1, 0x15, 0x00, 1)?;
        self.dev.demod_write_reg(1, 0x16, 0x00, 2)?;
        for addr in 0x16..=0x1a {
            self.dev.demod_write_reg(1, addr, 0x00, 1)?;
        }

        self.set_fir(&DEFAULT_FIR)?;

        self.dev.demod_write_reg(0, 0x19, 0x05, 1)?;
        self.dev.demod_write_reg(1, 0x93, 0xf0, 1)?;
        self.dev.demod_write_reg(1, 0x94, 0x0f, 1)?;
        self.dev.demod_write_reg(1, 0x11, 0x00, 1)?;
        self.dev.demod_write_reg(1, 0x04, 0x00, 1)?;
        self.dev.demod_write_reg(0, 0x61, 0x60, 1)?;
        self.dev.demod_write_reg(0, 0x06, 0x80, 1)?;
        self.dev.demod_write_reg(1, 0xb1, 0x1b, 1)?;
        self.dev.demod_write_reg(0, 0x0d, 0x83, 1)
    }

    fn set_fir(&self, fir: &[i16; 16]) -> Result<()> {
        let mut buf = [0u8; 20];
        for (byte, &tap) in buf.iter_mut().zip(&fir[..8]) {
            *byte = tap as i8 as u8;
        }
        for (chunk, taps) in buf[8..]
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .zip(fir[8..].as_chunks::<2>().0)
        {
            let (first, second) = (taps[0] as u16, taps[1] as u16);
            chunk[0] = (first >> 4) as u8;
            chunk[1] = (((first & 0x0f) << 4) | ((second >> 8) & 0x0f)) as u8;
            chunk[2] = second as u8;
        }
        for (offset, &byte) in buf.iter().enumerate() {
            self.dev
                .demod_write_reg(1, 0x1c + offset as u16, u16::from(byte), 1)?;
        }
        Ok(())
    }

    fn search_tuner(&self) -> Result<(TunerType, u8)> {
        for &(tuner_type, addr) in KNOWN_TUNERS {
            match self.dev.i2c_read_reg(addr, 0x00) {
                Ok(R82XX_CHECK_VAL) => {
                    info!("found tuner {tuner_type:?} at I2C address 0x{addr:02x}");
                    return Ok((tuner_type, addr));
                }
                Ok(other) => debug!("I2C probe addr=0x{addr:02x}: got 0x{other:02x}"),
                Err(e) => trace!("I2C probe addr=0x{addr:02x}: {e}"),
            }
        }
        Err(Error::TunerNotFound)
    }

    #[must_use]
    pub(crate) fn board_variant(&self) -> BoardVariant {
        self.board_variant
    }

    #[must_use]
    pub(crate) fn tuner_type(&self) -> TunerType {
        self.tuner.tuner_type()
    }

    #[must_use]
    pub(crate) fn gains(&self) -> &[i32] {
        self.tuner.gains()
    }

    #[must_use]
    pub(crate) fn center_freq(&self) -> u32 {
        self.center_freq
    }

    #[must_use]
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub(crate) fn direct_sampling(&self) -> DirectSampling {
        self.direct_sampling
    }

    pub(crate) fn set_center_freq(&mut self, freq: u32) -> Result<()> {
        if self.direct_sampling == DirectSampling::Off {
            self.dev.set_i2c_repeater(true)?;
            self.tuner.set_freq(&self.dev, freq)?;
            self.dev.set_i2c_repeater(false)?;
            self.set_if_freq(self.tuner.if_freq())?;
        } else {
            self.set_if_freq(freq)?;
        }
        self.center_freq = freq;
        Ok(())
    }

    pub(crate) fn set_direct_sampling(&mut self, mode: DirectSampling) -> Result<()> {
        match mode {
            DirectSampling::Off => {
                self.dev.set_i2c_repeater(true)?;
                self.tuner.init(&self.dev)?;
                self.dev.set_i2c_repeater(false)?;
                self.direct_sampling = mode;
                self.dev.demod_write_reg(1, 0xb1, 0x1a, 1)?;
                self.dev.demod_write_reg(0, 0x08, 0x4d, 1)?;
                self.set_if_freq(self.tuner.if_freq())?;
                self.dev.demod_write_reg(1, 0x15, 0x01, 1)?;
                self.dev.demod_write_reg(0, 0x06, 0x80, 1)?;
            }
            DirectSampling::IBranch | DirectSampling::QBranch => {
                self.dev.set_i2c_repeater(true)?;
                self.tuner.standby(&self.dev)?;
                self.dev.set_i2c_repeater(false)?;
                self.dev.demod_write_reg(1, 0xb1, 0x1a, 1)?;
                self.dev.demod_write_reg(1, 0x15, 0x00, 1)?;
                self.dev.demod_write_reg(0, 0x08, 0x4d, 1)?;
                let datapath = if mode == DirectSampling::QBranch {
                    0x90
                } else {
                    0x80
                };
                self.dev.demod_write_reg(0, 0x06, datapath, 1)?;
                self.direct_sampling = mode;
            }
        }
        info!(mode = mode.as_str(), "direct sampling");
        Ok(())
    }

    pub(crate) fn set_sample_rate(&mut self, rate: u32) -> Result<()> {
        if !(225_001..=300_000).contains(&rate) && !(900_001..=3_200_000).contains(&rate) {
            return Err(Error::InvalidSampleRate { rate });
        }
        let (rsamp_ratio, actual_rate) = resample_ratio(self.rtl_xtal_freq, rate);
        debug!(
            requested = rate,
            actual = actual_rate,
            ratio = rsamp_ratio,
            "set_sample_rate"
        );
        self.sample_rate = actual_rate;

        if self.direct_sampling == DirectSampling::Off {
            self.dev.set_i2c_repeater(true)?;
            let if_freq = self.tuner.set_bandwidth(&self.dev, actual_rate)?;
            self.dev.set_i2c_repeater(false)?;
            self.set_if_freq(if_freq)?;
            if self.center_freq != 0 {
                self.set_center_freq(self.center_freq)?;
            }
        }

        self.dev
            .demod_write_reg(1, 0x9f, (rsamp_ratio >> 16) as u16, 2)?;
        self.dev
            .demod_write_reg(1, 0xa1, (rsamp_ratio & 0xffff) as u16, 2)?;
        self.write_sample_freq_correction(self.ppm)?;
        self.dev.demod_write_reg(1, 0x01, 0x14, 1)?;
        self.dev.demod_write_reg(1, 0x01, 0x10, 1)
    }

    #[must_use]
    pub(crate) fn freq_correction(&self) -> i32 {
        self.ppm
    }

    pub(crate) fn set_freq_correction(&mut self, ppm: i32) -> Result<()> {
        if !(-MAX_PPM..=MAX_PPM).contains(&ppm) {
            return Err(Error::InvalidParam(format!(
                "ppm {ppm} outside the demodulator's ±{MAX_PPM} correction range"
            )));
        }
        self.ppm = ppm;
        self.write_sample_freq_correction(ppm)?;
        self.tuner
            .set_xtal_freq(corrected_xtal(self.tuner_xtal_freq, ppm));
        if self.center_freq != 0 {
            self.set_center_freq(self.center_freq)?;
        }
        Ok(())
    }

    fn write_sample_freq_correction(&self, ppm: i32) -> Result<()> {
        let offset = -(i64::from(ppm) << 24) / 1_000_000;
        self.dev
            .demod_write_reg(1, 0x3f, (offset & 0xff) as u16, 1)?;
        self.dev
            .demod_write_reg(1, 0x3e, ((offset >> 8) & 0x3f) as u16, 1)
    }

    fn set_if_freq(&self, freq: u32) -> Result<()> {
        let if_reg = if_freq_reg(freq, corrected_xtal(self.rtl_xtal_freq, self.ppm));
        self.dev
            .demod_write_reg(1, 0x19, ((if_reg >> 16) & 0x3f) as u16, 1)?;
        self.dev
            .demod_write_reg(1, 0x1a, ((if_reg >> 8) & 0xff) as u16, 1)?;
        self.dev.demod_write_reg(1, 0x1b, (if_reg & 0xff) as u16, 1)
    }

    fn require_tuner(&self, what: &str) -> Result<()> {
        if self.direct_sampling == DirectSampling::Off {
            return Ok(());
        }
        Err(Error::InvalidParam(format!(
            "{what}: the tuner is bypassed while direct sampling"
        )))
    }

    pub(crate) fn set_gain_auto(&mut self) -> Result<()> {
        self.require_tuner("tuner AGC")?;
        self.dev.set_i2c_repeater(true)?;
        self.tuner.set_gain_auto(&self.dev)?;
        self.dev.set_i2c_repeater(false)
    }

    pub(crate) fn set_gain_manual(&mut self, gain_tenth_db: i32) -> Result<()> {
        self.require_tuner("tuner gain")?;
        self.dev.set_i2c_repeater(true)?;
        self.tuner.set_gain_manual(&self.dev, gain_tenth_db)?;
        self.dev.set_i2c_repeater(false)
    }

    pub(crate) fn set_bandwidth(&mut self, bw: u32) -> Result<u32> {
        self.require_tuner("IF filter width")?;
        let bw = if bw == 0 { self.sample_rate } else { bw };
        self.dev.set_i2c_repeater(true)?;
        let if_freq = self.tuner.set_bandwidth(&self.dev, bw)?;
        self.dev.set_i2c_repeater(false)?;
        self.set_if_freq(if_freq)?;
        Ok(if_freq)
    }

    pub(crate) fn set_bias_t(&mut self, enable: bool) -> Result<()> {
        let actual = enable || self.force_bias_t;
        self.dev.set_gpio_output(0)?;
        self.dev.set_gpio_bit(0, actual)
    }

    pub(crate) fn start_streaming(&mut self) -> Result<RxStream> {
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_EPA_CTL, 0x1002, 2)?;
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_EPA_CTL, 0x0000, 2)?;

        let endpoint = NusbBulkIn::open(self.dev.interface(), BULK_ENDPOINT)?;
        let mut config = StreamConfig::new(TRANSFER_BUF_SIZE, "sdrmm-rtlsdr-usb");
        config.on_thread_start = Some(|| {
            sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
        });
        let stream = sdrmm_usb_stream::start(endpoint, config)?;
        Ok(stream)
    }
}

impl Drop for RtlSdr {
    fn drop(&mut self) {
        let _ = self.dev.set_i2c_repeater(true);
        let _ = self.tuner.standby(&self.dev);
        let _ = self.dev.set_i2c_repeater(false);
    }
}

fn if_freq_reg(freq: u32, xtal_hz: u32) -> i32 {
    -((i64::from(freq) * (1 << 22)) / i64::from(xtal_hz)) as i32
}

fn corrected_xtal(nominal_hz: u32, ppm: i32) -> u32 {
    let corrected = f64::from(nominal_hz) * (1.0 + f64::from(ppm) / 1e6);
    corrected.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

fn resample_ratio(xtal_freq: u32, rate: u32) -> (u32, u32) {
    let ratio = ((u64::from(xtal_freq) << 22) / u64::from(rate)) as u32 & 0x0fff_fffc;
    let real_ratio = ratio | ((ratio & 0x0800_0000) << 1);
    let actual = ((u64::from(xtal_freq) << 22) / u64::from(real_ratio)) as u32;
    (ratio, actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_realtek_rtl2832u_dongles_are_claimed() {
        assert!(is_known_rtl_device(0x0bda, 0x2838));
        assert!(is_known_rtl_device(0x0bda, 0x2832));
        assert!(!is_known_rtl_device(0x0bda, 0x2839));
        assert!(!is_known_rtl_device(0x1d50, 0x6089));
    }

    #[test]
    fn only_the_blog_v4_strings_select_the_blog_v4_path() {
        assert_eq!(
            classify_board_variant(Some("RTLSDRBlog"), Some("Blog V4")),
            BoardVariant::RtlSdrBlogV4
        );
        assert_eq!(
            classify_board_variant(Some("rtlsdrblog"), Some("blog v4")),
            BoardVariant::RtlSdrBlogV4
        );
        assert_eq!(
            classify_board_variant(Some("Realtek"), Some("RTL2838UHIDIR")),
            BoardVariant::Generic
        );
        assert_eq!(classify_board_variant(None, None), BoardVariant::Generic);
    }

    #[test]
    fn the_resampler_reproduces_the_requested_rate_on_the_stock_crystal() {
        for rate in [
            225_001, 250_000, 300_000, 900_001, 1_024_000, 2_048_000, 2_400_000, 3_200_000,
        ] {
            let (ratio, actual) = resample_ratio(DEF_RTL_XTAL_FREQ, rate);
            assert_eq!(actual, rate, "rate {rate}");
            assert_eq!(ratio & 0x3, 0, "rate {rate}: low ratio bits must be clear");
            assert_eq!(ratio & 0xf000_0000, 0, "rate {rate}: ratio is 28 bits");
        }
    }

    #[test]
    fn the_ppm_limit_is_the_register_width() {
        let offset = |ppm: i32| -(i64::from(ppm) << 24) / 1_000_000;
        assert!(offset(MAX_PPM).abs() <= 0x1fff, "{}", offset(MAX_PPM));
        assert!(offset(MAX_PPM + 1).abs() > 0x1fff);
    }

    #[test]
    fn the_downconverter_register_is_the_negated_fraction_of_the_crystal() {
        assert_eq!(if_freq_reg(0, DEF_RTL_XTAL_FREQ), 0);
        assert_eq!(if_freq_reg(3_570_000, DEF_RTL_XTAL_FREQ), -519_918);
        assert_eq!(if_freq_reg(7_200_000, DEF_RTL_XTAL_FREQ), -(1 << 20));
    }

    #[test]
    fn the_direct_sampling_ceiling_is_the_register_width() {
        let at_limit = if_freq_reg(DIRECT_SAMPLING_MAX_HZ, DEF_RTL_XTAL_FREQ);
        assert_eq!(at_limit, -(1 << 21), "the most negative 22-bit value");
        assert!(if_freq_reg(DIRECT_SAMPLING_MAX_HZ + 100, DEF_RTL_XTAL_FREQ) < -(1 << 21));
    }

    #[test]
    fn a_ppm_correction_moves_the_downconverter_register() {
        let nominal = if_freq_reg(7_100_000, DEF_RTL_XTAL_FREQ);
        let corrected = if_freq_reg(7_100_000, corrected_xtal(DEF_RTL_XTAL_FREQ, 100));
        assert_eq!(corrected - nominal, 103);
    }

    #[test]
    fn direct_sampling_modes_round_trip_their_wire_spelling() {
        for mode in DirectSampling::all() {
            assert_eq!(DirectSampling::parse(mode.as_str()), Some(mode));
        }
        let spellings: Vec<&str> = DirectSampling::all().iter().map(|m| m.as_str()).collect();
        assert_eq!(spellings, ["off", "i", "q"]);
        assert_eq!(DirectSampling::default(), DirectSampling::Off);
        for unknown in ["", "1", "Q", "on", "off "] {
            assert_eq!(DirectSampling::parse(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn a_ppm_correction_scales_the_crystal_both_ways() {
        assert_eq!(corrected_xtal(DEF_RTL_XTAL_FREQ, 0), DEF_RTL_XTAL_FREQ);
        assert_eq!(corrected_xtal(DEF_RTL_XTAL_FREQ, 50), 28_801_440);
        assert_eq!(corrected_xtal(DEF_RTL_XTAL_FREQ, -50), 28_798_560);
        assert_eq!(corrected_xtal(1_000_000, 0), 1_000_000);
    }
}
