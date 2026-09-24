use num_complex::Complex;

use super::CHANNEL_RATE;
use super::burst::{BurstDetector, Window};
use super::decode::{Reassembly, is_valid};
use super::demod::{DemodBurst, IridiumDemod, SYMBOL_RATE};
use super::ira::IridiumFrame;

const MF_TAPS: usize = 51;
const MF_ALPHA: f64 = 0.4;
const QUIET_GUARD: usize = 300;
const QUIET_TAIL: usize = 1_000;

pub fn rrc_taps(sps: f64, num_taps: usize, beta: f64) -> Vec<f32> {
    use std::f64::consts::{PI, SQRT_2};
    let mid = (num_taps - 1) as f64 / 2.0;
    let mut taps: Vec<f64> = (0..num_taps)
        .map(|n| {
            let t = (n as f64 - mid) / sps;
            if t.abs() < 1e-9 {
                1.0 - beta + 4.0 * beta / PI
            } else if (t.abs() - 1.0 / (4.0 * beta)).abs() < 1e-9 {
                (beta / SQRT_2)
                    * ((1.0 + 2.0 / PI) * (PI / (4.0 * beta)).sin()
                        + (1.0 - 2.0 / PI) * (PI / (4.0 * beta)).cos())
            } else {
                let pt = PI * t;
                ((pt * (1.0 - beta)).sin() + 4.0 * beta * t * (pt * (1.0 + beta)).cos())
                    / (pt * (1.0 - (4.0 * beta * t).powi(2)))
            }
        })
        .collect();
    let energy: f64 = taps.iter().map(|h| h * h).sum::<f64>().sqrt();
    taps.iter_mut().for_each(|h| *h /= energy);
    taps.into_iter().map(|h| h as f32).collect()
}

pub fn matched_filter(x: &[Complex<f32>], taps: &[f32]) -> Vec<Complex<f32>> {
    let half = taps.len() / 2;
    (0..x.len())
        .map(|i| {
            let first = half.saturating_sub(i);
            let last = taps.len().min(x.len() + half - i);
            taps[first..last]
                .iter()
                .enumerate()
                .map(|(k, &tap)| x[i + first + k - half] * tap)
                .sum()
        })
        .collect()
}

pub struct ChannelDecoder {
    detector: BurstDetector,
    demod: IridiumDemod,
    buf: Vec<Complex<f32>>,
    start_abs: u64,
    windows: Vec<Window>,
    taps: Vec<f32>,
    reassembly: Reassembly,
    bursts: Vec<DemodBurst>,
}

impl ChannelDecoder {
    pub fn new() -> Self {
        Self {
            detector: BurstDetector::new(),
            demod: IridiumDemod::new(CHANNEL_RATE),
            buf: Vec::new(),
            start_abs: 0,
            windows: Vec::new(),
            taps: rrc_taps(CHANNEL_RATE / SYMBOL_RATE, MF_TAPS, MF_ALPHA),
            reassembly: Reassembly::new(),
            bursts: Vec::new(),
        }
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<IridiumFrame>) {
        let time =
            (self.start_abs + self.buf.len() as u64 + input.len() as u64) as f64 / CHANNEL_RATE;
        let mut bursts = std::mem::take(&mut self.bursts);
        bursts.clear();
        self.demodulate(input, &mut bursts);
        for burst in &bursts {
            let first = out.len();
            self.reassembly
                .handle(&burst.bits, &burst.reliability, time, 0.0, out);
            for frame in &mut out[first..] {
                frame.offset_hz = Some(burst.cfo_hz as f32);
            }
        }
        self.bursts = bursts;
    }

    pub fn demodulate(&mut self, input: &[Complex<f32>], out: &mut Vec<DemodBurst>) {
        self.buf.extend_from_slice(input);
        self.windows.clear();
        self.detector.push(input, &mut self.windows);
        let now = self.start_abs + self.buf.len() as u64;
        for index in 0..self.windows.len() {
            let window = &self.windows[index];
            let first = window.start.max(self.start_abs);
            let from = (first - self.start_abs) as usize;
            let to = (window.end.min(now) - self.start_abs) as usize;
            let onset = window.onset.saturating_sub(first) as f64;
            let samples = self.buf[from..to].to_vec();
            out.extend(self.window_bursts(&samples, onset));
        }
        let keep_from = self.detector.earliest_needed().max(self.start_abs);
        let drop = ((keep_from - self.start_abs) as usize).min(self.buf.len());
        self.buf.drain(..drop);
        self.start_abs += drop as u64;
    }

    fn window_bursts(&mut self, window: &[Complex<f32>], onset: f64) -> Vec<DemodBurst> {
        let filtered = matched_filter(window, &self.taps);
        let bursts = self.demod_window(filtered, onset);
        if bursts.iter().any(|b| is_valid(&b.bits)) {
            return bursts;
        }
        self.demod_window(window.to_vec(), onset)
    }

    fn demod_window(&mut self, mut chan: Vec<Complex<f32>>, onset: f64) -> Vec<DemodBurst> {
        let quiet = (onset as usize).saturating_sub(QUIET_GUARD).min(chan.len());
        let noise = if quiet > 0 {
            chan[..quiet].iter().map(|s| s.norm_sqr()).sum::<f32>() / quiet as f32
        } else {
            1.0
        };
        chan.resize(chan.len() + QUIET_TAIL, Complex::default());
        self.demod.acquire(&chan, onset, noise)
    }
}
