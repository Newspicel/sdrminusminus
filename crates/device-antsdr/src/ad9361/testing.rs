use sdrmm_device::DeviceError;

use super::Bus;

const REGISTERS: usize = 0x400;
const PRODUCT_ID: u8 = 0x0a;

#[derive(Clone, Debug)]
pub(crate) struct FakeChip {
    pub(crate) registers: Vec<u8>,
    pub(crate) writes: Vec<(u16, u8)>,
    pub(crate) state: u8,
    pub(crate) synthesizers_lock: bool,
    pub(crate) product_id: u8,
}

impl Default for FakeChip {
    fn default() -> Self {
        let mut chip = Self {
            registers: vec![0; REGISTERS],
            writes: Vec::new(),
            state: 0,
            synthesizers_lock: true,
            product_id: PRODUCT_ID,
        };
        chip.reset();
        chip
    }
}

impl FakeChip {
    fn reset(&mut self) {
        self.registers.fill(0);
        self.registers[0x1eb] = 0x13;
        self.registers[0x1ec] = 0x40;
        self.registers[0x1e6] = 0x02;
        self.state = 0;
    }

    pub(crate) fn wrote(&self, register: u16, value: u8) -> bool {
        self.writes.contains(&(register, value))
    }

    pub(crate) fn writes_to(&self, register: u16) -> usize {
        self.writes.iter().filter(|(r, _)| *r == register).count()
    }

    pub(crate) fn last(&self, register: u16) -> Option<u8> {
        self.writes
            .iter()
            .rev()
            .find(|(r, _)| *r == register)
            .map(|(_, v)| *v)
    }

    pub(crate) fn apply(&mut self, register: u16, value: u8) {
        self.writes.push((register, value));
        let slot = usize::from(register) % REGISTERS;
        self.registers[slot] = value;
        match register {
            0x000 if value & 0x01 != 0 => self.reset(),
            0x014 => {
                self.state = if value & 0x20 != 0 {
                    0x0a
                } else if value & 0x01 != 0 {
                    0x05
                } else {
                    0x00
                };
            }
            0x016 => self.registers[slot] = 0,
            0x03f => self.registers[0x05e] = 0x80,
            0x23d if value & 0x04 != 0 => self.registers[0x244] = 0x80,
            0x27d if value & 0x04 != 0 => self.registers[0x284] = 0x80,
            _ => {}
        }
    }

    pub(crate) fn value(&self, register: u16) -> u8 {
        match register {
            0x017 => self.state,
            0x037 => self.product_id,
            0x2b0 => self.registers[0x109],
            0x2b5 => self.registers[0x10c],
            0x247 | 0x287 if self.synthesizers_lock => 0x02,
            0x247 | 0x287 => 0x00,
            _ => self.registers[usize::from(register) % REGISTERS],
        }
    }
}

impl Bus for FakeChip {
    fn write(&mut self, register: u16, value: u8) -> Result<(), DeviceError> {
        self.apply(register, value);
        Ok(())
    }

    fn read(&mut self, register: u16) -> Result<u8, DeviceError> {
        Ok(self.value(register))
    }
}
