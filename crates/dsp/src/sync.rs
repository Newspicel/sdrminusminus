use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use num_complex::Complex;

const ZERO: Complex<f32> = Complex::new(0.0, 0.0);

const FARROW_CURVATURE: f32 = 0.5;

const TRACKING_RANGE: f64 = 0.05;

const FREE_RUN_MEMORY_SYMBOLS: f64 = 1024.0;

const SYMMETRY_TOLERANCE: f64 = 0.15;

const AMPLITUDE_SYMBOLS: f32 = 64.0;

const SETTLE_SYMBOLS: f32 = 2.0 * AMPLITUDE_SYMBOLS;

const HOLD_SYMBOLS: f32 = 4.0 * AMPLITUDE_SYMBOLS;

const TRUST_MISFIT: f32 = 0.05;

const DISTRUST_MISFIT: f32 = 0.065;

#[derive(Clone, Debug)]
struct Decisions {
    levels: Vec<f32>,
    outer: f32,
    outer_edge: f32,
    mean_square: f32,
    amplitude: f32,
    previous: f32,
    seen: f32,
    since_outer: f32,
    spacing: f32,
    misfit: f32,
    trusted: bool,
}

impl Decisions {
    fn new(levels: &[f32]) -> Self {
        assert!(!levels.is_empty(), "decision levels must not be empty");
        let outer = levels.iter().fold(0.0f32, |acc, l| acc.max(l.abs()));
        assert!(outer > 0.0, "decision levels must not all be zero");
        let inner = levels
            .iter()
            .map(|l| l.abs())
            .filter(|&l| l < outer)
            .fold(0.0f32, f32::max);
        let mean_square = levels.iter().map(|l| l * l).sum::<f32>() / levels.len() as f32;
        let spacing = levels
            .iter()
            .flat_map(|a| levels.iter().map(move |b| (a - b).abs()))
            .filter(|&d| d > 0.0)
            .fold(f32::INFINITY, f32::min);
        Self {
            levels: levels.to_vec(),
            outer,
            outer_edge: 0.5 * (outer + inner) / outer,
            mean_square,
            amplitude: 0.0,
            previous: 0.0,
            seen: 0.0,
            since_outer: 0.0,
            spacing: if spacing.is_finite() { spacing } else { outer },
            misfit: 1.0,
            trusted: false,
        }
    }

    fn trusted(&self) -> bool {
        self.trusted && self.seen >= SETTLE_SYMBOLS
    }

    fn judge(&mut self, normalised: f32, decision: f32) {
        let miss = (normalised - decision) / self.spacing;
        self.misfit += (miss * miss - self.misfit) / AMPLITUDE_SYMBOLS;
        self.trusted = if self.trusted {
            self.misfit < DISTRUST_MISFIT
        } else {
            self.misfit < TRUST_MISFIT
        };
    }

    fn nearest(&self, x: f32) -> f32 {
        self.levels
            .iter()
            .copied()
            .min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()))
            .unwrap_or(0.0)
    }

    fn track_amplitude(&mut self, magnitude: f32) {
        self.seen += 1.0;
        if self.seen <= 1.0 {
            self.amplitude = magnitude;
        } else if magnitude >= self.outer_edge * self.amplitude {
            self.amplitude += (magnitude - self.amplitude) / self.seen.min(AMPLITUDE_SYMBOLS);
            self.since_outer = 0.0;
        } else {
            self.since_outer += 1.0;
            if self.since_outer > HOLD_SYMBOLS {
                self.amplitude *= 1.0 - 1.0 / AMPLITUDE_SYMBOLS;
            }
        }
    }

    fn error(&mut self, symbol: f32, previous: f32) -> Option<f64> {
        self.track_amplitude(symbol.abs());
        let scale = self.amplitude / self.outer;
        if scale <= 0.0 || !scale.is_finite() {
            return None;
        }
        let decision = self.nearest(symbol / scale);
        self.judge(symbol / scale, decision);
        let raw = decision * previous - self.previous * symbol;
        self.previous = decision;
        let err = f64::from(raw) / (f64::from(scale * self.mean_square) * std::f64::consts::PI);
        err.is_finite().then_some(err)
    }

    fn reset(&mut self) {
        self.amplitude = 0.0;
        self.previous = 0.0;
        self.seen = 0.0;
        self.since_outer = 0.0;
        self.misfit = 1.0;
        self.trusted = false;
    }
}

