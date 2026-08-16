pub mod block;
pub mod bptc;
pub mod conv;

const CCITT_POLY_REFLECTED: u16 = 0x8408;

fn crc16_reflected(init: u16, data: &[u8]) -> u16 {
    let mut crc = init;
    for &byte in data {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            let lsb = crc & 1;
            crc >>= 1;
            if lsb != 0 {
                crc ^= CCITT_POLY_REFLECTED;
            }
        }
    }
    crc
}

#[must_use]
pub fn crc16_x25(data: &[u8]) -> u16 {
    !crc16_reflected(0xFFFF, data)
}

#[must_use]
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    crc16_reflected(0, data)
}

#[must_use]
pub fn crc16_msb_bits(poly: u16, init: u16, bits: &[bool]) -> u16 {
    let mut crc = init;
    for &bit in bits {
        let msb = crc & 0x8000 != 0;
        crc <<= 1;
        if msb != bit {
            crc ^= poly;
        }
    }
    crc
}

#[must_use]
pub fn crc16_msb(poly: u16, init: u16, data: &[u8]) -> u16 {
    let mut crc = init;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            let msb = crc & 0x8000 != 0;
            crc <<= 1;
            if msb {
                crc ^= poly;
            }
        }
    }
    crc
}

#[must_use]
pub fn rs129_parity(msg: &[u8]) -> [u8; 3] {
    const POLY: [u8; 3] = [64, 56, 14];
    let mut parity = [0u8; 3];
    for &byte in msg {
        let feedback = byte ^ parity[2];
        parity[2] = parity[1] ^ gf256_mul(POLY[2], feedback);
        parity[1] = parity[0] ^ gf256_mul(POLY[1], feedback);
        parity[0] = gf256_mul(POLY[0], feedback);
    }
    [parity[2], parity[1], parity[0]]
}

#[must_use]
pub fn rs64_encode(data: &[u8], parity_symbols: usize) -> Vec<u8> {
    let mut generator = vec![1u8];
    for root in 1..=parity_symbols {
        let mut next = vec![0u8; generator.len() + 1];
        for (index, &coefficient) in generator.iter().enumerate() {
            next[index] ^= coefficient;
            next[index + 1] ^= gf64_mul(coefficient, gf64_pow(2, root as i32));
        }
        generator = next;
    }
    let mut codeword: Vec<u8> = data.iter().map(|symbol| symbol & 0x3F).collect();
    codeword.resize(data.len() + parity_symbols, 0);
    for index in 0..data.len() {
        let coefficient = codeword[index];
        if coefficient == 0 {
            continue;
        }
        for (offset, &factor) in generator.iter().enumerate() {
            codeword[index + offset] ^= gf64_mul(factor, coefficient);
        }
    }
    codeword[..data.len()].copy_from_slice(data);
    codeword
}

