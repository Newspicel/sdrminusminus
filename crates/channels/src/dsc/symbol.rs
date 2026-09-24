pub const ERASURE: i32 = -1;
pub const INFO_BITS: usize = 7;
pub const SYMBOL_BITS: usize = 10;
pub const LEADING_DX_PHASING: usize = 6;
pub const RX_DELAY: usize = 2;

pub fn decode_symbol(bits: &[u8; SYMBOL_BITS]) -> (u8, bool) {
    let value = bits[..INFO_BITS]
        .iter()
        .enumerate()
        .fold(0u8, |value, (index, &bit)| value | ((bit & 1) << index));
    let received_check = ((bits[7] & 1) << 2) | ((bits[8] & 1) << 1) | (bits[9] & 1);
    (value, received_check == zero_count(value))
}

pub fn symbol_at(bits: &[u8], start: usize) -> Option<(u8, bool)> {
    bits.get(start..)
        .and_then(<[u8]>::first_chunk::<SYMBOL_BITS>)
        .map(decode_symbol)
}

pub fn zero_count(value: u8) -> u8 {
    (0..INFO_BITS)
        .map(|bit| u8::from((value >> bit) & 1 == 0))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_symbol_values() {
        assert_eq!(decode_symbol(&[0, 1, 0, 0, 0, 0, 0, 1, 1, 0]).0, 2);
        assert_eq!(decode_symbol(&[0, 1, 0, 1, 1, 1, 1, 0, 1, 0]).0, 122);
        assert_eq!(decode_symbol(&[1, 1, 1, 1, 1, 1, 1, 0, 0, 0]).0, 127);
        assert_eq!(decode_symbol(&[1, 1, 0, 1, 0, 1, 0, 0, 1, 1]).0, 43);
    }

    #[test]
    fn reference_symbols_pass_check() {
        for bits in [
            [0, 1, 0, 0, 0, 0, 0, 1, 1, 0],
            [0, 1, 0, 1, 1, 1, 1, 0, 1, 0],
            [1, 1, 1, 1, 1, 1, 1, 0, 0, 0],
            [1, 1, 0, 1, 0, 1, 0, 0, 1, 1],
        ] {
            assert!(decode_symbol(&bits).1, "check failed for {bits:?}");
        }
    }

    #[test]
    fn corrupt_check_is_erasure() {
        assert!(!decode_symbol(&[0, 1, 0, 0, 0, 0, 0, 1, 0, 1]).1);
    }

    #[test]
    fn zero_count_matches_reference() {
        assert_eq!(zero_count(0b000_0010), 6);
        assert_eq!(zero_count(0b111_1010), 2);
        assert_eq!(zero_count(0b111_1111), 0);
        assert_eq!(zero_count(0b010_1011), 3);
    }

    #[test]
    fn short_input_has_no_symbol() {
        assert_eq!(symbol_at(&[1; 9], 0), None);
        assert_eq!(symbol_at(&[1; 12], 3), None);
        assert_eq!(
            symbol_at(&[1, 1, 1, 1, 1, 1, 1, 0, 0, 0], 0),
            Some((127, true))
        );
    }
}
