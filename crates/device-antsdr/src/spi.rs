use std::sync::Arc;

use sdrmm_device::DeviceError;

use crate::{
    ad9361::Bus,
    control::{Control, Target},
    regs::{SPI_READBACK, core},
};

const BUS_CLOCK_HZ: f64 = 100e6;
const CODEC_CLOCK_HZ: f64 = 1e6;
const REFERENCE_PLL_CLOCK_HZ: f64 = 10e3;
const WORD_BITS: u32 = 24;
const MOSI_ON_FALLING_EDGE: u32 = 1 << 31;
const MISO_ON_RISING_EDGE: u32 = 1 << 30;
const CODEC_WRITE: u32 = 0x0080_0000;
const REGISTER_MASK: u32 = 0x003f_ff00;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Peripheral {
    Codec,
    ReferencePll,
}

impl Peripheral {
    const fn select(self) -> u32 {
        match self {
            Self::Codec => 1,
            Self::ReferencePll => 2,
        }
    }

    const fn edges(self) -> u32 {
        match self {
            Self::Codec => MOSI_ON_FALLING_EDGE,
            Self::ReferencePll => MISO_ON_RISING_EDGE,
        }
    }

    fn divider(self) -> u32 {
        let clock = match self {
            Self::Codec => CODEC_CLOCK_HZ,
            Self::ReferencePll => REFERENCE_PLL_CLOCK_HZ,
        };
        (BUS_CLOCK_HZ / clock / 2.0 - 0.5) as u32
    }

    fn control(self) -> u32 {
        self.edges() | WORD_BITS << 24 | self.select()
    }
}

#[derive(Debug)]
pub(crate) struct Spi {
    control: Arc<Control>,
    configured: Option<Peripheral>,
}

impl Spi {
    pub(crate) const fn new(control: Arc<Control>) -> Self {
        Self {
            control,
            configured: None,
        }
    }

    fn talk_to(&mut self, peripheral: Peripheral) -> Result<(), DeviceError> {
        if self.configured == Some(peripheral) {
            return Ok(());
        }
        self.control
            .poke(Target::Local, core::SPI_DIVIDER, peripheral.divider())?;
        self.control
            .poke(Target::Local, core::SPI_CONTROL, peripheral.control())?;
        self.configured = Some(peripheral);
        Ok(())
    }

    fn shift(&mut self, peripheral: Peripheral, word: u32) -> Result<(), DeviceError> {
        self.talk_to(peripheral)?;
        self.control
            .poke(Target::Local, core::SPI_DATA, word << (32 - WORD_BITS))
    }

    pub(crate) fn reference_pll(&mut self, words: &[u32]) -> Result<(), DeviceError> {
        for word in words {
            self.shift(Peripheral::ReferencePll, *word)?;
        }
        self.control.settle()
    }
}

impl Bus for Spi {
    fn write(&mut self, register: u16, value: u8) -> Result<(), DeviceError> {
        let word = CODEC_WRITE | (u32::from(register) << 8 & REGISTER_MASK) | u32::from(value);
        self.shift(Peripheral::Codec, word)
    }

    fn read(&mut self, register: u16) -> Result<u8, DeviceError> {
        self.shift(Peripheral::Codec, u32::from(register) << 8 & REGISTER_MASK)?;
        Ok(self.control.peek32(Target::Local, SPI_READBACK)? as u8)
    }
}

pub(crate) mod reference {
    const REFERENCE_DIVIDER: u32 = 1;
    const FEEDBACK_DIVIDER: u32 = 4;
    const CHARGE_PUMP_CURRENT: u32 = 7;
    const DIGITAL_LOCK_DETECT: u32 = 1;
    const POSITIVE_PHASE: u32 = 1;

    pub(crate) fn words(locked_to_external: bool) -> [u32; 4] {
        let tristate = u32::from(!locked_to_external);
        let function = DIGITAL_LOCK_DETECT << 4
            | POSITIVE_PHASE << 7
            | tristate << 8
            | CHARGE_PUMP_CURRENT << 15
            | CHARGE_PUMP_CURRENT << 18;
        [
            function | 3,
            function | 2,
            REFERENCE_DIVIDER << 2,
            FEEDBACK_DIVIDER << 8 | 1,
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use sdrmm_device::lock;

    use super::*;
    use crate::{control::testing::responder, regs::READBACK};

    #[test]
    fn the_codec_is_clocked_at_one_megahertz_with_24_bit_words() {
        assert_eq!(Peripheral::Codec.divider(), 49);
        assert_eq!(Peripheral::Codec.control(), 0x9800_0001);
        assert_eq!(Peripheral::ReferencePll.divider(), 4999);
        assert_eq!(Peripheral::ReferencePll.control(), 0x5800_0002);
    }

    #[test]
    fn a_register_read_shifts_the_address_and_returns_the_readback_byte() {
        let last = Mutex::new(0u32);
        let echo = responder(Box::new(move |_, register, value| {
            if register == core::SPI_DATA {
                *lock(&last) = value;
            }
            if register == READBACK {
                u64::from((*lock(&last) >> 16) & 0xff) | 0xab00
            } else {
                0
            }
        }));
        let writes = echo.writes.clone();
        let mut spi = Spi::new(Arc::new(Control::over(echo.socket)));
        spi.write(0x3f5, 0x01).expect("write");
        let byte = spi.read(0x037).expect("read");
        assert_eq!(byte, 0x37);
        let seen = lock(&writes).clone();
        assert_eq!(seen[0], (Target::Local, core::SPI_DIVIDER, 49));
        assert_eq!(seen[2], (Target::Local, core::SPI_DATA, 0x83f5_0100));
        assert_eq!(seen.len(), 5, "the clock is set once");
    }

    #[test]
    fn the_reference_pll_is_left_free_running_on_the_internal_clock() {
        let words = reference::words(false);
        assert_eq!(words[0] & 3, 3);
        assert_ne!(words[0] & 1 << 8, 0);
        assert_eq!(reference::words(true)[1] & 1 << 8, 0);
        assert_eq!(words[3], 0x401);
    }
}
