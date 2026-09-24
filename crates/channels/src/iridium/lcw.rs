use sdrmm_dsp::crc16_msb;

use super::frame::{
    HDR_POLY, LCW2_POLY, LCW3_POLY, bch_repair, bits_to_u8, bits_to_u32, de_interleave2, lcw_bits,
};

pub const ACCH_BCH_POLY: u32 = 3545;
const CCITT_POLY: u16 = 0x1021;
const CCITT_INIT: u16 = 0xFFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lcw {
    pub frame_type: u8,
    pub control: u32,
    pub payload: u32,
    pub corrected: u32,
}

pub fn decode_lcw(bits: &[u8]) -> Option<Lcw> {
    if bits.len() < 46 {
        return None;
    }
    let lcw = lcw_bits(bits);
    let mut header = lcw[..7].to_vec();
    let header_errors = bch_repair(HDR_POLY, &mut header)?;
    let (control_errors, control) = repair_control(&lcw[7..20])?;
    let mut payload = lcw[20..].to_vec();
    let payload_errors = bch_repair(LCW3_POLY, &mut payload)?;
    Some(Lcw {
        frame_type: bits_to_u8(&header[..3]),
        control: bits_to_u32(&control[..6]),
        payload: bits_to_u32(&payload[..21]),
        corrected: header_errors + control_errors + payload_errors,
    })
}

fn repair_control(received: &[u8]) -> Option<(u32, Vec<u8>)> {
    let completed = |missing: u8| {
        let mut block = received.to_vec();
        block.push(missing);
        let errors = bch_repair(LCW2_POLY, &mut block);
        (errors, block)
    };
    let (zero_errors, zero) = completed(0);
    let (one_errors, one) = completed(1);
    match (zero_errors, one_errors) {
        (Some(a), Some(b)) if b < a => Some((b, one)),
        (Some(a), _) => Some((a, zero)),
        (None, Some(b)) => Some((b, one)),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaFrame {
    pub continuation: bool,
    pub ctr: u8,
    pub len: u8,
    pub data: [u8; 20],
    pub crc_ok: bool,
    pub bch_corrected: u32,
}

pub fn da_blocks(data: &[u8]) -> Vec<Vec<u8>> {
    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(10);
    for chunk in data[..248].chunks_exact(124) {
        let (first, second) = de_interleave2(chunk);
        let all: Vec<u8> = first.iter().chain(&second).copied().collect();
        let quarters: Vec<&[u8]> = all.chunks_exact(31).collect();
        for index in [3, 1, 2, 0] {
            blocks.push(quarters[index].to_vec());
        }
    }
    let (first, second) = de_interleave2(&data[248..312]);
    blocks.push(second[1..].to_vec());
    blocks.push(first[1..].to_vec());
    blocks
}

pub fn decode_da(data: &[u8]) -> Option<DaFrame> {
    if data.len() < 312 {
        return None;
    }
    let mut bits = Vec::with_capacity(200);
    let mut fixed = 0u32;
    for mut block in da_blocks(data) {
        if bch_repair(ACCH_BCH_POLY, &mut block)? > 0 {
            fixed += 1;
        }
        bits.extend_from_slice(&block[..20]);
    }
    da_frame(&bits, fixed)
}

pub fn da_frame(bits: &[u8], fixed: u32) -> Option<DaFrame> {
    if bits_to_u32(&bits[17..20]) != 0 || bits_to_u32(&bits[196..200]) != 0 {
        return None;
    }
    let len = bits_to_u8(&bits[11..16]);
    let mut data = [0u8; 20];
    for (byte, chunk) in data.iter_mut().zip(bits[20..180].chunks_exact(8)) {
        *byte = bits_to_u8(chunk);
    }
    Some(DaFrame {
        continuation: bits[3] == 1,
        ctr: bits_to_u8(&bits[5..8]),
        len,
        data,
        crc_ok: len > 0 && da_crc(bits, 196) == 0,
        bch_corrected: fixed,
    })
}

pub fn da_crc(bits: &[u8], end: usize) -> u16 {
    let mut stream: Vec<u8> = bits[..20].to_vec();
    stream.extend([0u8; 12]);
    stream.extend_from_slice(&bits[20..end]);
    let bytes: Vec<u8> = stream.chunks_exact(8).map(bits_to_u8).collect();
    crc16_msb(CCITT_POLY, CCITT_INIT, &bytes)
}
