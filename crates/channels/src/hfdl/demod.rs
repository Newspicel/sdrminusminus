use std::f32::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::SoftViterbi;

use super::{
    fec::{self, A, SEGMENT_SYMBOLS, SEQUENCE_LEN, SETTINGS, Setting, T, TRAINING_LEN},
    lms::{EQ_TAPS, Lms},
    pdu::PduParser,
};

pub const SYMBOL_RATE: f64 = 1_800.0;
const CORR_A1: f32 = 0.4;
const M1_THRESHOLD: f32 = 0.4;
const M1_CHUNK: usize = 16;
const PREAMBLE_TRAINING_SEGMENTS: usize = 9;
const PREAMBLE_SYMS: usize =
    3 * SEQUENCE_LEN + TRAINING_LEN + PREAMBLE_TRAINING_SEGMENTS * TRAINING_LEN;
const SEGMENT_TOTAL: usize = SEGMENT_SYMBOLS + TRAINING_LEN;
const M1_OFFSET_SYMS: f64 = 254.0;
const M1_END_SYMS: f64 = 384.0;
const TAIL_SYMS: f64 = 6.0;
const EQ_LOOKAHEAD: usize = EQ_TAPS / 2;
const EQ_MU: f32 = 0.10;
const DECISION_MU: f32 = 0.03;
const NOISE_FLOOR: f32 = 0.01;
const LOOP_FREQ_GAIN: f32 = 0.002;
const LOOP_PHASE_GAIN: f32 = 0.08;
const NOISE_FALL: f32 = 0.01;
const NOISE_RISE: f32 = 1e-5;
const TRIGGER_OVER_NOISE: f32 = 4.0;
const HUNT_SPAN_SYMS: f64 = 130.0;
const PEAK_STEPS: [f64; 6] = [-2.0, -1.0, -0.5, 0.5, 1.0, 2.0];
const FIT_STEP: f64 = 0.25;
const FIT_MIN_HALF_WIDTH: f64 = 4.0;
const FIT_HALF_WIDTH_SYMS: f64 = 0.6;
const MISS_SKIP: f64 = 64.0;
const KEEP_SYMS: f64 = 4.0;
const RESCUE_TIMING: [f64; 10] = [-1.0, -0.5, 0.5, 1.0, -1.5, 1.5, -2.0, 2.0, -3.0, 3.0];
const RESCUE_CARRIER: [f32; 9] = [
    0.0, -0.007, 0.007, -0.017, 0.017, -0.03, 0.03, -0.045, 0.045,
];

pub struct Burst {
    pub bps: u32,
    pub payload: Vec<u8>,
    pub fec_corrected: u32,
    pub freq_skew_hz: f32,
    pub snr_db: Option<f32>,
}

enum State {
    Hunt,
    Collect {
        a1_pos: f64,
        theta: f32,
        plan: Option<(Setting, usize)>,
    },
}

enum Detection {
    Pending,
    Found(Setting, usize),
    Missed,
}

#[derive(Clone, Copy)]
struct Carrier {
    a1_pos: f64,
    theta: f32,
    phase: f32,
    gain: f32,
}

struct Walk {
    eq: Lms,
    tap_pos: f64,
    loop_phase: f32,
    loop_freq: f32,
    signal_power: f64,
    error_power: f64,
    segment_error: f32,
}

struct Equalized {
    soft: Vec<f32>,
    loop_freq: f32,
    signal_power: f64,
    error_power: f64,
}

pub struct HfdlDemod {
    sps: f64,
    buf: Vec<Complex<f32>>,
    start_abs: f64,
    cursor: f64,
    noise: f32,
    state: State,
    viterbi: SoftViterbi,
}

impl HfdlDemod {
    pub fn new(channel_rate: f64) -> Self {
        Self {
            sps: channel_rate / SYMBOL_RATE,
            buf: Vec::new(),
            start_abs: 0.0,
            cursor: 0.0,
            noise: 1e-6,
            state: State::Hunt,
            viterbi: SoftViterbi::new(7, 0o133, 0o171),
        }
    }

    fn sample(&self, abs_pos: f64) -> Option<Complex<f32>> {
        let rel = abs_pos - self.start_abs;
        if rel < 0.0 {
            return None;
        }
        let index = rel.floor() as usize;
        let next = self.buf.get(index + 1)?;
        let frac = (rel - index as f64) as f32;
        Some(self.buf[index] * (1.0 - frac) + next * frac)
    }

