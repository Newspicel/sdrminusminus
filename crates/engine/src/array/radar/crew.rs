use std::{
    ops::Range,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{JoinHandle, Thread},
    time::Duration,
};

use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_channels::{
    ChannelError,
    passive_radar::{
        CafBackend, CafError, CpiJob, CubeOut, GateChunk, RadarPlan as StagePlan, SpectraShard,
        doppler_stage, gather_stage, residual_stage, solve_stage, spectra_stage,
    },
};
use sdrmm_device::{Latency, schedule::claim};
use sdrmm_dsp::radar::{
    batch::{BatchKernel, BatchShape},
    wiener::{GroupPlan, GroupSums, WeightTable, WienerSolver},
};

use super::{MAX_CREW, worker::Shared};

const MAX_PARTS: usize = MAX_CREW + 1;
const TASK_SLOTS: usize = 2;
const WAIT: Duration = Duration::from_millis(1);
const IDLE: Duration = Duration::from_millis(10);

enum Task {
    Spectra {
        shard: Box<SpectraShard>,
        job: Arc<CpiJob>,
    },
    Residual {
        shard: Box<SpectraShard>,
        table: Option<Arc<WeightTable>>,
    },
    Doppler {
        chunk: Box<GateChunk>,
        window: Arc<[f32]>,
    },
}

impl Task {
    fn into_part(self) -> Part {
        match self {
            Self::Spectra { shard, .. } | Self::Residual { shard, .. } => Part::Shard(shard),
            Self::Doppler { chunk, .. } => Part::Chunk(chunk),
        }
    }
}

enum Part {
    Shard(Box<SpectraShard>),
    Chunk(Box<GateChunk>),
}

struct Done {
    part: Part,
    result: Result<(), CafError>,
}

enum Failure {
    Lost,
    Stage(CafError),
}

impl From<CafError> for Failure {
    fn from(error: CafError) -> Self {
        Self::Stage(error)
    }
}

struct HelperPresence {
    shared: Arc<Shared>,
    alive: Arc<AtomicBool>,
}

impl HelperPresence {
    fn enter(shared: &Arc<Shared>, alive: &Arc<AtomicBool>) -> Self {
        shared.live.fetch_add(1, Ordering::AcqRel);
        Self {
            shared: Arc::clone(shared),
            alive: Arc::clone(alive),
        }
    }
}

impl Drop for HelperPresence {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.shared.live.fetch_sub(1, Ordering::AcqRel);
    }
}

struct HelperLoop {
    kernel: BatchKernel,
    tasks: Consumer<Task>,
    done: Producer<Done>,
    coordinator: Arc<OnceLock<Thread>>,
    #[cfg(test)]
    poison: Arc<std::sync::atomic::AtomicU8>,
}

impl HelperLoop {
    fn run(mut self) {
        loop {
            self.poisoned(POISON_NOW);
            match self.tasks.pop() {
                Ok(task) => {
                    self.poisoned(POISON_ON_TASK);
                    let done = self.work(task);
                    if self.done.push(done).is_err() {
                        return;
                    }
                    if let Some(coordinator) = self.coordinator.get() {
                        coordinator.unpark();
                    }
                }
                Err(_) if self.tasks.is_abandoned() => return,
                Err(_) => std::thread::park_timeout(IDLE),
            }
        }
    }

    #[cfg(test)]
    fn poisoned(&self, mode: u8) {
        assert_ne!(
            self.poison.load(Ordering::Acquire),
            mode,
            "radar crew helper poisoned"
        );
    }

    #[cfg(not(test))]
    const fn poisoned(&self, _mode: u8) {}

    fn work(&mut self, task: Task) -> Done {
        match task {
            Task::Spectra { mut shard, job } => {
                let result = spectra_stage(&mut self.kernel, &job, &mut shard);
                drop(job);
                Done {
                    part: Part::Shard(shard),
                    result,
                }
            }
            Task::Residual { mut shard, table } => {
                let result = residual_stage(&mut self.kernel, table.as_deref(), &mut shard);
                drop(table);
                Done {
                    part: Part::Shard(shard),
                    result,
                }
            }
            Task::Doppler { mut chunk, window } => {
                let result = doppler_stage(&mut self.kernel, &window, &mut chunk.series);
                drop(window);
                Done {
                    part: Part::Chunk(chunk),
                    result,
                }
            }
        }
    }
}

