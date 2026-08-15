use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::crc16_ccitt;

const BAUD: f64 = 2_400.0;
const CENTRE_HZ: f64 = 1_800.0;
const DEVIATION_HZ: f64 = 600.0;

const SYN: u8 = 0x16;
const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const ETB: u8 = 0x17;
const DEL: u8 = 0x7F;

const BSYNC: [u8; 2] = *b"+*";

const PREKEY_BITS: usize = 128;

const AM_DEPTH: f32 = 0.8;

#[derive(Clone, Copy, Debug)]
pub struct Block<'a> {
    pub mode: char,
    pub registration: &'a str,
    pub ack: char,
    pub label: &'a str,
    pub block_id: char,
    pub seq_no: Option<&'a str>,
    pub flight: Option<&'a str>,
    pub text: &'a str,
    pub more: bool,
}

#[must_use]
pub fn odd_parity(byte: u8) -> u8 {
    if (byte & 0x7F).count_ones().is_multiple_of(2) {
        (byte & 0x7F) | 0x80
    } else {
        byte & 0x7F
    }
}

#[must_use]
pub fn block_body(block: &Block<'_>) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(block.mode as u8);
    let mut address = block.registration.as_bytes().to_vec();
    address.resize(7, b'.');
    body.extend_from_slice(&address);
    body.push(block.ack as u8);
    let mut label = block.label.as_bytes().to_vec();
    label.resize(2, b' ');
    body.extend_from_slice(&label);
    body.push(block.block_id as u8);
    body.push(STX);
    if let Some(seq_no) = block.seq_no {
        let mut field = seq_no.as_bytes().to_vec();
        field.resize(4, b' ');
        body.extend_from_slice(&field);
    }
    if let Some(flight) = block.flight {
        let mut field = flight.as_bytes().to_vec();
        field.resize(6, b' ');
        body.extend_from_slice(&field);
    }
    body.extend_from_slice(block.text.as_bytes());
    body.push(if block.more { ETB } else { ETX });
    for byte in &mut body {
        *byte = odd_parity(*byte);
    }
    body
}

#[must_use]
pub fn block_bytes(block: &Block<'_>) -> Vec<u8> {
    let body = block_body(block);
    let crc = crc16_ccitt(&body);
    let mut bytes = vec![SYN, SYN, SOH];
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes.push(DEL);
    bytes
}

#[must_use]
pub fn bits(bytes: &[u8]) -> Vec<bool> {
    let mut out = Vec::with_capacity(bytes.len() * 8);
    for &byte in bytes {
        for i in 0..8 {
            out.push((byte >> i) & 1 == 1);
        }
    }
    out
}

#[must_use]
pub fn msk_audio(bits: &[bool], rate: f64) -> Vec<f32> {
    let sps = rate / BAUD;
    let len = (bits.len() as f64 * sps) as usize;
    let mut phase = 0.0f64;
    (0..len)
        .map(|k| {
            let idx = ((k as f64 / sps) as usize).min(bits.len().saturating_sub(1));
            let freq = if bits.get(idx).copied().unwrap_or(false) {
                CENTRE_HZ + DEVIATION_HZ
            } else {
                CENTRE_HZ - DEVIATION_HZ
            };
            phase += TAU * freq / rate;
            if phase > TAU {
                phase -= TAU;
            }
            phase.sin() as f32
        })
        .collect()
}

#[must_use]
pub fn transmission(block: &Block<'_>, rate: f64) -> Vec<Complex<f32>> {
    let mut stream: Vec<bool> = (0..PREKEY_BITS).map(|i| i.is_multiple_of(2)).collect();
    stream.extend(bits(&BSYNC));
    stream.extend(bits(&block_bytes(block)));
    msk_audio(&stream, rate)
        .iter()
        .map(|&s| Complex::new(1.0 + AM_DEPTH * s, 0.0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Block<'static> {
        Block {
            mode: '2',
            registration: ".D-AIBC",
            ack: '\x15',
            label: "H1",
            block_id: '3',
            seq_no: Some("M01A"),
            flight: Some("LH0400"),
            text: "TEST",
            more: false,
        }
    }

    #[test]
    fn every_character_carries_odd_parity() {
        for byte in block_body(&sample()) {
            assert_eq!(byte.count_ones() % 2, 1, "{byte:#04x}");
        }
    }

    #[test]
    fn appending_the_check_bytes_zeroes_the_remainder() {
        let bytes = block_bytes(&sample());
        let checked = &bytes[3..bytes.len() - 1];
        assert_eq!(crc16_ccitt(checked), 0);
    }

    #[test]
    fn the_block_start_character_sits_where_the_standard_puts_it() {
        let body = block_body(&sample());
        assert_eq!(body[12] & 0x7F, STX, "STX must be the 13th character");
        assert_eq!(body[0] & 0x7F, b'2');
        assert_eq!(
            &body[1..8].iter().map(|b| b & 0x7F).collect::<Vec<_>>(),
            b".D-AIBC"
        );
    }

    #[test]
    fn the_tone_pair_is_where_msk_puts_it() {
        const BITS: usize = 40;
        for (bit, tone_hz) in [
            (false, CENTRE_HZ - DEVIATION_HZ),
            (true, CENTRE_HZ + DEVIATION_HZ),
        ] {
            let audio = msk_audio(&[bit; BITS], 48_000.0);
            let crossings = audio
                .windows(2)
                .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
                .count();
            let expected = (BITS as f64 * tone_hz / BAUD).round() as usize;
            assert!(
                crossings.abs_diff(expected) <= 1,
                "{bit}: {crossings} cycles, expected {expected}"
            );
        }
    }
}
