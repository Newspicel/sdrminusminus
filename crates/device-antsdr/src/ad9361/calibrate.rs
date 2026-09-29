use std::time::Duration;

use sdrmm_device::DeviceError;

use super::{
    Ad9361, Bus, Direction, GainMode, MAX_FREQUENCY, State,
    analog::{self, FilterTrim},
    fir, reg, setup,
    tables::{self, Band},
};

const FIR_SLOTS: usize = 128;
const RX_FIR_DC_GAIN: f64 = 2.0;
const RX_FIR_MINUS_6_DB: u8 = 0x02;

#[derive(Clone, Copy)]
struct Poll {
    register: u16,
    mask: u8,
    until_set: bool,
    tries: u32,
    pause: Duration,
    what: &'static str,
}

const fn clears(register: u16, mask: u8, tries: u32, pause_ms: u64, what: &'static str) -> Poll {
    Poll {
        register,
        mask,
        until_set: false,
        tries,
        pause: Duration::from_millis(pause_ms),
        what,
    }
}

const fn sets(register: u16, mask: u8, tries: u32, pause_ms: u64, what: &'static str) -> Poll {
    Poll {
        until_set: true,
        ..clears(register, mask, tries, pause_ms, what)
    }
}

impl<B: Bus> Ad9361<B> {
    pub(super) fn calibrate_all(&mut self) -> Result<(), DeviceError> {
        self.write(reg::ENSM_CONFIG_2, setup::DUAL_SYNTHESIZER)?;
        self.write(reg::ENSM_CONFIG_1, setup::ENSM_ALERT_TX_ON)?;
        self.write(reg::ENSM_MODE, setup::ENSM_ENABLE)?;
        std::thread::sleep(Duration::from_millis(1));
        self.calibrate_charge_pumps()?;
        let rx = self.target(Direction::Rx);
        let tx = self.target(Direction::Tx);
        self.tune_synthesizer(Direction::Rx, rx)?;
        self.tune_synthesizer(Direction::Tx, tx)?;
        self.load_mixer_gm()?;
        self.load_gain_table(false)?;
        self.write_all(setup::MANUAL_GAIN)?;
        self.reapply_gains()?;
        self.calibrate_filters()?;
        self.setup_adc()?;
        self.calibrate_bb_dc()?;
        self.calibrate_rf_dc()?;
        self.calibrate_rx_quadrature()?;
        self.apply_dc_tracking()?;
        self.apply_quadrature_tracking()?;
        self.calibrated_rx_hz = self.rx_hz;
        self.calibrated_tx_hz = self.tx_hz;
        self.write(reg::PARALLEL_PORT_3, setup::PARALLEL_PORT_READY)?;
        self.write(reg::ENSM_MODE, setup::ENSM_ENABLE)?;
        self.write(reg::ENSM_CONFIG_2, setup::DUAL_SYNTHESIZER)?;
        for chain in 0..2 {
            let mode = self.gain_mode[chain];
            if mode != GainMode::Manual {
                self.set_gain_mode(chain, mode)?;
            }
        }
        Ok(())
    }

    fn target(&self, direction: Direction) -> f64 {
        let (requested, actual) = match direction {
            Direction::Rx => (self.requested_rx_hz, self.rx_hz),
            Direction::Tx => (self.requested_tx_hz, self.tx_hz),
        };
        if requested > 0.0 { requested } else { actual }
    }

    fn poll(&mut self, poll: Poll) -> Result<(), DeviceError> {
        for attempt in 0..=poll.tries {
            let set = self.read(poll.register)? & poll.mask != 0;
            if set == poll.until_set {
                return Ok(());
            }
            if attempt < poll.tries {
                std::thread::sleep(poll.pause);
            }
        }
        Err(DeviceError::Io(format!(
            "the AD9361 {} did not finish",
            poll.what
        )))
    }

    fn run_calibration(&mut self, bit: u8, poll: Poll) -> Result<(), DeviceError> {
        self.write(reg::CALIBRATION, bit)?;
        self.poll(Poll {
            register: reg::CALIBRATION,
            mask: bit,
            ..poll
        })
    }

    pub(super) fn lock_baseband_pll(&mut self) -> Result<(), DeviceError> {
        self.write(reg::BBPLL_CALIBRATION, 0x05)?;
        self.write(reg::BBPLL_CALIBRATION, 0x01)?;
        self.write(reg::BBPLL_LOOP_GAIN, 0x86)?;
        self.write(reg::BBPLL_LOOP_CONTROL, 0x01)?;
        self.write(reg::BBPLL_LOOP_CONTROL, 0x05)?;
        self.poll(sets(reg::BBPLL_STATUS, 0x80, 1000, 2, "converter clock"))
    }

