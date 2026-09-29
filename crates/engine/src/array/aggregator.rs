use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{JoinHandle, Thread},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_channels::array_processor::{ArrayBlock, CalView, MAX_LANES, Pose, ResetCause};
use sdrmm_device::{Latency, now_ns, schedule::claim};
use sdrmm_dsp::IqDcBlocker;
use sdrmm_wire::{ArrayOrientation, CalPhase, ProcessorGate, SyncState};

use super::{
    AggregatorEvent, CONTROL_EVENT_SLOTS, Command, LiveFrame, PoseSample, Retired,
    align::{AlignNotes, Aligner},
    board::StatusBoard,
    capture::{
        CAPTURE_MAX, CAPTURE_POOL, CalQuality, CaptureBuffers, CaptureJob, CaptureSlot,
        FillContext, Filled, Solution, SolveFailure, SolveSummary,
    },
    correct::{CorrectionSet, CorrectionStage, Corrector, Label, StageJob},
    host::{GateInputs, HostList, ProcessorHost},
    record::{RecordNote, RecordTap},
    tap::LaneFeed,
    window::{BlankCause, Observation, Windows},
    worker::{SyncLink, WorkerIo, link},
};
use crate::{EngineError, runtime::retire::Reclaimer};

pub(crate) const PARK: Duration = Duration::from_millis(2);
const POSE_SLOTS: usize = 64;
const POSE_HOLD_NS: i64 = 2_000_000_000;
const SPILL: usize = 8;
const RETIRE_ROOM: usize = MAX_LANES + 4;
const SOLUTION_SLOTS: usize = 4;
const JOB_SLOTS: usize = 2;
const COMMANDS_PER_STEP: usize = 16;
const CLIP_LEVEL: f32 = 0.999;
const LEVEL_FLOOR: f32 = 1e-20;
const DC_BINS: f64 = 4_096.0;
const NANOS_PER_SECOND: f64 = 1e9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Worked,
    Idle,
}

pub(crate) struct PoseRing {
    samples: [Option<PoseSample>; POSE_SLOTS],
    next: usize,
}

impl PoseRing {
    pub(crate) const fn new() -> Self {
        Self {
            samples: [None; POSE_SLOTS],
            next: 0,
        }
    }

    pub(crate) fn push(&mut self, sample: PoseSample) {
        self.samples[self.next] = Some(sample);
        self.next = (self.next + 1) % POSE_SLOTS;
    }

    pub(crate) fn at(&self, host_ns: i64) -> Pose {
        let mut before: Option<PoseSample> = None;
        let mut after: Option<PoseSample> = None;
        for sample in self.samples.iter().flatten() {
            if sample.host_ns <= host_ns {
                if before.is_none_or(|held| sample.host_ns >= held.host_ns) {
                    before = Some(*sample);
                }
            } else if after.is_none_or(|held| sample.host_ns < held.host_ns) {
                after = Some(*sample);
            }
        }
        match (before, after) {
            (Some(early), Some(late)) => between(&early, &late, host_ns),
            (Some(newest), None) => ahead_of(&newest, host_ns - newest.host_ns),
            (None, Some(oldest)) => pose_of(&oldest),
            (None, None) => Pose::default(),
        }
    }
}

fn pose_of(sample: &PoseSample) -> Pose {
    Pose {
        heading_deg: sample.heading_deg,
        heading_sigma_deg: sample.heading_sigma_deg,
        yaw_rate_dps: sample.yaw_rate_dps,
        fix: sample.fix,
        moving: sample.moving,
        follows: false,
    }
}

fn ahead_of(newest: &PoseSample, ahead_ns: i64) -> Pose {
    let mut pose = pose_of(newest);
    if ahead_ns > POSE_HOLD_NS {
        pose.heading_deg = None;
        pose.yaw_rate_dps = None;
    } else if let (Some(heading), Some(rate)) = (pose.heading_deg, pose.yaw_rate_dps) {
        let turned = f64::from(rate) * ahead_ns as f64 / NANOS_PER_SECOND;
        pose.heading_deg = Some((heading + turned).rem_euclid(360.0));
    }
    pose
}

fn between(early: &PoseSample, late: &PoseSample, host_ns: i64) -> Pose {
    let span = (late.host_ns - early.host_ns).max(1) as f64;
    let fraction = ((host_ns - early.host_ns) as f64 / span).clamp(0.0, 1.0);
    let heading_deg = match (early.heading_deg, late.heading_deg) {
        (Some(from), Some(to)) => {
            let turn = (to - from + 540.0).rem_euclid(360.0) - 180.0;
            Some((from + turn * fraction).rem_euclid(360.0))
        }
        (from, to) => from.or(to),
    };
    let sigma = f64::from(early.heading_sigma_deg)
        + (f64::from(late.heading_sigma_deg) - f64::from(early.heading_sigma_deg)) * fraction;
    Pose {
        heading_deg,
        heading_sigma_deg: sigma as f32,
        yaw_rate_dps: late.yaw_rate_dps,
        fix: early.fix.or(late.fix),
        moving: early.moving || late.moving,
        follows: false,
    }
}

