use std::f32::consts::PI;

use num_complex::Complex;
use sdrmm_dsp::ReedSolomon;

use super::avlc::{self, AvlcFrame};
use super::header::{self, HEADER_BITS};
use super::interleave::{self, Erasures};
use super::scramble::Scrambler;

pub const SYMBOL_RATE: f64 = 10_500.0;

pub const UW_DELTAS: [u8; 16] = [0, 3, 2, 4, 0, 1, 6, 4, 1, 7, 2, 5, 6, 5, 7, 3];

#[cfg(test)]
pub const GRAY_FWD: [u8; 8] = [0, 7, 3, 4, 1, 6, 2, 5];
const GRAY_INV: [(u8, u8, u8); 8] = [
    (0, 0, 0),
    (0, 0, 1),
    (0, 1, 1),
    (0, 1, 0),
    (1, 1, 0),
    (1, 1, 1),
    (1, 0, 1),
    (1, 0, 0),
];

const CORR_THRESHOLD: f32 = 0.6;
const FIT_COST_MAX: f32 = 0.25;
const ENERGY_FACTOR: f32 = 12.0;
const NOISE_ALPHA: f32 = 1e-4;
const DIFFERENTIAL_GAIN: f32 = 0.1;
const PROFILES: [Detector; 3] = [
    Detector::Coherent {
        carrier_gain: 0.3,
        frequency_gain: 0.02,
    },
    Detector::Coherent {
        carrier_gain: 0.15,
        frequency_gain: 0.005,
    },
    Detector::Differential,
];

struct Strategy {
    profiles: &'static [Detector],
    ladder: &'static [Erasures],
    fcs_arbitrates: bool,
}

const RECOVERY: Strategy = Strategy {
    profiles: &PROFILES,
    ladder: &[Erasures::Doubtful, Erasures::Least(2), Erasures::Least(4)],
    fcs_arbitrates: true,
};

#[cfg(test)]
const XNG: Strategy = Strategy {
    profiles: &[Detector::Differential],
    ladder: &[Erasures::Doubtful],
    fcs_arbitrates: false,
};
const MAX_TL_BITS: u32 = 16_000;
const MIN_EDGE_ENERGY: f32 = 0.01;
const FIT_STEP: f64 = 0.25;

#[derive(Clone, Copy)]
enum Detector {
    Coherent {
        carrier_gain: f32,
        frequency_gain: f32,
    },
    Differential,
}

struct Tracker {
    detector: Detector,
    reference: f32,
    prev: Complex<f32>,
    theta: f32,
}

impl Tracker {
    fn step(&mut self, s: Complex<f32>) -> (usize, f32) {
        let (ph, predicted) = match self.detector {
            Detector::Coherent { .. } => {
                let predicted = self.reference + self.theta;
                (wrap(s.arg() - predicted), predicted)
            }
            Detector::Differential => {
                let d = s * self.prev.conj();
                self.prev = s;
                (d.arg() - self.theta, 0.0)
            }
        };
        let idx_f = (ph / (PI / 4.0)).round();
        let residual = ph - idx_f * (PI / 4.0);
        match self.detector {
            Detector::Coherent {
                carrier_gain,
                frequency_gain,
            } => {
                self.reference = wrap(predicted + idx_f * (PI / 4.0) + carrier_gain * residual);
                self.theta += frequency_gain * residual;
            }
            Detector::Differential => self.theta += DIFFERENTIAL_GAIN * residual,
        }
        ((idx_f as i32).rem_euclid(8) as usize, residual)
    }
}

struct Collecting {
    lock: Lock,
    profile: usize,
    uw_start: f64,
    next_pos: f64,
    tracker: Tracker,
    bits: Vec<u8>,
    conf: Vec<f32>,
    scr: Scrambler,
    length: Option<BurstLength>,
}

#[derive(Clone, Copy)]
struct BurstLength {
    tl_bits: usize,
    total_bits: usize,
}

