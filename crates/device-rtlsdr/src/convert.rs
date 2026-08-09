//! Device-edge sample conversion: the RTL2832U's interleaved unsigned 8-bit IQ to the one
//! `cf32` format the rest of the pipeline speaks (PLAN §7). Pure and allocation-free after the
//! first block, so the capture thread stays real-time and every value is unit-testable.

use num_complex::Complex;
use sdrmm_device::Sample;

/// Mid-scale of the RTL2832U's 8-bit ADC. It is 127.4, not 127.5: librtlsdr's `rtl_sdr`/
/// `rtl_fm` and SoapyRTLSDR both centre on this measured DC bias, and using the arithmetic
/// mid-point instead leaves a visible DC spike at the centre bin.
const DC_OFFSET: f32 = 127.4;
/// Half the code span, so the full 0..=255 range maps to roughly ±1.0.
const FULL_SCALE: f32 = 127.5;

/// One entry per ADC code — the conversion is a 1 KB table lookup instead of a per-sample
/// integer-to-float convert and divide, on a path that runs at 2× the sample rate.
static CODE_TO_F32: [f32; 256] = build_table();

const fn build_table() -> [f32; 256] {
    let mut table = [0.0f32; 256];
    let mut code = 0usize;
    while code < table.len() {
        table[code] = (code as f32 - DC_OFFSET) / FULL_SCALE;
        code += 1;
    }
    table
}

/// Convert one ADC code to its `f32` sample value.
pub(crate) fn code_to_f32(code: u8) -> f32 {
    CODE_TO_F32[code as usize]
}

/// Stateful u8→`cf32` converter for a stream of USB blocks.
pub(crate) struct IqConverter {
    out: Vec<Complex<f32>>,
    /// A block whose length is odd ends mid-sample. Dropping that byte would swap I and Q for
    /// the entire rest of the stream, so it is held back and prepended to the next block.
    carry: Option<u8>,
}

impl IqConverter {
    pub(crate) fn with_capacity(samples: usize) -> Self {
        Self {
            out: Vec::with_capacity(samples),
            carry: None,
        }
    }

    /// Convert one block. The returned slice borrows the converter's buffer, which is reused
    /// across calls — no allocation once it has grown to the transfer size.
    pub(crate) fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        self.out.clear();
        if bytes.is_empty() {
            return &self.out;
        }
        let rest = match self.carry.take() {
            Some(i) => {
                self.out
                    .push(Complex::new(code_to_f32(i), code_to_f32(bytes[0])));
                &bytes[1..]
            }
            None => bytes,
        };
        let (pairs, remainder) = rest.as_chunks::<2>();
        self.out.extend(
            pairs
                .iter()
                .map(|iq| Complex::new(code_to_f32(iq[0]), code_to_f32(iq[1]))),
        );
        self.carry = remainder.first().copied();
        &self.out
    }

    /// Forget any half sample. Called on the restart path: the byte left over from the block
    /// that stalled belongs to a sample whose other half will never arrive, and prepending it to
    /// the fresh stream would swap I and Q for the rest of the session.
    pub(crate) fn reset(&mut self) {
        self.carry = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_across_full_scale() {
        for (code, expected) in [
            (0u8, -0.999_215_7f32),
            (127, -0.003_137_3),
            (128, 0.004_705_9),
            (255, 1.000_784_3),
        ] {
            let got = code_to_f32(code);
            assert!(
                (got - expected).abs() < 1e-6,
                "code {code}: {got} != {expected}"
            );
        }
    }

    #[test]
    fn conversion_is_monotonic_and_bounded() {
        let mut previous = f32::NEG_INFINITY;
        for code in 0..=255u8 {
            let value = code_to_f32(code);
            assert!(value > previous, "code {code} not monotonic");
            assert!(value.abs() <= 1.001, "code {code} exceeds full scale");
            previous = value;
        }
    }

    #[test]
    fn block_becomes_interleaved_complex_samples() {
        let mut converter = IqConverter::with_capacity(4);
        let samples = converter.convert(&[0, 255, 127, 128]);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].re, code_to_f32(0));
        assert_eq!(samples[0].im, code_to_f32(255));
        assert_eq!(samples[1].re, code_to_f32(127));
        assert_eq!(samples[1].im, code_to_f32(128));
    }

    #[test]
    fn odd_length_blocks_keep_iq_alignment() {
        let bytes: Vec<u8> = (0..64u16).map(|b| b as u8).collect();
        let whole = IqConverter::with_capacity(32).convert(&bytes).to_vec();

        let mut converter = IqConverter::with_capacity(32);
        let mut split = Vec::new();
        // 7 and 5 are odd, so every block but the first starts mid-sample.
        for chunk in bytes.chunks(7) {
            split.extend_from_slice(converter.convert(chunk));
        }
        assert_eq!(split, whole);

        let mut converter = IqConverter::with_capacity(32);
        let mut split = Vec::new();
        for chunk in bytes.chunks(5) {
            split.extend_from_slice(converter.convert(chunk));
        }
        assert_eq!(split, whole);
    }

    /// A stalled transfer can complete on an odd length. Carrying that byte into the restarted
    /// stream would swap I and Q for good, so the restart path resets the converter.
    #[test]
    fn reset_drops_a_half_sample_left_by_a_stall() {
        let mut converter = IqConverter::with_capacity(4);
        assert!(converter.convert(&[0x11]).is_empty());
        converter.reset();
        let samples = converter.convert(&[0x22, 0x33]);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].re, code_to_f32(0x22));
        assert_eq!(samples[0].im, code_to_f32(0x33));
    }

    #[test]
    fn empty_block_preserves_a_pending_carry() {
        let mut converter = IqConverter::with_capacity(4);
        assert!(converter.convert(&[9]).is_empty());
        assert!(converter.convert(&[]).is_empty());
        let samples = converter.convert(&[11]);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].re, code_to_f32(9));
        assert_eq!(samples[0].im, code_to_f32(11));
    }

    #[test]
    fn output_buffer_is_reused_between_blocks() {
        let block = [0u8; 1024];
        let mut converter = IqConverter::with_capacity(block.len() / 2);
        let first = converter.convert(&block).as_ptr();
        let second = converter.convert(&block).as_ptr();
        assert_eq!(first, second, "capture thread must not allocate per block");
    }
}