    fn calibrate_charge_pumps(&mut self) -> Result<(), DeviceError> {
        self.require_alert("charge pump calibration")?;
        for (control, status, what) in [
            (
                reg::RX_CHARGE_PUMP_CALIBRATION,
                reg::RX_CHARGE_PUMP_STATUS,
                "receive charge pump calibration",
            ),
            (
                reg::TX_CHARGE_PUMP_CALIBRATION,
                reg::TX_CHARGE_PUMP_STATUS,
                "transmit charge pump calibration",
            ),
        ] {
            self.write(control, 0x04)?;
            self.poll(sets(status, 0x80, 5, 1, what))?;
            self.write(control, 0x00)?;
        }
        Ok(())
    }

    fn require_alert(&mut self, what: &str) -> Result<(), DeviceError> {
        match self.state()? {
            State::Alert => Ok(()),
            other => Err(DeviceError::Io(format!(
                "the AD9361 must be in alert for {what}, it is in {other:?}"
            ))),
        }
    }

    pub(super) fn load_gain_table(&mut self, force: bool) -> Result<(), DeviceError> {
        let band = Band::of(self.rx_hz);
        if !force && self.band == Some(band) {
            return Ok(());
        }
        self.write(reg::GAIN_TABLE_CONFIG, 0x1a)?;
        for (index, row) in tables::gain_table(band).iter().enumerate() {
            self.write(reg::GAIN_TABLE_ADDRESS, index as u8)?;
            self.write(reg::GAIN_TABLE_WORD_1, row[0])?;
            self.write(reg::GAIN_TABLE_WORD_2, row[1])?;
            self.write(reg::GAIN_TABLE_WORD_3, row[2])?;
            self.write(reg::GAIN_TABLE_CONFIG, 0x1e)?;
            self.write(reg::GAIN_TABLE_READ, 0x00)?;
            self.write(reg::GAIN_TABLE_READ, 0x00)?;
        }
        self.write(reg::GAIN_TABLE_CONFIG, 0x1a)?;
        self.write(reg::GAIN_TABLE_READ, 0x00)?;
        self.write(reg::GAIN_TABLE_READ, 0x00)?;
        self.write(reg::GAIN_TABLE_CONFIG, 0x00)?;
        self.band = Some(band);
        Ok(())
    }

    fn load_mixer_gm(&mut self) -> Result<(), DeviceError> {
        self.write(reg::MIXER_GM_CONFIG, 0x02)?;
        for (step, (gain, transconductance)) in tables::MIXER_GM.iter().enumerate() {
            self.write(reg::MIXER_GM_ADDRESS, 15 - step as u8)?;
            self.write(reg::MIXER_GM_GAIN, *gain)?;
            self.write(reg::MIXER_GM_BIAS, 0x00)?;
            self.write(reg::MIXER_GM_TRANSCONDUCTANCE, *transconductance)?;
            self.write(reg::MIXER_GM_CONFIG, 0x06)?;
            self.write(reg::MIXER_GM_READ, 0x00)?;
            self.write(reg::MIXER_GM_READ, 0x00)?;
        }
        self.write(reg::MIXER_GM_CONFIG, 0x02)?;
        self.write(reg::MIXER_GM_READ, 0x00)?;
        self.write(reg::MIXER_GM_READ, 0x00)?;
        self.write(reg::MIXER_GM_CONFIG, 0x00)
    }

    pub(super) fn load_fir(
        &mut self,
        direction: Direction,
        taps: usize,
        factor: u32,
    ) -> Result<(), DeviceError> {
        let (base, dc_gain) = match direction {
            Direction::Rx => (reg::RX_FIR, RX_FIR_DC_GAIN),
            Direction::Tx => (reg::TX_FIR, f64::from(factor)),
        };
        let coefficients = fir::design(taps, factor, dc_gain);
        let config = ((taps / 16 - 1) as u8 & 0x07) << 5 | 0x03 << 3;
        self.write(base + 5, config | 0x02)?;
        std::thread::sleep(Duration::from_millis(1));
        for slot in 0..FIR_SLOTS {
            let tap = coefficients.get(slot).copied().unwrap_or(0) as u16;
            self.write(base, slot as u8)?;
            self.write(base + 1, tap as u8)?;
            self.write(base + 2, (tap >> 8) as u8)?;
            self.write(base + 5, config | 0x06)?;
            self.write(base + 4, 0x00)?;
            self.write(base + 4, 0x00)?;
        }
        self.write(base + 5, config | 0x02)?;
        self.write(base + 5, config)?;
        if direction == Direction::Rx {
            self.write(base + 6, RX_FIR_MINUS_6_DB)?;
        }
        Ok(())
    }

