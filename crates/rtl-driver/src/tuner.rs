//! R82xx (R820T / R828D) tuner driver.
//!
//! This module implements the complete tuner initialization, frequency setting,
//! gain control, and bandwidth configuration for the R820T and R828D tuner
//! chips used in RTL-SDR dongles. The tuner communicates with the RTL2832U
//! baseband via I2C.
//!
//! # Register Layout
//!
//! The R82xx has 32 registers (0x00-0x1f):
//! - 0x00-0x04: Read-only (status, PLL lock, VCO, filter calibration)
//! - 0x05-0x1f: Read-write (27 registers, cached locally to avoid reads)
//!
//! # I2C Protocol
//!
//! Data read from the R82xx has **bit-reversed** byte order. Every byte read
//! must be passed through [`bit_reverse`] before use. Writes are normal order.
//! Maximum I2C message length is 8 bytes.

use tracing::{debug, trace, warn};

use crate::{
    device::Device,
    error::{Error, Result},
};

/// Supported R82xx tuner variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunerType {
    /// R820T — most common RTL-SDR tuner, I2C address 0x34.
    R820T,
    /// R828D — found in RTL-SDR Blog V4 and some other dongles, I2C address 0x74.
    R828D,
}

// ── Known tuner I2C addresses ───────────────────────────────────────────────

/// R820T I2C address.
pub(crate) const R820T_I2C_ADDR: u8 = 0x34;
/// R828D I2C address.
pub(crate) const R828D_I2C_ADDR: u8 = 0x74;
/// Register 0x00 reads back as this on R82xx silicon; it is how the tuner is identified.
pub(crate) const R82XX_CHECK_VAL: u8 = 0x69;

/// Known tuners to probe during detection.
pub(crate) const KNOWN_TUNERS: &[(TunerType, u8)] = &[
    (TunerType::R820T, R820T_I2C_ADDR),
    (TunerType::R828D, R828D_I2C_ADDR),
];

// ── Constants ───────────────────────────────────────────────────────────────

/// Maximum I2C message length for R82xx.
const MAX_I2C_MSG_LEN: usize = 8;

/// Default IF frequency for 6 MHz DVB-T mode (Hz).
pub(crate) const R82XX_IF_FREQ: u32 = 3_570_000;

/// R82xx driver version number written to register 0x13.
const VER_NUM: u8 = 49;

/// Default crystal frequency (Hz) for R820T and Blog V4 R828D.
pub(crate) const XTAL_FREQ_28_8: u32 = 28_800_000;
/// Crystal frequency (Hz) for non-Blog-V4 R828D.
pub(crate) const XTAL_FREQ_16: u32 = 16_000_000;

/// Number of writeable registers (0x05 through 0x1f).
const NUM_REGS: usize = 27;
/// First writeable register address.
const REG_SHADOW_START: u8 = 0x05;

// ── Initial register values (0x05-0x1f) ────────────────────────────────────

/// Default register values for the R82xx, loaded during init.
/// These are the 27 bytes for registers 0x05 through 0x1f.
const REG_INIT: [u8; NUM_REGS] = [
    0x83, // 0x05
    0x32, // 0x06
    0x75, // 0x07
    0xc0, // 0x08
    0x40, // 0x09
    0xd6, // 0x0a
    0x6c, // 0x0b
    0xf5, // 0x0c
    0x63, // 0x0d
    0x75, // 0x0e
    0x68, // 0x0f
    0x6c, // 0x10
    0x83, // 0x11
    0x80, // 0x12
    0x00, // 0x13
    0x0f, // 0x14
    0x00, // 0x15
    0xc0, // 0x16
    0x30, // 0x17
    0x48, // 0x18
    0xcc, // 0x19
    0x60, // 0x1a
    0x00, // 0x1b
    0x54, // 0x1c
    0xae, // 0x1d
    0x4a, // 0x1e
    0xc0, // 0x1f
];

// ── Gain tables ─────────────────────────────────────────────────────────────

/// Available total gain values in tenths of a dB (cumulative LNA + mixer).
/// 29 discrete values from 0.0 dB to 49.6 dB.
pub const GAIN_VALUES: &[i32] = &[
    0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372,
    386, 402, 421, 434, 439, 445, 480, 496,
];

/// LNA gain steps (16 entries, index 0-15). Each is the incremental dB×10
/// added at that index.
const LNA_GAIN_STEPS: [i32; 16] = [0, 9, 13, 40, 38, 13, 31, 22, 26, 31, 26, 14, 19, 5, 35, 13];

/// Mixer gain steps (16 entries, index 0-15). Each is the incremental dB×10
/// added at that index.
const MIXER_GAIN_STEPS: [i32; 16] = [0, 5, 10, 10, 19, 9, 10, 25, 17, 10, 8, 16, 13, 6, 3, -8];

// ── Frequency range table for set_mux ───────────────────────────────────────

/// RF frontend filter configuration per frequency band.
struct FreqRange {
    freq_mhz: u32,
    open_d: u8,
    rf_mux_ploy: u8,
    tf_c: u8,
    xtal_cap0p: u8,
}

