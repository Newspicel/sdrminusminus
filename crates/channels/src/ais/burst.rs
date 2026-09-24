use std::f32::consts::FRAC_PI_2;

use num_complex::Complex;
use sdrmm_dsp::{HdlcDeframer, hdlc_fcs_ok};
use sdrmm_modem::pulse::{self, Norm};

use super::{BT, MAX_FRAME_BYTES, MIN_FRAME_BYTES};

pub(super) const SPS: usize = 5;
const PHASES: usize = 8;
const PULSE_SYMBOLS: usize = 3;
const PULSE_OVERSAMPLE: usize = 64;
const TRAINING_BITS: usize = 16;
const FLAG: [bool; 8] = [false, true, true, true, true, true, true, false];
const TEMPLATE_BITS: usize = TRAINING_BITS + FLAG.len();
const TEMPLATE_LEN: usize = TEMPLATE_BITS * SPS;
const MAX_BITS: usize = MAX_FRAME_BYTES * 8 * 6 / 5 + 2 * FLAG.len();
const LAG: usize = SPS;
const DETECT_THRESHOLD: f32 = 0.5;
const PEAK_HOLD: usize = 8 * SPS;
const TIMING_SPAN: isize = 2;
const STATES: usize = 16;
const PHASE_GAIN: f32 = 0.1;
const HISTORY: usize = 4 * (TEMPLATE_BITS + MAX_BITS) * SPS;
const TRACE_STRIDE: usize = 8;
const LOOKAHEAD_BITS: usize = 16;
const FIT_BITS: usize = 180;
const FIT_BLOCK_BITS: usize = 12;

type Branches = [[Complex<f32>; SPS]; 8];

#[derive(Clone, Copy)]
struct Start {
    quadrant: usize,
    prev: bool,
    cur: bool,
}

impl Start {
    fn state(self) -> usize {
        (self.quadrant << 2) | (usize::from(self.prev) << 1) | usize::from(self.cur)
    }
}

type Template = [Complex<f32>; TEMPLATE_LEN];

struct Polarity {
    templates: [Template; PHASES],
    lagged: [Complex<f32>; TEMPLATE_LEN - LAG],
    start: Start,
}

struct Shape {
    branches: [Branches; PHASES],
    polarities: [Polarity; 2],
}

fn phase_pulse() -> Vec<f32> {
    let freq = pulse::gaussian_freq(PULSE_OVERSAMPLE as f64, BT, PULSE_SYMBOLS, Norm::Area);
    let total: f32 = freq.iter().sum();
    let mut acc = 0.0;
    freq.iter()
        .map(|g| {
            acc += g / total;
            acc
        })
        .collect()
}

fn q(pulse: &[f32], u: f32) -> f32 {
    let x = (u + 1.0) * PULSE_OVERSAMPLE as f32 - 0.5;
    if x <= 0.0 {
        return 0.0;
    }
    let i = x.floor() as usize;
    match (pulse.get(i), pulse.get(i + 1)) {
        (Some(&a), Some(&b)) => a + (b - a) * (x - i as f32),
        _ => 1.0,
    }
}

fn level(bit: bool) -> f32 {
    if bit { 1.0 } else { -1.0 }
}

fn in_flight(pulse: &[f32], prev: bool, cur: bool, next: bool, t: f32) -> f32 {
    level(prev) * q(pulse, t + 1.0) + level(cur) * q(pulse, t) + level(next) * q(pulse, t - 1.0)
}

fn sample_time(j: usize, phase: usize) -> f32 {
    (j as f32 + phase as f32 / PHASES as f32) / SPS as f32
}

fn branches(pulse: &[f32], phase: usize) -> Branches {
    let mut out = [[Complex::new(0.0, 0.0); SPS]; 8];
    for (index, wave) in out.iter_mut().enumerate() {
        let (prev, cur, next) = (index & 4 != 0, index & 2 != 0, index & 1 != 0);
        for (j, s) in wave.iter_mut().enumerate() {
            let phi = FRAC_PI_2 * in_flight(pulse, prev, cur, next, sample_time(j, phase));
            *s = Complex::from_polar(1.0, phi);
        }
    }
    out
}

