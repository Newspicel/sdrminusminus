use std::time::Duration;

use super::{board::Board, chip::Chip, error::Result, usb::Transport};

mod filter;
mod gain;
mod input;
mod mux;
mod pll;
mod probe;
mod sequence;

pub(crate) use gain::GAINS;
pub(crate) use probe::identify;
pub(crate) use sequence::Tuner;

const R820T_ADDR: u8 = 0x34;
const R828D_ADDR: u8 = 0x74;
const CRYSTAL_28_8_HZ: u32 = 28_800_000;
const CRYSTAL_16_HZ: u32 = 16_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TunerKind {
    R820T,
    R828D,
}

impl TunerKind {
    pub(crate) const fn address(self) -> u8 {
        match self {
            Self::R820T => R820T_ADDR,
            Self::R828D => R828D_ADDR,
        }
    }

    pub(crate) const fn crystal_hz(self, board: Board) -> u32 {
        match (self, board) {
            (Self::R828D, Board::Generic | Board::BlogV4Lite) => CRYSTAL_16_HZ,
            _ => CRYSTAL_28_8_HZ,
        }
    }

    pub(crate) const fn vco_reference(self, board: Board) -> u8 {
        match (self, board) {
            (Self::R828D, _) | (Self::R820T, Board::BlogV4Lite) => 1,
            (Self::R820T, _) => 2,
        }
    }
}

pub(crate) trait TunerBus {
    fn write(&mut self, data: &[u8]) -> Result<()>;
    fn read(&mut self, len: usize) -> Result<Vec<u8>>;
    fn drive_pin(&mut self, pin: u8, high: bool) -> Result<()>;
    fn pause(&mut self, duration: Duration);
}

pub(crate) struct ChipBus<'a, T> {
    chip: &'a Chip<T>,
    addr: u8,
}

impl<'a, T: Transport> ChipBus<'a, T> {
    pub(crate) const fn new(chip: &'a Chip<T>, kind: TunerKind) -> Self {
        Self {
            chip,
            addr: kind.address(),
        }
    }
}

impl<T: Transport> TunerBus for ChipBus<'_, T> {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.chip.write_i2c(self.addr, data)
    }

    fn read(&mut self, len: usize) -> Result<Vec<u8>> {
        self.chip.read_i2c(self.addr, len)
    }

    fn drive_pin(&mut self, pin: u8, high: bool) -> Result<()> {
        self.chip.drive_pin(pin, high)
    }

    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crystals_follow_the_board() {
        assert_eq!(TunerKind::R820T.crystal_hz(Board::Generic), 28_800_000);
        assert_eq!(TunerKind::R820T.crystal_hz(Board::BlogV4Lite), 28_800_000);
        assert_eq!(TunerKind::R828D.crystal_hz(Board::BlogV4), 28_800_000);
        assert_eq!(TunerKind::R828D.crystal_hz(Board::Generic), 16_000_000);
    }

    #[test]
    fn the_vco_power_reference_is_one_for_r828d_and_the_v4_lite() {
        assert_eq!(TunerKind::R828D.vco_reference(Board::Generic), 1);
        assert_eq!(TunerKind::R828D.vco_reference(Board::BlogV4), 1);
        assert_eq!(TunerKind::R820T.vco_reference(Board::BlogV4Lite), 1);
        assert_eq!(TunerKind::R820T.vco_reference(Board::Generic), 2);
    }
}