pub struct SymbolSync {
    nominal_sps: f64,
    sps: f64,
    free_run_sps: f64,
    step: f64,
    alpha: f64,
    beta: f64,
    pos: usize,
    frac: f64,
    consumed: usize,
    buf: Vec<Complex<f32>>,
    at_symbol: bool,
    prev_symbol: Complex<f32>,
    mid: Complex<f32>,
    primed: bool,
    decisions: Option<Decisions>,
}

impl SymbolSync {
    #[must_use]
    pub fn new(sps: f64, loop_bw: f64) -> Self {
        assert!(
            sps.is_finite() && sps >= 2.0,
            "sps must be at least 2 samples per symbol"
        );
        assert!(
            loop_bw.is_finite() && loop_bw > 0.0 && loop_bw < 1.0,
            "loop_bw must be in (0, 1) cycles per symbol"
        );
        let denom = 1.0 + 2.0 * FRAC_1_SQRT_2 * loop_bw + loop_bw * loop_bw;
        let mut sync = Self {
            nominal_sps: sps,
            sps,
            free_run_sps: sps,
            step: sps,
            alpha: 4.0 * FRAC_1_SQRT_2 * loop_bw / denom,
            beta: 4.0 * loop_bw * loop_bw / denom,
            pos: 0,
            frac: 0.0,
            consumed: 0,
            buf: Vec::new(),
            at_symbol: true,
            prev_symbol: ZERO,
            mid: ZERO,
            primed: false,
            decisions: None,
        };
        sync.reset();
        sync
    }

    #[must_use]
    pub fn with_levels(mut self, levels: &[f32]) -> Self {
        self.decisions = Some(Decisions::new(levels));
        self
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.run(input, false, |y, at_symbol| {
            if at_symbol {
                out.push(y);
            }
            None
        });
    }

    pub fn process_held(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.run(input, true, |y, at_symbol| {
            if at_symbol {
                out.push(y);
            }
            None
        });
    }

    pub fn process_steered(
        &mut self,
        input: &[Complex<f32>],
        mut steer: impl FnMut(Complex<f32>) -> Option<f64>,
    ) {
        self.run(
            input,
            false,
            |y, at_symbol| {
                if at_symbol { steer(y) } else { None }
            },
        );
    }

    pub fn process_spaced(
        &mut self,
        input: &[Complex<f32>],
        steer: impl FnMut(Complex<f32>, bool) -> Option<f64>,
    ) {
        self.run(input, false, steer);
    }

    fn run(
        &mut self,
        input: &[Complex<f32>],
        hold: bool,
        mut on_sample: impl FnMut(Complex<f32>, bool) -> Option<f64>,
    ) {
        self.buf.extend_from_slice(input);
        while self.pos + 3 <= self.consumed + self.buf.len() {
            let base = self.pos - self.consumed;
            let y = farrow(&self.buf[base - 1..base + 3], self.frac as f32);
            if self.at_symbol {
                let steered = on_sample(y, true);
                if hold {
                    self.step = self.free_run_sps;
                } else if let Some(err) = steered.filter(|_| self.primed) {
                    self.steer(err);
                } else if self.primed {
                    self.retime(y);
                }
                self.prev_symbol = y;
                self.primed = true;
            } else {
                self.mid = y;
                let _ = on_sample(y, false);
            }
            self.at_symbol = !self.at_symbol;
            self.advance();
        }
        let drain = (self.pos - 1)
            .saturating_sub(self.consumed)
            .min(self.buf.len());
        self.buf.drain(..drain);
        self.consumed += drain;
    }

    #[must_use]
    pub fn sps(&self) -> f64 {
        self.sps
    }

    pub fn reset(&mut self) {
        self.sps = self.nominal_sps;
        self.free_run_sps = self.nominal_sps;
        self.step = self.nominal_sps;
        self.pos = 1;
        self.frac = 0.0;
        self.consumed = 0;
        self.buf.clear();
        self.at_symbol = true;
        self.prev_symbol = ZERO;
        self.mid = ZERO;
        self.primed = false;
        if let Some(decisions) = &mut self.decisions {
            decisions.reset();
        }
    }

    fn advance(&mut self) {
        self.frac += 0.5 * self.step;
        let whole = self.frac.floor();
        self.frac -= whole;
        self.pos += whole as usize;
    }