#[derive(Default)]
pub(crate) struct AggregatorExit {
    pub(crate) feeds: Vec<Option<LaneFeed>>,
    #[expect(clippy::vec_box)]
    pub(crate) hosts: Vec<Box<ProcessorHost>>,
}

pub(crate) struct AggregatorIo {
    events: Producer<AggregatorEvent>,
    jobs: Producer<CaptureJob>,
    solutions: Consumer<Box<Solution>>,
    buffers: Consumer<Box<CaptureBuffers>>,
    sets: Producer<Box<CorrectionSet>>,
    reclaimer: Reclaimer<Retired>,
    spill: [Option<Retired>; SPILL],
}

pub(crate) struct Wiring {
    pub(crate) aggregator: AggregatorIo,
    pub(crate) worker: WorkerIo,
    pub(crate) events: Consumer<AggregatorEvent>,
    pub(crate) link: SyncLink,
}

pub(crate) fn wire(
    lanes: usize,
    sample_rate: f64,
    stop: Arc<AtomicBool>,
) -> Result<Wiring, EngineError> {
    let (events_tx, events_rx) = RingBuffer::new(CONTROL_EVENT_SLOTS);
    let (jobs_tx, jobs_rx) = RingBuffer::new(JOB_SLOTS);
    let (solutions_tx, solutions_rx) = RingBuffer::new(SOLUTION_SLOTS);
    let (mut buffers_tx, buffers_rx) = RingBuffer::new(CAPTURE_POOL);
    let (mut sets_tx, sets_rx) = RingBuffer::new(CAPTURE_POOL);
    for _ in 0..CAPTURE_POOL {
        let _ = buffers_tx.push(Box::new(CaptureBuffers::new(lanes, CAPTURE_MAX)));
        let _ = sets_tx.push(Box::new(CorrectionSet::identity(lanes)));
    }
    let reclaimer = Reclaimer::new(Retired::release)
        .map_err(|error| EngineError::Processor(format!("start array reclaimer: {error}")))?;
    let (link, orders, reports) = link();
    Ok(Wiring {
        aggregator: AggregatorIo {
            events: events_tx,
            jobs: jobs_tx,
            solutions: solutions_rx,
            buffers: buffers_rx,
            sets: sets_tx,
            reclaimer,
            spill: [const { None }; SPILL],
        },
        worker: WorkerIo {
            lanes,
            sample_rate,
            jobs: jobs_rx,
            solutions: solutions_tx,
            buffers: buffers_tx,
            sets: sets_rx,
            stop,
            orders,
            reports,
        },
        events: events_rx,
        link,
    })
}

struct DcBank {
    blockers: [IqDcBlocker; MAX_LANES],
}

impl DcBank {
    fn new(rate: f64) -> Self {
        let rate = if rate.is_finite() && rate > 1.0 {
            rate
        } else {
            1.0
        };
        let corner = (rate / DC_BINS * 0.25).clamp(1.0, 500.0);
        Self {
            blockers: std::array::from_fn(|_| IqDcBlocker::new(rate, corner)),
        }
    }

    fn reset(&mut self) {
        for blocker in &mut self.blockers {
            blocker.reset();
        }
    }

    fn process(&mut self, lanes: &mut [Vec<Complex<f32>>], from: usize, to: usize, hold: bool) {
        for (blocker, lane) in self.blockers.iter_mut().zip(lanes) {
            let to = to.min(lane.len());
            let Some(span) = lane.get_mut(from..to) else {
                continue;
            };
            if hold {
                blocker.hold(span);
            } else {
                blocker.process(span);
            }
        }
    }
}

struct Timing {
    wall_offset_ns: i64,
}

impl Timing {
    fn new() -> Self {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_nanos()).unwrap_or(i64::MAX)
            });
        let host = i64::try_from(now_ns()).unwrap_or(i64::MAX);
        Self {
            wall_offset_ns: wall.saturating_sub(host),
        }
    }
}

pub(crate) struct Aggregator {
    aligner: Aligner,
    windows: Windows,
    capture: CaptureSlot,
    corrector: Corrector,
    dc: DcBank,
    hosts: HostList,
    frame: Box<LiveFrame>,
    board: Arc<StatusBoard>,
    commands: Consumer<Command>,
    io: AggregatorIo,
    worker: Option<Thread>,
    generation: u32,
    corrected_index: u64,
    record: Option<Box<RecordTap>>,
    poses: PoseRing,
    notes: AlignNotes,
    timing: Timing,
    origins: [Option<i64>; MAX_LANES],
    correcting: bool,
    gap_before: bool,
    phase_ready: bool,
    gain_ready: bool,
    quality: CalQuality,
    clipped: u32,
    feed_losses: u64,
    priors: u32,
    stage: Option<CorrectionStage>,
    stage_dropped: u64,
}

