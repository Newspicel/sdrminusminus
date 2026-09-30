use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use nusb::transfer::TransferError;

use super::usb::Transport;

const WRITE_FLAG: u16 = 0x10;
const I2C_INDEX: u16 = 0x0600;
const EEPROM: u8 = 0xa0;
const TUNER_ID: u8 = 0x69;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Transfer {
    Write {
        value: u16,
        index: u16,
        data: Vec<u8>,
    },
    Read {
        value: u16,
        index: u16,
        len: u16,
    },
}

impl Transfer {
    pub(crate) fn write(value: u16, index: u16, data: &[u8]) -> Self {
        Self::Write {
            value,
            index,
            data: data.to_vec(),
        }
    }

    pub(crate) const fn read(value: u16, index: u16, len: u16) -> Self {
        Self::Read { value, index, len }
    }
}

struct State {
    log: Vec<Transfer>,
    memory: HashMap<(u16, u16), Vec<u8>>,
    failing: HashSet<(u16, u16)>,
    tuner: Option<u8>,
    status: [u8; 5],
    pointers: HashMap<u8, u8>,
    eeprom: [u8; 256],
    eeprom_protected: bool,
    answering: HashMap<u8, [u8; 256]>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            log: Vec::new(),
            memory: HashMap::new(),
            failing: HashSet::new(),
            tuner: Some(0x34),
            status: [0, 0, 0x40, 0, 0x25],
            pointers: HashMap::new(),
            eeprom: [0xff; 256],
            eeprom_protected: false,
            answering: HashMap::new(),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Fake {
    state: Rc<RefCell<State>>,
}

impl Fake {
    pub(crate) fn answering_at(addr: Option<u8>) -> Self {
        let fake = Self::default();
        fake.state.borrow_mut().tuner = addr;
        fake
    }

    pub(crate) fn transfers(&self) -> Vec<Transfer> {
        self.state.borrow().log.clone()
    }

    pub(crate) fn clear(&self) {
        self.state.borrow_mut().log.clear();
    }

    pub(crate) fn answer(&self, value: u16, index: u16, bytes: &[u8]) {
        self.state
            .borrow_mut()
            .memory
            .insert((value, index), bytes.to_vec());
    }

    pub(crate) fn fail_reads(&self, value: u16, index: u16) {
        self.state.borrow_mut().failing.insert((value, index));
    }

    pub(crate) fn fail_writes(&self, value: u16, index: u16) {
        self.state
            .borrow_mut()
            .failing
            .insert((value, index | WRITE_FLAG));
    }

    pub(crate) fn heal(&self) {
        self.state.borrow_mut().failing.clear();
    }

    pub(crate) fn set_status(&self, byte: usize, value: u8) {
        self.state.borrow_mut().status[byte] = value;
    }

    pub(crate) fn set_eeprom(&self, offset: usize, value: u8) {
        self.state.borrow_mut().eeprom[offset] = value;
    }

    pub(crate) fn eeprom(&self) -> [u8; 256] {
        self.state.borrow().eeprom
    }

    pub(crate) fn protect_eeprom(&self) {
        self.state.borrow_mut().eeprom_protected = true;
    }

    pub(crate) fn answer_i2c(&self, addr: u8, reg: u8, value: u8) {
        let mut state = self.state.borrow_mut();
        let regs = state.answering.entry(addr).or_insert([0; 256]);
        regs[usize::from(reg)] = value;
    }

    pub(crate) fn writes(&self) -> Vec<(u16, u16, Vec<u8>)> {
        self.state
            .borrow()
            .log
            .iter()
            .filter_map(|transfer| match transfer {
                Transfer::Write { value, index, data } => Some((*value, *index, data.clone())),
                Transfer::Read { .. } => None,
            })
            .collect()
    }

    pub(crate) fn demod_writes(&self) -> Vec<(u8, u8, Vec<u8>)> {
        self.writes()
            .into_iter()
            .filter(|(value, index, _)| index >> 8 == 0 && value & 0xff == 0x20)
            .map(|(value, index, data)| ((index & 0x0f) as u8, (value >> 8) as u8, data))
            .collect()
    }