    pub(super) fn calibrate_filters(&mut self) -> Result<(), DeviceError> {
        self.calibrate_rx_baseband()?;
        self.calibrate_rx_tia()?;
        self.calibrate_tx_baseband()?;
        self.calibrate_tx_secondary()
    }

    fn calibrate_rx_baseband(&mut self) -> Result<(), DeviceError> {
        let filter = analog::rx_baseband(
            self.bandwidth,
            self.clocks.baseband_hz,
            self.clocks.bbpll_hz,
        );
        self.clocks.rx_tune_divider = filter.tune_divider;
        self.shadow.rx_tune_config =
            self.shadow.rx_tune_config & 0xfe | (filter.tune_divider >> 8) as u8 & 0x01;
        self.write(reg::RX_BB_MHZ, filter.mhz)?;
        self.write(reg::RX_BB_KHZ, filter.khz_steps)?;
        self.write(reg::RX_BB_TUNE_DIVIDER, filter.tune_divider as u8)?;
        self.write(reg::RX_BB_TUNE_CONFIG, self.shadow.rx_tune_config)?;
        self.write(0x1d5, 0x3f)?;
        self.write(0x1c0, 0x03)?;
        self.write(reg::RX1_BB_TUNER, 0x02)?;
        self.write(reg::RX2_BB_TUNER, 0x02)?;
        self.run_calibration(0x80, clears(0, 0, 100, 1, "receive filter calibration"))?;
        self.write(reg::RX1_BB_TUNER, 0x03)?;
        self.write(reg::RX2_BB_TUNER, 0x03)?;
        self.rx_baseband_bandwidth = filter.bandwidth;
        Ok(())
    }

    fn calibrate_tx_baseband(&mut self) -> Result<(), DeviceError> {
        let filter = analog::tx_baseband(
            self.bandwidth,
            self.clocks.baseband_hz,
            self.clocks.bbpll_hz,
        );
        self.shadow.tx_tune_config =
            self.shadow.tx_tune_config & 0xfe | (filter.tune_divider >> 8) as u8 & 0x01;
        self.write(reg::TX_BB_TUNE_DIVIDER, filter.tune_divider as u8)?;
        self.write(reg::TX_BB_TUNE_MODE, self.shadow.tx_tune_config)?;
        self.write(reg::TX_BB_TUNER, 0x22)?;
        self.run_calibration(0x40, clears(0, 0, 100, 1, "transmit filter calibration"))?;
        self.write(reg::TX_BB_TUNER, 0x26)
    }

    fn calibrate_tx_secondary(&mut self) -> Result<(), DeviceError> {
        let (_, filter) = analog::tx_secondary(self.bandwidth, self.clocks.baseband_hz);
        self.write(reg::TX_SECONDARY_CAPACITOR, filter.capacitor)?;
        self.write(reg::TX_SECONDARY_RESISTOR, filter.resistor_code)?;
        self.write(reg::TX_SECONDARY_BANDWIDTH, filter.bandwidth_code)
    }

    fn filter_trim(&mut self) -> Result<FilterTrim, DeviceError> {
        Ok(FilterTrim {
            c3_msb: self.read(reg::RX_BB_C3_MSB)? & 0x3f,
            c3_lsb: self.read(reg::RX_BB_C3_LSB)? & 0x7f,
            r2346: self.read(reg::RX_BB_R2346)? & 0x07,
        })
    }

    fn calibrate_rx_tia(&mut self) -> Result<(), DeviceError> {
        let trim = self.filter_trim()?;
        let (_, tia) = analog::rx_tia(self.bandwidth, self.clocks.baseband_hz, trim);
        self.write(reg::RX_TIA_BANDWIDTH, tia.bandwidth_code)?;
        self.write(reg::RX_TIA_1_MSB, tia.c1_msb)?;
        self.write(reg::RX_TIA_2_MSB, tia.c2_msb)?;
        self.write(reg::RX_TIA_1, tia.c1)?;
        self.write(reg::RX_TIA_2, tia.c2)
    }

