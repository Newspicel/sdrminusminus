use std::time::Duration;

use super::{
    TunerBus, TunerKind,
    filter::{self, DEFAULT_IF_HZ},
    gain::{self, Stages},
    input::{self, Band, Step},
    mux, pll,
};
use crate::dongle::{
    board::Board,
    chip::{Access, Dir},
    error::{Error, Invalid, PllFault, Result},
};

const FIRST_REG: u8 = 0x05;
const MESSAGE_DATA: usize = 7;
const INIT: [u8; 27] = [
    0x83, 0x32, 0x75, 0xc0, 0x40, 0xd6, 0x6c, 0xf5, 0x63, 0x75, 0x68, 0x6c, 0x83, 0x80, 0x00, 0x0f,
    0x00, 0xc0, 0x30, 0x48, 0xcc, 0x60, 0x00, 0x54, 0xae, 0x4a, 0xc0,
];
const VERSION: u8 = 49;
const CALIBRATION_LO_HZ: u64 = 56_000_000;
const CALIBRATION_ATTEMPTS: usize = 2;
const CALIBRATION_WAIT: Duration = Duration::from_millis(2);
const PLL_LOCKED: u8 = 0x40;

type Write = (u8, u8, u8);

const SETUP: [Write; 3] = [
    (0x0c, 0x00, 0x0f),
    (0x13, VERSION, 0x3f),
    (0x1d, 0x00, 0x38),
];

const AFTER_CALIBRATION: [Write; 8] = [
    (0x0b, 0x6b, 0xef),
    (0x07, 0x00, 0x80),
    (0x06, 0x10, 0x30),
    (0x1e, 0x60, 0x60),
    (0x05, 0x01, 0x80),
    (0x1f, 0x00, 0x80),
    (0x0f, 0x00, 0x80),
    (0x19, 0x60, 0x60),
];

const SYSTEM: [Write; 17] = [
    (0x1d, 0xe5, 0xc7),
    (0x1c, 0x24, 0xf8),
    (0x0d, 0x53, 0xff),
    (0x0e, 0x75, 0xff),
    (0x05, 0x00, 0x60),
    (0x06, 0x00, 0x08),
    (0x11, 0x38, 0x38),
    (0x17, 0x30, 0x30),
    (0x0a, 0x40, 0x60),
    (0x1d, 0x00, 0x38),
    (0x1c, 0x00, 0x04),
    (0x06, 0x00, 0x40),
    (0x1a, 0x30, 0x30),
    (0x1d, 0x18, 0x38),
    (0x1c, 0x24, 0x04),
    (0x1e, 0x0e, 0x1f),
    (0x1a, 0x20, 0x30),
];

const STANDBY: [(u8, u8); 11] = [
    (0x06, 0xb1),
    (0x05, 0xa0),
    (0x07, 0x3a),
    (0x08, 0x40),
    (0x09, 0xc0),
    (0x0a, 0x36),
    (0x0c, 0x35),
    (0x0f, 0x68),
    (0x11, 0x03),
    (0x17, 0xf4),
    (0x19, 0x0c),
];

const AUTO_GAIN: [Write; 3] = [(0x05, 0x00, 0x10), (0x07, 0x10, 0x10), (0x0c, 0x0b, 0x9f)];
const MANUAL_GAIN: [Write; 3] = [(0x05, 0x10, 0x10), (0x07, 0x00, 0x10), (0x0c, 0x08, 0x9f)];

pub(crate) struct Tuner {
    kind: TunerKind,
    board: Board,
    shadow: [u8; 27],
    reference_hz: u32,
    vpr: u8,
    if_hz: u32,
    dither: bool,
    band: Option<Band>,
}

impl Tuner {
    pub(crate) const fn new(kind: TunerKind, board: Board) -> Self {
        Self {
            kind,
            board,
            shadow: INIT,
            reference_hz: kind.crystal_hz(board),
            vpr: kind.vco_reference(board),
            if_hz: DEFAULT_IF_HZ,
            dither: true,
            band: None,
        }
    }

    pub(crate) const fn kind(&self) -> TunerKind {
        self.kind
    }

    pub(crate) const fn if_hz(&self) -> u32 {
        self.if_hz
    }