/// Frequency ranges for the tracking filter / RF mux configuration.
/// Sorted by ascending frequency. The first entry whose `freq_mhz` is >=
/// the target frequency is selected.
const FREQ_RANGES: &[FreqRange] = &[
    FreqRange {
        freq_mhz: 0,
        open_d: 0x08,
        rf_mux_ploy: 0x02,
        tf_c: 0xdf,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 50,
        open_d: 0x08,
        rf_mux_ploy: 0x02,
        tf_c: 0xbe,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 55,
        open_d: 0x08,
        rf_mux_ploy: 0x02,
        tf_c: 0x8b,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 60,
        open_d: 0x08,
        rf_mux_ploy: 0x02,
        tf_c: 0x7b,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 65,
        open_d: 0x08,
        rf_mux_ploy: 0x02,
        tf_c: 0x69,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 70,
        open_d: 0x08,
        rf_mux_ploy: 0x02,
        tf_c: 0x58,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 75,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x44,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 80,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x44,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 90,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x34,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 100,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x34,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 110,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x24,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 120,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x24,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 140,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x14,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 180,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x13,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 220,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x13,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 250,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x11,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 280,
        open_d: 0x00,
        rf_mux_ploy: 0x02,
        tf_c: 0x00,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 310,
        open_d: 0x00,
        rf_mux_ploy: 0x41,
        tf_c: 0x00,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 450,
        open_d: 0x00,
        rf_mux_ploy: 0x41,
        tf_c: 0x00,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 588,
        open_d: 0x00,
        rf_mux_ploy: 0x40,
        tf_c: 0x00,
        xtal_cap0p: 0x00,
    },
    FreqRange {
        freq_mhz: 650,
        open_d: 0x00,
        rf_mux_ploy: 0x40,
        tf_c: 0x00,
        xtal_cap0p: 0x00,
    },
];

// ── Bandwidth filter table ──────────────────────────────────────────────────

/// Low-pass filter cutoff values for narrow bandwidth modes (Hz).
const BW_LP_CUTOFFS: &[u32] = &[
    1_700_000, 1_600_000, 1_550_000, 1_450_000, 1_200_000, 900_000, 700_000, 550_000, 450_000,
    350_000,
];

/// Half-bandwidth for high-pass filter 1 (Hz).
const HP_BW1: u32 = 350_000;
/// Half-bandwidth for high-pass filter 2 (Hz).
const HP_BW2: u32 = 380_000;

// ── Bit reversal LUT for I2C reads ─────────────────────────────────────────

/// Reverse the bits in a byte. Required because the R82xx outputs data
/// in reversed bit order over I2C.
fn bit_reverse(byte: u8) -> u8 {
    const LUT: [u8; 16] = [
        0x0, 0x8, 0x4, 0xc, 0x2, 0xa, 0x6, 0xe, 0x1, 0x9, 0x5, 0xd, 0x3, 0xb, 0x7, 0xf,
    ];
    (LUT[(byte & 0xf) as usize] << 4) | LUT[(byte >> 4) as usize]
}

// ── R82xx driver ────────────────────────────────────────────────────────────

/// R82xx tuner driver state.
///
/// Maintains a shadow copy of all writable registers (0x05-0x1f) and provides
/// methods for initialization, frequency tuning, gain control, and bandwidth
/// configuration.
#[derive(Clone)]
pub(crate) struct R82xx {
    /// Tuner variant (R820T or R828D).
    tuner_type: TunerType,
    /// I2C address of this tuner.
    i2c_addr: u8,
    /// Shadow copy of registers 0x05-0x1f.
    regs: [u8; NUM_REGS],
    /// IF frequency in Hz (set during bandwidth configuration).
    pub(crate) int_freq: u32,
    /// Crystal frequency in Hz.
    xtal_freq: u32,
    /// Whether this is an RTL-SDR Blog V4 device.
    is_blog_v4: bool,
    /// Filter calibration code from init.
    fil_cal_code: u8,
    /// Whether the PLL has been locked at least once.
    has_lock: bool,
}

impl R82xx {
    /// Create a new R82xx driver for the given tuner type.
    pub(crate) fn new(
        tuner_type: TunerType,
        i2c_addr: u8,
        xtal_freq: u32,
        is_blog_v4: bool,
    ) -> Self {
        Self {
            tuner_type,
            i2c_addr,
            regs: REG_INIT,
            int_freq: R82XX_IF_FREQ,
            xtal_freq,
            is_blog_v4,
            fil_cal_code: 0,
            has_lock: false,
        }
    }

    /// Get the tuner type.
    pub(crate) fn tuner_type(&self) -> TunerType {
        self.tuner_type
    }

    /// Tell the tuner its crystal is not where it thought it was. Every PLL programming step
    /// divides by this, so a ppm correction is applied here and takes effect on the next tune.
    pub(crate) const fn set_xtal_freq(&mut self, freq: u32) {
        self.xtal_freq = freq;
    }

    /// Get the current IF frequency.
    pub(crate) fn if_freq(&self) -> u32 {
        self.int_freq
    }

    /// Get the list of supported gain values (in tenths of dB).
    pub(crate) fn gains(&self) -> &[i32] {
        GAIN_VALUES
    }

    // ── I2C register access ─────────────────────────────────────────────────

