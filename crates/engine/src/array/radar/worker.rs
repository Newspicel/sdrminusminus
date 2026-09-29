use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    thread::{JoinHandle, Thread},
    time::{Duration, Instant},
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_channels::{
    ChannelError,
    array_processor::{
        ArrayBlock, MAX_LANES, ProcessorAction, ProcessorFaults, ProcessorOutput,
        TUNING_TOLERANCE_HZ,
    },
    passive_radar::{
        CafBackend, CafError, CpiJob, CpiStage, FrontStage, InLanes, JobSink, LiveParams,
        RadarPlan as StagePlan, RadarPod,
    },
};
use sdrmm_device::{Latency, schedule::claim};

use super::{
    DedicatedRunner, Prepared, Retired,
    publish::{self, Mailbox, Publisher},
};

type C32 = Complex<f32>;

const IN_SECONDS: f64 = 0.25;
const IN_SLACK: usize = 4;
const JOBS: usize = 2;
pub(super) const COMMANDS: usize = 8;
const HANDOVERS: usize = 3;
const CARRIED: usize = 11;
const IDLE: Duration = Duration::from_millis(10);
const FOLLOW_POLL: Duration = Duration::from_millis(1);
const FOLLOW_WAIT: Duration = Duration::from_secs(2);
const LOAD_SMOOTHING: f32 = 0.05;
const NANOS_PER_SECOND: f64 = 1e9;

type Carried = [u64; CARRIED];

#[derive(Default)]
pub(crate) struct Shared {
    pub(super) stop: AtomicBool,
    pub(super) closing: AtomicBool,
    pub(super) dead: AtomicBool,
    pub(super) finished: AtomicBool,
    pub(super) live: AtomicUsize,
    pub(super) generation: AtomicU64,
    pub(super) dropped_samples: AtomicU64,
    pub(super) dropped_blocks: AtomicU64,
    pub(super) dropped_cpis: AtomicU64,
    pub(super) discarded_cpis: AtomicU64,
    pub(super) dropped_reports: AtomicU64,
    pub(super) gpu_failures: AtomicU64,
    pub(super) lane_mismatch: AtomicU64,
    pub(super) unsuppressed_groups: AtomicU64,
    pub(super) truncated_detections: AtomicU64,
    pub(super) dropped_tracks: AtomicU64,
    pub(super) retunes: AtomicU64,
    pub(super) front_load: AtomicU32,
    pub(super) next_track_id: AtomicU32,
    #[cfg(test)]
    pub(super) hold_in: AtomicBool,
    #[cfg(test)]
    pub(super) gaps_in: AtomicU64,
    #[cfg(test)]
    pub(super) blocks_done: AtomicU64,
    #[cfg(test)]
    pub(super) jobs_in: AtomicU64,
    #[cfg(test)]
    pub(super) jobs_out: AtomicU64,
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire) || self.closing.load(Ordering::Acquire)
    }

    const fn carried(&self) -> [&AtomicU64; CARRIED] {
        [
            &self.dropped_samples,
            &self.dropped_blocks,
            &self.dropped_cpis,
            &self.discarded_cpis,
            &self.dropped_reports,
            &self.gpu_failures,
            &self.lane_mismatch,
            &self.unsuppressed_groups,
            &self.truncated_detections,
            &self.dropped_tracks,
            &self.retunes,
        ]
    }

    fn inherit(&self, old: &Self) -> Carried {
        let mut seen = [0; CARRIED];
        for ((to, from), slot) in self.carried().into_iter().zip(old.carried()).zip(&mut seen) {
            *slot = from.load(Ordering::Relaxed);
            to.fetch_add(*slot, Ordering::Relaxed);
        }
        seen
    }

    fn inherit_rest(&self, old: &Self, seen: &Carried) {
        for ((to, from), before) in self.carried().into_iter().zip(old.carried()).zip(seen) {
            let late = from.load(Ordering::Relaxed).saturating_sub(*before);
            to.fetch_add(late, Ordering::Relaxed);
        }
    }

    fn faults(&self) -> ProcessorFaults {
        ProcessorFaults {
            lane_mismatch: self.lane_mismatch.load(Ordering::Relaxed),
            dropped_blocks: self.dropped_blocks.load(Ordering::Relaxed),
            solver_failures: self.unsuppressed_groups.load(Ordering::Relaxed),
            resets: self.retunes.load(Ordering::Relaxed),
            truncated: self.truncated_detections.load(Ordering::Relaxed)
                + self.dropped_tracks.load(Ordering::Relaxed),
        }
    }
}

