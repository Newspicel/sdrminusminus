use std::time::Duration;

use sdrmm_device::DeviceError;

use self::tables::Band;

mod analog;
mod calibrate;
mod fir;
mod reg;
mod setup;
mod synth;
mod tables;

pub(crate) const MAX_RATE: f64 = 61.44e6;
pub(crate) const MIN_RATE: f64 = 220e3;
pub(crate) const MIN_BANDWIDTH: f64 = 200e3;
pub(crate) const MAX_BANDWIDTH: f64 = 56e6;
pub(crate) const MAX_RX_GAIN: f64 = 76.0;
pub(crate) const MAX_TX_ATTENUATION: f64 = 89.75;
pub(crate) const MIN_FREQUENCY: f64 = 70e6;
pub(crate) const MAX_FREQUENCY: f64 = 6e9;
pub(crate) const REFERENCE_HZ: f64 = 40e6;
const RECALIBRATE_AFTER_HZ: f64 = 100e6;
const SAME_HZ: f64 = 1.0;

pub(crate) trait Bus {
    fn write(&mut self, register: u16, value: u8) -> Result<(), DeviceError>;
    fn read(&mut self, register: u16) -> Result<u8, DeviceError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Rx,
    Tx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GainMode {
    Manual,
    SlowAttack,
    FastAttack,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chains {
    pub(crate) rx: [bool; 2],
    pub(crate) tx: [bool; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tracking {
    pub(crate) quadrature: bool,
    pub(crate) dc: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Wait,
    Alert,
    Duplex,
    Flushing,
    Other(u8),
}

impl State {
    const fn of(code: u8) -> Self {
        match code & 0x0f {
            0x0 => Self::Wait,
            0x5 => Self::Alert,
            0xa => Self::Duplex,
            0xb => Self::Flushing,
            other => Self::Other(other),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Shadow {
    dividers: u8,
    input: u8,
    rx_filters: u8,
    tx_filters: u8,
    bbpll: u8,
    rx_tune_config: u8,
    tx_tune_config: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct Clocks {
    requested: f64,
    bbpll_hz: f64,
    adc_hz: f64,
    baseband_hz: f64,
    rx_fir_factor: u32,
    tx_fir_factor: u32,
    rx_tune_divider: u16,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Settings {
    pub(crate) rate: f64,
    pub(crate) rx_hz: f64,
    pub(crate) tx_hz: f64,
    pub(crate) bandwidth: f64,
}

pub(crate) struct Ad9361<B: Bus> {
    bus: B,
    reference_hz: f64,
    shadow: Shadow,
    clocks: Clocks,
    rx_hz: f64,
    tx_hz: f64,
    requested_rx_hz: f64,
    requested_tx_hz: f64,
    calibrated_rx_hz: f64,
    calibrated_tx_hz: f64,
    band: Option<Band>,
    bandwidth: f64,
    rx_baseband_bandwidth: f64,
    rx_gain: [f64; 2],
    tx_attenuation: [f64; 2],
    gain_mode: [GainMode; 2],
    tracking: Tracking,
}

impl<B: Bus> Ad9361<B> {
    pub(crate) fn new(bus: B, reference_hz: f64) -> Self {
        Self {
            bus,
            reference_hz,
            shadow: Shadow {
                input: 0x30,
                bbpll: 0x02,
                rx_tune_config: 0x1e,
                tx_tune_config: 0x1e,
                ..Shadow::default()
            },
            clocks: Clocks::default(),
            rx_hz: 800e6,
            tx_hz: 850e6,
            requested_rx_hz: 0.0,
            requested_tx_hz: 0.0,
            calibrated_rx_hz: 0.0,
            calibrated_tx_hz: 0.0,
            band: None,
            bandwidth: MAX_BANDWIDTH,
            rx_baseband_bandwidth: 0.0,
            rx_gain: [0.0; 2],
            tx_attenuation: [MAX_TX_ATTENUATION; 2],
            gain_mode: [GainMode::Manual; 2],
            tracking: Tracking {
                quadrature: true,
                dc: true,
            },
        }
    }

    #[cfg(test)]
    pub(crate) const fn bus(&mut self) -> &mut B {
        &mut self.bus
    }

    #[cfg(test)]
    pub(crate) const fn rate(&self) -> f64 {
        self.clocks.baseband_hz
    }

    pub(crate) const fn frequency(&self, direction: Direction) -> f64 {
        match direction {
            Direction::Rx => self.rx_hz,
            Direction::Tx => self.tx_hz,
        }
    }

    pub(crate) fn initialize(&mut self, settings: Settings) -> Result<(), DeviceError> {
        self.rx_hz = settings.rx_hz;
        self.tx_hz = settings.tx_hz;
        self.bandwidth = settings.bandwidth;
        self.reset()?;
        self.power_up()?;
        self.setup_rates(settings.rate)?;
        self.setup_interface()?;
        self.setup_auxiliaries()?;
        self.setup_synthesizers()?;
        self.calibrate_all()?;
        self.write_all(setup::RSSI)?;
        self.set_chains(Chains {
            rx: [true, false],
            tx: [true, false],
        })?;
        self.write(reg::ENSM_CONFIG_1, setup::ENSM_DUPLEX)
    }

    pub(crate) fn set_rate(&mut self, rate: f64) -> Result<f64, DeviceError> {
        if !(MIN_RATE..=MAX_RATE).contains(&rate) {
            return Err(DeviceError::Unsupported(format!(
                "the AD9361 converts at {MIN_RATE} to {MAX_RATE} Hz, not {rate}"
            )));
        }
        if (rate - self.clocks.requested).abs() < SAME_HZ {
            return Ok(self.clocks.baseband_hz);
        }
        let entered = self.state()?;
        match entered {
            State::Alert => {
                self.write(reg::ENSM_CONFIG_1, setup::ENSM_DUPLEX)?;
                std::thread::sleep(Duration::from_millis(5));
                self.write(reg::ENSM_CONFIG_1, setup::ENSM_WAIT)?;
            }
            State::Duplex => self.write(reg::ENSM_CONFIG_1, setup::ENSM_WAIT)?,
            other => {
                return Err(DeviceError::Io(format!(
                    "the AD9361 is in state {other:?} and cannot change rate"
                )));
            }
        }
        let rx_chains = self.shadow.rx_filters & 0xc0;
        let tx_chains = self.shadow.tx_filters & 0xc0;
        self.setup_rates(rate)?;
        self.calibrate_all()?;
        if entered == State::Duplex {
            self.shadow.rx_filters = self.shadow.rx_filters & 0x3f | rx_chains;
            self.shadow.tx_filters = self.shadow.tx_filters & 0x3f | tx_chains;
            self.write(reg::TX_FILTERS, self.shadow.tx_filters)?;
            self.write(reg::RX_FILTERS, self.shadow.rx_filters)?;
            self.write(reg::ENSM_CONFIG_1, setup::ENSM_DUPLEX)?;
        }
        Ok(self.clocks.baseband_hz)
    }

    pub(crate) fn tune(&mut self, direction: Direction, hz: f64) -> Result<f64, DeviceError> {
        if !(MIN_FREQUENCY..=MAX_FREQUENCY).contains(&hz) {
            return Err(DeviceError::Unsupported(format!(
                "the AD9361 tunes {MIN_FREQUENCY} to {MAX_FREQUENCY} Hz, not {hz}"
            )));
        }
        let requested = match direction {
            Direction::Rx => self.requested_rx_hz,
            Direction::Tx => self.requested_tx_hz,
        };
        if (hz - requested).abs() < SAME_HZ {
            return Ok(self.frequency(direction));
        }
        let leave_alert = self.state()? != State::Alert;
        if leave_alert {
            self.write(reg::ENSM_CONFIG_1, setup::ENSM_ALERT)?;
        }
        let tuned = self.tune_synthesizer(direction, hz)?;
        if direction == Direction::Rx {
            self.load_gain_table(false)?;
        }
        self.reapply_gains()?;
        let calibrated = match direction {
            Direction::Rx => self.calibrated_rx_hz,
            Direction::Tx => self.calibrated_tx_hz,
        };
        if (calibrated - tuned).abs() > RECALIBRATE_AFTER_HZ {
            match direction {
                Direction::Rx => {
                    self.calibrate_rf_dc()?;
                    if !self.tracking.quadrature {
                        self.calibrate_rx_quadrature()?;
                    }
                    self.apply_dc_tracking()?;
                    self.calibrated_rx_hz = tuned;
                }
                Direction::Tx => {
                    self.calibrate_tx_quadrature()?;
                    self.calibrated_tx_hz = tuned;
                }
            }
            self.apply_quadrature_tracking()?;
        }
        if leave_alert {
            self.write(reg::ENSM_CONFIG_1, setup::ENSM_DUPLEX)?;
        }
        Ok(tuned)
    }

    pub(crate) fn set_rx_gain(&mut self, chain: usize, db: f64) -> Result<f64, DeviceError> {
        let index = db.clamp(0.0, MAX_RX_GAIN).floor();
        self.rx_gain[chain.min(1)] = index;
        let register = if chain == 0 {
            reg::RX1_MANUAL_GAIN
        } else {
            reg::RX2_MANUAL_GAIN
        };
        self.write(register, index as u8)?;
        Ok(index)
    }

    pub(crate) fn set_tx_attenuation(&mut self, chain: usize, db: f64) -> Result<f64, DeviceError> {
        let steps = (db.clamp(0.0, MAX_TX_ATTENUATION) * 4.0).round() as u16;
        self.tx_attenuation[chain.min(1)] = f64::from(steps) / 4.0;
        self.write(reg::TX1_ATTENUATION_UPDATE, 0x40)?;
        self.write(reg::TX2_ATTENUATION_UPDATE, 0x40)?;
        let (low, high) = if chain == 0 {
            (reg::TX1_ATTENUATION_LOW, reg::TX1_ATTENUATION_HIGH)
        } else {
            (reg::TX2_ATTENUATION_LOW, reg::TX2_ATTENUATION_HIGH)
        };
        self.write(low, steps as u8)?;
        self.write(high, (steps >> 8) as u8 & 0x01)?;
        Ok(f64::from(steps) / 4.0)
    }

    pub(crate) fn set_gain_mode(
        &mut self,
        chain: usize,
        mode: GainMode,
    ) -> Result<(), DeviceError> {
        let chain = chain.min(1);
        let shift = 2 * chain as u8;
        let before = self.read(reg::GAIN_MODE)?;
        let bits = match mode {
            GainMode::Manual => 0,
            GainMode::FastAttack => 1,
            GainMode::SlowAttack => 2,
        };
        let after = before & !(0x03 << shift) | bits << shift;
        self.write(reg::GAIN_MODE, after)?;
        self.gain_mode[chain] = mode;
        let was_automatic = before & 0x0f != 0;
        let is_automatic = after & 0x0f != 0;
        if was_automatic != is_automatic {
            if is_automatic {
                self.write_all(setup::AUTOMATIC_GAIN)?;
            } else {
                self.write_all(setup::MANUAL_GAIN)?;
                self.reapply_gains()?;
            }
        }
        Ok(())
    }

    pub(crate) fn gain_index(&mut self, chain: usize) -> Result<f64, DeviceError> {
        let register = if chain == 0 {
            reg::RX1_GAIN_READBACK
        } else {
            reg::RX2_GAIN_READBACK
        };
        Ok(f64::from(self.read(register)? & 0x7f))
    }

    pub(crate) fn set_bandwidth(&mut self, hz: f64) -> Result<f64, DeviceError> {
        self.bandwidth = hz.clamp(MIN_BANDWIDTH, MAX_BANDWIDTH);
        self.calibrate_filters()?;
        Ok(self.bandwidth)
    }

    pub(crate) fn set_tracking(&mut self, tracking: Tracking) -> Result<(), DeviceError> {
        let quadrature_off = self.tracking.quadrature && !tracking.quadrature;
        self.tracking = tracking;
        self.apply_dc_tracking()?;
        self.apply_quadrature_tracking()?;
        if quadrature_off {
            self.write(reg::ENSM_CONFIG_1, setup::ENSM_ALERT_TX_ON)?;
            self.calibrate_rx_quadrature()?;
            self.write(reg::ENSM_CONFIG_1, setup::ENSM_DUPLEX)?;
        }
        Ok(())
    }

    pub(crate) fn set_chains(&mut self, chains: Chains) -> Result<(), DeviceError> {
        let bits = |on: [bool; 2]| u8::from(on[0]) << 6 | u8::from(on[1]) << 7;
        self.shadow.tx_filters = self.shadow.tx_filters & 0x3f | bits(chains.tx);
        self.shadow.rx_filters = self.shadow.rx_filters & 0x3f | bits(chains.rx);
        let mut state = self.state()?;
        let back_to_duplex = state == State::Duplex;
        if back_to_duplex {
            self.write(reg::ENSM_CONFIG_1, setup::ENSM_ALERT)?;
        }
        let mut polls = 0;
        while matches!(state, State::Duplex | State::Flushing) {
            polls += 1;
            if polls > 1000 {
                return Err(DeviceError::Io(
                    "the AD9361 did not leave duplex".to_string(),
                ));
            }
            state = self.state()?;
        }
        self.write(reg::TX_FILTERS, self.shadow.tx_filters)?;
        self.write(reg::RX_FILTERS, self.shadow.rx_filters)?;
        if chains.tx.iter().any(|on| *on) {
            self.calibrate_tx_quadrature()?;
        }
        if back_to_duplex {
            self.write(reg::ENSM_CONFIG_1, setup::ENSM_DUPLEX)?;
        }
        Ok(())
    }

    pub(crate) fn set_loopback(&mut self, on: bool) -> Result<(), DeviceError> {
        self.write(reg::BIST_LOOPBACK, u8::from(on))
    }

    pub(crate) fn set_reference(&mut self, reference_hz: f64) -> Result<(), DeviceError> {
        if (reference_hz - self.reference_hz).abs() < f64::EPSILON {
            return Ok(());
        }
        self.reference_hz = reference_hz;
        let (rx, tx) = (self.rx_hz, self.tx_hz);
        self.requested_rx_hz = 0.0;
        self.requested_tx_hz = 0.0;
        self.tune(Direction::Rx, rx)?;
        self.tune(Direction::Tx, tx)?;
        Ok(())
    }

    fn reset(&mut self) -> Result<(), DeviceError> {
        self.write(reg::SPI_CONFIG, 0x01)?;
        self.write(reg::SPI_CONFIG, 0x00)?;
        std::thread::sleep(Duration::from_millis(20));
        let id = self.read(reg::PRODUCT_ID)? & 0xf8;
        if id != 0x08 {
            return Err(DeviceError::Io(format!(
                "the transceiver answered with product id {id:#04x}, not an AD9361"
            )));
        }
        Ok(())
    }

    fn power_up(&mut self) -> Result<(), DeviceError> {
        self.write_all(setup::POWER_UP)?;
        std::thread::sleep(Duration::from_millis(20));
        Ok(())
    }

    fn setup_interface(&mut self) -> Result<(), DeviceError> {
        self.write_all(setup::PARALLEL_PORT)?;
        self.write_all(setup::INTERFACE_DELAYS)
    }

    fn setup_auxiliaries(&mut self) -> Result<(), DeviceError> {
        self.write_all(setup::AUX_DAC)?;
        self.write_all(setup::AUX_ADC)?;
        self.write_all(setup::CONTROL_OUTPUTS)?;
        self.write_all(setup::GPO)
    }

    fn setup_synthesizers(&mut self) -> Result<(), DeviceError> {
        self.write_all(setup::SYNTHESIZERS)
    }

    fn setup_rates(&mut self, rate: f64) -> Result<(), DeviceError> {
        let chain = setup::filter_chain(rate);
        self.clocks.requested = rate;
        self.clocks.rx_fir_factor = chain.fir_factor;
        self.clocks.tx_fir_factor = chain.fir_factor;
        self.shadow.rx_filters = chain.rx;
        self.shadow.tx_filters = chain.tx;
        let adc_hz = self.tune_baseband_pll(rate * f64::from(chain.divider))?;
        let dac_hz = if adc_hz > 336e6 {
            self.shadow.bbpll |= 0x08;
            adc_hz / 2.0
        } else {
            self.shadow.bbpll &= 0xf7;
            adc_hz
        };
        self.write(reg::TX_FILTERS, self.shadow.tx_filters)?;
        self.write(reg::RX_FILTERS, self.shadow.rx_filters)?;
        self.write(reg::INPUT_SELECT, self.shadow.input)?;
        self.write(reg::BBPLL, self.shadow.bbpll)?;
        self.clocks.baseband_hz = adc_hz / f64::from(chain.divider);
        let per_tap = |clock: f64| 16 * ((clock / rate) + 0.5) as usize;
        let tx_budget = per_tap(dac_hz).min(if chain.fir_factor == 1 { 64 } else { 128 });
        let rx_budget = per_tap(adc_hz).min(128);
        self.load_fir(Direction::Tx, fir::taps_for(tx_budget), chain.fir_factor)?;
        self.load_fir(Direction::Rx, fir::taps_for(rx_budget), chain.fir_factor)
    }

    fn tune_baseband_pll(&mut self, rate: f64) -> Result<f64, DeviceError> {
        let pll = synth::baseband(rate, self.reference_hz).ok_or_else(|| {
            DeviceError::Unsupported(format!("no converter clock reaches {rate} Hz"))
        })?;
        self.write(reg::BBPLL_REFERENCE_DIVIDER, 0x00)?;
        self.write(
            reg::BBPLL_CHARGE_PUMP,
            synth::baseband_charge_pump(pll.vco_hz),
        )?;
        self.write_all(setup::BBPLL_LOOP_FILTER)?;
        self.write(reg::BBPLL_FRACTION_LOW, pll.fraction as u8)?;
        self.write(reg::BBPLL_FRACTION_MID, (pll.fraction >> 8) as u8)?;
        self.write(reg::BBPLL_FRACTION_HIGH, (pll.fraction >> 16) as u8)?;
        self.write(reg::BBPLL_INTEGER, pll.integer as u8)?;
        self.lock_baseband_pll()?;
        self.shadow.bbpll = self.shadow.bbpll & 0xf8 | pll.divider_code;
        self.clocks.bbpll_hz = pll.vco_hz;
        self.clocks.adc_hz = pll.output_hz;
        Ok(pll.output_hz)
    }

    fn tune_synthesizer(&mut self, direction: Direction, hz: f64) -> Result<f64, DeviceError> {
        let pll = synth::rf(hz, 2.0 * self.reference_hz).ok_or_else(|| {
            DeviceError::Unsupported(format!("no synthesizer setting reaches {hz} Hz"))
        })?;
        let registers = setup::Synthesizer::of(direction);
        match direction {
            Direction::Rx => {
                self.requested_rx_hz = hz;
                self.shadow.input = self.shadow.input & 0xc0 | 0x03;
                self.shadow.dividers = self.shadow.dividers & 0xf0 | pll.divider_code;
            }
            Direction::Tx => {
                self.requested_tx_hz = hz;
                self.shadow.input &= 0xbf;
                self.shadow.dividers = self.shadow.dividers & 0x0f | pll.divider_code << 4;
            }
        }
        self.write(reg::INPUT_SELECT, self.shadow.input)?;
        self.load_vco(&registers, synth::vco_settings(pll.vco_hz))?;
        self.write(registers.fraction_low, pll.fraction as u8)?;
        self.write(registers.fraction_mid, (pll.fraction >> 8) as u8)?;
        self.write(registers.fraction_high, (pll.fraction >> 16) as u8)?;
        self.write(registers.integer_high, (pll.integer >> 8) as u8)?;
        self.write(registers.integer_low, pll.integer as u8)?;
        self.write(reg::RFPLL_DIVIDERS, self.shadow.dividers)?;
        std::thread::sleep(Duration::from_millis(2));
        if self.read(registers.lock)? & 0x02 == 0 {
            return Err(DeviceError::Io(format!(
                "the {direction:?} synthesizer did not lock at {hz} Hz"
            )));
        }
        match direction {
            Direction::Rx => self.rx_hz = pll.output_hz,
            Direction::Tx => self.tx_hz = pll.output_hz,
        }
        Ok(pll.output_hz)
    }

    fn load_vco(
        &mut self,
        registers: &setup::Synthesizer,
        vco: synth::VcoSettings,
    ) -> Result<(), DeviceError> {
        self.write(registers.vco_output, 0x40 | vco.output_level)?;
        self.write(registers.vco_varactor, 0xc0 | vco.varactor)?;
        self.write(registers.vco_bias, vco.bias_ref | vco.bias_tcf << 3)?;
        self.write(registers.vco_cal_offset, vco.cal_offset << 3)?;
        self.write(registers.vco_varactor_control, 0x00)?;
        self.write(registers.vco_varactor_ref, vco.varactor_ref)?;
        self.write(registers.vco_varactor_ref_tcf, 0x70)?;
        self.write(registers.charge_pump, 0x80 | vco.charge_pump)?;
        self.write(registers.loop_filter_1, vco.loop_c1 | vco.loop_c2 << 4)?;
        self.write(registers.loop_filter_2, vco.loop_c3 | vco.loop_r1 << 4)?;
        self.write(registers.loop_filter_3, vco.loop_r3)
    }

    fn reapply_gains(&mut self) -> Result<(), DeviceError> {
        for chain in 0..2 {
            self.set_rx_gain(chain, self.rx_gain[chain])?;
        }
        for chain in 0..2 {
            self.set_tx_attenuation(chain, self.tx_attenuation[chain])?;
        }
        Ok(())
    }

    fn state(&mut self) -> Result<State, DeviceError> {
        Ok(State::of(self.read(reg::STATE)?))
    }

    fn write(&mut self, register: u16, value: u8) -> Result<(), DeviceError> {
        self.bus.write(register, value)
    }

    fn read(&mut self, register: u16) -> Result<u8, DeviceError> {
        self.bus.read(register)
    }

    fn write_all(&mut self, writes: &[(u16, u8)]) -> Result<(), DeviceError> {
        for (register, value) in writes {
            self.write(*register, *value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests;