    /// Write registers starting at `start_reg` from the shadow cache.
    ///
    /// Chunks writes to MAX_I2C_MSG_LEN (8 bytes) because the I2C bus has
    /// a maximum transfer size. Each I2C message starts with the register
    /// address byte followed by data bytes.
    fn write_regs(&self, dev: &Device, start_reg: u8, count: usize) -> Result<()> {
        let shadow_off = (start_reg - REG_SHADOW_START) as usize;
        let data = &self.regs[shadow_off..shadow_off + count];

        let mut pos = 0;
        while pos < count {
            let chunk_len = (count - pos).min(MAX_I2C_MSG_LEN - 1); // -1 for addr byte
            let mut buf = Vec::with_capacity(chunk_len + 1);
            buf.push(start_reg + pos as u8);
            buf.extend_from_slice(&data[pos..pos + chunk_len]);
            dev.i2c_write(self.i2c_addr, &buf)?;
            pos += chunk_len;
        }

        Ok(())
    }

    /// Write a single register with a mask (read-modify-write on shadow).
    ///
    /// Updates the shadow register cache, then writes only the affected
    /// register to hardware. This avoids unnecessary I2C reads.
    fn write_reg_mask(&mut self, dev: &Device, reg: u8, val: u8, mask: u8) -> Result<()> {
        let idx = (reg - REG_SHADOW_START) as usize;
        let old = self.regs[idx];
        let new_val = (old & !mask) | (val & mask);
        self.regs[idx] = new_val;

        let buf = [reg, new_val];
        dev.i2c_write(self.i2c_addr, &buf)
    }

    /// Read registers from the tuner hardware.
    ///
    /// Applies bit-reversal to each byte (R82xx hardware quirk).
    fn read_regs(&self, dev: &Device, start_reg: u8, count: u16) -> Result<Vec<u8>> {
        dev.i2c_write(self.i2c_addr, &[start_reg])?;
        let data = dev.i2c_read(self.i2c_addr, count)?;
        Ok(data.into_iter().map(bit_reverse).collect())
    }

    // ── Initialization ──────────────────────────────────────────────────────

    /// Initialize the R82xx tuner.
    ///
    /// Writes default register values, configures for DVB-T 6 MHz mode,
    /// and performs filter calibration.
    pub(crate) fn init(&mut self, dev: &Device) -> Result<()> {
        debug!(
            "R82xx init: {:?} at 0x{:02x}, xtal={}Hz, blog_v4={}",
            self.tuner_type, self.i2c_addr, self.xtal_freq, self.is_blog_v4
        );

        // Reset shadow to defaults
        self.regs = REG_INIT;

        // Write all default register values (0x05-0x1f)
        self.write_regs(dev, REG_SHADOW_START, NUM_REGS)?;

        // Configure for DVB-T standard (6 MHz bandwidth)
        self.set_tv_standard(dev)?;

        // Configure system-frequency-dependent parameters
        self.sysfreq_sel(dev, 0)?;

        debug!(
            "R82xx init complete, fil_cal_code=0x{:02x}",
            self.fil_cal_code
        );
        Ok(())
    }

    /// Put the tuner into standby mode.
    pub(crate) fn standby(&mut self, dev: &Device) -> Result<()> {
        debug!("R82xx entering standby");

        self.write_reg_mask(dev, 0x06, 0xb1, 0xff)?;
        self.write_reg_mask(dev, 0x05, 0xa0, 0xff)?;
        self.write_reg_mask(dev, 0x07, 0x3a, 0xff)?;
        self.write_reg_mask(dev, 0x08, 0x40, 0xff)?;
        self.write_reg_mask(dev, 0x09, 0xc0, 0xff)?;
        self.write_reg_mask(dev, 0x0a, 0x36, 0xff)?;
        self.write_reg_mask(dev, 0x0c, 0x35, 0xff)?;
        self.write_reg_mask(dev, 0x0f, 0x68, 0xff)?;
        self.write_reg_mask(dev, 0x11, 0x03, 0xff)?;
        self.write_reg_mask(dev, 0x17, 0xf4, 0xff)?;
        self.write_reg_mask(dev, 0x19, 0x0c, 0xff)?;

        Ok(())
    }

    // ── TV Standard / Filter Configuration ──────────────────────────────────

