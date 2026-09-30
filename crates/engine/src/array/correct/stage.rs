use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{JoinHandle, Thread},
    time::Duration,
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_channels::array_processor::MAX_LANES;

use super::{CORR_FFT, CorrectionSet, Corrector};
use crate::{
    EngineError,
    array::{align::ALIGN_BLOCK, capture::CalQuality},
};

pub(crate) const STAGE_JOBS: usize = 4;
const IDLE: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Label {
    pub(crate) generation: u32,
    pub(crate) gap_before: bool,
    pub(crate) phase_ready: bool,
    pub(crate) gain_ready: bool,
    pub(crate) quality: CalQuality,
}

pub(crate) struct StageJob {
    raw: Vec<Vec<Complex<f32>>>,
    count: usize,
    index: u64,
    reset: bool,
    load: bool,
    spectra: Box<CorrectionSet>,
    out: Vec<Vec<Complex<f32>>>,
    ready: usize,
    first_index: u64,
    transient: usize,
    label: Label,
}

impl StageJob {
    pub(super) fn new(lanes: usize) -> Self {
        Self {
            raw: (0..lanes)
                .map(|_| Vec::with_capacity(ALIGN_BLOCK))
                .collect(),
            count: 0,
            index: 0,
            reset: false,
            load: false,
            spectra: Box::new(CorrectionSet::identity(lanes)),
            out: (0..lanes)
                .map(|_| Vec::with_capacity(ALIGN_BLOCK + CORR_FFT))
                .collect(),
            ready: 0,
            first_index: 0,
            transient: 0,
            label: Label::default(),
        }
    }

    pub(crate) const fn label(&self) -> Label {
        self.label
    }

    pub(crate) const fn first_index(&self) -> u64 {
        self.first_index
    }

    pub(crate) const fn transient(&self) -> usize {
        self.transient
    }

    pub(crate) const fn ready(&self) -> usize {
        self.ready
    }

    pub(crate) fn with_lanes<R>(&self, f: impl FnOnce(&[&[Complex<f32>]]) -> R) -> R {
        let mut view: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        for (slot, lane) in view.iter_mut().zip(&self.out) {
            *slot = &lane[..self.ready.min(lane.len())];
        }
        f(&view[..self.out.len()])
    }

    pub(super) fn fill(&mut self, raw: &[&[Complex<f32>]], index: u64, label: Label) {
        self.count = raw.first().map_or(0, |lane| lane.len());
        for (target, lane) in self.raw.iter_mut().zip(raw) {
            target.clear();
            target.extend_from_slice(lane);
        }
        self.index = index;
        self.label = label;
    }

    pub(super) fn correct(&mut self, corrector: &mut Corrector) {
        if self.reset {
            corrector.reset();
        }
        if self.load && !corrector.load(&self.spectra) {
            tracing::error!("the array correction stage kept its previous response");
        }
        let mut view: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        for (slot, lane) in view.iter_mut().zip(&self.raw) {
            *slot = &lane[..self.count.min(lane.len())];
        }
        corrector.push(&view[..self.raw.len()], self.index);
        self.first_index = corrector.first_index();
        self.transient = corrector.transient();
        let out = &mut self.out;
        self.ready = corrector.with_corrected(|lanes, _| {
            for (target, lane) in out.iter_mut().zip(lanes) {
                target.clear();
                target.extend_from_slice(lane);
            }
            lanes.first().map_or(0, |lane| lane.len())
        });
        corrector.consume();
    }
}

pub(crate) struct CorrectionStage {
    to_worker: Producer<Box<StageJob>>,
    from_worker: Consumer<Box<StageJob>>,
    idle: [Option<Box<StageJob>>; STAGE_JOBS],
    reset: bool,
    load: bool,
    worker: Arc<OnceLock<Thread>>,
}

impl CorrectionStage {
    pub(crate) fn reset(&mut self) {
        self.reset = true;
    }

    pub(crate) fn load(&mut self) {
        self.load = true;
    }

    pub(crate) fn submit(
        &mut self,
        raw: &[&[Complex<f32>]],
        index: u64,
        label: Label,
        active: &CorrectionSet,
    ) -> bool {
        let Some(mut job) = self.idle.iter_mut().find_map(Option::take) else {
            return false;
        };
        job.fill(raw, index, label);
        job.reset = std::mem::take(&mut self.reset);
        job.load = std::mem::take(&mut self.load);
        if job.load {
            job.spectra.copy_from(active);
        }
        match self.to_worker.push(job) {
            Ok(()) => {
                if let Some(worker) = self.worker.get() {
                    worker.unpark();
                }
                true
            }
            Err(rtrb::PushError::Full(job)) => {
                self.reset |= job.reset;
                self.load |= job.load;
                self.keep(job);
                false
            }
        }
    }

    pub(crate) fn finished(&mut self) -> Option<Box<StageJob>> {
        self.from_worker.pop().ok()
    }

    pub(crate) fn keep(&mut self, job: Box<StageJob>) {
        if let Some(slot) = self.idle.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(job);
        }
    }
}

pub(crate) struct StageWorker {
    jobs: Consumer<Box<StageJob>>,
    done: Producer<Box<StageJob>>,
    corrector: Corrector,
    stop: Arc<AtomicBool>,
    aggregator: Arc<OnceLock<Thread>>,
    worker: Arc<OnceLock<Thread>>,
}

impl StageWorker {
    fn run(mut self) {
        let _ = self.worker.set(std::thread::current());
        while !self.stop.load(Ordering::Acquire) {
            let Ok(mut job) = self.jobs.pop() else {
                std::thread::park_timeout(IDLE);
                continue;
            };
            job.correct(&mut self.corrector);
            if self.done.push(job).is_err() {
                tracing::error!("the array correction stage lost a block");
            }
            if let Some(aggregator) = self.aggregator.get() {
                aggregator.unpark();
            }
        }
    }
}

pub(crate) fn correction_stage(
    lanes: usize,
    stop: Arc<AtomicBool>,
    aggregator: Arc<OnceLock<Thread>>,
) -> (CorrectionStage, StageWorker) {
    let (to_worker, jobs) = RingBuffer::new(STAGE_JOBS);
    let (done, from_worker) = RingBuffer::new(STAGE_JOBS);
    let worker = Arc::new(OnceLock::new());
    let stage = CorrectionStage {
        to_worker,
        from_worker,
        idle: std::array::from_fn(|_| Some(Box::new(StageJob::new(lanes)))),
        reset: true,
        load: true,
        worker: worker.clone(),
    };
    let runner = StageWorker {
        jobs,
        done,
        corrector: Corrector::new(lanes),
        stop,
        aggregator,
        worker,
    };
    (stage, runner)
}

pub(crate) fn spawn_stage(
    name: String,
    worker: StageWorker,
) -> Result<JoinHandle<()>, EngineError> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || worker.run())
        .map_err(|error| EngineError::Processor(format!("start array correction: {error}")))
}
