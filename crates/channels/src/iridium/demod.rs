use std::f32::consts::PI;
use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::frame::{ACCESS_DL, ACCESS_UL};

pub const SYMBOL_RATE: f64 = 25_000.0;
const UW_DL: [u8; 12] = [0, 2, 2, 2, 2, 0, 0, 0, 2, 0, 0, 2];
const UW_UL: [u8; 12] = [2, 2, 0, 0, 0, 2, 0, 0, 2, 0, 2, 2];
const SYNC_DL: [u8; 28] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 2, 0, 0, 0, 2, 0, 0, 2,
];
const SYNC_UL: [u8; 28] = [
    2, 0, 2, 0, 2, 0, 2, 0, 2, 0, 2, 0, 2, 0, 2, 0, 2, 2, 0, 0, 0, 2, 0, 0, 2, 0, 2, 2,
];
const DQPSK_MAP: [u8; 4] = [0, 2, 3, 1];
const MAX_BURST_SYMS: usize = 2_250;
const ACCESS_TOL: usize = 12;
const PRE_SYMS: f64 = 64.0;
const UW_CORR: f32 = 0.97;
const GATE_MULT: f32 = 8.0;
const FRAME_CAP: usize = 200;
const CFO_REFINE: i32 = 2;
const CFO_REFINE_STEP: f32 = 0.08;
const MIN_SYMBOLS: usize = 12 + 32;
const MAX_FRAMES_PER_BURST: usize = 32;
const FFT_OVERSAMPLE: usize = 16;
const PLL_GAIN: f32 = 0.2;

type Uw = (&'static [u8; 12], &'static [u8; 24]);
type Sync = (&'static [u8; 28], &'static [u8; 24]);
const UWS: [Uw; 2] = [(&UW_DL, ACCESS_DL), (&UW_UL, ACCESS_UL)];
const SYNCS: [Sync; 2] = [(&SYNC_DL, ACCESS_DL), (&SYNC_UL, ACCESS_UL)];

pub struct DemodBurst {
    pub bits: Vec<u8>,
    pub cfo_hz: f64,
}

#[derive(Clone, Copy)]
struct Lock {
    corr: f32,
    pos: f64,
    theta: f32,
    phase: f32,
    access: &'static [u8; 24],
}

pub struct IridiumDemod {
    sps: f64,
    buf: Vec<Complex<f32>>,
    start_abs: f64,
    cursor: f64,
    noise: f32,
    pwr_win: [f32; 16],
    pwr_pos: usize,
    pwr_sum: f32,
    cfo_len: usize,
    cfo_fft: Arc<dyn Fft<f32>>,
    cfo_buf: Vec<Complex<f32>>,
    cfo_scratch: Vec<Complex<f32>>,
}

fn phasor(angle: f32) -> Complex<f32> {
    Complex::from_polar(1.0, angle)
}

fn slice(derotated: Complex<f32>) -> (u8, f32) {
    let angle = derotated.arg();
    let index = (angle / (PI / 2.0)).round();
    ((index as i32).rem_euclid(4) as u8, angle - index * (PI / 2.0))
}

fn differential_bits(symbols: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(symbols.len() * 2);
    let mut old = 0u8;
    for &symbol in symbols {
        let mapped = DQPSK_MAP[usize::from((symbol + 4 - old) % 4)];
        old = symbol;
        bits.push(mapped & 1);
        bits.push(mapped >> 1);
    }
    bits
}

fn access_errors(bits: &[u8], access: &[u8; 24]) -> usize {
    bits.iter().take(24).zip(access).filter(|(a, b)| a != b).count()
}

fn blackman(k: usize, denom: f32) -> f32 {
    0.42 - 0.5 * (2.0 * PI * k as f32 / denom).cos() + 0.08 * (4.0 * PI * k as f32 / denom).cos()
}

impl IridiumDemod {
    pub fn new(channel_rate: f64) -> Self {
        let sps = channel_rate / SYMBOL_RATE;
        let base = ((sps * 26.0) as usize).max(4);
        let cfo_len = 1usize << (usize::BITS - 1 - base.leading_zeros());
        let size = cfo_len * FFT_OVERSAMPLE;
        let cfo_fft = FftPlanner::new().plan_fft_forward(size);
        let scratch = cfo_fft.get_inplace_scratch_len();
        Self {
            sps,
            buf: Vec::new(),
            start_abs: 0.0,
            cursor: 0.0,
            noise: 1.0,
            pwr_win: [0.0; 16],
            pwr_pos: 0,
            pwr_sum: 0.0,
            cfo_len,
            cfo_fft,
            cfo_buf: vec![Complex::default(); size],
            cfo_scratch: vec![Complex::default(); scratch],
        }
    }

