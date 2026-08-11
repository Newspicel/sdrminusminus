//! Device lifecycle: enumeration, RTL2832U bring-up, tuner detection, and the settings the
//! backend drives.
//!
//! Streaming itself is not here — it is `sdrmm-usb-stream`, shared with the HackRF backend. All
//! this module does is reset the endpoint FIFO and hand the transport a claimed bulk-IN pipe.

use nusb::MaybeFuture;
use sdrmm_usb_stream::{NusbBulkIn, RxStream, StreamConfig};
use tracing::{debug, info, trace};

use super::{
    error::{Error, Result},
    regs::{self, Rtl2832u},
    tuner::{self, KNOWN_TUNERS, R82XX_CHECK_VAL, R82xx, TunerType},
};

/// RTL2832U crystal frequency (Hz) on every dongle this driver supports.
pub(crate) const DEF_RTL_XTAL_FREQ: u32 = 28_800_000;

/// Highest frequency direct sampling reaches: the ADC runs at the crystal rate, so its first
/// Nyquist zone ends here — and the downconverter register that does the tuning holds exactly
/// this much (see [`if_freq_reg`]). Above it the ADC aliases and the register wraps.
pub(crate) const DIRECT_SAMPLING_MAX_HZ: u32 = DEF_RTL_XTAL_FREQ / 2;

/// USB vendor ID for RTL-SDR devices (Realtek).
pub(crate) const RTL_USB_VID: u16 = 0x0bda;
/// Product IDs of the RTL2832U-based dongles this driver claims.
pub(crate) const RTL_USB_PIDS: &[u16] = &[0x2832, 0x2838];

/// Bulk-IN endpoint the IQ samples arrive on.
const BULK_ENDPOINT: u8 = 0x81;

/// Bytes per USB transfer. A multiple of the 512-byte high-speed max packet size, as
/// `Endpoint::submit` requires, and the size librtlsdr has used since forever.
pub(crate) const TRANSFER_BUF_SIZE: usize = 16_384;

/// The demodulator's resampler-correction register pair (page 1, 0x3f/0x3e) is 14 bits signed,
/// and one ppm is `2^24 / 1e6` counts — so this is the largest correction the hardware can
/// actually hold. Real dongles are within ±100 ppm; the backend advertises a tighter range.
pub(crate) const MAX_PPM: i32 = 488;

/// Demodulator FIR defaults: 16 taps, the first 8 signed 8-bit, the last 8 signed 12-bit.
const DEFAULT_FIR: [i16; 16] = [
    -54, -36, -41, -40, -32, -14, 14, 53, 101, 156, 215, 273, 327, 372, 404, 421,
];

/// What USB enumeration says about one attached dongle.
#[derive(Debug, Clone)]
pub(crate) struct DeviceDescriptor {
    /// Position within the filtered RTL-SDR enumeration — what [`DeviceDescriptors::open`]
    /// selects on.
    pub(crate) index: usize,
    /// USB bus identifier.
    pub(crate) bus: String,
    /// USB device address on the bus.
    pub(crate) address: u8,
    /// Manufacturer string, if the device has one.
    pub(crate) manufacturer: Option<String>,
    /// Product string, if the device has one.
    pub(crate) product: Option<String>,
    /// Serial number string, if the device has one.
    pub(crate) serial: Option<String>,
    /// Board identity, derived from the strings above.
    pub(crate) board_variant: BoardVariant,
}

/// Board identity. Only the Blog V4 differs in a way the driver has to know about: it upconverts
/// HF through the tuner instead of bypassing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoardVariant {
    /// Any dongle without a known quirk.
    Generic,
    /// RTL-SDR Blog V4 (R828D with a 28.8 MHz crystal and an HF upconverter).
    RtlSdrBlogV4,
}

