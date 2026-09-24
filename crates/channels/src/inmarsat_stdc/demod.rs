use std::{f32::consts::TAU, f64::consts::PI, sync::Arc};

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

pub const RATE: f64 = 12_000.0;
pub const SYMBOL_RATE: f64 = 1_200.0;
pub const RRC_BETA: f64 = 0.6;
const COARSE_FFT: usize = 8_192;
const LOWPASS_TAPS: usize = 121;
const LOWPASS_CUTOFF_HZ: f64 = 1_000.0;
const PHASE_GAIN: f32 = 0.05;
const FREQ_GAIN: f32 = 0.002;
const TIMING_GAIN: f64 = 0.02;
const TIMING_STEP_LIMIT: f64 = 0.08;
const AGC_ALPHA: f32 = 0.01;
const CARRIER_ALPHA: f32 = 0.05;
const CARRIER_LOCKED: f32 = 0.7;
const SOFT_LIMIT: f32 = 2.0;
const HISTORY: usize = 24;
const RESNAP_BINS: f32 = 4.0;

pub fn lowpass_taps(cutoff: f64, count: usize) -> Vec<f32> {
    const A: [f64; 4] = [0.35875, 0.48829, 0.14128, 0.01168];
    let center = (count as f64 - 1.0) / 2.0;
    let taps: Vec<f64> = (0..count)
        .map(|index| {
            let x = 2.0 * PI * index as f64 / (count as f64 - 1.0);
            let window = A[0] - A[1] * x.cos() + A[2] * (2.0 * x).cos() - A[3] * (3.0 * x).cos();
            let t = index as f64 - center;
            let sinc = if t.abs() < 1e-12 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * t).sin() / (PI * t)
            };
            sinc * window
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.into_iter().map(|tap| (tap / sum) as f32).collect()
}

pub fn rrc_taps(beta: f64, samples_per_symbol: f64, count: usize) -> Vec<f32> {
    let center = (count as f64 - 1.0) / 2.0;
    let taps: Vec<f64> = (0..count)
        .map(|index| {
            let t = (index as f64 - center) / samples_per_symbol;
            if t.abs() < 1e-12 {
                1.0 - beta + 4.0 * beta / PI
            } else if beta > 0.0 && (t.abs() - 1.0 / (4.0 * beta)).abs() < 1e-9 {
                let a = PI / (4.0 * beta);
                (beta / 2.0_f64.sqrt()) * ((1.0 + 2.0 / PI) * a.sin() + (1.0 - 2.0 / PI) * a.cos())
            } else {
                let numerator =
                    (PI * t * (1.0 - beta)).sin() + 4.0 * beta * t * (PI * t * (1.0 + beta)).cos();
                numerator / (PI * t * (1.0 - (4.0 * beta * t).powi(2)))
            }
        })
        .collect();
    let energy: f64 = taps.iter().map(|tap| tap * tap).sum::<f64>().sqrt();
    taps.into_iter().map(|tap| (tap / energy) as f32).collect()
}

pub struct SampleFir {
    taps: Vec<f32>,
    history: Vec<Complex<f32>>,
    head: usize,
}

impl SampleFir {
    pub fn new(taps: Vec<f32>) -> Self {
        let length = taps.len();
        Self {
            taps,
            history: vec![Complex::new(0.0, 0.0); 2 * length],
            head: 0,
        }
    }

    pub fn push(&mut self, sample: Complex<f32>) -> Complex<f32> {
        let length = self.taps.len();
        self.head = if self.head == 0 {
            length - 1
        } else {
            self.head - 1
        };
        self.history[self.head] = sample;
        self.history[self.head + length] = sample;
        let mut acc = Complex::new(0.0, 0.0);
        for (value, &tap) in self.history[self.head..self.head + length]
            .iter()
            .zip(&self.taps)
        {
            acc += *value * tap;
        }
        acc
    }

    #[cfg(test)]
    pub fn filter(&mut self, input: &[Complex<f32>]) -> Vec<Complex<f32>> {
        input.iter().map(|&sample| self.push(sample)).collect()
    }
}

pub struct BpskDemod {
    samples_per_symbol: f64,
    lowpass: SampleFir,
    matched: SampleFir,
    fft: Arc<dyn Fft<f32>>,
    coarse: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    pub locked: bool,
    nco_phase: f32,
    nco_freq: f32,
    timing: f64,
    history: [Complex<f32>; HISTORY],
    hist_pos: usize,
    sample_index: u64,
    prev_symbol: f32,
    agc: f32,
    carrier_error: f32,
}