    pub fn seed_noise(&mut self, noise: f32) {
        self.noise = noise.max(1e-12);
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

    fn push_power(&mut self, p: f32) {
        self.pwr_sum += p - self.pwr_win[self.pwr_pos];
        self.pwr_win[self.pwr_pos] = p;
        self.pwr_pos = (self.pwr_pos + 1) % 16;
    }

    fn gate_open(&self) -> bool {
        self.pwr_sum >= self.noise * GATE_MULT * 16.0
    }

    fn tone_cfo(&mut self, pos: f64) -> Option<f64> {
        let n = self.cfo_len;
        let m = self.cfo_buf.len();
        let rel = (pos - self.start_abs) as usize;
        if rel + n > self.buf.len() {
            return None;
        }
        let denom = (n - 1) as f32;
        for (k, slot) in self.cfo_buf.iter_mut().enumerate() {
            *slot = if k < n {
                let s = self.buf[rel + k];
                s * s * blackman(k, denom)
            } else {
                Complex::default()
            };
        }
        self.cfo_fft
            .process_with_scratch(&mut self.cfo_buf, &mut self.cfo_scratch);
        let spectrum = &self.cfo_buf;
        let peak =
            (0..m).max_by(|&a, &b| spectrum[a].norm_sqr().total_cmp(&spectrum[b].norm_sqr()))?;
        let a = spectrum[(peak + m - 1) % m].norm_sqr();
        let b = spectrum[peak].norm_sqr();
        let c = spectrum[(peak + 1) % m].norm_sqr();
        let d = a - 2.0 * b + c;
        let frac = if d.abs() > 1e-20 {
            f64::from((0.5 * (a - c) / d).clamp(-1.0, 1.0))
        } else {
            0.0
        };
        let mut index = peak as f64 + frac;
        if index >= m as f64 / 2.0 {
            index -= m as f64;
        }
        Some(index / m as f64 / 2.0 * (SYMBOL_RATE * self.sps))
    }

    fn uw_residual(&self, cand: f64, uw: &[u8; 12], theta0: f32) -> Option<Option<f32>> {
        let Some(first) = self.sample(cand) else {
            return None;
        };
        let mut prev = first * phasor(-(f32::from(uw[0]) * PI / 2.0));
        let mut sum = Complex::new(0.0f32, 0.0);
        for (k, &symbol) in uw.iter().enumerate().skip(1) {
            let Some(s) = self.sample(cand + k as f64 * self.sps) else {
                return Some(None);
            };
            let cur = s * phasor(-(f32::from(symbol) * PI / 2.0) - theta0 * k as f32);
            sum += cur * prev.conj();
            prev = cur;
        }
        Some(Some(sum.arg().clamp(-0.5, 0.5)))
    }

    fn uw_project(&self, cand: f64, uw: &[u8; 12], theta: f32) -> Option<(Complex<f32>, f32)> {
        let mut acc = Complex::new(0.0f32, 0.0);
        let mut mag_sum = 0.0f32;
        for (k, &symbol) in uw.iter().enumerate() {
            let s = self.sample(cand + k as f64 * self.sps)?;
            let expect = f32::from(symbol) * PI / 2.0;
            acc += s * phasor(-expect - theta * k as f32);
            mag_sum += s.norm();
        }
        (mag_sum > 1e-12).then_some((acc, mag_sum))
    }

    fn uw_fit(&self, pos: f64, uw: &[u8; 12], theta0: f32) -> Option<(f32, f64, f32, f32)> {
        let mut best: Option<(f32, f64, f32, f32)> = None;
        let mut t = -self.sps;
        while t <= self.sps {
            let cand = pos + t;
            t += 0.25;
            if cand < self.start_abs {
                continue;
            }
            let Some(residual) = self.uw_residual(cand, uw, theta0) else {
                break;
            };
            let Some(db) = residual else { continue };
            let Some((acc, mag_sum)) = self.uw_project(cand, uw, theta0 + db) else {
                continue;
            };
            let corr = acc.norm() / mag_sum;
            if best.is_none_or(|(c, _, _, _)| corr > c) {
                best = Some((corr, cand, theta0 + db, acc.arg()));
            }
        }
        best
    }

    fn hunt_uw(&self, bstart: f64, theta0: f32) -> Option<Lock> {
        let mut found: Option<Lock> = None;
        let mut hunt = 6.0 * self.sps;
        while hunt < (PRE_SYMS + 26.0) * self.sps {
            let cand = bstart + hunt;
            hunt += 2.0 * self.sps;
            if !self.sample(cand).is_some_and(|s| s.norm_sqr() > self.noise) {
                continue;
            }
            for (uw, access) in UWS {
                let Some((corr, pos, theta, phase)) = self.uw_fit(cand, uw, theta0) else {
                    continue;
                };
                if corr > UW_CORR && found.is_none_or(|lock| corr > lock.corr) {
                    found = Some(Lock {
                        corr,
                        pos,
                        theta,
                        phase,
                        access,
                    });
                }
            }
        }
        found
    }

    fn stream_symbols(&self, lock: &Lock) -> Vec<u8> {
        let mut symbols = Vec::new();
        let mut carrier = lock.phase;
        for k in 0..12 + MAX_BURST_SYMS {
            let Some(s) = self.sample(lock.pos + k as f64 * self.sps) else {
                break;
            };
            if k >= 12 && s.norm_sqr() < self.noise * 4.0 {
                break;
            }
            let (symbol, residual) = slice(s * phasor(-(lock.theta * k as f32) - carrier));
            if k >= 12 {
                carrier += PLL_GAIN * residual;
            }
            symbols.push(symbol);
        }
        symbols
    }

    fn advance_noise(&mut self, pos: f64) {
        let rel = (pos - self.start_abs) as usize;
        let p = self.buf[rel].norm_sqr();
        if p < self.noise {
            self.noise += 0.01 * (p - self.noise);
        } else {
            self.noise += 1e-5 * (p - self.noise);
        }
        self.push_power(p);
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<DemodBurst>) {
        self.buf.extend_from_slice(input);
        let span = (PRE_SYMS + 14.0 + MAX_BURST_SYMS as f64) * self.sps;
        while self.cursor < self.start_abs + self.buf.len() as f64 - span {
            let pos = self.cursor;
            self.cursor += 1.0;
            self.advance_noise(pos);
            if !self.gate_open() {
                continue;
            }
            let bstart = pos - 16.0;
            let Some(cfo) = self.tone_cfo(bstart) else {
                break;
            };
            let theta0 = (2.0 * std::f64::consts::PI * cfo / SYMBOL_RATE) as f32;
            let Some(lock) = self.hunt_uw(bstart, theta0) else {
                self.cursor = pos + 32.0 * self.sps;
                continue;
            };
            let symbols = self.stream_symbols(&lock);
            self.cursor = lock.pos + symbols.len().max(1) as f64 * self.sps;
            if symbols.len() < MIN_SYMBOLS {
                continue;
            }
            let bits = differential_bits(&symbols);
            if access_errors(&bits, lock.access) <= ACCESS_TOL {
                out.push(DemodBurst {
                    bits,
                    cfo_hz: f64::from(lock.theta) * SYMBOL_RATE / std::f64::consts::TAU,
                });
            }
        }
        let keep_from = (self.cursor - self.start_abs - 4.0 * self.sps).max(0.0) as usize;
        if keep_from > 0 && keep_from <= self.buf.len() {
            self.buf.drain(..keep_from);
            self.start_abs += keep_from as f64;
        }
    }

    fn frame_symbols(&self, uw_pos: f64, theta: f32, phase: f32) -> Vec<u8> {
        let mut symbols = Vec::new();
        let mut carrier = phase;
        let mut peak = 0.0f32;
        let mut weak_run = 0usize;
        for k in 0..12 + FRAME_CAP {
            let Some(s) = self.sample(uw_pos + k as f64 * self.sps) else {
                break;
            };
            let p = s.norm_sqr();
            if k >= 12 {
                peak = peak.max(p);
                if p * 64.0 < peak {
                    weak_run += 1;
                    if weak_run >= 3 {
                        symbols.truncate(symbols.len() + 1 - weak_run);
                        break;
                    }
                } else {
                    weak_run = 0;
                }
            }
            let (symbol, residual) = slice(s * phasor(-(theta * k as f32) - carrier));
            if k >= 12 {
                carrier += PLL_GAIN * residual;
            }
            symbols.push(symbol);
        }
        symbols
    }

    fn demod_from(
        &self,
        uw_pos: f64,
        theta: f32,
        phase: f32,
        access: &[u8; 24],
    ) -> (Option<DemodBurst>, usize) {
        let symbols = self.frame_symbols(uw_pos, theta, phase);
        let count = symbols.len();
        if count < MIN_SYMBOLS {
            return (None, count);
        }
        let bits = differential_bits(&symbols);
        if access_errors(&bits, access) > ACCESS_TOL {
            return (None, count);
        }
        let cfo_hz = f64::from(theta) * SYMBOL_RATE / std::f64::consts::TAU;
        (Some(DemodBurst { bits, cfo_hz }), count)
    }

    fn onset(&mut self, burst_start: f64) -> f64 {
        self.pwr_win = [0.0; 16];
        self.pwr_pos = 0;
        self.pwr_sum = 0.0;
        let scan_from = (burst_start - 8.0 * self.sps).max(0.0) as usize;
        for rel in scan_from..self.buf.len() {
            let p = self.buf[rel].norm_sqr();
            self.push_power(p);
            if self.gate_open() {
                return (rel as f64 - 16.0).max(0.0);
            }
        }
        burst_start
    }

    fn sync_score(&self, o: f64, sync: &[u8; 28], theta: f32) -> Option<f32> {
        let mut acc = Complex::new(0.0f32, 0.0);
        let mut mag = 0.0f32;
        for (k, &symbol) in sync.iter().enumerate() {
            let s = self.sample(o + k as f64 * self.sps)?;
            let expect = f32::from(symbol) * PI / 2.0;
            acc += s * phasor(-expect - theta * k as f32);
            mag += s.norm();
        }
        (mag > 1e-12).then(|| acc.norm() / mag)
    }

    fn best_sync(&self, lo: f64, hi: f64, theta: f32) -> Option<(f64, f32, Sync)> {
        let mut best: Option<(f32, f64, f32, Sync)> = None;
        let mut o = lo;
        while o < hi {
            for entry in SYNCS {
                for step in -CFO_REFINE..=CFO_REFINE {
                    let db = step as f32 * CFO_REFINE_STEP;
                    let Some(corr) = self.sync_score(o, entry.0, theta + db) else {
                        continue;
                    };
                    if best.is_none_or(|(c, ..)| corr > c) {
                        best = Some((corr, o, db, entry));
                    }
                }
            }
            o += 1.0;
        }
        best.map(|(_, o, db, entry)| (o, db, entry))
    }

    fn uw_phase(&self, uw_pos: f64, sync: &[u8; 28], theta: f32) -> f32 {
        let mut acc = Complex::new(0.0f32, 0.0);
        for (j, &symbol) in sync[16..].iter().enumerate() {
            if let Some(s) = self.sample(uw_pos + j as f64 * self.sps) {
                acc += s * phasor(-(f32::from(symbol) * PI / 2.0) - theta * j as f32);
            }
        }
        acc.arg()
    }

    pub fn acquire_multi(&mut self, chan: &[Complex<f32>], burst_start: f64) -> Vec<DemodBurst> {
        self.buf.clear();
        self.buf.extend_from_slice(chan);
        self.start_abs = 0.0;
        let onset = self.onset(burst_start);
        let Some(onset_cfo) = self.tone_cfo(onset) else {
            return Vec::new();
        };
        let to_theta = |cfo: f64| (2.0 * std::f64::consts::PI * cfo / SYMBOL_RATE) as f32;
        let onset_theta = to_theta(onset_cfo);
        let end = self.buf.len() as f64;
        let span = (PRE_SYMS + 20.0) * self.sps;
        let min_frame = MIN_SYMBOLS as f64 * self.sps;
        let mut out = Vec::new();
        let mut search_lo = (onset - 6.0 * self.sps).max(0.0);
        let mut cfo_pos = onset;
        for _ in 0..MAX_FRAMES_PER_BURST {
            if search_lo + min_frame > end {
                break;
            }
            let hi = (search_lo + span).min(end);
            let theta = self.tone_cfo(cfo_pos).map_or(onset_theta, to_theta);
            let Some((o, db, (sync, access))) = self.best_sync(search_lo, hi, theta) else {
                break;
            };
            let theta = theta + db;
            let uw_pos = o + 16.0 * self.sps;
            let phase = self.uw_phase(uw_pos, sync, theta);
            let (frame, count) = self.demod_from(uw_pos, theta, phase, access);
            out.extend(frame);
            if count < MIN_SYMBOLS {
                break;
            }
            let next = uw_pos + count as f64 * self.sps;
            if next <= search_lo + self.sps {
                break;
            }
            search_lo = next;
            cfo_pos = next;
        }
        out
    }
}