struct Presence {
    shared: Arc<Shared>,
    finishes: bool,
}

impl Presence {
    fn enter(shared: &Arc<Shared>, finishes: bool) -> Self {
        shared.live.fetch_add(1, Ordering::AcqRel);
        Self {
            shared: Arc::clone(shared),
            finishes,
        }
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        if std::thread::panicking() || !self.shared.stopping() {
            self.shared.dead.store(true, Ordering::Release);
        }
        if self.finishes {
            self.shared.finished.store(true, Ordering::Release);
        }
        self.shared.live.fetch_sub(1, Ordering::AcqRel);
    }
}

fn spawn(
    name: &str,
    shared: &Arc<Shared>,
    finishes: bool,
    body: impl FnOnce() + Send + 'static,
) -> Result<JoinHandle<()>, ChannelError> {
    let presence = Presence::enter(shared, finishes);
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _presence = presence;
            claim(Latency::Interactive);
            body();
        })
        .map_err(|error| ChannelError::Unsupported(format!("Radar thread did not start: {error}")))
}

struct InBlock {
    samples: Vec<C32>,
    capacity: usize,
    lanes: usize,
    len: usize,
    first_index: u64,
    unix_ns: u64,
    gap_before: bool,
    phase_ready: bool,
    generation: u64,
}

impl InBlock {
    fn new(lanes: usize, capacity: usize) -> Box<Self> {
        Box::new(Self {
            samples: vec![C32::default(); lanes * capacity],
            capacity,
            lanes,
            len: 0,
            first_index: 0,
            unix_ns: 0,
            gap_before: false,
            phase_ready: false,
            generation: 0,
        })
    }

    fn lane(&self, lane: usize) -> &[C32] {
        let start = lane * self.capacity;
        self.samples.get(start..start + self.len).unwrap_or(&[])
    }

    fn lane_mut(&mut self, lane: usize, len: usize) -> &mut [C32] {
        let start = lane * self.capacity;
        self.samples.get_mut(start..start + len).unwrap_or(&mut [])
    }
}

struct JobPool {
    free: Consumer<Arc<CpiJob>>,
    ready: Producer<Arc<CpiJob>>,
    spare: Option<Arc<CpiJob>>,
    shared: Arc<Shared>,
    cpi: Thread,
}

impl JobSink for JobPool {
    fn take(&mut self) -> Option<Arc<CpiJob>> {
        self.free.pop().ok().or_else(|| self.spare.take())
    }

    fn submit(&mut self, job: Arc<CpiJob>) {
        match self.ready.push(job) {
            Ok(()) => {
                #[cfg(test)]
                self.shared.jobs_in.fetch_add(1, Ordering::Relaxed);
            }
            Err(PushError::Full(job)) => self.dropped(Some(job)),
        }
        self.cpi.unpark();
    }

    fn dropped(&mut self, unused: Option<Arc<CpiJob>>) {
        if unused.is_some() {
            self.spare = unused;
        }
        self.shared.dropped_cpis.fetch_add(1, Ordering::Relaxed);
    }
}

struct InLoop {
    shared: Arc<Shared>,
    front: FrontStage,
    blocks: Consumer<Box<InBlock>>,
    free: Producer<Box<InBlock>>,
    tunes: Consumer<LiveParams>,
    sink: JobPool,
    input_rate: f64,
    decimation: f64,
    load: f32,
}

impl InLoop {
    fn run(mut self) {
        loop {
            while let Ok(live) = self.tunes.pop() {
                self.front.tune(&live);
            }
            if self.shared.stopping() {
                return;
            }
            if self.held() {
                continue;
            }
            match self.blocks.pop() {
                Ok(block) => {
                    self.block(&block);
                    let returned = self.free.push(block);
                    debug_assert!(returned.is_ok());
                }
                Err(_) => std::thread::park_timeout(IDLE),
            }
        }
    }

