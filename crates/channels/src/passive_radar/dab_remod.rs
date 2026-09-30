use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::fft::FftPair;
use sdrmm_wire::DabTransmissionMode;
use sdrmm_wire::radar::{DAB_SAMPLE_RATE_HZ, ReferenceHealth, ReferenceMode};

use super::plan::DAB_FRAME_LATENCY;
use crate::ChannelError;
use crate::dab::mode::Mode;
use crate::dab::ofdm::{FrameSync, reference_symbol_for_mode};

type C32 = Complex<f32>;
type C64 = Complex<f64>;

const RING: usize = 1 << 19;
const PIECE: usize = 1 << 16;
const EARLY: usize = 8;
const SEARCH: usize = 128;
const REFINE: usize = 16;
const CFO_SYMBOLS: usize = 5;
const INTEGER_SHIFTS: i64 = 8;
const MIN_COHERENCE: f64 = 0.5;
const MIN_MER_DB: f32 = 8.0;
const CLEAN_MER_DB: f32 = 99.0;
const CHANNEL_RATE: f32 = 0.1;
const RATE_TOLERANCE_HZ: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Acquire(Option<u64>),
    Locked(u64),
}

struct Timing {
    start: u64,
    coherence: f64,
}

pub struct DabRemod {
    mode: Mode,
    rate: f64,
    latency: usize,
    raw: Vec<C32>,
    rebuilt: Vec<C32>,
    total: u64,
    decided: u64,
    forced: u64,
    sync: FrameSync,
    state: State,
    cfo_hz: f64,
    fft: FftPair,
    carriers: Vec<usize>,
    signed: Vec<i64>,
    prs: Vec<C32>,
    prs_wave: Vec<C32>,
    prs_energy: f64,
    channel: Vec<C32>,
    smoothed: Vec<C32>,
    bins: Vec<C32>,
    segment: Vec<C32>,
    locked: bool,
    quality_db: f32,
    fallback_frames: u64,
}

impl DabRemod {
    pub fn new(sample_rate: f64) -> Result<Self, ChannelError> {
        if (sample_rate - DAB_SAMPLE_RATE_HZ).abs() > RATE_TOLERANCE_HZ {
            return Err(ChannelError::Refused("DAB needs 2.048 MS/s"));
        }
        let transmission = DabTransmissionMode::I;
        let mode = Mode::new(transmission);
        let half = (mode.carriers() / 2) as i64;
        let signed: Vec<i64> = (-half..=half).filter(|&carrier| carrier != 0).collect();
        let carriers: Vec<usize> = signed
            .iter()
            .map(|&carrier| carrier.rem_euclid(mode.useful as i64) as usize)
            .collect();
        let spectrum = reference_symbol_for_mode(transmission);
        let prs: Vec<C32> = carriers.iter().map(|&bin| spectrum[bin]).collect();
        let mut remod = Self {
            mode,
            rate: sample_rate,
            latency: DAB_FRAME_LATENCY,
            raw: vec![C32::default(); RING],
            rebuilt: vec![C32::default(); RING],
            total: 0,
            decided: 0,
            forced: 0,
            sync: FrameSync::for_mode(transmission),
            state: State::Acquire(None),
            cfo_hz: 0.0,
            fft: FftPair::new(mode.useful),
            channel: vec![C32::default(); carriers.len()],
            smoothed: vec![C32::default(); carriers.len()],
            carriers,
            signed,
            prs,
            prs_wave: vec![C32::default(); mode.symbol()],
            prs_energy: 0.0,
            bins: vec![C32::default(); mode.useful],
            segment: Vec::with_capacity(mode.symbol() + 2 * SEARCH + 1),
            locked: false,
            quality_db: 0.0,
            fallback_frames: 0,
        };
        remod.build_prs_wave();
        Ok(remod)
    }

    #[must_use]
    pub const fn latency(&self) -> usize {
        self.latency
    }

    #[must_use]
    pub const fn health(&self) -> ReferenceHealth {
        ReferenceHealth {
            mode: ReferenceMode::DabRemod,
            locked: self.locked,
            quality_db: self.quality_db,
            fallback_frames: self.fallback_frames,
        }
    }

    pub fn reset(&mut self) {
        self.total = 0;
        self.decided = 0;
        self.forced = 0;
        self.sync.reset();
        self.state = State::Acquire(None);
        self.cfo_hz = 0.0;
        self.locked = false;
        self.quality_db = 0.0;
    }

