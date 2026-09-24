use std::fmt;

use num_complex::Complex;

use super::carrier::{CarrierLoop, PhaseDetector};
use crate::constellation::Constellation;

const ZERO: Complex<f32> = Complex::new(0.0, 0.0);

const ONE: Complex<f32> = Complex::new(1.0, 0.0);

const POWER_MEMORY_SAMPLES: f32 = 8_192.0;

const MISFIT_SYMBOLS: f32 = 128.0;

const TRUST_MISFIT: f32 = 0.1;

const DISTRUST_MISFIT: f32 = 0.13;

const OUTPUT_MEMORY_SYMBOLS: f32 = 256.0;

const MAX_OUTPUT_POWER: f32 = 6.0;

const MIN_OUTPUT_POWER: f32 = 0.05;

const LEAKAGE: f32 = 1e-5;

const STEP_DECAY: f32 = 0.98;

const STEP_GROWTH: f32 = 2.0;

const STEP_MEMORY: f32 = 0.99;

const MIN_STEP_FRACTION: f32 = 1.0 / 16.0;

const ENERGY_FLOOR: f32 = 1e-6;

const LEVEL_TOLERANCE: f32 = 1e-3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqualiserConfig {
    pub span_symbols: usize,
    pub feedback_symbols: usize,
    pub blind_step: f32,
    pub decision_step: f32,
    pub burst_passes: usize,
}

impl EqualiserConfig {
    pub const DEFAULT: Self = Self {
        span_symbols: 8,
        feedback_symbols: 0,
        blind_step: 0.2,
        decision_step: 0.05,
        burst_passes: 3,
    };

    #[must_use]
    pub const fn with_feedback(mut self, symbols: usize) -> Self {
        self.feedback_symbols = symbols;
        self
    }

    #[must_use]
    pub const fn with_span(mut self, symbols: usize) -> Self {
        self.span_symbols = symbols;
        self
    }

    fn validate(&self) -> Result<(), EqualiserError> {
        if self.span_symbols == 0 {
            return Err(EqualiserError::NoSpan);
        }
        if self.burst_passes == 0 {
            return Err(EqualiserError::NoPasses);
        }
        for step in [self.blind_step, self.decision_step] {
            if !(step > 0.0 && step < 1.0) {
                return Err(EqualiserError::StepOutOfRange(step));
            }
        }
        Ok(())
    }
}

impl Default for EqualiserConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EqualiserError {
    NoSpan,
    NoPasses,
    StepOutOfRange(f32),
}

impl fmt::Display for EqualiserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSpan => write!(f, "equaliser needs a span of at least one symbol"),
            Self::NoPasses => write!(f, "burst equalisation needs at least one pass"),
            Self::StepOutOfRange(step) => write!(f, "equaliser step {step} is outside (0, 1)"),
        }
    }
}

impl std::error::Error for EqualiserError {}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BlindTarget {
    Modulus(f32),
    Axes { re: f32, im: f32 },
}

impl BlindTarget {
    fn for_table(table: &Constellation) -> Self {
        let points = table.points();
        let moment = |part: fn(&Complex<f32>) -> f32, low: i32, high: i32| {
            let below: f32 = points.iter().map(|p| part(p).abs().powi(low)).sum();
            let above: f32 = points.iter().map(|p| part(p).abs().powi(high)).sum();
            if below > ENERGY_FLOOR {
                above / below
            } else {
                0.0
            }
        };
        if is_separable(points) {
            Self::Axes {
                re: moment(|p| p.re, 1, 2),
                im: moment(|p| p.im, 1, 2),
            }
        } else {
            Self::Modulus(moment(|p| p.norm(), 2, 4))
        }
    }