    pub(crate) const fn nominal_crystal_hz(&self) -> u32 {
        self.kind.crystal_hz(self.board)
    }

    pub(crate) const fn set_reference(&mut self, hz: u32) {
        self.reference_hz = hz;
    }

    pub(crate) const fn set_dither(&mut self, on: bool) {
        self.dither = on;
    }

    fn slot(&mut self, reg: u8) -> Result<&mut u8> {
        reg.checked_sub(FIRST_REG)
            .and_then(|at| self.shadow.get_mut(usize::from(at)))
            .ok_or(Error::Invalid(Invalid::TunerRegister(reg)))
    }

    fn masked(&mut self, bus: &mut impl TunerBus, reg: u8, value: u8, mask: u8) -> Result<()> {
        let slot = self.slot(reg)?;
        let new = (*slot & !mask) | (value & mask);
        *slot = new;
        bus.write(&[reg, new])
    }

    fn full(&mut self, bus: &mut impl TunerBus, reg: u8, value: u8) -> Result<()> {
        self.masked(bus, reg, value, 0xff)
    }

    fn apply(&mut self, bus: &mut impl TunerBus, writes: &[Write]) -> Result<()> {
        for &(reg, value, mask) in writes {
            self.masked(bus, reg, value, mask)?;
        }
        Ok(())
    }

    fn write_shadow(&self, bus: &mut impl TunerBus) -> Result<()> {
        for (chunk, start) in self
            .shadow
            .chunks(MESSAGE_DATA)
            .zip((FIRST_REG..).step_by(MESSAGE_DATA))
        {
            let mut message = [0u8; MESSAGE_DATA + 1];
            message[0] = start;
            message[1..=chunk.len()].copy_from_slice(chunk);
            bus.write(&message[..=chunk.len()])?;
        }
        Ok(())
    }

    fn status(&self, bus: &mut impl TunerBus, len: usize) -> Result<Vec<u8>> {
        bus.write(&[0x00])?;
        let bytes = bus.read(len)?;
        if bytes.len() < len {
            return Err(Error::Short {
                access: Access::I2c {
                    addr: self.kind.address(),
                    reg: None,
                    dir: Dir::Read,
                },
                wanted: len,
                got: bytes.len(),
            });
        }
        Ok(bytes.into_iter().map(u8::reverse_bits).collect())
    }

    pub(crate) fn init(&mut self, bus: &mut impl TunerBus) -> Result<()> {
        self.shadow = INIT;
        self.band = None;
        self.if_hz = DEFAULT_IF_HZ;
        self.write_shadow(bus)?;
        self.apply(bus, &SETUP)?;
        let code = self.calibrate(bus)?;
        self.masked(bus, 0x0f, 0x00, 0x04)?;
        self.masked(bus, 0x0a, 0x10 | code, 0x1f)?;
        self.apply(bus, &AFTER_CALIBRATION)?;
        self.apply(bus, &SYSTEM)
    }

    fn calibrate(&mut self, bus: &mut impl TunerBus) -> Result<u8> {
        let mut code = 0;
        for _ in 0..CALIBRATION_ATTEMPTS {
            self.apply(
                bus,
                &[(0x0b, 0x6b, 0x60), (0x0f, 0x04, 0x04), (0x10, 0x00, 0x03)],
            )?;
            match self.program_pll(bus, CALIBRATION_LO_HZ) {
                Err(Error::Pll { .. }) => return Ok(0),
                outcome => outcome?,
            }
            self.masked(bus, 0x0b, 0x10, 0x10)?;
            bus.pause(CALIBRATION_WAIT);
            self.masked(bus, 0x0b, 0x00, 0x10)?;
            code = self.status(bus, 5)?[4] & 0x0f;
            if code != 0x00 && code != 0x0f {
                break;
            }
        }
        Ok(if code == 0x0f { 0 } else { code })
    }

    pub(crate) fn standby(&mut self, bus: &mut impl TunerBus) -> Result<()> {
        self.band = None;
        for &(reg, value) in &STANDBY {
            self.full(bus, reg, value)?;
        }
        Ok(())
    }

