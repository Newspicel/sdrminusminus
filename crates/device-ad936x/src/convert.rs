use sdrmm_device::{Sample, SampleConverter};

use crate::iio::Format;

/// How one scan element is packed, in the form the sample loop needs it.
#[derive(Clone, Copy, Debug)]
struct Coding {
    format: Format,
    /// What one count is worth, so that a full-scale reading arrives at ±1 whatever the width.
    scale: f32,
    bytes: usize,
}

impl Coding {
    fn new(format: Format) -> Self {
        let bits = format.bits.clamp(1, 32);
        let half = (1u64 << (bits - 1)) as f32;
        Self {
            format,
            scale: 1.0 / half,
            bytes: format.storage_bytes().clamp(1, 4),
        }
    }

    const fn pair(self) -> usize {
        self.bytes * 2
    }

    fn decode(self, bytes: &[u8]) -> Sample {
        Sample::new(
            self.element(&bytes[..self.bytes]),
            self.element(&bytes[self.bytes..]),
        )
    }

    fn element(self, bytes: &[u8]) -> f32 {
        let mut raw = 0u32;
        for (place, byte) in bytes.iter().enumerate() {
            let shift = if self.format.little_endian {
                place
            } else {
                bytes.len() - 1 - place
            };
            raw |= u32::from(*byte) << (8 * shift);
        }
        let value = raw >> self.format.shift;
        if self.format.signed {
            sign_extend(value, self.format.bits) as f32 * self.scale
        } else {
            (value as f32).mul_add(self.scale, -1.0)
        }
    }

    fn encode(self, part: f32, out: &mut Vec<u8>) {
        let bits = self.format.bits.clamp(1, 32);
        let half = (1u64 << (bits - 1)) as f32;
        let clamped = part.clamp(-1.0, 1.0);
        let value = if self.format.signed {
            (clamped * half).round().clamp(-half, half - 1.0) as i32
        } else {
            ((clamped + 1.0) * half)
                .round()
                .clamp(0.0, half * 2.0 - 1.0) as i32
        };
        let raw = ((value as u32) << self.format.shift) & self.mask();
        for place in 0..self.bytes {
            let shift = if self.format.little_endian {
                place
            } else {
                self.bytes - 1 - place
            };
            out.push(((raw >> (8 * shift)) & 0xff) as u8);
        }
    }

    const fn mask(self) -> u32 {
        match self.bytes {
            1 => 0xff,
            2 => 0xffff,
            3 => 0x00ff_ffff,
            _ => u32::MAX,
        }
    }
}

/// A 12-bit sample sits in a 16-bit slot already sign-extended, but a firmware that packs it
/// otherwise must not read as a large positive number.
const fn sign_extend(value: u32, bits: u32) -> i32 {
    let bits = if bits == 0 || bits > 32 { 32 } else { bits };
    let spare = 32 - bits;
    ((value << spare) as i32) >> spare
}

/// Turns a buffer of interleaved scan elements into complex samples.
///
/// Every lane of a 2×2 radio has its elements side by side in the one buffer, so the output is
/// the lanes interleaved sample by sample and the caller hands each lane on from there.
#[derive(Debug)]
pub(crate) struct IqConverter {
    coding: Coding,
    out: Vec<Sample>,
    carry: Vec<u8>,
}

impl IqConverter {
    pub(crate) fn new(format: Format, samples: usize) -> Self {
        let coding = Coding::new(format);
        Self {
            coding,
            out: Vec::with_capacity(samples),
            carry: Vec::with_capacity(coding.pair()),
        }
    }
}

impl SampleConverter for IqConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        let coding = self.coding;
        let pair = coding.pair();
        self.out.clear();
        let mut rest = bytes;
        if !self.carry.is_empty() {
            let taken = (pair - self.carry.len()).min(rest.len());
            self.carry.extend_from_slice(&rest[..taken]);
            rest = &rest[taken..];
            if self.carry.len() < pair {
                return &self.out;
            }
            self.out.push(coding.decode(&self.carry));
            self.carry.clear();
        }
        let whole = rest.len() / pair * pair;
        self.out
            .extend(rest[..whole].chunks_exact(pair).map(|iq| coding.decode(iq)));
        self.carry.extend_from_slice(&rest[whole..]);
        &self.out
    }

    fn reset(&mut self) {
        self.carry.clear();
    }
}

