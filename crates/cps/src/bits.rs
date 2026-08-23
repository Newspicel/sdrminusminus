pub fn get_u8(data: &[u8], offset: usize) -> u8 {
    data.get(offset).copied().unwrap_or(0)
}

pub fn set_u8(data: &mut [u8], offset: usize, value: u8) {
    if let Some(slot) = data.get_mut(offset) {
        *slot = value;
    }
}

pub fn get_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([get_u8(data, offset), get_u8(data, offset + 1)])
}

pub fn set_u16_le(data: &mut [u8], offset: usize, value: u16) {
    let bytes = value.to_le_bytes();
    set_u8(data, offset, bytes[0]);
    set_u8(data, offset + 1, bytes[1]);
}

pub fn get_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        get_u8(data, offset),
        get_u8(data, offset + 1),
        get_u8(data, offset + 2),
        get_u8(data, offset + 3),
    ])
}

pub fn set_u32_le(data: &mut [u8], offset: usize, value: u32) {
    for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
        set_u8(data, offset + index, byte);
    }
}

pub fn get_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        get_u8(data, offset),
        get_u8(data, offset + 1),
        get_u8(data, offset + 2),
        get_u8(data, offset + 3),
    ])
}

pub fn set_u32_be(data: &mut [u8], offset: usize, value: u32) {
    for (index, byte) in value.to_be_bytes().into_iter().enumerate() {
        set_u8(data, offset + index, byte);
    }
}

pub fn get_bit(data: &[u8], offset: usize, bit: u8) -> bool {
    get_u8(data, offset) & (1 << bit) != 0
}

pub fn set_bit(data: &mut [u8], offset: usize, bit: u8, value: bool) {
    let mask = 1u8 << bit;
    let byte = get_u8(data, offset);
    set_u8(data, offset, if value { byte | mask } else { byte & !mask });
}

pub fn get_bits(data: &[u8], offset: usize, bit: u8, width: u8) -> u8 {
    let mask = (1u16 << width) - 1;
    ((u16::from(get_u8(data, offset)) >> bit) & mask) as u8
}

pub fn set_bits(data: &mut [u8], offset: usize, bit: u8, width: u8, value: u8) {
    let mask = (((1u16 << width) - 1) << bit) as u8;
    let byte = get_u8(data, offset);
    set_u8(data, offset, (byte & !mask) | ((value << bit) & mask));
}

fn from_bcd(raw: u32) -> u32 {
    let mut value = 0;
    let mut scale = 1;
    for nibble in 0..8 {
        value += ((raw >> (nibble * 4)) & 0xf) * scale;
        scale *= 10;
    }
    value
}

fn to_bcd(mut value: u32) -> u32 {
    let mut raw = 0;
    for nibble in 0..8 {
        raw |= (value % 10) << (nibble * 4);
        value /= 10;
    }
    raw
}

pub fn get_bcd8_be(data: &[u8], offset: usize) -> u32 {
    from_bcd(get_u32_be(data, offset))
}

pub fn set_bcd8_be(data: &mut [u8], offset: usize, value: u32) {
    set_u32_be(data, offset, to_bcd(value));
}

pub fn get_bcd8_le(data: &[u8], offset: usize) -> u32 {
    from_bcd(get_u32_le(data, offset))
}

pub fn set_bcd8_le(data: &mut [u8], offset: usize, value: u32) {
    set_u32_le(data, offset, to_bcd(value));
}

pub fn read_ascii(data: &[u8], offset: usize, max_len: usize, eos: u8) -> String {
    let mut text = String::with_capacity(max_len);
    for index in 0..max_len {
        let byte = get_u8(data, offset + index);
        if byte == 0 || byte == eos {
            break;
        }
        text.push(char::from(byte));
    }
    text.trim().to_owned()
}

pub fn write_ascii(data: &mut [u8], offset: usize, text: &str, max_len: usize, eos: u8) {
    let mut written = 0;
    for byte in text
        .chars()
        .filter_map(|c| u8::try_from(u32::from(c)).ok())
        .filter(|byte| *byte >= 0x20)
        .take(max_len)
    {
        set_u8(data, offset + written, byte);
        written += 1;
    }
    if written < max_len {
        set_u8(data, offset + written, eos);
    }
}

pub fn read_utf16(data: &[u8], offset: usize, max_units: usize) -> String {
    let mut units = Vec::with_capacity(max_units);
    for index in 0..max_units {
        let unit = get_u16_le(data, offset + index * 2);
        if unit == 0 || unit == 0xffff {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units).trim().to_owned()
}

pub fn write_utf16(data: &mut [u8], offset: usize, text: &str, max_units: usize) {
    let mut written = 0;
    for unit in text.encode_utf16().take(max_units) {
        set_u16_le(data, offset + written * 2, unit);
        written += 1;
    }
    if written < max_units {
        set_u16_le(data, offset + written * 2, 0);
    }
}

pub fn is_erased(data: &[u8]) -> bool {
    data.iter().all(|byte| *byte == 0xff) || data.iter().all(|byte| *byte == 0x00)
}

pub fn is_blank(data: &[u8]) -> bool {
    data.iter().all(|byte| *byte == 0x00 || *byte == 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcd_round_trips_through_both_byte_orders() {
        let mut buffer = [0u8; 4];
        set_bcd8_be(&mut buffer, 0, 43_912_500);
        assert_eq!(buffer, [0x43, 0x91, 0x25, 0x00]);
        assert_eq!(get_bcd8_be(&buffer, 0), 43_912_500);

        set_bcd8_le(&mut buffer, 0, 1_234_567);
        assert_eq!(get_bcd8_le(&buffer, 0), 1_234_567);
    }

    #[test]
    fn bit_fields_stay_inside_their_own_width() {
        let mut buffer = [0xffu8; 1];
        set_bits(&mut buffer, 0, 4, 2, 0b01);
        assert_eq!(buffer[0], 0b1101_1111);
        assert_eq!(get_bits(&buffer, 0, 4, 2), 0b01);
        set_bit(&mut buffer, 0, 0, false);
        assert!(!get_bit(&buffer, 0, 0));
        assert!(get_bit(&buffer, 0, 1));
    }

    #[test]
    fn text_helpers_pad_and_stop_at_the_terminator() {
        let mut buffer = [0u8; 8];
        buffer.fill(0x41);
        write_ascii(&mut buffer, 0, "VHF", 8, 0xff);
        assert_eq!(&buffer[..3], b"VHF");
        assert_eq!(buffer[3], 0xff);
        assert_eq!(buffer[4], 0x41, "only one terminator is written");
        assert_eq!(read_ascii(&buffer, 0, 8, 0xff), "VHF");

        let mut wide = [0u8; 16];
        write_utf16(&mut wide, 0, "OE1XYZ", 8);
        assert_eq!(read_utf16(&wide, 0, 8), "OE1XYZ");
    }

    #[test]
    fn a_half_erased_slot_is_blank_without_being_uniformly_erased() {
        assert!(is_erased(&[0x00, 0x00]));
        assert!(is_erased(&[0xff, 0xff]));
        assert!(!is_erased(&[0x00, 0xff]));
        assert!(is_blank(&[0x00, 0xff]));
        assert!(!is_blank(&[0x00, 0x41]));
    }

    #[test]
    fn accessors_past_the_end_do_not_panic() {
        let mut buffer = [0u8; 2];
        assert_eq!(get_u32_le(&buffer, 0), 0);
        set_u32_le(&mut buffer, 0, 0xdead_beef);
        assert_eq!(buffer, [0xef, 0xbe]);
    }
}