    fn retime(&mut self, symbol: Complex<f32>) {
        if let Some(decisions) = &mut self.decisions {
            let decided = decisions.error(symbol.re, self.prev_symbol.re);
            if decisions.trusted() {
                match decided {
                    Some(err) => self.steer(err),
                    None => self.step = self.free_run_sps,
                }
                return;
            }
        }
        let (before, after) = (self.prev_symbol.norm_sqr(), symbol.norm_sqr());
        if before <= 0.0 || after <= 0.0 {
            self.step = self.free_run_sps;
            return;
        }
        let sum = f64::from((symbol + self.prev_symbol).norm_sqr());
        let diff = f64::from((symbol - self.prev_symbol).norm_sqr());
        if sum > SYMMETRY_TOLERANCE * diff {
            self.step = self.sps;
            return;
        }
        let raw = ((symbol - self.prev_symbol) * self.mid.conj()).re;
        let err = f64::from(raw) / (f64::from(0.5 * (before + after)) * TAU);
        if !err.is_finite() {
            self.step = self.free_run_sps;
            return;
        }
        self.steer(err);
    }

    fn steer(&mut self, err: f64) {
        let err = err.clamp(-0.5, 0.5);
        self.sps = (self.sps - self.beta * err * self.nominal_sps).clamp(
            self.nominal_sps * (1.0 - TRACKING_RANGE),
            self.nominal_sps * (1.0 + TRACKING_RANGE),
        );
        self.free_run_sps += (self.sps - self.free_run_sps) / FREE_RUN_MEMORY_SYMBOLS;
        self.step = self.sps - self.alpha * err * self.nominal_sps;
    }
}

#[must_use]
pub fn farrow(w: &[Complex<f32>], mu: f32) -> Complex<f32> {
    let curvature = w[3] - w[2] - w[1] + w[0];
    w[1] + (w[2] - w[1]) * mu + curvature * (FARROW_CURVATURE * mu * (mu - 1.0))
}

const CROSSING_NUDGE: f64 = 0.125;

#[derive(Clone, Debug)]
pub struct BitSync {
    sample_rate: f64,
    increment: f64,
    phase: f64,
    positive: bool,
    primed: bool,
    since_symbol: usize,
}

impl BitSync {
    #[must_use]
    pub fn new(sample_rate: f64, baud: f64) -> Self {
        assert!(sample_rate > 0.0, "sample rate must be positive");
        let mut sync = Self {
            sample_rate,
            increment: 0.0,
            phase: 0.0,
            positive: false,
            primed: false,
            since_symbol: 0,
        };
        sync.set_baud(baud);
        sync.reset();
        sync
    }

    pub fn set_baud(&mut self, baud: f64) {
        assert!(
            baud.is_finite() && baud > 0.0 && baud * 2.0 <= self.sample_rate,
            "baud must be positive and at most half the sample rate"
        );
        self.increment = baud / self.sample_rate;
    }

    pub fn push(&mut self, sample: f32) -> Option<bool> {
        self.push_soft(sample).map(|v| v >= 0.0)
    }

    pub fn push_soft(&mut self, sample: f32) -> Option<f32> {
        let positive = sample >= 0.0;
        if self.primed && positive != self.positive {
            self.phase += CROSSING_NUDGE * (0.5 - self.phase);
        }
        self.positive = positive;
        self.primed = true;

        self.phase += self.increment;
        self.since_symbol += 1;
        if self.phase < 1.0 {
            return None;
        }
        self.phase -= 1.0;
        self.since_symbol = 0;
        Some(sample)
    }

    #[must_use]
    pub fn samples_since_symbol(&self) -> usize {
        self.since_symbol
    }