    fn program_pll(&mut self, bus: &mut impl TunerBus, lo_hz: u64) -> Result<()> {
        let fault = |fault| Error::Pll { lo_hz, fault };
        self.apply(
            bus,
            &[(0x10, 0x00, 0x10), (0x1a, 0x00, 0x0c), (0x12, 0x80, 0xe0)],
        )?;
        let divider = pll::divider(lo_hz).map_err(fault)?;
        let fine = (self.status(bus, 5)?[4] & 0x30) >> 4;
        let code = pll::tuned_code(divider.code, fine, self.vpr);
        self.masked(bus, 0x10, code << 5, 0xe0)?;
        let synth = pll::synth(lo_hz, divider, self.reference_hz, self.vpr).map_err(fault)?;
        self.full(bus, 0x14, pll::nint_reg(synth.nint))?;
        self.masked(bus, 0x12, pll::modulator_bits(synth.sdm, self.dither), 0x18)?;
        self.full(bus, 0x16, (synth.sdm >> 8) as u8)?;
        self.full(bus, 0x15, (synth.sdm & 0xff) as u8)?;
        let locked = self.await_lock(bus)?;
        self.masked(bus, 0x1a, 0x08, 0x08)?;
        if locked {
            Ok(())
        } else {
            Err(fault(PllFault::NoLock))
        }
    }

    fn await_lock(&mut self, bus: &mut impl TunerBus) -> Result<bool> {
        for boost in [Some(0x60), Some(0x00), None] {
            if self.status(bus, 3)?[2] & PLL_LOCKED != 0 {
                return Ok(true);
            }
            if let Some(current) = boost {
                self.masked(bus, 0x12, current, 0xe0)?;
            }
        }
        Ok(false)
    }

    pub(crate) fn tune(&mut self, bus: &mut impl TunerBus, hz: u32) -> Result<()> {
        let lo_hz = input::rf_hz(self.board, hz).saturating_add(u64::from(self.if_hz));
        self.select_mux(bus, lo_hz)?;
        self.program_pll(bus, lo_hz)?;
        self.switch_input(bus, hz)
    }

    fn select_mux(&mut self, bus: &mut impl TunerBus, lo_hz: u64) -> Result<()> {
        let band = mux::mux_for(lo_hz);
        self.apply(
            bus,
            &[
                (0x17, band.open_drain, 0x08),
                (0x1a, band.rf_mux, 0xc3),
                (0x1b, band.tracking, 0xff),
                (0x10, 0x00, 0x0b),
                (0x08, 0x00, 0x3f),
                (0x09, 0x00, 0x3f),
            ],
        )
    }

    fn switch_input(&mut self, bus: &mut impl TunerBus, hz: u32) -> Result<()> {
        let switching = input::switching(self.board, self.kind, hz, self.band);
        for step in switching.steps {
            match step {
                Step::Reg { reg, value, mask } => self.masked(bus, reg, value, mask)?,
                Step::Pin { pin, high } => bus.drive_pin(pin, high)?,
            }
        }
        self.band = switching.band;
        Ok(())
    }

    pub(crate) fn set_bandwidth(&mut self, bus: &mut impl TunerBus, hz: u32) -> Result<u32> {
        let plan = filter::filter(hz);
        self.masked(bus, 0x0a, plan.reg0a, 0x10)?;
        self.masked(bus, 0x0b, plan.reg0b, 0xef)?;
        self.if_hz = plan.if_hz;
        Ok(plan.if_hz)
    }

    pub(crate) fn auto_gain(&mut self, bus: &mut impl TunerBus) -> Result<()> {
        self.apply(bus, &AUTO_GAIN)
    }

    pub(crate) fn manual_gain(&mut self, bus: &mut impl TunerBus, tenths: i32) -> Result<()> {
        self.apply(bus, &MANUAL_GAIN)?;
        let stages = gain::stages_for(tenths);
        self.masked(bus, 0x05, stages.lna, 0x0f)?;
        self.masked(bus, 0x07, stages.mixer, 0x0f)
    }

    pub(crate) fn gain(&self, bus: &mut impl TunerBus) -> Result<i32> {
        Ok(Stages::from_status(self.status(bus, 4)?[3]).tenths())
    }

    #[cfg(test)]
    pub(crate) fn locked(&self, bus: &mut impl TunerBus) -> Result<bool> {
        Ok(self.status(bus, 3)?[2] & PLL_LOCKED != 0)
    }
}

#[cfg(test)]
mod tests;
