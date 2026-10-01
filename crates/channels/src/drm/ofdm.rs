use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::fft::Transform;

use super::{estimate::Estimator, mode::Robustness};

pub const ACQUIRE_SAMPLES: usize = 19_200;
const MIN_COHERENCE: f32 = 0.3;
const PROMINENCE: f32 = 2.5;
const SYNC_METRIC: f32 = 0.6;
const KEEP_METRIC: f32 = 0.3;
const LOST_FRAMES: u32 = 3;
const SEARCH_HZ: f64 = 2_000.0;
const FREQUENCY_GAIN: f64 = 0.05;
const TIMING_WINDOW: usize = 8;
const TIMING_SMOOTHING: f32 = 0.9;
const MAX_SPACING: i32 = 2;
const PEAK_RATIO: f32 = 1.4;
const EMPTY_PENALTY: f32 = 0.25;
const OCCUPIED: f32 = 0.1;

pub struct Probe {
    pub mode: Robustness,
    history: Vec<Complex<f32>>,
    products: Vec<Complex<f32>>,
    powers: Vec<f32>,
    at: usize,
    product_at: usize,
    filled: usize,
    window: Complex<f32>,
    power_now: f32,
    power_then: f32,
    bins: Vec<Complex<f32>>,
    energy_now: Vec<f32>,
    energy_then: Vec<f32>,
    count: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Acquired {
    pub mode: Robustness,
    pub coherence: f32,
    pub start: u64,
    pub cycles_per_sample: f64,
}

impl Probe {
    #[must_use]
    pub fn new(mode: Robustness) -> Self {
        Self {
            mode,
            history: vec![Complex::default(); mode.useful()],
            products: vec![Complex::default(); mode.guard()],
            powers: vec![0.0; 2 * mode.guard()],
            at: 0,
            product_at: 0,
            filled: 0,
            window: Complex::default(),
            power_now: 0.0,
            power_then: 0.0,
            bins: vec![Complex::default(); mode.symbol()],
            energy_now: vec![0.0; mode.symbol()],
            energy_then: vec![0.0; mode.symbol()],
            count: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.mode);
    }

    pub fn push(&mut self, sample: Complex<f32>, time: u64) {
        let delayed = self.history[self.at];
        self.history[self.at] = sample;
        self.at = (self.at + 1) % self.history.len();
        if self.filled < self.history.len() {
            self.filled += 1;
            return;
        }
        let product = sample * delayed.conj();
        let guard = self.products.len();
        self.window += product - self.products[self.product_at];
        self.power_now += sample.norm_sqr() - self.powers[2 * self.product_at];
        self.power_then += delayed.norm_sqr() - self.powers[2 * self.product_at + 1];
        self.products[self.product_at] = product;
        self.powers[2 * self.product_at] = sample.norm_sqr();
        self.powers[2 * self.product_at + 1] = delayed.norm_sqr();
        self.product_at = (self.product_at + 1) % guard;
        self.count += 1;
        if self.count < guard {
            return;
        }
        let bin = (time % self.bins.len() as u64) as usize;
        self.bins[bin] += self.window;
        self.energy_now[bin] += self.power_now;
        self.energy_then[bin] += self.power_then;
    }

