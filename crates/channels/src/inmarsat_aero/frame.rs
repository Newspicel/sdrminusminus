use sdrmm_dsp::SoftViterbi;
use serde_json::{Value, json};

pub(super) const UW: u32 = 0xE15A_E893;
pub(super) const HEADER_BITS: usize = 16;
pub(super) const CODED_BITS: usize = 1152;
pub(super) const HIGH_RATE_BPS: u32 = 10_500;
const OVERLAP: usize = 62;
const INTERLEAVER_ROWS: usize = 64;
const ROW_STEP: usize = 27;
const SCRAMBLER_SEED: [u8; 15] = [1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1];
const POLYNOMIAL_FIRST: u32 = 0o133;
const POLYNOMIAL_SECOND: u32 = 0o171;
const CONSTRAINT_LENGTH: u32 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FrameHeader {
    pub format_id: u8,
    pub superframe: u8,
    pub frame_counter1: u8,
    pub frame_counter2: u8,
}

impl FrameHeader {
    pub(super) fn from_u16(word: u16) -> Self {
        Self {
            format_id: ((word >> 12) & 0x0F) as u8,
            superframe: ((word >> 8) & 0x0F) as u8,
            frame_counter1: ((word >> 4) & 0x0F) as u8,
            frame_counter2: (word & 0x0F) as u8,
        }
    }

    pub(super) fn from_soft_bits(bits: &[f32]) -> Self {
        let word = bits
            .iter()
            .take(HEADER_BITS)
            .fold(0u16, |word, &bit| (word << 1) | u16::from(bit >= 0.0));
        Self::from_u16(word)
    }

    pub(super) fn to_json(self) -> Value {
        json!({
            "format_id": self.format_id,
            "superframe": self.superframe,
            "frame_counter1": self.frame_counter1,
            "frame_counter2": self.frame_counter2,
        })
    }
}

pub(super) struct Scrambler {
    state: [u8; 15],
}

impl Scrambler {
    pub(super) fn new() -> Self {
        Self {
            state: SCRAMBLER_SEED,
        }
    }

    fn next_bit(&mut self) -> u8 {
        let out = self.state[0] ^ self.state[14];
        self.state.rotate_right(1);
        self.state[0] = out;
        out
    }

    pub(super) fn apply(&mut self, bits: &mut [u8]) {
        for bit in bits {
            *bit ^= self.next_bit();
        }
    }
}

pub(super) fn deinterleave(soft: &[f32], columns: usize, out: &mut Vec<f32>) {
    for column in 0..columns {
        for row in 0..INTERLEAVER_ROWS {
            out.push(soft[((ROW_STEP * row) % INTERLEAVER_ROWS) * columns + column]);
        }
    }
}

pub(super) fn pack_lsb_first(bits: &[u8]) -> Vec<u8> {
    bits.chunks_exact(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u8, |byte, (index, &bit)| byte | (bit << index))
        })
        .collect()
}

pub(super) fn aero_viterbi() -> SoftViterbi {
    SoftViterbi::new(CONSTRAINT_LENGTH, POLYNOMIAL_FIRST, POLYNOMIAL_SECOND)
}

fn columns_for(rate_bps: u32) -> usize {
    match rate_bps {
        HIGH_RATE_BPS => 78,
        rate if rate >= 1200 => 9,
        _ => 6,
    }
}

pub(super) fn coded_bits_for(rate_bps: u32) -> usize {
    if rate_bps == HIGH_RATE_BPS {
        INTERLEAVER_ROWS * columns_for(HIGH_RATE_BPS)
    } else {
        CODED_BITS
    }
}

pub(super) struct FrameDecoder {
    rate_bps: u32,
    viterbi: SoftViterbi,
    tail: [f32; OVERLAP],
    input: Vec<f32>,
    last_fec_corrected: u32,
}

impl FrameDecoder {
    pub(super) fn new(rate_bps: u32) -> Self {
        Self {
            rate_bps,
            viterbi: aero_viterbi(),
            tail: [0.0; OVERLAP],
            input: Vec::with_capacity(OVERLAP + coded_bits_for(rate_bps)),
            last_fec_corrected: 0,
        }
    }

    pub(super) fn last_fec_corrected(&self) -> u32 {
        self.last_fec_corrected
    }

