use std::{
    collections::VecDeque,
    sync::{
        Arc, Weak,
        mpsc::{self, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use num_complex::Complex;
use rtrb::Consumer;
use sdrmm_channels::array_processor::MAX_LANES;
use sdrmm_dsp::{
    array_sync::{COARSE_LAGS, coarse_decimation},
    manifold::{Direction, Vec3, steer},
};
use sdrmm_wire::{
    ArrayCal, ArrayCalRecord, ArrayCalSource, ArrayFailure, ArrayGain, ArrayTune, CalPhase,
    Coherence, LaneKey, NoiseSource, SyncState,
};
use tokio::sync::broadcast;

use super::{
    AggregatorEvent, ArrayControl, ArrayEvent, Command, CommandQueue, StatusBoard, TierDecision,
    capture::{CaptureKind, CaptureRequest, CaptureStart, SolveFailure, SolveSummary},
    track::{Measured, Observation, Tracker, Verdict},
    warm::{self, Band},
    worker::{
        COARSE_CAPTURE, CheckDetail, FINE_MARGIN, PILOT_CAPTURE, SOLVE_CAPTURE, SolveDetail,
        SyncLink, WarmOrder, WorkerOrder, WorkerReport,
    },
};
use crate::EngineError;

pub(crate) const TICK: Duration = Duration::from_millis(20);
pub(crate) const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];
pub(crate) const NOISE_SOLVE_TIMEOUT: Duration = Duration::from_secs(3);
pub(crate) const NOISE_COARSE_TIMEOUT: Duration = Duration::from_secs(6);
pub(crate) const RECORD_THROTTLE: Duration = Duration::from_secs(10);
pub(crate) const AUTO_GAIN_EVERY: Duration = Duration::from_secs(2);
pub(crate) const AUTO_GAIN_DWELL: Duration = Duration::from_secs(30);
pub(crate) const AUTO_GAIN_FLOOR_DBFS: f64 = -12.0;
pub(crate) const AUTO_GAIN_PEAK_DBFS: f64 = -6.0;
const WARM_TIMEOUT: Duration = Duration::from_secs(2);
const REFUSED_RETRIES: u32 = 3;
const DETAILS_KEPT: usize = 8;
const STEP_EPSILON_DB: f64 = 0.01;

pub(crate) enum ControlCommand {
    Configure(Box<ControlConfig>),
    Recalibrate,
    Resync { coarse: bool },
    NoiseSwitch(Option<NoiseSwitch>),
    Stop,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ControlConfig {
    pub(crate) cal: ArrayCal,
    pub(crate) gain: ArrayGain,
    pub(crate) needs_time: bool,
    pub(crate) needs_phase: bool,
    pub(crate) tier: TierDecision,
    pub(crate) sample_rate: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NoiseSwitch {
    pub(crate) device_set: u32,
    pub(crate) kind: NoiseSource,
    pub(crate) all_lanes_held: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SyncContext {
    pub(crate) lanes: Vec<LaneKey>,
    pub(crate) center_hz: f64,
    pub(crate) gain_db: Option<f64>,
    pub(crate) gain_steps_db: Vec<f64>,
    pub(crate) positions: Vec<[f64; 3]>,
    pub(crate) azimuth_deg: Option<f64>,
    pub(crate) warm: Option<ArrayCalRecord>,
}

pub(crate) struct ControllerIo {
    pub(crate) node: String,
    pub(crate) control: Weak<dyn ArrayControl>,
    pub(crate) commands: mpsc::Receiver<ControlCommand>,
    pub(crate) queue: Arc<CommandQueue>,
    pub(crate) events: Consumer<AggregatorEvent>,
    pub(crate) board: Arc<StatusBoard>,
    pub(crate) array_events: broadcast::Sender<ArrayEvent>,
    pub(crate) config: ControlConfig,
    pub(crate) link: SyncLink,
}

pub(crate) fn spawn_controller(
    name: String,
    io: ControllerIo,
) -> Result<JoinHandle<()>, EngineError> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || serve(io))
        .map_err(|error| EngineError::Processor(format!("start array controller: {error}")))
}

fn serve(io: ControllerIo) {
    let (mut controller, commands) = Controller::new(io, Instant::now());
    if let Ok(first) = commands.recv_timeout(TICK) {
        for command in std::iter::once(first).chain(std::iter::from_fn(|| commands.try_recv().ok()))
        {
            if !controller.command(command, Instant::now()) {
                controller.stop();
                return;
            }
        }
    }
    controller.start(Instant::now());
    'serving: loop {
        let first = match commands.recv_timeout(TICK) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        for command in first
            .into_iter()
            .chain(std::iter::from_fn(|| commands.try_recv().ok()))
        {
            if !controller.command(command, Instant::now()) {
                break 'serving;
            }
        }
        controller.poll(Instant::now());
    }
    controller.stop();
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Want {
    #[default]
    Nothing,
    Check,
    Solve,
    Coarse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Warm,
    Coarse,
    Solve,
    Check,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Run {
    want: Want,
    stage: Stage,
    id: u32,
    captured: bool,
    noise: Option<NoiseSource>,
    source: ArrayCalSource,
    started: Instant,
    timeout: Option<Duration>,
    refused: u32,
    escalated: bool,
    cal_before: CalPhase,
}

impl Run {
    fn switches(&self) -> bool {
        matches!(
            self.noise,
            Some(NoiseSource::Isolated | NoiseSource::Unisolated)
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct GainClock {
    next_eval: Instant,
    last_up: Option<Instant>,
    last_clip: Option<Instant>,
    clipped: bool,
}

pub(crate) struct Controller {
    node: String,
    control: Weak<dyn ArrayControl>,
    queue: Arc<CommandQueue>,
    events: Consumer<AggregatorEvent>,
    board: Arc<StatusBoard>,
    array_events: broadcast::Sender<ArrayEvent>,
    link: SyncLink,
    config: ControlConfig,
    lanes: usize,
    noise: Option<NoiseSwitch>,
    noise_on: bool,
    noise_off_at: Option<Instant>,
    next_id: u32,
    run: Option<Run>,
    problem: Option<ArrayFailure>,
    want: Want,
    cooling: Option<Instant>,
    attempts: u32,
    tracker: Tracker,
    details: VecDeque<Box<SolveDetail>>,
    checks: VecDeque<Box<CheckDetail>>,
    next_check: Option<Instant>,
    failure: Option<ArrayFailure>,
    clock_failed: bool,
    record_sent: Option<Instant>,
    record_held: Option<ArrayCalRecord>,
    gain: GainClock,
    started: Instant,
    skipped_checks: u64,
    warned_context: bool,
    running: bool,
}

impl Controller {
    pub(crate) fn new(io: ControllerIo, now: Instant) -> (Self, mpsc::Receiver<ControlCommand>) {
        let lanes = io.board.lane_count();
        let controller = Self {
            node: io.node,
            control: io.control,
            queue: io.queue,
            events: io.events,
            board: io.board,
            array_events: io.array_events,
            link: io.link,
            config: io.config,
            lanes,
            noise: None,
            noise_on: false,
            noise_off_at: None,
            next_id: 0,
            run: None,
            problem: None,
            want: Want::Nothing,
            cooling: None,
            attempts: 0,
            tracker: Tracker::new(lanes),
            details: VecDeque::with_capacity(DETAILS_KEPT),
            checks: VecDeque::with_capacity(DETAILS_KEPT),
            next_check: None,
            failure: None,
            clock_failed: false,
            record_sent: None,
            record_held: None,
            gain: GainClock {
                next_eval: now + AUTO_GAIN_EVERY,
                last_up: Some(now),
                last_clip: None,
                clipped: false,
            },
            started: now,
            skipped_checks: 0,
            warned_context: false,
            running: false,
        };
        (controller, io.commands)
    }

    pub(crate) fn start(&mut self, now: Instant) {
        self.running = true;
        self.board.set_cal(self.idle_cal());
        if self.config.tier.tier == Coherence::None {
            self.board.set_all_lanes(SyncState::Idle);
            self.set_failure(Some(ArrayFailure::NotCoherent));
            return;
        }
        if self.config.tier.structural_zero_delay {
            self.board.set_all_lanes(SyncState::Locked);
            if self.config.cal.source.kind().is_some() {
                self.want = Want::Solve;
            }
        } else {
            self.board.set_all_lanes(SyncState::Searching);
            self.want = Want::Coarse;
        }
        if self.config.cal.warm_start {
            self.warm_start(now);
        }
        self.poll(now);
    }

    pub(crate) fn command(&mut self, command: ControlCommand, now: Instant) -> bool {
        match command {
            ControlCommand::Configure(config) => self.configure(*config, now),
            ControlCommand::Recalibrate => self.trigger(Want::Solve, now),
            ControlCommand::Resync { coarse } => {
                if coarse {
                    self.tracker.reset();
                    self.abort_run();
                    self.trigger(Want::Coarse, now);
                } else {
                    self.trigger(Want::Solve, now);
                }
            }
            ControlCommand::NoiseSwitch(switch) => self.noise_switch(switch),
            ControlCommand::Stop => return false,
        }
        true
    }

    pub(crate) fn poll(&mut self, now: Instant) {
        self.answer_check(now);
        while let Ok(event) = self.events.pop() {
            self.event(event, now);
        }
        self.timers(now);
        self.auto_gain(now);
        self.flush_record(now, false);
        if self.cooling.is_none() {
            self.poll_begin(now);
        }
        self.board.control().next_check_at = self
            .next_check
            .filter(|_| self.checks_enabled() && !self.blocked());
    }

    pub(crate) fn stop(&mut self) {
        self.noise_off();
        self.flush_record(Instant::now(), true);
    }

    fn idle_cal(&self) -> CalPhase {
        if self.config.cal.source.kind().is_some() {
            CalPhase::Waiting
        } else {
            CalPhase::None
        }
    }

    fn blocked(&self) -> bool {
        self.config.tier.tier == Coherence::None || self.clock_failed
    }

    fn checks_enabled(&self) -> bool {
        self.config.cal.check_s > 0 && (self.config.needs_time || self.config.needs_phase)
    }

    fn id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }

    fn t_s(&self, now: Instant) -> f64 {
        now.saturating_duration_since(self.started).as_secs_f64()
    }

    fn trigger(&mut self, want: Want, now: Instant) {
        if self.clock_failed && want >= Want::Solve {
            self.clear_clock_drift();
            self.want = self.want.max(Want::Coarse);
        }
        self.want = self.want.max(want);
        self.attempts = 0;
        self.cooling = None;
        self.poll_begin(now);
    }

    fn poll_begin(&mut self, now: Instant) {
        if self.running && self.run.is_none() && self.want != Want::Nothing {
            self.begin(now);
        }
    }

    fn configure(&mut self, config: ControlConfig, now: Instant) {
        let checked = self.checks_enabled();
        let old = std::mem::replace(&mut self.config, config);
        if old.cal.source != self.config.cal.source
            || old.cal.equaliser != self.config.cal.equaliser
        {
            self.recalibrate();
            self.want = self.want.max(Want::Solve);
            self.attempts = 0;
            self.cooling = None;
        }
        if old.cal.check_s != self.config.cal.check_s || checked != self.checks_enabled() {
            self.schedule_check(now);
        }
        if old.sample_rate != self.config.sample_rate {
            self.tracker.reset();
            self.want = self.want.max(Want::Coarse);
        }
        if old.tier != self.config.tier {
            self.retier(&old.tier);
        }
        self.poll_begin(now);
    }

    fn retier(&mut self, old: &TierDecision) {
        let tier = self.config.tier;
        if tier.tier == Coherence::None {
            if !self.clock_failed {
                self.abort_run();
                self.want = Want::Nothing;
                self.set_failure(Some(ArrayFailure::NotCoherent));
            }
            return;
        }
        if self.failure == Some(ArrayFailure::NotCoherent) {
            self.set_failure(None);
        }
        if tier.structural_zero_delay {
            self.board.set_all_lanes(SyncState::Locked);
        }
        if old.tier == Coherence::None || old.structural_zero_delay != tier.structural_zero_delay {
            self.tracker.reset();
            self.want = self.want.max(if tier.structural_zero_delay {
                Want::Solve
            } else {
                Want::Coarse
            });
        }
    }

    fn recalibrate(&mut self) {
        if self.queue.send(Command::Recalibrate).is_err() {
            self.board.add_events_lost(1);
        }
        self.board.set_cal(self.idle_cal());
    }

    fn noise_switch(&mut self, switch: Option<NoiseSwitch>) {
        let was = self.noise_ready().is_ok();
        if self.noise_on
            && switch.map(|held| held.device_set) != self.noise.map(|held| held.device_set)
        {
            self.noise_off();
        }
        self.noise = switch;
        if matches!(self.config.cal.source, ArrayCalSource::Noise)
            && self.noise_ready().is_ok()
            && !was
        {
            if matches!(
                self.failure,
                Some(ArrayFailure::NoNoiseSource | ArrayFailure::NoiseShared)
            ) {
                self.set_failure(None);
            }
            self.want = self.want.max(Want::Solve);
        }
    }

    fn noise_ready(&self) -> Result<NoiseSource, ArrayFailure> {
        match self.noise {
            Some(NoiseSwitch {
                kind:
                    kind @ (NoiseSource::Isolated | NoiseSource::Unisolated | NoiseSource::Replayed),
                all_lanes_held: true,
                ..
            }) => Ok(kind),
            Some(NoiseSwitch {
                kind: NoiseSource::None,
                ..
            })
            | None => Err(ArrayFailure::NoNoiseSource),
            Some(NoiseSwitch { .. }) => Err(ArrayFailure::NoiseShared),
        }
    }

    fn warm_start(&mut self, now: Instant) {
        let Some(context) = self.context() else {
            return;
        };
        let Some(record) = context.warm else {
            return;
        };
        let Some(usage) = warm::assess(
            &record,
            &context.lanes,
            context.center_hz,
            self.config.sample_rate,
            context.gain_db,
            self.config.tier.keeps_phase,
        ) else {
            return;
        };
        let usage = warm::WarmUse {
            equaliser: usage.equaliser && self.config.cal.equaliser,
            phase: usage.phase && self.config.cal.source.kind().is_some(),
            ..usage
        };
        let offsets = usage.delay_prior && self.config.tier.devices == 1;
        let id = self.id();
        let order = WorkerOrder::Warm(Box::new(WarmOrder {
            id,
            record,
            usage,
            offsets,
        }));
        if self.link.orders.send(order).is_err() {
            tracing::warn!(node = %self.node, "array warm start skipped, the sync worker is gone");
            return;
        }
        if offsets && self.want == Want::Coarse {
            self.want = Want::Solve;
        }
        self.run = Some(Run {
            want: Want::Nothing,
            stage: Stage::Warm,
            id,
            captured: true,
            noise: None,
            source: self.config.cal.source,
            started: now,
            timeout: Some(WARM_TIMEOUT),
            refused: 0,
            escalated: false,
            cal_before: self.board.cal(),
        });
    }

    fn context(&mut self) -> Option<SyncContext> {
        let control = self.control.upgrade()?;
        match control.sync_context(&self.node) {
            Ok(context) => Some(context),
            Err(error) => {
                if !self.warned_context {
                    self.warned_context = true;
                    tracing::warn!(node = %self.node, %error, "array sync runs without engine context");
                }
                None
            }
        }
    }

    fn begin(&mut self, now: Instant) {
        let want = std::mem::take(&mut self.want);
        if self.blocked() || want == Want::Nothing {
            return;
        }
        let structural = self.config.tier.structural_zero_delay;
        let want = if structural {
            want.min(Want::Solve)
        } else {
            want
        };
        let (source, noise, problem) = self.plan_source();
        let stage = match want {
            Want::Coarse => Stage::Coarse,
            Want::Check if matches!(source, ArrayCalSource::Off) => Stage::Check,
            _ => Stage::Solve,
        };
        let mut run = Run {
            want,
            stage,
            id: 0,
            captured: false,
            noise,
            source,
            started: now,
            timeout: None,
            refused: 0,
            escalated: false,
            cal_before: self.board.cal(),
        };
        self.problem = problem;
        if run.switches() && !self.switch_noise(true) {
            run.noise = None;
            run.source = ArrayCalSource::Off;
            self.problem = Some(ArrayFailure::NoNoiseSource);
        }
        if let Some(problem) = self.problem.clone() {
            self.set_failure(Some(problem));
        }
        if structural && matches!(run.source, ArrayCalSource::Off) {
            self.board.set_all_lanes(SyncState::Locked);
            return;
        }
        if stage == Stage::Coarse {
            self.board.set_all_lanes(SyncState::Searching);
        } else if !self.locked() {
            self.board.set_sync(SyncState::Searching);
        }
        self.run = Some(run);
        self.capture(now);
    }

    fn locked(&self) -> bool {
        matches!(self.board.sync(), SyncState::Locked | SyncState::Drifting)
    }

    fn plan_source(&mut self) -> (ArrayCalSource, Option<NoiseSource>, Option<ArrayFailure>) {
        match self.config.cal.source {
            ArrayCalSource::Noise => match self.noise_ready() {
                Ok(kind) => (ArrayCalSource::Noise, Some(kind), None),
                Err(failure) => (ArrayCalSource::Off, None, Some(failure)),
            },
            source => (source, None, None),
        }
    }

    fn capture(&mut self, now: Instant) {
        let Some(mut run) = self.run else {
            return;
        };
        let id = self.id();
        run.id = id;
        run.captured = false;
        run.started = now;
        if run.stage == Stage::Solve
            && let ArrayCalSource::Emitter {
                offset_hz,
                bearing_deg,
                ..
            } = run.source
        {
            match self.steering(offset_hz, bearing_deg) {
                Ok(steering) => {
                    if self
                        .link
                        .orders
                        .send(WorkerOrder::Steer { id, steering })
                        .is_err()
                    {
                        self.run = Some(run);
                        self.retry(ArrayFailure::Busy, now);
                        return;
                    }
                }
                Err(problem) => {
                    run.source = ArrayCalSource::Off;
                    self.problem = Some(problem.clone());
                    self.set_failure(Some(problem));
                }
            }
        }
        let rate = self.config.sample_rate;
        let (kind, len, decimation) = match run.stage {
            Stage::Coarse => (
                CaptureKind::Coarse,
                COARSE_CAPTURE,
                coarse_decimation(rate, COARSE_LAGS),
            ),
            Stage::Check => (CaptureKind::Check, SOLVE_CAPTURE, 1),
            Stage::Solve | Stage::Warm => match run.source {
                ArrayCalSource::Pilot { .. } | ArrayCalSource::Emitter { .. } => {
                    (CaptureKind::Solve, PILOT_CAPTURE, 1)
                }
                ArrayCalSource::Noise | ArrayCalSource::Off => {
                    (CaptureKind::Solve, SOLVE_CAPTURE, 1)
                }
            },
        };
        run.timeout = stage_timeout(run.stage, run.noise, len * decimation, rate);
        let start = if run.noise.is_some() {
            CaptureStart::NoiseWindow
        } else {
            CaptureStart::Now
        };
        let request = CaptureRequest {
            id,
            kind,
            start,
            len,
            decimation,
            source: run.source,
            equaliser: self.config.cal.equaliser,
        };
        if run.stage == Stage::Solve && run.source.kind().is_some() {
            self.board.set_cal(CalPhase::Measuring);
        }
        self.run = Some(run);
        if self.queue.send(Command::Capture { request }).is_err() {
            self.retry(ArrayFailure::Busy, now);
        }
    }

    fn steering(
        &mut self,
        offset_hz: f64,
        bearing_deg: f64,
    ) -> Result<[Complex<f32>; MAX_LANES], ArrayFailure> {
        let lanes = self.lanes as u32;
        let context = self.context().ok_or(ArrayFailure::GeometryMismatch {
            positions: 0,
            lanes,
        })?;
        if context.positions.len() != self.lanes {
            return Err(ArrayFailure::GeometryMismatch {
                positions: context.positions.len() as u32,
                lanes,
            });
        }
        let azimuth = context.azimuth_deg.ok_or(ArrayFailure::NeedsPosition)?;
        let positions: Vec<Vec3> = context
            .positions
            .iter()
            .map(|[x, y, z]| Vec3::new(*x, *y, *z))
            .collect();
        let mut steering = [Complex::new(1.0, 0.0); MAX_LANES];
        steer(
            &positions,
            context.center_hz + offset_hz,
            Direction::horizon(bearing_deg - azimuth),
            &mut steering[..self.lanes],
        );
        Ok(steering)
    }

    fn switch_noise(&mut self, on: bool) -> bool {
        if on == self.noise_on {
            return true;
        }
        let Some(control) = self.control.upgrade() else {
            self.noise_on = false;
            return !on;
        };
        match control.switch_array_noise(&self.node, on) {
            Ok(()) => {
                self.noise_on = on;
                true
            }
            Err(error) => {
                tracing::warn!(node = %self.node, %error, on, "array noise switch failed");
                self.noise_on = false;
                false
            }
        }
    }

    fn noise_off(&mut self) {
        if self.noise_on {
            let _ = self.switch_noise(false);
            self.noise_on = false;
            self.noise_off_at = Some(Instant::now());
        }
    }

    fn abort_run(&mut self) {
        self.noise_off();
        if let Some(run) = self.run.take()
            && run.stage == Stage::Solve
            && self.board.cal() == CalPhase::Measuring
        {
            self.board.set_cal(run.cal_before);
        }
    }

    fn answer_check(&mut self, now: Instant) {
        self.drain_reports();
        let Some(run) = self.run.filter(|run| run.stage == Stage::Check) else {
            return;
        };
        if let Some(at) = self.checks.iter().position(|check| check.id == run.id)
            && let Some(check) = self.checks.remove(at)
        {
            self.checked(&check, now);
        }
    }

    fn take_detail(&mut self, id: u32) -> Option<Box<SolveDetail>> {
        self.drain_reports();
        let at = self.details.iter().position(|detail| detail.id == id)?;
        self.details.remove(at)
    }

    fn drain_reports(&mut self) {
        while let Ok(report) = self.link.reports.try_recv() {
            match report {
                WorkerReport::Solved(detail) => self.details.push_back(detail),
                WorkerReport::Checked(detail) => self.checks.push_back(detail),
            }
        }
        while self.details.len() > DETAILS_KEPT {
            self.details.pop_front();
        }
        while self.checks.len() > DETAILS_KEPT {
            self.checks.pop_front();
        }
    }

    fn event(&mut self, event: AggregatorEvent, now: Instant) {
        let current = self.run.map(|run| run.id);
        match event {
            AggregatorEvent::Captured { id } if current == Some(id) => self.captured(),
            AggregatorEvent::CaptureRefused { id } if current == Some(id) => self.refused(now),
            AggregatorEvent::Solved { id, summary } if current == Some(id) => {
                self.solved(&summary, now);
            }
            AggregatorEvent::SolveFailed { id, failure } if current == Some(id) => {
                self.solve_failed(failure, now);
            }
            AggregatorEvent::NoiseNotSeen => {
                if self
                    .run
                    .is_some_and(|run| run.noise.is_some() && !run.captured)
                {
                    self.retry(ArrayFailure::NoiseNotSeen, now);
                }
            }
            AggregatorEvent::BlankEnded { .. } => self.want = self.want.max(Want::Solve),
            AggregatorEvent::Uncertain { lane, error, .. } => {
                if error > FINE_MARGIN as u64 {
                    self.tracker.reset();
                    self.abort_run();
                    self.want = self.want.max(Want::Coarse);
                } else {
                    self.tracker.forget(lane);
                    self.want = self.want.max(Want::Solve);
                }
                self.cooling = None;
            }
            AggregatorEvent::Clipped { .. } => {
                let quiet = self
                    .noise_off_at
                    .is_none_or(|off| now.saturating_duration_since(off) >= AUTO_GAIN_EVERY);
                if !self.noise_on && quiet {
                    self.gain.clipped = true;
                    self.gain.last_clip = Some(now);
                }
            }
            AggregatorEvent::NoiseOnset { .. }
            | AggregatorEvent::NoiseEnded { .. }
            | AggregatorEvent::Captured { .. }
            | AggregatorEvent::CaptureRefused { .. }
            | AggregatorEvent::Solved { .. }
            | AggregatorEvent::SolveFailed { .. } => {}
        }
    }

    fn captured(&mut self) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        run.captured = true;
        if run.stage == Stage::Solve || run.stage == Stage::Check {
            self.noise_off();
        }
    }

    fn refused(&mut self, now: Instant) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        run.refused += 1;
        if run.refused > REFUSED_RETRIES {
            self.retry(ArrayFailure::Busy, now);
        } else {
            self.capture(now);
        }
    }

    fn solved(&mut self, summary: &SolveSummary, now: Instant) {
        let Some(run) = self.run else {
            return;
        };
        match run.stage {
            Stage::Warm => {
                self.board.set_cal(CalPhase::Warm);
                self.run = None;
            }
            Stage::Coarse => {
                self.tracker.reset();
                let drift = self.take_detail(run.id).and_then(|detail| detail.drift_ppm);
                if drift.is_some() {
                    self.board.control().drift_ppm = drift;
                }
                self.run = Some(Run {
                    want: Want::Solve,
                    stage: Stage::Solve,
                    refused: 0,
                    ..run
                });
                self.capture(now);
            }
            Stage::Solve => self.finish_solve(&run, summary, now),
            Stage::Check => {}
        }
    }

    fn finish_solve(&mut self, run: &Run, summary: &SolveSummary, now: Instant) {
        self.noise_off();
        self.run = None;
        let detail = self.take_detail(run.id);
        let phase = run.source.kind().is_some() && summary.phase_ready;
        let mut measured = [None; MAX_LANES];
        for (lane, slot) in measured.iter_mut().enumerate().take(self.lanes) {
            let delay = detail
                .as_ref()
                .map_or(f64::from(summary.delay[lane]), |detail| detail.delays[lane]);
            *slot = Some(Measured {
                delay,
                phase_deg: phase.then_some(f64::from(summary.phase_deg[lane])),
            });
        }
        let verdict = self.observe(&measured[..self.lanes], true, now);
        self.attempts = 0;
        self.set_failure(self.problem.clone());
        self.board.control().last_solve_at = Some(format!("{:.3}", jiff::Timestamp::now()));
        self.board.set_cal(if phase {
            CalPhase::Solved
        } else if run.source.kind().is_some() {
            run.cal_before
        } else {
            self.idle_cal()
        });
        self.apply_verdict(&verdict, Stage::Solve);
        if phase && let Some(detail) = detail {
            self.hold_record(run, summary, &detail);
        }
        self.schedule_check(now);
    }

    fn observe(&mut self, lanes: &[Option<Measured>], hold: bool, now: Instant) -> Verdict {
        self.tracker.observe(&Observation {
            t_s: self.t_s(now),
            lanes,
            sample_rate: self.config.sample_rate,
            devices: self.config.tier.devices,
            hold,
        })
    }

    fn apply_verdict(&mut self, verdict: &Verdict, stage: Stage) {
        for lane in 0..self.lanes {
            if let Some(board) = self.board.lane(lane) {
                board.set_sync(verdict.lanes[lane]);
                if let Some(residual) = verdict.residuals[lane] {
                    board.set_residual(residual.delay, residual.phase_deg);
                }
            }
        }
        self.board.set_sync(verdict.state);
        if verdict.drift_ppm.is_some() {
            self.board.control().drift_ppm = verdict.drift_ppm;
        }
        if let Some(ppm) = verdict.clock_drift {
            self.clock_drift(ppm);
            return;
        }
        if verdict.slips_repeated {
            self.set_failure(Some(ArrayFailure::SlipsRepeated));
        }
        if verdict.slipped != 0 {
            self.want = self
                .want
                .max(if verdict.worst_slip.abs() > FINE_MARGIN as f64 {
                    Want::Coarse
                } else {
                    Want::Solve
                });
        } else if stage == Stage::Check
            && (verdict.recentre || verdict.state == SyncState::Drifting)
        {
            self.want = self.want.max(Want::Solve);
        }
    }

    fn checked(&mut self, check: &CheckDetail, now: Instant) {
        self.run = None;
        let mut measured = [None; MAX_LANES];
        for (lane, slot) in measured.iter_mut().enumerate().take(self.lanes) {
            *slot = check.delays[lane].map(|delay| Measured {
                delay,
                phase_deg: None,
            });
        }
        if measured[1..self.lanes].iter().all(Option::is_none) {
            self.skipped_checks += 1;
            tracing::info!(node = %self.node, skipped = self.skipped_checks, "array check skipped, no common signal");
        } else {
            let verdict = self.observe(&measured[..self.lanes], false, now);
            self.apply_verdict(&verdict, Stage::Check);
        }
        self.attempts = 0;
        self.schedule_check(now);
    }

    fn solve_failed(&mut self, failure: SolveFailure, now: Instant) {
        let Some(run) = self.run else {
            return;
        };
        if let SolveFailure::Drift { ppm } = failure {
            self.clock_drift(f64::from(ppm));
            return;
        }
        match run.stage {
            Stage::Warm => {
                tracing::info!(node = %self.node, ?failure, "array warm start refused");
                self.run = None;
            }
            Stage::Solve
                if !run.escalated
                    && matches!(
                        failure,
                        SolveFailure::NoPeak { .. } | SolveFailure::Ambiguous { .. }
                    )
                    && !self.config.tier.structural_zero_delay =>
            {
                self.tracker.reset();
                let mut coarse = Run {
                    want: Want::Coarse,
                    stage: Stage::Coarse,
                    escalated: true,
                    refused: 0,
                    ..run
                };
                if coarse.switches() && !self.switch_noise(true) {
                    coarse.noise = None;
                    coarse.source = ArrayCalSource::Off;
                }
                self.board.set_all_lanes(SyncState::Searching);
                self.run = Some(coarse);
                self.capture(now);
            }
            _ => self.retry(array_failure(failure), now),
        }
    }

    fn retry(&mut self, failure: ArrayFailure, now: Instant) {
        self.noise_off();
        let Some(run) = self.run.take() else {
            return;
        };
        self.set_failure(Some(failure));
        if self.board.cal() == CalPhase::Measuring {
            self.board.set_cal(run.cal_before);
        }
        self.attempts += 1;
        match RETRY_DELAYS.get(self.attempts as usize - 1) {
            Some(delay) => {
                self.cooling = Some(now + *delay);
                self.want = self.want.max(run.want.max(Want::Check));
                if run.source.kind().is_some() && !self.board.phase_ready() {
                    self.board.set_cal(CalPhase::Waiting);
                }
            }
            None => {
                self.attempts = 0;
                if run.source.kind().is_some() && !self.board.phase_ready() {
                    self.board.set_cal(CalPhase::Failed);
                }
                if !self.locked() {
                    self.board.set_sync(SyncState::Searching);
                }
                self.schedule_check(now);
            }
        }
    }

    fn clock_drift(&mut self, ppm: f64) {
        self.abort_run();
        self.want = Want::Nothing;
        self.cooling = None;
        self.clock_failed = true;
        self.board.control().drift_ppm = Some(ppm);
        self.set_failure(Some(ArrayFailure::ClockDrift { ppm }));
        self.recalibrate();
        if let Some(control) = self.control.upgrade()
            && let Err(error) = control.clock_drift(&self.node, Some(ppm))
        {
            tracing::warn!(node = %self.node, %error, "array clock drift not passed to the tier");
        }
    }

    fn clear_clock_drift(&mut self) {
        self.clock_failed = false;
        if matches!(self.failure, Some(ArrayFailure::ClockDrift { .. })) {
            self.set_failure(None);
        }
        if let Some(control) = self.control.upgrade()
            && let Err(error) = control.clock_drift(&self.node, None)
        {
            tracing::warn!(node = %self.node, %error, "array clock drift not cleared from the tier");
        }
    }

    fn set_failure(&mut self, failure: Option<ArrayFailure>) {
        let mut control = self.board.control();
        let mine = control.failure.is_none() || control.failure == self.failure;
        if mine {
            control.failure.clone_from(&failure);
        }
        drop(control);
        self.failure = failure;
    }

    fn schedule_check(&mut self, now: Instant) {
        self.next_check = self
            .checks_enabled()
            .then(|| now + Duration::from_secs(u64::from(self.config.cal.check_s)));
    }

    fn timers(&mut self, now: Instant) {
        if let Some(run) = self.run
            && let Some(timeout) = run.timeout
            && now.saturating_duration_since(run.started) >= timeout
        {
            match run.stage {
                Stage::Warm => {
                    tracing::warn!(node = %self.node, "array warm start timed out");
                    self.run = None;
                }
                _ if run.noise.is_some() && !run.captured => {
                    self.retry(ArrayFailure::NoiseNotSeen, now);
                }
                _ => self.retry(ArrayFailure::Busy, now),
            }
        }
        if self.cooling.is_some_and(|until| now >= until) {
            self.cooling = None;
        }
        if self.checks_enabled()
            && self.next_check.is_some_and(|at| now >= at)
            && self.run.is_none()
            && !self.blocked()
        {
            self.want = self.want.max(Want::Check);
            self.next_check = None;
        }
    }

    fn hold_record(&mut self, run: &Run, summary: &SolveSummary, detail: &SolveDetail) {
        let Some(source) = run.source.kind() else {
            return;
        };
        let Some(context) = self.context() else {
            return;
        };
        if context.lanes.len() != self.lanes {
            tracing::warn!(node = %self.node, "array calibration not stored, lanes changed");
            return;
        }
        let band = Band {
            center_hz: context.center_hz,
            sample_rate: self.config.sample_rate,
            gain_db: self.board.control().gain_db.or(context.gain_db),
            keeps_phase: self.config.tier.keeps_phase,
        };
        let eq = if matches!(run.source, ArrayCalSource::Noise) {
            detail.equalisers.as_slice()
        } else {
            &[]
        };
        self.record_held = Some(warm::record(
            &context.lanes,
            &band,
            source,
            summary,
            &detail.delays[..self.lanes],
            eq,
        ));
    }

    fn flush_record(&mut self, now: Instant, force: bool) {
        if self.record_held.is_none()
            || (!force
                && self
                    .record_sent
                    .is_some_and(|at| now.saturating_duration_since(at) < RECORD_THROTTLE))
        {
            return;
        }
        if let Some(record) = self.record_held.take() {
            let _ = self.array_events.send(ArrayEvent::Solved {
                array: self.node.clone(),
                record,
            });
            self.record_sent = Some(now);
        }
    }

    fn auto_gain(&mut self, now: Instant) {
        if self.config.gain != ArrayGain::Auto
            || self.config.cal.source.kind().is_none()
            || now < self.gain.next_eval
        {
            return;
        }
        self.gain.next_eval = now + AUTO_GAIN_EVERY;
        if self.noise_on || self.run.is_some() {
            return;
        }
        let mut clipped = std::mem::take(&mut self.gain.clipped);
        let mut levels = [0.0f32; MAX_LANES];
        for (lane, level) in levels.iter_mut().enumerate().take(self.lanes) {
            if let Some(board) = self.board.lane(lane) {
                *level = board.level_cdb.load(std::sync::atomic::Ordering::Relaxed) as f32 / 100.0;
                clipped |= board.clipping.load(std::sync::atomic::Ordering::Relaxed);
            }
        }
        if clipped {
            self.gain.last_clip = Some(now);
        }
        let Some(context) = self.context() else {
            return;
        };
        let current = self.board.control().gain_db.or(context.gain_db);
        let Some(current) = current else {
            return;
        };
        let since = |at: Option<Instant>| at.map(|at| now.saturating_duration_since(at));
        let Some(db) = auto_step(&AutoGain {
            levels_dbfs: &levels[..self.lanes],
            clipped,
            current_db: current,
            steps_db: &context.gain_steps_db,
            since_up: since(self.gain.last_up),
            since_clip: since(self.gain.last_clip),
        }) else {
            return;
        };
        let Some(control) = self.control.upgrade() else {
            return;
        };
        let tune = ArrayTune {
            center_hz: context.center_hz,
            gain: ArrayGain::Manual { db },
        };
        match control.tune_array_internal(&self.node, tune) {
            Ok(()) if db > current => self.gain.last_up = Some(now),
            Ok(()) => {}
            Err(error) => {
                tracing::warn!(node = %self.node, %error, db, "array auto gain step failed")
            }
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.noise_off();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AutoGain<'a> {
    pub(crate) levels_dbfs: &'a [f32],
    pub(crate) clipped: bool,
    pub(crate) current_db: f64,
    pub(crate) steps_db: &'a [f64],
    pub(crate) since_up: Option<Duration>,
    pub(crate) since_clip: Option<Duration>,
}

pub(crate) fn auto_step(gain: &AutoGain<'_>) -> Option<f64> {
    let current = gain.current_db;
    if gain.clipped {
        return gain
            .steps_db
            .iter()
            .copied()
            .filter(|step| *step < current - STEP_EPSILON_DB)
            .max_by(f64::total_cmp);
    }
    let dwelt = |since: Option<Duration>| since.is_none_or(|since| since >= AUTO_GAIN_DWELL);
    let peak = gain
        .levels_dbfs
        .iter()
        .copied()
        .map(f64::from)
        .max_by(f64::total_cmp)?;
    if peak > AUTO_GAIN_FLOOR_DBFS || !dwelt(gain.since_up) || !dwelt(gain.since_clip) {
        return None;
    }
    gain.steps_db
        .iter()
        .copied()
        .filter(|step| *step > current + STEP_EPSILON_DB)
        .min_by(f64::total_cmp)
        .filter(|step| peak + (step - current) <= AUTO_GAIN_PEAK_DBFS)
}

fn stage_timeout(
    stage: Stage,
    noise: Option<NoiseSource>,
    raw_samples: usize,
    rate: f64,
) -> Option<Duration> {
    if noise == Some(NoiseSource::Replayed) {
        return None;
    }
    let base = match (stage, noise.is_some()) {
        (Stage::Coarse, true) => NOISE_COARSE_TIMEOUT,
        _ => NOISE_SOLVE_TIMEOUT,
    };
    let capture = if rate.is_finite() && rate > 0.0 {
        Duration::from_secs_f64((raw_samples as f64 / rate).min(3_600.0))
    } else {
        Duration::ZERO
    };
    Some(base.max(capture * 2 + Duration::from_secs(1)))
}

fn array_failure(failure: SolveFailure) -> ArrayFailure {
    match failure {
        SolveFailure::NoPeak { .. }
        | SolveFailure::Ambiguous { .. }
        | SolveFailure::Short
        | SolveFailure::FewBins => ArrayFailure::NoCommonSignal,
        SolveFailure::LowCoherence { lane, coherence } => ArrayFailure::LowCoherence {
            lane: u32::from(lane),
            coherence,
        },
        SolveFailure::Clipped { lane } => ArrayFailure::NoiseClips {
            lane: u32::from(lane),
        },
        SolveFailure::Drift { ppm } => ArrayFailure::ClockDrift {
            ppm: f64::from(ppm),
        },
        SolveFailure::Refused => ArrayFailure::Busy,
    }
}

#[cfg(test)]
mod tests;
