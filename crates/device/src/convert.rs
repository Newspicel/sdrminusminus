use crate::Sample;

pub trait SampleConverter: Send + 'static {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample];

    fn reset(&mut self);
}

#[derive(Debug)]
pub struct LutConverter {
    table: &'static [f32; 256],
    out: Vec<Sample>,
    carry: Option<u8>,
}

impl LutConverter {
    #[must_use]
    pub fn new(table: &'static [f32; 256], samples: usize) -> Self {
        Self {
            table,
            out: Vec::with_capacity(samples),
            carry: None,
        }
    }

    #[must_use]
    pub fn code(&self, code: u8) -> f32 {
        self.table[code as usize]
    }
}

impl SampleConverter for LutConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        self.out.clear();
        if bytes.is_empty() {
            return &self.out;
        }
        let rest = match self.carry.take() {
            Some(i) => {
                self.out.push(Sample::new(
                    self.table[i as usize],
                    self.table[bytes[0] as usize],
                ));
                &bytes[1..]
            }
            None => bytes,
        };
        let (pairs, remainder) = rest.as_chunks::<2>();
        let table = self.table;
        self.out.extend(
            pairs
                .iter()
                .map(|iq| Sample::new(table[iq[0] as usize], table[iq[1] as usize])),
        );
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

    static IDENTITY: [f32; 256] = {
        let mut table = [0.0f32; 256];
        let mut code = 0usize;
        while code < table.len() {
            table[code] = code as f32;
            code += 1;
        }
        table
    };

    fn converter() -> LutConverter {
        LutConverter::new(&IDENTITY, 8)
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