fn template_levels() -> [bool; TEMPLATE_BITS] {
    let mut bits = [false; TEMPLATE_BITS];
    for (k, b) in bits.iter_mut().enumerate().take(TRAINING_BITS) {
        *b = k % 2 == 1;
    }
    bits[TRAINING_BITS..].copy_from_slice(&FLAG);
    let mut levels = [true; TEMPLATE_BITS];
    let mut current = true;
    for (l, &b) in levels.iter_mut().zip(&bits) {
        if !b {
            current = !current;
        }
        *l = current;
    }
    levels
}

fn quadrant_step(quadrant: usize, bit: bool) -> usize {
    (quadrant + if bit { 1 } else { 3 }) % 4
}

fn template(pulse: &[f32], levels: &[bool], phase: usize) -> ([Complex<f32>; TEMPLATE_LEN], Start) {
    let at = |k: isize| levels[k.clamp(0, levels.len() as isize - 1) as usize];
    let mut out = [Complex::new(0.0, 0.0); TEMPLATE_LEN];
    let mut quadrant = 0usize;
    let mut start = Start {
        quadrant: 0,
        prev: true,
        cur: true,
    };
    for k in 0..levels.len() as isize {
        let (prev, cur, next) = (at(k - 1), at(k), at(k + 1));
        start = Start {
            quadrant,
            prev,
            cur,
        };
        for j in 0..SPS {
            let phi = FRAC_PI_2
                * (quadrant as f32 + in_flight(pulse, prev, cur, next, sample_time(j, phase)));
            out[k as usize * SPS + j] = Complex::from_polar(1.0, phi);
        }
        quadrant = quadrant_step(quadrant, prev);
    }
    (out, start)
}

impl Polarity {
    fn new(pulse: &[f32], levels: &[bool]) -> Self {
        let mut templates = [[Complex::new(0.0, 0.0); TEMPLATE_LEN]; PHASES];
        let mut start = Start {
            quadrant: 0,
            prev: true,
            cur: true,
        };
        for (phase, t) in templates.iter_mut().enumerate() {
            (*t, start) = template(pulse, levels, phase);
        }
        let mut lagged = [Complex::new(0.0, 0.0); TEMPLATE_LEN - LAG];
        for (n, d) in lagged.iter_mut().enumerate() {
            *d = templates[0][n + LAG] * templates[0][n].conj();
        }
        Self {
            templates,
            lagged,
            start,
        }
    }
}

impl Shape {
    fn new() -> Self {
        let pulse = phase_pulse();
        let upright = template_levels();
        let inverted = upright.map(|l| !l);
        Self {
            branches: std::array::from_fn(|phase| branches(&pulse, phase)),
            polarities: [
                Polarity::new(&pulse, &upright),
                Polarity::new(&pulse, &inverted),
            ],
        }
    }
}

#[derive(Clone, Copy)]
struct Anchor {
    at: usize,
    polarity: usize,
    phase: usize,
    step: Complex<f32>,
    carrier: Complex<f32>,
}

#[derive(Clone, Copy)]
struct Survivor {
    metric: f32,
    carrier: Complex<f32>,
}

const DEAD: Survivor = Survivor {
    metric: f32::NEG_INFINITY,
    carrier: Complex { re: 1.0, im: 0.0 },
};

#[derive(Clone, Copy)]
struct Peak {
    at: usize,
    metric: f32,
    polarity: usize,
}

impl Peak {
    fn shifted(self, by: usize) -> Self {
        Self {
            at: self.at - by,
            ..self
        }
    }
}

pub(super) struct Decoded {
    pub frame: Vec<u8>,
    pub end: u64,
}

pub(super) struct BurstReceiver {
    shape: Shape,
    samples: Vec<Complex<f32>>,
    products: Vec<Complex<f32>>,
    energies: Vec<f32>,
    base: u64,
    next: usize,
    peak: Option<Peak>,
    pending: Option<Run>,
    back: Vec<[u8; STATES]>,
    levels: Vec<bool>,
    deframer: HdlcDeframer,
}

