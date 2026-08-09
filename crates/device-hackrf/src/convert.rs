//! Device-edge sample conversion: the HackRF's interleaved *signed* 8-bit IQ to the one `cf32`
//! format the rest of the pipeline speaks, and back again for transmit (PLAN §7).
//!
//! Only the coding is here. The lookup, the reused output buffer and the carry byte that keeps I
//! and Q aligned across a short block are `sdrmm-device`'s
//! [`LutConverter`](sdrmm_device::LutConverter), shared with every other 8-bit radio — the
//! MAX2837 delivers two's-complement samples centred on zero while the RTL2832U delivers
//! unsigned codes around a measured DC offset, and that difference is the whole of what a
//! backend has to supply.
//!
//! The transmit direction lives here beside the receive one, for the same reason the driver
//! below is bytes-only in both directions: the sample format is a property of the radio's
//! converters, not of the USB transport.

use sdrmm_device::{LutConverter, Sample};

use crate::driver::RX_TRANSFER_SIZE;

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

/// A converter for one capture thread, sized so a whole transfer fits without growing.
pub(crate) fn converter() -> LutConverter {
    LutConverter::new(&CODE_TO_F32, RX_TRANSFER_SIZE / 2)
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
/// The inverse of the receive table, and the reason both live in one module.
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
    use sdrmm_device::SampleConverter;

    use super::*;

    fn code_to_f32(code: u8) -> f32 {
        CODE_TO_F32[code as usize]
    }

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

    /// The table reaches the pipeline through the shared converter, so the wiring is worth one
    /// assertion even though the machinery is tested in `sdrmm-device`.
    #[test]
    fn a_block_arrives_as_interleaved_complex_samples_in_this_coding() {
        let samples = converter().convert(&[0x80, 0x00, 0x7f, 0xff]).to_vec();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0], Sample::new(-1.0, 0.0));
        assert_eq!(samples[1], Sample::new(127.0 / 128.0, -1.0 / 128.0));
    }

    #[test]
    fn the_converter_is_sized_for_a_whole_transfer() {
        let block = vec![0u8; RX_TRANSFER_SIZE];
        let mut converter = converter();
        let first = converter.convert(&block).as_ptr();
        assert_eq!(
            converter.convert(&block).as_ptr(),
            first,
            "capture thread must not allocate per block"
        );
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
        let samples = [Sample::new(0.0, 1.0), Sample::new(-1.0, -0.5)];
        let mut out = Vec::new();
        samples_to_cs8(&samples, &mut out);
        assert_eq!(out, vec![0x00, 0x7f, 0x80, 0xc0]);
        let address = out.as_ptr();
        samples_to_cs8(&samples, &mut out);
        assert_eq!(out.as_ptr(), address, "burst path must not reallocate");
    }
}
