use std::f32::consts::PI;
use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::frame::{ACCESS_DL, ACCESS_UL, FrameKind, classify};
use super::lcw::decode_lcw;

pub const SYMBOL_RATE: f64 = 25_000.0;
const SYNC_DL: [u8; 28] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 2, 0, 0, 0, 2, 0, 0, 2,
];
const SYNC_UL: [u8; 28] = [
    2, 0, 2, 0, 2, 0, 2, 0, 2, 0, 2, 0, 2, 0, 2, 0, 2, 2, 0, 0, 0, 2, 0, 0, 2, 0, 2, 2,
];
const UW_OFFSET: usize = 16;
const UW_SYMBOLS: usize = 12;
const DQPSK_MAP: [u8; 4] = [0, 2, 3, 1];
const ACCESS_TOL: usize = 12;
const PRE_SYMS: f64 = 64.0;
const GATE_MULT: f32 = 8.0;
const SIMPLEX_SYMBOLS: usize = 444;
const DUPLEX_SYMBOLS: usize = 191;
const DUPLEX_MAX_LCW_ERRORS: u32 = 6;
const CFO_REFINE: i32 = 2;
const CFO_REFINE_STEP: f32 = 0.08;
const MAX_SLOPE: f32 = 0.2;
const MIN_SYMBOLS: usize = UW_SYMBOLS + 32;
const MAX_FRAMES_PER_BURST: usize = 32;
const FFT_OVERSAMPLE: usize = 16;
const PLL_GAIN: f32 = 0.2;
const FIRST_LEAD: f64 = 200.0;
const FIRST_SPAN: f64 = 1_100.0;
const FADE_RATIO: f32 = 64.0;
const FADE_RUN: usize = 3;
const BOXCAR: usize = 16;

type Sync = (&'static [u8; 28], &'static [u8; 24]);
const SYNCS: [Sync; 2] = [(&SYNC_DL, ACCESS_DL), (&SYNC_UL, ACCESS_UL)];

pub struct DemodBurst {
    pub bits: Vec<u8>,
    pub reliability: Vec<f32>,
    pub cfo_hz: f64,
}

struct Symbols {
    decisions: Vec<u8>,
    confidence: Vec<f32>,
}

pub struct IridiumDemod {
    sps: f64,
    buf: Vec<Complex<f32>>,
    noise: f32,
    cfo_len: usize,
    cfo_fft: Arc<dyn Fft<f32>>,
    cfo_buf: Vec<Complex<f32>>,
    cfo_scratch: Vec<Complex<f32>>,
}

fn phasor(angle: f32) -> Complex<f32> {
    Complex::from_polar(1.0, angle)
}

fn expected(symbol: u8) -> f32 {
    f32::from(symbol) * PI / 2.0
}

fn slice(derotated: Complex<f32>) -> (u8, f32) {
    let angle = derotated.arg();
    let index = (angle / (PI / 2.0)).round();
    (
        (index as i32).rem_euclid(4) as u8,
        angle - index * (PI / 2.0),
    )
}

fn differential(symbols: &Symbols) -> (Vec<u8>, Vec<f32>) {
    let count = symbols.decisions.len();
    let mut bits = Vec::with_capacity(count * 2);
    let mut reliability = Vec::with_capacity(count * 2);
    let mut old = 0u8;
    let mut old_confidence = symbols.confidence.first().copied().unwrap_or(0.0);
    for (&symbol, &confidence) in symbols.decisions.iter().zip(&symbols.confidence) {
        let mapped = DQPSK_MAP[usize::from((symbol + 4 - old) % 4)];
        let weakest = confidence.min(old_confidence);
        old = symbol;
        old_confidence = confidence;
        bits.extend([mapped & 1, mapped >> 1]);
        reliability.extend([weakest, weakest]);
    }
    (bits, reliability)
}

fn access_errors(bits: &[u8], access: &[u8; 24]) -> usize {
    bits.iter()
        .take(24)
        .zip(access)
        .filter(|(a, b)| a != b)
        .count()
}