impl BurstReceiver {
    pub(super) fn new() -> Self {
        Self {
            shape: Shape::new(),
            samples: Vec::with_capacity(3 * HISTORY),
            products: Vec::with_capacity(3 * HISTORY),
            energies: Vec::with_capacity(3 * HISTORY),
            base: 0,
            next: 0,
            peak: None,
            pending: None,
            back: Vec::with_capacity(MAX_BITS + 1),
            levels: Vec::with_capacity(MAX_BITS + 1),
            deframer: HdlcDeframer::new(MIN_FRAME_BYTES, MAX_FRAME_BYTES),
        }
    }

    pub(super) fn process(&mut self, iq: &[Complex<f32>], out: &mut Vec<Decoded>) {
        for chunk in iq.chunks(HISTORY) {
            self.extend(chunk);
            self.scan(out);
            self.trim();
        }
    }

    fn extend(&mut self, iq: &[Complex<f32>]) {
        for &s in iq {
            let lagged = self
                .samples
                .len()
                .checked_sub(LAG)
                .map_or(Complex::new(0.0, 0.0), |i| self.samples[i]);
            self.samples.push(s);
            let product = s * lagged.conj();
            self.products.push(product);
            self.energies.push(product.norm_sqr());
        }
    }

    fn trim(&mut self) {
        let oldest = self
            .pending
            .as_ref()
            .map_or(self.next, |run| run.anchor.at.min(self.next));
        let keep_from = oldest.saturating_sub(TEMPLATE_LEN + SPS * 4);
        if keep_from > HISTORY / 2 {
            self.samples.drain(..keep_from);
            self.products.drain(..keep_from);
            self.energies.drain(..keep_from);
            self.base += keep_from as u64;
            self.next -= keep_from;
            self.peak = self.peak.map(|p| p.shifted(keep_from));
            if let Some(run) = &mut self.pending {
                run.anchor.at -= keep_from;
            }
        }
    }

    fn scan(&mut self, out: &mut Vec<Decoded>) {
        loop {
            if self.pending.is_some() {
                let Progress::Finished(decoded) = self.advance() else {
                    return;
                };
                if let Some(decoded) = decoded {
                    self.next = self.next.max((decoded.end - self.base) as usize);
                    out.push(decoded);
                }
            }
            if self.next + TEMPLATE_LEN + SPS * (TIMING_SPAN as usize + 1) > self.samples.len() {
                return;
            }
            self.search();
        }
    }

    fn search(&mut self) {
        let at = self.next;
        let (metric, polarity) = self.detect(at);
        if metric > DETECT_THRESHOLD && self.peak.is_none_or(|p| metric > p.metric) {
            self.peak = Some(Peak {
                at,
                metric,
                polarity,
            });
        }
        self.next += 1;
        if let Some(peak) = self.peak
            && at - peak.at >= PEAK_HOLD
        {
            self.peak = None;
            self.pending = self.anchor(peak).map(|anchor| self.begin(anchor));
        }
    }

    fn begin(&self, anchor: Anchor) -> Run {
        let mut survivors = [DEAD; STATES];
        survivors[self.shape.polarities[anchor.polarity].start.state()] = Survivor {
            metric: 0.0,
            carrier: Complex::new(1.0, 0.0),
        };
        Run {
            anchor,
            survivors,
            rot: anchor.carrier * rotation_after(anchor.step, (TEMPLATE_BITS - 1) * SPS),
            bits: 0,
            traced: 0,
        }
    }

    fn available_bits(&self, anchor: &Anchor) -> usize {
        let first = anchor.at + (TEMPLATE_BITS - 1) * SPS;
        (self.samples.len().saturating_sub(first) / SPS).min(MAX_BITS)
    }

    fn lagged_correlation(&self, at: usize) -> (Complex<f32>, Complex<f32>) {
        let products = &self.products[at + LAG..at + TEMPLATE_LEN];
        let mut upright = Complex::new(0.0, 0.0);
        let mut inverted = Complex::new(0.0, 0.0);
        for (p, t) in products.iter().zip(&self.shape.polarities[0].lagged) {
            upright += p * t.conj();
            inverted += p * t;
        }
        (upright, inverted)
    }