impl Aggregator {
    pub(crate) fn new(
        feeds: Vec<Option<LaneFeed>>,
        frame: Box<LiveFrame>,
        board: Arc<StatusBoard>,
        commands: Consumer<Command>,
        io: AggregatorIo,
        worker: Option<Thread>,
        stage: Option<CorrectionStage>,
    ) -> Self {
        let lanes = feeds.len().min(MAX_LANES);
        Self {
            aligner: Aligner::new(feeds),
            windows: Windows::new(lanes, frame.sample_rate),
            capture: CaptureSlot::new(lanes),
            corrector: Corrector::new(lanes),
            dc: DcBank::new(frame.sample_rate),
            hosts: HostList::new(),
            board,
            commands,
            io,
            worker,
            generation: 0,
            corrected_index: 0,
            record: None,
            poses: PoseRing::new(),
            notes: AlignNotes::new(),
            timing: Timing::new(),
            origins: [None; MAX_LANES],
            correcting: false,
            gap_before: true,
            phase_ready: false,
            gain_ready: false,
            quality: CalQuality::default(),
            clipped: 0,
            feed_losses: 0,
            priors: foreign_lanes(&frame),
            frame,
            stage,
            stage_dropped: 0,
        }
    }

    pub(crate) fn step(&mut self) -> Step {
        self.flush_spill();
        self.drain_commands();
        self.drain_solutions();
        if self.priors != 0 {
            self.seed_offsets();
        }
        let step = match self.aligner.next(&mut self.notes) {
            Some(count) => {
                self.block(count);
                Step::Worked
            }
            None => {
                if self.aligner.has_lost_lane() {
                    self.hosts.hold(ProcessorGate::Sync, 0);
                }
                if self.drain_stage() {
                    Step::Worked
                } else {
                    Step::Idle
                }
            }
        };
        self.hosts.poll(self.frame.center_hz);
        self.counters();
        step
    }

    fn seed_offsets(&mut self) {
        self.aligner.settle(&mut self.notes);
        self.aligner.origins(&mut self.origins);
        let rate = self.frame.sample_rate;
        let Some(reference) = self.origins[0] else {
            return;
        };
        if !(rate.is_finite() && rate > 0.0) {
            return;
        }
        let lanes = self.aligner.lanes().min(MAX_LANES);
        let mut offsets = offsets_of(self.aligner.offsets());
        for (lane, origin) in self.origins.iter().enumerate().take(lanes).skip(1) {
            let bit = 1u32 << lane;
            if let Some(origin) = origin.filter(|_| self.priors & bit != 0) {
                let apart = reference.saturating_sub(origin) as f64;
                offsets[lane] = (apart * rate / NANOS_PER_SECOND).round() as i64;
                self.priors &= !bit;
            }
        }
        self.aligner.set_offsets(&offsets[..lanes]);
    }

    fn block(&mut self, count: usize) {
        let index = self.aligner.index();
        self.board.add_aligned(count as u64);
        self.board.add_events_lost(u64::from(self.notes.dropped()));
        let seen = {
            let (windows, notes, frame, board) =
                (&mut self.windows, &self.notes, &*self.frame, &*self.board);
            self.aligner.with_raw(count, |raw| {
                windows.observe(notes, raw, index, frame, board)
            })
        };
        self.notes.clear();
        self.forward_events();
        self.react(&seen);
        self.meter(count);
        self.record_block(count, index, &seen);
        if self.frame.dc_block {
            self.block_dc(count, index);
        }
        self.fill_capture(count, index, &seen);
        self.dispatch(count, index);
        self.windows.prune(self.corrected_index);
    }

    fn forward_events(&mut self) {
        let (windows, events, board) = (&mut self.windows, &mut self.io.events, &*self.board);
        windows.drain_events(|event| {
            if events.push(event).is_err() {
                board.add_events_lost(1);
            }
        });
        let lost = self.windows.take_events_lost();
        self.board.add_events_lost(lost);
    }

    fn event(&mut self, event: AggregatorEvent) {
        if self.io.events.push(event).is_err() {
            self.board.add_events_lost(1);
        }
    }

    fn react(&mut self, seen: &Observation) {
        if seen.discontinuity {
            self.discontinuity();
        }
        if let Some(cause) = seen.reset {
            self.hosts.reset(cause);
        }
        if let Some(cause) = seen.blank_began {
            self.stale(cause);
        }
        if seen.blank_ended.is_some() {
            self.bump_generation();
            self.discontinuity();
        }
    }