    /// Configure the tuner for DVB-T 6 MHz mode (SDR use).
    ///
    /// This sets up the IF filter, image rejection, and performs filter
    /// calibration. Called during init.
    fn set_tv_standard(&mut self, dev: &Device) -> Result<()> {
        // DVB-T 6 MHz constants (matching reference exactly)
        let if_khz: u32 = 3570;
        let filt_cal_lo: u32 = 56000;
        let filt_gain: u8 = 0x10; // +3dB, 6MHz on
        let img_r: u8 = 0x00; // image negative
        let filt_q: u8 = 0x10; // r10[4]: low q (1'b1)
        let hp_cor: u8 = 0x6b; // 1.7m disable, +2cap, 1.0MHz
        let ext_enable: u8 = 0x60; // r30[6]=1 ext enable; r30[5]:1 ext at lna max-1
        let loop_through: u8 = 0x01; // r5[7], lt off
        let lt_att: u8 = 0x00; // r31[7], lt att enable
        let flt_ext_widest: u8 = 0x00; // r15[7]: flt_ext_wide off
        let polyfil_cur: u8 = 0x60; // r25[6:5]: min

        // Re-initialize register cache
        self.regs = REG_INIT;

        // Clear VGA gain init (reg 0x0c bits [3:0])
        self.write_reg_mask(dev, 0x0c, 0x00, 0x0f)?;

        // Write version number (reg 0x13 bits [5:0])
        self.write_reg_mask(dev, 0x13, VER_NUM, 0x3f)?;

        // For LT Gain test (non-analog TV): clear LNA TOP bits
        self.write_reg_mask(dev, 0x1d, 0x00, 0x38)?;

        // Set int_freq early (before calibration)
        self.int_freq = if_khz * 1000;

        // --- Filter calibration (retry up to 2 times) ---
        for _ in 0..2 {
            // Set filt_cap (reg 0x0b bits [6:5])
            self.write_reg_mask(dev, 0x0b, hp_cor, 0x60)?;

            // Enable cal clock (reg 0x0f bit 2)
            self.write_reg_mask(dev, 0x0f, 0x04, 0x04)?;

            // Set xtal cap to 0pF for calibration (reg 0x10 bits [1:0])
            self.write_reg_mask(dev, 0x10, 0x00, 0x03)?;

            // Lock PLL at calibration frequency
            self.set_pll(dev, filt_cal_lo * 1000)?;

            // Start trigger (reg 0x0b bit 4)
            self.write_reg_mask(dev, 0x0b, 0x10, 0x10)?;

            // Allow calibration to settle
            std::thread::sleep(Duration::from_millis(2));

            // Stop trigger (reg 0x0b bit 4)
            self.write_reg_mask(dev, 0x0b, 0x00, 0x10)?;

            // Read calibration result from regs 0x00-0x04, cal code in byte 4
            let cal_data = self.read_regs(dev, 0x00, 5)?;
            self.fil_cal_code = cal_data[4] & 0x0f;

            if self.fil_cal_code != 0 && self.fil_cal_code != 0x0f {
                break;
            }
        }

        // If cal code is still 0x0f after retries, reset to 0
        if self.fil_cal_code == 0x0f {
            self.fil_cal_code = 0;
        }

        trace!("filter cal code: 0x{:02x}", self.fil_cal_code);

        // Disable calibration clock (reg 0x0f bit 2)
        self.write_reg_mask(dev, 0x0f, 0x00, 0x04)?;

        // --- Configure for 6 MHz DVB-T ---

        // Filter Q + cal code (reg 0x0a bits [4:0])
        self.write_reg_mask(dev, 0x0a, filt_q | self.fil_cal_code, 0x1f)?;

        // HP corner / BW (reg 0x0b bits [7:5,3:0])
        self.write_reg_mask(dev, 0x0b, hp_cor, 0xef)?;

        // Image reject (reg 0x07 bit 7)
        self.write_reg_mask(dev, 0x07, img_r, 0x80)?;

        // Filter gain +3dB, 6MHz on (reg 0x06 bits [5:4])
        self.write_reg_mask(dev, 0x06, filt_gain, 0x30)?;

        // Channel filter extension (reg 0x1e bits [6:5])
        self.write_reg_mask(dev, 0x1e, ext_enable, 0x60)?;

        // Loop-through off (reg 0x05 bit 7)
        self.write_reg_mask(dev, 0x05, loop_through, 0x80)?;

        // Loop-through attenuation (reg 0x1f bit 7)
        self.write_reg_mask(dev, 0x1f, lt_att, 0x80)?;

        // Filter extension widest (reg 0x0f bit 7)
        self.write_reg_mask(dev, 0x0f, flt_ext_widest, 0x80)?;

        // RF poly filter current (reg 0x19 bits [6:5])
        self.write_reg_mask(dev, 0x19, polyfil_cur, 0x60)?;

        Ok(())
    }

