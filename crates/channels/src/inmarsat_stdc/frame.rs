use sdrmm_dsp::SoftViterbi;

pub const FRAME_SYMBOLS: usize = 10_368;
pub const ROWS: usize = 64;
pub const COLS: usize = 162;
pub const FRAME_BYTES: usize = 640;
pub const UW: [u8; 8] = [0x07, 0xEA, 0xCD, 0xDA, 0x4E, 0x2F, 0x28, 0xC2];
pub const UW_MIN_MATCH: u32 = 121;
pub const UW_SYMBOLS: u32 = (ROWS * 2) as u32;
const DATA_COLS: usize = COLS - 2;
const ROW_PERMUTATION: usize = 23;
const POLY_FIRST: u32 = 0o133;
const POLY_SECOND: u32 = 0o171;

pub fn uw_bit(row: usize) -> u8 {
    (UW[row / 8] >> (7 - row % 8)) & 1
}

fn hard(soft: f32) -> u8 {
    u8::from(soft > 0.0)
}

fn row_matches(soft: &[f32], row: usize) -> u8 {
    let expect = uw_bit(row);
    u8::from(hard(soft[row * COLS]) == expect) + u8::from(hard(soft[row * COLS + 1]) == expect)
}

pub fn uw_score(soft: &[f32]) -> (u32, u32) {
    let normal: u32 = (0..ROWS).map(|row| u32::from(row_matches(soft, row))).sum();
    (normal, UW_SYMBOLS - normal)
}

