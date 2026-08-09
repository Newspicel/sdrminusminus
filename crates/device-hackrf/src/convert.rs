//! Device-edge sample conversion: the HackRF's interleaved *signed* 8-bit IQ to the one `cf32`
//! format the rest of the pipeline speaks (PLAN §7). Pure and allocation-free after the first
//! block, so the capture thread stays real-time and every value is unit-testable.
//!
//! Deliberately not shared with `device-rtlsdr`'s converter, which looks similar and is not: the
//! MAX2837 delivers two's-complement samples centred on zero, while the RTL2832U delivers
//! unsigned codes around a measured 127.4 DC offset. One table cannot be both.
//!
//! The transmit direction lives here too, beside the receive one, for the same reason the driver
//! below is bytes-only in both directions: the sample format is a property of the radio's
//! converters, not of the USB transport.

use num_complex::Complex;
use sdrmm_device::Sample;

/// Half the code span of a signed 8-bit sample, so −128..=127 maps to −1.0..=0.992.
const FULL_SCALE: f32 = 128.0;

/// One entry per ADC code — the conversion is a 1 KB table lookup instead of a per-sample
/// convert and divide, on a path that runs at 2× the sample rate (40 M lookups/s at 20 Msps).
static CODE_TO_F32: [f32; 256] = build_table();

const fn build_table() -> [f32; 256] {
    let mut table = [0.0f32; 256];
    let mut code = 0usize;
    while code < table.len() {
        table[code] = (code as u8 as i8) as f32 / FULL_SCALE;
        code += 1;
    }
    table
}

/// Convert one ADC code to its `f32` sample value.
pub(crate) fn code_to_f32(code: u8) -> f32 {
    CODE_TO_F32[code as usize]
}

/// Stateful cs8→`cf32` converter for a stream of USB blocks.
pub(crate) struct IqConverter {
    out: Vec<Complex<f32>>,
    /// A block whose length is odd ends mid-sample. Dropping that byte would swap I and Q for
    /// the entire rest of the stream, so it is held back and prepended to the next block.
    ///
    /// The HackRF's own transfers are always even, but a *stalled* one can complete short at any
    /// length, and the transport no longer rejects that on the driver's behalf.
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

/// One sample to its transmit code. Non-finite input becomes silence rather than a wrapped
/// integer: a `NaN` reaching the DAC is a full-scale spike on the air.
fn f32_to_code(value: f32) -> u8 {
    if value.is_finite() {
        (value * FULL_SCALE).round().clamp(-128.0, 127.0) as i8 as u8
    } else {
        0
    }
}

/// Append `samples` to `out` as interleaved signed 8-bit IQ, the format the HackRF's DAC takes.
/// The inverse of [`IqConverter::convert`], and the reason both live in one module.
pub(crate) fn samples_to_cs8(samples: &[Sample], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(samples.len() * 2);
    for sample in samples {
        out.push(f32_to_code(sample.re));
        out.push(f32_to_code(sample.im));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_twos_complement_and_normalized() {
        assert_eq!(code_to_f32(0x80), -1.0);
        assert_eq!(code_to_f32(0xff), -1.0 / 128.0);
        assert_eq!(code_to_f32(0x00), 0.0);
        assert_eq!(code_to_f32(0x7f), 127.0 / 128.0);
    }

    #[test]
    fn conversion_is_monotonic_across_the_signed_range() {
        let mut previous = f32::NEG_INFINITY;
        for code in (0x80..=0xffu8).chain(0x00..=0x7f) {
            let value = code_to_f32(code);
            assert!(value > previous, "code 0x{code:02x} not monotonic");
            assert!(value.abs() <= 1.0, "code 0x{code:02x} exceeds full scale");
            previous = value;
        }
    }

    #[test]
    fn block_becomes_interleaved_complex_samples() {
        let mut converter = IqConverter::with_capacity(4);
        let samples = converter.convert(&[0x80, 0x00, 0x7f, 0xff]);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0], Complex::new(-1.0, 0.0));
        assert_eq!(samples[1], Complex::new(127.0 / 128.0, -1.0 / 128.0));
    }

    #[test]
    fn odd_length_blocks_keep_iq_alignment() {
        let bytes: Vec<u8> = (0..64u16).map(|b| b as u8).collect();
        let whole = IqConverter::with_capacity(32).convert(&bytes).to_vec();

        for chunk_len in [5, 7] {
            let mut converter = IqConverter::with_capacity(32);
            let mut split = Vec::new();
            for chunk in bytes.chunks(chunk_len) {
                split.extend_from_slice(converter.convert(chunk));
            }
            assert_eq!(split, whole, "chunks of {chunk_len}");
        }
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
        assert_eq!(samples[0], Complex::new(code_to_f32(9), code_to_f32(11)));
    }

    #[test]
    fn transmit_codes_round_trip_through_the_receive_table() {
        for code in 0..=255u8 {
            assert_eq!(f32_to_code(code_to_f32(code)), code, "code 0x{code:02x}");
        }
    }

    #[test]
    fn transmit_clamps_rather_than_wrapping() {
        // Full scale is +127/128, so +1.0 has no code and must saturate, not wrap to -128.
        assert_eq!(f32_to_code(1.0) as i8, 127);
        assert_eq!(f32_to_code(9.0) as i8, 127);
        assert_eq!(f32_to_code(-1.0) as i8, -128);
        assert_eq!(f32_to_code(-9.0) as i8, -128);
    }

    /// A `NaN` cast to an integer is implementation-defined nonsense that would leave the
    /// antenna at whatever code it produced; silence is the only safe reading.
    #[test]
    fn transmit_turns_non_finite_samples_into_silence() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(f32_to_code(value), 0);
        }
    }

    #[test]
    fn transmit_interleaves_and_reuses_its_buffer() {
        let samples = [Complex::new(0.0, 1.0), Complex::new(-1.0, -0.5)];
        let mut out = Vec::new();
        samples_to_cs8(&samples, &mut out);
        assert_eq!(out, vec![0x00, 0x7f, 0x80, 0xc0]);
        let address = out.as_ptr();
        samples_to_cs8(&samples, &mut out);
        assert_eq!(out.as_ptr(), address, "burst path must not reallocate");
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
