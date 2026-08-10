//! ACARS reference modulator (PLAN §14): an ARINC 618 block → odd-parity characters and a
//! CRC-16 → MSK at 2400 bit/s → amplitude modulation onto a carrier at complex baseband.
//!
//! The field layout is written out here from the standard rather than shared with the decoder:
//! a block laid out by the same code that parses it would round-trip through any offset error
//! the two happened to agree on.

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

/// Bit-synchronisation characters that precede the sync pair on the air (ARINC 618 §4.3.1).
const BSYNC: [u8; 2] = *b"+*";

/// Alternating bits ahead of the block, so the receiver's bit clock and matched filter have
/// something to settle on before the first sync character.
const PREKEY_BITS: usize = 128;

/// Depth of the amplitude modulation. ACARS drives the carrier hard; anything shallow enough
/// to be gentle would also be unlike the signal a real receiver sees.
const AM_DEPTH: f32 = 0.8;

/// One ARINC 618 block. `registration` is the 7-character address field as transmitted, dots
/// and all; `seq_no`/`flight` belong to downlink blocks only.
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
    /// End the block with ETB rather than ETX: another block follows.
    pub more: bool,
}

/// Set the high bit where needed to give a 7-bit character odd parity (ARINC 618 §4.2).
#[must_use]
pub fn odd_parity(byte: u8) -> u8 {
    if (byte & 0x7F).count_ones().is_multiple_of(2) {
        (byte & 0x7F) | 0x80
    } else {
        byte & 0x7F
    }
}

/// The block's characters from the mode through the terminator — exactly the span the CRC
/// covers.
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

/// The framed byte stream a receiver sees: sync pair, SOH, the block, its CRC and the closing
/// DEL. The CRC bytes go out low byte first, which is what makes the receiver's running check
/// end at zero.
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

/// Bits as transmitted: least significant first within each character.
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

/// Minimum-shift keying: continuous phase, `1` at the upper tone, `0` at the lower. Returns
/// the real modulating waveform, which is what amplitude-modulates the carrier.
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

/// A complete transmission: pre-key, the bit-sync characters, then the framed block, as an AM
/// carrier at baseband. The `+*` pair is not a sync word a receiver matches on — it is there
/// because a real transmitter sends it, and a decoder must hunt past it to the sync pair.
#[must_use]
pub fn transmission(block: &Block<'_>, rate: f64) -> Vec<Complex<f32>> {
    let mut stream: Vec<bool> = (0..PREKEY_BITS).map(|i| i.is_multiple_of(2)).collect();
    stream.extend(bits(&BSYNC));
    stream.extend(bits(&block_bytes(block)));
    super::am_modulate(&msk_audio(&stream, rate), AM_DEPTH)
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

    /// The receiver validates by running the check bytes through the same register and
    /// expecting zero; if the byte order were wrong, only a decoder with the same bug would
    /// agree.
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

    /// A run of like bits is a steady tone, and counting its cycles says which of the two it
    /// is. Half a cycle per bit at the lower tone and a whole one at the upper — a quarter
    /// cycle either side of the 1800 Hz centre, which is MSK's ±90° per symbol.
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
