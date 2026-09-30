use crate::Sample;

pub trait SampleConverter: Send + 'static {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample];

    fn reset(&mut self);

    fn bytes_per_sample(&self) -> u64 {
        2
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ByteCoding {
    TwosComplement { full_scale: f32 },
    OffsetBinary { offset: f32, full_scale: f32 },
}

impl ByteCoding {
    #[must_use]
    pub fn level(self, code: u8) -> f32 {
        match self {
            Self::TwosComplement { full_scale } => twos_complement(code, full_scale),
            Self::OffsetBinary { offset, full_scale } => offset_binary(code, offset, full_scale),
        }
    }

    fn sample(self, i: u8, q: u8) -> Sample {
        Sample::new(self.level(i), self.level(q))
    }

    fn extend(self, pairs: &[[u8; 2]], out: &mut Vec<Sample>) {
        match self {
            Self::TwosComplement { full_scale } => out.extend(pairs.iter().map(|&[i, q]| {
                Sample::new(
                    twos_complement(i, full_scale),
                    twos_complement(q, full_scale),
                )
            })),
            Self::OffsetBinary { offset, full_scale } => {
                out.extend(pairs.iter().map(|&[i, q]| {
                    Sample::new(
                        offset_binary(i, offset, full_scale),
                        offset_binary(q, offset, full_scale),
                    )
                }));
            }
        }
    }
}

#[inline(always)]
fn twos_complement(code: u8, full_scale: f32) -> f32 {
    f32::from(code.cast_signed()) / full_scale
}

#[inline(always)]
fn offset_binary(code: u8, offset: f32, full_scale: f32) -> f32 {
    (f32::from(code) - offset) / full_scale
}

#[derive(Debug)]
pub struct ByteConverter {
    coding: ByteCoding,
    out: Vec<Sample>,
    carry: Option<u8>,
}

impl ByteConverter {
    #[must_use]
    pub fn new(coding: ByteCoding, samples: usize) -> Self {
        Self {
            coding,
            out: Vec::with_capacity(samples),
            carry: None,
        }
    }
}

impl SampleConverter for ByteConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        self.out.clear();
        let Some((&first, tail)) = bytes.split_first() else {
            return &self.out;
        };
        let rest = match self.carry.take() {
            Some(i) => {
                self.out.push(self.coding.sample(i, first));
                tail
            }
            None => bytes,
        };
        let (pairs, remainder) = rest.as_chunks::<2>();
        self.coding.extend(pairs, &mut self.out);
        self.carry = remainder.first().copied();
        &self.out
    }

    fn reset(&mut self) {
        self.carry = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: ByteCoding = ByteCoding::OffsetBinary {
        offset: 0.0,
        full_scale: 1.0,
    };

    const SIGNED: ByteCoding = ByteCoding::TwosComplement { full_scale: 128.0 };

    const OFFSET: ByteCoding = ByteCoding::OffsetBinary {
        offset: 127.4,
        full_scale: 127.5,
    };

    static SIGNED_TABLE: [f32; 256] = {
        let mut table = [0.0f32; 256];
        let mut code = 0usize;
        while code < table.len() {
            table[code] = (code as u8 as i8) as f32 / 128.0;
            code += 1;
        }
        table
    };

    static OFFSET_TABLE: [f32; 256] = {
        let mut table = [0.0f32; 256];
        let mut code = 0usize;
        while code < table.len() {
            table[code] = (code as f32 - 127.4) / 127.5;
            code += 1;
        }
        table
    };

    fn converter() -> ByteConverter {
        ByteConverter::new(IDENTITY, 8)
    }

    fn bits(samples: &[Sample]) -> Vec<(u32, u32)> {
        samples
            .iter()
            .map(|s| (s.re.to_bits(), s.im.to_bits()))
            .collect()
    }

    fn by_table(bytes: &[u8], table: &[f32; 256]) -> Vec<Sample> {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|iq| Sample::new(table[iq[0] as usize], table[iq[1] as usize]))
            .collect()
    }

    fn stream(len: usize) -> Vec<u8> {
        (0..len)
            .map(|n| (n as u32).wrapping_mul(2_654_435_761).to_le_bytes()[3])
            .collect()
    }

    #[test]
    fn every_code_matches_the_table_it_replaces_bit_for_bit() {
        let codes: Vec<u8> = (0..=255u8).chain(0..=255u8).collect();
        for (coding, table) in [(SIGNED, &SIGNED_TABLE), (OFFSET, &OFFSET_TABLE)] {
            for code in 0..=255u8 {
                assert_eq!(
                    coding.level(code).to_bits(),
                    table[code as usize].to_bits(),
                    "{coding:?} code {code}"
                );
            }
            for skew in [0, 1] {
                let bytes = &codes[skew..];
                let got = ByteConverter::new(coding, 256).convert(bytes).to_vec();
                assert_eq!(bits(&got), bits(&by_table(bytes, table)), "{coding:?}");
            }
        }
    }

    #[test]
    fn ragged_blocks_match_the_table_bit_for_bit() {
        let bytes = stream(4099);
        for (coding, table) in [(SIGNED, &SIGNED_TABLE), (OFFSET, &OFFSET_TABLE)] {
            let whole = by_table(&bytes, table);
            for split in [1, 3, 15, 17, 33, 4095] {
                let mut converter = ByteConverter::new(coding, 4096);
                let mut pieces = Vec::new();
                for chunk in bytes.chunks(split) {
                    pieces.extend_from_slice(converter.convert(chunk));
                }
                assert_eq!(bits(&pieces), bits(&whole), "{coding:?} split {split}");
            }
        }
    }

    #[test]
    fn a_block_becomes_interleaved_complex_samples() {
        let mut converter = converter();
        let samples = converter.convert(&[1, 2, 3, 4]);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0], Sample::new(1.0, 2.0));
        assert_eq!(samples[1], Sample::new(3.0, 4.0));
    }

    #[test]
    fn an_empty_block_converts_to_nothing() {
        assert!(converter().convert(&[]).is_empty());
    }

    #[test]
    fn an_odd_block_carries_its_last_byte_into_the_next() {
        let mut converter = converter();
        let samples = converter.convert(&[1, 2, 3]);
        assert_eq!(samples, [Sample::new(1.0, 2.0)]);
        let samples = converter.convert(&[4, 5, 6]);
        assert_eq!(samples, [Sample::new(3.0, 4.0), Sample::new(5.0, 6.0)]);
    }

    #[test]
    fn a_reset_drops_the_half_sample_a_restart_orphaned() {
        let mut converter = converter();
        converter.convert(&[1, 2, 3]);
        converter.reset();
        let samples = converter.convert(&[4, 5]);
        assert_eq!(
            samples,
            [Sample::new(4.0, 5.0)],
            "the orphaned byte must not lead the fresh stream"
        );
    }

    #[test]
    fn a_single_byte_block_yields_nothing_and_holds_it() {
        let mut converter = converter();
        assert!(converter.convert(&[7]).is_empty());
        assert_eq!(converter.convert(&[8]), [Sample::new(7.0, 8.0)]);
    }

    #[test]
    fn any_split_of_a_stream_yields_the_same_samples() {
        let bytes: Vec<u8> = (0..64u16).map(|b| b as u8).collect();
        let whole = converter().convert(&bytes).to_vec();
        for split in [7, 5, 1] {
            let mut converter = converter();
            let mut pieces = Vec::new();
            for chunk in bytes.chunks(split) {
                pieces.extend_from_slice(converter.convert(chunk));
            }
            assert_eq!(pieces, whole, "split into {split}-byte blocks");
        }
    }

    #[test]
    fn an_empty_block_preserves_a_pending_carry() {
        let mut converter = converter();
        assert!(converter.convert(&[9]).is_empty());
        assert!(converter.convert(&[]).is_empty());
        assert_eq!(converter.convert(&[11]), [Sample::new(9.0, 11.0)]);
    }

    #[test]
    fn the_output_buffer_is_reused_across_blocks() {
        let mut converter = converter();
        let address = converter.convert(&[1, 2]).as_ptr();
        assert_eq!(
            converter.convert(&[3, 4]).as_ptr(),
            address,
            "conversion must not allocate per block"
        );
    }
}
