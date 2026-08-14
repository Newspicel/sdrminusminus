//! Bit-level plumbing shared by the framed decoders ( "correlators", §13 phase 2):
//! NRZI/differential decoding, HDLC deframing, multiplicative scrambling and sync-word
//! correlation. Everything here is per-bit and allocation-free except the frame vector
//! [`HdlcDeframer`] hands out.

/// NRZI line decode: a 0 bit is a transition, a 1 bit is no transition (AX.25, AIS).
///
/// The line has no absolute polarity, so the very first output is relative to an assumed
/// low idle level; framing (a flag hunt) resolves the ambiguity, not this decoder.
#[derive(Clone, Debug, Default)]
pub struct NrziDecoder {
    last: bool,
}

impl NrziDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decode(&mut self, level: bool) -> bool {
        let bit = level == self.last;
        self.last = level;
        bit
    }

    pub fn reset(&mut self) {
        self.last = false;
    }
}

/// Differential BPSK bit decode `out = in XOR previous` (RDS), removing the 180° phase
/// ambiguity a Costas loop leaves behind.
#[derive(Clone, Debug, Default)]
pub struct DifferentialDecoder {
    last: bool,
}

impl DifferentialDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decode(&mut self, bit: bool) -> bool {
        let out = bit ^ self.last;
        self.last = bit;
        out
    }

    pub fn reset(&mut self) {
        self.last = false;
    }
}

/// HDLC deframer: consumes NRZI-decoded bits, syncs on 0x7E flags, removes stuffed zeros
/// after five ones, and emits byte-aligned frames LSB-first (AX.25/AIS bit order).
///
/// Seven ones is an abort and drops the frame in progress; frames outside
/// `min_bytes..=max_bytes` are dropped at the closing flag.
#[derive(Clone, Debug)]
pub struct HdlcDeframer {
    min_bytes: usize,
    max_bytes: usize,
    /// Consecutive 1 bits ending at the previous bit: 5 arms the stuffing rule, 6 means the
    /// next bit decides between a flag (0) and an abort (1).
    ones: u8,
    byte: u8,
    /// Bits held in `byte`. A frame is byte-aligned, so at the closing flag this must equal
    /// the debris the flag itself contributed — see [`HdlcDeframer::close`].
    nbits: u8,
    frame: Vec<u8>,
    /// True between an opening flag and the next flag or abort.
    in_frame: bool,
    /// Latched when the frame outgrew `max_bytes`; keeps the vector bounded while the
    /// oversized frame runs out, and drops it at the closing flag.
    overflow: bool,
}

impl HdlcDeframer {
    #[must_use]
    pub fn new(min_bytes: usize, max_bytes: usize) -> Self {
        assert!(
            min_bytes > 0 && max_bytes >= min_bytes,
            "hdlc frame bounds must be non-empty and ordered"
        );
        Self {
            min_bytes,
            max_bytes,
            ones: 0,
            byte: 0,
            nbits: 0,
            frame: Vec::with_capacity(max_bytes),
            in_frame: false,
            overflow: false,
        }
    }

    /// Feed one bit; returns a complete frame when a closing flag arrives.
    pub fn push(&mut self, bit: bool) -> Option<Vec<u8>> {
        match self.ones {
            0..=4 => {
                self.ones = if bit { self.ones + 1 } else { 0 };
                self.append(bit);
                None
            }
            // A zero after five ones was inserted by the transmitter and is never data.
            5 => {
                self.ones = if bit { 6 } else { 0 };
                None
            }
            6 if !bit => self.close(),
            // Seven ones: abort. Only a flag re-opens a frame after this.
            _ => {
                self.ones = if bit { 7 } else { 0 };
                self.in_frame = false;
                self.discard();
                None
            }
        }
    }

    pub fn reset(&mut self) {
        self.ones = 0;
        self.in_frame = false;
        self.discard();
    }

    fn append(&mut self, bit: bool) {
        if !self.in_frame {
            return;
        }
        self.byte |= u8::from(bit) << self.nbits;
        self.nbits += 1;
        if self.nbits == 8 {
            if self.frame.len() == self.max_bytes {
                self.overflow = true;
            } else {
                self.frame.push(self.byte);
            }
            self.byte = 0;
            self.nbits = 0;
        }
    }

    fn discard(&mut self) {
        self.frame.clear();
        self.byte = 0;
        self.nbits = 0;
        self.overflow = false;
    }