#[must_use]
pub fn rs64_decode(codeword: &[u8], data_symbols: usize) -> Option<(Vec<u8>, u32)> {
    let parity = codeword.len().checked_sub(data_symbols)?;
    if parity == 0 || parity >= 32 || codeword.len() > 63 {
        return None;
    }
    let mut corrected: Vec<u8> = codeword.iter().map(|symbol| symbol & 0x3F).collect();
    let mut syndromes = rs64_syndromes(&corrected, parity);
    if syndromes.iter().all(|&value| value == 0) {
        return Some((corrected[..data_symbols].to_vec(), 0));
    }

    let mut locator = vec![0u8; parity + 1];
    let mut previous = vec![0u8; parity + 1];
    locator[0] = 1;
    previous[0] = 1;
    let (mut degree, mut shift, mut discrepancy_at_update) = (0usize, 1usize, 1u8);
    for n in 0..parity {
        let mut discrepancy = syndromes[n];
        for i in 1..=degree {
            discrepancy ^= gf64_mul(locator[i], syndromes[n - i]);
        }
        if discrepancy == 0 {
            shift += 1;
            continue;
        }
        let saved = locator.clone();
        let scale = gf64_mul(discrepancy, gf64_inv(discrepancy_at_update)?);
        for i in 0..=parity - shift {
            locator[i + shift] ^= gf64_mul(scale, previous[i]);
        }
        if 2 * degree <= n {
            degree = n + 1 - degree;
            previous = saved;
            discrepancy_at_update = discrepancy;
            shift = 1;
        } else {
            shift += 1;
        }
    }
    if degree == 0 || degree > parity / 2 {
        return None;
    }

    let mut positions = Vec::with_capacity(degree);
    let mut powers = Vec::with_capacity(degree);
    for position in 0..corrected.len() {
        let power = corrected.len() - 1 - position;
        let root = gf64_pow(2, -(power as i32));
        if gf64_eval_ascending(&locator[..=degree], root) == 0 {
            positions.push(position);
            powers.push(power);
        }
    }
    if positions.len() != degree {
        return None;
    }

    let mut matrix = vec![vec![0u8; degree + 1]; degree];
    for row in 0..degree {
        for (column, &power) in powers.iter().enumerate() {
            matrix[row][column] = gf64_pow(2, ((row + 1) * power) as i32);
        }
        matrix[row][degree] = syndromes[row];
    }
    for column in 0..degree {
        let pivot = (column..degree).find(|&row| matrix[row][column] != 0)?;
        matrix.swap(column, pivot);
        let inverse = gf64_inv(matrix[column][column])?;
        for value in &mut matrix[column][column..=degree] {
            *value = gf64_mul(*value, inverse);
        }
        let pivot_row = matrix[column][column..=degree].to_vec();
        for (row, target_row) in matrix.iter_mut().enumerate() {
            if row == column {
                continue;
            }
            let scale = target_row[column];
            for (value, &pivot_value) in target_row[column..=degree].iter_mut().zip(&pivot_row) {
                *value ^= gf64_mul(scale, pivot_value);
            }
        }
    }
    for (row, &position) in positions.iter().enumerate() {
        corrected[position] ^= matrix[row][degree];
    }
    syndromes = rs64_syndromes(&corrected, parity);
    if syndromes.iter().any(|&value| value != 0) {
        return None;
    }
    Some((corrected[..data_symbols].to_vec(), degree as u32))
}

fn rs64_syndromes(codeword: &[u8], count: usize) -> Vec<u8> {
    (1..=count)
        .map(|root| {
            let x = gf64_pow(2, root as i32);
            codeword
                .iter()
                .fold(0, |value, &symbol| gf64_mul(value, x) ^ symbol)
        })
        .collect()
}

fn gf64_eval_ascending(poly: &[u8], x: u8) -> u8 {
    poly.iter()
        .rev()
        .fold(0, |value, &coefficient| gf64_mul(value, x) ^ coefficient)
}

fn gf64_mul(a: u8, b: u8) -> u8 {
    let (mut a, mut b, mut product) = (a & 0x3F, b & 0x3F, 0u8);
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        let high = a & 0x20 != 0;
        a <<= 1;
        if high {
            a ^= 0x43;
        }
        b >>= 1;
    }
    product & 0x3F
}

fn gf64_pow(base: u8, exponent: i32) -> u8 {
    let exponent = exponent.rem_euclid(63);
    (0..exponent).fold(1, |value, _| gf64_mul(value, base))
}

fn gf64_inv(value: u8) -> Option<u8> {
    (value != 0).then(|| gf64_pow(value, 62))
}

fn gf256_mul(a: u8, b: u8) -> u8 {
    let (mut a, mut b, mut product) = (a, b, 0u8);
    while b != 0 {
        if b & 1 == 1 {
            product ^= a;
        }
        let high = a & 0x80 != 0;
        a <<= 1;
        if high {
            a ^= 0x1D;
        }
        b >>= 1;
    }
    product
}

#[must_use]
pub fn hdlc_fcs_ok(frame: &[u8]) -> bool {
    if frame.len() < 3 {
        return false;
    }
    let (payload, fcs) = frame.split_at(frame.len() - 2);
    let [lo, hi] = fcs else { return false };
    crc16_x25(payload) == u16::from_le_bytes([*lo, *hi])
}

const MODE_S_POLY: u32 = 0x00FF_F409;
const MODE_S_MASK: u32 = 0x00FF_FFFF;
const MODE_S_SHORT_LEN: usize = 7;
const MODE_S_LONG_LEN: usize = 14;
const MODE_S_PARITY_LEN: usize = 3;

fn mode_s_step(reg: u32) -> u32 {
    if reg & 0x0080_0000 != 0 {
        ((reg << 1) ^ MODE_S_POLY) & MODE_S_MASK
    } else {
        (reg << 1) & MODE_S_MASK
    }
}

