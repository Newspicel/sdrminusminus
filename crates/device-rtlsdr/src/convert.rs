//! Device-edge sample conversion: the RTL2832U's interleaved unsigned 8-bit IQ to the one
//! `cf32` format the rest of the pipeline speaks (PLAN §7).
//!
//! Only the table is here. The lookup, the reused output buffer and the carry byte that keeps I
//! and Q aligned across a short block are `sdrmm-device`'s
//! [`LutConverter`](sdrmm_device::LutConverter), shared with every other 8-bit radio — the
//! coding is what differs between them, and it is the only thing that does.

use sdrmm_device::LutConverter;

use crate::driver::TRANSFER_BUF_SIZE;

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

/// A converter for one capture thread, sized so a whole transfer fits without growing.
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

    /// The table reaches the pipeline through the shared converter, so the wiring is worth one
    /// assertion even though the machinery is tested in `sdrmm-device`.
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
