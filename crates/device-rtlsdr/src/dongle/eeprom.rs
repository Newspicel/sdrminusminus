use std::time::Duration;

use super::{
    chip::Chip,
    error::{EepromFault, Result},
    usb::Transport,
};

const ADDR: u8 = 0xa0;
const SIGNATURE: [u8; 2] = [0x28, 0x32];
const SERIAL_FLAG: usize = 6;
const HAS_SERIAL: u8 = 0xa5;
const CONFIG: u8 = 0x07;
const IR_ENDPOINT: u8 = 0x02;
const STRINGS: usize = 0x09;
const STRING_TYPE: u8 = 0x03;
const IMAGE_LEN: u8 = 78;
const SERIAL_MAX: usize = 16;
const SETTLE: Duration = Duration::from_millis(5);

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

pub(crate) fn valid_serial(serial: &str) -> bool {
    (1..=SERIAL_MAX).contains(&serial.len()) && serial.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn string_end(image: &[u8], at: usize) -> Result<usize, EepromFault> {
    let len = usize::from(*image.get(at).ok_or(EepromFault::Layout)?);
    let well_formed = len >= 2
        && len % 2 == 0
        && image.get(at + 1) == Some(&STRING_TYPE)
        && at + len <= image.len();
    well_formed.then_some(at + len).ok_or(EepromFault::Layout)
}

fn string_descriptor(text: &str) -> Result<Vec<u8>, EepromFault> {
    let len = u8::try_from(2 + 2 * text.len()).map_err(|_| EepromFault::TooLong)?;
    Ok([len, STRING_TYPE]
        .into_iter()
        .chain(text.bytes().flat_map(|b| [b, 0]))
        .collect())
}

pub(crate) fn with_serial(image: &[u8], serial: &str) -> Result<Vec<u8>, EepromFault> {
    if !valid_serial(serial) {
        return Err(EepromFault::Serial);
    }
    if image.get(..2) != Some(&SIGNATURE[..]) {
        return Err(EepromFault::Blank);
    }
    let product = string_end(image, STRINGS)?;
    let at = string_end(image, product)?;
    let descriptor = string_descriptor(serial)?;
    let end = at + descriptor.len();
    if end > image.len() {
        return Err(EepromFault::TooLong);
    }
    let mut next = image.to_vec();
    next[SERIAL_FLAG] = HAS_SERIAL;
    next[at..end].copy_from_slice(&descriptor);
    Ok(next)
}

fn read_image<T: Transport>(chip: &Chip<T>) -> Result<Vec<u8>> {
    (0..IMAGE_LEN).map(|offset| byte(chip, offset)).collect()
}

fn program<T: Transport>(chip: &Chip<T>, serial: &str, mut settle: impl FnMut()) -> Result<()> {
    let image = read_image(chip)?;
    let next = with_serial(&image, serial)?;
    let changed: Vec<(u8, u8)> = (0..IMAGE_LEN)
        .zip(image.iter().zip(&next))
        .filter(|(_, (old, new))| old != new)
        .map(|(offset, (_, new))| (offset, *new))
        .collect();
    for (offset, value) in &changed {
        chip.write_i2c(ADDR, &[*offset, *value])?;
        settle();
    }
    for (offset, value) in changed {
        if byte(chip, offset)? != value {
            return Err(EepromFault::Unverified(offset).into());
        }
    }
    Ok(())
}

pub(crate) fn write_serial<T: Transport>(chip: &Chip<T>, serial: &str) -> Result<()> {
    program(chip, serial, || std::thread::sleep(SETTLE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dongle::{error::Error, fake::Fake};

    const SERIAL_AT: usize = 53;

    fn factory_image() -> Vec<u8> {
        let mut image = vec![0x28, 0x32, 0xda, 0x0b, 0x38, 0x28, HAS_SERIAL, 0x14, 0x02];
        for text in ["Realtek", "RTL2838UHIDIR", "00000001"] {
            image.extend(string_descriptor(text).unwrap());
        }
        image.resize(usize::from(IMAGE_LEN), 0xff);
        image
    }

    fn factory() -> Fake {
        let fake = Fake::default();
        for (offset, value) in factory_image().into_iter().enumerate() {
            fake.set_eeprom(offset, value);
        }
        fake
    }

    fn serial_of(fake: &Fake) -> String {
        let image = fake.eeprom();
        let len = usize::from(image[SERIAL_AT]);
        image[SERIAL_AT + 2..SERIAL_AT + len]
            .iter()
            .step_by(2)
            .map(|b| char::from(*b))
            .collect()
    }

    fn eeprom_writes(fake: &Fake) -> Vec<Vec<u8>> {
        fake.i2c_writes(ADDR)
            .into_iter()
            .filter(|write| write.len() == 2)
            .collect()
    }

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

    #[test]
    fn the_serial_follows_the_maker_and_product_strings() {
        let fake = factory();
        program(&Chip::new(fake.clone()), "00000002", || {}).unwrap();
        assert_eq!(serial_of(&fake), "00000002");
        assert_eq!(fake.eeprom()[..SERIAL_AT], factory_image()[..SERIAL_AT]);
    }

    #[test]
    fn a_dongle_without_a_serial_gets_the_flag_set() {
        let fake = factory();
        fake.set_eeprom(SERIAL_FLAG, 0x00);
        program(&Chip::new(fake.clone()), "Roof", || {}).unwrap();
        assert_eq!(fake.eeprom()[SERIAL_FLAG], HAS_SERIAL);
        assert_eq!(serial_of(&fake), "Roof");
    }

    #[test]
    fn only_changed_bytes_are_written_each_followed_by_a_pause() {
        let fake = factory();
        let mut pauses = 0;
        program(&Chip::new(fake.clone()), "00000002", || pauses += 1).unwrap();
        assert_eq!(eeprom_writes(&fake), [vec![69, b'2']]);
        assert_eq!(pauses, 1);
    }

    #[test]
    fn the_same_serial_writes_nothing() {
        let fake = factory();
        program(&Chip::new(fake.clone()), "00000001", || {}).unwrap();
        assert!(eeprom_writes(&fake).is_empty());
    }

    #[test]
    fn a_write_protected_eeprom_is_reported() {
        let fake = factory();
        fake.protect_eeprom();
        let error = program(&Chip::new(fake), "00000002", || {}).unwrap_err();
        assert!(
            matches!(error, Error::Eeprom(EepromFault::Unverified(69))),
            "{error}"
        );
    }

    #[test]
    fn a_blank_eeprom_is_left_alone() {
        let fake = Fake::default();
        let error = program(&Chip::new(fake.clone()), "00000002", || {}).unwrap_err();
        assert!(
            matches!(error, Error::Eeprom(EepromFault::Blank)),
            "{error}"
        );
        assert!(eeprom_writes(&fake).is_empty());
    }

    #[test]
    fn broken_strings_are_not_overwritten() {
        let mut image = factory_image();
        image[STRINGS + 1] = 0x04;
        assert_eq!(with_serial(&image, "1"), Err(EepromFault::Layout));
        image[STRINGS + 1] = STRING_TYPE;
        image[STRINGS] = 0x0f;
        assert_eq!(with_serial(&image, "1"), Err(EepromFault::Layout));
    }

    #[test]
    fn a_serial_must_fit_the_string_area() {
        let image = factory_image();
        assert!(with_serial(&image, "ABCDEFGHIJK").is_ok());
        assert_eq!(
            with_serial(&image, "ABCDEFGHIJKL"),
            Err(EepromFault::TooLong)
        );
    }

    #[test]
    fn serials_are_short_letters_and_digits() {
        assert!(valid_serial("00000002"));
        assert!(valid_serial("Roof1"));
        assert!(!valid_serial(""));
        assert!(!valid_serial("with space"));
        assert!(!valid_serial("ÄÖÜ"));
        assert!(!valid_serial("12345678901234567"));
    }
}