    fn setup_adc(&mut self) -> Result<(), DeviceError> {
        let trim = self.filter_trim()?;
        let config = analog::adc_config(
            self.clocks.bbpll_hz,
            self.clocks.rx_tune_divider,
            self.clocks.adc_hz,
            trim,
        );
        for (offset, value) in config.iter().enumerate() {
            self.write(reg::ADC_CONFIG + offset as u16, *value)?;
        }
        Ok(())
    }

    fn calibrate_bb_dc(&mut self) -> Result<(), DeviceError> {
        self.write_all(setup::BB_DC_CALIBRATION)?;
        self.run_calibration(0x01, clears(0, 0, 100, 5, "baseband DC calibration"))
    }

    pub(super) fn calibrate_rf_dc(&mut self) -> Result<(), DeviceError> {
        let (count, settle, decimation) = if self.rx_hz < 4e9 {
            (0x32, 0x24, 0x05)
        } else {
            (0x28, 0x34, 0x06)
        };
        self.write(reg::RF_DC_COUNT, count)?;
        self.write(reg::RF_DC_SETTLE, settle)?;
        self.write(reg::RF_DC_DECIMATION, decimation)?;
        self.write(reg::RF_DC_WAIT, 0x20)?;
        self.write(reg::DC_TRACKING, 0x83)?;
        self.write(reg::RF_DC_CONFIG, 0x30)?;
        self.run_calibration(0x02, clears(0, 0, 200, 50, "RF DC calibration"))?;
        self.write(reg::DC_TRACKING, 0x8d)
    }

    pub(super) fn apply_dc_tracking(&mut self) -> Result<(), DeviceError> {
        self.write(reg::DC_TRACKING, if self.tracking.dc { 0xad } else { 0x8d })
    }

    pub(super) fn apply_quadrature_tracking(&mut self) -> Result<(), DeviceError> {
        self.write(
            reg::RX_QUAD_TRACKING,
            if self.tracking.quadrature { 0xcf } else { 0xc0 },
        )
    }

    pub(super) fn calibrate_rx_quadrature(&mut self) -> Result<(), DeviceError> {
        self.write_all(setup::RX_QUADRATURE_CALIBRATION)?;
        let transmit = self.target(Direction::Tx);
        let tone = (self.rx_hz + self.rx_baseband_bandwidth / 2.0).min(MAX_FREQUENCY);
        self.tune_synthesizer(Direction::Tx, tone)?;
        let calibrated = self.run_calibration(
            0x20,
            clears(0, 0, 1000, 5, "receive quadrature calibration"),
        );
        self.write(reg::TX_MIXER_POWER, 0x30)?;
        self.tune_synthesizer(Direction::Tx, transmit)?;
        calibrated
    }

    pub(super) fn calibrate_tx_quadrature(&mut self) -> Result<(), DeviceError> {
        self.require_alert("transmit quadrature calibration")?;
        self.write(reg::RX_QUAD_TRACKING, 0xc0)?;
        let input = self.shadow.input;
        for side in [input & 0xbf, input | 0x40] {
            self.write(reg::INPUT_SELECT, side)?;
            self.tx_quadrature_pass()?;
        }
        self.write(reg::INPUT_SELECT, input)?;
        self.apply_quadrature_tracking()
    }

    fn tx_quadrature_pass(&mut self) -> Result<(), DeviceError> {
        let nco = self.read(reg::TX_QUAD_STATUS)? & 0xc0;
        self.write(reg::TX_QUAD_NCO, 0x15 | nco >> 1)?;
        let status = self.read(reg::TX_QUAD_STATUS)?;
        self.write(reg::TX_QUAD_STATUS, status & 0x3f | nco)?;
        let baseband = self.clocks.baseband_hz;
        let tone = baseband * f64::from(self.clocks.tx_fir_factor) * f64::from((nco >> 6) + 1)
            / 32.0
            * 2.0;
        let reach = (baseband / 2.0).clamp(0.2e6, 28e6);
        if tone > reach {
            tracing::warn!(
                tone,
                reach,
                "transmit quadrature tone outside the receive filter"
            );
            return Ok(());
        }
        self.write_all(setup::TX_QUADRATURE_CALIBRATION)?;
        self.write(0x0aa, if self.rx_hz < 1300e6 { 0x22 } else { 0x25 })?;
        self.write_all(setup::TX_QUADRATURE_FINISH)?;
        self.run_calibration(
            0x10,
            clears(0, 0, 100, 10, "transmit quadrature calibration"),
        )
    }
}