fn mode_s_crc(data: &[u8]) -> u32 {
    let mut reg = 0;
    for &byte in data {
        reg ^= u32::from(byte) << 16;
        for _ in 0..8 {
            reg = mode_s_step(reg);
        }
    }
    reg
}

#[must_use]
pub fn mode_s_syndrome(frame: &[u8]) -> u32 {
    if frame.len() != MODE_S_SHORT_LEN && frame.len() != MODE_S_LONG_LEN {
        return u32::MAX;
    }
    mode_s_crc(frame)
}

#[must_use]
pub fn mode_s_overlay(frame: &[u8]) -> Option<u32> {
    if frame.len() != MODE_S_SHORT_LEN && frame.len() != MODE_S_LONG_LEN {
        return None;
    }
    let (body, field) = frame.split_at(frame.len() - MODE_S_PARITY_LEN);
    let &[hi, mid, lo] = field else { return None };
    let parity = u32::from(hi) << 16 | u32::from(mid) << 8 | u32::from(lo);
    Some(mode_s_crc(body) ^ parity)
}

pub fn mode_s_append_parity(body: &mut Vec<u8>) {
    mode_s_append_overlaid_parity(body, 0);
}

pub fn mode_s_append_overlaid_parity(body: &mut Vec<u8>, overlay: u32) {
    assert!(
        body.len() == MODE_S_SHORT_LEN - MODE_S_PARITY_LEN
            || body.len() == MODE_S_LONG_LEN - MODE_S_PARITY_LEN,
        "mode s message body must be 4 or 11 bytes, got {}",
        body.len()
    );
    let parity = mode_s_crc(body) ^ (overlay & MODE_S_MASK);
    body.extend_from_slice(&[(parity >> 16) as u8, (parity >> 8) as u8, parity as u8]);
}

pub fn mode_s_fix_single_bit(frame: &mut [u8]) -> Option<usize> {
    let syndrome = mode_s_syndrome(frame);
    if syndrome == 0 {
        return None;
    }
    let bits = frame.len() * 8;
    let mut trial = MODE_S_POLY;
    for i in (0..bits).rev() {
        if trial == syndrome {
            frame[i / 8] ^= 0x80 >> (i % 8);
            return Some(i);
        }
        trial = mode_s_step(trial);
    }
    None
}

const GOLAY23_POLY: u32 = 0xC75;
const GOLAY23_CODE_BITS: u32 = 23;
const GOLAY23_DATA_BITS: u32 = 12;
const GOLAY23_CHECK_BITS: u32 = GOLAY23_CODE_BITS - GOLAY23_DATA_BITS;

const fn golay23_remainder(word: u32) -> u32 {
    let mut rem = word & ((1 << GOLAY23_CODE_BITS) - 1);
    let mut shift = GOLAY23_CODE_BITS;
    while shift > GOLAY23_CHECK_BITS {
        shift -= 1;
        if rem & (1 << shift) != 0 {
            rem ^= GOLAY23_POLY << (shift - GOLAY23_CHECK_BITS);
        }
    }
    rem
}

#[must_use]
pub const fn golay23_encode(data: u16) -> u32 {
    let payload = (data as u32 & 0x0FFF) << GOLAY23_CHECK_BITS;
    payload | golay23_remainder(payload)
}

#[must_use]
pub const fn golay23_ok(word: u32) -> bool {
    golay23_remainder(word) == 0
}

#[must_use]
pub fn golay23_correct(word: u32) -> Option<(u32, u32)> {
    let word = word & 0x7F_FFFF;
    if golay23_ok(word) {
        return Some((word, 0));
    }
    for first in 0..23 {
        let one = word ^ 1 << first;
        if golay23_ok(one) {
            return Some((one, 1));
        }
        for second in first + 1..23 {
            let two = one ^ 1 << second;
            if golay23_ok(two) {
                return Some((two, 2));
            }
            for third in second + 1..23 {
                let three = two ^ 1 << third;
                if golay23_ok(three) {
                    return Some((three, 3));
                }
            }
        }
    }
    None
}

const POCSAG_GEN: u32 = 0x769;
const POCSAG_CODE_BITS: u32 = 31;
const POCSAG_DATA_BITS: u32 = 21;

const fn pocsag_syndrome(field: u32) -> u32 {
    let mut rem = field & ((1 << POCSAG_CODE_BITS) - 1);
    let mut shift = POCSAG_CODE_BITS;
    while shift > 10 {
        shift -= 1;
        if rem & (1 << shift) != 0 {
            rem ^= POCSAG_GEN << (shift - 10);
        }
    }
    rem
}