    #[cfg(test)]
    fn held(&self) -> bool {
        let held = self.shared.hold_in.load(Ordering::Acquire);
        if held {
            std::thread::park_timeout(FOLLOW_POLL);
        }
        held
    }

    #[cfg(not(test))]
    const fn held(&self) -> bool {
        false
    }

    fn block(&mut self, block: &InBlock) {
        #[cfg(test)]
        if block.gap_before {
            self.shared.gaps_in.fetch_add(1, Ordering::Relaxed);
        }
        let mut lanes: [&[C32]; MAX_LANES] = [&[]; MAX_LANES];
        let count = block.lanes.min(MAX_LANES);
        for (lane, slot) in lanes.iter_mut().enumerate().take(count) {
            *slot = block.lane(lane);
        }
        let input = InLanes {
            lanes: &lanes[..count],
            first_index: block.first_index,
            unix_ns: block.unix_ns,
            gap_before: block.gap_before,
            phase_ready: block.phase_ready,
            generation: block.generation,
        };
        let stats = self.front.push(&input, &mut self.sink);
        if stats.mismatched {
            self.shared.lane_mismatch.fetch_add(1, Ordering::Relaxed);
        }
        if stats.overflowed_samples > 0 {
            let lost = (stats.overflowed_samples as f64 * self.decimation).round() as u64;
            self.shared
                .dropped_samples
                .fetch_add(lost, Ordering::Relaxed);
        }
        self.note_load(stats.busy_ns, block.len);
        #[cfg(test)]
        self.shared.blocks_done.fetch_add(1, Ordering::Release);
    }

    fn note_load(&mut self, busy_ns: u64, len: usize) {
        let span_ns = len as f64 / self.input_rate * NANOS_PER_SECOND;
        if span_ns.is_nan() || span_ns <= 0.0 {
            return;
        }
        let now = (busy_ns as f64 / span_ns) as f32;
        self.load += LOAD_SMOOTHING * (now - self.load);
        self.shared
            .front_load
            .store(self.load.to_bits(), Ordering::Relaxed);
    }
}

enum RadarCommand {
    Tune(Box<LiveParams>),
    ClearTracks,
    Follow { shared: Arc<Shared>, seen: Carried },
}

struct Follow {
    shared: Arc<Shared>,
    seen: Carried,
    since: Instant,
}

struct CpiLoop {
    shared: Arc<Shared>,
    stage: CpiStage,
    pod: Box<RadarPod>,
    jobs: Consumer<Arc<CpiJob>>,
    free: Producer<Arc<CpiJob>>,
    commands: Consumer<RadarCommand>,
    publisher: Publisher,
    handled: u64,
    follow: Option<Follow>,
    stage_seen: [u64; 3],
    warned: bool,
}

impl CpiLoop {
    fn run(mut self) {
        loop {
            self.commands();
            if self.shared.stopping() {
                self.finish();
                return;
            }
            if self.following() {
                std::thread::park_timeout(FOLLOW_POLL);
                continue;
            }
            let current = self.shared.generation.load(Ordering::Acquire);
            if current > self.handled {
                self.retune(current);
            }
            match self.jobs.pop() {
                Ok(job) => self.job(job),
                Err(_) => std::thread::park_timeout(IDLE),
            }
        }
    }

