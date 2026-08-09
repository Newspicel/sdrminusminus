//! RTL2832U register access and the I2C bridge to the tuner.
//!
//! Every operation is a vendor control transfer with `bRequest = 0`; the register address and
//! block are encoded in `wValue`/`wIndex`:
//!
//! | operation | direction | `wValue` | `wIndex` |
//! |---|---|---|---|
//! | register read | IN | `addr` | `block << 8` |
//! | register write | OUT | `addr` | `(block << 8) \| 0x10` |
//! | demod read | IN | `(addr << 8) \| 0x20` | `page` |
//! | demod write | OUT | `(addr << 8) \| 0x20` | `0x10 \| page` |
//!
//! Register reads decode little-endian, writes encode big-endian. This asymmetry is the chip's,
//! not ours.

use std::time::Duration;

use nusb::{
    Interface, MaybeFuture,
    transfer::{ControlIn, ControlOut, ControlType, Recipient},
};

use super::error::{Error, Result};

/// Matches librtlsdr's control timeout.
const CTRL_TIMEOUT: Duration = Duration::from_millis(300);

/// USB controller register block.
pub(crate) const BLOCK_USB: u16 = 1;
/// System register block.
pub(crate) const BLOCK_SYS: u16 = 2;
/// I2C bridge block, used for tuner communication.
pub(crate) const BLOCK_IIC: u16 = 6;

/// System control register.
pub(crate) const USB_SYSCTL: u16 = 0x2000;
/// Endpoint A maximum packet size.
pub(crate) const USB_EPA_MAXPKT: u16 = 0x2158;
/// Endpoint A control register.
pub(crate) const USB_EPA_CTL: u16 = 0x2148;
/// Demodulator control register.
pub(crate) const DEMOD_CTL: u16 = 0x3000;
/// GPIO output register.
pub(crate) const GPO: u16 = 0x3001;
/// GPIO output enable register.
pub(crate) const GPOE: u16 = 0x3003;
/// GPIO direction register.
pub(crate) const GPD: u16 = 0x3004;
/// Demodulator control 1 register.
pub(crate) const DEMOD_CTL_1: u16 = 0x300b;

/// I2C address of the EEPROM.
const EEPROM_ADDR: u8 = 0xa0;
/// Total EEPROM size in bytes.
const EEPROM_SIZE: usize = 256;

/// Low-level RTL2832U handle over a claimed USB interface.
///
/// Cheap to clone — `nusb::Interface` is `Arc`-backed — which is what lets the control plane and
/// the streaming endpoint work the same device from different threads.
#[derive(Clone, Debug)]
pub(crate) struct Rtl2832u {
    iface: Interface,
}

impl Rtl2832u {
    pub(crate) fn new(iface: Interface) -> Self {
        Self { iface }
    }

    /// The claimed interface, for opening the bulk endpoint the samples arrive on.
    pub(crate) fn interface(&self) -> &Interface {
        &self.iface
    }

    /// Read a 1- or 2-byte USB/SYS register, decoded little-endian.
    pub(crate) fn read_reg(&self, block: u16, addr: u16, len: u16) -> Result<u16> {
        let data = self
            .iface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0,
                    value: addr,
                    index: block << 8,
                    length: len,
                },
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(Error::ControlTransfer)?;