const POISON_NOW: u8 = 1;
const POISON_ON_TASK: u8 = 2;

struct Helper {
    tasks: Option<Producer<Task>>,
    done: Consumer<Done>,
    alive: Arc<AtomicBool>,
    wake: Thread,
    thread: Option<JoinHandle<()>>,
    #[cfg(test)]
    poison: Arc<std::sync::atomic::AtomicU8>,
}

impl Helper {
    fn spawn(
        index: usize,
        shape: BatchShape,
        shared: &Arc<Shared>,
        coordinator: &Arc<OnceLock<Thread>>,
    ) -> Result<Self, ChannelError> {
        let (tasks, task_rx) = RingBuffer::new(TASK_SLOTS);
        let (done_tx, done) = RingBuffer::new(TASK_SLOTS);
        let alive = Arc::new(AtomicBool::new(true));
        #[cfg(test)]
        let poison = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let helper = HelperLoop {
            kernel: BatchKernel::new(shape),
            tasks: task_rx,
            done: done_tx,
            coordinator: Arc::clone(coordinator),
            #[cfg(test)]
            poison: Arc::clone(&poison),
        };
        let presence = HelperPresence::enter(shared, &alive);
        let thread = std::thread::Builder::new()
            .name(format!("sdrmm-radar-crew-{index}"))
            .spawn(move || {
                let _presence = presence;
                claim(Latency::Interactive);
                helper.run();
            })
            .map_err(|error| {
                ChannelError::Unsupported(format!("Radar crew did not start: {error}"))
            })?;
        Ok(Self {
            tasks: Some(tasks),
            done,
            alive,
            wake: thread.thread().clone(),
            thread: Some(thread),
            #[cfg(test)]
            poison,
        })
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::Acquire) && self.tasks.is_some()
    }

    fn send(&mut self, task: Task) -> Result<(), Task> {
        let Some(tasks) = self.tasks.as_mut() else {
            return Err(task);
        };
        match tasks.push(task) {
            Ok(()) => {
                self.wake.unpark();
                Ok(())
            }
            Err(PushError::Full(task)) => Err(task),
        }
    }

    fn release(&mut self) {
        self.tasks = None;
        self.wake.unpark();
    }
}

struct Eca {
    solver: WienerSolver,
    sums: GroupSums,
    table: Arc<WeightTable>,
    groups: GroupPlan,
}

pub(super) struct CrewCaf {
    shape: BatchShape,
    helpers: Vec<Helper>,
    coordinator: Arc<OnceLock<Thread>>,
    kernel: BatchKernel,
    eca: Option<Eca>,
    shards: [Option<Box<SpectraShard>>; MAX_PARTS],
    chunks: [Option<Box<GateChunk>>; MAX_PARTS],
    batch_parts: [Range<usize>; MAX_PARTS],
    gate_parts: [Range<usize>; MAX_PARTS],
    parts: usize,
    window: Arc<[f32]>,
    fallen: bool,
}

fn split(total: usize, parts: usize, index: usize) -> Range<usize> {
    if index >= parts {
        return 0..0;
    }
    total * index / parts..total * (index + 1) / parts
}

fn eca_of(stage: &StagePlan) -> Result<Option<Eca>, ChannelError> {
    let shape = stage.shape;
    let Some(groups) = stage.groups.as_ref().filter(|_| shape.eca()) else {
        return Ok(None);
    };
    let refused = |_| ChannelError::Refused("Clutter order over 512");
    Ok(Some(Eca {
        solver: WienerSolver::new(shape, groups, stage.params.clutter.loading).map_err(refused)?,
        sums: GroupSums::new(&shape, groups),
        table: Arc::new(WeightTable::new(&shape, groups).map_err(refused)?),
        groups: groups.clone(),
    }))
}

