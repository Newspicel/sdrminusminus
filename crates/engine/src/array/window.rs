use num_complex::Complex;
use sdrmm_channels::array_processor::{MAX_LANES, ResetCause};
use sdrmm_device::{GapScope, LaneMark, UNKNOWN_ERROR};
use sdrmm_wire::{ProcessorGate, SyncState};

use super::{
    AggregatorEvent, LiveFrame,
    align::{AlignNote, AlignNotes},
    board::StatusBoard,
};

pub(crate) const PRE_GUARD: u64 = 4_096;
pub(crate) const SUB_BLOCK: usize = 1_024;
pub(crate) const ONSET_RATIO: f32 = 4.0;
pub(crate) const ONSET_SETTLE: u64 = 1_024;
pub(crate) const BASELINE_SUBS: usize = 8;
pub(crate) const ONSET_REACH_S: f64 = 0.05;
pub(crate) const PLL_SETTLE_S: f64 = 0.005;
pub(crate) const WINDOW_EVENTS: usize = 32;
const HISTORY: usize = 16;
const CLOSED: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlankCause {
    Retune,
    Gain,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Interval {
    from: u64,
    until: u64,
    gate: ProcessorGate,
}

impl Interval {
    const fn covers(&self, index: u64) -> bool {
        index >= self.from && index < self.until
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Noise {
    Idle,
    Seeking {
        gate_from: u64,
        mark: u64,
        deadline: u64,
        baseline: Option<[f32; MAX_LANES]>,
    },
    On {
        sum: [f64; MAX_LANES],
        subs: u32,
    },
    Unseen,
    Ending {
        search_from: u64,
        limit: u64,
        off: u64,
        level: Option<[f32; MAX_LANES]>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Observation {
    pub(crate) discontinuity: bool,
    pub(crate) reset: Option<ResetCause>,
    pub(crate) blank_began: Option<BlankCause>,
    pub(crate) blank_ended: Option<BlankCause>,
    pub(crate) noise_began: Option<u64>,
    pub(crate) noise_ended: Option<u64>,
    pub(crate) lost: u32,
}

impl Observation {
    fn reset(&mut self, cause: ResetCause) {
        self.discontinuity = true;
        if self.reset.is_none() || cause == ResetCause::Realigned {
            self.reset = Some(cause);
        }
    }
}

pub(crate) struct Windows {
    lanes: usize,
    rate: f64,
    acc: [f64; MAX_LANES],
    acc_len: usize,
    acc_start: u64,
    history: [[f32; MAX_LANES]; HISTORY],
    history_end: [u64; HISTORY],
    head: usize,
    filled: usize,
    noise: Noise,
    noise_gate: Option<Interval>,
    reference: Option<(u64, u64)>,
    blank: Option<(Interval, BlankCause)>,
    closed: [Option<Interval>; CLOSED],
    closed_at: usize,
    events: [Option<AggregatorEvent>; WINDOW_EVENTS],
    event_count: usize,
    events_lost: u64,
    block_energy: [f32; MAX_LANES],
    block_peak: [f32; MAX_LANES],
}

impl Windows {
    pub(crate) const fn new(lanes: usize, rate: f64) -> Self {
        Self {
            lanes: if lanes < MAX_LANES { lanes } else { MAX_LANES },
            rate,
            acc: [0.0; MAX_LANES],
            acc_len: 0,
            acc_start: 0,
            history: [[0.0; MAX_LANES]; HISTORY],
            history_end: [0; HISTORY],
            head: 0,
            filled: 0,
            noise: Noise::Idle,
            noise_gate: None,
            reference: None,
            blank: None,
            closed: [None; CLOSED],
            closed_at: 0,
            events: [None; WINDOW_EVENTS],
            event_count: 0,
            events_lost: 0,
            block_energy: [0.0; MAX_LANES],
            block_peak: [0.0; MAX_LANES],
        }
    }

    pub(crate) const fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
    }

    pub(crate) fn observe(
        &mut self,
        notes: &AlignNotes,
        lanes: &[&[Complex<f32>]],
        index: u64,
        frame: &LiveFrame,
        board: &StatusBoard,
    ) -> Observation {
        let mut seen = Observation::default();
        for note in notes.as_slice() {
            self.note(*note, frame, board, &mut seen);
        }
        if notes.forced() {
            self.lose(u32::MAX, UNKNOWN_ERROR, GapScope::Device, board, &mut seen);
        }
        if seen.discontinuity {
            self.acc_len = 0;
        }
        self.powers(lanes, index, &mut seen);
        let end = index + lanes.first().map_or(0, |lane| lane.len()) as u64;
        self.advance(end, &mut seen);
        seen
    }

    fn note(
        &mut self,
        note: AlignNote,
        frame: &LiveFrame,
        board: &StatusBoard,
        seen: &mut Observation,
    ) {
        match note {
            AlignNote::Gap { lane, missing, .. } => {
                if let Some(lane) = board.lane(usize::from(lane)) {
                    lane.add_gap(missing);
                }
                seen.reset(ResetCause::Gap);
            }
            AlignNote::Realigned { .. } => seen.reset(ResetCause::Realigned),
            AlignNote::Uncertain {
                lane, error, scope, ..
            } => {
                if let Some(held) = board.lane(usize::from(lane)) {
                    held.add_uncertain();
                }
                if scope == GapScope::Device && frame.one_device() {
                    seen.reset(ResetCause::Gap);
                } else {
                    let mut affected = 0u32;
                    for other in 0..self.lanes {
                        let same = other == usize::from(lane)
                            || (scope == GapScope::Device
                                && frame.same_device(other, usize::from(lane)));
                        if same {
                            affected |= 1 << other;
                        }
                    }
                    self.lose(affected, error, scope, board, seen);
                }
            }
            AlignNote::Mark { at, mark, .. } => match mark {
                LaneMark::NoiseSource {
                    on: true,
                    in_flight,
                } => self.noise_on(at, in_flight),
                LaneMark::NoiseSource {
                    on: false,
                    in_flight,
                } => self.noise_off(at, in_flight),
                LaneMark::Retuned { in_flight } => {
                    self.blank(at, in_flight, BlankCause::Retune, seen);
                }
                LaneMark::GainChanged { in_flight } => {
                    self.blank(at, in_flight, BlankCause::Gain, seen);
                }
            },
        }
    }

    fn lose(
        &mut self,
        affected: u32,
        error: u64,
        scope: GapScope,
        board: &StatusBoard,
        seen: &mut Observation,
    ) {
        for lane in 0..self.lanes {
            let bit = 1u32 << lane;
            if affected & bit == 0 || seen.lost & bit != 0 {
                continue;
            }
            seen.lost |= bit;
            if let Some(held) = board.lane(lane) {
                held.set_sync(SyncState::Lost);
            }
            self.push_event(AggregatorEvent::Uncertain { lane, error, scope });
        }
        board.set_sync(SyncState::Lost);
        seen.reset(ResetCause::Realigned);
    }

    fn noise_on(&mut self, mark: u64, in_flight: u64) {
        if !matches!(self.noise, Noise::Idle | Noise::Ending { .. }) {
            return;
        }
        let gate_from = mark.saturating_sub(PRE_GUARD);
        let reach = (ONSET_REACH_S * self.rate).max(0.0) as u64;
        self.noise = Noise::Seeking {
            gate_from,
            mark,
            deadline: mark.saturating_add(in_flight).saturating_add(reach),
            baseline: self.baseline(gate_from),
        };
        let from = match self.noise_gate.take() {
            Some(open) if open.until > gate_from => open.from.min(gate_from),
            Some(previous) => {
                self.close(previous);
                gate_from
            }
            None => gate_from,
        };
        self.noise_gate = Some(Interval {
            from,
            until: u64::MAX,
            gate: ProcessorGate::Calibrating,
        });
        self.reference = None;
    }

    fn noise_off(&mut self, mark: u64, in_flight: u64) {
        let level = match self.noise {
            Noise::On { sum, subs } if subs > 0 => {
                let mut level = [0.0; MAX_LANES];
                for (level, sum) in level.iter_mut().zip(sum) {
                    *level = (sum / f64::from(subs)) as f32;
                }
                Some(level)
            }
            Noise::On { .. } | Noise::Seeking { .. } | Noise::Unseen => None,
            Noise::Idle | Noise::Ending { .. } => return,
        };
        let search_from = mark.saturating_sub(PRE_GUARD);
        self.noise = Noise::Ending {
            search_from,
            limit: mark.saturating_add(in_flight),
            off: mark,
            level,
        };
        if let Some((from, until)) = self.reference {
            self.reference = Some((from, until.min(search_from)));
        }
    }

    fn blank(&mut self, mark: u64, in_flight: u64, cause: BlankCause, seen: &mut Observation) {
        let settle = (PLL_SETTLE_S * self.rate).max(0.0) as u64;
        let from = mark.saturating_sub(PRE_GUARD);
        let until = mark.saturating_add(in_flight).saturating_add(settle);
        let (interval, cause) = match self.blank {
            Some((held, held_cause)) => (
                Interval {
                    from: held.from.min(from),
                    until: held.until.max(until),
                    gate: ProcessorGate::Retuning,
                },
                if held_cause == BlankCause::Gain || cause == BlankCause::Gain {
                    BlankCause::Gain
                } else {
                    BlankCause::Retune
                },
            ),
            None => (
                Interval {
                    from,
                    until,
                    gate: ProcessorGate::Retuning,
                },
                cause,
            ),
        };
        self.blank = Some((interval, cause));
        seen.blank_began = Some(cause);
    }

    fn baseline(&self, before: u64) -> Option<[f32; MAX_LANES]> {
        let mut sum = [0.0f64; MAX_LANES];
        let mut count = 0u32;
        for back in 0..self.filled {
            if count as usize >= BASELINE_SUBS {
                break;
            }
            let slot = (self.head + HISTORY - 1 - back) % HISTORY;
            if self.history_end[slot] > before {
                continue;
            }
            for (sum, power) in sum.iter_mut().zip(&self.history[slot]) {
                *sum += f64::from(*power);
            }
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let mut baseline = [0.0; MAX_LANES];
        for (baseline, sum) in baseline.iter_mut().zip(sum) {
            *baseline = (sum / f64::from(count)) as f32;
        }
        Some(baseline)
    }

    fn powers(&mut self, lanes: &[&[Complex<f32>]], index: u64, seen: &mut Observation) {
        let count = lanes.first().map_or(0, |lane| lane.len());
        self.block_energy = [0.0; MAX_LANES];
        self.block_peak = [0.0; MAX_LANES];
        let mut at = 0;
        while at < count {
            if self.acc_len == 0 {
                self.acc_start = index + at as u64;
                self.acc = [0.0; MAX_LANES];
            }
            let take = (SUB_BLOCK - self.acc_len).min(count - at);
            let totals = self.block_energy.iter_mut().zip(self.block_peak.iter_mut());
            for ((acc, lane), (total, peak)) in self.acc.iter_mut().zip(lanes).zip(totals) {
                let (energy, loudest) = energy_and_peak(&lane[at..at + take]);
                *acc += f64::from(energy);
                *total += energy;
                *peak = peak.max(loudest);
            }
            self.acc_len += take;
            at += take;
            if self.acc_len == SUB_BLOCK {
                let mut power = [0.0f32; MAX_LANES];
                for (power, acc) in power.iter_mut().zip(&self.acc) {
                    *power = (acc / SUB_BLOCK as f64) as f32;
                }
                self.acc_len = 0;
                self.sub_block(self.acc_start, power, seen);
            }
        }
    }

    fn sub_block(&mut self, start: u64, power: [f32; MAX_LANES], seen: &mut Observation) {
        let end = start + SUB_BLOCK as u64;
        self.history[self.head] = power;
        self.history_end[self.head] = end;
        self.head = (self.head + 1) % HISTORY;
        self.filled = (self.filled + 1).min(HISTORY);
        match self.noise {
            Noise::Seeking {
                gate_from,
                mark,
                deadline,
                baseline,
            } if start >= gate_from => match baseline {
                Some(baseline) => self.seek(start, end, deadline, &power, &baseline, seen),
                None if end <= deadline => {
                    self.noise = Noise::Seeking {
                        gate_from,
                        mark,
                        deadline,
                        baseline: Some(power),
                    };
                }
                None => self.noise_unseen(),
            },
            Noise::On { mut sum, subs } => {
                if self.reference.is_some_and(|(from, _)| start >= from) {
                    for (sum, power) in sum.iter_mut().zip(power) {
                        *sum += f64::from(power);
                    }
                    self.noise = Noise::On {
                        sum,
                        subs: subs + 1,
                    };
                }
            }
            Noise::Ending {
                search_from,
                limit,
                off,
                level,
            } if start >= search_from => {
                let dropped = level.is_some_and(|level| {
                    self.majority(|lane| power[lane] < level[lane] / ONSET_RATIO)
                });
                if dropped {
                    self.end_noise(start, start + ONSET_SETTLE, seen);
                    if let Some((from, until)) = self.reference {
                        self.reference = Some((from, until.max(off.min(start))));
                    }
                } else if end > limit {
                    self.end_noise(limit, limit, seen);
                }
            }
            _ => {}
        }
    }

    fn seek(
        &mut self,
        start: u64,
        end: u64,
        deadline: u64,
        power: &[f32; MAX_LANES],
        baseline: &[f32; MAX_LANES],
        seen: &mut Observation,
    ) {
        if self.majority(|lane| power[lane] > ONSET_RATIO * baseline[lane]) {
            self.push_event(AggregatorEvent::NoiseOnset { at: start });
            seen.noise_began = Some(start);
            self.reference = Some((start + ONSET_SETTLE, u64::MAX));
            self.noise = Noise::On {
                sum: [0.0; MAX_LANES],
                subs: 0,
            };
        } else if end > deadline {
            self.noise_unseen();
        }
    }

    fn noise_unseen(&mut self) {
        self.push_event(AggregatorEvent::NoiseNotSeen);
        self.noise = Noise::Unseen;
    }

    fn end_noise(&mut self, at: u64, gate_until: u64, seen: &mut Observation) {
        self.push_event(AggregatorEvent::NoiseEnded { at });
        seen.noise_ended = Some(at);
        if let Some(gate) = self.noise_gate.as_mut() {
            gate.until = gate_until.max(gate.from);
        }
        self.noise = Noise::Idle;
    }

    fn majority(&self, test: impl Fn(usize) -> bool) -> bool {
        let hits = (0..self.lanes).filter(|lane| test(*lane)).count();
        self.lanes > 0 && hits * 2 >= self.lanes
    }

    fn advance(&mut self, end: u64, seen: &mut Observation) {
        if let Some((interval, cause)) = self.blank
            && interval.until <= end
        {
            self.push_event(AggregatorEvent::BlankEnded {
                cause,
                at: interval.until,
            });
            seen.blank_ended = Some(cause);
            self.close(interval);
            self.blank = None;
        }
        if let Some(gate) = self.noise_gate
            && gate.until <= end
        {
            self.close(gate);
            self.noise_gate = None;
        }
    }

    fn close(&mut self, interval: Interval) {
        self.closed[self.closed_at] = Some(interval);
        self.closed_at = (self.closed_at + 1) % CLOSED;
    }

    fn intervals(&self) -> impl Iterator<Item = Interval> + '_ {
        self.blank
            .map(|(interval, _)| interval)
            .into_iter()
            .chain(self.noise_gate)
            .chain(self.closed.iter().flatten().copied())
    }

    pub(crate) fn gate_at(&self, index: u64) -> Option<ProcessorGate> {
        let mut gate = None;
        for interval in self.intervals().filter(|interval| interval.covers(index)) {
            if interval.gate == ProcessorGate::Retuning || gate.is_none() {
                gate = Some(interval.gate);
            }
        }
        gate
    }

    pub(crate) fn next_change(&self, index: u64) -> Option<u64> {
        self.intervals()
            .flat_map(|interval| [interval.from, interval.until])
            .filter(|edge| *edge > index)
            .min()
    }

    pub(crate) fn prune(&mut self, processed: u64) {
        for slot in &mut self.closed {
            if slot.is_some_and(|interval| interval.until <= processed) {
                *slot = None;
            }
        }
    }

    pub(crate) fn block_level(&self, lane: usize) -> (f32, f32) {
        (
            self.block_energy.get(lane).copied().unwrap_or(0.0),
            self.block_peak.get(lane).copied().unwrap_or(0.0),
        )
    }

    pub(crate) const fn reference(&self) -> Option<(u64, u64)> {
        self.reference
    }

    pub(crate) fn noise_active(&self) -> bool {
        self.noise != Noise::Idle
    }

    fn push_event(&mut self, event: AggregatorEvent) {
        match self.events.get_mut(self.event_count) {
            Some(slot) => {
                *slot = Some(event);
                self.event_count += 1;
            }
            None => self.events_lost += 1,
        }
    }

    pub(crate) fn drain_events(&mut self, mut deliver: impl FnMut(AggregatorEvent)) {
        for slot in &mut self.events[..self.event_count] {
            if let Some(event) = slot.take() {
                deliver(event);
            }
        }
        self.event_count = 0;
    }

    pub(crate) const fn take_events_lost(&mut self) -> u64 {
        let lost = self.events_lost;
        self.events_lost = 0;
        lost
    }
}

fn energy_and_peak(samples: &[Complex<f32>]) -> (f32, f32) {
    let (chunks, tail) = samples.as_chunks::<LEVEL_LANES>();
    let mut energy = [0.0f32; LEVEL_LANES];
    let mut peak_re = [0.0f32; LEVEL_LANES];
    let mut peak_im = [0.0f32; LEVEL_LANES];
    for chunk in chunks {
        for slot in 0..LEVEL_LANES {
            let sample = chunk[slot];
            energy[slot] += sample.re * sample.re + sample.im * sample.im;
            peak_re[slot] = peak_re[slot].max(sample.re.abs());
            peak_im[slot] = peak_im[slot].max(sample.im.abs());
        }
    }
    let mut total = energy.iter().sum::<f32>();
    let mut peak = peak_re
        .iter()
        .chain(&peak_im)
        .copied()
        .fold(0.0f32, f32::max);
    for sample in tail {
        total += sample.norm_sqr();
        peak = peak.max(sample.re.abs()).max(sample.im.abs());
    }
    (total, peak)
}

const LEVEL_LANES: usize = 8;

#[cfg(test)]
mod tests;