        match *data.as_slice() {
            [low] => Ok(u16::from(low)),
            [low, high, ..] => Ok(u16::from_le_bytes([low, high])),
            [] => Err(Error::ShortResponse {
                what: "register read",
                got: 0,
            }),
        }
    }

    /// Write a 1- or 2-byte USB/SYS register, encoded big-endian.
    pub(crate) fn write_reg(&self, block: u16, addr: u16, val: u16, len: u8) -> Result<()> {
        let data = reg_bytes(val, len);
        self.iface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0,
                    value: addr,
                    index: (block << 8) | 0x10,
                    data: &data,
                },
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(Error::ControlTransfer)
    }

    /// Read one byte from a demodulator register.
    pub(crate) fn demod_read_reg(&self, page: u16, addr: u16) -> Result<u8> {
        let data = self
            .iface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0,
                    value: (addr << 8) | 0x20,
                    index: page,
                    length: 1,
                },
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(Error::ControlTransfer)?;

        data.first().copied().ok_or(Error::ShortResponse {
            what: "demod register read",
            got: 0,
        })
    }

    /// Write a 1- or 2-byte demodulator register.
    pub(crate) fn demod_write_reg(&self, page: u16, addr: u16, val: u16, len: u8) -> Result<()> {
        let data = reg_bytes(val, len);
        self.iface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0,
                    value: (addr << 8) | 0x20,
                    index: 0x10 | page,
                    data: &data,
                },
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(Error::ControlTransfer)?;

        // The RTL2832U needs a read on the demod bus to latch the write; without it, writes are
        // silently lost. librtlsdr uses this same dummy address and ignores its result.
        let _ = self.demod_read_reg(0x0a, 0x01);
        Ok(())
    }

    /// Write bytes to an I2C device through the RTL2832U's bridge.
    pub(crate) fn i2c_write(&self, i2c_addr: u8, data: &[u8]) -> Result<()> {
        self.iface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0,
                    value: u16::from(i2c_addr),
                    index: (BLOCK_IIC << 8) | 0x10,
                    data,
                },
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(Error::ControlTransfer)
    }

    /// Read bytes from an I2C device through the RTL2832U's bridge.
    pub(crate) fn i2c_read(&self, i2c_addr: u8, len: u16) -> Result<Vec<u8>> {
        self.iface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0,
                    value: u16::from(i2c_addr),
                    index: BLOCK_IIC << 8,
                    length: len,
                },
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(Error::ControlTransfer)
    }

    /// Read one I2C register: set the pointer, then read it back.
    pub(crate) fn i2c_read_reg(&self, i2c_addr: u8, reg: u8) -> Result<u8> {
        self.i2c_write(i2c_addr, &[reg])?;
        let data = self.i2c_read(i2c_addr, 1)?;
        data.first().copied().ok_or(Error::ShortResponse {
            what: "i2c register read",
            got: 0,
        })
    }

    /// Read the whole EEPROM, one byte per transfer — the bridge cannot do a longer burst.
    pub(crate) fn read_eeprom(&self) -> Result<[u8; EEPROM_SIZE]> {
        let mut buf = [0u8; EEPROM_SIZE];
        for (offset, byte) in buf.iter_mut().enumerate() {
            let address = u8::try_from(offset).map_err(|_| Error::ShortResponse {
                what: "eeprom offset",
                got: offset,
            })?;
            self.i2c_write(EEPROM_ADDR, &[address])?;
            let data = self.i2c_read(EEPROM_ADDR, 1)?;
            *byte = data.first().copied().ok_or(Error::ShortResponse {
                what: "eeprom read",
                got: 0,
            })?;
        }
        Ok(buf)
    }

    /// Open or close the I2C gate between the RTL2832U and the tuner. Every tuner access must
    /// be bracketed by these; leaving it open lets tuner traffic corrupt demod register writes.
    pub(crate) fn set_i2c_repeater(&self, enable: bool) -> Result<()> {
        self.demod_write_reg(1, 0x01, if enable { 0x18 } else { 0x10 }, 1)
    }

    /// Drive a GPIO pin as an output.
    pub(crate) fn set_gpio_output(&self, gpio: u8) -> Result<()> {
        let mask = 1u16 << gpio;
        let direction = self.read_reg(BLOCK_SYS, GPD, 1)?;
        self.write_reg(BLOCK_SYS, GPD, direction & !mask, 1)?;
        let enable = self.read_reg(BLOCK_SYS, GPOE, 1)?;
        self.write_reg(BLOCK_SYS, GPOE, enable | mask, 1)
    }

    /// Set or clear a GPIO output bit.
    pub(crate) fn set_gpio_bit(&self, gpio: u8, on: bool) -> Result<()> {
        let mask = 1u16 << gpio;
        let current = self.read_reg(BLOCK_SYS, GPO, 1)?;
        let value = if on { current | mask } else { current & !mask };
        self.write_reg(BLOCK_SYS, GPO, value, 1)
    }
}

/// Register payloads are big-endian, unlike the little-endian reads.
fn reg_bytes(val: u16, len: u8) -> Vec<u8> {
    if len == 1 {
        vec![val as u8]
    } else {
        val.to_be_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_writes_are_big_endian() {
        assert_eq!(reg_bytes(0x1002, 2), vec![0x10, 0x02]);
        assert_eq!(reg_bytes(0x00ab, 1), vec![0xab]);
        // A two-byte write of a value that fits in one byte must still send the high zero.
        assert_eq!(reg_bytes(0x0009, 2), vec![0x00, 0x09]);
    }
}