    fn error(self, z: Complex<f32>, decision: Complex<f32>) -> Complex<f32> {
        match self {
            Self::Modulus(r2) => z * (r2 - z.norm_sqr()),
            Self::Axes { re, im } => {
                let directed = decision - z;
                let sato = Complex::new(re * z.re.signum() - z.re, im * z.im.signum() - z.im);
                Complex::new(
                    agreeing(directed.re, sato.re),
                    agreeing(directed.im, sato.im),
                )
            }
        }
    }
}

fn agreeing(directed: f32, blind: f32) -> f32 {
    if directed * blind > 0.0 {
        directed
    } else {
        0.0
    }
}

fn is_separable(points: &[Complex<f32>]) -> bool {
    let levels = |part: fn(&Complex<f32>) -> f32| {
        let mut found: Vec<f32> = Vec::new();
        for p in points {
            let v = part(p);
            if found.iter().all(|&f| (f - v).abs() > LEVEL_TOLERANCE) {
                found.push(v);
            }
        }
        found.len()
    };
    levels(|p| p.re) * levels(|p| p.im) == points.len()
}

#[derive(Clone, Debug)]
struct DelayLine {
    samples: Vec<Complex<f32>>,
    head: usize,
    len: usize,
}

impl DelayLine {
    fn new(len: usize) -> Self {
        Self {
            samples: vec![ZERO; 2 * len],
            head: 0,
            len,
        }
    }

    fn push(&mut self, x: Complex<f32>) {
        if self.len == 0 {
            return;
        }
        self.head = (self.head + self.len - 1) % self.len;
        self.samples[self.head] = x;
        self.samples[self.head + self.len] = x;
    }

    fn window(&self) -> &[Complex<f32>] {
        &self.samples[self.head..self.head + self.len]
    }