    /// A flag closes any frame in progress and opens the next one (adjacent frames share a
    /// flag). The flag's own leading zero and first five ones are appended as data before it
    /// is recognised, so a byte-aligned frame always leaves 6 bits of debris — or 5 when the
    /// transmitter's stuffed zero swallowed the flag's leading zero. Any other remainder
    /// means the bit stream was not byte-aligned and the frame is not a frame.
    fn close(&mut self) -> Option<Vec<u8>> {
        let aligned = matches!(self.nbits, 5 | 6);
        let keep = self.in_frame
            && aligned
            && !self.overflow
            && (self.min_bytes..=self.max_bytes).contains(&self.frame.len());
        let out =
            keep.then(|| std::mem::replace(&mut self.frame, Vec::with_capacity(self.max_bytes)));
        self.ones = 0;
        self.in_frame = true;
        self.discard();
        out
    }
}

/// Multiplicative (self-synchronising) descrambler: `out = in XOR reg[t]…`, register fed with
/// the received stream. Defaults to G3RUH's x^17 + x^12 + 1 (9600 baud packet, AIS).
///
/// Self-synchronising means no preamble is needed, at the price of error multiplication: one
/// channel bit error produces one output error per tap plus one.
#[derive(Clone, Debug)]
pub struct Descrambler {
    reg: u32,
    taps: u32,
}

impl Descrambler {
    /// x^17 + x^12 + 1.
    #[must_use]
    pub fn g3ruh() -> Self {
        Self::new(&[17, 12])
    }

    /// `taps` are polynomial exponents (the implicit `+ 1` term is the input itself).
    #[must_use]
    pub fn new(taps: &[u8]) -> Self {
        Self {
            reg: 0,
            taps: taps_mask(taps),
        }
    }

    pub fn push(&mut self, bit: bool) -> bool {
        let out = bit ^ feedback(self.reg, self.taps);
        self.reg = (self.reg << 1) | u32::from(bit);
        out
    }

    pub fn reset(&mut self) {
        self.reg = 0;
    }
}

/// The scrambler matching [`Descrambler`]: same feedback, but the register is fed with the
/// transmitted (scrambled) bit so both registers see the same sequence.
#[derive(Clone, Debug)]
pub struct Scrambler {
    reg: u32,
    taps: u32,
}

impl Scrambler {
    /// x^17 + x^12 + 1.
    #[must_use]
    pub fn g3ruh() -> Self {
        Self::new(&[17, 12])
    }

    /// `taps` are polynomial exponents (the implicit `+ 1` term is the input itself).
    #[must_use]
    pub fn new(taps: &[u8]) -> Self {
        Self {
            reg: 0,
            taps: taps_mask(taps),
        }
    }

    pub fn push(&mut self, bit: bool) -> bool {
        let out = bit ^ feedback(self.reg, self.taps);
        self.reg = (self.reg << 1) | u32::from(out);
        out
    }

    pub fn reset(&mut self) {
        self.reg = 0;
    }
}

/// Exponent `k` addresses the bit delayed by `k`, held at register bit `k − 1`.
fn taps_mask(taps: &[u8]) -> u32 {
    let mut mask = 0u32;
    for &k in taps {
        assert!((1..=32).contains(&k), "tap exponent {k} outside 1..=32");
        mask |= 1 << (k - 1);
    }
    mask
}

fn feedback(reg: u32, taps: u32) -> bool {
    (reg & taps).count_ones() % 2 == 1
}

/// Sliding sync-word correlator: shifts bits into a register (first bit toward the MSB of the
/// significant field) and reports a match when the Hamming distance to `word` is within
/// `tolerance`.
#[derive(Clone, Debug)]
pub struct SyncDetector {
    reg: u64,
    word: u64,
    mask: u64,
    tolerance: u32,
}

impl SyncDetector {
    #[must_use]
    pub fn new(word: u64, bits: u32, tolerance: u32) -> Self {
        assert!((1..=64).contains(&bits), "sync word must be 1..=64 bits");
        assert!(tolerance <= bits, "tolerance exceeds sync word length");
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        Self {
            reg: 0,
            word: word & mask,
            mask,
            tolerance,
        }
    }

    pub fn push(&mut self, bit: bool) -> bool {
        self.reg = ((self.reg << 1) | u64::from(bit)) & self.mask;
        hamming_distance(self.reg, self.word) <= self.tolerance
    }

    /// The significant bits currently in the register, oldest at the MSB.
    #[must_use]
    pub fn register(&self) -> u64 {
        self.reg
    }

    pub fn reset(&mut self) {
        self.reg = 0;
    }
}

#[must_use]
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

#[must_use]
pub fn reverse_byte(b: u8) -> u8 {
    b.reverse_bits()
}