/// Which ADC input feeds the demodulator when the tuner is bypassed.
///
/// The RTL2832U samples both ADC pins at the crystal rate. Normally the tuner drives them with a
/// 3.57 MHz IF and the demodulator's downconverter mixes that to zero; in direct sampling the
/// tuner is put in standby and the *downconverter itself* is tuned instead, so whatever is wired
/// to the ADC pin is received across the first Nyquist zone — DC to half the crystal. That is how
/// a dongle whose tuner starts at 24 MHz hears HF.
///
/// Which branch is usable is a property of the board, not the chip: the RTL-SDR Blog V3 wires its
/// antenna port to the Q branch, and other dongles need the well-known solder mod. The driver
/// cannot tell them apart, so the operator selects the branch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DirectSampling {
    /// Tuner in circuit — the normal receive path.
    #[default]
    Off,
    /// ADC I branch.
    IBranch,
    /// ADC Q branch, which is what the Blog V3 and the usual mod connect.
    QBranch,
}

impl DirectSampling {
    /// The wire spelling, as offered in the `direct_sampling` enum setting.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::IBranch => "i",
            Self::QBranch => "q",
        }
    }

    /// Parse a wire spelling. `None` for anything else — a client bug, not a value to coerce.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        [Self::Off, Self::IBranch, Self::QBranch]
            .into_iter()
            .find(|mode| mode.as_str() == text)
    }

    /// Every mode, in the order the setting offers them.
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

/// Every RTL-SDR currently attached.
pub(crate) struct DeviceDescriptors {
    devices: Vec<EnumeratedDevice>,
}

impl DeviceDescriptors {
    /// Enumerate all attached RTL-SDR dongles.
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

    /// Iterate the enumerated descriptors, in index order.
    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = &DeviceDescriptor> {
        self.devices.iter().map(|device| &device.descriptor)
    }

    /// Open one of the already-enumerated devices, without walking the bus again.
    ///
    /// Selection is by position, not serial: most dongles ship with the same factory serial, so
    /// `caps` decides which key identifies a device and re-derives the position from it.
    pub(crate) fn open(&self, index: usize) -> Result<RtlSdr> {
        let enumerated = self.devices.get(index).ok_or(Error::DeviceNotFound)?;
        RtlSdr::open_enumerated(enumerated)
    }
}

/// An opened RTL-SDR dongle.
///
/// Holds the claimed interface, the tuner's register shadow, and the values last written to the
/// hardware. Streaming borrows the same interface — `nusb::Interface` is `Arc`-backed — so
/// retunes ride the control endpoint while samples flow on the bulk one.
pub(crate) struct RtlSdr {
    dev: Rtl2832u,
    /// Kept alive for the lifetime of the interface claim.
    _usb_device: nusb::Device,
    tuner: R82xx,
    center_freq: u32,
    sample_rate: u32,
    rtl_xtal_freq: u32,
    /// The tuner's nominal crystal. The tuner itself is told a *corrected* value (see
    /// [`RtlSdr::set_freq_correction`]), so the nominal one has to be kept here to correct from.
    tuner_xtal_freq: u32,
    /// Crystal correction in ppm, applied to both the resampler and the tuner.
    ppm: i32,
    /// Set by the EEPROM on dongles wired to power the antenna port unconditionally.
    force_bias_t: bool,
    board_variant: BoardVariant,
    /// Whether the tuner is bypassed, and which ADC branch is being received if it is.
    direct_sampling: DirectSampling,
}