    fn clear(&mut self) {
        self.samples.fill(ZERO);
        self.head = 0;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct StepControl {
    step: Option<f32>,
    correlation: Complex<f32>,
    last: Complex<f32>,
}

impl StepControl {
    fn next(&mut self, error: Complex<f32>, max: f32) -> f32 {
        self.correlation =
            self.correlation * STEP_MEMORY + error * self.last.conj() * (1.0 - STEP_MEMORY);
        self.last = error;
        let grown =
            STEP_DECAY * self.step.unwrap_or(max) + STEP_GROWTH * self.correlation.norm_sqr();
        let step = grown.clamp(max * MIN_STEP_FRACTION, max);
        self.step = Some(step);
        step
    }
}

#[derive(Clone, Debug)]
pub struct Equaliser {
    config: EqualiserConfig,
    blind: BlindTarget,
    blind_detector: PhaseDetector,
    spacing2: f32,
    forward: Vec<Complex<f32>>,
    feedback: Vec<Complex<f32>>,
    line: DelayLine,
    decisions: DelayLine,
    centre: usize,
    power: f32,
    seen: f32,
    misfit: f32,
    output_power: f32,
    trusted: bool,
    step: StepControl,
    resets: u64,
    slips: u64,
}

impl Equaliser {
    pub fn new(config: EqualiserConfig, table: &Constellation) -> Result<Self, EqualiserError> {
        config.validate()?;
        let taps = 2 * config.span_symbols;
        let spacing = table.min_distance().max(f64::from(ENERGY_FLOOR)) as f32;
        let mut eq = Self {
            config,
            blind: BlindTarget::for_table(table),
            blind_detector: PhaseDetector::MthPower {
                m: table.rotational_order().max(2),
            },
            spacing2: spacing * spacing,
            forward: vec![ZERO; taps],
            feedback: vec![ZERO; config.feedback_symbols],
            line: DelayLine::new(taps),
            decisions: DelayLine::new(config.feedback_symbols),
            centre: 2 * (config.span_symbols / 2),
            power: 0.0,
            seen: 0.0,
            misfit: 1.0,
            output_power: 1.0,
            trusted: false,
            step: StepControl::default(),
            resets: 0,
            slips: 0,
        };
        eq.reset();
        Ok(eq)
    }

    #[must_use]
    pub fn config(&self) -> &EqualiserConfig {
        &self.config
    }

    #[must_use]
    pub fn delay_symbols(&self) -> usize {
        self.centre / 2
    }

    #[must_use]
    pub fn decision_directed(&self) -> bool {
        self.trusted
    }

    #[must_use]
    pub fn resets(&self) -> u64 {
        self.resets
    }

    #[must_use]
    pub fn slips(&self) -> u64 {
        self.slips
    }

    #[must_use]
    pub fn taps(&self) -> &[Complex<f32>] {
        &self.forward
    }

    pub fn push(&mut self, x: Complex<f32>) {
        let x = if x.re.is_finite() && x.im.is_finite() {
            x
        } else {
            ZERO
        };
        self.seen = (self.seen + 1.0).min(POWER_MEMORY_SAMPLES);
        self.power += (x.norm_sqr() - self.power) / self.seen;
        let scale = self.power.max(ENERGY_FLOOR).sqrt().recip();
        self.line.push(x * scale);
    }

    pub fn symbol(
        &mut self,
        carrier: Option<&mut CarrierLoop>,
        table: &Constellation,
    ) -> Complex<f32> {
        let rotation = carrier.as_ref().map_or(ONE, |c| c.rotation());
        let z = dot(&self.forward, self.line.window()) * rotation
            + dot(&self.feedback, self.decisions.window());
        let decision = table.nearest(z);
        self.judge(z, decision);
        self.adapt(z, decision, rotation);
        if let Some(carrier) = carrier {
            if self.trusted {
                carrier.steer(z, table);
            } else {
                carrier.steer_with(self.blind_detector, z, table);
            }
        }
        self.decisions
            .push(if self.trusted { decision } else { ZERO });
        self.guard(z);
        self.recentre();
        z
    }

    fn recentre(&mut self) {
        let drift = self.timing_error_symbols();
        if drift > 1.0 {
            self.forward.rotate_left(2);
            self.forward
                .iter_mut()
                .rev()
                .take(2)
                .for_each(|w| *w = ZERO);
            self.slips += 1;
        } else if drift < -1.0 {
            self.forward.rotate_right(2);
            self.forward.iter_mut().take(2).for_each(|w| *w = ZERO);
            self.slips += 1;
        }
    }

    #[must_use]
    pub fn timing_error_symbols(&self) -> f64 {
        let mut energy = 0.0f64;
        let mut moment = 0.0f64;
        for (k, w) in self.forward.iter().enumerate() {
            let e = f64::from(w.norm_sqr());
            energy += e;
            moment += e * k as f64;
        }
        if energy <= f64::from(ENERGY_FLOOR) {
            return 0.0;
        }
        (moment / energy - self.centre as f64) / 2.0
    }

    pub fn equalise_burst(
        &mut self,
        halves: &[Complex<f32>],
        carrier: Option<&CarrierLoop>,
        table: &Constellation,
        out: &mut Vec<Complex<f32>>,
    ) {
        self.hold_power(halves);
        for _ in 1..self.config.burst_passes {
            let start = out.len();
            self.run_pass(halves, carrier, table, out);
            out.truncate(start);
        }
        self.run_pass(halves, carrier, table, out);
    }

    fn hold_power(&mut self, halves: &[Complex<f32>]) {
        let finite = halves
            .iter()
            .filter(|x| x.re.is_finite() && x.im.is_finite());
        let count = finite.clone().count();
        if count == 0 {
            return;
        }
        self.power = finite.map(Complex::norm_sqr).sum::<f32>() / count as f32;
        self.seen = POWER_MEMORY_SAMPLES;
    }

    fn run_pass(
        &mut self,
        halves: &[Complex<f32>],
        carrier: Option<&CarrierLoop>,
        table: &Constellation,
        out: &mut Vec<Complex<f32>>,
    ) {
        let mut carrier = carrier.cloned();
        self.line.clear();
        self.decisions.clear();
        let delay = self.delay_symbols();
        let mut emitted = 0usize;
        let mut emit = |eq: &mut Self, carrier: &mut Option<CarrierLoop>| {
            let rotation = carrier.as_ref().map_or(ONE, CarrierLoop::rotation);
            let z = eq.symbol(carrier.as_mut(), table);
            if emitted >= delay {
                out.push(z * rotation.conj());
            }
            emitted += 1;
        };
        for pair in halves.chunks(2) {
            self.push(pair[0]);
            emit(self, &mut carrier);
            if let Some(&mid) = pair.get(1) {
                self.push(mid);
            }
        }
        for _ in 0..delay {
            self.line.push(ZERO);
            emit(self, &mut carrier);
            self.line.push(ZERO);
        }
    }

    pub fn reset(&mut self) {
        self.forward.fill(ZERO);
        if let Some(tap) = self.forward.get_mut(self.centre) {
            *tap = ONE;
        }
        self.feedback.fill(ZERO);
        self.line.clear();
        self.decisions.clear();
        self.power = 0.0;
        self.seen = 0.0;
        self.misfit = 1.0;
        self.output_power = 1.0;
        self.trusted = false;
        self.step = StepControl::default();
    }

    fn judge(&mut self, z: Complex<f32>, decision: Complex<f32>) {
        let miss = (z - decision).norm_sqr() / self.spacing2;
        self.misfit += (miss.min(1.0) - self.misfit) / MISFIT_SYMBOLS;
        let was = self.trusted;
        self.trusted = if was {
            self.misfit < DISTRUST_MISFIT
        } else {
            self.misfit < TRUST_MISFIT
        };
        if was && !self.trusted {
            self.feedback.fill(ZERO);
        }
        if !was && self.trusted {
            self.step = StepControl::default();
        }
    }

    fn adapt(&mut self, z: Complex<f32>, decision: Complex<f32>, rotation: Complex<f32>) {
        let (error, step) = if self.trusted {
            let error = decision - z;
            let max = self.config.decision_step;
            (error, self.step.next(error / self.spacing2.sqrt(), max))
        } else {
            (self.blind.error(z, decision), self.config.blind_step)
        };
        let expected = 0.5 * (self.forward.len() + self.feedback.len()) as f32;
        let energy = energy(self.line.window()) + energy(self.decisions.window());
        let gain = step / energy.max(expected);
        let back = error * rotation.conj() * gain;
        for (w, x) in self.forward.iter_mut().zip(self.line.window()) {
            *w = *w * (1.0 - LEAKAGE) + back * x.conj();
        }
        if self.trusted {
            let forward = error * gain;
            for (b, d) in self.feedback.iter_mut().zip(self.decisions.window()) {
                *b += forward * d.conj();
            }
        }
    }

    fn guard(&mut self, z: Complex<f32>) {
        self.output_power += (z.norm_sqr() - self.output_power) / OUTPUT_MEMORY_SYMBOLS;
        let finite = self.output_power.is_finite()
            && self
                .forward
                .iter()
                .all(|w| w.re.is_finite() && w.im.is_finite());
        if !finite || !(MIN_OUTPUT_POWER..=MAX_OUTPUT_POWER).contains(&self.output_power) {
            self.resets += 1;
            self.reset();
        }
    }
}

fn dot(taps: &[Complex<f32>], window: &[Complex<f32>]) -> Complex<f32> {
    taps.iter().zip(window).map(|(w, x)| w * x).sum()
}

fn energy(window: &[Complex<f32>]) -> f32 {
    window.iter().map(Complex::norm_sqr).sum()
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::{
        impair::{ClockError, Impairment, MovingEcho},
        perf::assert_no_alloc,
        rng::Rng,
    };

    use super::*;
    use crate::{
        constellation::tables,
        linear::{
            LinearBurstDemod, LinearDemod, LinearMod, LinearParams, LinearTiming, PhaseDetector,
        },
        pulse::{self, Norm},
    };

    const SPS: usize = 8;

    fn rrc() -> Vec<f32> {
        pulse::root_raised_cosine(SPS as f64, 0.35, 8, Norm::Energy)
    }

    fn labels(n: usize, m: usize, seed: u64) -> Vec<u32> {
        let mut rng = Rng::new(seed);
        (0..n).map(|_| (rng.next_u64() % m as u64) as u32).collect()
    }

    fn echoes() -> Vec<(usize, Complex<f32>)> {
        vec![
            (0, Complex::new(1.0, 0.0)),
            (SPS, Complex::from_polar(0.35, 1.0)),
            (SPS + SPS / 2, Complex::from_polar(0.2, -2.0)),
            (3 * SPS, Complex::from_polar(0.1, 0.5)),
        ]
    }

    fn smear(wave: &[Complex<f32>], taps: &[(usize, Complex<f32>)]) -> Vec<Complex<f32>> {
        let power: f32 = taps.iter().map(|(_, h)| h.norm_sqr()).sum();
        let scale = power.sqrt().recip();
        (0..wave.len())
            .map(|n| {
                taps.iter()
                    .filter_map(|&(d, h)| n.checked_sub(d).map(|i| wave[i] * h))
                    .sum::<Complex<f32>>()
                    * scale
            })
            .collect()
    }

    fn add_noise(wave: &mut [Complex<f32>], es_n0_db: f64, seed: u64) {
        let mut rng = Rng::new(seed);
        let sigma = (0.5 / 10f64.powf(es_n0_db / 10.0)).sqrt();
        for s in wave {
            let (a, b) = rng.normal_pair();
            *s += Complex::new((a * sigma) as f32, (b * sigma) as f32);
        }
    }

    const GAIN_BLOCK: usize = 256;

    fn symbol_errors(
        got: &[Complex<f32>],
        sent: &[u32],
        table: &Constellation,
        from: usize,
    ) -> usize {
        let points: Vec<Complex<f32>> = sent
            .iter()
            .map(|&l| table.points()[table.labels().iter().position(|&x| x == l).unwrap()])
            .collect();
        (0..=64usize)
            .map(|lag| {
                (from..sent.len())
                    .step_by(GAIN_BLOCK)
                    .map(|start| {
                        let block = start..(start + GAIN_BLOCK).min(sent.len());
                        block_errors(got, &points, sent, table, block, lag)
                    })
                    .sum::<usize>()
            })
            .min()
            .unwrap_or(usize::MAX)
    }

    fn block_errors(
        got: &[Complex<f32>],
        points: &[Complex<f32>],
        sent: &[u32],
        table: &Constellation,
        block: std::ops::Range<usize>,
        lag: usize,
    ) -> usize {
        let pairs = || {
            block
                .clone()
                .filter_map(|k| got.get(k + lag).map(|&y| (y, points[k])))
        };
        let cross: Complex<f32> = pairs().map(|(y, x)| y * x.conj()).sum();
        let power: f32 = pairs().map(|(_, x)| x.norm_sqr()).sum();
        let gain = Complex::new(power, 0.0) / cross;
        block
            .filter(|&k| {
                got.get(k + lag)
                    .is_none_or(|&y| table.hard_slice(y * gain) != sent[k])
            })
            .count()
    }

    fn params(table: &Constellation) -> LinearParams {
        LinearParams::new(table.clone(), rrc(), SPS).unwrap()
    }

    fn carrier() -> Option<CarrierLoop> {
        Some(CarrierLoop::new(PhaseDetector::DecisionDirected, 0.003))
    }

    fn burst(
        table: &Constellation,
        wave: &[Complex<f32>],
        eq: Option<EqualiserConfig>,
    ) -> Vec<Complex<f32>> {
        let p = params(table);
        let mut demod = LinearBurstDemod::new(&p, &rrc(), f64::INFINITY, carrier());
        if let Some(config) = eq {
            demod = demod.with_equaliser(config).unwrap();
        }
        let mut out = Vec::new();
        demod.process(wave, &mut out);
        out
    }

    fn streamed(
        table: &Constellation,
        wave: &[Complex<f32>],
        eq: Option<EqualiserConfig>,
    ) -> Vec<Complex<f32>> {
        let p = params(table);
        let mut demod = LinearDemod::new(&p, &rrc(), LinearTiming::CONTINUOUS, carrier());
        if let Some(config) = eq {
            demod = demod.with_equaliser(config).unwrap();
        }
        let mut out = Vec::new();
        demod.process(wave, &mut out);
        out
    }

    fn wave_through(
        table: &Constellation,
        sent: &[u32],
        channel: &[(usize, Complex<f32>)],
        es_n0_db: f64,
    ) -> Vec<Complex<f32>> {
        let mut wave = smear(&LinearMod::transmission(&params(table), sent), channel);
        add_noise(&mut wave, es_n0_db, 0x5eed);
        wave
    }

    fn cases() -> [(&'static str, Constellation, f64); 3] {
        [
            ("qpsk", tables::qam_square(4).unwrap(), 17.0),
            ("16-qam", tables::qam_square(16).unwrap(), 25.0),
            ("64-qam", tables::qam_square(64).unwrap(), 31.0),
        ]
    }

    #[test]
    fn a_static_echo_is_equalised_on_a_burst() {
        for (name, table, snr) in cases() {
            let sent = labels(8_000, table.len(), 0xe0);
            let wave = wave_through(&table, &sent, &echoes(), snr);
            let tail = &sent[..7_000];
            let plain = symbol_errors(&burst(&table, &wave, None), tail, &table, 1_000);
            let equalised = burst(&table, &wave, Some(EqualiserConfig::DEFAULT));
            let errors = symbol_errors(&equalised, tail, &table, 1_000);
            println!("burst {name}: {errors} equalised, {plain} plain");
            assert_eq!(errors, 0, "{name}: {errors} errors, {plain} without");
        }
    }

    #[test]
    fn the_equalised_burst_keeps_the_plain_alignment() {
        let table = tables::qam_square(16).unwrap();
        let sent = labels(4_000, 16, 0xa1);
        let wave = wave_through(&table, &sent, &[(0, ONE)], 30.0);
        let lag = |out: &[Complex<f32>]| {
            (0..64usize).find(|&l| {
                (100..200).all(|k| {
                    out.get(k + l)
                        .is_some_and(|&y| table.hard_slice(y) == sent[k])
                })
            })
        };
        let plain = burst(&table, &wave, None);
        let equalised = burst(&table, &wave, Some(EqualiserConfig::DEFAULT));
        assert!(lag(&plain).is_some());
        assert_eq!(lag(&equalised), lag(&plain));
        assert_eq!(equalised.len(), plain.len());
    }

    #[test]
    fn a_static_echo_is_equalised_while_streaming() {
        for (name, table, snr) in cases() {
            let sent = labels(20_000, table.len(), 0xe1);
            let wave = wave_through(&table, &sent, &echoes(), snr);
            let tail = &sent[..19_000];
            let plain = symbol_errors(&streamed(&table, &wave, None), tail, &table, 10_000);
            let equalised = streamed(&table, &wave, Some(EqualiserConfig::DEFAULT));
            let errors = symbol_errors(&equalised, tail, &table, 10_000);
            println!("stream {name}: {errors} equalised, {plain} plain");
            assert_eq!(errors, 0, "{name}: {errors} errors, {plain} without");
        }
    }

    #[test]
    fn a_moving_echo_is_tracked() {
        let table = tables::qam_square(16).unwrap();
        let sent = labels(40_000, 16, 0xe2);
        let mut wave = LinearMod::transmission(&params(&table), &sent);
        MovingEcho::new(SPS, -8.0, 1e-5).apply(&mut wave, &mut Rng::new(1));
        add_noise(&mut wave, 22.0, 0x6060);
        let tail = &sent[..39_000];
        let plain = symbol_errors(&streamed(&table, &wave, None), tail, &table, 10_000);
        let equalised = streamed(&table, &wave, Some(EqualiserConfig::DEFAULT));
        let errors = symbol_errors(&equalised, tail, &table, 10_000);
        println!("moving echo: {errors} equalised, {plain} plain");
        assert!(errors <= 30, "{errors} errors, {plain} without");
        assert!(plain > 1_000, "the moving echo only cost {plain} errors");
    }

    #[test]
    fn a_clock_offset_is_followed_while_streaming() {
        let table = tables::qam_square(16).unwrap();
        let sent = labels(30_000, 16, 0xe3);
        let mut wave = smear(&LinearMod::transmission(&params(&table), &sent), &echoes());
        ClockError::new(150.0).apply(&mut wave, &mut Rng::new(2));
        add_noise(&mut wave, 22.0, 0x7070);
        let p = params(&table);
        let mut demod = LinearDemod::new(&p, &rrc(), LinearTiming::CONTINUOUS, carrier())
            .with_equaliser(EqualiserConfig::DEFAULT)
            .unwrap();
        let mut out = Vec::new();
        demod.process(&wave, &mut out);
        let errors = symbol_errors(&out, &sent[..29_000], &table, 15_000);
        assert_eq!(errors, 0);
        assert_eq!(demod.equaliser().map(Equaliser::slips), Some(0));
    }

    #[test]
    fn awgn_alone_costs_next_to_nothing() {
        for (name, table, snr, burst_mode) in [
            ("qpsk burst", tables::qam_square(4).unwrap(), 8.0, true),
            ("16-qam burst", tables::qam_square(16).unwrap(), 15.0, true),
            ("qpsk stream", tables::qam_square(4).unwrap(), 8.0, false),
            (
                "16-qam stream",
                tables::qam_square(16).unwrap(),
                15.0,
                false,
            ),
        ] {
            let sent = labels(20_000, table.len(), 0xa2);
            let wave = wave_through(&table, &sent, &[(0, ONE)], snr);
            let run = |eq| {
                let out = if burst_mode {
                    burst(&table, &wave, eq)
                } else {
                    streamed(&table, &wave, eq)
                };
                symbol_errors(&out, &sent[..19_000], &table, 4_000)
            };
            let plain = run(None);
            let equalised = run(Some(EqualiserConfig::DEFAULT));
            println!("awgn {name}: {equalised} equalised, {plain} plain");
            assert!(plain > 50, "{name}: too few errors ({plain}) to compare");
            assert!(
                equalised as f64 <= 1.2 * plain as f64,
                "{name}: {equalised} errors equalised, {plain} plain"
            );
        }
    }

    #[test]
    fn a_blown_up_equaliser_resets_and_recovers() {
        let table = tables::qam_square(4).unwrap();
        let sent = labels(6_000, 4, 0xa3);
        let wave = wave_through(&table, &sent, &echoes(), 14.0);
        let mut eq = Equaliser::new(EqualiserConfig::DEFAULT, &table).unwrap();
        eq.forward.fill(Complex::new(40.0, 0.0));
        let p = params(&table);
        let mut timing = crate::linear::FeedforwardTiming::new(&p, &rrc());
        let mut halves = Vec::new();
        timing.process_spaced(&wave, 2, &mut halves);
        let mut out = Vec::new();
        eq.equalise_burst(&halves, carrier().as_ref(), &table, &mut out);
        assert!(eq.resets() >= 1);
        assert!(out.iter().all(|z| z.re.is_finite() && z.im.is_finite()));
        assert_eq!(symbol_errors(&out, &sent[..5_000], &table, 3_000), 0);
    }

    #[test]
    fn decision_feedback_cancels_a_strong_postcursor() {
        let table = tables::qam_square(16).unwrap();
        let sent = labels(8_000, 16, 0xa4);
        let channel = [(0, ONE), (SPS, Complex::from_polar(0.7, 2.0))];
        let wave = wave_through(&table, &sent, &channel, 20.0);
        let tail = &sent[..7_000];
        let run = |config| symbol_errors(&burst(&table, &wave, Some(config)), tail, &table, 1_000);
        let linear = run(EqualiserConfig::DEFAULT);
        let feedback = run(EqualiserConfig::DEFAULT.with_feedback(4));
        println!("postcursor: linear {linear}, feedback {feedback}");
        assert!(feedback <= linear, "feedback {feedback}, linear {linear}");
    }

    #[test]
    fn the_blind_target_follows_the_table() {
        let target = |t: Constellation| BlindTarget::for_table(&t);
        assert!(matches!(
            target(tables::qam_square(16).unwrap()),
            BlindTarget::Axes { .. }
        ));
        assert!(matches!(
            target(tables::pam(2).unwrap()),
            BlindTarget::Axes { im, .. } if im == 0.0
        ));
        assert!(matches!(
            target(tables::psk(8).unwrap()),
            BlindTarget::Modulus(r) if (r - 1.0).abs() < 1e-5
        ));
        assert!(matches!(
            target(tables::apsk16_dvbs2(2.57).unwrap()),
            BlindTarget::Modulus(_)
        ));
    }

    #[test]
    fn a_bad_configuration_is_refused() {
        let table = tables::qam_square(4).unwrap();
        let bad = |config| Equaliser::new(config, &table).err();
        assert_eq!(
            bad(EqualiserConfig::DEFAULT.with_span(0)),
            Some(EqualiserError::NoSpan)
        );
        assert_eq!(
            bad(EqualiserConfig {
                burst_passes: 0,
                ..EqualiserConfig::DEFAULT
            }),
            Some(EqualiserError::NoPasses)
        );
        assert_eq!(
            bad(EqualiserConfig {
                decision_step: 1.5,
                ..EqualiserConfig::DEFAULT
            }),
            Some(EqualiserError::StepOutOfRange(1.5))
        );
    }

    #[test]
    fn any_block_split_gives_the_same_symbols() {
        let table = tables::qam_square(16).unwrap();
        let sent = labels(3_000, 16, 0xa5);
        let wave = wave_through(&table, &sent, &echoes(), 25.0);
        let p = params(&table);
        let demod = || {
            LinearDemod::new(&p, &rrc(), LinearTiming::CONTINUOUS, carrier())
                .with_equaliser(EqualiserConfig::DEFAULT.with_feedback(2))
                .unwrap()
        };
        let mut whole = demod();
        let mut a = Vec::new();
        whole.process(&wave, &mut a);
        let mut split = demod();
        let mut b = Vec::new();
        for chunk in wave.chunks(313) {
            split.process(chunk, &mut b);
        }
        assert_eq!(a, b);
    }

    #[test]
    fn steady_state_allocates_nothing() {
        let table = tables::qam_square(16).unwrap();
        let p = params(&table);
        let wave = LinearMod::transmission(&p, &labels(2_048, 16, 0xa6));
        let mut demod = LinearDemod::new(&p, &rrc(), LinearTiming::CONTINUOUS, carrier())
            .with_equaliser(EqualiserConfig::DEFAULT.with_feedback(3))
            .unwrap();
        let mut out = Vec::with_capacity(wave.len());
        demod.process(&wave, &mut out);
        out.clear();
        demod.process(&wave, &mut out);
        out.clear();
        assert_no_alloc("LinearDemod::process (equalised)", || {
            demod.process(&wave, &mut out);
        });
        assert!(!out.is_empty());
    }
}
