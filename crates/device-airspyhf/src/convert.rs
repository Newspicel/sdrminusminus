use sdrmm_device::{Sample, SampleConverter};

/// The radio sends signed 16-bit pairs, imaginary part first.
const FULL_SCALE: f32 = 32_768.0;
const BYTES_PER_SAMPLE: usize = 4;

#[derive(Debug)]
pub(crate) struct AirspyHfConverter {
    out: Vec<Sample>,
    carry: Vec<u8>,
}

impl AirspyHfConverter {
    pub(crate) fn new(samples: usize) -> Self {
        Self {
            out: Vec::with_capacity(samples),
            carry: Vec::with_capacity(BYTES_PER_SAMPLE),
        }
    }

    fn push(&mut self, quad: &[u8; BYTES_PER_SAMPLE]) {
        let im = i16::from_le_bytes([quad[0], quad[1]]);
        let re = i16::from_le_bytes([quad[2], quad[3]]);
        self.out.push(Sample::new(
            f32::from(re) / FULL_SCALE,
            f32::from(im) / FULL_SCALE,
        ));
    }
}

impl SampleConverter for AirspyHfConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        self.out.clear();
        let mut rest = bytes;
        if !self.carry.is_empty() {
            let wanted = BYTES_PER_SAMPLE - self.carry.len();
            let take = wanted.min(rest.len());
            self.carry.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if let Ok(quad) = <[u8; BYTES_PER_SAMPLE]>::try_from(self.carry.as_slice()) {
                self.carry.clear();
                self.push(&quad);
            }
        }
        let (quads, remainder) = rest.as_chunks::<BYTES_PER_SAMPLE>();
        for quad in quads {
            self.push(quad);
        }
        self.carry.extend_from_slice(remainder);
        &self.out
    }

    fn reset(&mut self) {
        self.out.clear();
        self.carry.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(re: i16, im: i16) -> [u8; 4] {
        let mut bytes = [0u8; 4];
        bytes[..2].copy_from_slice(&im.to_le_bytes());
        bytes[2..].copy_from_slice(&re.to_le_bytes());
        bytes
    }

    #[test]
    fn the_imaginary_part_arrives_first() {
        let mut converter = AirspyHfConverter::new(4);
        let out = converter.convert(&quad(1000, -2000));
        assert_eq!(out.len(), 1);
        assert!((out[0].re - 1000.0 / FULL_SCALE).abs() < 1e-9);
        assert!((out[0].im + 2000.0 / FULL_SCALE).abs() < 1e-9);
    }

    #[test]
    fn the_rails_reach_full_scale() {
        let mut converter = AirspyHfConverter::new(4);
        let out = converter.convert(&quad(i16::MIN, i16::MAX));
        assert!((out[0].re + 1.0).abs() < 1e-9);
        assert!((out[0].im - 0.99997).abs() < 1e-4);
    }

    #[test]
    fn four_bytes_of_input_become_one_sample() {
        let bytes: Vec<u8> = (0..256).flat_map(|n| quad(n, -n)).collect();
        let mut converter = AirspyHfConverter::new(256);
        assert_eq!(converter.convert(&bytes).len(), 256);
    }

    #[test]
    fn a_transfer_split_inside_a_sample_loses_none_of_it() {
        let bytes: Vec<u8> = (0..512).flat_map(|n| quad(n, -n)).collect();
        let mut whole = AirspyHfConverter::new(512);
        let expected = whole.convert(&bytes).to_vec();

        let mut split = AirspyHfConverter::new(512);
        let mut got = Vec::new();
        let mut at = 0;
        for len in [1usize, 2, 3, 5, 7, 129, 1023].iter().cycle() {
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
    fn a_trailing_part_sample_waits_for_the_rest_rather_than_being_dropped() {
        let bytes = quad(7, 9);
        let mut converter = AirspyHfConverter::new(4);
        assert!(converter.convert(&bytes[..3]).is_empty());
        let out = converter.convert(&bytes[3..]);
        assert_eq!(out.len(), 1);
        assert!((out[0].re - 7.0 / FULL_SCALE).abs() < 1e-9);
    }

    #[test]
    fn a_reset_converter_forgets_a_part_sample() {
        let bytes = quad(7, 9);
        let mut converter = AirspyHfConverter::new(4);
        assert!(converter.convert(&bytes[..3]).is_empty());
        converter.reset();
        assert!(converter.convert(&bytes[..2]).is_empty());
        assert_eq!(converter.convert(&bytes[2..]).len(), 1);
    }
}