/// Writes complex samples back out in the transmit buffer's own element format.
pub(crate) fn to_elements(samples: &[Sample], format: Format, out: &mut Vec<u8>) {
    let coding = Coding::new(format);
    out.clear();
    out.reserve(samples.len() * coding.pair());
    for sample in samples {
        coding.encode(sample.re, out);
        coding.encode(sample.im, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RX: Format = Format {
        little_endian: true,
        signed: true,
        bits: 12,
        storage_bits: 16,
        shift: 0,
    };

    const TX: Format = Format {
        little_endian: true,
        signed: true,
        bits: 16,
        storage_bits: 16,
        shift: 0,
    };

    fn converter(format: Format) -> IqConverter {
        IqConverter::new(format, 64)
    }

    fn le(values: &[i16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn words(bytes: &[u8]) -> Vec<i16> {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect()
    }

    #[test]
    fn a_twelve_bit_sample_reaches_full_scale_at_the_top_of_its_range() {
        let samples = converter(RX).convert(&le(&[2047, -2048, 0, 1024])).to_vec();
        assert_eq!(samples.len(), 2);
        assert!((samples[0].re - 2047.0 / 2048.0).abs() < 1e-6);
        assert!((samples[0].im + 1.0).abs() < 1e-6);
        assert!(samples[1].re.abs() < 1e-6);
        assert!((samples[1].im - 0.5).abs() < 1e-6);
    }

    #[test]
    fn lanes_come_out_interleaved_sample_by_sample() {
        let block = le(&[100, 200, 300, 400, 500, 600, 700, 800]);
        let samples = converter(RX).convert(&block).to_vec();
        assert_eq!(samples.len(), 4, "two lanes of two samples each");
        let lane0: Vec<f32> = samples.iter().step_by(2).map(|s| s.re).collect();
        let lane1: Vec<f32> = samples.iter().skip(1).step_by(2).map(|s| s.re).collect();
        assert!((lane0[0] - 100.0 / 2048.0).abs() < 1e-6);
        assert!((lane1[0] - 300.0 / 2048.0).abs() < 1e-6);
        assert!((lane0[1] - 500.0 / 2048.0).abs() < 1e-6);
        assert!((lane1[1] - 700.0 / 2048.0).abs() < 1e-6);
    }

    #[test]
    fn any_split_of_a_buffer_yields_the_same_samples() {
        let block = le(&(0..32i16).map(|v| v * 37 - 500).collect::<Vec<_>>());
        let whole = converter(RX).convert(&block).to_vec();
        for split in [1, 3, 7, 16] {
            let mut converter = converter(RX);
            let mut pieces = Vec::new();
            for chunk in block.chunks(split) {
                pieces.extend_from_slice(converter.convert(chunk));
            }
            assert_eq!(pieces, whole, "split into {split}-byte pieces");
        }
    }

    #[test]
    fn a_reset_drops_the_half_sample_a_restart_orphaned() {
        let mut converter = converter(RX);
        assert!(converter.convert(&[1, 2, 3]).is_empty());
        converter.reset();
        let samples = converter.convert(&le(&[7, 8])).to_vec();
        assert_eq!(samples.len(), 1);
        assert!((samples[0].re - 7.0 / 2048.0).abs() < 1e-6);
    }

    #[test]
    fn the_output_buffer_is_reused_across_blocks() {
        let mut converter = converter(RX);
        let block = vec![0u8; 4096];
        let first = converter.convert(&block).as_ptr();
        assert_eq!(
            converter.convert(&block).as_ptr(),
            first,
            "the capture thread must not allocate per block"
        );
    }

    #[test]
    fn a_big_endian_unsigned_element_is_read_the_way_it_is_declared() {
        let format = Format {
            little_endian: false,
            signed: false,
            bits: 8,
            storage_bits: 8,
            shift: 0,
        };
        let samples = converter(format).convert(&[255, 128, 0, 128]).to_vec();
        assert_eq!(samples.len(), 2);
        assert!((samples[0].re - (255.0 / 128.0 - 1.0)).abs() < 1e-6);
        assert!(samples[0].im.abs() < 1e-6, "mid code is zero");
        assert!((samples[1].re + 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_shifted_element_is_moved_into_place_before_it_is_read() {
        let format = Format {
            little_endian: true,
            signed: true,
            bits: 12,
            storage_bits: 16,
            shift: 4,
        };
        let samples = converter(format).convert(&le(&[0x0FF0, 0x0010])).to_vec();
        assert!((samples[0].re - 255.0 / 2048.0).abs() < 1e-6);
        assert!((samples[0].im - 1.0 / 2048.0).abs() < 1e-6);
    }

    #[test]
    fn transmit_samples_go_back_out_at_the_scale_they_came_in_at() {
        let mut bytes = Vec::new();
        to_elements(
            &[Sample::new(1.0, -1.0), Sample::new(0.0, 0.5)],
            TX,
            &mut bytes,
        );
        assert_eq!(words(&bytes), vec![i16::MAX, i16::MIN, 0, 16_384]);
    }

    #[test]
    fn a_transmit_sample_beyond_full_scale_is_clipped_rather_than_wrapped() {
        let mut bytes = Vec::new();
        to_elements(&[Sample::new(4.0, -4.0)], TX, &mut bytes);
        assert_eq!(words(&bytes), vec![i16::MAX, i16::MIN]);
    }

    #[test]
    fn what_goes_out_comes_back_as_what_went_in() {
        let sent = [
            Sample::new(0.25, -0.5),
            Sample::new(0.0, 0.0),
            Sample::new(-0.75, 0.125),
        ];
        let mut bytes = Vec::new();
        to_elements(&sent, TX, &mut bytes);
        let back = converter(TX).convert(&bytes).to_vec();
        assert_eq!(back.len(), sent.len());
        for (out, back) in sent.iter().zip(back) {
            assert!((out.re - back.re).abs() < 1e-4, "{out} vs {back}");
            assert!((out.im - back.im).abs() < 1e-4, "{out} vs {back}");
        }
    }

    #[test]
    fn a_transmit_buffer_is_reused_between_writes() {
        let mut bytes = Vec::new();
        to_elements(&[Sample::new(0.25, 0.25); 8], TX, &mut bytes);
        let first = bytes.as_ptr();
        to_elements(&[Sample::new(0.5, 0.5); 4], TX, &mut bytes);
        assert_eq!(bytes.as_ptr(), first);
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn sign_extension_matches_the_declared_width() {
        assert_eq!(sign_extend(0xFFF, 12), -1);
        assert_eq!(sign_extend(0x800, 12), -2048);
        assert_eq!(sign_extend(0x7FF, 12), 2047);
        assert_eq!(sign_extend(0xFFFF, 16), -1);
        assert_eq!(sign_extend(0x7FFF_FFFF, 32), 0x7FFF_FFFF);
    }
}
