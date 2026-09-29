use std::fmt;

use super::{
    error::{Error, Invalid, Result},
    usb::Transport,
};

const WRITE_FLAG: u16 = 0x10;
const DEMOD_VALUE_FLAG: u16 = 0x20;
const QUIRK_PAGE: u8 = 0x0a;
const QUIRK_REG: u8 = 0x01;
const REPEATER_REG: u8 = 0x01;
const REPEATER_OPEN: u8 = 0x18;
const REPEATER_CLOSED: u8 = 0x10;
const GPO: u16 = 0x3001;
const GPOE: u16 = 0x3003;
const GPD: u16 = 0x3004;
const PINS: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Block {
    Usb = 1,
    Sys = 2,
    Iic = 6,
}

impl Block {
    fn name(self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::Sys => "SYS",
            Self::Iic => "IIC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dir {
    Read,
    Write,
}

impl Dir {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    fn flag(self) -> u16 {
        match self {
            Self::Read => 0,
            Self::Write => WRITE_FLAG,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    Block { block: Block, addr: u16, dir: Dir },
    Demod { page: u8, reg: u8, dir: Dir },
    I2c { addr: u8, reg: Option<u8>, dir: Dir },
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Block { block, addr, dir } => {
                write!(f, "{} block {} 0x{addr:04x}", block.name(), dir.name())
            }
            Self::Demod { page, reg, dir } => {
                write!(f, "demod {} {page}:0x{reg:02x}", dir.name())
            }
            Self::I2c {
                addr,
                reg: Some(reg),
                dir,
            } => write!(f, "I2C {} 0x{addr:02x} reg 0x{reg:02x}", dir.name()),
            Self::I2c {
                addr,
                reg: None,
                dir,
            } => write!(f, "I2C {} 0x{addr:02x}", dir.name()),
        }
    }
}

impl Access {
    fn setup(self) -> (u16, u16) {
        match self {
            Self::Block { block, addr, dir } => (addr, ((block as u16) << 8) | dir.flag()),
            Self::Demod { page, reg, dir } => (
                (u16::from(reg) << 8) | DEMOD_VALUE_FLAG,
                u16::from(page) | dir.flag(),
            ),
            Self::I2c { addr, dir, .. } => {
                (u16::from(addr), ((Block::Iic as u16) << 8) | dir.flag())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value {
    Byte(u8),
    Word(u16),
}

impl Value {
    fn wire(self) -> ([u8; 2], usize) {
        match self {
            Self::Byte(byte) => ([byte, 0], 1),
            Self::Word(word) => (word.to_be_bytes(), 2),
        }
    }
}

pub(crate) struct Chip<T> {
    link: T,
}

impl<T: Transport> Chip<T> {
    pub(crate) const fn new(link: T) -> Self {
        Self { link }
    }

    pub(crate) const fn link(&self) -> &T {
        &self.link
    }

    fn send(&self, access: Access, data: &[u8]) -> Result<()> {
        let (value, index) = access.setup();
        self.link
            .write(value, index, data)
            .map_err(|source| Error::Control { access, source })
    }

    fn fetch(&self, access: Access, len: usize) -> Result<Vec<u8>> {
        let (value, index) = access.setup();
        let wanted = u16::try_from(len).map_err(|_| Error::Short {
            access,
            wanted: len,
            got: 0,
        })?;
        let bytes = self
            .link
            .read(value, index, wanted)
            .map_err(|source| Error::Control { access, source })?;
        if bytes.len() < len {
            return Err(Error::Short {
                access,
                wanted: len,
                got: bytes.len(),
            });
        }
        Ok(bytes)
    }

    pub(crate) fn read_block(&self, block: Block, addr: u16, len: usize) -> Result<u16> {
        let access = Access::Block {
            block,
            addr,
            dir: Dir::Read,
        };
        let bytes = self.fetch(access, len)?;
        Ok(bytes
            .iter()
            .take(2)
            .rev()
            .fold(0, |word, byte| (word << 8) | u16::from(*byte)))
    }

    pub(crate) fn write_block(&self, block: Block, addr: u16, value: Value) -> Result<()> {
        let (bytes, len) = value.wire();
        let access = Access::Block {
            block,
            addr,
            dir: Dir::Write,
        };
        self.send(access, &bytes[..len])
    }

    pub(crate) fn read_demod(&self, page: u8, reg: u8) -> Result<u8> {
        let access = Access::Demod {
            page,
            reg,
            dir: Dir::Read,
        };
        Ok(self.fetch(access, 1)?[0])
    }

    pub(crate) fn write_demod(&self, page: u8, reg: u8, value: Value) -> Result<()> {
        let (bytes, len) = value.wire();
        let access = Access::Demod {
            page,
            reg,
            dir: Dir::Write,
        };
        self.send(access, &bytes[..len])?;
        self.read_demod(QUIRK_PAGE, QUIRK_REG).ok();
        Ok(())
    }

    pub(crate) fn demod(&self, page: u8, reg: u8, byte: u8) -> Result<()> {
        self.write_demod(page, reg, Value::Byte(byte))
    }

    pub(crate) fn write_i2c(&self, addr: u8, data: &[u8]) -> Result<()> {
        let access = Access::I2c {
            addr,
            reg: data.first().copied(),
            dir: Dir::Write,
        };
        self.send(access, data)
    }

    pub(crate) fn read_i2c(&self, addr: u8, len: usize) -> Result<Vec<u8>> {
        let access = Access::I2c {
            addr,
            reg: None,
            dir: Dir::Read,
        };
        self.fetch(access, len)
    }

    pub(crate) fn i2c_register(&self, addr: u8, reg: u8) -> Result<u8> {
        self.write_i2c(addr, &[reg])?;
        Ok(self.read_i2c(addr, 1)?[0])
    }

    pub(crate) fn set_repeater(&self, open: bool) -> Result<()> {
        let value = if open { REPEATER_OPEN } else { REPEATER_CLOSED };
        self.demod(1, REPEATER_REG, value)
    }

    pub(crate) fn through_repeater<R>(&self, op: impl FnOnce(&Self) -> Result<R>) -> Result<R> {
        let outcome = self.set_repeater(true).and_then(|()| op(self));
        let closed = self.set_repeater(false);
        let value = outcome?;
        closed?;
        Ok(value)
    }

    fn update_sys_bit(&self, addr: u16, bit: u8, set: bool) -> Result<()> {
        let old = self.read_block(Block::Sys, addr, 1)? as u8;
        let new = if set { old | bit } else { old & !bit };
        self.write_block(Block::Sys, addr, Value::Byte(new))
    }

    pub(crate) fn make_output(&self, pin: u8) -> Result<()> {
        let bit = pin_bit(pin)?;
        self.update_sys_bit(GPD, bit, false)?;
        self.update_sys_bit(GPOE, bit, true)
    }

    pub(crate) fn drive_pin(&self, pin: u8, high: bool) -> Result<()> {
        self.make_output(pin)?;
        self.update_sys_bit(GPO, pin_bit(pin)?, high)
    }
}

fn pin_bit(pin: u8) -> Result<u8> {
    if pin < PINS {
        Ok(1 << pin)
    } else {
        Err(Invalid::Pin(pin).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dongle::fake::{Fake, Transfer};

    fn chip() -> (Chip<Fake>, Fake) {
        let fake = Fake::default();
        (Chip::new(fake.clone()), fake)
    }

    #[test]
    fn writes_of_two_bytes_go_big_endian() {
        let (chip, fake) = chip();
        chip.write_block(Block::Usb, 0x2148, Value::Word(0x1002))
            .unwrap();
        chip.write_block(Block::Usb, 0x2000, Value::Byte(0xab))
            .unwrap();
        chip.write_block(Block::Usb, 0x2158, Value::Word(0x0009))
            .unwrap();
        assert_eq!(
            fake.transfers(),
            [
                Transfer::write(0x2148, 0x0110, &[0x10, 0x02]),
                Transfer::write(0x2000, 0x0110, &[0xab]),
                Transfer::write(0x2158, 0x0110, &[0x00, 0x09]),
            ]
        );
    }

    #[test]
    fn reads_of_two_bytes_come_back_little_endian() {
        let (chip, fake) = chip();
        fake.answer(0x2158, 0x0100, &[0x00, 0x02]);
        assert_eq!(chip.read_block(Block::Usb, 0x2158, 2).unwrap(), 0x0200);
        fake.answer(0x3001, 0x0200, &[0x5a]);
        assert_eq!(chip.read_block(Block::Sys, 0x3001, 1).unwrap(), 0x5a);
    }

    #[test]
    fn a_short_read_is_an_error() {
        let (chip, fake) = chip();
        fake.answer(0x2158, 0x0100, &[0x02]);
        let error = chip.read_block(Block::Usb, 0x2158, 2).unwrap_err();
        assert!(
            matches!(
                error,
                Error::Short {
                    wanted: 2,
                    got: 1,
                    ..
                }
            ),
            "{error}"
        );
        fake.answer(0x2158, 0x0100, &[]);
        assert!(chip.read_block(Block::Usb, 0x2158, 1).is_err());
    }

    #[test]
    fn every_demod_write_is_followed_by_the_dummy_read() {
        let (chip, fake) = chip();
        chip.write_demod(1, 0x9f, Value::Word(0x0300)).unwrap();
        assert_eq!(
            fake.transfers(),
            [
                Transfer::write(0x9f20, 0x0011, &[0x03, 0x00]),
                Transfer::read(0x0120, 0x000a, 1),
            ]
        );
    }

    #[test]
    fn a_failed_dummy_read_is_ignored() {
        let (chip, fake) = chip();
        fake.fail_reads(0x0120, 0x000a);
        assert!(chip.demod(0, 0x19, 0x05).is_ok());
    }

    #[test]
    fn i2c_goes_through_block_six() {
        let (chip, fake) = chip();
        chip.write_i2c(0x34, &[0x05, 0x83]).unwrap();
        fake.answer(0x0034, 0x0600, &[0x69]);
        assert_eq!(chip.read_i2c(0x34, 1).unwrap(), [0x69]);
        assert_eq!(
            fake.transfers(),
            [
                Transfer::write(0x0034, 0x0610, &[0x05, 0x83]),
                Transfer::read(0x0034, 0x0600, 1),
            ]
        );
    }

    #[test]
    fn the_repeater_closes_even_when_the_tuner_failed() {
        let (chip, fake) = chip();
        let outcome: Result<()> = chip.through_repeater(|_| Err(Error::NoTuner));
        assert!(matches!(outcome, Err(Error::NoTuner)));
        let writes = fake.demod_writes();
        assert_eq!(writes, [(1, 0x01, vec![0x18]), (1, 0x01, vec![0x10])]);
    }

    #[test]
    fn the_tuner_error_wins_over_a_failed_close() {
        let (chip, fake) = chip();
        let outcome: Result<()> = chip.through_repeater(|_| {
            fake.fail_writes(0x0120, 0x0011);
            Err(Error::NoTuner)
        });
        assert!(matches!(outcome, Err(Error::NoTuner)));
    }

    #[test]
    fn a_failed_close_surfaces_when_the_tuner_worked() {
        let (chip, fake) = chip();
        let outcome = chip.through_repeater(|_| {
            fake.fail_writes(0x0120, 0x0011);
            Ok(())
        });
        assert!(matches!(outcome, Err(Error::Control { .. })));
    }

    #[test]
    fn driving_a_pin_makes_it_an_output_first() {
        let (chip, fake) = chip();
        fake.answer(0x3004, 0x0200, &[0xff]);
        fake.answer(0x3003, 0x0200, &[0x00]);
        fake.answer(0x3001, 0x0200, &[0x01]);
        chip.drive_pin(5, true).unwrap();
        assert_eq!(
            fake.block_writes(),
            [
                (0x3004, vec![0xdf]),
                (0x3003, vec![0x20]),
                (0x3001, vec![0x21])
            ]
        );
    }

    #[test]
    fn driving_a_pin_low_clears_only_that_bit() {
        let (chip, fake) = chip();
        fake.answer(0x3001, 0x0200, &[0xff]);
        chip.drive_pin(0, false).unwrap();
        assert_eq!(fake.block_writes().last(), Some(&(0x3001, vec![0xfe])));
    }

    #[test]
    fn there_are_eight_pins() {
        let (chip, _) = chip();
        assert!(matches!(
            chip.drive_pin(8, true),
            Err(Error::Invalid(Invalid::Pin(8)))
        ));
    }

    #[test]
    fn accesses_name_their_register() {
        let block = Access::Block {
            block: Block::Usb,
            addr: 0x2148,
            dir: Dir::Write,
        };
        assert_eq!(block.to_string(), "USB block write 0x2148");
        let i2c = Access::I2c {
            addr: 0x34,
            reg: Some(0x05),
            dir: Dir::Write,
        };
        assert_eq!(i2c.to_string(), "I2C write 0x34 reg 0x05");
    }
}