    fn a_diff_correlate(&self, pos: f64) -> Option<(f32, f32)> {
        let mut corr = Complex::new(0.0f32, 0.0);
        let mut norm = 0.0f32;
        let mut prev = self.sample(pos)?;
        for j in 1..SEQUENCE_LEN {
            let s = self.sample(pos + j as f64 * self.sps)?;
            let d = s * prev.conj();
            prev = s;
            let sign = if A[j] != A[j - 1] { -1.0 } else { 1.0 };
            corr += d * sign;
            norm += d.norm();
        }
        if norm < 1e-9 {
            return None;
        }
        Some((corr.norm() / norm, corr.arg()))
    }

    fn coherent(&self, pos: f64, bits: &[u8], theta: f32) -> Option<Complex<f32>> {
        let mut corr = Complex::new(0.0f32, 0.0);
        for (j, &bit) in bits.iter().enumerate() {
            let derot = self.sample(pos + j as f64 * self.sps)?
                * Complex::from_polar(1.0, -theta * j as f32);
            corr += if bit == 1 { -derot } else { derot };
        }
        Some(corr)
    }

    fn m1_metric(&self, pos: f64, setting: &Setting, theta: f32) -> Option<f32> {
        let mut total = 0.0f32;
        let mut energy = 0.0f32;
        let mut corr = Complex::new(0.0f32, 0.0);
        for j in 0..SEQUENCE_LEN {
            let s = self.sample(pos + j as f64 * self.sps)?;
            energy += s.norm_sqr();
            let derot = s * Complex::from_polar(1.0, -theta * j as f32);
            corr += if fec::m1_chip(setting, j) == 1 {
                -derot
            } else {
                derot
            };
            if (j + 1) % M1_CHUNK == 0 || j + 1 == SEQUENCE_LEN {
                total += corr.norm();
                corr = Complex::new(0.0, 0.0);
            }
        }
        Some(total / (energy * SEQUENCE_LEN as f32).sqrt().max(1e-12))
    }

    fn a1_fit(&self, pos: f64, theta0: f32) -> Option<(f64, f32)> {
        let mut best: Option<(f32, f64, f32)> = None;
        let half = (FIT_HALF_WIDTH_SYMS * self.sps).max(FIT_MIN_HALF_WIDTH);
        let mut offset = -half;
        while offset <= half {
            let candidate = pos + offset;
            offset += FIT_STEP;
            if candidate < self.start_abs {
                continue;
            }
            let Some((cost, slope)) = self.a1_line_fit(candidate, theta0)? else {
                continue;
            };
            if best.is_none_or(|(c, _, _)| cost < c) {
                best = Some((cost, candidate, theta0 + slope));
            }
        }
        best.map(|(_, position, theta)| (position, theta))
    }

    fn a1_line_fit(&self, candidate: f64, theta0: f32) -> Option<Option<(f32, f32)>> {
        let mut residual = [0.0f32; SEQUENCE_LEN];
        let mut weight = [0.0f32; SEQUENCE_LEN];
        let mut prev = 0.0f32;
        for k in 0..SEQUENCE_LEN {
            let s = self.sample(candidate + k as f64 * self.sps)?;
            let sign = if A[k] == 1 { -1.0f32 } else { 1.0 };
            let mut phase = (s * Complex::from_polar(sign, -theta0 * k as f32)).arg();
            while phase - prev > PI {
                phase -= 2.0 * PI;
            }
            while phase - prev < -PI {
                phase += 2.0 * PI;
            }
            prev = phase;
            residual[k] = phase;
            weight[k] = s.norm_sqr();
        }
        Some(weighted_line_fit(&residual, &weight))
    }

    fn hunt(&mut self) -> Option<(f64, f32)> {
        let span = HUNT_SPAN_SYMS * self.sps;
        let end_abs = self.start_abs + self.buf.len() as f64 - span - 2.0;
        while self.cursor < end_abs {
            let pos = self.cursor;
            self.cursor += 1.0;
            let rel = (pos - self.start_abs) as usize;
            let power = self.buf[rel].norm_sqr();
            if power < self.noise {
                self.noise += NOISE_FALL * (power - self.noise);
            } else {
                self.noise += NOISE_RISE * (power - self.noise);
            }
            if power < self.noise * TRIGGER_OVER_NOISE {
                continue;
            }
            if let Some((metric, theta)) = self.a_diff_correlate(pos)
                && metric > CORR_A1
            {
                return Some(self.refine_trigger(pos, metric, theta));
            }
        }
        None
    }