impl BpskDemod {
    pub fn new(channel_rate: f64) -> Self {
        let samples_per_symbol = channel_rate / SYMBOL_RATE;
        let matched_taps = (8.0 * samples_per_symbol).round() as usize | 1;
        let fft = FftPlanner::new().plan_fft_forward(COARSE_FFT);
        let scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
        Self {
            samples_per_symbol,
            lowpass: SampleFir::new(lowpass_taps(LOWPASS_CUTOFF_HZ / channel_rate, LOWPASS_TAPS)),
            matched: SampleFir::new(rrc_taps(RRC_BETA, samples_per_symbol, matched_taps)),
            fft,
            coarse: Vec::with_capacity(COARSE_FFT),
            scratch,
            locked: false,
            nco_phase: 0.0,
            nco_freq: 0.0,
            timing: 0.0,
            history: [Complex::new(0.0, 0.0); HISTORY],
            hist_pos: 0,
            sample_index: 0,
            prev_symbol: 0.0,
            agc: 1e-3,
            carrier_error: 1.0,
        }
    }

    fn past(&self, delay: f64) -> Complex<f32> {
        let whole = delay.floor() as usize;
        let frac = (delay - whole as f64) as f32;
        let newer = self.history[(self.hist_pos + HISTORY - 1 - whole) % HISTORY];
        let older = self.history[(self.hist_pos + HISTORY - 2 - whole) % HISTORY];
        newer * (1.0 - frac) + older * frac
    }

    fn coarse_estimate(&mut self) -> Option<f32> {
        self.fft
            .process_with_scratch(&mut self.coarse, &mut self.scratch);
        let best = self
            .coarse
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm_sqr().total_cmp(&b.1.norm_sqr()))
            .map(|(index, _)| index);
        self.coarse.clear();
        let best = best?;
        let bin = if best <= COARSE_FFT / 2 {
            best as f32
        } else {
            best as f32 - COARSE_FFT as f32
        };
        Some(TAU * bin / COARSE_FFT as f32 / 2.0)
    }

    fn acquire(&mut self, sample: Complex<f32>) {
        self.coarse.push(sample * sample);
        if self.coarse.len() == COARSE_FFT
            && let Some(estimate) = self.coarse_estimate()
        {
            let bin = TAU / COARSE_FFT as f32 / 2.0;
            if (-estimate - self.nco_freq).abs() > RESNAP_BINS * bin {
                self.nco_freq = -estimate;
            }
        }
    }

    fn filtered(&mut self, sample: Complex<f32>) -> Complex<f32> {
        let mixed = sample * Complex::from_polar(1.0, self.nco_phase);
        self.nco_phase += self.nco_freq;
        if self.nco_phase.abs() > TAU {
            self.nco_phase %= TAU;
        }
        let lowpassed = self.lowpass.push(mixed);
        self.matched.push(lowpassed)
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<f32>) {
        for &sample in input {
            if !self.locked {
                self.acquire(sample);
            }
            let filtered = self.filtered(sample);
            self.history[self.hist_pos] = filtered;
            self.hist_pos = (self.hist_pos + 1) % HISTORY;
            self.sample_index += 1;
            if self.sample_index < HISTORY as u64 {
                continue;
            }
            self.timing += 1.0;
            if self.timing < self.samples_per_symbol {
                continue;
            }
            self.timing -= self.samples_per_symbol;
            out.push(self.symbol());
        }
    }

    fn symbol(&mut self) -> f32 {
        let now = self.past(self.timing);
        let mid = self.past(self.timing + self.samples_per_symbol / 2.0);
        self.agc += AGC_ALPHA * (now.re.abs() - self.agc);
        let gain = self.agc.max(1e-9);
        let symbol = now.re / gain;
        let phase_error = now.im / gain * symbol.signum();
        self.nco_phase -= PHASE_GAIN * phase_error;
        self.nco_freq -= FREQ_GAIN * phase_error / self.samples_per_symbol as f32;
        self.carrier_error += CARRIER_ALPHA * (phase_error.abs() - self.carrier_error);
        if self.carrier_error < CARRIER_LOCKED {
            let timing_error = f64::from((now.re - self.prev_symbol * self.agc) * mid.re)
                / f64::from((self.agc * self.agc).max(1e-9));
            self.timing +=
                (TIMING_GAIN * timing_error).clamp(-TIMING_STEP_LIMIT, TIMING_STEP_LIMIT);
        }
        let limited = symbol.clamp(-SOFT_LIMIT, SOFT_LIMIT);
        self.prev_symbol = limited;
        limited
    }
}
