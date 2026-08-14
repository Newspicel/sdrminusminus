use crate::Sample;

/// Turns raw device bytes into samples for one capture thread.
///
/// Implementations are stateful — a block can end mid-sample — and must be cheap: this runs on
/// the capture thread for every block, at the full sample rate.
pub trait SampleConverter: Send + 'static {
    /// Convert one block. The returned slice may borrow a buffer the converter reuses.
    fn convert(&mut self, bytes: &[u8]) -> &[Sample];

    /// Forget any partial sample.
    ///
    /// Called when the stream is re-armed: a block that stalled can complete on an *odd* length,
    /// and the byte left over belongs to a sample whose other half is never coming. Carried into
    /// the fresh stream it would swap I and Q for good.
    fn reset(&mut self);
}

/// Interleaved 8-bit IQ to `cf32` through a 256-entry table.
///
/// Allocation-free after the first block: the output buffer is reused, and `convert` hands out a
/// borrow of it.
#[derive(Debug)]
pub struct LutConverter {
    /// One entry per ADC code, in the radio's own coding.
    table: &'static [f32; 256],
    out: Vec<Sample>,
    /// A block whose length is odd ends mid-sample. Dropping that byte would swap I and Q for
    /// the entire rest of the stream, so it is held back and prepended to the next block.
    carry: Option<u8>,
}

impl LutConverter {
    /// A converter over `table`, with room for `samples` before it has to grow.
    #[must_use]
    pub fn new(table: &'static [f32; 256], samples: usize) -> Self {
        Self {
            table,
            out: Vec::with_capacity(samples),
            carry: None,
        }
    }

    /// One ADC code as its sample value.
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

    /// A table with a recognisable shape: code `n` becomes `n` as an `f32`, so a test can name
    /// the byte it expects instead of a normalised fraction.
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

    /// The rule the carry exists for: a short block must not shift every later sample by one
    /// byte, which would swap I and Q for the rest of the stream.
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

    /// However a stream is cut into blocks, the samples that come out are the same ones. This is
    /// the property the carry exists for, stated directly.
    #[test]
    fn any_split_of_a_stream_yields_the_same_samples() {
        let bytes: Vec<u8> = (0..64u16).map(|b| b as u8).collect();
        let whole = converter().convert(&bytes).to_vec();
        // 7 and 5 are odd, so every block but the first starts mid-sample.
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