    fn refine_trigger(&self, pos: f64, metric: f32, theta: f32) -> (f64, f32) {
        let mut best = (metric, pos);
        for step in PEAK_STEPS {
            if let Some((m, _)) = self.a_diff_correlate(pos + step)
                && m > best.0
            {
                best = (m, pos + step);
            }
        }
        let theta2 = self
            .a_diff_correlate(best.1)
            .map_or(theta, |(_, refined)| refined);
        self.a1_fit(best.1, theta2).unwrap_or((best.1, theta2))
    }

    fn acquire(&self, a1_pos: f64, theta0: f32, setting: &Setting) -> Option<Carrier> {
        let c1 = self.coherent(a1_pos, &A, theta0)?;
        let c2 = self.coherent(a1_pos + SEQUENCE_LEN as f64 * self.sps, &A, theta0)?;
        let dphi = (c2 * c1.conj()).arg();
        let theta = theta0 + dphi / SEQUENCE_LEN as f32;
        let c1r = self.coherent(a1_pos, &A, theta)?;
        let flip = if c1r.re < 0.0 { -1.0f32 } else { 1.0 };
        let phase = (c1r * flip).arg();
        let amp = (c1r.norm() / SEQUENCE_LEN as f32).max(1e-9);
        let m1_pos = a1_pos + M1_OFFSET_SYMS * self.sps;
        if self.m1_metric(m1_pos, setting, theta)? < M1_THRESHOLD {
            return None;
        }
        Some(Carrier {
            a1_pos,
            theta,
            phase,
            gain: flip / amp,
        })
    }

    fn derotated(&self, carrier: &Carrier, pos: f64, extra: f32) -> Option<Complex<f32>> {
        let y = self.sample(pos)?;
        let rel = ((pos - carrier.a1_pos) / self.sps) as f32;
        Some(y * Complex::from_polar(carrier.gain, -carrier.theta * rel - carrier.phase - extra))
    }

    fn start_walk(&self, carrier: &Carrier) -> Option<Walk> {
        let seg0 = carrier.a1_pos + (3 * SEQUENCE_LEN + TRAINING_LEN) as f64 * self.sps;
        let mut walk = Walk {
            eq: Lms::new(),
            tap_pos: seg0 - EQ_LOOKAHEAD as f64 * self.sps,
            loop_phase: 0.0,
            loop_freq: 0.0,
            signal_power: 0.0,
            error_power: 0.0,
            segment_error: 0.0,
        };
        for _ in 0..EQ_TAPS {
            walk.eq
                .push(self.derotated(carrier, walk.tap_pos, walk.loop_phase)?);
            walk.tap_pos += self.sps;
        }
        Some(walk)
    }

    fn symbol(
        &self,
        carrier: &Carrier,
        walk: &mut Walk,
        step: f32,
        train: Option<u8>,
        measure: bool,
    ) -> Option<Complex<f32>> {
        let y = walk.eq.exec();
        let phase_error = match train {
            Some(bit) => {
                let d = Complex::new(if bit == 1 { -1.0 } else { 1.0 }, 0.0);
                walk.eq.step(d, y, EQ_MU);
                walk.segment_error += (y - d).norm_sqr() / TRAINING_LEN as f32;
                if measure {
                    walk.signal_power += f64::from(d.norm_sqr());
                    walk.error_power += f64::from((y - d).norm_sqr());
                }
                (y * d.conj()).arg()
            }
            None => {
                let angle = y.arg();
                let nearest = (angle / step).round() * step;
                walk.eq
                    .step(Complex::from_polar(1.0, nearest), y, DECISION_MU);
                angle - nearest
            }
        };
        walk.loop_freq += LOOP_FREQ_GAIN * phase_error;
        walk.loop_phase += walk.loop_freq + LOOP_PHASE_GAIN * phase_error;
        walk.eq
            .push(self.derotated(carrier, walk.tap_pos, walk.loop_phase)?);
        walk.tap_pos += self.sps;
        Some(y)
    }

    fn train(&self, carrier: &Carrier, walk: &mut Walk, step: f32, measure: bool) -> Option<()> {
        walk.segment_error = 0.0;
        for &bit in &T {
            self.symbol(carrier, walk, step, Some(bit), measure)?;
        }
        Some(())
    }