    fn block_dc(&mut self, count: usize, index: u64) {
        let end = index + count as u64;
        let mut at = index;
        while at < end {
            let hold = self.windows.gate_at(at) == Some(ProcessorGate::Calibrating);
            let until = self
                .windows
                .next_change(at)
                .map_or(end, |edge| edge.min(end));
            let (from, to) = ((at - index) as usize, (until - index) as usize);
            self.dc.process(self.aligner.raw_mut(), from, to, hold);
            at = until;
        }
    }

    fn discontinuity(&mut self) {
        self.corrector.reset();
        if let Some(stage) = self.stage.as_mut() {
            stage.reset();
        }
        self.dc.reset();
        self.gap_before = true;
    }

    fn stale(&mut self, cause: BlankCause) {
        let calibrated =
            self.phase_ready || matches!(self.board.cal(), CalPhase::Solved | CalPhase::Warm);
        if !calibrated || (cause == BlankCause::Retune && self.frame.keeps_phase) {
            return;
        }
        self.phase_ready = false;
        self.board.phase_ready.store(false, Ordering::Relaxed);
        self.board.set_cal(CalPhase::Stale);
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.corrector.set_generation(self.generation);
    }

    fn meter(&mut self, count: usize) {
        let mut clipped = 0u32;
        for lane in 0..self.aligner.lanes() {
            let (energy, peak) = self.windows.block_level(lane);
            let clip = peak >= CLIP_LEVEL;
            let power = energy / count.max(1) as f32;
            let level = 10.0 * power.max(LEVEL_FLOOR).log10();
            if let Some(board) = self.board.lane(lane) {
                board.set_level(level, clip);
            }
            if clip {
                clipped |= 1 << lane;
            }
        }
        let fresh = clipped & !self.clipped;
        self.clipped = clipped;
        for lane in 0..MAX_LANES {
            if fresh & (1 << lane) != 0 {
                self.event(AggregatorEvent::Clipped { lane });
            }
        }
    }

    fn record_block(&mut self, count: usize, index: u64, seen: &Observation) {
        let Some(record) = self.record.as_mut() else {
            return;
        };
        let mut notes = [RecordNote::Start { at: 0 }; 2];
        let mut held = 0;
        if let Some(at) = seen.noise_began {
            notes[held] = RecordNote::NoiseOn { at };
            held += 1;
        }
        if let Some(at) = seen.noise_ended {
            notes[held] = RecordNote::NoiseOff { at };
            held += 1;
        }
        self.aligner
            .with_raw(count, |raw| record.push(raw, index, &notes[..held]));
    }

    fn fill_capture(&mut self, count: usize, index: u64, seen: &Observation) {
        if !self.capture.armed() {
            return;
        }
        let outcome = {
            let context = FillContext {
                windows: &self.windows,
                discontinuous: seen.discontinuity
                    || seen.blank_began.is_some()
                    || seen.blank_ended.is_some(),
                generation: self.generation,
                sample_rate: self.frame.sample_rate,
                offsets: self.aligner.offsets(),
                centers_hz: &self.frame.lane_centers_hz,
                devices: &self.frame.devices,
            };
            let capture = &mut self.capture;
            self.aligner
                .with_raw(count, |raw| capture.fill(raw, index, &context))
        };
        match outcome {
            Filled::Waiting => {}
            Filled::Aborted(id) => self.event(AggregatorEvent::CaptureRefused { id }),
            Filled::Done(job) => {
                let id = job.request.id;
                match self.io.jobs.push(job) {
                    Ok(()) => {
                        if let Some(worker) = &self.worker {
                            worker.unpark();
                        }
                        self.event(AggregatorEvent::Captured { id });
                    }
                    Err(PushError::Full(job)) => {
                        if let Some(extra) = self.capture.keep(job.buffers) {
                            self.retire(Retired::Buffers(extra));
                        }
                        self.event(AggregatorEvent::CaptureRefused { id });
                    }
                }
            }
        }
    }