impl CrewCaf {
    pub(super) fn new(
        stage: &StagePlan,
        helpers: usize,
        shared: &Arc<Shared>,
    ) -> Result<Self, ChannelError> {
        let shape = stage.shape;
        let parts = (helpers.min(MAX_CREW) + 1)
            .min(shape.batches)
            .min(shape.gates)
            .max(1);
        let batch_parts: [Range<usize>; MAX_PARTS] =
            std::array::from_fn(|index| split(shape.batches, parts, index));
        let gate_parts: [Range<usize>; MAX_PARTS] =
            std::array::from_fn(|index| split(shape.gates, parts, index));
        let mut crew = Self {
            shape,
            helpers: Vec::with_capacity(parts - 1),
            coordinator: Arc::new(OnceLock::new()),
            kernel: BatchKernel::new(shape),
            eca: eca_of(stage)?,
            shards: std::array::from_fn(|index| {
                (index < parts)
                    .then(|| Box::new(SpectraShard::new(&shape, batch_parts[index].clone())))
            }),
            chunks: std::array::from_fn(|index| {
                (index < parts).then(|| Box::new(GateChunk::new(&shape, gate_parts[index].clone())))
            }),
            batch_parts,
            gate_parts,
            parts,
            window: Arc::from(stage.window.as_slice()),
            fallen: false,
        };
        for index in 0..parts - 1 {
            let helper = Helper::spawn(index, shape, shared, &crew.coordinator)?;
            crew.helpers.push(helper);
        }
        Ok(crew)
    }

    fn check(&self, job: &CpiJob, out: &CubeOut) -> Result<(), CafError> {
        let shape = self.shape;
        if job.window < shape.window()
            || out.cube.len() != shape.lanes * shape.gates * shape.batches
        {
            return Err(CafError::Shape);
        }
        Ok(())
    }

    fn fall(&mut self) {
        if self.fallen {
            return;
        }
        self.fallen = true;
        for helper in &mut self.helpers {
            helper.release();
        }
        tracing::warn!("a radar crew helper stopped, the CAF continues on one thread");
    }

    fn recover(&mut self) {
        let shape = self.shape;
        for index in 0..self.parts {
            if self.shards[index].is_none() {
                let batches = self.batch_parts[index].clone();
                self.shards[index] = Some(Box::new(SpectraShard::new(&shape, batches)));
            }
            if self.chunks[index].is_none() {
                let gates = self.gate_parts[index].clone();
                self.chunks[index] = Some(Box::new(GateChunk::new(&shape, gates)));
            }
        }
        if let Some(eca) = self.eca.as_mut()
            && Arc::get_mut(&mut eca.table).is_none()
            && let Ok(table) = WeightTable::new(&shape, &eca.groups)
        {
            eca.table = Arc::new(table);
        }
    }

    fn together(&mut self, job: &Arc<CpiJob>, out: &mut CubeOut) -> Result<(), Failure> {
        let dispatched = self.send_shards(|shard| Task::Spectra {
            shard,
            job: Arc::clone(job),
        });
        let local = self.spectra_local(job);
        let collected = self.collect(dispatched);
        collected?;
        local?;
        self.solve(job, out)?;
        let table = self.eca.as_ref().map(|eca| Arc::clone(&eca.table));
        let dispatched = self.send_shards(|shard| Task::Residual {
            shard,
            table: table.clone(),
        });
        drop(table);
        let local = self.residual_local();
        let collected = self.collect(dispatched);
        collected?;
        local?;
        self.gather()?;
        let window = Arc::clone(&self.window);
        let dispatched = self.send_chunks(|chunk| Task::Doppler {
            chunk,
            window: Arc::clone(&window),
        });
        let local = self.doppler_local(0..1);
        let collected = self.collect(dispatched);
        collected?;
        local?;
        self.copy_out(out).map_err(Failure::Stage)
    }

    fn alone(&mut self, job: &CpiJob, out: &mut CubeOut) -> Result<(), Failure> {
        for index in 0..self.parts {
            let shard = self.shards[index].as_deref_mut().ok_or(Failure::Lost)?;
            spectra_stage(&mut self.kernel, job, shard)?;
        }
        self.solve(job, out)?;
        let table = self.eca.as_ref().map(|eca| &*eca.table);
        for shard in self.shards.iter_mut().take(self.parts) {
            let shard = shard.as_deref_mut().ok_or(Failure::Lost)?;
            residual_stage(&mut self.kernel, table, shard)?;
        }
        self.gather()?;
        self.doppler_local(0..self.parts)?;
        self.copy_out(out).map_err(Failure::Stage)
    }