    fn equalize(&self, carrier: &Carrier, setting: &Setting) -> Option<Equalized> {
        let mut walk = self.start_walk(carrier)?;
        let step = TAU / (1u32 << setting.bits_per_symbol) as f32;
        for _ in 0..PREAMBLE_TRAINING_SEGMENTS {
            self.train(carrier, &mut walk, step, false)?;
        }
        let levels = 1u32 << setting.bits_per_symbol;
        let mut soft = Vec::with_capacity(setting.chips());
        let mut data_index = 0usize;
        for _ in 0..setting.data_segments() {
            let before = walk.segment_error;
            let start = soft.len();
            for _ in 0..SEGMENT_SYMBOLS {
                let y = self.symbol(carrier, &mut walk, step, None, false)?;
                let point = if fec::scramble_flip(data_index) {
                    -y
                } else {
                    y
                };
                data_index += 1;
                for bit in 0..setting.bits_per_symbol {
                    soft.push(gray_llr(point, levels, bit));
                }
            }
            self.train(carrier, &mut walk, step, true)?;
            let weight = 2.0 / (before + walk.segment_error).max(2.0 * NOISE_FLOOR);
            for value in &mut soft[start..] {
                *value *= weight;
            }
        }
        Some(Equalized {
            soft,
            loop_freq: walk.loop_freq,
            signal_power: walk.signal_power,
            error_power: walk.error_power,
        })
    }

    fn decode_soft(&self, soft: Vec<f32>, setting: &Setting) -> (Vec<u8>, u32) {
        let deleaved = fec::deinterleave(&soft, setting);
        let viterbi_in: Vec<f32> = if setting.rate_quarter {
            deleaved
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[first, second]| (first + second) / 2.0)
                .collect()
        } else {
            deleaved
        };
        let bits = self.viterbi.decode(&viterbi_in);
        let recoded = self.viterbi.encode(&bits);
        let corrected = recoded
            .iter()
            .zip(&viterbi_in)
            .filter(|&(&chip, &soft)| (chip == 1) != (soft > 0.0))
            .count();
        let payload = bits
            .chunks(8)
            .map(|byte| {
                byte.iter()
                    .enumerate()
                    .fold(0u8, |acc, (i, &bit)| acc | (bit << i))
            })
            .collect();
        (payload, u32::try_from(corrected).unwrap_or(u32::MAX))
    }

    fn finish(&self, a1_pos: f64, theta0: f32, setting: Setting) -> Option<Burst> {
        let carrier = self.acquire(a1_pos, theta0, &setting)?;
        let equalized = self.equalize(&carrier, &setting)?;
        let (payload, fec_corrected) = self.decode_soft(equalized.soft, &setting);
        let rotation = carrier.theta + equalized.loop_freq;
        let freq_skew_hz = (f64::from(rotation) * SYMBOL_RATE / std::f64::consts::TAU) as f32;
        let snr_db = (equalized.signal_power > 0.0 && equalized.error_power > 0.0)
            .then(|| 10.0 * (equalized.signal_power / equalized.error_power).log10() as f32);
        Some(Burst {
            bps: setting.bps,
            payload,
            fec_corrected,
            freq_skew_hz,
            snr_db,
        })
    }

    fn detect_setting(&self, a1_pos: f64, theta: f32) -> Detection {
        if self.sample(a1_pos + M1_END_SYMS * self.sps).is_none() {
            return Detection::Pending;
        }
        let m1_pos = a1_pos + M1_OFFSET_SYMS * self.sps;
        let mut best: Option<(f32, Setting)> = None;
        for setting in &SETTINGS {
            if let Some(metric) = self.m1_metric(m1_pos, setting, theta)
                && best.is_none_or(|(b, _)| metric > b)
            {
                best = Some((metric, *setting));
            }
        }
        match best {
            Some((metric, setting)) if metric > M1_THRESHOLD => Detection::Found(
                setting,
                PREAMBLE_SYMS + setting.data_segments() * SEGMENT_TOTAL,
            ),
            _ => Detection::Missed,
        }
    }

    fn finish_with_rescue(&self, a1_pos: f64, theta: f32, setting: Setting) -> Option<Burst> {
        let nominal = self.finish(a1_pos, theta, setting);
        if nominal.as_ref().is_some_and(parses) {
            return nominal;
        }
        let mut chosen = nominal;
        for dt in RESCUE_TIMING {
            for dth in RESCUE_CARRIER {
                if let Some(burst) = self.finish(a1_pos + dt, theta + dth, setting) {
                    if parses(&burst) {
                        return Some(burst);
                    }
                    if chosen.is_none() {
                        chosen = Some(burst);
                    }
                }
            }
        }
        chosen
    }

    fn step_state(&mut self, state: State, out: &mut Vec<Burst>) -> Option<State> {
        match state {
            State::Hunt => {
                let (a1_pos, theta) = self.hunt()?;
                Some(State::Collect {
                    a1_pos,
                    theta,
                    plan: None,
                })
            }
            State::Collect {
                a1_pos,
                theta,
                plan,
            } => self.collect(a1_pos, theta, plan, out),
        }
    }

    fn collect(
        &mut self,
        a1_pos: f64,
        theta: f32,
        plan: Option<(Setting, usize)>,
        out: &mut Vec<Burst>,
    ) -> Option<State> {
        let pending = State::Collect {
            a1_pos,
            theta,
            plan,
        };
        let (setting, total) = match plan {
            Some(plan) => plan,
            None => match self.detect_setting(a1_pos, theta) {
                Detection::Pending => {
                    self.state = pending;
                    return None;
                }
                Detection::Missed => {
                    self.cursor = a1_pos + MISS_SKIP;
                    return Some(State::Hunt);
                }
                Detection::Found(setting, total) => (setting, total),
            },
        };
        let end = a1_pos + (total as f64 + TAIL_SYMS) * self.sps;
        if self.sample(end).is_none() {
            self.state = State::Collect {
                a1_pos,
                theta,
                plan: Some((setting, total)),
            };
            return None;
        }
        out.extend(self.finish_with_rescue(a1_pos, theta, setting));
        self.cursor = end;
        Some(State::Hunt)
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Burst>) {
        self.buf.extend_from_slice(input);
        loop {
            let state = std::mem::replace(&mut self.state, State::Hunt);
            match self.step_state(state, out) {
                Some(next) => self.state = next,
                None => break,
            }
        }
        self.drop_consumed();
    }

    fn drop_consumed(&mut self) {
        let active = match &self.state {
            State::Collect { a1_pos, .. } => *a1_pos - KEEP_SYMS * self.sps,
            State::Hunt => self.cursor - KEEP_SYMS * self.sps,
        };
        let keep_from = (active - self.start_abs).max(0.0) as usize;
        if keep_from > 0 && keep_from <= self.buf.len() {
            self.buf.drain(..keep_from);
            self.start_abs += keep_from as f64;
        }
    }
}

