//! Checksums and error-correcting codes for the wave-1 decoders (PLAN §7): the HDLC/AIS
//! CRC-16, the Mode S CRC-24 with single-bit correction, the POCSAG BCH(31,21) codeword and
//! the RDS shortened cyclic (26,16) block.
//!
//! All of it is stateless integer arithmetic over GF(2) — no allocation, no locks — so a
//! decoder may call it straight from the DSP thread.

/// CRC-16/X-25 polynomial 0x1021 in its reflected form, for the LSB-first loop below.
const X25_POLY: u16 = 0x8408;

/// CRC-16/X-25 (reflected 0x1021, init 0xFFFF, xorout 0xFFFF) — the AX.25 FCS and AIS CRC.
#[must_use]
pub fn crc16_x25(data: &[u8]) -> u16 {
    let mut crc = 0xFFFF_u16;
    for &byte in data {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            let lsb = crc & 1;
            crc >>= 1;
            if lsb != 0 {
                crc ^= X25_POLY;
            }
        }
    }
    !crc
}

/// True when an HDLC frame's trailing little-endian 2-byte FCS matches its payload.
#[must_use]
pub fn hdlc_fcs_ok(frame: &[u8]) -> bool {
    // FCS plus at least one payload byte; anything shorter is a framing artefact, not a frame.
    if frame.len() < 3 {
        return false;
    }
    let (payload, fcs) = frame.split_at(frame.len() - 2);
    let [lo, hi] = fcs else { return false };
    crc16_x25(payload) == u16::from_le_bytes([*lo, *hi])
}

/// Mode S CRC-24 generator 0x1FFF409 without its implicit x^24 term (DO-260 §2.2.3.2).
const MODE_S_POLY: u32 = 0x00FF_F409;
const MODE_S_MASK: u32 = 0x00FF_FFFF;
/// Short (DF < 16) and long (DF >= 16) transmission lengths, parity bytes included.
const MODE_S_SHORT_LEN: usize = 7;
const MODE_S_LONG_LEN: usize = 14;
const MODE_S_PARITY_LEN: usize = 3;

/// Multiply a 24-bit remainder by x, reducing modulo the generator.
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

/// Mode S / ADS-B CRC-24, generator 0xFFF409, over a 7- or 14-byte frame including its
/// trailing 3 parity bytes. Returns the syndrome; 0 means the frame checks out. Any other
/// frame length is not a Mode S transmission and yields a sentinel non-syndrome.
#[must_use]
pub fn mode_s_syndrome(frame: &[u8]) -> u32 {
    if frame.len() != MODE_S_SHORT_LEN && frame.len() != MODE_S_LONG_LEN {
        return u32::MAX;
    }
    mode_s_crc(frame)
}

/// Append the 3 Mode S parity bytes to a 4- or 11-byte message body (used by test signals).
///
/// # Panics
/// If `body` is not a valid Mode S message body.
pub fn mode_s_append_parity(body: &mut Vec<u8>) {
    assert!(
        body.len() == MODE_S_SHORT_LEN - MODE_S_PARITY_LEN
            || body.len() == MODE_S_LONG_LEN - MODE_S_PARITY_LEN,
        "mode s message body must be 4 or 11 bytes, got {}",
        body.len()
    );
    let parity = mode_s_crc(body);
    body.extend_from_slice(&[(parity >> 16) as u8, (parity >> 8) as u8, parity as u8]);
}