    pub fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        out.clear();
        for piece in input.chunks(PIECE) {
            for &sample in piece {
                self.take(sample);
            }
            self.advance();
            self.emit(piece.len(), out);
        }
    }

    fn take(&mut self, sample: C32) {
        let sample = if sample.is_finite() {
            sample
        } else {
            C32::default()
        };
        let index = self.total;
        self.raw[slot(index)] = sample;
        self.total += 1;
        if let State::Acquire(None) = self.state
            && let Some(run) = self.sync.push(sample)
        {
            let candidate = (index + 1).checked_sub(run as u64);
            self.state = State::Acquire(candidate.filter(|&c| c > SEARCH as u64));
        }
    }

    fn advance(&mut self) {
        let symbol = self.mode.symbol() as u64;
        loop {
            match self.state {
                State::Acquire(Some(candidate))
                    if self.total
                        > candidate + SEARCH as u64 + (CFO_SYMBOLS as u64 + 1) * symbol =>
                {
                    self.acquire(candidate);
                }
                State::Locked(start) if self.total > start + REFINE as u64 + self.body() => {
                    self.frame(start);
                }
                _ => break,
            }
        }
    }

    fn body(&self) -> u64 {
        self.mode.frame_samples() as u64
    }

    fn emit(&mut self, count: usize, out: &mut Vec<C32>) {
        let produced = self.total - count as u64;
        for output in produced..self.total {
            let Some(index) = output.checked_sub(self.latency as u64) else {
                out.push(C32::default());
                continue;
            };
            if index >= self.decided {
                self.force_raw(index + 1);
            }
            out.push(self.rebuilt[slot(index)]);
        }
    }

    fn force_raw(&mut self, until: u64) {
        for index in self.decided..until {
            self.rebuilt[slot(index)] = self.raw[slot(index)];
        }
        self.forced += until - self.decided;
        self.decided = until;
        self.locked = false;
        let frame = self.mode.frame() as u64;
        while self.forced >= frame {
            self.forced -= frame;
            self.fallback_frames += 1;
        }
    }

    fn acquire(&mut self, candidate: u64) {
        self.state = State::Acquire(None);
        let coarse = self.cfo_from_prefix(candidate, CFO_SYMBOLS, 0.0);
        let shift = self.integer_shift(candidate, coarse);
        let spacing = self.rate / self.mode.useful as f64;
        let cfo = coarse + shift as f64 * spacing;
        let timing = self
            .best_timing(candidate, SEARCH, cfo)
            .filter(|timing| timing.coherence >= MIN_COHERENCE);
        let Some(timing) = timing else {
            self.sync.reset();
            return;
        };
        let refined = cfo + self.cfo_from_prefix(timing.start, CFO_SYMBOLS, cfo);
        if !refined.is_finite() {
            self.sync.reset();
            return;
        }
        self.cfo_hz = refined;
        self.state = State::Locked(timing.start);
    }

    fn frame(&mut self, predicted: u64) {
        let timing = self
            .best_timing(predicted, REFINE, self.cfo_hz)
            .filter(|timing| timing.coherence >= MIN_COHERENCE);
        let Some(timing) = timing else {
            self.lose_lock();
            return;
        };
        let start = timing.start;
        let residual_hz = self.cfo_from_prefix(start, self.mode.symbols, self.cfo_hz);
        if residual_hz.is_finite() {
            self.cfo_hz += residual_hz;
        }
        let null_start = start.saturating_sub(self.mode.null as u64);
        if null_start > self.decided {
            self.force_raw(null_start);
        }
        for index in self.decided.max(null_start)..start {
            self.rebuilt[slot(index)] = C32::default();
        }
        let first = self.decided.max(start);
        let mer = self.remodulate(start, first);
        let end = start + self.body();
        if !(mer.is_finite() && mer >= MIN_MER_DB) {
            for index in self.decided.max(null_start)..end {
                self.rebuilt[slot(index)] = self.raw[slot(index)];
            }
            self.fallback_frames += 1;
            self.locked = false;
        } else {
            self.locked = true;
        }
        self.quality_db = mer;
        self.decided = self.decided.max(end);
        self.state = State::Locked(start + self.mode.frame() as u64);
    }

    fn lose_lock(&mut self) {
        self.state = State::Acquire(None);
        self.sync.reset();
        self.locked = false;
    }

    fn remodulate(&mut self, start: u64, first: u64) -> f32 {
        let (useful, guard, symbol) =
            (self.mode.useful, self.mode.guard, self.mode.symbol() as u64);
        let mut signal = 0.0f64;
        let mut error = 0.0f64;
        let mut gain = C32::default();
        for index in 0..self.mode.symbols {
            let symbol_start = start + index as u64 * symbol;
            self.load_window(symbol_start + (guard - EARLY) as u64, self.cfo_hz);
            if index == 0 {
                gain = self.estimate_channel();
                self.place_prs();
            } else {
                let (s, e) = self.decide(index);
                signal += s;
                error += e;
            }
            self.fft.inverse(&mut self.bins);
            self.write_symbol(symbol_start, first, gain / useful as f32);
        }
        if error <= 0.0 {
            return CLEAN_MER_DB;
        }
        (10.0 * (signal / error).log10()) as f32
    }

    fn load_window(&mut self, from: u64, cfo_hz: f64) {
        let mut turn = rotation(-cfo_hz * from as f64 / self.rate);
        let step = rotation(-cfo_hz / self.rate);
        for (offset, bin) in self.bins.iter_mut().enumerate() {
            let value = self.raw[slot(from + offset as u64)];
            *bin = narrow(widen(value) * turn);
            turn *= step;
        }
        self.fft.forward(&mut self.bins);
    }

    fn estimate_channel(&mut self) -> C32 {
        let useful = self.mode.useful as f64;
        let mut gain = C64::default();
        for (((channel, &bin), prs), &carrier) in self
            .channel
            .iter_mut()
            .zip(&self.carriers)
            .zip(&self.prs)
            .zip(&self.signed)
        {
            *channel = self.bins[bin] * prs.conj();
            let ramp = rotation(carrier as f64 * EARLY as f64 / useful);
            gain += widen(*channel) * ramp;
        }
        let count = self.channel.len();
        for index in 0..count {
            let low = index.saturating_sub(1);
            let high = (index + 1).min(count - 1);
            let sum: C32 = self.channel[low..=high].iter().sum();
            self.smoothed[index] = sum / (high - low + 1) as f32;
        }
        self.channel.copy_from_slice(&self.smoothed);
        narrow(gain / count as f64)
    }

    fn place_prs(&mut self) {
        self.bins.fill(C32::default());
        for (&bin, &value) in self.carriers.iter().zip(&self.prs) {
            self.bins[bin] = value;
        }
    }

    fn decide(&mut self, index: usize) -> (f64, f64) {
        let rotate = rotation(index as f64 / 8.0);
        let rotate = narrow(rotate);
        let mut signal = 0.0f64;
        let mut error = 0.0f64;
        for (channel, &bin) in self.channel.iter_mut().zip(&self.carriers) {
            let received = self.bins[bin];
            let equalised = if channel.norm_sqr() > 0.0 {
                received / *channel
            } else {
                C32::default()
            };
            let decided = rotate * nearest_axis(equalised * rotate.conj());
            signal += f64::from(decided.norm_sqr());
            error += f64::from((equalised - decided).norm_sqr());
            *channel += (received * decided.conj() - *channel) * CHANNEL_RATE;
            self.bins[bin] = decided;
        }
        for (bin, value) in self.bins.iter_mut().enumerate() {
            if !is_active(bin, self.mode.useful) {
                *value = C32::default();
            }
        }
        (signal, error)
    }

    fn write_symbol(&mut self, symbol_start: u64, first: u64, gain: C32) {
        let (useful, guard) = (self.mode.useful, self.mode.guard);
        let mut turn = rotation(self.cfo_hz * symbol_start as f64 / self.rate);
        let step = rotation(self.cfo_hz / self.rate);
        for offset in 0..useful + guard {
            let index = symbol_start + offset as u64;
            let sample = self.bins[(offset + useful - guard) % useful] * gain;
            if index >= first {
                self.rebuilt[slot(index)] = narrow(widen(sample) * turn);
            }
            turn *= step;
        }
    }

    fn cfo_from_prefix(&self, start: u64, symbols: usize, assumed_hz: f64) -> f64 {
        let (useful, guard, symbol) =
            (self.mode.useful, self.mode.guard, self.mode.symbol() as u64);
        let mut sum = C64::default();
        for index in 0..symbols as u64 {
            let at = start + index * symbol;
            for offset in 0..guard as u64 {
                let prefix = widen(self.raw[slot(at + offset)]);
                let tail = widen(self.raw[slot(at + useful as u64 + offset)]);
                sum += prefix * tail.conj();
            }
        }
        if sum.norm_sqr() == 0.0 {
            return 0.0;
        }
        let per_hz = TAU * useful as f64 / self.rate;
        let predicted = -assumed_hz * per_hz;
        let residual = wrap(sum.arg() - predicted);
        -residual / per_hz
    }

    fn integer_shift(&mut self, candidate: u64, cfo_hz: f64) -> i64 {
        let useful = self.mode.useful;
        self.load_window(candidate + self.mode.guard as u64, cfo_hz);
        let mut best = (0, 0.0f64);
        for shift in -INTEGER_SHIFTS..=INTEGER_SHIFTS {
            let mut score = C64::default();
            for pair in 0..self.signed.len() - 1 {
                if self.signed[pair + 1] - self.signed[pair] != 1 {
                    continue;
                }
                let bin = |at: usize| {
                    (self.carriers[at] as i64 + shift).rem_euclid(useful as i64) as usize
                };
                let received = self.bins[bin(pair)] * self.bins[bin(pair + 1)].conj();
                let known = self.prs[pair] * self.prs[pair + 1].conj();
                score += widen(received * known.conj());
            }
            if score.norm() > best.1 {
                best = (shift, score.norm());
            }
        }
        best.0
    }

    fn best_timing(&mut self, centre: u64, spread: usize, cfo_hz: f64) -> Option<Timing> {
        let first = centre.checked_sub(spread as u64)?;
        let length = self.mode.symbol() + 2 * spread;
        if first + length as u64 > self.total || self.total - first > RING as u64 {
            return None;
        }
        self.segment.clear();
        let mut turn = rotation(-cfo_hz * first as f64 / self.rate);
        let step = rotation(-cfo_hz / self.rate);
        for offset in 0..length {
            let value = self.raw[slot(first + offset as u64)];
            self.segment.push(narrow(widen(value) * turn));
            turn *= step;
        }
        let symbol = self.mode.symbol();
        let mut best: Option<Timing> = None;
        for shift in 0..=2 * spread {
            let window = &self.segment[shift..shift + symbol];
            let mut sum = C64::default();
            let mut energy = 0.0f64;
            for (value, reference) in window.iter().zip(&self.prs_wave) {
                sum += widen(*value * reference.conj());
                energy += f64::from(value.norm_sqr());
            }
            let coherence = sum.norm() / (energy * self.prs_energy).sqrt().max(f64::MIN_POSITIVE);
            if best
                .as_ref()
                .is_none_or(|timing| coherence > timing.coherence)
            {
                best = Some(Timing {
                    start: first + shift as u64,
                    coherence,
                });
            }
        }
        best
    }

    fn build_prs_wave(&mut self) {
        let (useful, guard) = (self.mode.useful, self.mode.guard);
        self.place_prs();
        self.fft.inverse(&mut self.bins);
        let scale = 1.0 / (useful as f32).sqrt();
        for (offset, value) in self.prs_wave.iter_mut().enumerate() {
            *value = self.bins[(offset + useful - guard) % useful] * scale;
        }
        self.prs_energy = self.prs_wave.iter().map(|v| f64::from(v.norm_sqr())).sum();
    }
}

const fn slot(index: u64) -> usize {
    (index % RING as u64) as usize
}

fn is_active(bin: usize, useful: usize) -> bool {
    let half = useful * 3 / 8;
    bin != 0 && (bin <= half || bin >= useful - half)
}

fn rotation(turns: f64) -> C64 {
    C64::from_polar(1.0, TAU * turns.rem_euclid(1.0))
}

fn wrap(angle: f64) -> f64 {
    (angle + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI
}

fn widen(value: C32) -> C64 {
    C64::new(f64::from(value.re), f64::from(value.im))
}

fn narrow(value: C64) -> C32 {
    C32::new(value.re as f32, value.im as f32)
}

fn nearest_axis(value: C32) -> C32 {
    if value.re.abs() >= value.im.abs() {
        C32::new(value.re.signum(), 0.0)
    } else {
        C32::new(0.0, value.im.signum())
    }
}