fn is_duplex(bits: &[u8]) -> bool {
    let Some(data) = bits.get(24..) else {
        return false;
    };
    let simplex = matches!(
        classify(data),
        FrameKind::Ms | FrameKind::Itl | FrameKind::Bc | FrameKind::Ra
    );
    !simplex && decode_lcw(data).is_some_and(|lcw| lcw.corrected <= DUPLEX_MAX_LCW_ERRORS)
}

fn blackman(k: usize, denom: f32) -> f32 {
    0.42 - 0.5 * (2.0 * PI * k as f32 / denom).cos() + 0.08 * (4.0 * PI * k as f32 / denom).cos()
}

fn line_slope(phases: &[f32]) -> f32 {
    let n = phases.len() as f32;
    let mean_k = (n - 1.0) / 2.0;
    let mean_phase = phases.iter().sum::<f32>() / n;
    let (num, den) = phases
        .iter()
        .enumerate()
        .fold((0.0f32, 0.0f32), |(num, den), (k, &p)| {
            let dk = k as f32 - mean_k;
            (num + dk * (p - mean_phase), den + dk * dk)
        });
    if den > 0.0 { num / den } else { 0.0 }
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
            noise: 1.0,
            cfo_len,
            cfo_fft,
            cfo_buf: vec![Complex::default(); size],
            cfo_scratch: vec![Complex::default(); scratch],
        }
    }

    fn sample(&self, pos: f64) -> Option<Complex<f32>> {
        if pos < 0.0 {
            return None;
        }
        let i = pos.floor() as usize;
        if i + 1 >= self.buf.len() {
            return None;
        }
        let frac = (pos - i as f64) as f32;
        Some(self.buf[i] * (1.0 - frac) + self.buf[i + 1] * frac)
    }

    fn tone_cfo(&mut self, pos: f64) -> Option<f64> {
        let n = self.cfo_len;
        let m = self.cfo_buf.len();
        let rel = pos.max(0.0) as usize;
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

    fn onset(&self, burst_start: f64) -> f64 {
        let mut window = [0.0f32; BOXCAR];
        let mut sum = 0.0f32;
        let scan_from = (burst_start - 8.0 * self.sps).max(0.0) as usize;
        for (rel, s) in self.buf.iter().enumerate().skip(scan_from) {
            let p = s.norm_sqr();
            sum += p - window[rel % BOXCAR];
            window[rel % BOXCAR] = p;
            if sum >= self.noise * GATE_MULT * BOXCAR as f32 {
                return (rel as f64 - BOXCAR as f64).max(0.0);
            }
        }
        burst_start
    }

    fn frame_symbols(&self, uw_pos: f64, theta: f32, phase: f32) -> Symbols {
        let mut decisions = Vec::new();
        let mut confidence = Vec::new();
        let mut carrier = phase;
        let mut peak = 0.0f32;
        let mut weak_run = 0usize;
        for k in 0..SIMPLEX_SYMBOLS {
            let Some(s) = self.sample(uw_pos + k as f64 * self.sps) else {
                break;
            };
            let p = s.norm_sqr();
            if k >= UW_SYMBOLS {
                peak = peak.max(p);
                weak_run = if p * FADE_RATIO < peak {
                    weak_run + 1
                } else {
                    0
                };
                if weak_run >= FADE_RUN {
                    decisions.truncate(decisions.len() + 1 - weak_run);
                    confidence.truncate(decisions.len());
                    break;
                }
            }
            let derotated = s * phasor(-(theta * k as f32) - carrier);
            let (symbol, residual) = slice(derotated);
            if k >= UW_SYMBOLS {
                carrier += PLL_GAIN * residual;
            }
            decisions.push(symbol);
            confidence.push(derotated.norm() * (2.0 * residual).cos().max(0.0));
        }
        Symbols {
            decisions,
            confidence,
        }
    }

    fn demod_from(
        &self,
        uw_pos: f64,
        theta: f32,
        phase: f32,
        access: &[u8; 24],
    ) -> (Option<DemodBurst>, usize) {
        let symbols = self.frame_symbols(uw_pos, theta, phase);
        let count = symbols.decisions.len();
        if count < MIN_SYMBOLS {
            return (None, count);
        }
        let (mut bits, mut reliability) = differential(&symbols);
        if access_errors(&bits, access) > ACCESS_TOL {
            return (None, count);
        }
        let count = if is_duplex(&bits) {
            bits.truncate(2 * DUPLEX_SYMBOLS);
            reliability.truncate(2 * DUPLEX_SYMBOLS);
            count.min(DUPLEX_SYMBOLS)
        } else {
            count
        };
        let cfo_hz = f64::from(theta) * SYMBOL_RATE / std::f64::consts::TAU;
        let burst = DemodBurst {
            bits,
            reliability,
            cfo_hz,
        };
        (Some(burst), count)
    }

    fn sync_score(&self, o: f64, sync: &[u8; 28], theta: f32) -> Option<f32> {
        let mut acc = Complex::new(0.0f32, 0.0);
        let mut mag = 0.0f32;
        for (k, &symbol) in sync.iter().enumerate() {
            let s = self.sample(o + k as f64 * self.sps)?;
            acc += s * phasor(-expected(symbol) - theta * k as f32);
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

    fn sync_slope(&self, o: f64, sync: &[u8; 28], theta: f32) -> Option<f32> {
        let mut phases = [0.0f32; 28];
        let mut previous = 0.0f32;
        for (k, &symbol) in sync.iter().enumerate() {
            let s = self.sample(o + k as f64 * self.sps)?;
            let z = s * phasor(-expected(symbol) - theta * k as f32);
            let wraps = ((z.arg() - previous) / (2.0 * PI)).round();
            phases[k] = z.arg() - wraps * 2.0 * PI;
            previous = phases[k];
        }
        Some(line_slope(&phases))
    }

    fn refine_sync(&self, o: f64, sync: &[u8; 28], theta: f32) -> (f64, f32) {
        let theta = theta
            + self
                .sync_slope(o, sync, theta)
                .map_or(0.0, |slope| slope.clamp(-MAX_SLOPE, MAX_SLOPE));
        let score = |t: f64| self.sync_score(t, sync, theta);
        let (Some(left), Some(mid), Some(right)) = (score(o - 1.0), score(o), score(o + 1.0))
        else {
            return (o, theta);
        };
        let curve = left - 2.0 * mid + right;
        if curve >= 0.0 {
            return (o, theta);
        }
        let shift = (0.5 * (left - right) / curve).clamp(-0.5, 0.5);
        (o + f64::from(shift), theta)
    }

    fn uw_phase(&self, uw_pos: f64, sync: &[u8; 28], theta: f32) -> f32 {
        let mut acc = Complex::new(0.0f32, 0.0);
        for (j, &symbol) in sync[UW_OFFSET..].iter().enumerate() {
            if let Some(s) = self.sample(uw_pos + j as f64 * self.sps) {
                acc += s * phasor(-expected(symbol) - theta * j as f32);
            }
        }
        acc.arg()
    }

    pub fn acquire(
        &mut self,
        chan: &[Complex<f32>],
        burst_start: f64,
        noise: f32,
    ) -> Vec<DemodBurst> {
        self.buf.clear();
        self.buf.extend_from_slice(chan);
        self.noise = noise.max(1e-12);
        let onset = self.onset(burst_start);
        let Some(onset_cfo) = self.tone_cfo(onset) else {
            return Vec::new();
        };
        let to_theta = |cfo: f64| (2.0 * std::f64::consts::PI * cfo / SYMBOL_RATE) as f32;
        let onset_theta = to_theta(onset_cfo);
        let end = self.buf.len() as f64;
        let mut out = Vec::new();
        let mut search_lo = (onset - FIRST_LEAD).max(0.0);
        let mut span = FIRST_SPAN;
        let mut cfo_pos = onset;
        for _ in 0..MAX_FRAMES_PER_BURST {
            if search_lo + MIN_SYMBOLS as f64 * self.sps > end {
                break;
            }
            let hi = (search_lo + span).min(end);
            span = (PRE_SYMS + 20.0) * self.sps;
            let theta = self.tone_cfo(cfo_pos).map_or(onset_theta, to_theta);
            let Some((o, db, (sync, access))) = self.best_sync(search_lo, hi, theta) else {
                break;
            };
            let (o, theta) = self.refine_sync(o, sync, theta + db);
            let uw_pos = o + UW_OFFSET as f64 * self.sps;
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