impl RtlSdr {
    /// Enumerate and open the dongle at `index` in the filtered RTL-SDR enumeration.
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
            // dvb_usb_rtl28xxu binds these dongles as a DVB-T receiver on a stock kernel; the
            // interface cannot be claimed until it lets go.
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
            // `init` programs the tuner-in-circuit registers, so a freshly opened dongle is in
            // this mode whatever the last process left behind.
            direct_sampling: DirectSampling::Off,
        };
        sdr.init()?;
        Ok(sdr)
    }

    fn init(&mut self) -> Result<()> {
        // Doubles as a liveness check: if this control transfer fails, nothing below is worth
        // attempting.
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_SYSCTL, 0x09, 1)?;
        self.init_baseband()?;

        self.dev.set_i2c_repeater(true)?;
        let (tuner_type, i2c_addr) = self.search_tuner()?;
        let is_blog_v4 = self.board_variant == BoardVariant::RtlSdrBlogV4;
        // The Blog V4 is an R828D on a 28.8 MHz crystal; every other R828D board uses 16 MHz.
        let tuner_xtal = if tuner_type == TunerType::R828D && !is_blog_v4 {
            tuner::XTAL_FREQ_16
        } else {
            DEF_RTL_XTAL_FREQ
        };
        self.tuner_xtal_freq = tuner_xtal;
        self.tuner = R82xx::new(tuner_type, i2c_addr, tuner_xtal, is_blog_v4);
        self.tuner.init(&self.dev)?;
        self.dev.set_i2c_repeater(false)?;

        // Zero-IF off, in-phase ADC only, spectrum inversion on: the RTL2832U's SDR mode.
        self.dev.demod_write_reg(1, 0xb1, 0x1a, 1)?;
        self.dev.demod_write_reg(0, 0x08, 0x4d, 1)?;
        self.set_if_freq(self.tuner.if_freq())?;
        self.dev.demod_write_reg(1, 0x15, 0x01, 1)?;

        match self.dev.read_eeprom() {
            Ok(eeprom) => {
                self.force_bias_t = eeprom[7] & 0x02 == 0;
                if self.force_bias_t {
                    debug!("EEPROM forces bias-T on");
                }
            }
            // Not every board populates the EEPROM, and the defaults above are the safe ones.
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

        // soft_rst set, then cleared.
        self.dev.demod_write_reg(1, 0x01, 0x14, 1)?;
        self.dev.demod_write_reg(1, 0x01, 0x10, 1)?;

        self.dev.demod_write_reg(1, 0x15, 0x00, 1)?;
        self.dev.demod_write_reg(1, 0x16, 0x00, 2)?;
        // DDC shift and IF registers 0x16..0x1a.
        for addr in 0x16..=0x1a {
            self.dev.demod_write_reg(1, addr, 0x00, 1)?;
        }

        self.set_fir(&DEFAULT_FIR)?;

        // SDR mode on, DAGC off.
        self.dev.demod_write_reg(0, 0x19, 0x05, 1)?;
        // FSM initial state.
        self.dev.demod_write_reg(1, 0x93, 0xf0, 1)?;
        self.dev.demod_write_reg(1, 0x94, 0x0f, 1)?;
        // AGC off.
        self.dev.demod_write_reg(1, 0x11, 0x00, 1)?;
        self.dev.demod_write_reg(1, 0x04, 0x00, 1)?;
        // PID filter off.
        self.dev.demod_write_reg(0, 0x61, 0x60, 1)?;
        // Default ADC I/Q datapath.
        self.dev.demod_write_reg(0, 0x06, 0x80, 1)?;
        // Zero-IF, DC cancel and I/Q compensation on.
        self.dev.demod_write_reg(1, 0xb1, 0x1b, 1)?;
        // 4.096 MHz clock output off.
        self.dev.demod_write_reg(0, 0x0d, 0x83, 1)
    }

    /// Write the demodulator FIR: taps 0-7 as signed bytes, taps 8-15 packed two per three
    /// bytes as signed 12-bit values.
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

    /// Probe the known tuner I2C addresses. Register 0x00 reads back [`R82XX_CHECK_VAL`] on
    /// R82xx silicon.
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

    /// The board this driver detected at open time.
    #[must_use]
    pub(crate) fn board_variant(&self) -> BoardVariant {
        self.board_variant
    }

    /// The tuner this driver found on the I2C bus.
    #[must_use]
    pub(crate) fn tuner_type(&self) -> TunerType {
        self.tuner.tuner_type()
    }

    /// The tuner's discrete gain steps, in tenths of a dB.
    #[must_use]
    pub(crate) fn gains(&self) -> &[i32] {
        self.tuner.gains()
    }

    /// The centre frequency last written to the PLL.
    #[must_use]
    pub(crate) fn center_freq(&self) -> u32 {
        self.center_freq
    }

    /// The rate the resampler actually runs at, which integer division can move off the request.
    #[must_use]
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The ADC branch the demodulator is receiving, or [`DirectSampling::Off`] for the tuner.
    #[must_use]
    pub(crate) fn direct_sampling(&self) -> DirectSampling {
        self.direct_sampling
    }

    /// Tune. The R820T covers roughly 24 MHz–1.766 GHz; a Blog V4 reaches HF by upconverting
    /// through the same tuner. In direct sampling there is no PLL in the path at all — the
    /// demodulator's own downconverter is the tuning, so the IF register *is* the frequency
    /// (librtlsdr's `rtlsdr_set_center_freq` splits on the same condition).
    pub(crate) fn set_center_freq(&mut self, freq: u32) -> Result<()> {
        if self.direct_sampling == DirectSampling::Off {
            self.dev.set_i2c_repeater(true)?;
            self.tuner.set_freq(&self.dev, freq)?;
            self.dev.set_i2c_repeater(false)?;
            // A width change may have moved the IF since the last tune.
            self.set_if_freq(self.tuner.if_freq())?;
        } else {
            self.set_if_freq(freq)?;
        }
        self.center_freq = freq;
        Ok(())
    }

    /// Bypass the tuner and receive an ADC branch directly, or put the tuner back in circuit.
    ///
    /// The register sequence is librtlsdr's `rtlsdr_set_direct_sampling`. It deliberately does
    /// *not* retune: the caller holds a centre for the mode being entered, and the cached one
    /// belongs to the mode being left — on a generic dongle those two live in ranges the other
    /// mode cannot reach, so tuning here would drive the tuner at an HF frequency it has no
    /// divider for.
    ///
    /// Leaving also re-initializes the tuner, which resets its register shadow: the gain and the
    /// IF filter are gone with it and the caller has to write them again.
    pub(crate) fn set_direct_sampling(&mut self, mode: DirectSampling) -> Result<()> {
        match mode {
            DirectSampling::Off => {
                self.dev.set_i2c_repeater(true)?;
                self.tuner.init(&self.dev)?;
                self.dev.set_i2c_repeater(false)?;
                self.direct_sampling = mode;
                // Zero-IF off and in-phase ADC only: the R82xx path's own settings, as `init`
                // writes them. Re-asserted rather than assumed, so the mode switch stands on its
                // own registers instead of on what was last left in them.
                self.dev.demod_write_reg(1, 0xb1, 0x1a, 1)?;
                self.dev.demod_write_reg(0, 0x08, 0x4d, 1)?;
                self.set_if_freq(self.tuner.if_freq())?;
                // Spectrum inversion back on: the R82xx's low-side injection inverts, and the
                // ADC branch feeding the demodulator directly does not.
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
                // Bit 4 swaps the ADC inputs, which is the whole of the branch selection.
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

    /// Set the sample rate. Valid windows are 225001–300000 Hz and 900001–3200000 Hz; read
    /// [`RtlSdr::sample_rate`] back for what the resampler could actually produce.
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
        // The tuner's bandwidth calculation reads this back, so it has to land first.
        self.sample_rate = actual_rate;

        // Only the tuner's IF filter tracks the rate. In direct sampling the tuner is in standby
        // and the IF registers hold the *tuning*, so following librtlsdr here — which re-runs the
        // filter calculation and rewrites the IF whatever the mode — would silently retune the
        // radio to 3.57 MHz on every rate change.
        if self.direct_sampling == DirectSampling::Off {
            self.dev.set_i2c_repeater(true)?;
            let if_freq = self.tuner.set_bandwidth(&self.dev, actual_rate)?;
            self.dev.set_i2c_repeater(false)?;
            self.set_if_freq(if_freq)?;
            // The IF moved, so the PLL has to follow it or the radio receives the wrong frequency.
            if self.center_freq != 0 {
                self.set_center_freq(self.center_freq)?;
            }
        }

        self.dev
            .demod_write_reg(1, 0x9f, (rsamp_ratio >> 16) as u16, 2)?;
        self.dev
            .demod_write_reg(1, 0xa1, (rsamp_ratio & 0xffff) as u16, 2)?;
        // The correction registers sit beside the ratio and are not carried over by a rate
        // change; librtlsdr re-writes them here for the same reason.
        self.write_sample_freq_correction(self.ppm)?;
        self.dev.demod_write_reg(1, 0x01, 0x14, 1)?;
        self.dev.demod_write_reg(1, 0x01, 0x10, 1)
    }

    /// The crystal correction in ppm the device is currently applying.
    #[must_use]
    pub(crate) fn freq_correction(&self) -> i32 {
        self.ppm
    }

    /// Correct for a crystal that is not at its nominal frequency, librtlsdr-shaped.
    ///
    /// Both halves matter and they are different mechanisms. The RTL2832U's *resampler* is
    /// corrected through a dedicated register pair, which fixes the sample rate. The *tuner* has
    /// no such register — its PLL is programmed by dividing the crystal — so it is corrected by
    /// telling it the crystal is somewhere else and re-tuning. Doing only the first leaves every
    /// received frequency off by `ppm`; doing only the second leaves the sample rate wrong,
    /// which mistimes every decoder downstream.
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

    /// Write the resampler's correction. The value is negated because the register shifts the
    /// resampler's *phase increment*, which moves the output rate the opposite way.
    fn write_sample_freq_correction(&self, ppm: i32) -> Result<()> {
        let offset = -(i64::from(ppm) << 24) / 1_000_000;
        self.dev
            .demod_write_reg(1, 0x3f, (offset & 0xff) as u16, 1)?;
        self.dev
            .demod_write_reg(1, 0x3e, ((offset >> 8) & 0x3f) as u16, 1)
    }

    /// Point the demodulator's downconverter at `freq`, so it mixes that back down to zero.
    ///
    /// The crystal is the *corrected* one, as librtlsdr's `rtlsdr_set_if_freq` reads it: the NCO
    /// is derived from it, so an uncorrected value leaves the mixdown off by the crystal's own
    /// error. It is a few hertz at a 3.57 MHz IF and easy to overlook — but in direct sampling
    /// this register is the tuning, and the error is the operator's whole ppm at the dial.
    fn set_if_freq(&self, freq: u32) -> Result<()> {
        let if_reg = if_freq_reg(freq, corrected_xtal(self.rtl_xtal_freq, self.ppm));
        self.dev
            .demod_write_reg(1, 0x19, ((if_reg >> 16) & 0x3f) as u16, 1)?;
        self.dev
            .demod_write_reg(1, 0x1a, ((if_reg >> 8) & 0xff) as u16, 1)?;
        self.dev.demod_write_reg(1, 0x1b, (if_reg & 0xff) as u16, 1)
    }

    /// Refuse a write aimed at a tuner that is not in the signal path. The backend's `apply`
    /// plans no such write; reaching one means a caller has bypassed the plan, and driving a
    /// standby tuner over I2C would half-wake it while changing nothing a listener can hear.
    fn require_tuner(&self, what: &str) -> Result<()> {
        if self.direct_sampling == DirectSampling::Off {
            return Ok(());
        }
        Err(Error::InvalidParam(format!(
            "{what}: the tuner is bypassed while direct sampling"
        )))
    }

    /// Hand the LNA and mixer back to the tuner's own AGC.
    pub(crate) fn set_gain_auto(&mut self) -> Result<()> {
        self.require_tuner("tuner AGC")?;
        self.dev.set_i2c_repeater(true)?;
        self.tuner.set_gain_auto(&self.dev)?;
        self.dev.set_i2c_repeater(false)
    }

    /// Set manual gain in tenths of a dB, snapped to the nearest step the tuner supports.
    pub(crate) fn set_gain_manual(&mut self, gain_tenth_db: i32) -> Result<()> {
        self.require_tuner("tuner gain")?;
        self.dev.set_i2c_repeater(true)?;
        self.tuner.set_gain_manual(&self.dev, gain_tenth_db)?;
        self.dev.set_i2c_repeater(false)
    }

    /// Set the IF filter width in Hz, or 0 to follow the sample rate. Returns the IF frequency
    /// the tuner ended up on, which narrow widths move off the 3.57 MHz default.
    pub(crate) fn set_bandwidth(&mut self, bw: u32) -> Result<u32> {
        self.require_tuner("IF filter width")?;
        let bw = if bw == 0 { self.sample_rate } else { bw };
        self.dev.set_i2c_repeater(true)?;
        let if_freq = self.tuner.set_bandwidth(&self.dev, bw)?;
        self.dev.set_i2c_repeater(false)?;
        self.set_if_freq(if_freq)?;
        Ok(if_freq)
    }

    /// Switch phantom power on the antenna port (GPIO0). A dongle whose EEPROM forces it on
    /// cannot be switched off.
    pub(crate) fn set_bias_t(&mut self, enable: bool) -> Result<()> {
        let actual = enable || self.force_bias_t;
        self.dev.set_gpio_output(0)?;
        self.dev.set_gpio_bit(0, actual)
    }

    /// Start streaming IQ bytes.
    ///
    /// Safe to call again on a live handle: the endpoint FIFO reset below and a fresh
    /// [`NusbBulkIn`] are all a restart needs, which is what makes recovering from a stalled
    /// pipe cost milliseconds instead of a re-open. Nothing about the tuning is disturbed.
    pub(crate) fn start_streaming(&mut self) -> Result<RxStream> {
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_EPA_CTL, 0x1002, 2)?;
        self.dev
            .write_reg(regs::BLOCK_USB, regs::USB_EPA_CTL, 0x0000, 2)?;

        let endpoint = NusbBulkIn::open(self.dev.interface(), BULK_ENDPOINT)?;
        let stream = sdrmm_usb_stream::start(
            endpoint,
            StreamConfig::new(TRANSFER_BUF_SIZE, "sdrmm-rtlsdr-usb"),
        )?;
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

/// The demodulator's downconverter register value for an IF of `freq` on a crystal of `xtal_hz`.
///
/// Negated because the register shifts the NCO, which moves the received frequency the opposite
/// way. It is written as 22 bits two's complement across `0x19`/`0x1a`/`0x1b`, so the largest
/// magnitude it holds is half the crystal — which is also the Nyquist limit of what the ADC could
/// have delivered ([`DIRECT_SAMPLING_MAX_HZ`]).
fn if_freq_reg(freq: u32, xtal_hz: u32) -> i32 {
    -((i64::from(freq) * (1 << 22)) / i64::from(xtal_hz)) as i32
}

/// A crystal frequency scaled by a ppm correction.
fn corrected_xtal(nominal_hz: u32, ppm: i32) -> u32 {
    let corrected = f64::from(nominal_hz) * (1.0 + f64::from(ppm) / 1e6);
    corrected.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The demodulator's resampler ratio, and the rate it actually produces.
///
/// The ratio is a 28-bit field with the low two bits masked off, so a requested rate can come
/// back changed and the caller must report the second value. On a 28.8 MHz crystal it never
/// does — the masked bits are worth well under 1 Hz across both valid windows — but the
/// arithmetic is librtlsdr's and holds for a crystal corrected off nominal too.
fn resample_ratio(xtal_freq: u32, rate: u32) -> (u32, u32) {
    let ratio = ((u64::from(xtal_freq) << 22) / u64::from(rate)) as u32 & 0x0fff_fffc;
    // Bit 27 is the sign bit of the 28-bit field; librtlsdr extends it before dividing back.
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
        // The strings are matched case-insensitively, as firmware casing varies.
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

    /// Both ends of both valid windows, so a rate that the mask *did* move would show up here
    /// rather than silently mistiming every decoder downstream.
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

    /// The register pair is 14 bits signed; the published limit must be the largest ppm that
    /// still fits, or a correction would silently wrap into the opposite sign.
    #[test]
    fn the_ppm_limit_is_the_register_width() {
        let offset = |ppm: i32| -(i64::from(ppm) << 24) / 1_000_000;
        assert!(offset(MAX_PPM).abs() <= 0x1fff, "{}", offset(MAX_PPM));
        assert!(offset(MAX_PPM + 1).abs() > 0x1fff);
    }

    /// The three-byte register the demodulator tunes with. In direct sampling it is the whole of
    /// the frequency control, so its arithmetic is worth pinning to values that can be checked by
    /// hand: `freq / xtal * 2^22`, negated.
    #[test]
    fn the_downconverter_register_is_the_negated_fraction_of_the_crystal() {
        assert_eq!(if_freq_reg(0, DEF_RTL_XTAL_FREQ), 0);
        // 3.57 MHz of 28.8 MHz is 0.123958…, and 0.123958 × 2^22 = 519 918.
        assert_eq!(if_freq_reg(3_570_000, DEF_RTL_XTAL_FREQ), -519_918);
        // A quarter of the crystal is a quarter turn of the NCO.
        assert_eq!(if_freq_reg(7_200_000, DEF_RTL_XTAL_FREQ), -(1 << 20));
    }

    /// The register is 22 bits signed. The advertised direct-sampling ceiling must be the largest
    /// frequency that still fits, or the top of HF would wrap to the bottom of it.
    #[test]
    fn the_direct_sampling_ceiling_is_the_register_width() {
        let at_limit = if_freq_reg(DIRECT_SAMPLING_MAX_HZ, DEF_RTL_XTAL_FREQ);
        assert_eq!(at_limit, -(1 << 21), "the most negative 22-bit value");
        assert!(if_freq_reg(DIRECT_SAMPLING_MAX_HZ + 100, DEF_RTL_XTAL_FREQ) < -(1 << 21));
    }

    /// The correction has to reach the downconverter, not just the tuner: in direct sampling the
    /// tuner is in standby and this register is the only thing that moves the dial.
    #[test]
    fn a_ppm_correction_moves_the_downconverter_register() {
        let nominal = if_freq_reg(7_100_000, DEF_RTL_XTAL_FREQ);
        let corrected = if_freq_reg(7_100_000, corrected_xtal(DEF_RTL_XTAL_FREQ, 100));
        // A crystal 100 ppm fast needs 100 ppm less of a turn per sample to sit on the same
        // frequency, so the magnitude drops — by 100 ppm of it, which is 103 counts.
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
        // +50 ppm of 28.8 MHz is 1440 Hz.
        assert_eq!(corrected_xtal(DEF_RTL_XTAL_FREQ, 50), 28_801_440);
        assert_eq!(corrected_xtal(DEF_RTL_XTAL_FREQ, -50), 28_798_560);
        // A correction smaller than the rounding step must not move the crystal at all.
        assert_eq!(corrected_xtal(1_000_000, 0), 1_000_000);
    }

    // No test may open or enumerate a device: both walk the USB bus, so what they find is
    // whatever is plugged into the machine running them (PLAN §14: no hardware in CI, ever).
}
