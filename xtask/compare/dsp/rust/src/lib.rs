use std::{hint::black_box, time::Instant};

use num_complex::Complex32;

pub const TAPS: usize = 127;
pub const CUTOFF: f64 = 0.11;
pub const DECIMATION: usize = 4;
pub const FFT: usize = 4096;
pub const DDC_INPUT_RATE: f64 = 20e6;
pub const DDC_OUTPUT_RATE: f64 = 48e3;
pub const DDC_OFFSET: f64 = 187_500.0;
pub const RESAMPLE_RATIO: f64 = 48_000.0 / 44_100.0;

const MAX_REPS: usize = 1001;
const RATIO_STEPS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub block: usize,
    pub reps: usize,
    pub rep_seconds: f64,
}

impl Timing {
    pub fn from_args(args: &[String]) -> Result<Self, String> {
        let [block, reps, rep_seconds] = args else {
            return Err("usage: <block> <reps> <rep_seconds>".to_owned());
        };
        let timing = Self {
            block: block.parse().map_err(|_| format!("bad block {block}"))?,
            reps: reps.parse().map_err(|_| format!("bad reps {reps}"))?,
            rep_seconds: rep_seconds
                .parse()
                .map_err(|_| format!("bad rep_seconds {rep_seconds}"))?,
        };
        let valid = timing.block >= FFT
            && (1..=MAX_REPS).contains(&timing.reps)
            && timing.rep_seconds > 0.0;
        valid
            .then_some(timing)
            .ok_or_else(|| "invalid timing arguments".to_owned())
    }

    pub fn from_env() -> Result<Self, String> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        Self::from_args(&args)
    }
}

fn calibrate(step: &mut impl FnMut(), seconds: f64) -> u64 {
    let mut iterations = 1u64;
    loop {
        let start = Instant::now();
        for _ in 0..iterations {
            step();
        }
        if start.elapsed().as_secs_f64() >= seconds {
            return iterations;
        }
        iterations *= 2;
    }
}

pub fn median(times: &mut [f64]) -> f64 {
    times.sort_by(f64::total_cmp);
    times.get(times.len() / 2).copied().unwrap_or(f64::NAN)
}

pub fn run(id: &str, samples: usize, timing: &Timing, mut step: impl FnMut()) {
    let iterations = calibrate(&mut step, timing.rep_seconds);
    let mut times: Vec<f64> = (0..timing.reps)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..iterations {
                step();
            }
            start.elapsed().as_secs_f64()
        })
        .collect();
    let seconds = median(&mut times);
    println!(
        "{id}\t{:.6}",
        samples as f64 * iterations as f64 / seconds / 1e6
    );
}

pub fn ratio(id: &str, samples: usize, mut step: impl FnMut() -> usize) {
    let produced: usize = (0..RATIO_STEPS).map(|_| step()).sum();
    println!(
        "ratio\t{id}\t{:.9}",
        produced as f64 / (samples * RATIO_STEPS) as f64
    );
}

pub fn keep<T>(value: &T) {
    black_box(value);
}

fn random(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*state >> 8) as f32 / 8_388_608.0 - 1.0
}

pub fn signal(n: usize, seed: u32) -> Vec<Complex32> {
    let mut state = seed;
    (0..n)
        .map(|_| {
            let re = 0.7 * random(&mut state);
            let im = 0.7 * random(&mut state);
            Complex32::new(re, im)
        })
        .collect()
}

pub fn taps(n: usize, cutoff: f64) -> Vec<f32> {
    use std::f64::consts::PI;
    let middle = (n - 1) as f64 / 2.0;
    (0..n)
        .map(|k| {
            let x = k as f64 - middle;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * x).sin() / (PI * x)
            };
            let window = 0.5 - 0.5 * (2.0 * PI * k as f64 / (n - 1) as f64).cos();
            (sinc * window) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn timing_reads_three_arguments() {
        let timing = Timing::from_args(&args(&["8192", "51", "0.02"])).expect("valid");
        assert_eq!(timing.block, 8192);
        assert_eq!(timing.reps, 51);
        assert!(Timing::from_args(&args(&["8192", "51"])).is_err());
        assert!(Timing::from_args(&args(&["1024", "51", "0.02"])).is_err());
        assert!(Timing::from_args(&args(&["8192", "0", "0.02"])).is_err());
    }

    #[test]
    fn median_takes_the_middle_time() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
    }

    #[test]
    fn signal_matches_the_c_generator() {
        let first = signal(1, 0x11D)[0];
        let mut state = 0x11Du32;
        assert_eq!(first.re, 0.7 * random(&mut state));
        assert!(first.norm() < 1.0);
    }

    #[test]
    fn taps_are_symmetric_with_unit_dc_gain() {
        let taps = taps(TAPS, CUTOFF);
        assert_eq!(taps[0], taps[TAPS - 1]);
        let gain: f32 = taps.iter().sum();
        assert!((gain - 1.0).abs() < 0.01, "gain {gain}");
    }
}