    fn dispatch(&mut self, count: usize, index: u64) {
        let needs = self.hosts.needs_corrector();
        if needs != self.correcting {
            self.correcting = needs;
            self.corrector.reset();
            if let Some(stage) = self.stage.as_mut() {
                stage.reset();
            }
            self.gap_before = true;
            self.hosts.reset(ResetCause::Realigned);
        }
        self.aligner.origins(&mut self.origins);
        if self.correcting && self.stage.is_some() {
            self.stage_block(count, index);
            return;
        }
        let clock = self.clock();
        if self.correcting {
            let corrector = &mut self.corrector;
            let produced = self
                .aligner
                .with_raw(count, |raw| corrector.push(raw, index));
            if produced == 0 {
                return;
            }
        }
        let transient = self.corrector.transient();
        let mut run = HostRun {
            hosts: &mut self.hosts,
            windows: &self.windows,
            frame: &self.frame,
            board: &self.board,
            poses: &self.poses,
            timing: &self.timing,
            clock,
            corrector: &self.corrector,
            generation: self.generation,
            gap_before: &mut self.gap_before,
            phase_ready: self.phase_ready,
            gain_ready: self.gain_ready,
            quality: self.quality,
        };
        let end = if self.correcting {
            self.corrector.with_corrected(|lanes, first| {
                run.hosts(lanes, first, true, transient);
                first + lanes.first().map_or(0, |lane| lane.len()) as u64
            })
        } else {
            self.aligner.with_raw(count, |raw| {
                run.hosts(raw, index, false, 0);
                index + count as u64
            })
        };
        if self.correcting {
            self.corrector.consume();
        }
        self.corrected_index = end;
    }

    fn clock(&self) -> Option<(i64, i64)> {
        self.origins
            .iter()
            .zip(self.aligner.offsets())
            .find_map(|(origin, offset)| origin.map(|origin| (origin, *offset)))
    }

    fn stage_block(&mut self, count: usize, index: u64) {
        let label = Label {
            generation: self.generation,
            gap_before: std::mem::take(&mut self.gap_before),
            phase_ready: self.phase_ready,
            gain_ready: self.gain_ready,
            quality: self.quality,
        };
        let sent = match self.stage.as_mut() {
            Some(stage) => {
                let active = self.corrector.active();
                self.aligner
                    .with_raw(count, |raw| stage.submit(raw, index, label, active))
            }
            None => false,
        };
        if !sent {
            self.stage_dropped += count as u64;
            self.discontinuity();
        }
        self.drain_stage();
    }

    fn drain_stage(&mut self) -> bool {
        let mut worked = false;
        while let Some(job) = self.stage.as_mut().and_then(CorrectionStage::finished) {
            if self.correcting {
                self.run_job(&job);
            } else {
                self.stage_dropped += job.ready() as u64;
            }
            if let Some(stage) = self.stage.as_mut() {
                stage.keep(job);
            }
            worked = true;
        }
        worked
    }

    fn run_job(&mut self, job: &StageJob) {
        if job.ready() == 0 {
            return;
        }
        let label = job.label();
        let mut gap_before = label.gap_before;
        let clock = self.clock();
        let mut run = HostRun {
            hosts: &mut self.hosts,
            windows: &self.windows,
            frame: &self.frame,
            board: &self.board,
            poses: &self.poses,
            timing: &self.timing,
            clock,
            corrector: &self.corrector,
            generation: label.generation,
            gap_before: &mut gap_before,
            phase_ready: label.phase_ready,
            gain_ready: label.gain_ready,
            quality: label.quality,
        };
        let first = job.first_index();
        job.with_lanes(|lanes| run.hosts(lanes, first, true, job.transient()));
        self.corrected_index = first + job.ready() as u64;
        self.windows.prune(self.corrected_index);
    }

    fn counters(&mut self) {
        self.board
            .realigns
            .store(self.aligner.realigns(), Ordering::Relaxed);
        self.board
            .generation
            .store(self.generation, Ordering::Relaxed);
        self.board.dropped_samples.store(
            self.aligner.skipped() + self.stage_dropped,
            Ordering::Relaxed,
        );
        let losses = self.aligner.events_lost();
        if losses > self.feed_losses {
            self.board.add_events_lost(losses - self.feed_losses);
        }
        self.feed_losses = losses;
    }

