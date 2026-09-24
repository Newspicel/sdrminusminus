use std::f32::consts::TAU;

use num_complex::Complex;

use super::symbol::{SYMBOL_BITS, symbol_at};

pub const RATE: f64 = 8_000.0;
pub const BAUD: f64 = 100.0;
pub const SHIFT_HZ: f64 = 85.0;
pub const PHASING: u8 = 125;
const SAMPLES_PER_BIT: usize = (RATE / BAUD) as usize;
const TIMING_GAIN: f64 = 0.05;
const FREQ_ALPHA: f32 = 0.0005;
const RESUM_EVERY: u32 = 1 << 16;

pub struct FskDemod {
    prev_sample: Complex<f32>,
    freq_offset: f32,
    center_phase: f32,
    shift_phase: f32,
    shift_step: f32,
    mark: [Complex<f32>; SAMPLES_PER_BIT],
    space: [Complex<f32>; SAMPLES_PER_BIT],
    mark_sum: Complex<f64>,
    space_sum: Complex<f64>,
    slot: usize,
    since_resum: u32,
    prev_metric: f32,
    timing: f64,
}

impl FskDemod {
    pub fn new() -> Self {
        Self {
            prev_sample: Complex::new(0.0, 0.0),
            freq_offset: 0.0,
            center_phase: 0.0,
            shift_phase: 0.0,
            shift_step: (f64::from(TAU) * SHIFT_HZ / RATE) as f32,
            mark: [Complex::new(0.0, 0.0); SAMPLES_PER_BIT],
            space: [Complex::new(0.0, 0.0); SAMPLES_PER_BIT],
            mark_sum: Complex::new(0.0, 0.0),
            space_sum: Complex::new(0.0, 0.0),
            slot: 0,
            since_resum: 0,
            prev_metric: 0.0,
            timing: 0.0,
        }
    }

    fn track_carrier(&mut self, sample: Complex<f32>) {
        let raw = (sample * self.prev_sample.conj()).arg();
        self.prev_sample = sample;
        self.freq_offset += FREQ_ALPHA * (raw - self.freq_offset);
        self.center_phase = (self.center_phase + self.freq_offset).rem_euclid(TAU);
        self.shift_phase = (self.shift_phase + self.shift_step).rem_euclid(TAU);
    }

    fn correlate(&mut self, sample: Complex<f32>) -> (f32, f32) {
        let mark = sample * Complex::from_polar(1.0, -(self.center_phase + self.shift_phase));
        let space = sample * Complex::from_polar(1.0, -(self.center_phase - self.shift_phase));
        self.mark_sum += widen(mark) - widen(self.mark[self.slot]);
        self.space_sum += widen(space) - widen(self.space[self.slot]);
        self.mark[self.slot] = mark;
        self.space[self.slot] = space;
        self.slot = (self.slot + 1) % SAMPLES_PER_BIT;
        self.since_resum += 1;
        if self.since_resum >= RESUM_EVERY {
            self.since_resum = 0;
            self.mark_sum = self.mark.iter().copied().map(widen).sum();
            self.space_sum = self.space.iter().copied().map(widen).sum();
        }
        (
            self.mark_sum.norm_sqr() as f32,
            self.space_sum.norm_sqr() as f32,
        )
    }

    pub fn process(&mut self, input: &[Complex<f32>], soft: &mut Vec<f32>) {
        let samples_per_bit = SAMPLES_PER_BIT as f64;
        for &sample in input {
            self.track_carrier(sample);
            let (mark, space) = self.correlate(sample);
            let metric = mark - space;
            if metric != 0.0
                && self.prev_metric != 0.0
                && (metric < 0.0) != (self.prev_metric < 0.0)
            {
                self.timing -= TIMING_GAIN * (self.timing - samples_per_bit / 2.0);
            }
            self.prev_metric = metric;
            self.timing += 1.0;
            if self.timing >= samples_per_bit {
                self.timing -= samples_per_bit;
                soft.push(metric / (mark + space).max(f32::MIN_POSITIVE));
            }
        }
    }
}

fn widen(sample: Complex<f32>) -> Complex<f64> {
    Complex::new(f64::from(sample.re), f64::from(sample.im))
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

    #[test]
    fn soft_bits_follow_the_tones() {
        let bits = [1u8, 0, 1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1];
        let mut pattern: Vec<u8> = (0..60).map(|index| (index % 2) as u8).collect();
        pattern.extend(bits);
        let iq = crate::dsc::modulate::modulate_iq(&pattern, RATE, 12.0, SHIFT_HZ, 0.5);
        let mut soft = Vec::new();
        FskDemod::new().process(&iq, &mut soft);
        let hard: Vec<u8> = soft.iter().map(|&value| u8::from(value >= 0.0)).collect();
        let found = hard.windows(bits.len()).any(|window| window == bits);
        assert!(found, "{hard:?}");
    }
}
