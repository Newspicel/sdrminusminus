pub const ACCESS_DL: &[u8; 24] = &[
    0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1,
];
pub const ACCESS_UL: &[u8; 24] = &[
    1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1, 0, 0,
];

pub const HEADER_MESSAGING: &[u8; 32] = &[
    0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1,
];

pub const RINGALERT_BCH_POLY: u32 = 1207;
pub const MESSAGING_BCH_POLY: u32 = 1897;
pub const HDR_POLY: u32 = 29;
pub const LCW2_POLY: u32 = 465;
pub const LCW3_POLY: u32 = 41;

const CHASE_BITS: usize = 5;
const FILL_A: u32 = 0b1010_0010_0111_0011_1011_1111_0110_1101;
const FILL_B: u32 = 0b0101_0100_0100_0101_1100_0010_1110_0110;

pub const LCW_TABLE: [usize; 46] = [
    40, 39, 36, 35, 32, 31, 28, 27, 24, 23, 20, 19, 16, 15, 12, 11, 8, 7, 4, 3, 41, 38, 37, 34, 33,
    30, 29, 26, 25, 22, 21, 18, 17, 14, 13, 10, 9, 6, 5, 2, 1, 46, 45, 44, 43, 42,
];

pub fn bits_to_u32(bits: &[u8]) -> u32 {
    bits.iter()
        .fold(0u32, |value, &bit| (value << 1) | u32::from(bit))
}

pub fn bits_to_u8(bits: &[u8]) -> u8 {
    bits.iter().fold(0u8, |value, &bit| (value << 1) | bit)
}

pub fn ndivide(poly: u32, bits: &[u8]) -> u32 {
    let num = bits
        .iter()
        .fold(0u64, |value, &bit| (value << 1) | u64::from(bit));
    let poly_bits = 32 - poly.leading_zeros();
    let len = bits.len() as u32;
    if len < poly_bits {
        return num as u32;
    }
    let mut remainder = num;
    for shift in (0..=len - poly_bits).rev() {
        if (remainder >> (shift + poly_bits - 1)) & 1 == 1 {
            remainder ^= u64::from(poly) << shift;
        }
    }
    remainder as u32
}

pub fn bch_repair_soft(poly: u32, block: &[u8], reliability: &[f32]) -> Option<Vec<u8>> {
    let mut order: Vec<usize> = (0..block.len()).collect();
    order.sort_by(|&a, &b| reliability[a].total_cmp(&reliability[b]));
    let weakest = &order[..CHASE_BITS.min(order.len())];
    let mut best: Option<(f32, Vec<u8>)> = None;
    for mask in 0u32..(1 << weakest.len()) {
        let mut candidate = block.to_vec();
        for (k, &position) in weakest.iter().enumerate() {
            candidate[position] ^= ((mask >> k) & 1) as u8;
        }
        if bch_repair(poly, &mut candidate).is_none() {
            continue;
        }
        let distance: f32 = candidate
            .iter()
            .zip(block)
            .zip(reliability)
            .filter(|((a, b), _)| a != b)
            .map(|(_, r)| r)
            .sum();
        if best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, codeword)| codeword)
}

pub fn bch_repair(poly: u32, block: &mut [u8]) -> Option<u32> {
    if ndivide(poly, block) == 0 {
        return Some(0);
    }
    for i in 0..block.len() {
        block[i] ^= 1;
        if ndivide(poly, block) == 0 {
            return Some(1);
        }
        for j in i + 1..block.len() {
            block[j] ^= 1;
            if ndivide(poly, block) == 0 {
                return Some(2);
            }
            block[j] ^= 1;
        }
        block[i] ^= 1;
    }
    None
}

#[cfg(test)]
pub fn symbol_reverse(bits: &[u8]) -> Vec<u8> {
    let mut out = bits.to_vec();
    for pair in out.as_chunks_mut::<2>().0.iter_mut() {
        pair.swap(0, 1);
    }
    out
}

fn swapped_symbols<T: Copy>(group: &[T]) -> Vec<[T; 2]> {
    group
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| [pair[1], pair[0]])
        .collect()
}

fn every_nth_backwards<T: Copy>(symbols: &[[T; 2]], start: isize, step: usize) -> Vec<T> {
    let mut out = Vec::with_capacity(2 * symbols.len() / step + 2);
    let mut index = start;
    while index >= 0 {
        out.extend_from_slice(&symbols[index as usize]);
        index -= step as isize;
    }
    out
}

pub fn de_interleave2<T: Copy>(group: &[T]) -> (Vec<T>, Vec<T>) {
    let symbols = swapped_symbols(group);
    let n = symbols.len() as isize;
    (
        every_nth_backwards(&symbols, n - 1, 2),
        every_nth_backwards(&symbols, n - 2, 2),
    )
}

