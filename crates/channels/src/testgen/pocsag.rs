use num_complex::Complex;
use sdrmm_dsp::pocsag_bch_encode;
use sdrmm_modem::{
    cpm::{CpmMod, CpmParams, Mapping},
    pulse::{self, Norm},
};

const FRAME_SYNC: u32 = 0x7CD2_15D8;
/// Idle codeword — fills unused slots and terminates a message.
const IDLE: u32 = 0x7A89_C197;
const BATCH_CODEWORDS: usize = 16;
const CODEWORD_BITS: u32 = 32;
/// Payload bits a message codeword carries (32 minus the flag, BCH check bits and parity).
const PAYLOAD_BITS: usize = 20;
const ALPHA_BITS: usize = 7;
const NUMERIC_BITS: usize = 4;
const PREAMBLE_BITS: usize = 576;

const NUMERIC_ALPHABET: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '*', 'U', ' ', '-', ')', '(',
];
/// The space code, which is also what a transmitter pads the last numeric codeword with.
const NUMERIC_SPACE: u8 = 12;

/// One page to transmit: a 21-bit receiver address (RIC), its function bits, and the message
/// body. `numeric` picks the 4-bit BCD alphabet; a real transmitter pairs it with function 0.
/// An empty `text` makes a tone-only page (no message codewords).
#[derive(Clone, Debug)]
pub struct Page {
    pub address: u32,
    pub function: u8,
    pub text: String,
    pub numeric: bool,
}

/// Encode `pages` into the codeword stream that follows the preamble: whole batches, each a
/// frame sync codeword plus 16 codewords, unused slots idle. An all-idle batch is always
/// appended so the final message is terminated in-stream rather than by loss of carrier.
#[must_use]
pub fn codewords(pages: &[Page]) -> Vec<u32> {
    let mut batches = vec![[IDLE; BATCH_CODEWORDS]];
    let mut pos = 0;
    for page in pages {
        let slot = (page.address & 7) as usize * 2;
        while pos % BATCH_CODEWORDS != slot {
            pos += 1;
        }
        put(&mut batches, pos, address_codeword(page));
        pos += 1;
        for word in message_codewords(page) {
            put(&mut batches, pos, word);
            pos += 1;
        }
    }
    batches.push([IDLE; BATCH_CODEWORDS]);
    batches
        .iter()
        .flat_map(|batch| std::iter::once(FRAME_SYNC).chain(batch.iter().copied()))
        .collect()
}

/// Encode `pages` into a complete POCSAG transmission (preamble + batches) as complex baseband
/// IQ at `rate`, keyed at `baud` with ±`deviation_hz`.
#[must_use]
pub fn transmission(pages: &[Page], baud: u16, deviation_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    keyed(&codewords(pages), baud, deviation_hz, rate)
}

/// Key an arbitrary codeword stream behind the preamble — [`transmission`] without the
/// framing, so a test can transmit a deliberately damaged codeword sequence.
#[must_use]
pub fn keyed(words: &[u32], baud: u16, deviation_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let mut symbols = Vec::with_capacity(PREAMBLE_BITS + words.len() * CODEWORD_BITS as usize);
    symbols.extend((0..PREAMBLE_BITS).map(|i| u8::from(i.is_multiple_of(2))));
    for &word in words {
        symbols.extend((0..CODEWORD_BITS).rev().map(|i| (word >> i & 1) as u8));
    }
    let sps = rate / f64::from(baud);
    let mut modulator = CpmMod::new(CpmParams::from_deviation(
        Mapping::new(vec![1.0, -1.0]),
        deviation_hz,
        f64::from(baud),
        pulse::rect(sps, Norm::Area),
        sps,
    ));
    let mut iq = Vec::with_capacity((symbols.len() as f64 * sps) as usize + sps as usize + 1);
    modulator.modulate(&symbols, &mut iq);
    modulator.flush(&mut iq);
    iq
}

fn put(batches: &mut Vec<[u32; BATCH_CODEWORDS]>, pos: usize, word: u32) {
    let (batch, slot) = (pos / BATCH_CODEWORDS, pos % BATCH_CODEWORDS);
    while batches.len() <= batch {
        batches.push([IDLE; BATCH_CODEWORDS]);
    }
    batches[batch][slot] = word;
}

fn address_codeword(page: &Page) -> u32 {
    let address = (page.address >> 3) & 0x3_FFFF;
    pocsag_bch_encode(address << 2 | u32::from(page.function & 3))
}

