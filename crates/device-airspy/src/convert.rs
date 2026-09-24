use sdrmm_device::{Sample, SampleConverter};
use sdrmm_dsp::{RealToIq, iir::DcBlocker};

/// The ADC delivers unsigned 12-bit codes, so mid-scale is the zero of the signal.
const MID_SCALE: f32 = 2048.0;
const CODE_MASK: u16 = 0x0fff;

/// Turns the real codes an Airspy sends into the complex baseband the engine reads.
///
/// The radio samples the band around a quarter of its ADC rate and sends what it measured, one
/// real value per sample. The wanted signal only becomes complex here, at half the rate that
/// arrives, which is why the rate the firmware publishes is already the one this produces.
#[derive(Debug)]
pub(crate) struct AirspyConverter {
    dc: DcBlocker,
    converter: RealToIq,
    real: Vec<f32>,
    out: Vec<Sample>,
    carry: Option<u8>,
}

impl AirspyConverter {
    pub(crate) fn new(samples: usize) -> Self {
        Self {
            dc: DcBlocker::new(),
            converter: RealToIq::default(),
            real: Vec::with_capacity(samples),
            out: Vec::with_capacity(samples / 2),
            carry: None,
        }
    }
}

impl SampleConverter for AirspyConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        self.real.clear();
        let mut rest = bytes;
        if let Some(low) = self.carry.take()
            && let Some((high, tail)) = rest.split_first()
        {
            self.real
                .push(code_to_f32(u16::from_le_bytes([low, *high])));
            rest = tail;
        }
        let (pairs, remainder) = rest.as_chunks::<2>();
        for pair in pairs {
            self.real.push(code_to_f32(u16::from_le_bytes(*pair)));
        }
        self.carry = remainder.first().copied();

        self.dc.process(&mut self.real);
        self.converter.process(&self.real, &mut self.out);
        &self.out
    }

    fn reset(&mut self) {
        self.dc = DcBlocker::new();
        self.converter.reset();
        self.real.clear();
        self.out.clear();
        self.carry = None;
    }
}

fn code_to_f32(word: u16) -> f32 {
    (f32::from(word & CODE_MASK) - MID_SCALE) / MID_SCALE
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;

    fn tone_bytes(freq_norm: f64, len: usize) -> Vec<u8> {
        (0..len)
            .flat_map(|n| {
                let value = (TAU * freq_norm * n as f64).cos();
                let code = (MID_SCALE as f64 + value * 2000.0).round() as u16;
                (code & CODE_MASK).to_le_bytes()
            })
            .collect()
    }

    #[test]
    fn mid_scale_is_silence_and_the_rails_are_full_scale() {
        assert!(code_to_f32(2048).abs() < 1e-6);
        assert!((code_to_f32(4095) - 0.9995).abs() < 1e-3);
        assert!((code_to_f32(0) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_four_bits_above_a_code_are_not_part_of_it() {
        assert_eq!(code_to_f32(0xf000 | 2048), code_to_f32(2048));
        assert_eq!(code_to_f32(0xa000 | 3000), code_to_f32(3000));
    }

    #[test]
    fn two_bytes_of_input_become_half_a_complex_sample() {
        let mut converter = AirspyConverter::new(4096);
        let out = converter.convert(&tone_bytes(0.25, 4096));
        assert_eq!(out.len(), 2048);
    }

    #[test]
    fn a_transfer_split_between_two_codes_loses_no_sample() {
        let bytes = tone_bytes(0.26, 4096);
        let mut whole = AirspyConverter::new(4096);
        let expected = whole.convert(&bytes).to_vec();

        let mut split = AirspyConverter::new(4096);
        let mut got = Vec::new();
        let mut at = 0;
        for len in [1usize, 3, 511, 7, 1025].iter().cycle() {
            if at >= bytes.len() {
                break;
            }
            let end = (at + len).min(bytes.len());
            got.extend_from_slice(split.convert(&bytes[at..end]));
            at = end;
        }
        assert_eq!(expected, got);
    }

    #[test]
    fn a_steady_offset_does_not_reach_the_output() {
        let mut converter = AirspyConverter::new(4096);
        let biased: Vec<u8> = (0..4096).flat_map(|_| 3000_u16.to_le_bytes()).collect();
        let out = converter.convert(&biased);
        let settled = &out[512..];
        let mean = settled.iter().sum::<Sample>() / settled.len() as f32;
        assert!(
            mean.norm() < 0.02,
            "a constant code should decay away, left {}",
            mean.norm()
        );
    }

    #[test]
    fn a_reset_converter_repeats_its_first_output() {
        let bytes = tone_bytes(0.3, 2048);
        let mut converter = AirspyConverter::new(2048);
        let first = converter.convert(&bytes).to_vec();
        converter.reset();
        assert_eq!(first, converter.convert(&bytes));
    }
}