pub fn de_interleave3(group: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let symbols = swapped_symbols(group);
    let n = symbols.len() as isize;
    (
        every_nth_backwards(&symbols, n - 1, 3),
        every_nth_backwards(&symbols, n - 2, 3),
        every_nth_backwards(&symbols, n - 3, 3),
    )
}

pub fn ecc_blocks(blocks: &[Vec<u8>], poly: u32) -> (Vec<u8>, u32) {
    let mut data = Vec::new();
    let mut fixed = 0u32;
    for block in blocks {
        if block.len() != 32 {
            break;
        }
        let mut repaired: Vec<u8> = block[..31].to_vec();
        let Some(errors) = bch_repair(poly, &mut repaired) else {
            break;
        };
        let ones = repaired.iter().map(|&bit| u32::from(bit)).sum::<u32>() + u32::from(block[31]);
        if ones % 2 == 1 && errors >= 2 {
            break;
        }
        if errors > 0 {
            fixed += 1;
        }
        data.extend_from_slice(&repaired[..21]);
    }
    (data, fixed)
}

pub fn strip_fill(blocks: &mut Vec<Vec<u8>>) {
    while blocks.len() >= 2 {
        let a = bits_to_u32(&blocks[blocks.len() - 2]);
        let b = bits_to_u32(&blocks[blocks.len() - 1]);
        if (a ^ FILL_A).count_ones() > 2 || (b ^ FILL_B).count_ones() > 2 {
            break;
        }
        blocks.truncate(blocks.len() - 2);
    }
}

pub fn pair_blocks(data: &[u8]) -> Vec<Vec<u8>> {
    let mut blocks = Vec::with_capacity(data.len() / 32);
    for chunk in data.as_chunks::<64>().0.iter() {
        let (odd, even) = de_interleave2(chunk);
        blocks.push(odd);
        blocks.push(even);
    }
    blocks
}

pub fn ra_blocks(data: &[u8]) -> Vec<Vec<u8>> {
    let (b1, b2, b3) = de_interleave3(&data[..96]);
    let mut blocks = vec![b1, b2, b3];
    blocks.extend(pair_blocks(&data[96..]));
    blocks
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Ra,
    Bc,
    Ms,
    Itl,
    Lw,
    Unknown,
}

pub fn classify(data: &[u8]) -> FrameKind {
    if data.len() >= 32 && data[..32] == HEADER_MESSAGING[..] {
        FrameKind::Ms
    } else if is_itl(data) {
        FrameKind::Itl
    } else if is_bc(data) {
        FrameKind::Bc
    } else if is_lcw(data) {
        FrameKind::Lw
    } else if is_ra(data) {
        FrameKind::Ra
    } else {
        FrameKind::Unknown
    }
}

fn is_itl(data: &[u8]) -> bool {
    if data.len() < 96 {
        return false;
    }
    let diff = u32::from(data[0] == 0)
        + u32::from(data[1] == 0)
        + data[2..96].iter().map(|&bit| u32::from(bit)).sum::<u32>();
    diff <= 3
}

fn is_bc(data: &[u8]) -> bool {
    if data.len() <= 6 + 64 || ndivide(HDR_POLY, &data[..6]) != 0 {
        return false;
    }
    let (b1, b2) = de_interleave2(&data[6..6 + 64]);
    ndivide(RINGALERT_BCH_POLY, &b1[..31]) == 0 && ndivide(RINGALERT_BCH_POLY, &b2[..31]) == 0
}

pub fn lcw_bits(data: &[u8]) -> Vec<u8> {
    LCW_TABLE.iter().map(|&index| data[index - 1]).collect()
}

fn is_lcw(data: &[u8]) -> bool {
    if data.len() <= 64 {
        return false;
    }
    let lcw = lcw_bits(data);
    if ndivide(HDR_POLY, &lcw[..7]) != 0 || ndivide(LCW3_POLY, &lcw[20..]) != 0 {
        return false;
    }
    [0u8, 1].iter().any(|&missing| {
        let mut completed = lcw[7..20].to_vec();
        completed.push(missing);
        ndivide(LCW2_POLY, &completed) == 0
    })
}

fn is_ra(data: &[u8]) -> bool {
    if data.len() < 96 {
        return false;
    }
    let (b1, b2, b3) = de_interleave3(&data[..96]);
    let mut clean = 0u32;
    let mut errors = 0u32;
    for mut block in [b1, b2, b3] {
        if ndivide(RINGALERT_BCH_POLY, &block[..31]) == 0 {
            clean += 1;
        }
        let Some(fixed) = bch_repair(RINGALERT_BCH_POLY, &mut block[..31]) else {
            return false;
        };
        errors += fixed;
    }
    clean >= 1 && errors <= 3
}