/// Locate and repair a single-bit error using the syndrome. Returns the corrected bit index
/// (0 = MSB of the first byte), or `None` when the frame is already clean or the damage is
/// not a single bit.
///
/// The generator is divisible by (x + 1), so every syndrome carries the parity of the error
/// weight: an even-weight error can never match a single-bit syndrome and is therefore
/// rejected rather than "repaired" into a plausible frame.
pub fn mode_s_fix_single_bit(frame: &mut [u8]) -> Option<usize> {
    let syndrome = mode_s_syndrome(frame);
    if syndrome == 0 {
        return None;
    }
    let bits = frame.len() * 8;
    // Syndrome of an error at bit i is x^(bits−1−i)·x^24 mod g. The last bit's is x^24 ≡ g
    // minus its leading term; walking backwards is one multiply by x per position.
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

/// POCSAG BCH(31,21) generator x^10+x^9+x^8+x^6+x^5+x^3+1.
const POCSAG_GEN: u32 = 0x769;
/// Codeword bits covered by the BCH code — everything but the trailing parity bit.
const POCSAG_CODE_BITS: u32 = 31;
const POCSAG_DATA_BITS: u32 = 21;

/// Remainder of the 31-bit BCH field modulo the generator; 0 for a valid field.
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

/// Syndrome of a single-bit error at each position of the 31-bit BCH field.
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

/// Odd parity of the whole 32-bit codeword: 0 when the even-parity bit is consistent.
fn pocsag_parity(word: u32) -> u32 {
    word.count_ones() & 1
}

/// POCSAG codeword: 1 sync/flag bit + 20 data bits + 10 BCH(31,21) check bits + 1 even
/// parity bit, transmitted MSB first. Decode corrects up to 2 bit errors.
/// Returns (corrected codeword, number of bits corrected), or None when uncorrectable.
///
/// Correction is a table search: 31 comparisons for one error and at most 31 × 31 for two,
/// allocation-free — about a microsecond against POCSAG's 512–2400 bit/s.
#[must_use]
pub fn pocsag_bch_decode(word: u32) -> Option<(u32, u32)> {
    let syndrome = pocsag_syndrome(word >> 1);
    let odd = pocsag_parity(word) == 1;
    if syndrome == 0 {
        // The BCH code has minimum distance 5, so a clean field with bad parity can only be
        // the parity bit itself — no error of weight 2..=4 hides here.
        return Some(if odd { (word ^ 1, 1) } else { (word, 0) });
    }
    if let Some(bit) = pocsag_find_bit(syndrome) {
        // Odd parity means the field error stands alone; even parity means the parity bit
        // was hit too. Distance 5 rules out a two-error field with a single-bit syndrome.
        let fix = (1 << (bit + 1)) | u32::from(!odd);
        return Some((word ^ fix, if odd { 1 } else { 2 }));
    }
    if odd {
        return None;
    }
    let (a, b) = pocsag_find_pair(syndrome)?;
    Some((word ^ (1 << (a + 1)) ^ (1 << (b + 1)), 2))
}

/// Build a codeword from its 21 leading bits (flag + 20 data), adding BCH + parity. The bits
/// sit in the low 21 bits of `word21`, i.e. `word21 == codeword >> 11`.
///
/// # Panics
/// If `word21` does not fit in 21 bits.
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

/// RDS block generator x^10+x^8+x^7+x^5+x^4+x^3+1 (EN 50067 §B.2.1).
const RDS_GEN: u32 = 0x5B9;
const RDS_BLOCK_BITS: u32 = 26;
const RDS_CHECK_BITS: u32 = 10;

/// RDS block offset words (EN 50067 §B.2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdsOffset {
    A,
    B,
    C,
    CPrime,
    D,
}

impl RdsOffset {
    /// The 10-bit word added to the block's checkword, which both identifies the block
    /// within the group and provides block synchronisation.
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

/// The 10-bit syndrome of a 26-bit RDS block under the shortened cyclic (26,16) code.
///
/// The syndrome is the plain remainder `block(x) mod g(x)`, so a block carrying offset word
/// `o` yields exactly `o` — block synchronisation matches against [`RdsOffset::word`], not
/// against the re-based syndrome constants some published tables list. Only the low 26 bits
/// of `block` are considered.
#[must_use]
pub fn rds_syndrome(block: u32) -> u16 {
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

/// Check a received 26-bit block against `offset`; returns the 16 data bits when valid.
#[must_use]
pub fn rds_check_block(block: u32, offset: RdsOffset) -> Option<u16> {
    (rds_syndrome(block) == offset.word()).then_some((block >> RDS_CHECK_BITS) as u16)
}

/// Build a 26-bit block from 16 data bits plus the offset word (used by test signals).
#[must_use]
pub fn rds_encode_block(data: u16, offset: RdsOffset) -> u32 {
    let payload = u32::from(data) << RDS_CHECK_BITS;
    payload | u32::from(rds_syndrome(payload) ^ offset.word())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The DF17 extended squitter from the Mode S literature, body only.
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
    }

    #[test]
    fn pocsag_round_trips_a_codeword() {
        // A typical address codeword: flag 0 + address + function bits.
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

    /// Parity extends the BCH code to distance 6, so no weight-3 error can sit within two
    /// bits of another codeword: every one of them must be rejected outright.
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

    /// Weight-4 errors are past the code's guarantee — some are miscorrected by construction
    /// — but none may ever come back flagged as an intact codeword.
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
