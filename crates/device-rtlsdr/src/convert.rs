use sdrmm_device::LutConverter;

use crate::driver::TRANSFER_BUF_SIZE;

const DC_OFFSET: f32 = 127.4;
const FULL_SCALE: f32 = 127.5;

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

pub(crate) fn converter() -> LutConverter {
    LutConverter::new(&CODE_TO_F32, TRANSFER_BUF_SIZE / 2)
}

#[cfg(test)]
mod tests {
    use sdrmm_device::SampleConverter;

    use super::*;

    fn code_to_f32(code: u8) -> f32 {
        CODE_TO_F32[code as usize]
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
        let block = vec![0u8; TRANSFER_BUF_SIZE];
        let mut converter = converter();
        let first = converter.convert(&block).as_ptr();
        assert_eq!(
            converter.convert(&block).as_ptr(),
            first,
            "capture thread must not allocate per block"
        );
    }
}
