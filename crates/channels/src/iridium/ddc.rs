use std::f64::consts::{PI, TAU};

use num_complex::Complex;

const TAPS_PER_TRANSITION: f64 = 5.5;
const MAX_TAPS: usize = 8192;
const RESYNC_INTERVAL: usize = 256;
const MAX_STAGE_DECIMATION: usize = 16;

struct Nco {
    phase: f64,
    step: f64,
}

impl Nco {
    fn mix(&mut self, samples: &mut [Complex<f32>]) {
        let (ss, sc) = self.step.sin_cos();
        let rotation = Complex::new(sc as f32, ss as f32);
        for chunk in samples.chunks_mut(RESYNC_INTERVAL) {
            let (sin, cos) = self.phase.sin_cos();
            let mut current = Complex::new(cos as f32, sin as f32);
            for x in chunk.iter_mut() {
                *x *= current;
                current *= rotation;
            }
            self.phase += self.step * chunk.len() as f64;
            if self.phase > TAU {
                self.phase %= TAU;
            } else if self.phase < -TAU {
                self.phase = -((-self.phase) % TAU);
            }
        }
    }
}

struct Fir {
    taps: Vec<f32>,
    history: Vec<Complex<f32>>,
    head: usize,
    decimation: usize,
    phase: usize,
}

impl Fir {
    fn new(taps: Vec<f32>, decimation: usize) -> Self {
        let n = taps.len();
        Self {
            taps,
            history: vec![Complex::default(); 2 * n],
            head: 0,
            decimation,
            phase: 0,
        }
    }

    fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let n = self.taps.len();
        for &x in input {
            self.head = if self.head == 0 { n - 1 } else { self.head - 1 };
            self.history[self.head] = x;
            self.history[self.head + n] = x;
            self.phase += 1;
            if self.phase == self.decimation {
                self.phase = 0;
                let window = &self.history[self.head..self.head + n];
                let mut acc = Complex::new(0.0f32, 0.0);
                for (w, &tap) in window.iter().zip(&self.taps) {
                    acc += *w * tap;
                }
                out.push(acc);
            }
        }
    }
}

fn blackman_harris(n: usize) -> Vec<f64> {
    const A: [f64; 4] = [0.35875, 0.48829, 0.14128, 0.01168];
    (0..n)
        .map(|i| {
            let x = 2.0 * PI * i as f64 / (n as f64 - 1.0);
            A[0] - A[1] * x.cos() + A[2] * (2.0 * x).cos() - A[3] * (3.0 * x).cos()
        })
        .collect()
}

fn lowpass_taps(cutoff: f64, num_taps: usize) -> Vec<f32> {
    let window = blackman_harris(num_taps);
    let center = (num_taps as f64 - 1.0) / 2.0;
    let taps: Vec<f64> = (0..num_taps)
        .map(|i| {
            let t = i as f64 - center;
            let sinc = if t.abs() < 1e-12 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * t).sin() / (PI * t)
            };
            sinc * window[i]
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.into_iter().map(|t| (t / sum) as f32).collect()
}

fn stage_factors(decimation: usize) -> Vec<usize> {
    if decimation <= MAX_STAGE_DECIMATION {
        return vec![decimation];
    }
    (2..=MAX_STAGE_DECIMATION)
        .rev()
        .find(|d| decimation % d == 0)
        .map_or_else(|| vec![decimation], |d| vec![decimation / d, d])
}

pub struct ChannelDdc {
    nco: Nco,
    stages: Vec<Fir>,
    mixed: Vec<Complex<f32>>,
    between: Vec<Complex<f32>>,
}

impl ChannelDdc {
    pub fn new(
        input_rate: f64,
        output_rate: f64,
        offset_hz: f64,
        passband_hz: f64,
    ) -> Result<Self, String> {
        let ratio = input_rate / output_rate;
        let decimation = ratio.floor() as usize;
        if decimation == 0 || (ratio - decimation as f64).abs() > 1e-9 {
            return Err(format!("rate {input_rate} is no multiple of {output_rate}"));
        }
        if output_rate < 2.0 * passband_hz {
            return Err(format!("{output_rate} S/s cannot carry ±{passband_hz} Hz"));
        }
        let factors = stage_factors(decimation);
        let last = factors.len() - 1;
        let mut stages = Vec::with_capacity(factors.len());
        let mut rate = input_rate;
        for (index, &factor) in factors.iter().enumerate() {
            let out = rate / factor as f64;
            let transition = out - 2.0 * passband_hz;
            if transition <= 0.0 {
                return Err(format!("{out} S/s too low for ±{passband_hz} Hz"));
            }
            let mut taps = ((TAPS_PER_TRANSITION * rate / transition).ceil() as usize | 1).max(9);
            if index == last && out >= 12.0 * passband_hz {
                let sharp = ((TAPS_PER_TRANSITION * rate / passband_hz).ceil() as usize | 1).max(9);
                taps = taps.max(sharp);
            }
            if taps > MAX_TAPS {
                return Err(format!("decimation {decimation} needs {taps} taps"));
            }
            stages.push(Fir::new(lowpass_taps(passband_hz / rate, taps), factor));
            rate = out;
        }
        Ok(Self {
            nco: Nco {
                phase: 0.0,
                step: -TAU * offset_hz / input_rate,
            },
            stages,
            mixed: Vec::new(),
            between: Vec::new(),
        })
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.mixed.clear();
        self.mixed.extend_from_slice(input);
        self.nco.mix(&mut self.mixed);
        match self.stages.as_mut_slice() {
            [only] => only.process(&self.mixed, out),
            [first, second] => {
                self.between.clear();
                first.process(&self.mixed, &mut self.between);
                second.process(&self.between, out);
            }
            _ => {}
        }
    }
}