/// Pack bits into bytes, most significant bit first. A trailing partial byte is zero-padded.
#[must_use]
pub fn pack_msb(bits: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len().div_ceil(8)];
    for (i, &bit) in bits.iter().enumerate() {
        out[i / 8] |= u8::from(bit) << (7 - i % 8);
    }
    out
}

/// Pack bits into bytes, least significant bit first. A trailing partial byte is zero-padded.
#[must_use]
pub fn pack_lsb(bits: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; bits.len().div_ceil(8)];
    for (i, &bit) in bits.iter().enumerate() {
        out[i / 8] |= u8::from(bit) << (i % 8);
    }
    out
}

/// Read a big-endian bit field of `len` bits starting at bit `offset` (MSB of byte 0 = bit 0).
/// Returns 0 when the field runs past the end — callers bounds-check their own frames.
#[must_use]
pub fn bits_be(bytes: &[u8], offset: usize, len: usize) -> u64 {
    if len == 0 || len > 64 {
        return 0;
    }
    let (Some(end), Some(available)) = (offset.checked_add(len), bytes.len().checked_mul(8)) else {
        return 0;
    };
    if end > available {
        return 0;
    }
    let mut value = 0u64;
    for i in offset..end {
        value = (value << 1) | u64::from(bytes[i / 8] >> (7 - i % 8) & 1);
    }
    value
}