pub fn uw_ber_ppt(matches: u32) -> u32 {
    let errors = UW_SYMBOLS.saturating_sub(matches);
    (errors * 1000 + UW_SYMBOLS / 2) / UW_SYMBOLS
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolarityFlip {
    pub flip_symbol: usize,
    pub first_half_inverted: bool,
    pub uw_score: u32,
}

pub fn detect_polarity_flip(soft: &[f32], min_gain: u32) -> Option<PolarityFlip> {
    let rows: [u32; ROWS] = std::array::from_fn(|row| u32::from(row_matches(soft, row)));
    let total_normal: u32 = rows.iter().sum();
    let total_inverted = UW_SYMBOLS - total_normal;
    let base = total_normal.max(total_inverted);
    let mut best: Option<(usize, u32, bool)> = None;
    let mut prefix_normal = 0u32;
    let mut prefix_inverted = 0u32;
    for boundary in 1..ROWS {
        prefix_normal += rows[boundary - 1];
        prefix_inverted += 2 - rows[boundary - 1];
        let normal_first = prefix_normal + (total_inverted - prefix_inverted);
        let inverted_first = prefix_inverted + (total_normal - prefix_normal);
        let (score, first_inverted) = if normal_first >= inverted_first {
            (normal_first, false)
        } else {
            (inverted_first, true)
        };
        if best.is_none_or(|(_, best_score, _)| score > best_score) {
            best = Some((boundary, score, first_inverted));
        }
    }
    let (row, score, first_half_inverted) = best?;
    (score >= base + min_gain).then_some(PolarityFlip {
        flip_symbol: row * COLS,
        first_half_inverted,
        uw_score: score,
    })
}

pub fn apply_polarity_flip(soft: &mut [f32], flip: &PolarityFlip) {
    let end = FRAME_SYMBOLS.min(soft.len());
    let range = if flip.first_half_inverted {
        0..flip.flip_symbol
    } else {
        flip.flip_symbol..end
    };
    for symbol in &mut soft[range] {
        *symbol = -*symbol;
    }
}

pub fn descramble(frame: &mut [u8; FRAME_BYTES]) {
    let mut register: u8 = 0x80;
    for group in frame.as_chunks_mut::<4>().0 {
        let out = register & 1;
        let feedback = out ^ ((register >> 2) & 1) ^ ((register >> 3) & 1) ^ ((register >> 4) & 1);
        register = (register >> 1) | (feedback << 7);
        if out == 1 {
            for byte in group {
                *byte ^= 0xFF;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStats {
    pub fec_corrected: u32,
    pub uw_ber_ppt: u32,
}

pub struct FrameDecoder {
    viterbi: SoftViterbi,
    deleaved: Vec<f32>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self {
            viterbi: SoftViterbi::new(7, POLY_FIRST, POLY_SECOND),
            deleaved: vec![0.0; ROWS * DATA_COLS],
        }
    }

    pub fn decode(&mut self, soft: &[f32], invert: bool) -> ([u8; FRAME_BYTES], FrameStats) {
        let sign = if invert { -1.0 } else { 1.0 };
        for row in 0..ROWS {
            let source = ((row * ROW_PERMUTATION) % ROWS) * COLS;
            for col in 0..DATA_COLS {
                self.deleaved[col * ROWS + row] = soft[source + col + 2] * sign;
            }
        }
        let bits = self.viterbi.decode(&self.deleaved);
        let fec_corrected = self
            .viterbi
            .encode(&bits)
            .iter()
            .zip(&self.deleaved)
            .map(|(&coded, &received)| u32::from(hard(received) != coded))
            .sum();
        let mut frame = [0u8; FRAME_BYTES];
        for (byte, chunk) in frame.iter_mut().zip(bits.as_chunks::<8>().0) {
            *byte = chunk
                .iter()
                .enumerate()
                .fold(0u8, |acc, (bit, &value)| acc | (value << bit));
        }
        descramble(&mut frame);
        (
            frame,
            FrameStats {
                fec_corrected,
                uw_ber_ppt: 0,
            },
        )
    }
}

#[cfg(test)]
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = [0u8; FRAME_BYTES];
    bytes[..payload.len()].copy_from_slice(payload);
    descramble(&mut bytes);
    let bits: Vec<u8> = bytes
        .iter()
        .flat_map(|&byte| (0..8).map(move |bit| (byte >> bit) & 1))
        .collect();
    let coded = SoftViterbi::new(7, POLY_FIRST, POLY_SECOND).encode(&bits);
    let mut matrix = vec![0u8; ROWS * DATA_COLS];
    for (index, &bit) in coded.iter().enumerate() {
        matrix[(index % ROWS) * DATA_COLS + index / ROWS] = bit;
    }
    let mut out = vec![0u8; FRAME_SYMBOLS];
    for row in 0..ROWS {
        let original = (row * 39) % ROWS;
        out[row * COLS] = uw_bit(row);
        out[row * COLS + 1] = uw_bit(row);
        out[row * COLS + 2..(row + 1) * COLS]
            .copy_from_slice(&matrix[original * DATA_COLS..(original + 1) * DATA_COLS]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn antipodal(symbols: &[u8], invert: bool) -> Vec<f32> {
        symbols
            .iter()
            .map(|&bit| if (bit == 1) != invert { 1.0 } else { -1.0 })
            .collect()
    }

    fn payload(seed: u8) -> Vec<u8> {
        (0..639u16)
            .map(|index| (index as u8).wrapping_mul(31) ^ seed)
            .collect()
    }

    #[test]
    fn descrambler_table_prefix() {
        let expected = [
            0u8, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0,
            0, 0, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0, 1, 1, 1, 0, 0, 1, 0, 0, 0, 0,
        ];
        let mut frame = [0u8; FRAME_BYTES];
        descramble(&mut frame);
        for (index, &bit) in expected.iter().enumerate() {
            assert_eq!(frame[index * 4] == 0xFF, bit == 1, "table entry {index}");
        }
    }

    #[test]
    fn frame_roundtrip_bits() {
        let data = payload(0x5C);
        let symbols = encode_frame(&data);
        assert_eq!(symbols.len(), FRAME_SYMBOLS);
        let soft = antipodal(&symbols, false);
        assert_eq!(uw_score(&soft), (128, 0));
        let (frame, stats) = FrameDecoder::new().decode(&soft, false);
        assert_eq!(&frame[..639], &data[..]);
        assert_eq!(stats.fec_corrected, 0);
    }

    #[test]
    fn inverted_frame_roundtrip() {
        let data = payload(0xA7);
        let soft = antipodal(&encode_frame(&data), true);
        let (normal, inverted) = uw_score(&soft);
        assert!(inverted >= UW_MIN_MATCH && normal < 8);
        assert_eq!(&FrameDecoder::new().decode(&soft, true).0[..639], &data[..]);
    }

    #[test]
    fn fec_corrected_counts_overrides() {
        let data = payload(0x17);
        let mut soft = antipodal(&encode_frame(&data), false);
        let mut flipped = 0u32;
        for index in (300..soft.len()).step_by(53) {
            soft[index] = -soft[index];
            flipped += 1;
        }
        let (frame, stats) = FrameDecoder::new().decode(&soft, false);
        assert_eq!(&frame[..639], &data[..]);
        assert!(stats.fec_corrected > 0 && stats.fec_corrected <= flipped + 4);
    }

    #[test]
    fn uw_ber_matches_error_count() {
        assert_eq!(uw_ber_ppt(UW_SYMBOLS), 0);
        assert_eq!(uw_ber_ppt(0), 1000);
        assert_eq!(uw_ber_ppt(124), 31);
    }

    #[test]
    fn mid_frame_polarity_flip_recovered() {
        let data = payload(0x3C);
        let mut soft = antipodal(&encode_frame(&data), false);
        for symbol in &mut soft[40 * COLS..] {
            *symbol = -*symbol;
        }
        let (normal, inverted) = uw_score(&soft);
        assert!(normal < UW_MIN_MATCH && inverted < UW_MIN_MATCH);
        let flip = detect_polarity_flip(&soft, 24).expect("flip detected");
        assert!(flip.uw_score >= UW_MIN_MATCH);
        apply_polarity_flip(&mut soft, &flip);
        assert_eq!(
            &FrameDecoder::new().decode(&soft, false).0[..639],
            &data[..]
        );
    }

    #[test]
    fn no_false_flip_on_clean_frame() {
        let soft = antipodal(&encode_frame(&payload(0)), false);
        assert!(detect_polarity_flip(&soft, 24).is_none());
    }
}
