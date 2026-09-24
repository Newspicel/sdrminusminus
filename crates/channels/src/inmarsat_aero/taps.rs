use std::f64::consts::{PI, SQRT_2};

use num_complex::Complex;

const BLACKMAN_HARRIS: [f64; 4] = [0.35875, 0.48829, 0.14128, 0.01168];

fn blackman_harris(count: usize) -> Vec<f64> {
    (0..count)
        .map(|index| {
            let x = 2.0 * PI * index as f64 / (count as f64 - 1.0);
            BLACKMAN_HARRIS[0] - BLACKMAN_HARRIS[1] * x.cos() + BLACKMAN_HARRIS[2] * (2.0 * x).cos()
                - BLACKMAN_HARRIS[3] * (3.0 * x).cos()
        })
        .collect()
}

pub(super) fn lowpass_taps(cutoff: f64, count: usize) -> Vec<f32> {
    let window = blackman_harris(count);
    let center = (count as f64 - 1.0) / 2.0;
    let taps: Vec<f64> = window
        .iter()
        .enumerate()
        .map(|(index, weight)| {
            let t = index as f64 - center;
            let sinc = if t.abs() < 1e-12 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * t).sin() / (PI * t)
            };
            sinc * weight
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.into_iter().map(|tap| (tap / sum) as f32).collect()
}

pub(super) fn rrc_taps(samples_per_symbol: f64, count: usize, beta: f64) -> Vec<f32> {
    let middle = (count - 1) as f64 / 2.0;
    let taps: Vec<f64> = (0..count)
        .map(|index| rrc_pulse((index as f64 - middle) / samples_per_symbol, beta))
        .collect();
    let energy = taps.iter().map(|tap| tap * tap).sum::<f64>().sqrt();
    taps.into_iter().map(|tap| (tap / energy) as f32).collect()
}

fn rrc_pulse(t: f64, beta: f64) -> f64 {
    if t.abs() < 1e-9 {
        return 1.0 - beta + 4.0 * beta / PI;
    }
    if (t.abs() - 1.0 / (4.0 * beta)).abs() < 1e-9 {
        let quarter = PI / (4.0 * beta);
        return (beta / SQRT_2)
            * ((1.0 + 2.0 / PI) * quarter.sin() + (1.0 - 2.0 / PI) * quarter.cos());
    }
    let pt = PI * t;
    ((pt * (1.0 - beta)).sin() + 4.0 * beta * t * (pt * (1.0 + beta)).cos())
        / (pt * (1.0 - (4.0 * beta * t).powi(2)))
}

pub(super) struct Fir {
    taps: Vec<f32>,
    history: Vec<Complex<f32>>,
    head: usize,
    decimation: usize,
    phase: usize,
}

impl Fir {
    pub(super) fn new(taps: Vec<f32>, decimation: usize) -> Self {
        let length = taps.len();
        Self {
            taps,
            history: vec![Complex::new(0.0, 0.0); 2 * length],
            head: 0,
            decimation: decimation.max(1),
            phase: 0,
        }
    }

    pub(super) fn filter(&mut self, sample: Complex<f32>) -> Complex<f32> {
        self.push(sample);
        self.output()
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        for &sample in input {
            self.push(sample);
            self.phase += 1;
            if self.phase == self.decimation {
                self.phase = 0;
                out.push(self.output());
            }
        }
    }

    fn push(&mut self, sample: Complex<f32>) {
        let length = self.taps.len();
        self.head = if self.head == 0 {
            length - 1
        } else {
            self.head - 1
        };
        self.history[self.head] = sample;
        self.history[self.head + length] = sample;
    }

    fn output(&self) -> Complex<f32> {
        self.history[self.head..self.head + self.taps.len()]
            .iter()
            .zip(&self.taps)
            .fold(Complex::new(0.0, 0.0), |sum, (value, &tap)| sum + *value * tap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowpass_passes_dc_and_blocks_high_frequencies() {
        let mut fir = Fir::new(lowpass_taps(0.1, 101), 1);
        let mut out = Vec::new();
        fir.process(&vec![Complex::new(1.0, 0.0); 512], &mut out);
        let mean = out[200..].iter().map(|value| value.re).sum::<f32>() / 312.0;
        assert!((mean - 1.0).abs() < 1e-3);
        let tone: Vec<Complex<f32>> = (0..512)
            .map(|index| Complex::from_polar(1.0, (index as f32) * 0.4 * std::f32::consts::TAU))
            .collect();
        out.clear();
        fir.process(&tone, &mut out);
        let power = out[200..].iter().map(|value| value.norm_sqr()).sum::<f32>() / 312.0;
        assert!(power < 1e-6);
    }

    #[test]
    fn decimation_keeps_every_second_output() {
        let taps = lowpass_taps(0.05, 15);
        let input: Vec<Complex<f32>> = (0..64)
            .map(|index| Complex::new(index as f32, -(index as f32)))
            .collect();
        let mut full = Fir::new(taps.clone(), 1);
        let mut half = Fir::new(taps, 2);
        let (mut every, mut decimated) = (Vec::new(), Vec::new());
        full.process(&input, &mut every);
        half.process(&input, &mut decimated);
        let expected: Vec<_> = every.iter().skip(1).step_by(2).copied().collect();
        assert_eq!(decimated, expected);
    }

    #[test]
    fn rrc_has_unit_energy() {
        let energy: f32 = rrc_taps(9.14, 55, 1.0).iter().map(|tap| tap * tap).sum();
        assert!((energy - 1.0).abs() < 1e-5);
    }
}
