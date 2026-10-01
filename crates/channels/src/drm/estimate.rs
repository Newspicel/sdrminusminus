use std::f32::consts::{PI, SQRT_2};

use num_complex::Complex;

use super::mode::Robustness;

#[derive(Clone, Copy, Debug, Default)]
struct Occurrence {
    time: u64,
    value: Complex<f32>,
    seen: bool,
}

pub struct Estimator {
    mode: Robustness,
    low: i32,
    width: usize,
    pilot_low: i32,
    pilot_high: i32,
    pilots: Vec<Vec<(usize, Complex<f32>)>>,
    grid: Vec<i32>,
    latest: Vec<Occurrence>,
    previous: Vec<Occurrence>,
    ring: Vec<Complex<f32>>,
    ring_symbols: Vec<usize>,
    channel: Vec<Complex<f32>>,
    response: Vec<Complex<f32>>,
    time: u64,
    pub equalized: Vec<Complex<f32>>,
    pub weights: Vec<f32>,
    pub symbol: usize,
}

fn reference(mode: Robustness, occupancy: u8, symbol: usize, k: i32) -> Complex<f32> {
    let amplitude = if mode.boosted(occupancy).contains(&k) {
        2.0
    } else {
        SQRT_2
    };
    let phase = mode
        .frequency_phase(symbol, k)
        .or_else(|| (symbol == 0).then(|| mode.time_phase(k)).flatten())
        .unwrap_or_else(|| mode.gain_phase(symbol, k));
    Complex::from_polar(amplitude, 2.0 * PI * f32::from(phase) / 1024.0)
}

impl Estimator {
    #[must_use]
    pub fn new(mode: Robustness, occupancy: u8) -> Self {
        let (low, high) = mode.span();
        let width = (high - low + 1) as usize;
        let (x, y, k0) = mode.gain_grid();
        let (pilot_low, pilot_high) = mode
            .carriers(occupancy)
            .unwrap_or_else(|| mode.carriers(mode.widest()).unwrap_or((low, high)));
        let grid: Vec<i32> = (pilot_low..=pilot_high)
            .filter(|k| (k - k0).rem_euclid(x) == 0)
            .collect();
        let pilots = (0..mode.symbols())
            .map(|symbol| {
                grid.iter()
                    .enumerate()
                    .filter(|&(_, &k)| mode.is_gain_ref(symbol, k))
                    .map(|(index, &k)| (index, reference(mode, occupancy, symbol, k)))
                    .collect()
            })
            .collect();
        let depth = y as usize;
        Self {
            mode,
            low,
            width,
            pilot_low,
            pilot_high,
            pilots,
            latest: vec![Occurrence::default(); grid.len()],
            previous: vec![Occurrence::default(); grid.len()],
            grid,
            ring: vec![Complex::default(); depth * width],
            ring_symbols: vec![0; depth],
            channel: vec![Complex::default(); width],
            response: Vec::new(),
            time: 0,
            equalized: vec![Complex::default(); width],
            weights: vec![0.0; width],
            symbol: 0,
        }
    }

    pub fn reset(&mut self) {
        self.latest.fill(Occurrence::default());
        self.previous.fill(Occurrence::default());
        self.time = 0;
    }

    fn depth(&self) -> usize {
        self.mode.gain_grid().1 as usize
    }

    pub fn push(&mut self, bins: &[Complex<f32>], symbol: usize) -> bool {
        let depth = self.depth();
        let slot = (self.time % depth as u64) as usize;
        self.ring[slot * self.width..(slot + 1) * self.width].copy_from_slice(bins);
        self.ring_symbols[slot] = symbol;
        for &(index, reference) in &self.pilots[symbol % self.mode.symbols()] {
            let k = self.grid[index];
            let value = bins[(k - self.low) as usize] / reference;
            self.previous[index] = self.latest[index];
            self.latest[index] = Occurrence {
                time: self.time,
                value,
                seen: true,
            };
        }
        self.time += 1;
        if self.time < depth as u64 {
            return false;
        }
        let target = self.time - depth as u64;
        self.interpolate_time(target);
        self.interpolate_frequency();
        let slot = (target % depth as u64) as usize;
        self.symbol = self.ring_symbols[slot];
        for index in 0..self.width {
            let channel = self.channel[index];
            let power = channel.norm_sqr();
            let raw = self.ring[slot * self.width + index];
            self.equalized[index] = if power > f32::EPSILON {
                raw / channel
            } else {
                Complex::default()
            };
            self.weights[index] = power;
        }
        true
    }

    fn interpolate_time(&mut self, target: u64) {
        self.response.clear();
        for (latest, previous) in self.latest.iter().zip(&self.previous) {
            let value = match (latest.seen, previous.seen) {
                (true, true) if latest.time > previous.time && latest.time > target => {
                    let fraction = (target.saturating_sub(previous.time)) as f32
                        / (latest.time - previous.time) as f32;
                    previous.value + (latest.value - previous.value) * fraction
                }
                (true, _) => latest.value,
                _ => Complex::default(),
            };
            self.response.push(value);
        }
    }

    fn interpolate_frequency(&mut self) {
        let (x, _, _) = self.mode.gain_grid();
        let slope: Complex<f32> = self
            .response
            .windows(2)
            .map(|pair| pair[1] * pair[0].conj())
            .sum();
        let step = if slope.norm() > f32::EPSILON {
            slope.arg()
        } else {
            0.0
        };
        let last = self.grid.len().saturating_sub(1);
        for (index, channel) in self.channel.iter_mut().enumerate() {
            let k = self.low + index as i32;
            let position =
                (k - self.grid.first().copied().unwrap_or(self.pilot_low)) as f32 / x as f32;
            let clamped = position.clamp(0.0, last as f32);
            let lower = (clamped.floor() as usize).min(last);
            let upper = (lower + 1).min(last);
            let fraction = clamped - lower as f32;
            let derotate = |grid: usize| {
                self.response.get(grid).copied().unwrap_or_default()
                    * Complex::from_polar(1.0, -step * grid as f32)
            };
            let base = derotate(lower) * (1.0 - fraction) + derotate(upper) * fraction;
            *channel = base * Complex::from_polar(1.0, step * position);
            if k < self.pilot_low - x || k > self.pilot_high + x {
                *channel = Complex::default();
            }
        }
    }
}
