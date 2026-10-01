use std::f32::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::design_rrc;
use sdrmm_wire::LrptMode;

pub const ROLL_OFF: f64 = 0.6;
const FILTER_SPAN: usize = 6;
const SOFT_GAIN: f32 = 64.0;
const SOFT_LIMIT: f32 = 127.0;
const POWER_SMOOTHING: f32 = 1.0 / 512.0;
const LOCK_SMOOTHING: f32 = 1.0 / 256.0;
const LOCK_ENTER: f32 = 0.4;
const LOCK_LEAVE: f32 = 0.15;
const FLL_GAIN: f32 = 1e-4;
const MAX_STEP: f32 = 0.35;
const CARRIER_BANDWIDTH: f32 = 0.006;
const TIMING_PROPORTIONAL: f32 = 0.015;
const TIMING_INTEGRAL: f32 = 0.000_08;
const MAX_TRIM: f32 = 0.05;
const DAMPING: f32 = std::f32::consts::FRAC_1_SQRT_2;
const COSTAS_GAIN: f32 = std::f32::consts::SQRT_2;

struct MatchedFilter {
    taps: Vec<f32>,
    history: Vec<Complex<f32>>,
    at: usize,
}

impl MatchedFilter {
    fn new(sps: f64) -> Self {
        let taps = design_rrc(sps, ROLL_OFF, FILTER_SPAN);
        let len = taps.len();
        Self {
            taps,
            history: vec![Complex::default(); 2 * len],
            at: 0,
        }
    }

    fn reset(&mut self) {
        self.history.fill(Complex::default());
        self.at = 0;
    }

    fn push(&mut self, sample: Complex<f32>) -> Complex<f32> {
        let len = self.taps.len();
        self.history[self.at] = sample;
        self.history[self.at + len] = sample;
        self.at = (self.at + 1) % len;
        let window = &self.history[self.at..self.at + len];
        window
            .iter()
            .zip(&self.taps)
            .fold(Complex::default(), |acc, (&x, &h)| acc + x * h)
    }
}

#[derive(Clone, Copy)]
struct LoopGains {
    proportional: f32,
    integral: f32,
}