    /// Configure system-frequency-dependent parameters for DVB-T.
    fn sysfreq_sel(&mut self, dev: &Device, freq: u32) -> Result<()> {
        // DVB-T defaults (SysUndefined / 8MHz path in reference)
        let mut mixer_top: u8 = 0x24; // mixer top:13, top-1, low-discharge
        let lna_top: u8 = 0xe5; // detect bw 3, lna top:4, predet top:2
        let mut cp_cur: u8 = 0x38; // 111, auto
        let lna_vth_l: u8 = 0x53; // lna vth 0.84, vtl 0.64
        let mixer_vth_l: u8 = 0x75; // mixer vth 1.04, vtl 0.84
        let air_cable1_in: u8 = 0x00;
        let cable2_in: u8 = 0x00;
        let lna_discharge: u8 = 14;
        let filter_cur: u8 = 0x40; // 10, low
        let mut div_buf_cur: u8 = 0x30; // 11, 150uA

        // Frequency-dependent overrides (DVB-T specific frequencies)
        if freq == 506_000_000 || freq == 666_000_000 || freq == 818_000_000 {
            mixer_top = 0x14; // mixer top:14, top-1, low-discharge
            cp_cur = 0x28; // 101, 0.2
            div_buf_cur = 0x20; // 10, 200uA
        }

        // --- Write registers in exact order matching reference ---

        // LNA top (reg 0x1d, mask 0xc7)
        self.write_reg_mask(dev, 0x1d, lna_top, 0xc7)?;
        // Mixer top (reg 0x1c, mask 0xf8)
        self.write_reg_mask(dev, 0x1c, mixer_top, 0xf8)?;
        // LNA vth/vtl (reg 0x0d, full write)
        self.write_reg_mask(dev, 0x0d, lna_vth_l, 0xff)?;
        // Mixer vth/vtl (reg 0x0e, full write)
        self.write_reg_mask(dev, 0x0e, mixer_vth_l, 0xff)?;

        // Air-in (reg 0x05 bits [6:5])
        self.write_reg_mask(dev, 0x05, air_cable1_in, 0x60)?;
        // Cable2 (reg 0x06 bit 3)
        self.write_reg_mask(dev, 0x06, cable2_in, 0x08)?;

        // Charge pump current (reg 0x11, mask 0x38)
        self.write_reg_mask(dev, 0x11, cp_cur, 0x38)?;

        // Divider buffer current (reg 0x17, mask 0x30)
        self.write_reg_mask(dev, 0x17, div_buf_cur, 0x30)?;
        // Filter current (reg 0x0a, mask 0x60)
        self.write_reg_mask(dev, 0x0a, filter_cur, 0x60)?;

        // --- LNA ramp sequence (digital TV, non-analog) ---

        // LNA TOP: lowest (reg 0x1d bits [5:3])
        self.write_reg_mask(dev, 0x1d, 0, 0x38)?;
        // Normal mode (reg 0x1c bit 2)
        self.write_reg_mask(dev, 0x1c, 0, 0x04)?;
        // PRE_DECT off (reg 0x06 bit 6)
        self.write_reg_mask(dev, 0x06, 0, 0x40)?;
        // AGC clock 250Hz (reg 0x1a bits [5:4])
        self.write_reg_mask(dev, 0x1a, 0x30, 0x30)?;

        // Write LNA TOP = 3 (reg 0x1d bits [5:3])
        self.write_reg_mask(dev, 0x1d, 0x18, 0x38)?;

        // Discharge mode (reg 0x1c bit 2). The mask looks too narrow for `mixer_top`, but
        // librtlsdr writes exactly this, and the tuner is calibrated against it.
        self.write_reg_mask(dev, 0x1c, mixer_top, 0x04)?;
        // LNA discharge current (reg 0x1e bits [4:0])
        self.write_reg_mask(dev, 0x1e, lna_discharge, 0x1f)?;
        // AGC clock 60Hz (reg 0x1a bits [5:4])
        self.write_reg_mask(dev, 0x1a, 0x20, 0x30)?;

        Ok(())
    }

    // ── Frequency Setting ───────────────────────────────────────────────────

    /// Set the tuner center frequency in Hz.
    ///
    /// This configures the RF frontend filters (tracking filter, image reject),
    /// programs the PLL synthesizer, and handles R828D Blog V4 upconversion
    /// and input switching.
    pub(crate) fn set_freq(&mut self, dev: &Device, freq: u32) -> Result<()> {
        debug!("R82xx set_freq: {} Hz", freq);

        // Blog V4 upconversion for HF
        let upconverted_freq =
            if self.is_blog_v4 && self.tuner_type == TunerType::R828D && freq < XTAL_FREQ_28_8 {
                debug!(
                    "Blog V4 HF upconversion: {} + {} = {} Hz",
                    freq,
                    XTAL_FREQ_28_8,
                    freq + XTAL_FREQ_28_8
                );
                freq + XTAL_FREQ_28_8
            } else {
                freq
            };

        // LO = RF + IF
        let lo_freq = upconverted_freq.saturating_add(self.int_freq);

        // Configure RF frontend mux/filters
        self.set_mux(dev, lo_freq)?;

        // Program PLL
        self.set_pll(dev, lo_freq)?;

        // R828D input switching
        if self.tuner_type == TunerType::R828D {
            if self.is_blog_v4 {
                self.set_blog_v4_input(dev, freq)?;
            } else {
                self.set_r828d_input(dev, freq)?;
            }
        }

        Ok(())
    }

    /// Configure RF mux and tracking filter for the given LO frequency.
    fn set_mux(&mut self, dev: &Device, lo_freq: u32) -> Result<()> {
        let freq_mhz = lo_freq / 1_000_000;

        // Find the matching frequency range
        let range = FREQ_RANGES
            .iter()
            .rev()
            .find(|r| freq_mhz >= r.freq_mhz)
            .unwrap_or(&FREQ_RANGES[0]);

        // Open drain control (reg 0x17 bit 3)
        self.write_reg_mask(dev, 0x17, range.open_d, 0x08)?;

        // RF mux + polymux (reg 0x1a, mask 0xc3)
        self.write_reg_mask(dev, 0x1a, range.rf_mux_ploy, 0xc3)?;

        // Tracking filter band (reg 0x1b)
        self.write_reg_mask(dev, 0x1b, range.tf_c, 0xff)?;

        // Crystal cap selection — use xtal_cap0p (high cap, most common).
        // Note: XtalHighCap0p does NOT set crystal drive bit (0x08).
        // Other xtal_cap_sel values (LowCap30p/20p/10p/0p) would OR 0x08,
        // but R82xx init always selects XtalHighCap0p.
        self.write_reg_mask(dev, 0x10, range.xtal_cap0p, 0x0b)?;

        // Clear tracking filter calibration bits
        self.write_reg_mask(dev, 0x08, 0x00, 0x3f)?;
        self.write_reg_mask(dev, 0x09, 0x00, 0x3f)?;

        Ok(())
    }

