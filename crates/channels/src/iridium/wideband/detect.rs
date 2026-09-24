use num_complex::Complex;

pub const DEPTH: usize = 4;
pub const GUARD: usize = 21;
const SMOOTH: usize = 12;
const WARMUP: u64 = 64;
const DETECT_RATIO: f32 = 2.0;
const HOLD_RATIO: f32 = 1.5;
const FLOOR_RATE: f32 = 1.0 / 128.0;
const FLOOR_CREEP: f32 = 1.001;

pub struct Detector {
    size: usize,
    low: usize,
    high: usize,
    power: Vec<f64>,
    history: Vec<f32>,
    level: Vec<f32>,
    floor: Vec<f32>,
    masked: Vec<bool>,
    candidates: Vec<(usize, f32)>,
    frames: u64,
}

impl Detector {
    pub fn new(size: usize, usable_bins: usize) -> Self {
        let half = size / 2;
        let reach = usable_bins.min(half - GUARD - 1);
        Self {
            size,
            low: half - reach,
            high: half + reach,
            power: vec![0.0; size + 1],
            history: vec![0.0; DEPTH * size],
            level: vec![0.0; size],
            floor: vec![0.0; size],
            masked: vec![false; size],
            candidates: Vec::new(),
            frames: 0,
        }
    }

    pub fn ready(&self) -> bool {
        self.frames > DEPTH as u64 + WARMUP
    }

    pub fn measure(&mut self, spectrum: &[Complex<f32>]) {
        self.cumulative_power(spectrum);
        let row = (self.frames % DEPTH as u64) as usize * self.size;
        let smoothed = &mut self.history[row..row + self.size];
        for (s, value) in smoothed.iter_mut().enumerate() {
            let from = s.saturating_sub(SMOOTH);
            let to = (s + SMOOTH + 1).min(self.size);
            *value = (self.power[to] - self.power[from]) as f32;
        }
        let (first, rest) = self.history.split_at(self.size);
        self.level.copy_from_slice(first);
        for row in rest.chunks_exact(self.size) {
            for (level, value) in self.level.iter_mut().zip(row) {
                *level += value;
            }
        }
        self.frames += 1;
        self.warm_up();
    }

    fn cumulative_power(&mut self, spectrum: &[Complex<f32>]) {
        let n = self.size;
        let half = n / 2;
        let mut total = 0.0f64;
        self.power[0] = 0.0;
        for s in 0..n {
            let m = if s < half { s + half } else { s - half };
            let before = if m == 0 { n - 1 } else { m - 1 };
            let after = if m + 1 == n { 0 } else { m + 1 };
            let windowed = spectrum[m] - (spectrum[before] + spectrum[after]) * 0.5;
            total += f64::from(windowed.norm_sqr());
            self.power[s + 1] = total;
        }
    }

    fn warm_up(&mut self) {
        let settled = self.frames.saturating_sub(DEPTH as u64);
        if settled == 0 || settled > WARMUP {
            return;
        }
        let weight = 1.0 / WARMUP as f32;
        for (floor, level) in self.floor.iter_mut().zip(&self.level) {
            *floor += level * weight;
        }
    }

    pub fn begin(&mut self) {
        self.masked.fill(false);
    }

    pub fn holds(&self, bin: usize) -> bool {
        (bin.saturating_sub(1)..=(bin + 1).min(self.size - 1))
            .any(|s| self.level[s] > self.floor[s] * HOLD_RATIO)
    }

    pub fn mask(&mut self, bin: usize) {
        let from = bin.saturating_sub(GUARD);
        let to = (bin + GUARD).min(self.size - 1);
        self.masked[from..=to].fill(true);
    }

    fn ratio(&self, s: usize) -> f32 {
        if self.floor[s] > 0.0 {
            self.level[s] / self.floor[s]
        } else {
            0.0
        }
    }

    fn is_peak(&self, s: usize) -> bool {
        let peak = self.ratio(s);
        let from = s.saturating_sub(GUARD);
        let to = (s + GUARD).min(self.size - 1);
        (from..s).all(|t| self.ratio(t) < peak) && (s + 1..=to).all(|t| self.ratio(t) <= peak)
    }

    pub fn claim(&mut self, found: &mut Vec<usize>) {
        self.candidates.clear();
        for s in self.low..self.high {
            if self.level[s] > self.floor[s] * DETECT_RATIO && !self.masked[s] && self.is_peak(s) {
                self.candidates.push((s, self.ratio(s)));
            }
        }
        self.candidates.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        for index in 0..self.candidates.len() {
            let bin = self.candidates[index].0;
            if !self.masked[bin] {
                found.push(bin);
                self.mask(bin);
            }
        }
    }

    pub fn settle(&mut self) {
        for ((floor, &level), &masked) in self.floor.iter_mut().zip(&self.level).zip(&self.masked) {
            if masked || level > *floor * DETECT_RATIO {
                *floor = (*floor * FLOOR_CREEP).min(level).max(*floor);
            } else {
                *floor += FLOOR_RATE * (level - *floor);
            }
        }
    }
}