    fn drain_commands(&mut self) {
        for _ in 0..COMMANDS_PER_STEP {
            if self.io.reclaimer.room() < RETIRE_ROOM || self.io.spill.iter().any(Option::is_some) {
                return;
            }
            let Ok(command) = self.commands.pop() else {
                return;
            };
            self.command(command);
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::SwapFeeds { mut slots } => {
                self.priors |= foreign_lanes(&self.frame);
                for (slot, feed) in slots.drain(..) {
                    self.swap_feed(slot, Some(feed));
                    if let Some(lane) = self.board.lane(slot) {
                        lane.set_sync(SyncState::Searching);
                    }
                }
                self.discontinuity();
                self.retire(Retired::Command(Command::SwapFeeds { slots }));
            }
            Command::LanesLost { slots } => {
                for slot in &slots {
                    self.swap_feed(*slot, None);
                    if let Some(lane) = self.board.lane(*slot) {
                        lane.set_sync(SyncState::Lost);
                    }
                }
                self.board.set_sync(SyncState::Lost);
                self.discontinuity();
                self.retire(Retired::Command(Command::LanesLost { slots }));
            }
            Command::AddHost { host } => {
                if let Err(host) = self.hosts.add(host) {
                    self.retire(Retired::Host(host));
                }
            }
            Command::ReplaceHost { host } => match self.hosts.replace(host) {
                Ok(Some(old)) | Err(old) => self.retire(Retired::Host(old)),
                Ok(None) => {}
            },
            Command::RemoveHost { node } => {
                if let Some(host) = self.hosts.remove(&node) {
                    self.retire(Retired::Host(host));
                }
                self.retire(Retired::Node(node));
            }
            Command::ApplyParams { node, params } => {
                let left = match self.hosts.find_mut(&node) {
                    Some(host) => host.apply(params),
                    None => Some(params),
                };
                if let Some(params) = left {
                    self.retire(Retired::Params(params));
                }
                self.retire(Retired::Node(node));
            }
            Command::SwapVirtual { node, port, sink } => {
                let left = match self.hosts.find_mut(&node) {
                    Some(host) => host.set_sink(port, sink),
                    None => Some(sink),
                };
                if let Some(sink) = left {
                    self.retire(Retired::Sink(sink));
                }
                self.retire(Retired::Node(node));
            }
            Command::Action { node, action } => {
                if let Some(host) = self.hosts.find_mut(&node) {
                    host.action(action);
                }
                self.retire(Retired::Node(node));
            }
            Command::Frame { frame } => self.swap_frame(frame),
            Command::Capture { request } => {
                if let Some(stale) = self.capture.cancel() {
                    self.event(AggregatorEvent::CaptureRefused { id: stale });
                }
                if !self.capture.arm(request, &mut self.io.buffers) {
                    self.event(AggregatorEvent::CaptureRefused { id: request.id });
                }
            }
            Command::Recalibrate => self.recalibrate(),
            Command::Record { writer } => self.swap_record(writer),
            Command::Pose { sample } => self.poses.push(sample),
            Command::CommitDedicated { node, prepared } => {
                let left = match self.hosts.find_mut(&node) {
                    Some(host) => host.commit(prepared).map(Retired::Runner),
                    None => Some(Retired::Prepared(prepared)),
                };
                if let Some(left) = left {
                    self.retire(left);
                }
                self.retire(Retired::Node(node));
            }
            #[cfg(any(test, feature = "probe"))]
            Command::Hold(hold) => hold(),
        }
    }

    fn swap_feed(&mut self, slot: usize, feed: Option<LaneFeed>) {
        if let Some(left) = self.aligner.swap_feed(slot, feed) {
            self.retire(Retired::Feed(left));
        }
    }

    fn swap_frame(&mut self, frame: Box<LiveFrame>) {
        let rate_changed = frame.sample_rate != self.frame.sample_rate;
        let centers_changed = frame.center_hz != self.frame.center_hz
            || frame.lane_centers_hz != self.frame.lane_centers_hz;
        let same_lanes = frame.lanes() == self.frame.lanes();
        if frame.devices != self.frame.devices {
            self.priors = foreign_lanes(&frame);
        }
        let old = std::mem::replace(&mut self.frame, frame);
        if rate_changed {
            self.windows.set_rate(self.frame.sample_rate);
            self.dc = DcBank::new(self.frame.sample_rate);
        }
        if rate_changed || !same_lanes {
            self.hosts.rebuild();
        } else if centers_changed {
            self.hosts.retune(&self.frame);
        }
        if rate_changed || centers_changed {
            self.bump_generation();
            self.discontinuity();
            if let Some(record) = self.record.as_mut() {
                record.note(RecordNote::Retuned {
                    at: record.next().unwrap_or(0),
                    centers: centers_of(&self.frame),
                });
            }
        }
        self.retire(Retired::Frame(old));
    }

    fn recalibrate(&mut self) {
        self.bump_generation();
        self.corrector.clear_to_identity(self.generation);
        if let Some(stage) = self.stage.as_mut() {
            stage.load();
        }
        self.phase_ready = false;
        self.gain_ready = false;
        self.quality = CalQuality::default();
        self.board.phase_ready.store(false, Ordering::Relaxed);
        self.board.gain_ready.store(false, Ordering::Relaxed);
        self.hosts.reset(ResetCause::Calibrated);
        self.gap_before = true;
    }

    fn swap_record(&mut self, writer: Option<Box<RecordTap>>) {
        let old = std::mem::replace(&mut self.record, writer);
        if let Some(old) = old {
            self.retire(Retired::Record(old));
        }
        let centers = centers_of(&self.frame);
        let offsets = offsets_of(self.aligner.offsets());
        if let Some(record) = self.record.as_mut() {
            let at = record.next().unwrap_or(0);
            record.note(RecordNote::Retuned { at, centers });
            record.note(RecordNote::Offsets { at, offsets });
        }
    }