enum Collected {
    Pending,
    BadHeader,
    Complete(BurstLength),
}

enum State {
    Hunt,
    Collect(Box<Collecting>),
}

pub struct Vdl2Demod {
    last_rs_fail: f64,
    sps: f64,
    buf: Vec<Complex<f32>>,
    start_abs: f64,
    cursor: f64,
    noise: f32,
    state: State,
    strategy: &'static Strategy,
}

pub struct Burst {
    pub frames: Vec<AvlcFrame>,
    pub rs_corrected: usize,
    pub freq_skew_hz: f32,
    pub snr_db: f32,
}

#[derive(Clone, Copy)]
struct Lock {
    uw_pos: f64,
    theta: f32,
    phase: f32,
}

struct Fit {
    cost: f32,
    slope: f32,
    intercept: f32,
}

impl Vdl2Demod {
    pub fn new(channel_rate: f64) -> Self {
        Self {
            sps: channel_rate / SYMBOL_RATE,
            buf: Vec::new(),
            start_abs: 0.0,
            cursor: 0.0,
            noise: 1e-6,
            state: State::Hunt,
            strategy: &RECOVERY,
            last_rs_fail: f64::NEG_INFINITY,
        }
    }

    #[cfg(test)]
    pub fn differential(channel_rate: f64) -> Self {
        Self {
            strategy: &XNG,
            ..Self::new(channel_rate)
        }
    }

    fn sample(&self, abs_pos: f64) -> Option<Complex<f32>> {
        let rel = abs_pos - self.start_abs;
        if rel < 0.0 {
            return None;
        }
        let i = rel.floor() as usize;
        if i + 1 >= self.buf.len() {
            return None;
        }
        let frac = (rel - i as f64) as f32;
        Some(self.buf[i] * (1.0 - frac) + self.buf[i + 1] * frac)
    }

    fn uw_correlate(&self, pos: f64) -> Option<f32> {
        let mut corr = Complex::new(0.0f32, 0.0);
        let mut norm = 0.0f32;
        let mut min_d = f32::MAX;
        let mut prev = self.sample(pos)?;
        for (j, &delta) in UW_DELTAS.iter().enumerate().skip(1) {
            let s = self.sample(pos + j as f64 * self.sps)?;
            let d = s * prev.conj();
            prev = s;
            let expected = Complex::from_polar(1.0, f32::from(delta) * PI / 4.0);
            corr += d * expected.conj();
            norm += d.norm();
            min_d = min_d.min(d.norm());
        }
        if norm < 1e-9 {
            return None;
        }
        let mean_d = norm / (UW_DELTAS.len() - 1) as f32;
        if min_d < MIN_EDGE_ENERGY * mean_d {
            return Some(0.0);
        }
        Some(corr.norm() / norm)
    }