#[cfg(test)]
mod tests {
    use super::super::encode::{bch_encode, interleave2, interleave3};
    use super::*;

    pub fn rand_bits(n: usize, mut s: u64) -> Vec<u8> {
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((s >> 33) & 1) as u8
            })
            .collect()
    }

    #[test]
    fn interleave_roundtrips() {
        let odd = rand_bits(32, 1);
        let even = rand_bits(32, 2);
        let (o, e) = de_interleave2(&interleave2(&odd, &even));
        assert_eq!(o, odd);
        assert_eq!(e, even);
        let b1 = rand_bits(32, 3);
        let b2 = rand_bits(32, 4);
        let b3 = rand_bits(32, 5);
        let (r1, r2, r3) = de_interleave3(&interleave3(&b1, &b2, &b3));
        assert_eq!(r1, b1);
        assert_eq!(r2, b2);
        assert_eq!(r3, b3);
    }

    #[test]
    fn bch_roundtrip_and_repair() {
        let data = rand_bits(21, 7);
        let block = bch_encode(RINGALERT_BCH_POLY, &data);
        assert_eq!(block.len(), 32);
        assert_eq!(ndivide(RINGALERT_BCH_POLY, &block[..31]), 0);
        let mut b31: Vec<u8> = block[..31].to_vec();
        b31[3] ^= 1;
        b31[17] ^= 1;
        assert_eq!(bch_repair(RINGALERT_BCH_POLY, &mut b31), Some(2));
        assert_eq!(&b31[..21], &data[..]);
    }

    #[test]
    fn itl_header_classifies_as_itl_not_ra() {
        let mut data = vec![0u8; 96 + 64];
        data[0] = 1;
        data[1] = 1;
        for (i, bit) in data.iter_mut().enumerate().skip(96) {
            *bit = u8::from((i * 7) % 3 == 0);
        }
        assert_eq!(classify(&data), FrameKind::Itl);
        let mut noisy = data.clone();
        noisy[40] = 1;
        noisy[71] = 1;
        assert_eq!(classify(&noisy), FrameKind::Itl);
        assert_ne!(classify(&[0u8; 96 + 64]), FrameKind::Ra);
    }

    #[test]
    fn ira_header_still_classifies_as_ra() {
        let b1 = bch_encode(RINGALERT_BCH_POLY, &rand_bits(21, 101));
        let b2 = bch_encode(RINGALERT_BCH_POLY, &rand_bits(21, 102));
        let b3 = bch_encode(RINGALERT_BCH_POLY, &rand_bits(21, 103));
        let mut data = interleave3(&b1, &b2, &b3);
        data.extend([0u8; 64]);
        assert_eq!(classify(&data), FrameKind::Ra);
    }

    #[test]
    fn bch_min_distance_is_5() {
        let weight = |data: &[u8]| -> u32 {
            bch_encode(RINGALERT_BCH_POLY, data)
                .iter()
                .take(31)
                .map(|&b| u32::from(b))
                .sum()
        };
        let mut min = u32::MAX;
        for i in 0..21usize {
            let mut single = vec![0u8; 21];
            single[i] = 1;
            min = min.min(weight(&single));
            for j in i + 1..21 {
                let mut double = single.clone();
                double[j] = 1;
                min = min.min(weight(&double));
            }
        }
        assert_eq!(min, 5);
    }

    #[test]
    fn ecc_blocks_tolerates_flipped_parity_bit() {
        let mut blocks = vec![
            bch_encode(RINGALERT_BCH_POLY, &rand_bits(21, 11)),
            bch_encode(RINGALERT_BCH_POLY, &rand_bits(21, 12)),
            bch_encode(RINGALERT_BCH_POLY, &rand_bits(21, 13)),
        ];
        blocks[2][5] ^= 1;
        blocks[2][31] ^= 1;
        let (payload, fixed) = ecc_blocks(&blocks, RINGALERT_BCH_POLY);
        assert_eq!(payload.len(), 63);
        assert!(fixed >= 1);
    }

    #[test]
    fn ecc_blocks_stops_at_a_three_error_block() {
        let mut blocks = vec![
            bch_encode(RINGALERT_BCH_POLY, &rand_bits(21, 21)),
            bch_encode(RINGALERT_BCH_POLY, &rand_bits(21, 22)),
        ];
        for position in [4, 13, 27] {
            blocks[1][position] ^= 1;
        }
        let (payload, _) = ecc_blocks(&blocks, RINGALERT_BCH_POLY);
        assert!(payload.len() <= 42);
        assert_eq!(&payload[..21], &rand_bits(21, 21)[..]);
    }
}