    pub fn reset(&mut self) {
        self.phase = 0.5;
        self.positive = false;
        self.primed = false;
        self.since_symbol = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;
    use crate::testutil::XorShift32;

    const ROLLOFF: f64 = 0.35;
    const SPAN: f64 = 6.0;

    fn sinc(t: f64) -> f64 {
        if t.abs() < 1e-12 {
            1.0
        } else {
            (PI * t).sin() / (PI * t)
        }
    }

    fn raised_cosine(t: f64) -> f64 {
        let denom = 1.0 - (2.0 * ROLLOFF * t).powi(2);
        if denom.abs() < 1e-8 {
            return std::f64::consts::FRAC_PI_4 * sinc(t);
        }
        sinc(t) * (PI * ROLLOFF * t).cos() / denom
    }

    fn symbols(count: usize, seed: u32) -> Vec<f32> {
        let mut rng = XorShift32(seed);
        (0..count)
            .map(|_| if rng.next_f32() >= 0.0 { 1.0 } else { -1.0 })
            .collect()
    }

    fn bpsk(syms: &[f32], sps: f64, offset: f64) -> Vec<Complex<f32>> {
        let len = ((syms.len() as f64 - SPAN) * sps) as usize;
        (0..len)
            .map(|n| {
                let t = n as f64 / sps - offset;
                let lo = (t - SPAN).ceil().max(0.0) as usize;
                let hi = ((t + SPAN) as usize).min(syms.len() - 1);
                let v: f64 = (lo..=hi)
                    .map(|k| f64::from(syms[k]) * raised_cosine(t - k as f64))
                    .sum();
                Complex::new(v as f32, 0.0)
            })
            .collect()
    }

    fn matches_at(syms: &[f32], out: &[Complex<f32>], settle: usize, d: usize) -> bool {
        let end = out.len().saturating_sub(4);
        settle < end
            && (settle..end).all(|j| j + d < syms.len() && (out[j].re > 0.0) == (syms[j + d] > 0.0))
    }

    fn alignment(syms: &[f32], out: &[Complex<f32>], settle: usize) -> Option<usize> {
        (0..8).find(|&d| matches_at(syms, out, settle, d))
    }

    #[test]
    fn recovers_bpsk_symbols_through_a_fractional_timing_offset() {
        let syms = symbols(600, 0x1234_5678);
        let signal = bpsk(&syms, 4.0, 0.37);
        let mut sync = SymbolSync::new(4.0, 0.02);
        let mut out = Vec::new();
        sync.process(&signal, &mut out);
        assert!(
            alignment(&syms, &out, 60).is_some(),
            "no offset aligns {} recovered symbols",
            out.len()
        );
    }

    fn four_level(count: usize, seed: u32) -> Vec<f32> {
        let mut rng = XorShift32(seed);
        (0..count)
            .map(|_| {
                let u = rng.next_f32();
                if u < -0.5 {
                    -3.0
                } else if u < 0.0 {
                    -1.0
                } else if u < 0.5 {
                    1.0
                } else {
                    3.0
                }
            })
            .collect()
    }

    fn residual(syms: &[f32], out: &[Complex<f32>], settle: usize) -> f32 {
        (0..8)
            .map(|d| {
                let n = out.len().saturating_sub(settle + 8);
                (settle..settle + n)
                    .map(|j| (out[j].re - syms[j + d]).powi(2))
                    .sum::<f32>()
                    / n as f32
            })
            .fold(f32::INFINITY, f32::min)
            .sqrt()
    }

    #[test]
    fn decisions_lock_four_levels_tighter_than_the_gardner_detector() {
        let syms = four_level(4_000, 0x4c4c_0001);
        for offset in [0.0, 0.3, 0.61] {
            let signal = bpsk(&syms, 8.0, offset);
            let run = |sync: SymbolSync| {
                let mut sync = sync;
                let mut out = Vec::new();
                sync.process(&signal, &mut out);
                residual(&syms, &out, 1_000)
            };
            let gardner = run(SymbolSync::new(8.0, 0.01));
            let decided = run(SymbolSync::new(8.0, 0.01).with_levels(&[-3.0, -1.0, 1.0, 3.0]));
            assert!(decided < 0.02, "offset {offset}: residual {decided}");
            assert!(
                decided < gardner,
                "offset {offset}: {decided} vs gardner {gardner}"
            );
        }
    }

    #[test]
    fn a_long_run_of_one_inner_level_keeps_the_decisions_scaled() {
        let mut syms = four_level(1_500, 0x4c4c_0002);
        syms.extend(std::iter::repeat_n(1.0, 150));
        syms.extend(four_level(1_500, 0x4c4c_0003));
        let signal = bpsk(&syms, 8.0, 0.4);
        let mut sync = SymbolSync::new(8.0, 0.01).with_levels(&[-3.0, -1.0, 1.0, 3.0]);
        let mut out = Vec::new();
        sync.process(&signal, &mut out);
        let tail = residual(&syms, &out, 1_700);
        assert!(tail < 0.02, "residual after the run {tail}");
    }

    #[test]
    fn ragged_blocks_match_one_shot_exactly() {
        let syms = symbols(400, 0x2bad_c0de);
        let signal = bpsk(&syms, 4.0, 0.21);

        let mut whole = SymbolSync::new(4.0, 0.02);
        let mut expected = Vec::new();
        whole.process(&signal, &mut expected);

        let mut ragged = SymbolSync::new(4.0, 0.02);
        let mut got = Vec::new();
        let mut pos = 0;
        for len in [1usize, 7, 64, 3, 129, 1024, 17].iter().cycle() {
            if pos >= signal.len() {
                break;
            }
            let end = (pos + len).min(signal.len());
            ragged.process(&signal[pos..end], &mut got);
            pos = end;
        }
        assert_eq!(expected, got);
    }

    #[test]
    fn tracks_a_half_percent_clock_error_without_slipping() {
        const TRUE_SPS: f64 = 4.02;
        let syms = symbols(3_000, 0x0f0f_1234);
        let signal = bpsk(&syms, TRUE_SPS, 0.1);
        let mut sync = SymbolSync::new(4.0, 0.02);
        let mut out = Vec::new();
        sync.process(&signal, &mut out);

        assert!(
            (sync.sps() - TRUE_SPS).abs() < 5e-3,
            "sps estimate {} did not converge on {TRUE_SPS}",
            sync.sps()
        );
        let ideal = ((signal.len() - 4) as f64 / TRUE_SPS).floor() as i64;
        assert!(
            (out.len() as i64 - ideal).abs() <= 2,
            "recovered {} symbols, ideal {ideal}",
            out.len()
        );
        assert!(
            alignment(&syms, &out, 300).is_some(),
            "symbol stream slipped while tracking"
        );
    }

    #[test]
    fn silence_free_runs_at_the_nominal_rate() {
        let mut sync = SymbolSync::new(4.0, 0.02);
        let mut out = Vec::new();
        sync.process(&vec![ZERO; 4_000], &mut out);
        assert_eq!(sync.sps(), 4.0, "silence must not pull the clock");
        assert!((out.len() as i64 - 999).abs() <= 2, "{} symbols", out.len());
    }

    const RATE: f64 = 48_000.0;
    const BAUD: f64 = 1_200.0;
    const SAMPLES_PER_BIT: usize = 40;

    fn nrz(bits: &[bool]) -> Vec<f32> {
        bits.iter()
            .flat_map(|&b| std::iter::repeat_n(if b { 1.0 } else { -1.0 }, SAMPLES_PER_BIT))
            .collect()
    }

    fn bit_pattern(count: usize, seed: u32) -> Vec<bool> {
        let mut rng = XorShift32(seed);
        (0..count).map(|_| rng.next_f32() >= 0.0).collect()
    }

    fn recover(samples: &[f32]) -> Vec<bool> {
        let mut sync = BitSync::new(RATE, BAUD);
        samples.iter().filter_map(|&s| sync.push(s)).collect()
    }

    fn bits_align(tx: &[bool], rx: &[bool], settle: usize) -> bool {
        (0..3).any(|d| {
            let end = rx.len().saturating_sub(1);
            settle < end && (settle..end).all(|j| j + d < tx.len() && rx[j] == tx[j + d])
        })
    }

    #[test]
    fn soft_slices_agree_with_hard_ones_instant_for_instant() {
        let samples = nrz(&bit_pattern(200, 0x0bad_c0de));
        let mut hard = BitSync::new(RATE, BAUD);
        let mut soft = BitSync::new(RATE, BAUD);
        for &s in &samples {
            assert_eq!(hard.push(s), soft.push_soft(s).map(|v| v >= 0.0));
        }
    }

    #[test]
    fn recovers_a_bit_pattern_locked_to_the_waveform() {
        let bits = bit_pattern(500, 0x5eed_1234);
        let rx = recover(&nrz(&bits));
        assert_eq!(rx.len(), 500, "one slice per bit expected");
        assert_eq!(rx, bits);
    }

    #[test]
    fn locks_from_a_half_bit_phase_offset_and_stays_locked() {
        let bits = bit_pattern(500, 0xfeed_beef);
        let waveform = nrz(&bits);
        let rx = recover(&waveform[SAMPLES_PER_BIT / 2..]);
        assert!(
            (rx.len() as i64 - 500).abs() <= 2,
            "recovered {} bits from 500",
            rx.len()
        );
        assert!(bits_align(&bits, &rx, 64), "never locked to the bit clock");
    }

    #[test]
    fn free_runs_on_a_crossing_free_input() {
        let mut sync = BitSync::new(RATE, BAUD);
        let (mut count, mut since) = (0, 0);
        for _ in 0..400 {
            since += 1;
            if sync.push(1.0).is_some() {
                count += 1;
                since = 0;
            }
            assert_eq!(sync.samples_since_symbol(), since);
        }
        assert_eq!(count, 10, "constant input must keep the clock running");
    }

    #[test]
    fn set_baud_keeps_the_current_phase() {
        let mut sync = BitSync::new(RATE, BAUD);
        while sync.push(1.0).is_none() {}
        sync.set_baud(2.0 * BAUD);
        let mut gap = 0;
        while sync.push(1.0).is_none() {
            gap += 1;
        }
        assert_eq!(gap + 1, SAMPLES_PER_BIT / 2);
    }
}
