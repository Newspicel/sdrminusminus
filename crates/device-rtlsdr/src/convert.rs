use sdrmm_device::{ByteCoding, ByteConverter};

use crate::dongle::TRANSFER_BYTES;

const DC_OFFSET: f32 = 127.4;
const FULL_SCALE: f32 = 127.5;

const CODING: ByteCoding = ByteCoding::OffsetBinary {
    offset: DC_OFFSET,
    full_scale: FULL_SCALE,
};

pub(crate) fn converter() -> ByteConverter {
    ByteConverter::new(CODING, TRANSFER_BYTES / 2)
}

#[cfg(test)]
mod tests {
    use sdrmm_device::SampleConverter;

    use super::*;

    fn code_to_f32(code: u8) -> f32 {
        CODING.level(code)
    }

    #[test]
    fn every_code_converts_as_the_former_table_did() {
        let codes: Vec<u8> = (0..=255u8).collect();
        let samples = converter().convert(&codes).to_vec();
        for (pair, sample) in codes.as_chunks::<2>().0.iter().zip(samples) {
            let [i, q] = pair.map(|code| ((code as f32 - DC_OFFSET) / FULL_SCALE).to_bits());
            assert_eq!((sample.re.to_bits(), sample.im.to_bits()), (i, q));
        }
    }

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
    fn a_block_arrives_as_interleaved_complex_samples_in_this_coding() {
        let samples = converter().convert(&[0, 255, 127, 128]).to_vec();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].re, code_to_f32(0));
        assert_eq!(samples[0].im, code_to_f32(255));
        assert_eq!(samples[1].re, code_to_f32(127));
        assert_eq!(samples[1].im, code_to_f32(128));
    }

    #[test]
    fn the_converter_is_sized_for_a_whole_transfer() {
        let block = vec![0u8; TRANSFER_BYTES];
        let mut converter = converter();
        let first = converter.convert(&block).as_ptr();
        assert_eq!(
            converter.convert(&block).as_ptr(),
            first,
            "capture thread must not allocate per block"
        );
    }
}
