use std::sync::Arc;
use std::time::Instant;

use num_complex::Complex;
use sdrmm_dsp::Ddc;
use sdrmm_dsp::radar::batch::MAX_SURVEILLANCE;
use sdrmm_dsp::radar::nlms::{BlockNlms, SurveillanceNlms};
use sdrmm_wire::radar::ReferenceHealth;

use super::assemble::{CpiAssembler, CpiJob};
use super::plan::{CancellerPlan, LiveParams, RadarPlan};
use super::reference::ReferenceCleaner;
use crate::ChannelError;

type C32 = Complex<f32>;

pub const FRONT_CHUNK: usize = 8_192;

const RESAMPLER_SLACK: usize = 64;

pub struct InLanes<'a> {
    pub lanes: &'a [&'a [C32]],
    pub first_index: u64,
    pub unix_ns: u64,
    pub gap_before: bool,
    pub phase_ready: bool,
    pub generation: u64,
}

pub trait JobSink {
    fn take(&mut self) -> Option<Arc<CpiJob>>;
    fn submit(&mut self, job: Arc<CpiJob>);
    fn dropped(&mut self);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrontStats {
    pub busy_ns: u64,
    pub submitted: u32,
    pub dropped: u32,
    pub discarded_samples: u64,
    pub overflowed_samples: u64,
    pub mismatched: bool,
}

enum Canceller {
    Sample(Box<SurveillanceNlms>),
    Block(Box<BlockNlms>),
}

impl Canceller {
    fn new(plan: CancellerPlan, rate: f64) -> Result<Self, ChannelError> {
        let refused = |_| ChannelError::Refused("Step out of range");
        if plan.block {
            BlockNlms::new(plan.taps, plan.lead, plan.step, rate)
                .map(|nlms| Self::Block(Box::new(nlms)))
                .map_err(refused)
        } else {
            SurveillanceNlms::new(plan.taps, plan.lead, plan.step, rate)
                .map(|nlms| Self::Sample(Box::new(nlms)))
                .map_err(refused)
        }
    }

    fn process(&mut self, reference: &[C32], surveillance: &[C32], out: &mut [C32]) {
        match self {
            Self::Sample(nlms) => nlms.process(reference, surveillance, out),
            Self::Block(nlms) => nlms.process(reference, surveillance, out),
        };
    }

    const fn latency(&self) -> usize {
        match self {
            Self::Sample(nlms) => nlms.latency(),
            Self::Block(nlms) => nlms.latency(),
        }
    }

    fn suppression_db(&self) -> f32 {
        match self {
            Self::Sample(nlms) => nlms.suppression_db(),
            Self::Block(nlms) => nlms.suppression_db(),
        }
    }

    fn reset(&mut self) {
        match self {
            Self::Sample(nlms) => nlms.reset(),
            Self::Block(nlms) => nlms.reset(),
        }
    }

    const fn resets(&self) -> u64 {
        match self {
            Self::Sample(nlms) => nlms.resets(),
            Self::Block(nlms) => nlms.resets(),
        }
    }
}

struct Delay {
    line: Vec<C32>,
    at: usize,
}

impl Delay {
    fn new(samples: usize) -> Self {
        Self {
            line: vec![C32::default(); samples],
            at: 0,
        }
    }

    fn process(&mut self, input: &[C32], out: &mut Vec<C32>) {
        out.clear();
        if self.line.is_empty() {
            out.extend_from_slice(input);
            return;
        }
        for &sample in input {
            out.push(std::mem::replace(&mut self.line[self.at], sample));
            self.at = (self.at + 1) % self.line.len();
        }
    }