const POCSAG_BIT_SYNDROMES: [u32; 31] = {
    let mut table = [0; 31];
    let mut i = 0;
    while i < table.len() {
        table[i] = pocsag_syndrome(1 << i);
        i += 1;
    }
    table
};

fn pocsag_find_bit(syndrome: u32) -> Option<usize> {
    POCSAG_BIT_SYNDROMES.iter().position(|s| *s == syndrome)
}

fn pocsag_find_pair(syndrome: u32) -> Option<(usize, usize)> {
    for (a, syn_a) in POCSAG_BIT_SYNDROMES.iter().enumerate() {
        if let Some(b) = pocsag_find_bit(syndrome ^ syn_a)
            && b > a
        {
            return Some((a, b));
        }
    }
    None
}

fn pocsag_parity(word: u32) -> u32 {
    word.count_ones() & 1
}

#[must_use]
pub fn pocsag_bch_decode(word: u32) -> Option<(u32, u32)> {
    let syndrome = pocsag_syndrome(word >> 1);
    let odd = pocsag_parity(word) == 1;
    if syndrome == 0 {
        return Some(if odd { (word ^ 1, 1) } else { (word, 0) });
    }
    if let Some(bit) = pocsag_find_bit(syndrome) {
        let fix = (1 << (bit + 1)) | u32::from(!odd);
        return Some((word ^ fix, if odd { 1 } else { 2 }));
    }
    if odd {
        return None;
    }
    let (a, b) = pocsag_find_pair(syndrome)?;
    Some((word ^ (1 << (a + 1)) ^ (1 << (b + 1)), 2))
}

#[must_use]
pub fn pocsag_bch_encode(word21: u32) -> u32 {
    assert!(
        (word21 >> POCSAG_DATA_BITS) == 0,
        "pocsag codeword carries 21 leading bits"
    );
    let field = word21 << 10;
    let word = (field | pocsag_syndrome(field)) << 1;
    word | pocsag_parity(word)
}

const ERMES_GEN: u32 = 0o15315;
const ERMES_CODE_BITS: u32 = 30;
const ERMES_DATA_BITS: u32 = 18;
const ERMES_CHECK_BITS: u32 = ERMES_CODE_BITS - ERMES_DATA_BITS;

const fn ermes_syndrome(mut word: u32) -> u32 {
    let mut bit = ERMES_CODE_BITS;
    while bit > ERMES_CHECK_BITS {
        bit -= 1;
        if word & (1 << bit) != 0 {
            word ^= ERMES_GEN << (bit - ERMES_CHECK_BITS);
        }
    }
    word & ((1 << ERMES_CHECK_BITS) - 1)
}

const ERMES_BIT_SYNDROMES: [u32; ERMES_CODE_BITS as usize] = {
    let mut table = [0; ERMES_CODE_BITS as usize];
    let mut i = 0;
    while i < table.len() {
        table[i] = ermes_syndrome(1 << i);
        i += 1;
    }
    table
};

#[must_use]
pub fn ermes_bch_decode(word: u32) -> Option<(u32, u32)> {
    let word = word & ((1 << ERMES_CODE_BITS) - 1);
    let syndrome = ermes_syndrome(word);
    if syndrome == 0 {
        return Some((word, 0));
    }
    if let Some(bit) = ERMES_BIT_SYNDROMES
        .iter()
        .position(|&value| value == syndrome)
    {
        return Some((word ^ (1 << bit), 1));
    }
    for (first, &first_syndrome) in ERMES_BIT_SYNDROMES.iter().enumerate() {
        if let Some(second) = ERMES_BIT_SYNDROMES
            .iter()
            .position(|&value| value == syndrome ^ first_syndrome)
            && second > first
        {
            return Some((word ^ (1 << first) ^ (1 << second), 2));
        }
    }
    None
}

#[must_use]
pub fn ermes_bch_encode(info: u32) -> u32 {
    assert!(
        info >> ERMES_DATA_BITS == 0,
        "ERMES information exceeds 18 bits"
    );
    let field = info << ERMES_CHECK_BITS;
    field | ermes_syndrome(field)
}

const RDS_GEN: u32 = 0x5B9;
const RDS_BLOCK_BITS: u32 = 26;
const RDS_CHECK_BITS: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdsOffset {
    A,
    B,
    C,
    CPrime,
    D,
}