    fn send_shards(&mut self, task: impl Fn(Box<SpectraShard>) -> Task) -> usize {
        let mut sent = 0;
        for (index, helper) in self.helpers.iter_mut().enumerate() {
            let Some(shard) = self.shards[index + 1].take() else {
                break;
            };
            if let Err(task) = helper.send(task(shard)) {
                restore(
                    &mut self.shards,
                    &mut self.chunks,
                    index + 1,
                    task.into_part(),
                );
                break;
            }
            sent += 1;
        }
        sent
    }

    fn send_chunks(&mut self, task: impl Fn(Box<GateChunk>) -> Task) -> usize {
        let mut sent = 0;
        for (index, helper) in self.helpers.iter_mut().enumerate() {
            let Some(chunk) = self.chunks[index + 1].take() else {
                break;
            };
            if let Err(task) = helper.send(task(chunk)) {
                restore(
                    &mut self.shards,
                    &mut self.chunks,
                    index + 1,
                    task.into_part(),
                );
                break;
            }
            sent += 1;
        }
        sent
    }

    fn collect(&mut self, dispatched: usize) -> Result<(), Failure> {
        let mut pending = [false; MAX_PARTS];
        for flag in pending.iter_mut().take(dispatched) {
            *flag = true;
        }
        let mut outcome = Ok(());
        let mut lost = dispatched < self.helpers.len();
        loop {
            let mut waiting = false;
            for (index, helper) in self.helpers.iter_mut().enumerate() {
                if !pending[index] {
                    continue;
                }
                let dead = !helper.alive.load(Ordering::Acquire);
                match helper.done.pop() {
                    Ok(done) => {
                        pending[index] = false;
                        restore(&mut self.shards, &mut self.chunks, index + 1, done.part);
                        if let Err(error) = done.result
                            && outcome.is_ok()
                        {
                            outcome = Err(error);
                        }
                    }
                    Err(_) if dead => {
                        pending[index] = false;
                        lost = true;
                    }
                    Err(_) => waiting = true,
                }
            }
            if !waiting {
                break;
            }
            std::thread::park_timeout(WAIT);
        }
        if lost {
            return Err(Failure::Lost);
        }
        outcome.map_err(Failure::Stage)
    }

    fn spectra_local(&mut self, job: &CpiJob) -> Result<(), CafError> {
        let shard = self.shards[0].as_deref_mut().ok_or(CafError::Crew)?;
        spectra_stage(&mut self.kernel, job, shard)
    }

    fn residual_local(&mut self) -> Result<(), CafError> {
        let table = self.eca.as_ref().map(|eca| &*eca.table);
        let shard = self.shards[0].as_deref_mut().ok_or(CafError::Crew)?;
        residual_stage(&mut self.kernel, table, shard)
    }

    fn doppler_local(&mut self, parts: Range<usize>) -> Result<(), CafError> {
        for chunk in self.chunks.iter_mut().take(parts.end).skip(parts.start) {
            let chunk = chunk.as_deref_mut().ok_or(CafError::Crew)?;
            doppler_stage(&mut self.kernel, &self.window, &mut chunk.series)?;
        }
        Ok(())
    }

    fn solve(&mut self, job: &CpiJob, out: &mut CubeOut) -> Result<(), Failure> {
        let Some(eca) = self.eca.as_mut() else {
            out.suppression_db = job.front_suppression_db;
            out.unsuppressed_groups = 0;
            return Ok(());
        };
        let shards = shard_refs(&self.shards, self.parts).ok_or(Failure::Lost)?;
        let table = Arc::get_mut(&mut eca.table).ok_or(Failure::Lost)?;
        let stats = solve_stage(&mut eca.solver, &shards[..self.parts], &mut eca.sums, table)?;
        out.suppression_db = stats.suppression_db;
        out.unsuppressed_groups = stats.unsuppressed_groups;
        Ok(())
    }

    fn gather(&mut self) -> Result<(), Failure> {
        let shards = shard_refs(&self.shards, self.parts).ok_or(Failure::Lost)?;
        for (chunk, gates) in self
            .chunks
            .iter_mut()
            .zip(&self.gate_parts)
            .take(self.parts)
        {
            let chunk = chunk.as_deref_mut().ok_or(Failure::Lost)?;
            gather_stage(
                &shards[..self.parts],
                &self.shape,
                gates.clone(),
                &mut chunk.series,
            )?;
        }
        Ok(())
    }