    fn detect(&self, at: usize) -> (f32, usize) {
        let energy: f32 = self.energies[at + LAG..at + TEMPLATE_LEN].iter().sum();
        if energy <= f32::MIN_POSITIVE {
            return (0.0, 0);
        }
        let energy = (energy * (TEMPLATE_LEN - LAG) as f32).sqrt();
        let (upright, inverted) = self.lagged_correlation(at);
        if upright.norm_sqr() >= inverted.norm_sqr() {
            (upright.norm() / energy, 0)
        } else {
            (inverted.norm() / energy, 1)
        }
    }

    fn coarse_step(&self, at: usize, polarity: usize) -> Complex<f32> {
        let (upright, inverted) = self.lagged_correlation(at);
        let corr = if polarity == 0 { upright } else { inverted };
        Complex::from_polar(1.0, corr.arg() / LAG as f32)
    }

    fn correlate(
        &self,
        at: usize,
        template: &Template,
        step: Complex<f32>,
    ) -> (Complex<f32>, Complex<f32>) {
        let mut rot = Complex::new(1.0, 0.0);
        let mut halves = [Complex::new(0.0, 0.0); 2];
        for (n, (s, t)) in self.samples[at..at + TEMPLATE_LEN]
            .iter()
            .zip(template)
            .enumerate()
        {
            halves[n * 2 / TEMPLATE_LEN] += s * rot.conj() * t.conj();
            rot *= step;
        }
        (halves[0], halves[1])
    }

    fn anchor(&self, peak: Peak) -> Option<Anchor> {
        let templates = &self.shape.polarities[peak.polarity].templates;
        let coarse = self.coarse_step(peak.at, peak.polarity);
        let mut best: Option<(f32, usize, usize)> = None;
        for offset in -TIMING_SPAN..=TIMING_SPAN {
            let Some(at) = peak.at.checked_add_signed(offset) else {
                continue;
            };
            for (phase, template) in templates.iter().enumerate() {
                let (a, b) = self.correlate(at, template, coarse);
                let power = (a + b).norm();
                if best.is_none_or(|(p, _, _)| power > p) {
                    best = Some((power, at, phase));
                }
            }
        }
        let (_, at, phase) = best?;
        let (a, b) = self.correlate(at, &templates[phase], coarse);
        let fine = Complex::from_polar(1.0, (b * a.conj()).arg() / (TEMPLATE_LEN / 2) as f32);
        let step = coarse * fine;
        let (a, b) = self.correlate(at, &templates[phase], step);
        let carrier = (a + b) / (a + b).norm();
        Some(Anchor {
            at,
            polarity: peak.polarity,
            phase,
            step,
            carrier,
        })
    }

    fn advance(&mut self) -> Progress {
        let Some(mut run) = self.pending.take() else {
            return Progress::Finished(None);
        };
        let target = self.available_bits(&run.anchor);
        if target < run.traced + TRACE_STRIDE && target < MAX_BITS {
            self.pending = Some(run);
            return Progress::Waiting;
        }
        self.back.truncate(run.bits);
        self.extend_trellis(&mut run, target, PHASE_GAIN);
        run.traced = run.bits;
        self.traceback(&run.survivors);
        let complete = run.bits == MAX_BITS;
        let settled = |close: usize| complete || close + LOOKAHEAD_BITS <= run.bits;
        let outcome = self.deframe(&run.anchor);
        match outcome {
            Outcome::Frame(decoded, close) if settled(close) => Progress::Finished(Some(decoded)),
            Outcome::Corrupt(close) if settled(close) => {
                Progress::Finished(self.retry(&run.anchor, close, run.bits))
            }
            Outcome::Open if complete => {
                Progress::Finished(self.retry(&run.anchor, FIT_BITS, run.bits))
            }
            _ => {
                self.pending = Some(run);
                Progress::Waiting
            }
        }
    }

    fn extend_trellis(&mut self, run: &mut Run, target: usize, gain: f32) {
        let branches = &self.shape.branches[run.anchor.phase];
        for k in run.bits..target {
            let at = run.anchor.at + (TEMPLATE_BITS - 1 + k) * SPS;
            let rx = derotate(&self.samples[at..], &mut run.rot, run.anchor.step);
            let (next, back) = step_trellis(&run.survivors, &matched(&rx, branches), gain);
            run.survivors = next;
            self.back.push(back);
        }
        run.bits = target;
    }