    #[must_use]
    pub fn result(&self, now: u64) -> Option<Acquired> {
        let coherence = |bin: usize| {
            let energy = (self.energy_now[bin].max(0.0) * self.energy_then[bin].max(0.0)).sqrt();
            self.bins[bin].norm() / energy.max(f32::EPSILON)
        };
        let best = (0..self.bins.len()).max_by(|&a, &b| coherence(a).total_cmp(&coherence(b)))?;
        let peak = coherence(best);
        let floor = (0..self.bins.len()).map(coherence).fold(f32::MAX, f32::min);
        if peak < MIN_COHERENCE || peak < PROMINENCE * floor {
            return None;
        }
        let symbol = self.mode.symbol() as u64;
        let phase = (best as u64 + 1) % symbol;
        let start = now - now % symbol + phase;
        let start = if start <= now { start + symbol } else { start };
        Some(Acquired {
            mode: self.mode,
            coherence: peak,
            start,
            cycles_per_sample: f64::from(self.bins[best].arg()) / (TAU * self.mode.useful() as f64),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sync {
    Searching,
    Confirming { symbols: usize },
    Locked,
}

pub struct Demodulator {
    pub mode: Robustness,
    fft: Transform,
    buffer: Vec<Complex<f32>>,
    buffer_start: u64,
    next_symbol: u64,
    cycles_per_sample: f64,
    theta: f64,
    window: Vec<Complex<f32>>,
    bins: Vec<Complex<f32>>,
    pairs: Vec<(usize, usize, Complex<f32>)>,
    mean_power: f32,
    sync: Sync,
    symbol: usize,
    misses: u32,
    scores: Vec<f32>,
    searched: usize,
    timing: Vec<f32>,
    estimator: Estimator,
    pub lost: bool,
    pub frame_start: bool,
}

impl Demodulator {
    #[must_use]
    pub fn new(acquired: Acquired, occupancy: u8) -> Self {
        let mode = acquired.mode;
        let (low, high) = mode.span();
        let width = (high - low + 1) as usize;
        let refs = mode.time_refs();
        let pairs = refs
            .windows(2)
            .filter(|pair| pair[1].0 - pair[0].0 <= MAX_SPACING)
            .map(|pair| {
                let a = Complex::from_polar(1.0, TAU as f32 * f32::from(pair[0].1) / 1024.0);
                let b = Complex::from_polar(1.0, TAU as f32 * f32::from(pair[1].1) / 1024.0);
                (
                    (pair[0].0 - low) as usize,
                    (pair[1].0 - low) as usize,
                    (a * b.conj()).conj(),
                )
            })
            .collect();
        Self {
            mode,
            fft: Transform::forward(mode.useful()),
            buffer: Vec::with_capacity(4 * mode.symbol()),
            buffer_start: acquired.start,
            next_symbol: acquired.start,
            cycles_per_sample: acquired.cycles_per_sample,
            theta: 0.0,
            window: vec![Complex::default(); mode.useful()],
            bins: vec![Complex::default(); width],
            pairs,
            mean_power: 0.0,
            sync: Sync::Searching,
            symbol: 0,
            misses: 0,
            scores: vec![
                0.0;
                mode.symbols() * (2 * (SEARCH_HZ / mode.spacing_hz()).ceil() as usize + 1)
            ],
            searched: 0,
            timing: vec![0.0; 2 * TIMING_WINDOW + 1],
            estimator: Estimator::new(mode, occupancy),
            lost: false,
            frame_start: false,
        }
    }

    pub fn set_occupancy(&mut self, occupancy: u8) {
        self.estimator = Estimator::new(self.mode, occupancy);
    }

    #[must_use]
    pub fn frequency_offset_hz(&self) -> f64 {
        self.cycles_per_sample * self.mode.rate_hz()
    }

    #[must_use]
    pub fn locked(&self) -> bool {
        self.sync == Sync::Locked
    }

    #[must_use]
    pub fn estimator(&self) -> &Estimator {
        &self.estimator
    }

    pub fn push(&mut self, sample: Complex<f32>, time: u64) {
        if time >= self.buffer_start {
            self.buffer.push(sample);
        }
    }

    fn symbol_ready(&self) -> bool {
        let end = self.next_symbol + (self.mode.symbol() + TIMING_WINDOW) as u64;
        self.buffer_start + self.buffer.len() as u64 >= end
    }

    pub fn next(&mut self) -> Option<bool> {
        if !self.symbol_ready() {
            return None;
        }
        let offset = (self.next_symbol - self.buffer_start) as usize;
        let (adjust, frequency) = self.track(offset);
        self.transform(offset);
        let step = self.mode.symbol() as i64 + adjust;
        self.theta = (self.theta + self.cycles_per_sample * step as f64).rem_euclid(1.0);
        self.cycles_per_sample = frequency;
        self.next_symbol = (self.next_symbol as i64 + step) as u64;
        let drop = (self.next_symbol - self.buffer_start).saturating_sub(TIMING_WINDOW as u64 + 1);
        self.buffer.drain(..drop as usize);
        self.buffer_start += drop;
        Some(self.advance())
    }

    fn cp_correlation(&self, start: usize) -> Complex<f32> {
        let useful = self.mode.useful();
        (0..self.mode.guard())
            .map(|m| self.buffer[start + useful + m] * self.buffer[start + m].conj())
            .sum()
    }

    fn track(&mut self, offset: usize) -> (i64, f64) {
        if offset < TIMING_WINDOW {
            return (0, self.cycles_per_sample);
        }
        let expected = Complex::from_polar(
            1.0,
            (-TAU * self.cycles_per_sample * self.mode.useful() as f64) as f32,
        );
        let centre = self.cp_correlation(offset) * expected;
        let error = f64::from(centre.arg()) / (TAU * self.mode.useful() as f64);
        let frequency = self.cycles_per_sample + FREQUENCY_GAIN * error;
        for index in 0..self.timing.len() {
            let measured = self.cp_correlation(offset + index - TIMING_WINDOW).norm();
            let value = &mut self.timing[index];
            *value = TIMING_SMOOTHING * *value + (1.0 - TIMING_SMOOTHING) * measured;
        }
        let peak = (0..self.timing.len())
            .max_by(|&a, &b| self.timing[a].total_cmp(&self.timing[b]))
            .unwrap_or(TIMING_WINDOW);
        let adjust = if peak >= TIMING_WINDOW + 3 {
            self.timing.rotate_left(1);
            1
        } else if peak + 3 <= TIMING_WINDOW {
            self.timing.rotate_right(1);
            -1
        } else {
            0
        };
        (adjust, frequency)
    }

    fn transform(&mut self, offset: usize) {
        let useful = self.mode.useful();
        let guard = self.mode.guard();
        let shift = guard / 2;
        let start = offset + shift;
        let mut phasor = Complex::from_polar(
            1.0,
            (-TAU * (self.theta + self.cycles_per_sample * shift as f64)) as f32,
        );
        let step = Complex::from_polar(1.0, (-TAU * self.cycles_per_sample) as f32);
        for (slot, &sample) in self
            .window
            .iter_mut()
            .zip(&self.buffer[start..start + useful])
        {
            *slot = sample * phasor;
            phasor *= step;
        }
        self.fft.process(&mut self.window);
        let (low, _) = self.mode.span();
        let scale = (useful as f32).sqrt().recip();
        for (index, bin) in self.bins.iter_mut().enumerate() {
            let k = low + index as i32;
            let fft_index = k.rem_euclid(useful as i32) as usize;
            let correction = Complex::from_polar(
                scale,
                (TAU * k as f64 * (guard - shift) as f64 / useful as f64) as f32,
            );
            *bin = self.window[fft_index] * correction;
        }
        let mean = self.bins.iter().map(|bin| bin.norm_sqr()).sum::<f32>() / self.bins.len() as f32;
        let (power, count) = self
            .bins
            .iter()
            .map(|bin| bin.norm_sqr())
            .filter(|&power| power > OCCUPIED * mean)
            .fold((0.0f32, 0usize), |(sum, count), power| {
                (sum + power, count + 1)
            });
        self.mean_power = power / count.max(1) as f32;
    }

    fn reference_metric(&self, shift: i32) -> f32 {
        let mut sum = Complex::<f32>::default();
        let mut norm = 0.0f32;
        let width = self.bins.len() as i32;
        for &(a, b, reference) in &self.pairs {
            let (ia, ib) = (a as i32 + shift, b as i32 + shift);
            if ia < 0 || ib < 0 || ia >= width || ib >= width {
                return 0.0;
            }
            let product = self.bins[ia as usize] * self.bins[ib as usize].conj() * reference;
            sum += product;
            norm += product.norm();
        }
        let floor = EMPTY_PENALTY * self.pairs.len() as f32 * self.mean_power;
        sum.norm() / (norm + floor).max(f32::EPSILON)
    }

    fn reach(&self) -> i32 {
        (SEARCH_HZ / self.mode.spacing_hz()).ceil() as i32
    }

    fn search(&mut self) -> Option<i32> {
        let reach = self.reach();
        let shifts = (2 * reach + 1) as usize;
        let symbols = self.mode.symbols();
        let row = self.searched % symbols;
        let settled = self.searched >= symbols;
        self.searched += 1;
        let mut best = (0, 0.0f32);
        for (index, shift) in (-reach..=reach).enumerate() {
            let metric = self.reference_metric(shift);
            let score = &mut self.scores[row * shifts + index];
            *score = if settled {
                0.5 * (*score + metric)
            } else {
                metric
            };
            if *score > best.1 {
                best = (shift, *score);
            }
        }
        if !settled || best.1 < SYNC_METRIC {
            return None;
        }
        let candidate = row * shifts + (best.0 + reach) as usize;
        let runner_up = self
            .scores
            .iter()
            .enumerate()
            .filter(|&(index, _)| index != candidate)
            .map(|(_, &score)| score)
            .fold(0.0f32, f32::max);
        (best.1 > PEAK_RATIO * runner_up).then_some(best.0)
    }

    fn shift_bins(&mut self, shift: i32) {
        if shift > 0 {
            self.bins.rotate_left(shift as usize);
        } else if shift < 0 {
            self.bins.rotate_right(shift.unsigned_abs() as usize);
        }
    }

    fn advance(&mut self) -> bool {
        self.frame_start = false;
        match self.sync {
            Sync::Searching => {
                let Some(shift) = self.search() else {
                    return false;
                };
                self.cycles_per_sample += f64::from(shift) / self.mode.useful() as f64;
                self.shift_bins(shift);
                self.sync = Sync::Confirming { symbols: 0 };
                self.symbol = 0;
                self.estimator.reset();
                self.frame_start = true;
                self.estimator.push(&self.bins, 0)
            }
            Sync::Confirming { symbols } => {
                self.symbol = (self.symbol + 1) % self.mode.symbols();
                if self.symbol == 0 {
                    if self.reference_metric(0) > SYNC_METRIC {
                        self.sync = Sync::Locked;
                        self.misses = 0;
                        self.frame_start = true;
                    } else {
                        self.sync = Sync::Searching;
                        self.searched = 0;
                        return false;
                    }
                } else {
                    self.sync = Sync::Confirming {
                        symbols: symbols + 1,
                    };
                }
                self.estimator.push(&self.bins, self.symbol)
            }
            Sync::Locked => {
                self.symbol = (self.symbol + 1) % self.mode.symbols();
                if self.symbol == 0 {
                    self.frame_start = true;
                    if self.reference_metric(0) > KEEP_METRIC {
                        self.misses = 0;
                    } else {
                        self.misses += 1;
                        if self.misses >= LOST_FRAMES {
                            self.lost = true;
                        }
                    }
                }
                self.estimator.push(&self.bins, self.symbol)
            }
        }
    }
}
