use super::{
    board::Board,
    chip::Chip,
    demod::{self, Branch, CRYSTAL_HZ, DIRECT_MAX_HZ},
    eeprom,
    error::{Invalid, Result},
    tuner::{self, ChipBus, GAINS, Tuner, TunerKind},
    usb::{Transport, UsbControl},
};

const BIAS_TEE_PIN: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DirectSampling {
    #[default]
    Off,
    I,
    Q,
}

impl DirectSampling {
    pub(crate) const MODES: [Self; 3] = [Self::Off, Self::I, Self::Q];

    pub(crate) const fn wire_name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::I => "i",
            Self::Q => "q",
        }
    }

    pub(crate) fn from_wire(text: &str) -> Option<Self> {
        Self::MODES
            .into_iter()
            .find(|mode| mode.wire_name() == text)
    }

    const fn branch(self) -> Option<Branch> {
        match self {
            Self::Off => None,
            Self::I => Some(Branch::I),
            Self::Q => Some(Branch::Q),
        }
    }
}

pub(crate) type Dongle = Radio<UsbControl>;

pub(crate) struct Radio<T: Transport> {
    chip: Chip<T>,
    tuner: Tuner,
    board: Board,
    ppm: i32,
    rate_hz: u32,
    center_hz: Option<u32>,
    direct: DirectSampling,
    bias_tee_at_start: bool,
}

impl<T: Transport> Radio<T> {
    pub(crate) fn start(link: T, board: Board) -> Result<Self> {
        let chip = Chip::new(link);
        demod::reset_usb(&chip)?;
        demod::init_baseband(&chip)?;
        let tuner = chip.through_repeater(|chip| {
            let kind = tuner::identify(chip)?;
            let mut tuner = Tuner::new(kind, board);
            tuner.init(&mut ChipBus::new(chip, kind))?;
            Ok(tuner)
        })?;
        demod::tuner_path(&chip, tuner.if_hz(), CRYSTAL_HZ)?;
        let bias_tee_at_start = eeprom::read_bias_tee(&chip).unwrap_or_else(|error| {
            tracing::warn!(%error, "EEPROM unreadable, bias tee starts off");
            false
        });
        Ok(Self {
            chip,
            tuner,
            board,
            ppm: 0,
            rate_hz: 0,
            center_hz: None,
            direct: DirectSampling::Off,
            bias_tee_at_start,
        })
    }

    pub(crate) const fn chip(&self) -> &Chip<T> {
        &self.chip
    }

    pub(crate) const fn board(&self) -> Board {
        self.board
    }

    pub(crate) const fn tuner_kind(&self) -> TunerKind {
        self.tuner.kind()
    }