    fn retry(&mut self, anchor: &Anchor, fit_bits: usize, span: usize) -> Option<Decoded> {
        let refined = self.refine(anchor, fit_bits);
        for (anchor, gain) in [
            (&refined, PHASE_GAIN),
            (&refined, 0.0),
            (anchor, 0.0),
            (anchor, 2.5 * PHASE_GAIN),
        ] {
            if let Outcome::Frame(decoded, _) = self.attempt(anchor, gain, span) {
                return Some(decoded);
            }
        }
        None
    }

    fn attempt(&mut self, anchor: &Anchor, gain: f32, bits: usize) -> Outcome {
        self.viterbi(anchor, gain, bits);
        self.deframe(anchor)
    }

    fn viterbi(&mut self, anchor: &Anchor, gain: f32, bits: usize) {
        let mut run = self.begin(*anchor);
        self.back.clear();
        self.extend_trellis(&mut run, bits, gain);
        self.traceback(&run.survivors);
    }

    fn refine(&self, anchor: &Anchor, bits: usize) -> Anchor {
        let branches = &self.shape.branches[anchor.phase];
        let first = TEMPLATE_BITS - 1;
        let mut rot = anchor.carrier * rotation_after(anchor.step, first * SPS);
        let start = self.shape.polarities[anchor.polarity].start;
        let mut quadrant = start.quadrant;
        let mut prev = start.prev;
        let mut fit = PhaseFit::default();
        let mut block = Complex::new(0.0, 0.0);
        let bits = bits.min(self.levels.len().saturating_sub(1));
        for k in 0..bits {
            let (cur, next) = (self.levels[k], self.levels[k + 1]);
            let rx = derotate(
                &self.samples[anchor.at + (first + k) * SPS..],
                &mut rot,
                anchor.step,
            );
            let wave = &branches[branch_index(prev, cur, next)];
            let c: Complex<f32> = rx.iter().zip(wave).map(|(r, w)| r * w.conj()).sum();
            block += c * QUADRANTS[quadrant];
            if (k + 1) % FIT_BLOCK_BITS == 0 {
                let centre = ((first + k + 1) * SPS) as f32 - (FIT_BLOCK_BITS * SPS) as f32 / 2.0;
                fit.add(centre, block);
                block = Complex::new(0.0, 0.0);
            }
            quadrant = quadrant_step(quadrant, prev);
            prev = cur;
        }
        let Some((offset, slope)) = fit.solve() else {
            return *anchor;
        };
        Anchor {
            step: anchor.step * Complex::from_polar(1.0, slope),
            carrier: anchor.carrier * Complex::from_polar(1.0, offset),
            ..*anchor
        }
    }

    fn traceback(&mut self, survivors: &[Survivor; STATES]) {
        let mut state = (0..STATES)
            .max_by(|&a, &b| survivors[a].metric.total_cmp(&survivors[b].metric))
            .unwrap_or(0);
        self.levels.clear();
        for back in self.back.iter().rev() {
            self.levels.push(state & 1 == 1);
            state = usize::from(back[state]);
        }
        self.levels.push(state & 1 == 1);
        self.levels.reverse();
    }

    fn deframe(&mut self, anchor: &Anchor) -> Outcome {
        self.deframer.reset();
        for &b in &FLAG {
            self.deframer.push(b);
        }
        let mut ones = 0;
        for (k, pair) in self.levels.windows(2).enumerate() {
            let bit = pair[0] == pair[1];
            ones = if bit { ones + 1 } else { 0 };
            if ones > FLAG.len() - 2 {
                return Outcome::Corrupt(k + 1);
            }
            let Some(frame) = self.deframer.push(bit) else {
                continue;
            };
            if !hdlc_fcs_ok(&frame) {
                return Outcome::Corrupt(k + 1);
            }
            let end = anchor.at + (TEMPLATE_BITS + k + 1) * SPS;
            return Outcome::Frame(
                Decoded {
                    frame,
                    end: self.base + end as u64,
                },
                k + 1,
            );
        }
        Outcome::Open
    }
}