impl RdsOffset {
    #[must_use]
    pub const fn word(self) -> u16 {
        match self {
            Self::A => 0x0FC,
            Self::B => 0x198,
            Self::C => 0x168,
            Self::CPrime => 0x350,
            Self::D => 0x1B4,
        }
    }
}

#[must_use]
pub const fn rds_syndrome(block: u32) -> u16 {
    let mut rem = block & ((1 << RDS_BLOCK_BITS) - 1);
    let mut shift = RDS_BLOCK_BITS;
    while shift > RDS_CHECK_BITS {
        shift -= 1;
        if rem & (1 << shift) != 0 {
            rem ^= RDS_GEN << (shift - RDS_CHECK_BITS);
        }
    }
    rem as u16
}

const RDS_MAX_BURST: u32 = 2;
const RDS_SYNDROMES: usize = 1 << RDS_CHECK_BITS;
const RDS_AMBIGUOUS: u32 = u32::MAX;

const RDS_BURST_TABLE: [u32; RDS_SYNDROMES] = {
    let mut table = [0u32; RDS_SYNDROMES];
    let mut shift = 0;
    while shift < RDS_BLOCK_BITS {
        let mut pattern = 1u32;
        while pattern < 1 << RDS_MAX_BURST {
            let vector = pattern << shift;
            if vector >> RDS_BLOCK_BITS == 0 {
                let at = rds_syndrome(vector) as usize;
                if table[at] == 0 {
                    table[at] = vector;
                } else if table[at] != vector {
                    table[at] = RDS_AMBIGUOUS;
                }
            }
            pattern += 1;
        }
        shift += 1;
    }
    table
};

#[must_use]
pub fn rds_check_block(block: u32, offset: RdsOffset) -> Option<u16> {
    (rds_syndrome(block) == offset.word()).then_some((block >> RDS_CHECK_BITS) as u16)
}

#[must_use]
pub fn rds_correct_block(block: u32, offset: RdsOffset) -> Option<(u16, u32)> {
    let syndrome = rds_syndrome(block) ^ offset.word();
    if syndrome == 0 {
        return Some(((block >> RDS_CHECK_BITS) as u16, 0));
    }
    match RDS_BURST_TABLE.get(usize::from(syndrome)).copied() {
        Some(0) | Some(RDS_AMBIGUOUS) | None => None,
        Some(vector) => Some((
            ((block ^ vector) >> RDS_CHECK_BITS) as u16,
            vector.count_ones(),
        )),
    }
}