    fn copy_out(&self, out: &mut CubeOut) -> Result<(), CafError> {
        let BatchShape {
            gates,
            batches,
            lanes,
            ..
        } = self.shape;
        for (chunk, range) in self.chunks.iter().zip(&self.gate_parts).take(self.parts) {
            let chunk = chunk.as_deref().ok_or(CafError::Crew)?;
            let run = range.len() * batches;
            for lane in 0..lanes {
                let source = chunk
                    .series
                    .get(lane * run..(lane + 1) * run)
                    .ok_or(CafError::Shape)?;
                let start = (lane * gates + range.start) * batches;
                let target = out
                    .cube
                    .get_mut(start..start + run)
                    .ok_or(CafError::Shape)?;
                target.copy_from_slice(source);
            }
        }
        Ok(())
    }

    fn settle(
        &mut self,
        result: Result<(), Failure>,
        job: &CpiJob,
        out: &mut CubeOut,
    ) -> Result<(), CafError> {
        match result {
            Ok(()) => Ok(()),
            Err(Failure::Stage(error)) => Err(error),
            Err(Failure::Lost) => {
                self.fall();
                self.recover();
                match self.alone(job, out) {
                    Ok(()) => Ok(()),
                    Err(Failure::Stage(error)) => Err(error),
                    Err(Failure::Lost) => Err(CafError::Crew),
                }
            }
        }
    }

    #[cfg(test)]
    fn poison(&self, helper: usize, mode: u8) {
        if let Some(helper) = self.helpers.get(helper) {
            helper.poison.store(mode, Ordering::Release);
            helper.wake.unpark();
        }
    }

    #[cfg(test)]
    fn helper_alive(&self, helper: usize) -> bool {
        self.helpers
            .get(helper)
            .is_some_and(|helper| helper.alive.load(Ordering::Acquire))
    }
}

fn restore(
    shards: &mut [Option<Box<SpectraShard>>; MAX_PARTS],
    chunks: &mut [Option<Box<GateChunk>>; MAX_PARTS],
    index: usize,
    part: Part,
) {
    match part {
        Part::Shard(shard) => {
            if let Some(slot) = shards.get_mut(index) {
                *slot = Some(shard);
            }
        }
        Part::Chunk(chunk) => {
            if let Some(slot) = chunks.get_mut(index) {
                *slot = Some(chunk);
            }
        }
    }
}

fn shard_refs(
    shards: &[Option<Box<SpectraShard>>; MAX_PARTS],
    parts: usize,
) -> Option<[&SpectraShard; MAX_PARTS]> {
    let first = shards[0].as_deref()?;
    let mut refs = [first; MAX_PARTS];
    for (slot, shard) in refs.iter_mut().zip(shards).take(parts) {
        *slot = shard.as_deref()?;
    }
    Some(refs)
}

impl CafBackend for CrewCaf {
    fn run(&mut self, job: &CpiJob, out: &mut CubeOut) -> Result<(), CafError> {
        self.check(job, out)?;
        let result = self.alone(job, out);
        self.settle(result, job, out)
    }

    fn run_shared(&mut self, job: &Arc<CpiJob>, out: &mut CubeOut) -> Result<(), CafError> {
        self.check(job, out)?;
        if !self.fallen && self.helpers.iter().any(|helper| !helper.alive()) {
            self.fall();
        }
        if self.fallen {
            let result = self.alone(job, out);
            return self.settle(result, job, out);
        }
        self.coordinator.get_or_init(std::thread::current);
        let result = self.together(job, out);
        self.settle(result, job, out)
    }

    fn gpu(&self) -> bool {
        false
    }

    fn threads(&self) -> u32 {
        if self.fallen {
            0
        } else {
            u32::try_from(self.helpers.len()).unwrap_or(u32::MAX)
        }
    }
}

impl Drop for CrewCaf {
    fn drop(&mut self) {
        for helper in &mut self.helpers {
            helper.release();
        }
        for helper in &mut self.helpers {
            if let Some(thread) = helper.thread.take()
                && thread.join().is_err()
            {
                tracing::warn!("a radar crew helper had panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests;