impl LoopGains {
    fn second_order(bandwidth: f32, detector_gain: f32) -> Self {
        let theta = bandwidth / (DAMPING + 0.25 / DAMPING);
        let denominator = 1.0 + 2.0 * DAMPING * theta + theta * theta;
        Self {
            proportional: 4.0 * DAMPING * theta / denominator / detector_gain,
            integral: 4.0 * theta * theta / denominator / detector_gain,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Strobes {
    on_previous: Complex<f32>,
    on_current: Complex<f32>,
    mid_previous: Complex<f32>,
    mid: bool,
}

pub struct Demod {
    offset: bool,
    sps: f32,
    filter: MatchedFilter,
    phase: f32,
    step: f32,
    fll_previous: Complex<f32>,
    sample_power: f32,
    filtered: [Complex<f32>; 4],
    clock: f32,
    trim: f32,
    strobes: Strobes,
    symbol_power: f32,
    lock: f32,
    locked: bool,
    carrier: LoopGains,
}

impl Demod {
    #[must_use]
    pub fn new(mode: LrptMode, input_rate: f64) -> Self {
        let sps = input_rate / mode.symbol_rate();
        Self {
            offset: mode.offset(),
            sps: sps as f32,
            filter: MatchedFilter::new(sps),
            phase: 0.0,
            step: 0.0,
            fll_previous: Complex::default(),
            sample_power: 1e-9,
            filtered: [Complex::default(); 4],
            clock: 0.0,
            trim: 0.0,
            strobes: Strobes::default(),
            symbol_power: 1e-9,
            lock: 0.0,
            locked: false,
            carrier: LoopGains::second_order(CARRIER_BANDWIDTH, COSTAS_GAIN),
        }
    }

    pub fn reset(&mut self) {
        self.filter.reset();
        self.phase = 0.0;
        self.step = 0.0;
        self.fll_previous = Complex::default();
        self.sample_power = 1e-9;
        self.filtered = [Complex::default(); 4];
        self.clock = 0.0;
        self.trim = 0.0;
        self.strobes = Strobes::default();
        self.symbol_power = 1e-9;
        self.lock = 0.0;
        self.locked = false;
    }

    pub fn process(&mut self, iq: &[Complex<f32>], soft: &mut Vec<i16>) {
        for &sample in iq {
            let (sin, cos) = self.phase.sin_cos();
            let rotated = sample * Complex::new(cos, -sin);
            self.phase = (self.phase + self.step).rem_euclid(TAU);
            let filtered = self.filter.push(rotated);
            self.track_frequency(filtered);
            self.filtered.copy_within(1.., 0);
            self.filtered[3] = filtered;
            self.clock += 1.0;
            let period = self.sps / 2.0 * (1.0 + self.trim);
            if self.clock >= period {
                self.clock -= period;
                let strobe = self.interpolate(1.0 - self.clock.clamp(0.0, 1.0));
                self.on_strobe(strobe, soft);
            }
        }
    }

    fn track_frequency(&mut self, filtered: Complex<f32>) {
        let power = filtered.norm_sqr();
        self.sample_power += (power - self.sample_power) * POWER_SMOOTHING;
        let error = (filtered * self.fll_previous.conj()).im / self.sample_power.max(1e-12);
        self.fll_previous = filtered;
        if self.locked {
            return;
        }
        self.step = (self.step + FLL_GAIN * error.clamp(-1.0, 1.0)).clamp(-MAX_STEP, MAX_STEP);
    }

    fn interpolate(&self, fraction: f32) -> Complex<f32> {
        let d = fraction;
        let [p0, p1, p2, p3] = self.filtered;
        let c0 = -d * (d - 1.0) * (d - 2.0) / 6.0;
        let c1 = (d + 1.0) * (d - 1.0) * (d - 2.0) / 2.0;
        let c2 = -(d + 1.0) * d * (d - 2.0) / 2.0;
        let c3 = (d + 1.0) * d * (d - 1.0) / 6.0;
        p0 * c0 + p1 * c1 + p2 * c2 + p3 * c3
    }

    fn on_strobe(&mut self, strobe: Complex<f32>, soft: &mut Vec<i16>) {
        let mut strobes = self.strobes;
        let symbol = if strobes.mid {
            let symbol = self.offset.then(|| {
                let error = strobes.mid_previous.re
                    * (strobes.on_previous.re - strobes.on_current.re)
                    + strobes.on_current.im * (strobes.mid_previous.im - strobe.im);
                (Complex::new(strobes.on_current.re, strobe.im), error)
            });
            strobes.mid_previous = strobe;
            symbol
        } else {
            strobes.on_previous = strobes.on_current;
            strobes.on_current = strobe;
            (!self.offset).then(|| {
                let error = strobes.mid_previous.re * (strobes.on_previous.re - strobe.re)
                    + strobes.mid_previous.im * (strobes.on_previous.im - strobe.im);
                (strobe, error)
            })
        };
        strobes.mid = !strobes.mid;
        self.strobes = strobes;
        if let Some((symbol, error)) = symbol {
            self.symbol(symbol, error, soft);
        }
    }

    fn symbol(&mut self, symbol: Complex<f32>, timing_error: f32, soft: &mut Vec<i16>) {
        self.symbol_power += (symbol.norm_sqr() - self.symbol_power) * POWER_SMOOTHING;
        let scale = self.symbol_power.max(1e-12).sqrt().recip();
        let normalised = symbol * scale;
        self.track_timing(timing_error * scale * scale);
        self.track_carrier(normalised);
        soft.push(to_soft(normalised.re));
        soft.push(to_soft(normalised.im));
    }

    fn track_timing(&mut self, error: f32) {
        let error = error.clamp(-1.0, 1.0);
        self.clock -= TIMING_PROPORTIONAL * error * self.sps;
        self.trim = (self.trim + TIMING_INTEGRAL * error).clamp(-MAX_TRIM, MAX_TRIM);
    }

    fn track_carrier(&mut self, symbol: Complex<f32>) {
        let error = symbol.re.signum() * symbol.im - symbol.im.signum() * symbol.re;
        let error = error.clamp(-1.0, 1.0);
        self.phase = (self.phase + self.carrier.proportional * error).rem_euclid(TAU);
        self.step =
            (self.step + self.carrier.integral * error / self.sps).clamp(-MAX_STEP, MAX_STEP);
        let fourth = symbol * symbol * symbol * symbol;
        let quality = -fourth.re / symbol.norm_sqr().powi(2).max(1e-12);
        self.lock += (quality - self.lock) * LOCK_SMOOTHING;
        self.locked = if self.locked {
            self.lock > LOCK_LEAVE
        } else {
            self.lock > LOCK_ENTER
        };
    }
}

fn to_soft(value: f32) -> i16 {
    (value * SOFT_GAIN).round().clamp(-SOFT_LIMIT, SOFT_LIMIT) as i16
}
