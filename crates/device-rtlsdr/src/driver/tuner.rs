use tracing::{debug, trace, warn};

use super::{
    error::{Error, Result},
    regs::Rtl2832u,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TunerType {
    R820T,
    R828D,
}

pub(crate) const R820T_I2C_ADDR: u8 = 0x34;
pub(crate) const R828D_I2C_ADDR: u8 = 0x74;
pub(crate) const R82XX_CHECK_VAL: u8 = 0x69;

pub(crate) const KNOWN_TUNERS: &[(TunerType, u8)] = &[
    (TunerType::R820T, R820T_I2C_ADDR),
    (TunerType::R828D, R828D_I2C_ADDR),
];

const MAX_I2C_MSG_LEN: usize = 8;

pub(crate) const R82XX_IF_FREQ: u32 = 3_570_000;

const VER_NUM: u8 = 49;

pub(crate) const XTAL_FREQ_28_8: u32 = 28_800_000;
pub(crate) const XTAL_FREQ_16: u32 = 16_000_000;

const NUM_REGS: usize = 27;
const REG_SHADOW_START: u8 = 0x05;

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

pub(crate) const GAIN_VALUES: &[i32] = &[
    0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372,
    386, 402, 421, 434, 439, 445, 480, 496,
];

const LNA_GAIN_STEPS: [i32; 16] = [0, 9, 13, 40, 38, 13, 31, 22, 26, 31, 26, 14, 19, 5, 35, 13];

const MIXER_GAIN_STEPS: [i32; 16] = [0, 5, 10, 10, 19, 9, 10, 25, 17, 10, 8, 16, 13, 6, 3, -8];

struct FreqRange {
    freq_mhz: u32,
    open_d: u8,
    rf_mux_ploy: u8,
    tf_c: u8,
    xtal_cap0p: u8,
}

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

const BW_LP_CUTOFFS: &[u32] = &[
    1_700_000, 1_600_000, 1_550_000, 1_450_000, 1_200_000, 900_000, 700_000, 550_000, 450_000,
    350_000,
];

const HP_BW1: u32 = 350_000;
const HP_BW2: u32 = 380_000;

fn bit_reverse(byte: u8) -> u8 {
    const LUT: [u8; 16] = [
        0x0, 0x8, 0x4, 0xc, 0x2, 0xa, 0x6, 0xe, 0x1, 0x9, 0x5, 0xd, 0x3, 0xb, 0x7, 0xf,
    ];
    (LUT[(byte & 0xf) as usize] << 4) | LUT[(byte >> 4) as usize]
}

#[derive(Clone)]
pub(crate) struct R82xx {
    tuner_type: TunerType,
    i2c_addr: u8,
    regs: [u8; NUM_REGS],
    pub(crate) int_freq: u32,
    xtal_freq: u32,
    is_blog_v4: bool,
    fil_cal_code: u8,
}

impl R82xx {
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
        }
    }

    pub(crate) fn tuner_type(&self) -> TunerType {
        self.tuner_type
    }

    pub(crate) const fn set_xtal_freq(&mut self, freq: u32) {
        self.xtal_freq = freq;
    }

    pub(crate) fn if_freq(&self) -> u32 {
        self.int_freq
    }

    pub(crate) fn gains(&self) -> &[i32] {
        GAIN_VALUES
    }

    fn write_regs(&self, dev: &Rtl2832u, start_reg: u8, count: usize) -> Result<()> {
        let shadow_off = (start_reg - REG_SHADOW_START) as usize;
        let data = &self.regs[shadow_off..shadow_off + count];

        let mut pos = 0;
        while pos < count {
            let chunk_len = (count - pos).min(MAX_I2C_MSG_LEN - 1);
            let mut buf = Vec::with_capacity(chunk_len + 1);
            buf.push(start_reg + pos as u8);
            buf.extend_from_slice(&data[pos..pos + chunk_len]);
            dev.i2c_write(self.i2c_addr, &buf)?;
            pos += chunk_len;
        }

        Ok(())
    }

    fn write_reg_mask(&mut self, dev: &Rtl2832u, reg: u8, val: u8, mask: u8) -> Result<()> {
        let idx = (reg - REG_SHADOW_START) as usize;
        let old = self.regs[idx];
        let new_val = (old & !mask) | (val & mask);
        self.regs[idx] = new_val;

        let buf = [reg, new_val];
        dev.i2c_write(self.i2c_addr, &buf)
    }

    fn read_regs(&self, dev: &Rtl2832u, start_reg: u8, count: u16) -> Result<Vec<u8>> {
        dev.i2c_write(self.i2c_addr, &[start_reg])?;
        let data = dev.i2c_read(self.i2c_addr, count)?;
        if data.len() < usize::from(count) {
            return Err(Error::ShortResponse {
                what: "tuner register read",
                got: data.len(),
            });
        }
        Ok(data.into_iter().map(bit_reverse).collect())
    }

    pub(crate) fn init(&mut self, dev: &Rtl2832u) -> Result<()> {
        debug!(
            "R82xx init: {:?} at 0x{:02x}, xtal={}Hz, blog_v4={}",
            self.tuner_type, self.i2c_addr, self.xtal_freq, self.is_blog_v4
        );

        self.regs = REG_INIT;

        self.write_regs(dev, REG_SHADOW_START, NUM_REGS)?;

        self.set_tv_standard(dev)?;

        self.sysfreq_sel(dev, 0)?;

        debug!(
            "R82xx init complete, fil_cal_code=0x{:02x}",
            self.fil_cal_code
        );
        Ok(())
    }

    pub(crate) fn standby(&mut self, dev: &Rtl2832u) -> Result<()> {
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

    fn set_tv_standard(&mut self, dev: &Rtl2832u) -> Result<()> {
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

        self.regs = REG_INIT;

        self.write_reg_mask(dev, 0x0c, 0x00, 0x0f)?;

        self.write_reg_mask(dev, 0x13, VER_NUM, 0x3f)?;

        self.write_reg_mask(dev, 0x1d, 0x00, 0x38)?;

        self.int_freq = if_khz * 1000;

        for _ in 0..2 {
            self.write_reg_mask(dev, 0x0b, hp_cor, 0x60)?;

            self.write_reg_mask(dev, 0x0f, 0x04, 0x04)?;

            self.write_reg_mask(dev, 0x10, 0x00, 0x03)?;

            self.set_pll(dev, filt_cal_lo * 1000)?;

            self.write_reg_mask(dev, 0x0b, 0x10, 0x10)?;

            std::thread::sleep(Duration::from_millis(2));

            self.write_reg_mask(dev, 0x0b, 0x00, 0x10)?;

            let cal_data = self.read_regs(dev, 0x00, 5)?;
            self.fil_cal_code = cal_data[4] & 0x0f;

            if self.fil_cal_code != 0 && self.fil_cal_code != 0x0f {
                break;
            }
        }

        if self.fil_cal_code == 0x0f {
            self.fil_cal_code = 0;
        }

        trace!("filter cal code: 0x{:02x}", self.fil_cal_code);

        self.write_reg_mask(dev, 0x0f, 0x00, 0x04)?;

        self.write_reg_mask(dev, 0x0a, filt_q | self.fil_cal_code, 0x1f)?;

        self.write_reg_mask(dev, 0x0b, hp_cor, 0xef)?;

        self.write_reg_mask(dev, 0x07, img_r, 0x80)?;

        self.write_reg_mask(dev, 0x06, filt_gain, 0x30)?;

        self.write_reg_mask(dev, 0x1e, ext_enable, 0x60)?;

        self.write_reg_mask(dev, 0x05, loop_through, 0x80)?;

        self.write_reg_mask(dev, 0x1f, lt_att, 0x80)?;

        self.write_reg_mask(dev, 0x0f, flt_ext_widest, 0x80)?;

        self.write_reg_mask(dev, 0x19, polyfil_cur, 0x60)?;

        Ok(())
    }

    fn sysfreq_sel(&mut self, dev: &Rtl2832u, freq: u32) -> Result<()> {
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

        if freq == 506_000_000 || freq == 666_000_000 || freq == 818_000_000 {
            mixer_top = 0x14; // mixer top:14, top-1, low-discharge
            cp_cur = 0x28; // 101, 0.2
            div_buf_cur = 0x20; // 10, 200uA
        }

        self.write_reg_mask(dev, 0x1d, lna_top, 0xc7)?;
        self.write_reg_mask(dev, 0x1c, mixer_top, 0xf8)?;
        self.write_reg_mask(dev, 0x0d, lna_vth_l, 0xff)?;
        self.write_reg_mask(dev, 0x0e, mixer_vth_l, 0xff)?;

        self.write_reg_mask(dev, 0x05, air_cable1_in, 0x60)?;
        self.write_reg_mask(dev, 0x06, cable2_in, 0x08)?;

        self.write_reg_mask(dev, 0x11, cp_cur, 0x38)?;

        self.write_reg_mask(dev, 0x17, div_buf_cur, 0x30)?;
        self.write_reg_mask(dev, 0x0a, filter_cur, 0x60)?;

        self.write_reg_mask(dev, 0x1d, 0, 0x38)?;
        self.write_reg_mask(dev, 0x1c, 0, 0x04)?;
        self.write_reg_mask(dev, 0x06, 0, 0x40)?;
        self.write_reg_mask(dev, 0x1a, 0x30, 0x30)?;

        self.write_reg_mask(dev, 0x1d, 0x18, 0x38)?;

        self.write_reg_mask(dev, 0x1c, mixer_top, 0x04)?;
        self.write_reg_mask(dev, 0x1e, lna_discharge, 0x1f)?;
        self.write_reg_mask(dev, 0x1a, 0x20, 0x30)?;

        Ok(())
    }

    pub(crate) fn set_freq(&mut self, dev: &Rtl2832u, freq: u32) -> Result<()> {
        debug!("R82xx set_freq: {} Hz", freq);

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

        let lo_freq = upconverted_freq.saturating_add(self.int_freq);

        self.set_mux(dev, lo_freq)?;

        self.set_pll(dev, lo_freq)?;

        if self.tuner_type == TunerType::R828D {
            if self.is_blog_v4 {
                self.set_blog_v4_input(dev, freq)?;
            } else {
                self.set_r828d_input(dev, freq)?;
            }
        }

        Ok(())
    }

    fn set_mux(&mut self, dev: &Rtl2832u, lo_freq: u32) -> Result<()> {
        let freq_mhz = lo_freq / 1_000_000;

        let range = FREQ_RANGES
            .iter()
            .rev()
            .find(|r| freq_mhz >= r.freq_mhz)
            .unwrap_or(&FREQ_RANGES[0]);

        self.write_reg_mask(dev, 0x17, range.open_d, 0x08)?;

        self.write_reg_mask(dev, 0x1a, range.rf_mux_ploy, 0xc3)?;

        self.write_reg_mask(dev, 0x1b, range.tf_c, 0xff)?;

        self.write_reg_mask(dev, 0x10, range.xtal_cap0p, 0x0b)?;

        self.write_reg_mask(dev, 0x08, 0x00, 0x3f)?;
        self.write_reg_mask(dev, 0x09, 0x00, 0x3f)?;

        Ok(())
    }

    fn set_pll(&mut self, dev: &Rtl2832u, freq: u32) -> Result<()> {
        let pll_ref = self.xtal_freq;
        let freq_khz = (freq + 500) / 1000;

        trace!("set_pll: freq={}kHz, pll_ref={}Hz", freq_khz, pll_ref);

        self.write_reg_mask(dev, 0x10, 0x00, 0x10)?;

        self.write_reg_mask(dev, 0x1a, 0x00, 0x0c)?;

        self.write_reg_mask(dev, 0x12, 0x80, 0xe0)?;

        let vco_min: u32 = 1_770_000;
        let vco_max: u32 = vco_min * 2;
        let mut mix_div: u8 = 2;
        let mut div_num: u8 = 0;

        while mix_div <= 64 {
            if (freq_khz * mix_div as u32) >= vco_min && (freq_khz * mix_div as u32) < vco_max {
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

        let data = self.read_regs(dev, 0x00, 5)?;
        let vco_fine_tune = (data[4] & 0x30) >> 4;

        let vco_power_ref: u8 = match self.tuner_type {
            TunerType::R820T => 2,
            TunerType::R828D => 1,
        };

        if vco_fine_tune > vco_power_ref {
            div_num = div_num.wrapping_sub(1);
        } else if vco_fine_tune < vco_power_ref {
            div_num = div_num.wrapping_add(1);
        }

        self.write_reg_mask(dev, 0x10, div_num << 5, 0xe0)?;

        let vco_freq: u64 = freq as u64 * mix_div as u64;
        trace!("vco_freq: {}", vco_freq);

        let vco_div: u64 = (pll_ref as u64 + 65536u64 * vco_freq) / (2 * pll_ref as u64);
        let nint = (vco_div / 65536) as u8;
        let sdm = (vco_div % 65536) as u32;

        trace!("nint: {}, sdm: 0x{:04x}", nint, sdm);

        if nint > ((128 / vco_power_ref) - 1) {
            warn!("PLL: no valid PLL values for {} Hz", freq);
            return Err(Error::PllLockFailed {
                freq_hz: freq as u64,
            });
        }

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

        if sdm == 0 {
            self.write_reg_mask(dev, 0x12, 0x08, 0x08)?;
        } else {
            self.write_reg_mask(dev, 0x12, 0x00, 0x08)?;
        }

        self.write_reg_mask(dev, 0x16, (sdm >> 8) as u8, 0xff)?;
        self.write_reg_mask(dev, 0x15, (sdm & 0xff) as u8, 0xff)?;

        trace!("PLL SDM: 0x{:04x}", sdm);

        for attempt in 0..2 {
            let lock_data = self.read_regs(dev, 0x00, 3)?;
            if lock_data[2] & 0x40 != 0 {
                trace!("PLL locked on attempt {}", attempt + 1);
                self.write_reg_mask(dev, 0x1a, 0x08, 0x08)?;
                return Ok(());
            }

            if attempt == 0 {
                trace!("PLL not locked, increasing VCO current");
                self.write_reg_mask(dev, 0x12, 0x60, 0xe0)?;
            }
        }

        self.write_reg_mask(dev, 0x1a, 0x08, 0x08)?;
        warn!("PLL failed to lock at {} Hz", freq);
        Err(Error::PllLockFailed {
            freq_hz: u64::from(freq),
        })
    }

    fn set_blog_v4_input(&mut self, dev: &Rtl2832u, freq: u32) -> Result<()> {
        if freq <= XTAL_FREQ_28_8 {
            self.write_reg_mask(dev, 0x06, 0x08, 0x08)?; // Cable2 on
            self.write_reg_mask(dev, 0x05, 0x20, 0x60)?; // bit 5 = air_in enable
        } else if freq <= 250_000_000 {
            self.write_reg_mask(dev, 0x06, 0x00, 0x08)?; // Cable2 off
            self.write_reg_mask(dev, 0x05, 0x60, 0x60)?; // bits 6+5 = cable1 + air_in
        } else {
            self.write_reg_mask(dev, 0x06, 0x00, 0x08)?; // Cable2 off
            self.write_reg_mask(dev, 0x05, 0x00, 0x60)?; // Neither = air input (default)
        }

        let notch_on = !matches!(freq,
            0..=2_200_000 | 85_000_000..=112_000_000 | 172_000_000..=242_000_000
        );
        self.write_reg_mask(dev, 0x17, if notch_on { 0x08 } else { 0x00 }, 0x08)?;

        Ok(())
    }

    fn set_r828d_input(&mut self, dev: &Rtl2832u, freq: u32) -> Result<()> {
        if freq <= 345_000_000 {
            self.write_reg_mask(dev, 0x05, 0x60, 0x60)?;
        } else {
            self.write_reg_mask(dev, 0x05, 0x00, 0x60)?;
        }
        Ok(())
    }

    pub(crate) fn set_gain_auto(&mut self, dev: &Rtl2832u) -> Result<()> {
        debug!("R82xx gain mode: auto");

        self.write_reg_mask(dev, 0x05, 0x00, 0x10)?;

        self.write_reg_mask(dev, 0x07, 0x10, 0x10)?;

        self.write_reg_mask(dev, 0x0c, 0x0b, 0x9f)?;

        Ok(())
    }

    pub(crate) fn set_gain_manual(&mut self, dev: &Rtl2832u, gain_tenth_db: i32) -> Result<()> {
        debug!("R82xx gain mode: manual {} dB", gain_tenth_db as f32 / 10.0);

        self.write_reg_mask(dev, 0x05, 0x10, 0x10)?;

        self.write_reg_mask(dev, 0x07, 0x00, 0x10)?;

        self.write_reg_mask(dev, 0x0c, 0x08, 0x9f)?;

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

        self.write_reg_mask(dev, 0x05, lna_index, 0x0f)?;

        self.write_reg_mask(dev, 0x07, mix_index, 0x0f)?;

        Ok(())
    }

    pub(crate) fn set_bandwidth(&mut self, dev: &Rtl2832u, bw: u32) -> Result<u32> {
        let (reg_0a, reg_0b): (u8, u8) = if bw > 7_000_000 {
            self.int_freq = 4_570_000;
            (0x10, 0x0b)
        } else if bw > 6_000_000 {
            self.int_freq = 4_570_000;
            (0x10, 0x2a)
        } else if bw > (BW_LP_CUTOFFS[0] + HP_BW1 + HP_BW2) {
            self.int_freq = R82XX_IF_FREQ;
            (0x10, 0x6b)
        } else {
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

        self.write_reg_mask(dev, 0x0a, reg_0a, 0x10)?;
        self.write_reg_mask(dev, 0x0b, reg_0b, 0xef)?;

        debug!("set_bandwidth: bw={}Hz, if_freq={}Hz", bw, self.int_freq);
        Ok(self.int_freq)
    }
}

use std::time::Duration;