    pub(crate) const fn gain_table(&self) -> &'static [i32] {
        GAINS
    }

    pub(crate) const fn center_hz(&self) -> Option<u32> {
        self.center_hz
    }

    pub(crate) const fn sample_rate(&self) -> u32 {
        self.rate_hz
    }

    pub(crate) const fn ppm(&self) -> i32 {
        self.ppm
    }

    pub(crate) const fn direct_sampling(&self) -> DirectSampling {
        self.direct
    }

    pub(crate) const fn bias_tee_at_start(&self) -> bool {
        self.bias_tee_at_start
    }

    fn on_tuner<R>(
        &mut self,
        op: impl FnOnce(&mut Tuner, &mut ChipBus<'_, T>) -> Result<R>,
    ) -> Result<R> {
        let kind = self.tuner.kind();
        let tuner = &mut self.tuner;
        self.chip
            .through_repeater(|chip| op(tuner, &mut ChipBus::new(chip, kind)))
    }

    fn require_tuner(&self) -> Result<()> {
        match self.direct {
            DirectSampling::Off => Ok(()),
            DirectSampling::I | DirectSampling::Q => Err(Invalid::TunerBypassed.into()),
        }
    }

    fn write_if(&self, if_hz: u32) -> Result<()> {
        demod::write_if(&self.chip, if_hz, demod::corrected(CRYSTAL_HZ, self.ppm))
    }

    fn retune(&mut self) -> Result<()> {
        match self.center_hz {
            Some(hz) => self.set_center(hz),
            None => Ok(()),
        }
    }

    pub(crate) fn set_center(&mut self, hz: u32) -> Result<()> {
        self.center_hz = None;
        if self.direct == DirectSampling::Off {
            self.on_tuner(|tuner, bus| tuner.tune(bus, hz))?;
            self.write_if(self.tuner.if_hz())?;
        } else {
            if hz > DIRECT_MAX_HZ {
                return Err(Invalid::DirectCenter(hz).into());
            }
            self.write_if(hz)?;
        }
        self.center_hz = Some(hz);
        Ok(())
    }

    pub(crate) fn set_sample_rate(&mut self, rate: u32) -> Result<()> {
        let plan = demod::resampler(rate)?;
        if self.direct == DirectSampling::Off {
            let if_hz = self.on_tuner(|tuner, bus| tuner.set_bandwidth(bus, plan.actual))?;
            self.write_if(if_hz)?;
            self.retune()?;
        }
        demod::write_ratio(&self.chip, plan.ratio)?;
        demod::write_ppm(&self.chip, self.ppm)?;
        demod::soft_reset(&self.chip)?;
        self.rate_hz = plan.actual;
        Ok(())
    }

    pub(crate) fn set_bandwidth(&mut self, hz: u32) -> Result<u32> {
        self.require_tuner()?;
        let wanted = if hz == 0 { self.rate_hz } else { hz };
        let if_hz = self.on_tuner(|tuner, bus| tuner.set_bandwidth(bus, wanted))?;
        self.write_if(if_hz)?;
        self.retune()?;
        Ok(if_hz)
    }

    pub(crate) fn set_ppm(&mut self, ppm: i32) -> Result<()> {
        demod::write_ppm(&self.chip, ppm)?;
        self.ppm = ppm;
        let reference = demod::corrected(self.tuner.nominal_crystal_hz(), ppm);
        self.tuner.set_reference(reference);
        self.retune()
    }

    pub(crate) fn set_auto_gain(&mut self) -> Result<()> {
        self.require_tuner()?;
        self.on_tuner(|tuner, bus| tuner.auto_gain(bus))
    }

    pub(crate) fn set_manual_gain(&mut self, tenths: i32) -> Result<()> {
        self.require_tuner()?;
        self.on_tuner(|tuner, bus| tuner.manual_gain(bus, tenths))
    }

    pub(crate) fn measured_gain(&mut self) -> Result<i32> {
        self.require_tuner()?;
        self.on_tuner(|tuner, bus| tuner.gain(bus))
    }

    pub(crate) fn set_direct_sampling(&mut self, mode: DirectSampling) -> Result<()> {
        if mode != DirectSampling::Off && self.board.has_upconverter() {
            return Err(Invalid::NoDirectSampling.into());
        }
        self.center_hz = None;
        match mode.branch() {
            Some(branch) => {
                self.on_tuner(|tuner, bus| tuner.standby(bus))?;
                demod::direct_path(&self.chip, branch)?;
            }
            None => {
                self.on_tuner(|tuner, bus| tuner.init(bus))?;
                self.leave_direct()?;
            }
        }
        self.direct = mode;
        Ok(())
    }

    fn leave_direct(&self) -> Result<()> {
        let crystal = demod::corrected(CRYSTAL_HZ, self.ppm);
        demod::tuner_path(&self.chip, self.tuner.if_hz(), crystal)?;
        demod::select_adc(&self.chip, Branch::I)
    }

    pub(crate) fn set_dither(&mut self, on: bool) -> Result<()> {
        self.tuner.set_dither(on);
        match self.direct {
            DirectSampling::Off => self.retune(),
            DirectSampling::I | DirectSampling::Q => Ok(()),
        }
    }

    pub(crate) fn set_bias_tee(&self, on: bool) -> Result<()> {
        self.chip.drive_pin(BIAS_TEE_PIN, on)
    }

    pub(crate) fn set_pin(&self, pin: u8, high: bool) -> Result<()> {
        self.chip.drive_pin(pin, high)
    }

    #[cfg(test)]
    pub(crate) fn pin_high(&self, pin: u8) -> Result<bool> {
        self.chip.pin_high(pin)
    }

    #[cfg(test)]
    pub(crate) fn pll_locked(&mut self) -> Result<bool> {
        self.require_tuner()?;
        self.on_tuner(|tuner, bus| tuner.locked(bus))
    }

    #[cfg(test)]
    pub(crate) fn set_counter(&self, on: bool) -> Result<()> {
        demod::set_counter(&self.chip, on)
    }
}

impl<T: Transport> Drop for Radio<T> {
    fn drop(&mut self) {
        match demod::hold_endpoint(&self.chip) {
            Ok(()) => {}
            Err(error) if error.is_disconnected() => {
                tracing::debug!(%error, "released an unplugged RTL-SDR");
            }
            Err(error) => tracing::warn!(%error, "RTL-SDR did not stop sampling"),
        }
    }
}

#[cfg(test)]
mod tests;
