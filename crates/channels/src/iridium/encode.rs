use super::frame::{
    ACCESS_DL, HDR_POLY, HEADER_MESSAGING, LCW_TABLE, LCW2_POLY, LCW3_POLY, MESSAGING_BCH_POLY,
    RINGALERT_BCH_POLY, ndivide,
};
use super::lcw::{ACCH_BCH_POLY, da_crc};

pub fn bch_encode_raw(poly: u32, data: &[u8], check: usize) -> Vec<u8> {
    let mut padded = data.to_vec();
    padded.resize(data.len() + check, 0);
    let remainder = ndivide(poly, &padded);
    let mut block = data.to_vec();
    block.extend((0..check).rev().map(|k| ((remainder >> k) & 1) as u8));
    block
}

pub fn bch_encode(poly: u32, data21: &[u8]) -> Vec<u8> {
    let mut block = bch_encode_raw(poly, data21, 10);
    let ones: u32 = block.iter().map(|&b| u32::from(b)).sum();
    block.push((ones % 2) as u8);
    block
}

pub fn interleave2(odd: &[u8], even: &[u8]) -> Vec<u8> {
    let n = (odd.len() + even.len()) / 2;
    let mut symbols = vec![[0u8, 0]; n];
    for (start, block) in [(1usize, odd), (2, even)] {
        let slots = (0..=n - start).rev().step_by(2);
        for (slot, pair) in slots.zip(block.chunks_exact(2)) {
            symbols[slot] = [pair[0], pair[1]];
        }
    }
    symbols.into_iter().flat_map(|[a, b]| [b, a]).collect()
}

pub fn interleave3(b1: &[u8], b2: &[u8], b3: &[u8]) -> Vec<u8> {
    let n = (b1.len() + b2.len() + b3.len()) / 2;
    let mut symbols = vec![[0u8, 0]; n];
    for (start, block) in [(1usize, b1), (2, b2), (3, b3)] {
        let slots = (0..=n - start).rev().step_by(3);
        for (slot, pair) in slots.zip(block.chunks_exact(2)) {
            symbols[slot] = [pair[0], pair[1]];
        }
    }
    symbols.into_iter().flat_map(|[a, b]| [b, a]).collect()
}

pub fn encode_lcw(ft: u8, lcw2_data: u32, lcw3_data: u32) -> Vec<u8> {
    let ft_bits: Vec<u8> = (0..3).rev().map(|k| (ft >> k) & 1).collect();
    let p1 = bch_encode_raw(HDR_POLY, &ft_bits, 4);
    let d2: Vec<u8> = (0..6).rev().map(|k| ((lcw2_data >> k) & 1) as u8).collect();
    let mut p2 = bch_encode_raw(LCW2_POLY, &d2, 8);
    p2.pop();
    let d3: Vec<u8> = (0..21)
        .rev()
        .map(|k| ((lcw3_data >> k) & 1) as u8)
        .collect();
    let p3 = bch_encode_raw(LCW3_POLY, &d3, 5);
    let mut out = vec![0u8; 46];
    for (k, bit) in p1.into_iter().chain(p2).chain(p3).enumerate() {
        out[LCW_TABLE[k] - 1] = bit;
    }
    out
}

pub fn encode_da_payload(bits200: &[u8]) -> Vec<u8> {
    let blocks: Vec<Vec<u8>> = bits200
        .chunks_exact(20)
        .map(|d| bch_encode_raw(ACCH_BCH_POLY, d, 11))
        .collect();
    let mut out = Vec::with_capacity(312);
    for group in blocks[..8].chunks_exact(4) {
        let all: Vec<u8> = [&group[3], &group[1], &group[2], &group[0]]
            .into_iter()
            .flatten()
            .copied()
            .collect();
        out.extend(interleave2(&all[..62], &all[62..]));
    }
    let mut b1 = vec![0u8];
    b1.extend_from_slice(&blocks[9]);
    let mut b2 = vec![0u8];
    b2.extend_from_slice(&blocks[8]);
    out.extend(interleave2(&b1, &b2));
    out
}

pub fn build_da_bits(cont: bool, ctr: u8, len: u8, payload: &[u8; 20]) -> Vec<u8> {
    let mut bits = vec![0u8; 200];
    bits[3] = u8::from(cont);
    for k in 0..3 {
        bits[5 + k] = (ctr >> (2 - k)) & 1;
    }
    for k in 0..5 {
        bits[11 + k] = (len >> (4 - k)) & 1;
    }
    for (i, &b) in payload.iter().enumerate() {
        for k in 0..8 {
            bits[20 + i * 8 + k] = (b >> (7 - k)) & 1;
        }
    }
    let crc = da_crc(&bits, 180);
    for k in 0..16 {
        bits[180 + k] = ((crc >> (15 - k)) & 1) as u8;
    }
    bits
}