    pub(super) fn decode(&mut self, coded_soft: &[f32]) -> Vec<u8> {
        let columns = columns_for(self.rate_bps);
        let block = INTERLEAVER_ROWS * columns;
        self.input.clear();
        self.input.extend_from_slice(&self.tail);
        for chunk in coded_soft.chunks_exact(block) {
            deinterleave(chunk, columns, &mut self.input);
        }
        let carried = self.input.len() - OVERLAP;
        self.tail.copy_from_slice(&self.input[carried..]);
        let decoded = self.viterbi.decode(&self.input);
        self.last_fec_corrected = self.count_corrections(&decoded);
        let skip = OVERLAP / 2;
        let count = coded_bits_for(self.rate_bps) / 2;
        let Some(frame_bits) = decoded.get(skip..skip + count) else {
            return Vec::new();
        };
        let mut bits = frame_bits.to_vec();
        Scrambler::new().apply(&mut bits);
        pack_lsb_first(&bits)
    }

    fn count_corrections(&self, decoded: &[u8]) -> u32 {
        let reencoded = self.viterbi.encode(decoded);
        let differing = reencoded
            .iter()
            .zip(&self.input)
            .skip(OVERLAP)
            .filter(|&(&bit, &soft)| bit != u8::from(soft >= 0.0))
            .count();
        u32::try_from(differing).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
pub(super) use encoder::{FRAME_BITS, FrameEncoder, frame_bytes_for, interleave};

#[cfg(test)]
mod encoder {
    use super::*;

    pub const FRAME_BITS: usize = 32 + HEADER_BITS + CODED_BITS;

    pub fn frame_bytes_for(rate_bps: u32) -> usize {
        coded_bits_for(rate_bps) / 16
    }

    pub fn interleave(bits: &[u8], columns: usize, out: &mut Vec<u8>) {
        let mut block = vec![0u8; INTERLEAVER_ROWS * columns];
        for (index, &bit) in bits.iter().enumerate() {
            let row = index % INTERLEAVER_ROWS;
            block[((ROW_STEP * row) % INTERLEAVER_ROWS) * columns + index / INTERLEAVER_ROWS] =
                bit;
        }
        out.extend_from_slice(&block);
    }

    impl FrameHeader {
        pub fn to_u16(self) -> u16 {
            (u16::from(self.format_id & 0x0F) << 12)
                | (u16::from(self.superframe & 0x0F) << 8)
                | (u16::from(self.frame_counter1 & 0x0F) << 4)
                | u16::from(self.frame_counter2 & 0x0F)
        }
    }

    pub struct FrameEncoder {
        rate_bps: u32,
        viterbi: SoftViterbi,
        state_bits: Vec<u8>,
    }

    impl FrameEncoder {
        pub fn new(rate_bps: u32) -> Self {
            Self {
                rate_bps,
                viterbi: aero_viterbi(),
                state_bits: vec![0; 6],
            }
        }

        pub fn encode(&mut self, su_bytes: &[u8], frame_counter: u8) -> Vec<u8> {
            let mut bits: Vec<u8> = su_bytes
                .iter()
                .flat_map(|&byte| (0..8).map(move |index| (byte >> index) & 1))
                .collect();
            Scrambler::new().apply(&mut bits);
            let mut input = self.state_bits.clone();
            input.extend_from_slice(&bits);
            self.state_bits = bits[bits.len() - 6..].to_vec();
            let coded_all = self.viterbi.encode(&input);
            let coded = &coded_all[12..];
            let columns = columns_for(self.rate_bps);
            let mut out = Vec::with_capacity(FRAME_BITS);
            out.extend((0..32).rev().map(|index| ((UW >> index) & 1) as u8));
            let header = FrameHeader {
                format_id: 1,
                superframe: 0,
                frame_counter1: frame_counter & 0x0F,
                frame_counter2: frame_counter & 0x0F,
            }
            .to_u16();
            out.extend((0..16).rev().map(|index| ((header >> index) & 1) as u8));
            for chunk in coded.chunks_exact(INTERLEAVER_ROWS * columns) {
                interleave(chunk, columns, &mut out);
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inmarsat_aero::state::{CarrierState, SuperframeLockStateMachine};

    fn antipodal(bits: &[u8]) -> Vec<f32> {
        bits.iter()
            .map(|&bit| if bit == 1 { 1.0 } else { -1.0 })
            .collect()
    }

    fn header_soft(superframe: u8, counter: u8) -> Vec<f32> {
        let word = FrameHeader {
            format_id: 1,
            superframe: superframe & 0xF,
            frame_counter1: counter & 0xF,
            frame_counter2: counter & 0xF,
        }
        .to_u16();
        (0..HEADER_BITS)
            .rev()
            .map(|index| if (word >> index) & 1 == 1 { 1.0 } else { -1.0 })
            .collect()
    }

    #[test]
    fn scrambler_keystream_matches_the_spec() {
        let expected = "000100110001101111000100001001010000111110001100";
        let mut scrambler = Scrambler::new();
        let got: String = (0..48)
            .map(|_| char::from(b'0' + scrambler.next_bit()))
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn superframe_lock_acquires_then_loses_on_synthetic_sequence() {
        let mut machine = SuperframeLockStateMachine::with_thresholds(3, 4);
        let mut acquired_at = None;
        for counter in 0..=5u8 {
            let status = machine.update(FrameHeader::from_soft_bits(&header_soft(0, counter)));
            if status.superframe_lock && acquired_at.is_none() {
                acquired_at = Some(counter);
            }
        }
        assert_eq!(acquired_at, Some(3));
        let locked = machine.status();
        assert!(locked.superframe_lock && locked.dcd && locked.afc_locked);
        assert_eq!(locked.carrier_state, CarrierState::Locked);
        let held: Vec<bool> = [11u8, 3, 14, 7]
            .iter()
            .map(|&counter| {
                machine
                    .update(FrameHeader::from_soft_bits(&header_soft(0, counter)))
                    .superframe_lock
            })
            .collect();
        assert_eq!(held, [true, true, true, false]);
        let after = machine.status();
        assert!(!after.superframe_lock && !after.afc_locked);
        assert_eq!(after.carrier_state, CarrierState::Searching);
    }

    #[test]
    fn frame_roundtrip_both_rates() {
        for rate in [600u32, 1200] {
            let mut encoder = FrameEncoder::new(rate);
            let mut decoder = FrameDecoder::new(rate);
            for frame in 0..4u8 {
                let bytes: Vec<u8> = (0..72)
                    .map(|index| (index as u8).wrapping_mul(7) ^ frame.wrapping_mul(31))
                    .collect();
                let bits = encoder.encode(&bytes, frame);
                assert_eq!(bits.len(), FRAME_BITS);
                assert_eq!(decoder.decode(&antipodal(&bits[48..])), bytes);
            }
        }
    }

    #[test]
    fn frame_header_splits_nibbles() {
        let header = FrameHeader::from_u16(0x1234);
        assert_eq!(
            (
                header.format_id,
                header.superframe,
                header.frame_counter1,
                header.frame_counter2
            ),
            (1, 2, 3, 4)
        );
        assert_eq!(header.to_u16(), 0x1234);
        let bits: Vec<f32> = (0..16)
            .rev()
            .map(|index| if (0xABCDu16 >> index) & 1 == 1 { 0.9 } else { -0.9 })
            .collect();
        let json = FrameHeader::from_soft_bits(&bits).to_json();
        assert_eq!(json["format_id"], 0xA);
        assert_eq!(json["superframe"], 0xB);
        assert_eq!(json["frame_counter1"], 0xC);
        assert_eq!(json["frame_counter2"], 0xD);
    }

    #[test]
    fn frame_header_roundtrips_through_encoder() {
        let mut encoder = FrameEncoder::new(600);
        for counter in [0u8, 3, 9, 15] {
            let bits = encoder.encode(&vec![0u8; frame_bytes_for(600)], counter);
            let header = FrameHeader::from_soft_bits(&antipodal(&bits[32..48]));
            assert_eq!(header.format_id, 1);
            assert_eq!(header.superframe, 0);
            assert_eq!(header.frame_counter1, counter);
            assert_eq!(header.frame_counter2, counter);
        }
    }

    #[test]
    fn survives_sparse_coded_errors() {
        let mut encoder = FrameEncoder::new(1200);
        let mut decoder = FrameDecoder::new(1200);
        let bytes: Vec<u8> = (0..72).map(|index| index as u8 ^ 0x5A).collect();
        decoder.decode(&[-1.0; CODED_BITS]);
        let mut soft = antipodal(&encoder.encode(&bytes, 0)[48..]);
        for index in (5..soft.len()).step_by(40) {
            soft[index] = -soft[index];
        }
        assert_eq!(decoder.decode(&soft), bytes);
    }

    #[test]
    fn fec_corrected_counts_real_corrections() {
        for rate in [600u32, 1200] {
            let mut encoder = FrameEncoder::new(rate);
            let mut decoder = FrameDecoder::new(rate);
            let bytes: Vec<u8> = (0..frame_bytes_for(rate))
                .map(|index| (index as u8).wrapping_mul(13) ^ 0xA5)
                .collect();
            let clean = antipodal(&encoder.encode(&bytes, 0)[48..]);
            assert_eq!(decoder.decode(&clean), bytes);
            assert_eq!(decoder.last_fec_corrected(), 0);
            let mut noisy = antipodal(&encoder.encode(&bytes, 1)[48..]);
            let flips: Vec<usize> = (37..coded_bits_for(rate)).step_by(97).collect();
            for &index in &flips {
                noisy[index] = -noisy[index];
            }
            assert_eq!(decoder.decode(&noisy), bytes);
            assert_eq!(decoder.last_fec_corrected(), flips.len() as u32);
        }
    }
}