enum Progress {
    Waiting,
    Finished(Option<Decoded>),
}

struct Run {
    anchor: Anchor,
    survivors: [Survivor; STATES],
    rot: Complex<f32>,
    bits: usize,
    traced: usize,
}

enum Outcome {
    Frame(Decoded, usize),
    Corrupt(usize),
    Open,
}

#[derive(Default)]
struct PhaseFit {
    last: Option<f32>,
    unwrapped: f32,
    w: f32,
    wt: f32,
    wtt: f32,
    wp: f32,
    wtp: f32,
}

impl PhaseFit {
    fn add(&mut self, t: f32, c: Complex<f32>) {
        let phase = c.arg();
        self.unwrapped = match self.last {
            None => phase,
            Some(last) => self.unwrapped + Complex::from_polar(1.0, phase - last).arg(),
        };
        self.last = Some(phase);
        let w = c.norm();
        self.w += w;
        self.wt += w * t;
        self.wtt += w * t * t;
        self.wp += w * self.unwrapped;
        self.wtp += w * t * self.unwrapped;
    }

    fn solve(&self) -> Option<(f32, f32)> {
        let det = self.w * self.wtt - self.wt * self.wt;
        if det.abs() <= f32::EPSILON * self.w * self.wtt {
            return None;
        }
        let slope = (self.w * self.wtp - self.wt * self.wp) / det;
        let offset = (self.wp - slope * self.wt) / self.w;
        Some((offset, slope))
    }
}

fn derotate(
    samples: &[Complex<f32>],
    rot: &mut Complex<f32>,
    step: Complex<f32>,
) -> [Complex<f32>; SPS] {
    let mut rx = [Complex::new(0.0, 0.0); SPS];
    for (r, &s) in rx.iter_mut().zip(samples) {
        *r = s * rot.conj();
        *rot *= step;
    }
    rx
}

fn matched(rx: &[Complex<f32>; SPS], branches: &Branches) -> [Complex<f32>; 8] {
    let mut out = [Complex::new(0.0, 0.0); 8];
    for (m, wave) in out.iter_mut().zip(branches) {
        *m = rx.iter().zip(wave).map(|(r, w)| r * w.conj()).sum();
    }
    out
}

fn branch_index(prev: bool, cur: bool, next: bool) -> usize {
    (usize::from(prev) << 2) | (usize::from(cur) << 1) | usize::from(next)
}

fn rotation_after(step: Complex<f32>, n: usize) -> Complex<f32> {
    let mut acc = Complex::new(1.0, 0.0);
    for _ in 0..n {
        acc *= step;
    }
    acc
}

const QUADRANTS: [Complex<f32>; 4] = [
    Complex { re: 1.0, im: 0.0 },
    Complex { re: 0.0, im: -1.0 },
    Complex { re: -1.0, im: 0.0 },
    Complex { re: 0.0, im: 1.0 },
];

fn step_trellis(
    survivors: &[Survivor; STATES],
    matched: &[Complex<f32>; 8],
    gain: f32,
) -> ([Survivor; STATES], [u8; STATES]) {
    let mut next = [DEAD; STATES];
    let mut back = [0u8; STATES];
    for (state, s) in survivors.iter().enumerate() {
        if s.metric == f32::NEG_INFINITY {
            continue;
        }
        let quadrant = state >> 2;
        let prev = (state >> 1) & 1;
        let cur = state & 1;
        let rot = QUADRANTS[quadrant] * s.carrier.conj();
        let to_quadrant = quadrant_step(quadrant, prev == 1);
        for bit in 0..2 {
            let c = matched[(prev << 2) | (cur << 1) | bit] * rot;
            let metric = s.metric + c.re;
            let target = (to_quadrant << 2) | (cur << 1) | bit;
            if metric > next[target].metric {
                let nudge = Complex::from_polar(1.0, gain * c.arg());
                next[target] = Survivor {
                    metric,
                    carrier: s.carrier * nudge,
                };
                back[target] = state as u8;
            }
        }
    }
    (next, back)
}
