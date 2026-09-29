use super::{chip::Chip, error::Result, usb::Transport};

const ADDR: u8 = 0xa0;
const SIGNATURE: [u8; 2] = [0x28, 0x32];
const CONFIG: u8 = 0x07;
const IR_ENDPOINT: u8 = 0x02;

pub(crate) fn bias_tee_requested(signature: [u8; 2], config: u8) -> bool {
    signature == SIGNATURE && config & IR_ENDPOINT == 0
}

fn byte<T: Transport>(chip: &Chip<T>, offset: u8) -> Result<u8> {
    chip.i2c_register(ADDR, offset)
}

pub(crate) fn read_bias_tee<T: Transport>(chip: &Chip<T>) -> Result<bool> {
    let signature = [byte(chip, 0)?, byte(chip, 1)?];
    Ok(bias_tee_requested(signature, byte(chip, CONFIG)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dongle::fake::Fake;

    #[test]
    fn only_a_programmed_eeprom_with_the_ir_bit_clear_asks_for_the_bias_tee() {
        assert!(bias_tee_requested([0x28, 0x32], 0x00));
        assert!(!bias_tee_requested([0x28, 0x32], 0x02));
        assert!(!bias_tee_requested([0xff, 0xff], 0x00));
        assert!(!bias_tee_requested([0x00, 0x00], 0x00));
        assert!(bias_tee_requested([0x28, 0x32], 0x14));
    }

    #[test]
    fn the_flag_is_read_from_bytes_0_1_and_7() {
        let fake = Fake::default();
        fake.set_eeprom(0, 0x28);
        fake.set_eeprom(1, 0x32);
        fake.set_eeprom(7, 0x14);
        assert!(read_bias_tee(&Chip::new(fake.clone())).unwrap());
        assert_eq!(fake.i2c_writes(ADDR), [vec![0x00], vec![0x01], vec![0x07]]);
    }

    #[test]
    fn a_blank_eeprom_asks_for_nothing() {
        assert!(!read_bias_tee(&Chip::new(Fake::default())).unwrap());
    }
}
