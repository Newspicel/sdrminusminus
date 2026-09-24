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

pub fn decode_bitstream(bits: &[u8]) -> Vec<i32> {
    bits.as_chunks::<SYMBOL_BITS>()
        .0
        .iter()
        .map(|chunk| match decode_symbol(chunk) {
            (value, true) => i32::from(value),
            (_, false) => ERASURE,
        })
        .collect()
}

pub fn deinterleave_dx_rx(chars: &[i32], dx_skip: usize, rx_offset: usize) -> Vec<i32> {
    let dx: Vec<i32> = chars.iter().step_by(2).copied().collect();
    let rx: Vec<i32> = chars.iter().skip(1).step_by(2).copied().collect();
    dx.iter()
        .enumerate()
        .skip(dx_skip)
        .map(|(index, &symbol)| {
            if symbol == ERASURE {
                rx.get(index + rx_offset).copied().unwrap_or(ERASURE)
            } else {
                symbol
            }
        })
        .collect()
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
        assert_eq!(
            decode_bitstream(&[0, 1, 0, 0, 0, 0, 0, 1, 0, 1]),
            vec![ERASURE]
        );
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

    fn interleave(dx: &[i32], rx: &[i32]) -> Vec<i32> {
        dx.iter().zip(rx).flat_map(|(&d, &r)| [d, r]).collect()
    }

    #[test]
    fn dx_rx_recovers_erased_dx_from_rx() {
        let mut dx = [0i32; 12];
        dx[..6].fill(125);
        dx[6] = ERASURE;
        for (k, d) in dx.iter_mut().enumerate().skip(7) {
            *d = 70 + k as i32;
        }
        let mut rx = [0i32; 12];
        rx[8] = 66;
        let symbols = deinterleave_dx_rx(&interleave(&dx, &rx), 6, 2);
        assert_eq!(symbols, vec![66, 77, 78, 79, 80, 81]);
    }

    #[test]
    fn dx_rx_unrecoverable_is_erasure() {
        let mut dx = [125i32; 9];
        dx[6] = ERASURE;
        dx[7] = 99;
        dx[8] = 100;
        let mut rx = [0i32; 9];
        rx[8] = ERASURE;
        let symbols = deinterleave_dx_rx(&interleave(&dx, &rx), 6, 2);
        assert_eq!(symbols, vec![ERASURE, 99, 100]);
    }
}