    /// Program the PLL synthesizer to produce the given frequency (Hz).
    ///
    /// The R82xx PLL consists of a VCO with programmable integer and
    /// fractional (sigma-delta modulated) dividers. The algorithm:
    ///
    /// 1. Find `mix_div` so that `freq × mix_div` falls in the VCO range
    /// 2. Calculate integer (Ni, Si) and fractional (SDM) components
    /// 3. Write to PLL registers and verify lock
    ///
    /// This implementation matches librtlsdr's r82xx_set_pll:
    /// - Frequency rounding: (freq + 500) / 1000 for kHz conversion in VCO range check
    /// - VCO computed in Hz (u64) for maximum precision
    /// - Exact fixed-point SDM calculation: vco_div = (pll_ref + 65536*vco_freq) / (2*pll_ref)
    fn set_pll(&mut self, dev: &Device, freq: u32) -> Result<()> {
        let pll_ref = self.xtal_freq;
        let freq_khz = (freq + 500) / 1000;

        trace!("set_pll: freq={}kHz, pll_ref={}Hz", freq_khz, pll_ref);

        // Disable refdiv2
        self.write_reg_mask(dev, 0x10, 0x00, 0x10)?;

        // Set PLL auto-tune to 128kHz
        self.write_reg_mask(dev, 0x1a, 0x00, 0x0c)?;

        // Set VCO current (reg 0x12)
        self.write_reg_mask(dev, 0x12, 0x80, 0xe0)?;

        // Find the right mix_div and div_num.
        // mix_div is the actual divider, div_num is its log2 encoding for the register.
        // These are computed independently (matching librtlsdr).
        let vco_min: u32 = 1_770_000;
        let vco_max: u32 = vco_min * 2;
        let mut mix_div: u8 = 2;
        let mut div_num: u8 = 0;

        while mix_div <= 64 {
            if (freq_khz * mix_div as u32) >= vco_min && (freq_khz * mix_div as u32) < vco_max {
                // Calculate div_num from mix_div by counting right-shifts
                let mut div_buf = mix_div;
                while div_buf > 2 {
                    div_buf >>= 1;
                    div_num += 1;
                }
                break;
            }
            mix_div <<= 1;
        }

        if mix_div > 64 {
            warn!("PLL: no valid mix_div found for {}kHz", freq_khz);
            return Err(Error::PllLockFailed {
                freq_hz: freq as u64,
            });
        }

        // Read VCO fine tune from register shadow (5 bytes from reg 0x00)
        let data = self.read_regs(dev, 0x00, 5)?;
        let vco_fine_tune = (data[4] & 0x30) >> 4;

        // VCO power reference differs between R820T and R828D
        let vco_power_ref: u8 = match self.tuner_type {
            TunerType::R820T => 2,
            TunerType::R828D => 1,
        };

        if vco_fine_tune > vco_power_ref {
            div_num = div_num.wrapping_sub(1);
        } else if vco_fine_tune < vco_power_ref {
            div_num = div_num.wrapping_add(1);
        }

        // Write divider (reg 0x10 bits [7:5])
        self.write_reg_mask(dev, 0x10, div_num << 5, 0xe0)?;

        // Compute VCO frequency in Hz (u64) for maximum precision.
        // Use the original mix_div (not recomputed from div_num), matching librtlsdr.
        let vco_freq: u64 = freq as u64 * mix_div as u64;
        trace!("vco_freq: {}", vco_freq);

        // Exact fixed-point PLL divider calculation (matching librtlsdr).
        // vco_div is a 16.16 fixed-point representation of vco_freq / (2 * pll_ref),
        // with rounding: (pll_ref + 65536 * vco_freq) / (2 * pll_ref).
        let vco_div: u64 = (pll_ref as u64 + 65536u64 * vco_freq) / (2 * pll_ref as u64);
        let nint = (vco_div / 65536) as u8;
        let sdm = (vco_div % 65536) as u32;

        trace!("nint: {}, sdm: 0x{:04x}", nint, sdm);

        // Check nint overflow
        if nint > ((128 / vco_power_ref) - 1) {
            warn!("PLL: no valid PLL values for {} Hz", freq);
            return Err(Error::PllLockFailed {
                freq_hz: freq as u64,
            });
        }

        // Encode Ni/Si for register 0x14.
        // nint = 4 * Ni + Si + 13
        let ni = nint.wrapping_sub(13) / 4;
        let si = nint.wrapping_sub(4 * ni).wrapping_sub(13);

        trace!(
            "PLL: mix_div={}, nint={}, ni={}, si={}, reg=0x{:02x}",
            mix_div,
            nint,
            ni,
            si,
            ni.wrapping_add(si << 6),
        );

        self.write_reg_mask(dev, 0x14, ni.wrapping_add(si << 6), 0xff)?;

        // Fractional divider (SDM)
        if sdm == 0 {
            // Disable SDM
            self.write_reg_mask(dev, 0x12, 0x08, 0x08)?;
        } else {
            // Enable SDM
            self.write_reg_mask(dev, 0x12, 0x00, 0x08)?;
        }

        // Always write SDM registers (matching librtlsdr bulk write behavior)
        self.write_reg_mask(dev, 0x16, (sdm >> 8) as u8, 0xff)?;
        self.write_reg_mask(dev, 0x15, (sdm & 0xff) as u8, 0xff)?;

        trace!("PLL SDM: 0x{:04x}", sdm);

        // Check PLL lock (up to 2 attempts).
        // IMPORTANT: R82xx I2C reads always start from register 0x00.
        // We must read 3 bytes (regs 0x00-0x02) and check byte 2 for lock bit.
        // Previous code incorrectly used read_regs(dev, 0x02, 1) which would
        // actually read register 0x00 (since R82xx ignores the start address),
        // causing PLL to never appear locked.
        let mut data = [0u8; 5];
        for attempt in 0..2 {
            let lock_data = self.read_regs(dev, 0x00, 3)?;
            data[0] = lock_data[0];
            data[1] = lock_data[1];
            data[2] = lock_data[2];
            if data[2] & 0x40 != 0 {
                self.has_lock = true;
                trace!("PLL locked on attempt {}", attempt + 1);

                // Set PLL auto-tune to 8kHz
                self.write_reg_mask(dev, 0x1a, 0x08, 0x08)?;
                return Ok(());
            }

            if attempt == 0 {
                // Increase VCO current and retry (matching librtlsdr: 0x60)
                trace!("PLL not locked, increasing VCO current");
                self.write_reg_mask(dev, 0x12, 0x60, 0xe0)?;
            }
        }

        if data[2] & 0x40 == 0 {
            warn!("PLL failed to lock at {} Hz", freq);
            self.has_lock = false;
        }

        // Set PLL auto-tune to 8kHz anyway
        self.write_reg_mask(dev, 0x1a, 0x08, 0x08)?;

        Ok(())
    }