#[must_use]
pub fn rds_encode_block(data: u16, offset: RdsOffset) -> u32 {
    let payload = u32::from(data) << RDS_CHECK_BITS;
    payload | u32::from(rds_syndrome(payload) ^ offset.word())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p25_reed_solomon_corrects_to_each_codes_capacity() {
        for (data_symbols, parity_symbols) in [(12, 12), (16, 8), (20, 16)] {
            let data: Vec<u8> = (0..data_symbols)
                .map(|index| (index * 7 + 3) as u8 & 0x3F)
                .collect();
            let clean = rs64_encode(&data, parity_symbols);
            assert_eq!(rs64_decode(&clean, data_symbols), Some((data.clone(), 0)));
            let mut damaged = clean;
            for index in 0..parity_symbols / 2 {
                damaged[index * 2] ^= (index as u8 + 1) & 0x3F;
            }
            assert_eq!(
                rs64_decode(&damaged, data_symbols),
                Some((data, (parity_symbols / 2) as u32))
            );
        }
    }

    const RDS_OFFSETS: [RdsOffset; 5] = [
        RdsOffset::A,
        RdsOffset::B,
        RdsOffset::C,
        RdsOffset::CPrime,
        RdsOffset::D,
    ];

    fn flip(frame: &mut [u8], bit: usize) {
        frame[bit / 8] ^= 0x80 >> (bit % 8);
    }

    fn squitter_body() -> Vec<u8> {
        vec![
            0x8D, 0x40, 0x62, 0x1D, 0x58, 0xC3, 0x82, 0xD6, 0x90, 0xC8, 0xAC,
        ]
    }

    #[test]
    fn crc16_x25_matches_catalogue_check_value() {
        assert_eq!(crc16_x25(b"123456789"), 0x906E);
    }

    #[test]
    fn crc16_ccitt_matches_catalogue_check_value() {
        assert_eq!(crc16_ccitt(b"123456789"), 0x2189);
    }

    #[test]
    fn crc16_ccitt_check_bytes_leave_a_zero_remainder() {
        let message = b"2.D-AIBC\x01H1B\x02HELLO\x03";
        let mut with_check = message.to_vec();
        with_check.extend_from_slice(&crc16_ccitt(message).to_le_bytes());
        assert_eq!(crc16_ccitt(&with_check), 0);

        for bit in 0..with_check.len() * 8 {
            let mut corrupt = with_check.clone();
            flip(&mut corrupt, bit);
            assert_ne!(crc16_ccitt(&corrupt), 0, "bit {bit} slipped past the CRC");
        }
    }

    #[test]
    fn hdlc_fcs_round_trips_and_catches_a_bit_flip() {
        let payload = b"\x82\xa0\xa4\xa6@@`\x9c\x94n\xa0@@\xe1\x03\xf0hello";
        let mut frame = payload.to_vec();
        frame.extend_from_slice(&crc16_x25(payload).to_le_bytes());
        assert!(hdlc_fcs_ok(&frame));

        for bit in 0..frame.len() * 8 {
            let mut corrupt = frame.clone();
            flip(&mut corrupt, bit);
            assert!(!hdlc_fcs_ok(&corrupt), "bit {bit} slipped past the FCS");
        }
    }

    #[test]
    fn hdlc_rejects_frames_too_short_to_hold_an_fcs() {
        assert!(!hdlc_fcs_ok(&[]));
        assert!(!hdlc_fcs_ok(&[0x00, 0x00]));
    }

    #[test]
    fn mode_s_parity_matches_the_published_squitter() {
        let mut frame = squitter_body();
        mode_s_append_parity(&mut frame);
        assert_eq!(frame[11..], [0x28, 0x63, 0xA7]);
        assert_eq!(mode_s_syndrome(&frame), 0);
    }

    #[test]
    fn mode_s_parity_closes_short_frames_too() {
        let mut frame = vec![0x5D, 0x48, 0x40, 0xD6];
        mode_s_append_parity(&mut frame);
        assert_eq!(frame.len(), MODE_S_SHORT_LEN);
        assert_eq!(mode_s_syndrome(&frame), 0);
    }

    #[test]
    fn mode_s_repairs_any_single_bit_error() {
        let mut clean = squitter_body();
        mode_s_append_parity(&mut clean);

        for bit in 0..clean.len() * 8 {
            let mut frame = clean.clone();
            flip(&mut frame, bit);
            assert_ne!(mode_s_syndrome(&frame), 0, "bit {bit} left no syndrome");
            assert_eq!(mode_s_fix_single_bit(&mut frame), Some(bit));
            assert_eq!(mode_s_syndrome(&frame), 0);
            assert_eq!(frame, clean, "bit {bit} repaired to the wrong frame");
        }
    }

    #[test]
    fn mode_s_refuses_to_repair_a_two_bit_error() {
        let mut clean = squitter_body();
        mode_s_append_parity(&mut clean);
        let bits = clean.len() * 8;

        for a in 0..bits {
            for b in [(a + 1) % bits, (a + 7) % bits, (a + 53) % bits] {
                if a == b {
                    continue;
                }
                let mut frame = clean.clone();
                flip(&mut frame, a);
                flip(&mut frame, b);
                assert_eq!(mode_s_fix_single_bit(&mut frame), None, "bits {a},{b}");
                assert_ne!(mode_s_syndrome(&frame), 0, "bits {a},{b}");
            }
        }
    }

    #[test]
    fn mode_s_leaves_a_clean_frame_alone() {
        let mut frame = squitter_body();
        mode_s_append_parity(&mut frame);
        let clean = frame.clone();
        assert_eq!(mode_s_fix_single_bit(&mut frame), None);
        assert_eq!(frame, clean);
    }

    #[test]
    fn mode_s_rejects_lengths_that_are_not_transmissions() {
        assert_ne!(mode_s_syndrome(&[0; 8]), 0);
        assert_ne!(mode_s_syndrome(&[]), 0);
        assert_eq!(mode_s_fix_single_bit(&mut [0; 8]), None);
        assert_eq!(mode_s_overlay(&[0; 8]), None);
        assert_eq!(mode_s_overlay(&[]), None);
    }

    #[test]
    fn an_overlaid_address_comes_back_out_of_the_parity() {
        for overlay in [0x00_0001, 0x3C_6444, 0x48_40D6, 0xFF_FFFF] {
            for body in [vec![0x20, 0x00, 0x19, 0x10], squitter_body()] {
                let mut frame = body;
                mode_s_append_overlaid_parity(&mut frame, overlay);
                assert_eq!(mode_s_overlay(&frame), Some(overlay));
                assert_ne!(mode_s_syndrome(&frame), overlay);
            }
        }
    }

    #[test]
    fn golay23_matches_the_published_dcs_word() {
        let data = 0b1000_0001_0011;
        let word = golay23_encode(data);
        assert_eq!(format!("{word:023b}"), "10000001001111101100011");
        assert!(golay23_ok(word));
        assert_eq!(word >> GOLAY23_CHECK_BITS, u32::from(data));
    }

    #[test]
    fn golay23_accepts_what_it_encodes_and_nothing_one_bit_away() {
        for data in 0..1u16 << GOLAY23_DATA_BITS {
            let word = golay23_encode(data);
            assert!(golay23_ok(word), "data {data:#05x}");
            for bit in 0..GOLAY23_CODE_BITS {
                assert!(!golay23_ok(word ^ 1 << bit), "data {data:#05x} bit {bit}");
            }
        }
    }

    #[test]
    fn golay23_repairs_every_error_up_to_its_radius() {
        let word = golay23_encode(0xA53);
        assert_eq!(golay23_correct(word), Some((word, 0)));
        for first in 0..GOLAY23_CODE_BITS {
            assert_eq!(golay23_correct(word ^ 1 << first), Some((word, 1)));
            for second in first + 1..GOLAY23_CODE_BITS {
                assert_eq!(
                    golay23_correct(word ^ 1 << first ^ 1 << second),
                    Some((word, 2))
                );
                for third in second + 1..GOLAY23_CODE_BITS {
                    assert_eq!(
                        golay23_correct(word ^ 1 << first ^ 1 << second ^ 1 << third),
                        Some((word, 3))
                    );
                }
            }
        }
    }

    #[test]
    fn every_rotation_and_the_complement_of_a_golay_word_is_also_one() {
        let word = golay23_encode(0b1000_0001_0011);
        for k in 0..GOLAY23_CODE_BITS {
            let mask = (1 << GOLAY23_CODE_BITS) - 1;
            let rotated = (word << k | word >> (GOLAY23_CODE_BITS - k)) & mask;
            assert!(golay23_ok(rotated), "rotation {k}");
            assert!(golay23_ok(rotated ^ mask), "complement of rotation {k}");
        }
    }

    #[test]
    fn a_bare_parity_frame_reads_as_a_zero_overlay() {
        let mut frame = squitter_body();
        mode_s_append_parity(&mut frame);
        assert_eq!(mode_s_overlay(&frame), Some(0));
        assert_eq!(mode_s_syndrome(&frame), 0);

        flip(&mut frame, 42);
        assert_ne!(mode_s_overlay(&frame), Some(0));
    }

    #[test]
    fn pocsag_round_trips_a_codeword() {
        let word21 = 0x0007_ABCD & ((1 << 21) - 1);
        let encoded = pocsag_bch_encode(word21);
        assert_eq!(encoded >> 11, word21);
        assert_eq!(pocsag_bch_decode(encoded), Some((encoded, 0)));
    }

    #[test]
    fn pocsag_corrects_every_single_bit_error() {
        let clean = pocsag_bch_encode(0x0015_5555);
        for bit in 0..32 {
            let corrupt = clean ^ (1 << bit);
            assert_eq!(
                pocsag_bch_decode(corrupt),
                Some((clean, 1)),
                "single error at bit {bit}"
            );
        }
    }

    #[test]
    fn pocsag_corrects_every_two_bit_error() {
        let clean = pocsag_bch_encode(0x000A_C3F1);
        for a in 0..32 {
            for b in (a + 1)..32 {
                let corrupt = clean ^ (1 << a) ^ (1 << b);
                assert_eq!(
                    pocsag_bch_decode(corrupt),
                    Some((clean, 2)),
                    "double error at bits {a},{b}"
                );
            }
        }
    }

    #[test]
    fn pocsag_reports_a_four_bit_error_as_uncorrectable() {
        let clean = pocsag_bch_encode(0x000A_C3F1);
        assert_eq!(pocsag_bch_decode(clean ^ 0b1111), None);
    }

    #[test]
    fn pocsag_detects_every_three_bit_error() {
        let clean = pocsag_bch_encode(0x0001_0F0F);
        for a in 0..32 {
            for b in (a + 1)..32 {
                for c in (b + 1)..32 {
                    let corrupt = clean ^ (1 << a) ^ (1 << b) ^ (1 << c);
                    assert_eq!(
                        pocsag_bch_decode(corrupt),
                        None,
                        "triple error at bits {a},{b},{c}"
                    );
                }
            }
        }
    }

    #[test]
    fn pocsag_never_calls_a_damaged_codeword_clean() {
        let clean = pocsag_bch_encode(0x000A_C3F1);
        for a in 0..32 {
            for b in (a + 1)..32 {
                for c in (b + 1)..32 {
                    for d in (c + 1)..32 {
                        let corrupt = clean ^ (1 << a) ^ (1 << b) ^ (1 << c) ^ (1 << d);
                        assert_ne!(
                            pocsag_bch_decode(corrupt),
                            Some((corrupt, 0)),
                            "quad error at bits {a},{b},{c},{d} passed as clean"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ermes_matches_the_analytic_vectors() {
        assert_eq!(ermes_bch_encode(1), 0o15315);
        assert_eq!(ermes_bch_encode((1 << 18) - 1), 0x3FFF_FC4B);
    }

    #[test]
    fn ermes_corrects_every_one_and_two_bit_error() {
        let clean = ermes_bch_encode(0x2_A5A5);
        for first in 0..30 {
            assert_eq!(ermes_bch_decode(clean ^ (1 << first)), Some((clean, 1)));
            for second in (first + 1)..30 {
                assert_eq!(
                    ermes_bch_decode(clean ^ (1 << first) ^ (1 << second)),
                    Some((clean, 2)),
                    "errors at {first},{second}"
                );
            }
        }
    }

    #[test]
    fn rds_round_trips_every_offset() {
        for offset in RDS_OFFSETS {
            for data in [0x0000, 0xFFFF, 0x1234, 0xACDC] {
                let block = rds_encode_block(data, offset);
                assert_eq!(block >> 26, 0, "block must fit 26 bits");
                assert_eq!(rds_syndrome(block), offset.word());
                assert_eq!(rds_check_block(block, offset), Some(data));
            }
        }
    }

    #[test]
    fn rds_rejects_any_single_bit_flip() {
        let block = rds_encode_block(0x3A5C, RdsOffset::B);
        for bit in 0..26 {
            let corrupt = block ^ (1 << bit);
            assert_eq!(
                rds_check_block(corrupt, RdsOffset::B),
                None,
                "bit {bit} slipped past the block check"
            );
        }
    }

    #[test]
    fn rds_repairs_a_single_flip_or_an_adjacent_pair_anywhere_in_the_block() {
        for offset in RDS_OFFSETS {
            for data in [0x0000, 0xFFFF, 0x3A5C, 0xACDC] {
                let block = rds_encode_block(data, offset);
                assert_eq!(rds_correct_block(block, offset), Some((data, 0)));
                for bit in 0..26 {
                    let single = block ^ (1 << bit);
                    assert_eq!(
                        rds_correct_block(single, offset),
                        Some((data, 1)),
                        "{offset:?} bit {bit}"
                    );
                    if bit + 1 < 26 {
                        let pair = block ^ (0b11 << bit);
                        assert_eq!(
                            rds_correct_block(pair, offset),
                            Some((data, 2)),
                            "{offset:?} bits {bit}..{}",
                            bit + 1
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn rds_correction_leaves_no_syndrome_pointing_at_two_error_patterns() {
        assert!(
            !RDS_BURST_TABLE.contains(&RDS_AMBIGUOUS),
            "the burst table cannot resolve every pattern it holds"
        );
        let reachable = RDS_BURST_TABLE.iter().filter(|&&v| v != 0).count();
        assert_eq!(reachable, 26 + 25, "one syndrome per correctable pattern");
    }

    #[test]
    fn rds_rejects_a_block_checked_against_the_wrong_offset() {
        for offset in RDS_OFFSETS {
            let block = rds_encode_block(0x5A5A, offset);
            for other in RDS_OFFSETS {
                if other == offset {
                    continue;
                }
                assert_eq!(
                    rds_check_block(block, other),
                    None,
                    "{offset:?} vs {other:?}"
                );
            }
        }
    }
}