    fn commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                RadarCommand::Tune(live) => {
                    if let Err(error) = self.stage.tune(&live) {
                        tracing::error!(%error, "radar settings refused on the CPI thread");
                    }
                }
                RadarCommand::ClearTracks => {
                    self.stage.clear_tracks(&mut self.pod);
                    self.publish(false);
                }
                RadarCommand::Follow { shared, seen } => {
                    self.follow = Some(Follow {
                        shared,
                        seen,
                        since: Instant::now(),
                    });
                }
            }
        }
    }

    fn following(&mut self) -> bool {
        let Some(follow) = &self.follow else {
            return false;
        };
        let finished = follow.shared.finished.load(Ordering::Acquire);
        if !finished && follow.since.elapsed() < FOLLOW_WAIT {
            return true;
        }
        self.carry();
        false
    }

    fn carry(&mut self) {
        if let Some(follow) = self.follow.take() {
            self.shared.inherit_rest(&follow.shared, &follow.seen);
            self.stage
                .carry_ids(follow.shared.next_track_id.load(Ordering::Acquire));
        }
    }

    fn job(&mut self, job: Arc<CpiJob>) {
        if job.generation < self.handled {
            self.shared.discarded_cpis.fetch_add(1, Ordering::Relaxed);
            self.recycle(job);
        } else {
            if job.generation > self.handled {
                self.retune(job.generation);
            }
            let outcome = self.stage.run_shared(&job, &mut self.pod);
            self.recycle(job);
            if let Some(error) = &outcome.failed {
                self.failed(error);
            }
            self.publish(true);
        }
        #[cfg(test)]
        self.shared.jobs_out.fetch_add(1, Ordering::Release);
    }

    fn failed(&mut self, error: &CafError) {
        self.shared.dropped_cpis.fetch_add(1, Ordering::Relaxed);
        if !self.warned {
            self.warned = true;
            tracing::error!(%error, "a radar CPI failed and was dropped");
        }
    }

    fn retune(&mut self, generation: u64) {
        self.stage.retuned(&mut self.pod);
        self.publish(false);
        self.handled = generation;
    }

    fn recycle(&mut self, job: Arc<CpiJob>) {
        let returned = self.free.push(job);
        debug_assert!(returned.is_ok());
    }

    fn publish(&mut self, with_surface: bool) {
        self.note();
        if !self.publisher.publish(&self.pod, with_surface) {
            self.shared.dropped_reports.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn note(&mut self) {
        let shared = &self.shared;
        let stage = self.stage.counters();
        let totals = [
            &shared.unsuppressed_groups,
            &shared.truncated_detections,
            &shared.dropped_tracks,
        ];
        let now = [
            stage.unsuppressed_groups,
            stage.truncated_detections,
            stage.dropped_tracks,
        ];
        for ((total, now), seen) in totals.into_iter().zip(now).zip(&mut self.stage_seen) {
            total.fetch_add(now.saturating_sub(*seen), Ordering::Relaxed);
            *seen = now;
        }
        let counters = &mut self.pod.counters;
        counters.unsuppressed_groups = shared.unsuppressed_groups.load(Ordering::Relaxed);
        counters.truncated_detections = shared.truncated_detections.load(Ordering::Relaxed);
        counters.dropped_tracks = shared.dropped_tracks.load(Ordering::Relaxed);
        counters.dropped_samples = shared.dropped_samples.load(Ordering::Relaxed);
        counters.dropped_cpis = shared.dropped_cpis.load(Ordering::Relaxed);
        counters.discarded_cpis = shared.discarded_cpis.load(Ordering::Relaxed);
        counters.dropped_reports = shared.dropped_reports.load(Ordering::Relaxed);
        counters.gpu_failures = shared.gpu_failures.load(Ordering::Relaxed);
        self.pod.front_load = f32::from_bits(shared.front_load.load(Ordering::Relaxed));
        shared
            .next_track_id
            .store(self.stage.next_track_id(), Ordering::Release);
    }

    fn finish(&mut self) {
        while let Ok(job) = self.jobs.pop() {
            self.shared.discarded_cpis.fetch_add(1, Ordering::Relaxed);
            self.recycle(job);
        }
        self.carry();
        self.stage.retuned(&mut self.pod);
        self.publish(false);
    }
}

struct Outbox {
    commands: Producer<RadarCommand>,
    hops: Producer<LiveParams>,
    held: Option<RadarCommand>,
    clear: bool,
    tune: Option<Box<LiveParams>>,
    hop: Option<LiveParams>,
    in_thread: Thread,
    cpi_thread: Thread,
}

impl Outbox {
    const fn pending(&self) -> bool {
        self.held.is_some() || self.clear || self.tune.is_some() || self.hop.is_some()
    }

    fn flush(&mut self) {
        if let Some(live) = self.hop.take() {
            match self.hops.push(live) {
                Ok(()) => self.in_thread.unpark(),
                Err(PushError::Full(live)) => self.hop = Some(live),
            }
        }
        if let Some(command) = self.held.take() {
            self.push_or_hold(command);
        }
        if self.held.is_none() && std::mem::take(&mut self.clear) {
            self.push_or_hold(RadarCommand::ClearTracks);
        }
        if self.held.is_none()
            && !self.clear
            && let Some(live) = self.tune.take()
        {
            self.push_or_hold(RadarCommand::Tune(live));
        }
    }

    fn send(&mut self, command: RadarCommand) {
        self.flush();
        if self.held.is_some() || self.clear || self.tune.is_some() {
            self.hold(command);
        } else {
            self.push_or_hold(command);
        }
    }

    fn tune(&mut self, live: Box<LiveParams>) {
        self.hop = Some(*live);
        self.send(RadarCommand::Tune(live));
    }

    fn push_or_hold(&mut self, command: RadarCommand) {
        match self.commands.push(command) {
            Ok(()) => self.cpi_thread.unpark(),
            Err(PushError::Full(command)) => self.hold(command),
        }
    }

    fn hold(&mut self, command: RadarCommand) {
        match command {
            RadarCommand::Tune(live) => match self.tune.as_mut() {
                Some(pending) => **pending = *live,
                None => self.tune = Some(live),
            },
            RadarCommand::ClearTracks => self.clear = true,
            follow @ RadarCommand::Follow { .. } => self.held = Some(follow),
        }
    }
}

pub(crate) struct RadarWorker {
    shared: Arc<Shared>,
    blocks: Producer<Box<InBlock>>,
    free: Consumer<Box<InBlock>>,
    spare: Option<Box<InBlock>>,
    outbox: Outbox,
    mailbox: Option<Mailbox>,
    predecessors: [Option<Mailbox>; HANDOVERS],
    lanes: [usize; MAX_LANES],
    lane_count: usize,
    elements: usize,
    reference: usize,
    capacity: usize,
    input_rate: f64,
    center_hz: f64,
    generation: u64,
    gap: bool,
    in_thread: Thread,
    cpi_thread: Thread,
    threads: Vec<JoinHandle<()>>,
    #[cfg(test)]
    blocks_pushed: u64,
}

struct Rings {
    blocks: (Producer<Box<InBlock>>, Consumer<Box<InBlock>>),
    free_blocks: (Producer<Box<InBlock>>, Consumer<Box<InBlock>>),
    jobs: (Producer<Arc<CpiJob>>, Consumer<Arc<CpiJob>>),
    free_jobs: (Producer<Arc<CpiJob>>, Consumer<Arc<CpiJob>>),
    commands: (Producer<RadarCommand>, Consumer<RadarCommand>),
    tunes: (Producer<LiveParams>, Consumer<LiveParams>),
}

impl Rings {
    fn new(stage: &StagePlan, capacity: usize) -> Self {
        let lanes = stage.front.lanes.len();
        let count = (IN_SECONDS * stage.front.input_rate / capacity as f64).ceil() as usize;
        let count = count + IN_SLACK;
        let blocks = RingBuffer::new(count);
        let mut free_blocks = RingBuffer::new(count);
        for _ in 0..count {
            let filled = free_blocks.0.push(InBlock::new(lanes, capacity));
            debug_assert!(filled.is_ok());
        }
        let jobs = RingBuffer::new(JOBS);
        let mut free_jobs = RingBuffer::new(JOBS);
        for _ in 0..JOBS {
            let job = Arc::new(CpiJob::new(lanes, stage.shape.window()));
            let filled = free_jobs.0.push(job);
            debug_assert!(filled.is_ok());
        }
        Self {
            blocks,
            free_blocks,
            jobs,
            free_jobs,
            commands: RingBuffer::new(COMMANDS),
            tunes: RingBuffer::new(COMMANDS),
        }
    }
}

impl RadarWorker {
    pub(crate) fn start(
        stage: &StagePlan,
        backend: Box<dyn CafBackend>,
        block: usize,
        shared: Arc<Shared>,
    ) -> Result<Self, ChannelError> {
        let lane_count = stage.front.lanes.len();
        if lane_count == 0 || lane_count > MAX_LANES {
            return Err(ChannelError::Refused("Surveillance not in the array"));
        }
        let capacity = block.max(1);
        let front = FrontStage::new(stage)?;
        let cpi = CpiStage::new(stage, backend)?;
        let pod = RadarPod::new(stage);
        let (publisher, mailbox) = publish::channel(stage.shape.gates * stage.report_rows.len());
        let rings = Rings::new(stage, capacity);
        let cpi_loop = CpiLoop {
            shared: Arc::clone(&shared),
            stage: cpi,
            pod,
            jobs: rings.jobs.1,
            free: rings.free_jobs.0,
            commands: rings.commands.1,
            publisher,
            handled: 0,
            follow: None,
            stage_seen: [0; 3],
            warned: false,
        };
        let cpi_handle = spawn("sdrmm-radar-cpi", &shared, true, move || cpi_loop.run())?;
        let cpi_thread = cpi_handle.thread().clone();
        let in_loop = InLoop {
            shared: Arc::clone(&shared),
            front,
            blocks: rings.blocks.1,
            free: rings.free_blocks.0,
            tunes: rings.tunes.1,
            sink: JobPool {
                free: rings.free_jobs.1,
                ready: rings.jobs.0,
                spare: None,
                shared: Arc::clone(&shared),
                cpi: cpi_thread.clone(),
            },
            input_rate: stage.front.input_rate,
            decimation: stage.front.input_rate / stage.front.radar_rate,
            load: 0.0,
        };
        let in_handle = match spawn("sdrmm-radar-in", &shared, false, move || in_loop.run()) {
            Ok(handle) => handle,
            Err(error) => {
                shared.stop.store(true, Ordering::Release);
                cpi_thread.unpark();
                if cpi_handle.join().is_err() {
                    tracing::error!("radar CPI thread panicked while starting");
                }
                return Err(error);
            }
        };
        let in_thread = in_handle.thread().clone();
        let mut lanes = [0; MAX_LANES];
        lanes[..lane_count].copy_from_slice(&stage.front.lanes);
        Ok(Self {
            blocks: rings.blocks.0,
            free: rings.free_blocks.1,
            spare: None,
            outbox: Outbox {
                commands: rings.commands.0,
                hops: rings.tunes.0,
                held: None,
                clear: false,
                tune: None,
                hop: None,
                in_thread: in_thread.clone(),
                cpi_thread: cpi_thread.clone(),
            },
            mailbox: Some(mailbox),
            predecessors: Default::default(),
            lanes,
            lane_count,
            elements: stage.ctx.elements,
            reference: stage.front.lanes[0],
            capacity,
            input_rate: stage.front.input_rate,
            center_hz: stage.ctx.center_hz,
            generation: 0,
            gap: false,
            in_thread,
            cpi_thread,
            threads: vec![in_handle, cpi_handle],
            shared,
            #[cfg(test)]
            blocks_pushed: 0,
        })
    }

    #[cfg(test)]
    pub(super) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    #[cfg(test)]
    pub(super) fn settled(&self) -> bool {
        let shared = &self.shared;
        shared.blocks_done.load(Ordering::Acquire) == self.blocks_pushed
            && shared.jobs_in.load(Ordering::Acquire) == shared.jobs_out.load(Ordering::Acquire)
            && !self.outbox.pending()
    }

    fn track_center(&mut self, centers_hz: &[f64]) {
        let Some(&center) = centers_hz.get(self.reference) else {
            return;
        };
        if (center - self.center_hz).abs() <= TUNING_TOLERANCE_HZ {
            return;
        }
        self.center_hz = center;
        self.generation += 1;
        self.shared.retunes.fetch_add(1, Ordering::Relaxed);
        self.shared
            .generation
            .store(self.generation, Ordering::Release);
        self.cpi_thread.unpark();
    }

    fn copy_chunk(&mut self, block: &ArrayBlock<'_>, at: usize, len: usize, gap: bool) {
        let Some(mut slot) = self.spare.take().or_else(|| self.free.pop().ok()) else {
            self.drop_chunk(len);
            return;
        };
        let mut short = false;
        for (lane, &element) in self.lanes[..self.lane_count].iter().enumerate() {
            let source = block
                .lanes
                .get(element)
                .and_then(|samples| samples.get(at..at + len))
                .unwrap_or(&[]);
            let target = slot.lane_mut(lane, len);
            if source.len() == target.len() {
                target.copy_from_slice(source);
            } else {
                target.fill(C32::default());
                short = true;
            }
        }
        if short {
            self.shared.lane_mismatch.fetch_add(1, Ordering::Relaxed);
        }
        slot.len = len;
        slot.first_index = block.first_index + at as u64;
        let offset_ns = (at as f64 / self.input_rate * NANOS_PER_SECOND) as u64;
        slot.unix_ns = block.unix_ns.saturating_add(offset_ns);
        slot.gap_before = gap || std::mem::take(&mut self.gap);
        slot.phase_ready = block.cal.phase_ready;
        slot.generation = self.generation;
        match self.blocks.push(slot) {
            Ok(()) => {
                #[cfg(test)]
                {
                    self.blocks_pushed += 1;
                }
            }
            Err(PushError::Full(slot)) => {
                self.spare = Some(slot);
                self.drop_chunk(len);
            }
        }
    }

    fn drop_chunk(&mut self, len: usize) {
        self.shared
            .dropped_samples
            .fetch_add(len as u64, Ordering::Relaxed);
        self.shared.dropped_blocks.fetch_add(1, Ordering::Relaxed);
        self.gap = true;
    }

    fn adopt(&mut self, old: &mut Self) {
        old.shared.closing.store(true, Ordering::Release);
        old.in_thread.unpark();
        old.cpi_thread.unpark();
        let seen = self.shared.inherit(&old.shared);
        self.predecessors = std::mem::take(&mut old.predecessors);
        if let Some(mailbox) = old.mailbox.take() {
            if self.predecessors.iter().all(Option::is_some) {
                self.predecessors.rotate_left(1);
                old.mailbox = self.predecessors[HANDOVERS - 1].take();
            }
            if let Some(slot) = self.predecessors.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some(mailbox);
            }
        }
        self.outbox.send(RadarCommand::Follow {
            shared: Arc::clone(&old.shared),
            seen,
        });
    }

    fn halt(&self) {
        self.shared.stop.store(true, Ordering::Release);
        self.in_thread.unpark();
        self.cpi_thread.unpark();
    }
}