    fn reset(&mut self) {
        self.line.fill(C32::default());
        self.at = 0;
    }
}

pub struct FrontStage {
    lanes: usize,
    input_rate: f64,
    radar_rate: f64,
    ddcs: Vec<Ddc>,
    decimated: Vec<Vec<C32>>,
    cleaner: ReferenceCleaner,
    cleaned: Vec<C32>,
    delays: Vec<Delay>,
    delayed: Vec<C32>,
    cancellers: Vec<Canceller>,
    residual: Vec<C32>,
    assembler: CpiAssembler,
    generation: Option<u64>,
    next_index: Option<u64>,
    seq: u64,
    delivered_resets: u64,
}

impl FrontStage {
    pub fn new(plan: &RadarPlan) -> Result<Self, ChannelError> {
        let front = &plan.front;
        let lanes = front.lanes.len();
        let ddcs = (0..lanes)
            .map(|_| Ddc::new(front.input_rate, front.radar_rate, front.offset_hz))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ChannelError::Refused("Band outside the capture"))?;
        let cleaner = ReferenceCleaner::new(front.cleaning, front.radar_rate)?;
        let cancellers = front
            .canceller
            .map(|canceller| {
                (1..lanes)
                    .map(|_| Canceller::new(canceller, front.radar_rate))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let reference_latency = cleaner.latency();
        let delays = if cancellers.is_empty() || reference_latency == 0 {
            Vec::new()
        } else {
            (1..lanes).map(|_| Delay::new(reference_latency)).collect()
        };
        let chunk = chunk_out(front.input_rate, front.radar_rate);
        let mut latency = vec![reference_latency; lanes];
        for (slot, canceller) in latency.iter_mut().skip(1).zip(&cancellers) {
            *slot += canceller.latency();
        }
        if cancellers.is_empty() {
            latency[1..].fill(0);
        }
        let assembler = CpiAssembler::new(
            plan.shape.window(),
            plan.hop,
            plan.shape.pre(),
            front.radar_rate,
            &latency,
            chunk,
        );
        Ok(Self {
            lanes,
            input_rate: front.input_rate,
            radar_rate: front.radar_rate,
            ddcs,
            decimated: (0..lanes).map(|_| Vec::with_capacity(chunk)).collect(),
            cleaner,
            cleaned: Vec::with_capacity(chunk),
            delays,
            delayed: Vec::with_capacity(chunk),
            cancellers,
            residual: Vec::with_capacity(chunk),
            assembler,
            generation: None,
            next_index: None,
            seq: 0,
            delivered_resets: 0,
        })
    }

    #[must_use]
    pub const fn lanes(&self) -> usize {
        self.lanes
    }

    #[must_use]
    pub const fn window(&self) -> usize {
        self.assembler.window()
    }

    #[must_use]
    pub fn reference_health(&self) -> ReferenceHealth {
        self.cleaner.health()
    }

    #[must_use]
    pub const fn lost_samples(&self) -> u64 {
        self.assembler.lost()
    }

    pub fn tune(&mut self, live: &LiveParams) {
        self.assembler.set_hop(live.hop);
    }

    pub fn reset(&mut self) {
        for ddc in &mut self.ddcs {
            ddc.reset();
        }
        self.cleaner.reset();
        for delay in &mut self.delays {
            delay.reset();
        }
        for canceller in &mut self.cancellers {
            canceller.reset();
        }
        self.generation = None;
        self.next_index = None;
    }

    pub fn push(&mut self, input: &InLanes<'_>, sink: &mut impl JobSink) -> FrontStats {
        let started = Instant::now();
        let mut stats = FrontStats::default();
        let len = input.lanes.first().map_or(0, |lane| lane.len());
        if input.lanes.len() != self.lanes || input.lanes.iter().any(|lane| lane.len() != len) {
            stats.mismatched = true;
            stats.discarded_samples = len as u64;
            stats.busy_ns = elapsed_ns(started);
            return stats;
        }
        let restart = input.gap_before
            || self.generation != Some(input.generation)
            || self.next_index != Some(input.first_index);
        if restart {
            stats.discarded_samples += self.input_samples(self.assembler.buffered());
            self.restart(input);
        }
        let first = self.radar_index(input.first_index);
        self.assembler.anchor(first, input.unix_ns);
        if !input.phase_ready {
            self.assembler
                .mark_unready(self.radar_index(input.first_index + len as u64));
        }
        self.next_index = Some(input.first_index + len as u64);
        let lost = self.assembler.lost();
        let mut start = 0;
        while start < len {
            let end = (start + FRONT_CHUNK).min(len);
            self.process_chunk(input.lanes, start..end);
            self.emit(sink, &mut stats);
            start = end;
        }
        stats.overflowed_samples = self.assembler.lost() - lost;
        stats.busy_ns = elapsed_ns(started);
        stats
    }

    fn restart(&mut self, input: &InLanes<'_>) {
        self.reset();
        self.generation = Some(input.generation);
        let origin = self.radar_index(input.first_index);
        self.assembler.restart(origin);
    }

    fn radar_index(&self, input_index: u64) -> u64 {
        (input_index as f64 * self.radar_rate / self.input_rate).round() as u64
    }

    fn input_samples(&self, radar_samples: usize) -> u64 {
        (radar_samples as f64 * self.input_rate / self.radar_rate).round() as u64
    }

    fn process_chunk(&mut self, lanes: &[&[C32]], range: std::ops::Range<usize>) {
        for ((ddc, out), lane) in self.ddcs.iter_mut().zip(&mut self.decimated).zip(lanes) {
            ddc.process(&lane[range.clone()], out);
        }
        self.cleaner.process(&self.decimated[0], &mut self.cleaned);
        self.assembler.push(0, &self.cleaned);
        for lane in 1..self.lanes {
            if self.cancellers.is_empty() {
                self.assembler.push(lane, &self.decimated[lane]);
                continue;
            }
            let surveillance = match self.delays.get_mut(lane - 1) {
                Some(delay) => {
                    delay.process(&self.decimated[lane], &mut self.delayed);
                    &self.delayed
                }
                None => &self.decimated[lane],
            };
            let count = surveillance.len().min(self.cleaned.len());
            self.residual.resize(count, C32::default());
            self.cancellers[lane - 1].process(
                &self.cleaned[..count],
                &surveillance[..count],
                &mut self.residual,
            );
            self.assembler.push(lane, &self.residual);
        }
    }

    fn emit(&mut self, sink: &mut impl JobSink, stats: &mut FrontStats) {
        while self.assembler.ready() {
            let seq = self.seq;
            self.seq += 1;
            let Some(mut job) = sink.take() else {
                self.assembler.skip();
                sink.dropped();
                stats.dropped += 1;
                continue;
            };
            let reference = self.cleaner.health();
            let suppression = self.suppression();
            let generation = self.generation.unwrap_or(0);
            let resets = self.canceller_resets();
            let fresh_resets = resets.saturating_sub(self.delivered_resets);
            let assembler = &mut self.assembler;
            let filled = Arc::get_mut(&mut job).is_some_and(|slot| {
                if !assembler.fill(slot) {
                    return false;
                }
                slot.generation = generation;
                slot.seq = seq;
                slot.reference = reference;
                slot.front_suppression_db = suppression;
                slot.canceller_resets = fresh_resets;
                true
            });
            if filled {
                self.delivered_resets = resets;
                sink.submit(job);
                stats.submitted += 1;
            } else {
                self.assembler.skip();
                sink.dropped();
                stats.dropped += 1;
            }
        }
    }

    fn canceller_resets(&self) -> u64 {
        self.cancellers.iter().map(Canceller::resets).sum()
    }

    fn suppression(&self) -> [f32; MAX_SURVEILLANCE] {
        let mut out = [0.0; MAX_SURVEILLANCE];
        for (slot, canceller) in out.iter_mut().zip(&self.cancellers) {
            *slot = canceller.suppression_db();
        }
        out
    }
}

fn chunk_out(input_rate: f64, radar_rate: f64) -> usize {
    (FRONT_CHUNK as f64 * radar_rate / input_rate).ceil() as usize + RESAMPLER_SLACK
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