    fn drain_solutions(&mut self) {
        while self.io.reclaimer.room() >= RETIRE_ROOM {
            let Ok(mut solution) = self.io.solutions.pop() else {
                return;
            };
            let installed = match solution.correction.take() {
                Some(set) if solution.outcome.is_ok() => self.install(set),
                Some(set) => {
                    self.recycle(set);
                    true
                }
                None => true,
            };
            let outcome = if installed {
                solution.outcome
            } else {
                Err(SolveFailure::Refused)
            };
            match outcome {
                Ok(summary) => self.solved(&solution, summary),
                Err(failure) => self.event(AggregatorEvent::SolveFailed {
                    id: solution.id,
                    failure,
                }),
            }
            self.retire(Retired::Solution(solution));
        }
    }

    fn solved(&mut self, solution: &Solution, summary: SolveSummary) {
        if let Some(offsets) = solution.offsets {
            let lanes = self.aligner.lanes();
            self.aligner.set_offsets(&offsets[..lanes]);
            if let Some(record) = self.record.as_mut() {
                record.note(RecordNote::Offsets {
                    at: record.next().unwrap_or(0),
                    offsets,
                });
            }
        }
        for lane in 0..usize::from(summary.lanes).min(MAX_LANES) {
            if let Some(board) = self.board.lane(lane) {
                board.set_solution(
                    f64::from(summary.delay[lane]),
                    f64::from(summary.phase_deg[lane]),
                    f64::from(summary.gain_db[lane]),
                    summary.coherence[lane],
                );
            }
        }
        self.phase_ready = summary.phase_ready;
        self.gain_ready = summary.gain_ready;
        self.quality = solution.quality;
        self.board
            .phase_ready
            .store(summary.phase_ready, Ordering::Relaxed);
        self.board
            .gain_ready
            .store(summary.gain_ready, Ordering::Relaxed);
        self.bump_generation();
        self.hosts.reset(ResetCause::Calibrated);
        self.gap_before = true;
        self.event(AggregatorEvent::Solved {
            id: solution.id,
            summary,
        });
    }

    fn install(&mut self, set: Box<CorrectionSet>) -> bool {
        match self.corrector.swap(set) {
            Ok(old) => {
                self.recycle(old);
                if let Some(stage) = self.stage.as_mut() {
                    stage.load();
                }
                true
            }
            Err(rejected) => {
                self.recycle(rejected);
                false
            }
        }
    }

    fn recycle(&mut self, set: Box<CorrectionSet>) {
        if let Err(PushError::Full(set)) = self.io.sets.push(set) {
            self.retire(Retired::Correction(set));
        }
    }

    fn retire(&mut self, item: Retired) {
        self.flush_spill();
        if self.io.spill.iter().all(Option::is_none) && self.io.reclaimer.room() > 0 {
            self.io.reclaimer.retire(item);
            return;
        }
        match self.io.spill.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(item),
            None => {
                self.board.busy.store(true, Ordering::Relaxed);
                drop(item);
            }
        }
    }

    fn flush_spill(&mut self) {
        for slot in &mut self.io.spill {
            if slot.is_none() {
                continue;
            }
            if self.io.reclaimer.room() == 0 {
                return;
            }
            if let Some(item) = slot.take() {
                self.io.reclaimer.retire(item);
            }
        }
    }

    pub(crate) fn exit(&mut self) -> AggregatorExit {
        AggregatorExit {
            feeds: self.aligner.take_feeds(),
            hosts: self.hosts.take_all(),
        }
    }

    #[cfg(test)]
    pub(crate) fn hosts(&mut self) -> &mut HostList {
        &mut self.hosts
    }

    #[cfg(test)]
    pub(crate) const fn generation(&self) -> u32 {
        self.generation
    }

    #[cfg(test)]
    pub(crate) fn set_ready(&mut self, phase: bool, gain: bool) {
        self.phase_ready = phase;
        self.gain_ready = gain;
    }
}

struct HostRun<'a> {
    hosts: &'a mut HostList,
    windows: &'a Windows,
    frame: &'a LiveFrame,
    board: &'a StatusBoard,
    poses: &'a PoseRing,
    timing: &'a Timing,
    clock: Option<(i64, i64)>,
    corrector: &'a Corrector,
    generation: u32,
    gap_before: &'a mut bool,
    phase_ready: bool,
    gain_ready: bool,
    quality: CalQuality,
}