    pub(crate) fn block_writes(&self) -> Vec<(u16, Vec<u8>)> {
        self.writes()
            .into_iter()
            .filter(|(_, index, _)| matches!(index, 0x0110 | 0x0210))
            .map(|(value, _, data)| (value, data))
            .collect()
    }

    pub(crate) fn i2c_writes(&self, addr: u8) -> Vec<Vec<u8>> {
        self.writes()
            .into_iter()
            .filter(|(value, index, _)| {
                *value == u16::from(addr) && *index == I2C_INDEX | WRITE_FLAG
            })
            .map(|(_, _, data)| data)
            .collect()
    }

    pub(crate) fn tuner_writes(&self) -> Vec<(u8, u8)> {
        let addr = self.state.borrow().tuner.unwrap_or(0x34);
        self.i2c_writes(addr)
            .into_iter()
            .filter(|message| message.len() > 1)
            .flat_map(|message| {
                let start = message[0];
                message[1..]
                    .iter()
                    .enumerate()
                    .map(move |(at, byte)| (start + at as u8, *byte))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(crate) fn tuner_value(&self, reg: u8) -> Option<u8> {
        self.tuner_writes()
            .into_iter()
            .rev()
            .find(|(written, _)| *written == reg)
            .map(|(_, value)| value)
    }
}

impl State {
    fn i2c_read(&mut self, addr: u8, len: u16) -> Result<Vec<u8>, TransferError> {
        let pointer = self.pointers.get(&addr).copied().unwrap_or(0);
        let wanted = usize::from(len);
        if addr == EEPROM {
            return Ok(self
                .eeprom
                .iter()
                .cycle()
                .skip(usize::from(pointer))
                .take(wanted)
                .copied()
                .collect());
        }
        if Some(addr) == self.tuner {
            return Ok(self.tuner_bytes(wanted));
        }
        match self.answering.get(&addr) {
            Some(regs) => Ok(vec![regs[usize::from(pointer)]; wanted]),
            None => Err(TransferError::Stall),
        }
    }

    fn tuner_bytes(&self, wanted: usize) -> Vec<u8> {
        let mut bytes: Vec<u8> = self.status.iter().map(|byte| byte.reverse_bits()).collect();
        bytes[0] = TUNER_ID;
        bytes.resize(wanted, 0);
        bytes
    }

    fn i2c_write(&mut self, addr: u8, data: &[u8]) -> Result<(), TransferError> {
        let known =
            addr == EEPROM || Some(addr) == self.tuner || self.answering.contains_key(&addr);
        if !known {
            return Err(TransferError::Stall);
        }
        if let Some(reg) = data.first() {
            self.pointers.insert(addr, *reg);
        }
        if addr == EEPROM && !self.eeprom_protected {
            self.store_eeprom(data);
        }
        Ok(())
    }

    fn store_eeprom(&mut self, data: &[u8]) {
        if let Some((reg, bytes)) = data.split_first() {
            for (offset, byte) in (usize::from(*reg)..).zip(bytes) {
                self.eeprom[offset % 256] = *byte;
            }
        }
    }
}

impl Transport for Fake {
    fn read(&self, value: u16, index: u16, len: u16) -> Result<Vec<u8>, TransferError> {
        let mut state = self.state.borrow_mut();
        state.log.push(Transfer::read(value, index, len));
        if state.failing.contains(&(value, index)) {
            return Err(TransferError::Stall);
        }
        if index == I2C_INDEX {
            return state.i2c_read(value as u8, len);
        }
        Ok(state
            .memory
            .get(&(value, index))
            .cloned()
            .unwrap_or_else(|| vec![0; usize::from(len)]))
    }

    fn write(&self, value: u16, index: u16, data: &[u8]) -> Result<(), TransferError> {
        let mut state = self.state.borrow_mut();
        state.log.push(Transfer::write(value, index, data));
        if state.failing.contains(&(value, index)) {
            return Err(TransferError::Stall);
        }
        if index == I2C_INDEX | WRITE_FLAG {
            return state.i2c_write(value as u8, data);
        }
        state
            .memory
            .insert((value, index & !WRITE_FLAG), data.to_vec());
        Ok(())
    }
}