    // ── Input selection (R828D variants) ────────────────────────────────────

    /// RTL-SDR Blog V4 three-input switching.
    ///
    /// Reg 0x06 bit 3: Cable2 enable
    /// Reg 0x05 bits [6:5]: antenna input select
    ///   Cable2 (HF): bit 5 set  = 0x20
    ///   Cable1 (VHF): bits 6+5  = 0x60
    ///   AirIn (UHF): neither    = 0x00
    fn set_blog_v4_input(&mut self, dev: &Device, freq: u32) -> Result<()> {
        if freq <= XTAL_FREQ_28_8 {
            // HF: Cable2 input
            self.write_reg_mask(dev, 0x06, 0x08, 0x08)?; // Cable2 on
            self.write_reg_mask(dev, 0x05, 0x20, 0x60)?; // bit 5 = air_in enable
        } else if freq <= 250_000_000 {
            // VHF: Cable1 input
            self.write_reg_mask(dev, 0x06, 0x00, 0x08)?; // Cable2 off
            self.write_reg_mask(dev, 0x05, 0x60, 0x60)?; // bits 6+5 = cable1 + air_in
        } else {
            // UHF: Air input
            self.write_reg_mask(dev, 0x06, 0x00, 0x08)?; // Cable2 off
            self.write_reg_mask(dev, 0x05, 0x00, 0x60)?; // Neither = air input (default)
        }

        // Notch filter control
        let notch_on = !matches!(freq,
            0..=2_200_000 | 85_000_000..=112_000_000 | 172_000_000..=242_000_000
        );
        self.write_reg_mask(dev, 0x17, if notch_on { 0x08 } else { 0x00 }, 0x08)?;

        Ok(())
    }

    /// Standard R828D two-input switching.
    fn set_r828d_input(&mut self, dev: &Device, freq: u32) -> Result<()> {
        if freq <= 345_000_000 {
            // Cable1 input for low frequencies
            self.write_reg_mask(dev, 0x05, 0x60, 0x60)?;
        } else {
            // Air input for high frequencies
            self.write_reg_mask(dev, 0x05, 0x00, 0x60)?;
        }
        Ok(())
    }

    // ── Gain Control ────────────────────────────────────────────────────────

    /// Enable automatic gain control (LNA AGC + Mixer AGC).
    pub(crate) fn set_gain_auto(&mut self, dev: &Device) -> Result<()> {
        debug!("R82xx gain mode: auto");

        // LNA AGC on (reg 0x05 bit 4 = 0)
        self.write_reg_mask(dev, 0x05, 0x00, 0x10)?;

        // Mixer AGC on (reg 0x07 bit 4 = 1)
        self.write_reg_mask(dev, 0x07, 0x10, 0x10)?;

        // VGA gain = 26.5 dB (reg 0x0c bits [4:0] = 0x0b)
        // Matches librtlsdr. Note: for strong signals (e.g. local FM), the
        // hardware AGC may still clip; applications should use manual gain
        // when they know the signal environment.
        self.write_reg_mask(dev, 0x0c, 0x0b, 0x9f)?;

        Ok(())
    }