impl HostRun<'_> {
    fn hosts(&mut self, lanes: &[&[Complex<f32>]], first: u64, corrected: bool, transient: usize) {
        let total = lanes.first().map_or(0, |lane| lane.len());
        let mut at = 0;
        while at < total {
            let index = first + at as u64;
            let mut end = total;
            let mut gate = self.windows.gate_at(index);
            if at < transient {
                gate = gate.or(Some(ProcessorGate::Sync));
                end = end.min(transient);
            }
            if let Some(change) = self.windows.next_change(index) {
                end = end.min((change - first).min(total as u64) as usize);
            }
            let end = end.max(at + 1);
            let mut view: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
            for (slot, lane) in view.iter_mut().zip(lanes) {
                *slot = &lane[at..end];
            }
            self.run(&view[..lanes.len()], index, corrected, gate);
            at = end;
        }
    }

    fn run(
        &mut self,
        lanes: &[&[Complex<f32>]],
        index: u64,
        corrected: bool,
        gate: Option<ProcessorGate>,
    ) {
        let (unix_ns, capture_ns) = self.times(index);
        let block = ArrayBlock {
            lanes,
            corrected,
            correction: self.corrector.view(self.frame.sample_rate),
            first_index: index,
            unix_ns,
            generation: self.generation,
            gap_before: std::mem::take(self.gap_before),
            centers_hz: &self.frame.lane_centers_hz[..self.frame.lanes()],
            cal: self.cal_view(),
            pose: self.pose(capture_ns),
        };
        let inputs = GateInputs {
            sample_rate: self.frame.sample_rate,
            window: gate,
            tier: self.frame.tier,
            tuning: self.frame.tuning,
            synced: matches!(self.board.sync(), SyncState::Locked | SyncState::Drifting),
            phase_ready: self.phase_ready,
            gain_ready: self.gain_ready,
        };
        self.hosts.process(&block, &inputs, self.corrector.active());
    }

    fn times(&self, index: u64) -> (u64, i64) {
        let rate = self.frame.sample_rate;
        let arrival = match self.clock {
            Some((origin, offset)) if rate.is_finite() && rate > 0.0 => {
                let lane_index = i128::from(index) + i128::from(offset);
                origin.saturating_add((lane_index as f64 * 1e9 / rate) as i64)
            }
            _ => i64::try_from(now_ns()).unwrap_or(i64::MAX),
        };
        let unix = u64::try_from(arrival.saturating_add(self.timing.wall_offset_ns)).unwrap_or(0);
        (unix, arrival.saturating_sub(self.frame.latency_ns()))
    }

    fn cal_view(&self) -> CalView {
        CalView {
            phase: self.board.cal(),
            source: self.quality.source,
            phase_ready: self.phase_ready,
            gain_ready: self.gain_ready,
            generation: self.generation,
            phase_sigma_deg: self.quality.phase_sigma_deg,
            gain_sigma_db: self.quality.gain_sigma_db,
            valid_hz: self.quality.valid_hz,
        }
    }

    fn pose(&self, capture_ns: i64) -> Pose {
        oriented(self.poses.at(capture_ns), self.frame.orientation)
    }
}

pub(crate) fn oriented(mut pose: Pose, orientation: ArrayOrientation) -> Pose {
    match orientation {
        ArrayOrientation::Fixed { azimuth_deg } => {
            pose.heading_deg = Some(azimuth_deg.rem_euclid(360.0));
            pose.heading_sigma_deg = 0.0;
            pose.yaw_rate_dps = Some(0.0);
            pose.follows = false;
        }
        ArrayOrientation::Heading { mount_offset_deg } => {
            pose.heading_deg = pose
                .heading_deg
                .map(|heading| (heading + mount_offset_deg).rem_euclid(360.0));
            pose.follows = true;
        }
    }
    pose
}

fn foreign_lanes(frame: &LiveFrame) -> u32 {
    (1..frame.lanes())
        .filter(|lane| !frame.same_device(0, *lane))
        .fold(0, |mask, lane| mask | 1 << lane)
}

fn centers_of(frame: &LiveFrame) -> [f64; MAX_LANES] {
    let mut centers = [0.0; MAX_LANES];
    let lanes = frame.lanes();
    centers[..lanes].copy_from_slice(&frame.lane_centers_hz[..lanes]);
    centers
}

fn offsets_of(offsets: &[i64]) -> [i64; MAX_LANES] {
    let mut held = [0; MAX_LANES];
    let lanes = offsets.len().min(MAX_LANES);
    held[..lanes].copy_from_slice(&offsets[..lanes]);
    held
}

pub(crate) fn spawn(
    name: String,
    mut aggregator: Aggregator,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<AggregatorExit>, EngineError> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            claim(Latency::Critical);
            let board = aggregator.board.clone();
            let run = catch_unwind(AssertUnwindSafe(|| {
                while !stop.load(Ordering::Acquire) {
                    if aggregator.step() == Step::Idle {
                        std::thread::park_timeout(PARK);
                    }
                }
            }));
            match run {
                Ok(()) => aggregator.exit(),
                Err(payload) => {
                    board.stopped(panic_text(payload.as_ref()));
                    AggregatorExit::default()
                }
            }
        })
        .map_err(|error| EngineError::Processor(format!("start array aggregator: {error}")))
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "aggregator panicked".to_owned())
}

#[cfg(test)]
mod tests;