fn message_codewords(page: &Page) -> Vec<u32> {
    let mut bits = Vec::new();
    if page.numeric {
        for ch in page.text.chars() {
            push_lsb_first(&mut bits, numeric_code(ch), NUMERIC_BITS);
        }
        while !bits.len().is_multiple_of(PAYLOAD_BITS) {
            push_lsb_first(&mut bits, NUMERIC_SPACE, NUMERIC_BITS);
        }
    } else {
        for ch in page.text.chars() {
            push_lsb_first(&mut bits, ascii7(ch), ALPHA_BITS);
        }
        // NUL padding: the decoder stops at the first one, so it never reaches the message.
        while !bits.len().is_multiple_of(PAYLOAD_BITS) {
            bits.push(false);
        }
    }
    bits.as_chunks::<PAYLOAD_BITS>()
        .0
        .iter()
        .map(|chunk| {
            let payload = chunk
                .iter()
                .fold(0u32, |acc, &bit| acc << 1 | u32::from(bit));
            pocsag_bch_encode(1 << PAYLOAD_BITS | payload)
        })
        .collect()
}

fn push_lsb_first(bits: &mut Vec<bool>, value: u8, len: usize) {
    bits.extend((0..len).map(|i| value >> i & 1 == 1));
}

/// Characters outside the BCD alphabet have no encoding; they become spaces.
fn numeric_code(ch: char) -> u8 {
    NUMERIC_ALPHABET
        .iter()
        .position(|&c| c == ch)
        .map_or(NUMERIC_SPACE, |i| i as u8)
}

/// The alphanumeric alphabet is 7-bit ASCII; anything else becomes `?`.
fn ascii7(ch: char) -> u8 {
    if ch.is_ascii() { ch as u8 } else { b'?' }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(address: u32, function: u8, text: &str, numeric: bool) -> Page {
        Page {
            address,
            function,
            text: text.to_owned(),
            numeric,
        }
    }

    #[test]
    fn batches_are_sync_word_plus_sixteen_codewords() {
        let words = codewords(&[page(1_234_568, 3, "HI", false)]);
        assert!(words.len().is_multiple_of(BATCH_CODEWORDS + 1));
        for batch in words.as_chunks::<{ BATCH_CODEWORDS + 1 }>().0 {
            assert_eq!(batch.first(), Some(&FRAME_SYNC));
        }
        let last = words.len() - BATCH_CODEWORDS;
        assert!(words[last..].iter().all(|&w| w == IDLE));
    }

    #[test]
    fn address_codeword_lands_in_the_frame_its_low_bits_name() {
        for low in 0..8 {
            let address = 0x0002_A340 | low;
            let words = codewords(&[page(address, 1, "", false)]);
            let slot = 1 + (low as usize) * 2;
            let word = words[slot];
            assert_eq!(word >> 31, 0, "address codeword must carry flag 0");
            assert_eq!((word >> 13) & 0x3_FFFF, address >> 3);
            assert_eq!((word >> 11) & 3, 1);
        }
    }

    #[test]
    fn message_codewords_carry_the_flag_and_pad_to_full_codewords() {
        let words = message_codewords(&page(8, 3, "ABC", false));
        assert_eq!(words.len(), 2, "21 character bits need two codewords");
        for word in words {
            assert_eq!(word >> 31, 1, "message codeword must carry flag 1");
        }
        assert!(message_codewords(&page(8, 3, "", false)).is_empty());
    }

    #[test]
    fn every_codeword_is_a_valid_bch_word() {
        let words = codewords(&[
            page(1_234_567, 3, "HELLO WORLD", false),
            page(9_876_540, 0, "123456", true),
        ]);
        for word in words {
            assert_eq!(
                sdrmm_dsp::pocsag_bch_decode(word),
                Some((word, 0)),
                "codeword {word:#010x} does not check"
            );
        }
    }

    #[test]
    fn transmission_is_preamble_then_batches_at_the_requested_rate() {
        let pages = [page(1_234_567, 3, "TEST", false)];
        let words = codewords(&pages);
        let iq = transmission(&pages, 1_200, 4_500.0, 48_000.0);
        let expected = (PREAMBLE_BITS + words.len() * CODEWORD_BITS as usize) * 40 + 40;
        assert_eq!(iq.len(), expected);
        for s in &iq {
            assert!((s.norm() - 1.0).abs() < 1e-3, "magnitude {}", s.norm());
        }
    }
}
