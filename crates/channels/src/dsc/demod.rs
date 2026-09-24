use num_complex::Complex;

use super::symbol::{SYMBOL_BITS, symbol_at};

pub const RATE: f64 = 8_000.0;
pub const BAUD: f64 = 100.0;
pub const PHASING: u8 = 125;
const TIMING_GAIN: f64 = 0.10;
const FREQ_ALPHA: f32 = 0.0005;

pub struct FskDemod {
    prev_sample: Complex<f32>,
    prev_disc: f32,
    freq_offset: f32,
    timing: f64,
    samples_per_bit: f64,
    acc: f32,
}

impl FskDemod {
    pub fn new() -> Self {
        Self {
            prev_sample: Complex::new(0.0, 0.0),
            prev_disc: 0.0,
            freq_offset: 0.0,
            timing: 0.0,
            samples_per_bit: RATE / BAUD,
            acc: 0.0,
        }
    }

    pub fn process(&mut self, input: &[Complex<f32>], bits: &mut Vec<u8>) {
        for &sample in input {
            let raw = (sample * self.prev_sample.conj()).arg();
            self.prev_sample = sample;
            self.freq_offset += FREQ_ALPHA * (raw - self.freq_offset);
            let disc = raw - self.freq_offset;
            if disc != 0.0 && self.prev_disc != 0.0 && (disc < 0.0) != (self.prev_disc < 0.0) {
                let error = self.timing
                    - (self.timing / self.samples_per_bit).round() * self.samples_per_bit;
                self.timing -= TIMING_GAIN * error;
            }
            self.prev_disc = disc;
            self.acc += disc;
            self.timing += 1.0;
            if self.timing >= self.samples_per_bit {
                self.timing -= self.samples_per_bit;
                bits.push(u8::from(self.acc >= 0.0));
                self.acc = 0.0;
            }
        }
    }
}

pub fn find_phasing(bits: &[u8]) -> Option<usize> {
    (0..bits.len().saturating_sub(SYMBOL_BITS - 1)).find(|&start| {
        symbol_at(bits, start) == Some((PHASING, true))
            && symbol_at(bits, start + 2 * SYMBOL_BITS).is_none_or(|(_, valid)| valid)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phasing_needs_a_valid_next_dx_symbol() {
        let mut bits = vec![0u8; 3];
        bits.extend([1, 0, 1, 1, 1, 1, 1, 0, 0, 1]);
        bits.extend([0u8; 10]);
        bits.extend([0, 0, 0, 0, 0, 0, 0, 1, 1, 0]);
        assert_eq!(find_phasing(&bits), None);
        let tail = bits.len() - 10;
        bits[tail..].copy_from_slice(&[1, 1, 1, 1, 1, 1, 1, 0, 0, 0]);
        assert_eq!(find_phasing(&bits), Some(3));
        assert_eq!(find_phasing(&bits[..20]), Some(3));
    }
}