pub fn da_burst_bits(cont: bool, ctr: u8, len: u8, payload: &[u8; 20]) -> Vec<u8> {
    let mut bits = ACCESS_DL.to_vec();
    bits.extend(encode_lcw(2, 0, 0));
    bits.extend(encode_da_payload(&build_da_bits(cont, ctr, len, payload)));
    bits
}

pub fn push_field(bits: &mut Vec<u8>, value: u32, width: usize) {
    bits.extend((0..width).rev().map(|k| ((value >> k) & 1) as u8));
}

pub fn ira_payload(sat: u32, beam: u32, xyz: [i32; 3], tmsis: &[u32]) -> Vec<u8> {
    let mut d = Vec::new();
    push_field(&mut d, sat, 7);
    push_field(&mut d, beam, 6);
    for v in xyz {
        let sign = u32::from(v < 0);
        let magnitude = if v < 0 { v + (1 << 11) } else { v } as u32;
        push_field(&mut d, sign, 1);
        push_field(&mut d, magnitude, 11);
    }
    push_field(&mut d, 17, 7);
    push_field(&mut d, 1, 1);
    push_field(&mut d, 0, 1);
    push_field(&mut d, 9, 5);
    for &tmsi in tmsis {
        push_field(&mut d, tmsi, 32);
        push_field(&mut d, 0, 2);
        push_field(&mut d, 14, 5);
        push_field(&mut d, 0, 3);
    }
    d.extend([1u8; 42]);
    d
}

pub fn ira_bits(payload: &[u8]) -> Vec<u8> {
    let mut padded = payload.to_vec();
    while padded.len() % 21 != 0 {
        padded.push(0);
    }
    while (padded.len() / 21 - 3) % 2 != 0 {
        padded.extend([0u8; 21]);
    }
    let blocks: Vec<Vec<u8>> = padded
        .chunks_exact(21)
        .map(|d| bch_encode(RINGALERT_BCH_POLY, d))
        .collect();
    let mut bits = ACCESS_DL.to_vec();
    bits.extend(interleave3(&blocks[0], &blocks[1], &blocks[2]));
    for pair in blocks[3..].chunks_exact(2) {
        bits.extend(interleave2(&pair[0], &pair[1]));
    }
    bits
}

pub fn ims_bits(blocks21: &[Vec<u8>]) -> Vec<u8> {
    let encoded: Vec<Vec<u8>> = blocks21
        .iter()
        .map(|d| bch_encode(MESSAGING_BCH_POLY, d))
        .collect();
    let mut bits = ACCESS_DL.to_vec();
    bits.extend(HEADER_MESSAGING.iter().copied());
    for pair in encoded.chunks_exact(2) {
        bits.extend(interleave2(&pair[0], &pair[1]));
    }
    bits
}

pub fn bits_of_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks(2)
        .flat_map(|pair| {
            let text = std::str::from_utf8(pair).unwrap_or("00");
            let byte = u8::from_str_radix(text, 16).unwrap_or(0);
            (0..8).rev().map(move |i| (byte >> i) & 1)
        })
        .collect()
}

pub fn bits_of_str(text: &str) -> Vec<u8> {
    text.bytes().map(|b| u8::from(b == b'1')).collect()
}

pub fn pager_blocks(ric: u32, text: &str) -> Vec<Vec<u8>> {
    let mut rest: Vec<u8> = (0..22).map(|k| ((ric >> k) & 1) as u8).collect();
    push_field(&mut rest, 5, 5);
    push_field(&mut rest, 7, 6);
    push_field(&mut rest, 0, 4);
    push_field(&mut rest, 0, 6);
    push_field(&mut rest, 0, 4);
    rest.push(0);
    rest.push(0);
    push_field(&mut rest, 0, 7);
    for c in text.bytes() {
        push_field(&mut rest, u32::from(c), 7);
    }
    push_field(&mut rest, 3, 7);
    let mut blocks: Vec<Vec<u8>> = rest
        .chunks(20)
        .map(|chunk| {
            let mut block = vec![0u8];
            block.extend_from_slice(chunk);
            block.resize(21, 0);
            block
        })
        .collect();
    let bch_blocks = (blocks.len() + 2) / 2;
    let mut header = vec![0u8];
    push_field(&mut header, 0, 4);
    push_field(&mut header, 3, 4);
    push_field(&mut header, 9, 6);
    push_field(&mut header, bch_blocks as u32, 4);
    push_field(&mut header, 1, 2);
    blocks.insert(0, header);
    if blocks.len() % 2 == 1 {
        blocks.push(vec![1u8; 21]);
    }
    blocks
}
