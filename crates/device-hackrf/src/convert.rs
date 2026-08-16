use sdrmm_device::{LutConverter, Sample};

use crate::driver::{RX_TRANSFER_SIZE, SWEEP_BLOCK_SAMPLES};

const FULL_SCALE: f32 = 128.0;

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

pub(crate) fn converter() -> LutConverter {
    LutConverter::new(&CODE_TO_F32, RX_TRANSFER_SIZE / 2)
}

pub(crate) fn sweep_converter() -> LutConverter {
    LutConverter::new(&CODE_TO_F32, SWEEP_BLOCK_SAMPLES)
}

fn f32_to_code(value: f32) -> u8 {
    if value.is_finite() {
        (value * FULL_SCALE).round().clamp(-128.0, 127.0) as i8 as u8
    } else {
        0
    }
}

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
        assert_eq!(f32_to_code(1.0) as i8, 127);
        assert_eq!(f32_to_code(9.0) as i8, 127);
        assert_eq!(f32_to_code(-1.0) as i8, -128);
        assert_eq!(f32_to_code(-9.0) as i8, -128);
    }

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