    fn preamble_fit(&self, pos: f64) -> Option<(f64, Fit)> {
        let ramp = uw_ramp();
        let mut best: Option<(f64, Fit)> = None;
        let half = (0.63 * self.sps).max(3.0);
        let mut t = -half;
        while t <= half {
            let cand = pos + t;
            t += FIT_STEP;
            if cand < self.start_abs {
                continue;
            }
            let mut r = [0.0f32; 16];
            let mut w = [0.0f32; 16];
            for k in 0..16 {
                let s = self.sample(cand + k as f64 * self.sps)?;
                r[k] = s.arg() - ramp[k];
                w[k] = s.norm_sqr();
            }
            let Some(fit) = line_fit(&mut r, &w) else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, b)| fit.cost < b.cost) {
                best = Some((cand, fit));
            }
        }
        best
    }

    fn hunt(&mut self) -> Option<Lock> {
        let span = 19.0 * self.sps;
        let end_abs = self.start_abs + self.buf.len() as f64 - span - 4.0;
        while self.cursor < end_abs {
            let pos = self.cursor;
            self.cursor += 1.0;
            let rel = (pos - self.start_abs) as usize;
            let p = self.buf[rel].norm_sqr();
            if p < self.noise * ENERGY_FACTOR {
                self.noise += NOISE_ALPHA * (p - self.noise);
                continue;
            }
            self.noise *= 1.0 + NOISE_ALPHA * 0.1;
            if !self.uw_correlate(pos).is_some_and(|m| m > CORR_THRESHOLD) {
                continue;
            }
            if let Some((uw_pos, fit)) = self.preamble_fit(pos)
                && fit.cost < FIT_COST_MAX
            {
                let last = (UW_DELTAS.len() - 1) as f32;
                return Some(Lock {
                    uw_pos,
                    theta: fit.slope,
                    phase: fit.intercept + fit.slope * last + uw_ramp()[UW_DELTAS.len() - 1],
                });
            }
        }
        None
    }

    fn collect(&self, c: &mut Collecting) -> Collected {
        loop {
            if c.length.is_none() && c.bits.len() >= HEADER_BITS {
                match burst_length(&c.bits) {
                    Some(length) => c.length = Some(length),
                    None => return Collected::BadHeader,
                }
            }
            if let Some(length) = c.length
                && c.bits.len() >= length.total_bits
            {
                return Collected::Complete(length);
            }
            let Some(s) = self.sample(c.next_pos) else {
                return Collected::Pending;
            };
            c.next_pos += self.sps;
            let (idx, residual) = c.tracker.step(s);
            c.conf.push(residual.abs());
            let (x, y, z) = GRAY_INV[idx];
            for b in [x, y, z] {
                c.bits.push(b ^ c.scr.next_bit());
            }
        }
    }

    fn start_collect(&mut self, lock: Lock) -> bool {
        if (lock.uw_pos - self.last_rs_fail).abs() < 1.5 {
            self.cursor = lock.uw_pos + 17.0 * self.sps;
            return true;
        }
        match self.collecting(lock, 0) {
            Some(c) => {
                self.state = State::Collect(Box::new(c));
                true
            }
            None => false,
        }
    }

    fn collecting(&self, lock: Lock, profile: usize) -> Option<Collecting> {
        let detector = *self.strategy.profiles.get(profile)?;
        let last_uw = lock.uw_pos + 15.0 * self.sps;
        let prev = self.sample(last_uw)?;
        Some(Collecting {
            uw_start: lock.uw_pos,
            next_pos: last_uw + self.sps,
            tracker: Tracker {
                detector,
                reference: wrap(lock.phase),
                prev,
                theta: lock.theta,
            },
            lock,
            profile,
            bits: Vec::new(),
            conf: Vec::new(),
            scr: Scrambler::new(),
            length: None,
        })
    }

    fn retry(&mut self, c: &Collecting) -> bool {
        match self.collecting(c.lock, c.profile + 1) {
            Some(next) => {
                self.state = State::Collect(Box::new(next));
                true
            }
            None => false,
        }
    }

    fn finish(&mut self, c: &Collecting, length: BurstLength, rs: &ReedSolomon) -> Option<Burst> {
        let decoded = self.strategy.ladder.iter().find_map(|&erasures| {
            let decoded = interleave::deinterleave_soft(
                &c.bits[HEADER_BITS..length.total_bits],
                &c.conf,
                HEADER_BITS,
                length.tl_bits,
                rs,
                erasures,
            )?;
            let frames = avlc::scan(&decoded.bits);
            (!self.strategy.fcs_arbitrates || !frames.is_empty()).then_some((decoded, frames))
        });
        let Some((decoded, frames)) = decoded else {
            if self.retry(c) {
                return None;
            }
            self.last_rs_fail = c.uw_start;
            self.cursor = c.uw_start + 1.0;
            return None;
        };
        if decoded.soft_assisted {
            self.last_rs_fail = c.uw_start;
            self.cursor = c.uw_start + 1.0;
        } else {
            self.cursor = c.next_pos;
        }
        Some(Burst {
            frames,
            rs_corrected: decoded.corrected,
            freq_skew_hz: (f64::from(c.lock.theta) * SYMBOL_RATE / std::f64::consts::TAU) as f32,
            snr_db: evm_snr_db(&c.conf),
        })
    }

    pub fn process(&mut self, input: &[Complex<f32>], rs: &ReedSolomon, out: &mut Vec<Burst>) {
        self.buf.extend_from_slice(input);
        loop {
            match std::mem::replace(&mut self.state, State::Hunt) {
                State::Hunt => match self.hunt() {
                    Some(lock) => {
                        if !self.start_collect(lock) {
                            break;
                        }
                    }
                    None => break,
                },
                State::Collect(mut c) => match self.collect(&mut c) {
                    Collected::Pending => {
                        self.state = State::Collect(c);
                        break;
                    }
                    Collected::BadHeader => {
                        self.retry(&c);
                    }
                    Collected::Complete(length) => {
                        out.extend(self.finish(&c, length, rs));
                    }
                },
            }
        }
        self.trim();
    }

    fn trim(&mut self) {
        let active = match &self.state {
            State::Collect(c) => c.uw_start.min(c.next_pos),
            State::Hunt => self.cursor,
        };
        let keep_from = (active - self.start_abs - 4.0 * self.sps).max(0.0) as usize;
        if keep_from > 0 && keep_from <= self.buf.len() {
            self.buf.drain(..keep_from);
            self.start_abs += keep_from as f64;
        }
    }
}