fn parses(burst: &Burst) -> bool {
    let mut events = Vec::new();
    PduParser::new().parse(&burst.payload, burst.bps, &mut events);
    !events.is_empty()
}

fn weighted_line_fit(residual: &[f32], weight: &[f32]) -> Option<(f32, f32)> {
    let total: f32 = weight.iter().sum();
    if total < 1e-12 {
        return None;
    }
    let k_mean = weight
        .iter()
        .enumerate()
        .map(|(k, &w)| w * k as f32)
        .sum::<f32>()
        / total;
    let r_mean = weight
        .iter()
        .zip(residual)
        .map(|(&w, &r)| w * r)
        .sum::<f32>()
        / total;
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    for (k, (&w, &r)) in weight.iter().zip(residual).enumerate() {
        let dk = k as f32 - k_mean;
        num += w * dk * (r - r_mean);
        den += w * dk * dk;
    }
    if den < 1e-12 {
        return None;
    }
    let slope = num / den;
    let intercept = r_mean - slope * k_mean;
    let mut cost = 0.0f32;
    for (k, (&w, &r)) in weight.iter().zip(residual).enumerate() {
        let e = r - intercept - slope * k as f32;
        cost += w * e * e;
    }
    Some((cost / total, slope))
}

fn gray_llr(y: Complex<f32>, levels: u32, bit: u32) -> f32 {
    let bits = levels.trailing_zeros();
    let mut d0 = f32::MAX;
    let mut d1 = f32::MAX;
    for n in 0..levels {
        let label = n ^ (n >> 1);
        let point = Complex::from_polar(1.0, TAU * n as f32 / levels as f32);
        let d = (y - point).norm_sqr();
        if (label >> (bits - 1 - bit)) & 1 == 1 {
            d1 = d1.min(d);
        } else {
            d0 = d0.min(d);
        }
    }
    (d0 - d1) / 2.0
}