    /// Set manual gain to the nearest supported value (in tenths of dB).
    ///
    /// The gain is distributed across LNA and mixer stages by iterating
    /// through alternating gain steps until the total matches or exceeds
    /// the requested value.
    pub(crate) fn set_gain_manual(&mut self, dev: &Device, gain_tenth_db: i32) -> Result<()> {
        debug!("R82xx gain mode: manual {} dB", gain_tenth_db as f32 / 10.0);

        // LNA AGC off (reg 0x05 bit 4 = 1)
        self.write_reg_mask(dev, 0x05, 0x10, 0x10)?;

        // Mixer AGC off (reg 0x07 bit 4 = 0)
        self.write_reg_mask(dev, 0x07, 0x00, 0x10)?;

        // VGA gain = 16.3 dB (reg 0x0c bits [4:0] = 0x08)
        self.write_reg_mask(dev, 0x0c, 0x08, 0x9f)?;

        // Compute LNA and mixer gain indices by stepping through both
        // in each iteration (matching librtlsdr's r82xx_set_gain algorithm).
        let mut total_gain: i32 = 0;
        let mut lna_index: u8 = 0;
        let mut mix_index: u8 = 0;

        for _ in 0..15 {
            if total_gain >= gain_tenth_db {
                break;
            }

            if (lna_index as usize + 1) < LNA_GAIN_STEPS.len() {
                lna_index += 1;
                total_gain += LNA_GAIN_STEPS[lna_index as usize];
            }

            if total_gain >= gain_tenth_db {
                break;
            }

            if (mix_index as usize + 1) < MIXER_GAIN_STEPS.len() {
                mix_index += 1;
                total_gain += MIXER_GAIN_STEPS[mix_index as usize];
            }
        }

        trace!(
            "manual gain: lna_idx={}, mix_idx={}, total={}",
            lna_index, mix_index, total_gain
        );

        // Write LNA gain (reg 0x05 bits [3:0])
        self.write_reg_mask(dev, 0x05, lna_index, 0x0f)?;

        // Write mixer gain (reg 0x07 bits [3:0])
        self.write_reg_mask(dev, 0x07, mix_index, 0x0f)?;

        Ok(())
    }

    // ── Bandwidth Control ───────────────────────────────────────────────────

    /// Set the tuner IF bandwidth and return the actual IF frequency used.
    ///
    /// - bw > 7 MHz: 8 MHz mode (IF = 4.57 MHz)
    /// - bw > 6 MHz: 7 MHz mode (IF = 4.57 MHz)
    /// - bw > ~2.43 MHz: 6 MHz mode (IF = 3.57 MHz)
    /// - bw <= ~2.43 MHz: narrow mode with fine-grained filter control
    ///
    /// Matches librtlsdr's `r82xx_set_bandwidth`, including register masks and filter
    /// calibration code handling.
    pub(crate) fn set_bandwidth(&mut self, dev: &Device, bw: u32) -> Result<u32> {
        let (reg_0a, reg_0b): (u8, u8) = if bw > 7_000_000 {
            // 8 MHz mode
            self.int_freq = 4_570_000;
            (0x10, 0x0b)
        } else if bw > 6_000_000 {
            // 7 MHz mode
            self.int_freq = 4_570_000;
            (0x10, 0x2a)
        } else if bw > (BW_LP_CUTOFFS[0] + HP_BW1 + HP_BW2) {
            // 6 MHz mode (default)
            self.int_freq = R82XX_IF_FREQ;
            (0x10, 0x6b)
        } else {
            // Narrow bandwidth mode — inline matching librtlsdr exactly
            let mut bw_i32 = bw as i32;
            self.int_freq = 2_300_000;
            let reg_0a_n: u8 = 0x00;
            let mut reg_0b_n: u8 = 0x80;
            let mut real_bw: i32 = 0;

            if bw_i32 > (BW_LP_CUTOFFS[0] as i32 + HP_BW1 as i32) {
                bw_i32 -= HP_BW2 as i32;
                self.int_freq += HP_BW2;
                real_bw += HP_BW2 as i32;
            } else {
                reg_0b_n |= 0x20;
            }

            if bw_i32 > BW_LP_CUTOFFS[0] as i32 {
                bw_i32 -= HP_BW1 as i32;
                self.int_freq += HP_BW1;
                real_bw += HP_BW1 as i32;
            } else {
                reg_0b_n |= 0x40;
            }

            // The wanted cutoff is the one before the first that falls below `bw`.
            let mut lp_idx = 0;
            for (i, &cutoff) in BW_LP_CUTOFFS.iter().enumerate() {
                if bw_i32 > cutoff as i32 {
                    break;
                }
                lp_idx = i;
            }

            reg_0b_n |= (15 - lp_idx as u8) & 0x0f;
            real_bw += BW_LP_CUTOFFS[lp_idx] as i32;

            self.int_freq -= (real_bw / 2) as u32;

            (reg_0a_n, reg_0b_n)
        };

        // Mask 0x10 in every mode: narrow mode must leave `fil_cal_code` in [3:0] alone.
        self.write_reg_mask(dev, 0x0a, reg_0a, 0x10)?;
        self.write_reg_mask(dev, 0x0b, reg_0b, 0xef)?;

        debug!("set_bandwidth: bw={}Hz, if_freq={}Hz", bw, self.int_freq);
        Ok(self.int_freq)
    }
}

use std::time::Duration;