fn burst_length(bits: &[u8]) -> Option<BurstLength> {
    let header: &[u8; HEADER_BITS] = bits.get(..HEADER_BITS)?.try_into().ok()?;
    let tl_bits = header::decode(header).filter(|&tl| tl <= MAX_TL_BITS)? as usize;
    let layout = interleave::layout(tl_bits)?;
    Some(BurstLength {
        tl_bits,
        total_bits: HEADER_BITS + layout.total_tx_bits,
    })
}

fn uw_ramp() -> [f32; 16] {
    let mut ramp = [0.0f32; 16];
    for k in 1..16 {
        ramp[k] = ramp[k - 1] + f32::from(UW_DELTAS[k]) * PI / 4.0;
    }
    ramp
}

fn wrap(phase: f32) -> f32 {
    (phase + PI).rem_euclid(2.0 * PI) - PI
}

fn line_fit(r: &mut [f32; 16], w: &[f32; 16]) -> Option<Fit> {
    for k in 1..16 {
        let mut d = r[k] - r[k - 1];
        while d > PI {
            r[k] -= 2.0 * PI;
            d = r[k] - r[k - 1];
        }
        while d < -PI {
            r[k] += 2.0 * PI;
            d = r[k] - r[k - 1];
        }
    }
    let sw: f32 = w.iter().sum();
    if sw < 1e-12 {
        return None;
    }
    let kbar = w
        .iter()
        .enumerate()
        .map(|(k, &wk)| wk * k as f32)
        .sum::<f32>()
        / sw;
    let rbar = w
        .iter()
        .zip(r.iter())
        .map(|(&wk, &rk)| wk * rk)
        .sum::<f32>()
        / sw;
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    for k in 0..16 {
        let dk = k as f32 - kbar;
        num += w[k] * dk * (r[k] - rbar);
        den += w[k] * dk * dk;
    }
    if den < 1e-12 {
        return None;
    }
    let b = num / den;
    let a = rbar - b * kbar;
    let mut cost = 0.0f32;
    for k in 0..16 {
        let e = r[k] - a - b * k as f32;
        cost += w[k] * e * e;
    }
    Some(Fit {
        cost: cost / sw,
        slope: b,
        intercept: a,
    })
}

fn evm_snr_db(residuals: &[f32]) -> f32 {
    if residuals.is_empty() {
        return 0.0;
    }
    let evm2 = residuals.iter().map(|r| r * r).sum::<f32>() / residuals.len() as f32;
    -10.0 * evm2.max(1e-9).log10()
}