impl DedicatedRunner for RadarWorker {
    fn push(&mut self, block: &ArrayBlock<'_>) {
        self.outbox.flush();
        if block.lanes.len() != self.elements {
            self.shared.lane_mismatch.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.track_center(block.centers_hz);
        let total = block.len();
        let mut at = 0;
        let mut gap = block.gap_before;
        while at < total {
            let len = (total - at).min(self.capacity);
            self.copy_chunk(block, at, len, gap);
            gap = false;
            at += len;
        }
        self.in_thread.unpark();
    }

    fn poll(&mut self, out: &mut ProcessorOutput<'_>) {
        self.outbox.flush();
        for predecessor in self.predecessors.iter_mut().flatten() {
            if predecessor.deliver(out) {
                return;
            }
        }
        if let Some(mailbox) = self.mailbox.as_mut() {
            mailbox.deliver(out);
        }
    }

    fn commit(&mut self, prepared: Prepared) -> Option<Box<dyn DedicatedRunner>> {
        match prepared {
            Prepared::Same => None,
            Prepared::Live(live) => {
                self.outbox.tune(live);
                None
            }
            Prepared::Rebuild(mut next) => {
                match next.as_any_mut().downcast_mut::<Self>() {
                    Some(worker) => {
                        worker.adopt(self);
                        std::mem::swap(self, worker);
                    }
                    None => {
                        tracing::error!(
                            "radar rebuild skipped: the new runner is not a radar worker"
                        )
                    }
                }
                Some(next)
            }
        }
    }

    fn action(&mut self, action: ProcessorAction) -> Result<(), ChannelError> {
        match action {
            ProcessorAction::ClearTracks => {
                self.outbox.send(RadarCommand::ClearTracks);
                Ok(())
            }
        }
    }

    fn faults(&self) -> ProcessorFaults {
        self.shared.faults()
    }

    fn dropped_samples(&self) -> u64 {
        self.shared.dropped_samples.load(Ordering::Relaxed)
    }

    fn running(&self) -> bool {
        !self.shared.dead.load(Ordering::Acquire)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn retire(mut self: Box<Self>) -> Retired {
        self.halt();
        Retired::new(std::mem::take(&mut self.threads))
    }
}

impl Drop for RadarWorker {
    fn drop(&mut self) {
        self.halt();
    }
}