/// Manchester/biphase symbol pair -> data bit; `None` when the pair has no transition (an
/// error). A 1 is high-then-low — the G. E. Thomas convention, as used by RDS (IEEE 802.3 is
/// the complement, 1 = low-then-high).
#[must_use]
pub fn manchester_decode(first: bool, second: bool) -> Option<bool> {
    (first != second).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAG: [bool; 8] = [false, true, true, true, true, true, true, false];

    fn nrzi_encode(bits: &[bool]) -> Vec<bool> {
        let mut level = false;
        bits.iter()
            .map(|&b| {
                if !b {
                    level = !level;
                }
                level
            })
            .collect()
    }

    /// Payload bits LSB-first with a zero stuffed after every five ones.
    fn stuff(payload: &[u8]) -> Vec<bool> {
        let mut out = Vec::new();
        let mut ones = 0;
        for &byte in payload {
            for i in 0..8 {
                let bit = byte >> i & 1 == 1;
                out.push(bit);
                ones = if bit { ones + 1 } else { 0 };
                if ones == 5 {
                    out.push(false);
                    ones = 0;
                }
            }
        }
        out
    }

    fn hdlc_frame(payload: &[u8]) -> Vec<bool> {
        let mut bits = FLAG.to_vec();
        bits.extend(stuff(payload));
        bits.extend(FLAG);
        bits
    }

    fn deframe(deframer: &mut HdlcDeframer, bits: &[bool]) -> Vec<Vec<u8>> {
        bits.iter().filter_map(|&b| deframer.push(b)).collect()
    }

    #[test]
    fn nrzi_decodes_hand_encoded_levels() {
        // 1 holds the level, 0 toggles it, starting low.
        let levels = [false, true, true, true, false, true, true];
        let mut nrzi = NrziDecoder::new();
        let bits: Vec<bool> = levels.iter().map(|&l| nrzi.decode(l)).collect();
        assert_eq!(bits, [true, false, true, true, false, false, true]);
    }

    #[test]
    fn nrzi_round_trips_and_resets() {
        let data: Vec<bool> = (0..64)
            .map(|i: u32| i.count_ones().is_multiple_of(3))
            .collect();
        let mut nrzi = NrziDecoder::new();
        let decoded: Vec<bool> = nrzi_encode(&data).iter().map(|&l| nrzi.decode(l)).collect();
        assert_eq!(decoded, data);

        nrzi.reset();
        assert!(nrzi.decode(false), "reset must restore the low idle level");
    }

    #[test]
    fn differential_decodes_against_previous_bit() {
        let mut diff = DifferentialDecoder::new();
        let input = [true, true, false, false, true];
        let out: Vec<bool> = input.iter().map(|&b| diff.decode(b)).collect();
        assert_eq!(out, [true, false, true, false, true]);

        diff.reset();
        assert!(diff.decode(true));
    }

    #[test]
    fn hdlc_returns_exactly_the_payload() {
        let payload = [0x82u8, 0xa0, 0x01, 0x5c];
        let mut bits = vec![false; 5];
        bits.extend(hdlc_frame(&payload));
        let frames = deframe(&mut HdlcDeframer::new(2, 64), &bits);
        assert_eq!(frames, vec![payload.to_vec()]);
    }

    #[test]
    fn hdlc_unstuffs_long_runs_of_ones() {
        // 0xff then 0x03 is ten consecutive ones LSB-first — two stuffed zeros to remove.
        let payload = [0xffu8, 0x03, 0x7f];
        let bits = hdlc_frame(&payload);
        assert!(
            bits.len() > 8 + payload.len() * 8 + 8,
            "fixture must actually contain stuffed bits"
        );
        let frames = deframe(&mut HdlcDeframer::new(2, 64), &bits);
        assert_eq!(frames, vec![payload.to_vec()]);
    }

    #[test]
    fn hdlc_drops_frames_outside_the_length_bounds() {
        let mut deframer = HdlcDeframer::new(4, 8);
        assert!(deframe(&mut deframer, &hdlc_frame(&[0x11, 0x22])).is_empty());
        assert!(deframe(&mut deframer, &hdlc_frame(&[0xaa; 9])).is_empty());
        // The bounded frames on either side still decode, so the drops left no residue.
        let ok = [0x11u8, 0x22, 0x33, 0x44];
        assert_eq!(deframe(&mut deframer, &hdlc_frame(&ok)), vec![ok.to_vec()]);
    }

    #[test]
    fn hdlc_handles_frames_sharing_one_flag() {
        let (first, second) = ([0x01u8, 0x02, 0x03], [0xf8u8, 0xfc, 0xfe]);
        let mut bits = FLAG.to_vec();
        bits.extend(stuff(&first));
        bits.extend(FLAG);
        bits.extend(stuff(&second));
        bits.extend(FLAG);
        let frames = deframe(&mut HdlcDeframer::new(2, 64), &bits);
        assert_eq!(frames, vec![first.to_vec(), second.to_vec()]);
    }

    #[test]
    fn hdlc_abort_drops_the_frame_in_progress() {
        let payload = [0x11u8, 0x22, 0x33];
        let mut bits = FLAG.to_vec();
        bits.extend(stuff(&payload));
        bits.extend([true; 7]);
        bits.push(false);
        bits.extend(hdlc_frame(&payload));
        let frames = deframe(&mut HdlcDeframer::new(2, 64), &bits);
        assert_eq!(
            frames,
            vec![payload.to_vec()],
            "only the clean frame survives"
        );
    }

    #[test]
    fn hdlc_reset_discards_a_partial_frame() {
        let payload = [0x11u8, 0x22, 0x33];
        let bits = hdlc_frame(&payload);
        let mut deframer = HdlcDeframer::new(2, 64);
        let split = bits.len() - 4;
        assert!(deframe(&mut deframer, &bits[..split]).is_empty());
        deframer.reset();
        assert!(deframe(&mut deframer, &bits[split..]).is_empty());
        assert_eq!(deframe(&mut deframer, &bits), vec![payload.to_vec()]);
    }

    #[test]
    fn hdlc_rejects_a_misaligned_frame() {
        // One extra data bit before the closing flag: complete octets exist, alignment does not.
        let mut bits = FLAG.to_vec();
        bits.extend(stuff(&[0x11, 0x22, 0x33]));
        bits.push(false);
        bits.extend(FLAG);
        assert!(deframe(&mut HdlcDeframer::new(2, 64), &bits).is_empty());
    }

    #[test]
    fn descrambler_inverts_scrambler() {
        let data: Vec<bool> = (0..500).map(|i: u32| i % 7 < 3).collect();
        let mut tx = Scrambler::g3ruh();
        let mut rx = Descrambler::g3ruh();
        let out: Vec<bool> = data.iter().map(|&b| rx.push(tx.push(b))).collect();
        assert_eq!(out, data);
    }

    #[test]
    fn descrambler_multiplies_a_single_error_by_the_tap_count() {
        let data: Vec<bool> = (0..200)
            .map(|i: u32| i.count_ones().is_multiple_of(2))
            .collect();
        let mut tx = Scrambler::g3ruh();
        let mut channel: Vec<bool> = data.iter().map(|&b| tx.push(b)).collect();
        let flip = 100;
        channel[flip] = !channel[flip];

        let mut rx = Descrambler::g3ruh();
        let out: Vec<bool> = channel.iter().map(|&b| rx.push(b)).collect();
        let errors: Vec<usize> = (0..data.len()).filter(|&i| out[i] != data[i]).collect();
        // x^17 + x^12 + 1: the bit itself, then 12 and 17 bits later.
        assert_eq!(errors, vec![flip, flip + 12, flip + 17]);
    }

    #[test]
    fn scrambler_whitens_a_constant_input() {
        let mut tx = Scrambler::new(&[17, 12]);
        let out: Vec<bool> = (0..1024).map(|_| tx.push(true)).collect();
        let ones = out.iter().filter(|&&b| b).count();
        assert!(
            (412..612).contains(&ones),
            "unbalanced: {ones} ones in 1024"
        );

        let longest = out
            .chunk_by(|a, b| a == b)
            .map(<[bool]>::len)
            .max()
            .unwrap_or(0);
        assert!(longest < 24, "run of {longest} defeats the clock recovery");
    }

    #[test]
    fn sync_detector_matches_within_tolerance() {
        const WORD: u64 = 0xabcd;
        let bits: Vec<bool> = (0..16).rev().map(|i| WORD >> i & 1 == 1).collect();

        for (errors, expect) in [(0usize, true), (1, true), (2, false)] {
            let mut detector = SyncDetector::new(WORD, 16, 1);
            let mut corrupted = bits.clone();
            for c in corrupted.iter_mut().take(errors) {
                *c = !*c;
            }
            let hits = corrupted.iter().filter(|&&b| detector.push(b)).count();
            assert_eq!(hits > 0, expect, "{errors} bit errors");
        }
    }

    #[test]
    fn sync_detector_slides_over_leading_noise() {
        const WORD: u64 = 0x7e;
        let mut detector = SyncDetector::new(WORD, 8, 0);
        let noise = [true, false, true, false, false];
        assert!(!noise.iter().any(|&b| detector.push(b)));
        let word: Vec<bool> = (0..8).rev().map(|i| WORD >> i & 1 == 1).collect();
        let hit_at = word.iter().position(|&b| detector.push(b));
        assert_eq!(
            hit_at,
            Some(7),
            "match only once the word is fully shifted in"
        );
        assert_eq!(detector.register(), WORD);

        detector.reset();
        assert_eq!(detector.register(), 0);
    }

    #[test]
    fn hamming_distance_counts_differing_bits() {
        assert_eq!(hamming_distance(0, 0), 0);
        assert_eq!(hamming_distance(0b1011, 0b0001), 2);
        assert_eq!(hamming_distance(u64::MAX, 0), 64);
    }

    #[test]
    fn reverse_byte_flips_bit_order() {
        assert_eq!(reverse_byte(0b1000_0001), 0b1000_0001);
        assert_eq!(reverse_byte(0b0101_0000), 0b0000_1010);
        assert_eq!(reverse_byte(0x01), 0x80);
    }

    #[test]
    fn pack_msb_and_lsb_are_bit_reverses_of_each_other() {
        let bits = [true, false, true, false, false, false, false, true];
        assert_eq!(pack_msb(&bits), [0xa1]);
        assert_eq!(pack_lsb(&bits), [0x85]);
        assert_eq!(pack_lsb(&bits)[0], reverse_byte(pack_msb(&bits)[0]));

        // A partial byte is zero-padded at the unused end.
        assert_eq!(pack_msb(&[true; 3]), [0xe0]);
        assert_eq!(pack_lsb(&[true; 3]), [0x07]);
        assert!(pack_msb(&[]).is_empty());
    }

    #[test]
    fn bits_be_reads_fields_across_byte_boundaries() {
        let bytes = [0xa1u8, 0x5c];
        assert_eq!(bits_be(&bytes, 0, 8), 0xa1);
        assert_eq!(bits_be(&bytes, 4, 8), 0x15);
        assert_eq!(bits_be(&bytes, 12, 4), 0b1100);
        assert_eq!(bits_be(&bytes, 0, 16), 0xa15c);
        assert_eq!(bits_be(&[0xff; 8], 0, 64), u64::MAX);
    }

    #[test]
    fn bits_be_returns_zero_past_the_end() {
        let bytes = [0xa1u8, 0x5c];
        assert_eq!(bits_be(&bytes, 12, 8), 0);
        assert_eq!(bits_be(&bytes, 16, 1), 0);
        assert_eq!(bits_be(&bytes, 0, 0), 0);
        assert_eq!(bits_be(&bytes, 0, 65), 0);
        assert_eq!(bits_be(&[], 0, 1), 0);
    }

    #[test]
    fn manchester_needs_a_mid_symbol_transition() {
        assert_eq!(manchester_decode(true, false), Some(true));
        assert_eq!(manchester_decode(false, true), Some(false));
        assert_eq!(manchester_decode(true, true), None);
        assert_eq!(manchester_decode(false, false), None);
    }
}
