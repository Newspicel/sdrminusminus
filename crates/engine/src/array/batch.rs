use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_channels::array_processor::{
    ArrayBlock, ArrayProcessor, CalView, CorrectionView, MAX_LANES, Pose, ProcessorAction,
    ProcessorFaults, ResetCause, Steer,
};
use sdrmm_wire::{Coherence, ProcessorParams};

use super::{
    CorrectionSet, LiveFrame,
    correct::CORR_FFT,
    host::{ArrayShape, Outputs, ProcessorStats, SteerMailbox},
};
use crate::{EngineError, runtime::VirtualLaneSink};

pub(crate) const BATCH_JOBS: usize = 3;
const CONTROL_SLOTS: usize = 16;
const IDLE: Duration = Duration::from_millis(2);

pub(crate) struct BatchJob {
    lanes: Vec<Vec<Complex<f32>>>,
    count: usize,
    first_index: u64,
    unix_ns: u64,
    generation: u32,
    gap_before: bool,
    corrected: bool,
    cal: CalView,
    pose: Pose,
    centers: [f64; MAX_LANES],
    lane_count: usize,
    correction: Vec<Vec<Complex<f32>>>,
    correction_generation: Option<u32>,
    sample_rate: f64,
    skip_before: u64,
}

impl BatchJob {
    fn new(lanes: usize, batch: usize) -> Self {
        Self {
            lanes: (0..lanes).map(|_| Vec::with_capacity(batch)).collect(),
            count: 0,
            first_index: 0,
            unix_ns: 0,
            generation: 0,
            gap_before: false,
            corrected: false,
            cal: CalView::default(),
            pose: Pose::default(),
            centers: [0.0; MAX_LANES],
            lane_count: lanes,
            correction: (0..lanes).map(|_| Vec::with_capacity(CORR_FFT)).collect(),
            correction_generation: None,
            sample_rate: 0.0,
            skip_before: 0,
        }
    }

    fn clear(&mut self) {
        for lane in &mut self.lanes {
            lane.clear();
        }
        self.count = 0;
        self.skip_before = 0;
        self.gap_before = false;
    }

    fn start(&mut self, block: &ArrayBlock<'_>, correction: &CorrectionSet, gap: bool) {
        self.first_index = block.first_index;
        self.unix_ns = block.unix_ns;
        self.generation = block.generation;
        self.gap_before = block.gap_before || gap;
        self.corrected = block.corrected;
        self.cal = block.cal;
        self.pose = block.pose;
        let centers = block.centers_hz.len().min(MAX_LANES);
        self.centers[..centers].copy_from_slice(&block.centers_hz[..centers]);
        if !block.corrected
            && self.correction_generation != Some(correction.generation)
            && correction.fits(self.lane_count)
        {
            for (mine, theirs) in self.correction.iter_mut().zip(&correction.spectra) {
                mine.clear();
                mine.extend_from_slice(theirs);
            }
            self.correction_generation = Some(correction.generation);
        }
    }

    fn follows(&self, block: &ArrayBlock<'_>) -> bool {
        self.first_index + self.count as u64 == block.first_index
            && self.generation == block.generation
            && !block.gap_before
            && self.corrected == block.corrected
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RetuneFrame {
    pub(crate) sample_rate: f64,
    pub(crate) center_hz: f64,
    pub(crate) centers: [f64; MAX_LANES],
    pub(crate) lanes: usize,
    pub(crate) tier: Coherence,
}

impl RetuneFrame {
    fn of(frame: &LiveFrame) -> Self {
        let lanes = frame.lanes();
        let mut centers = [0.0; MAX_LANES];
        centers[..lanes].copy_from_slice(&frame.lane_centers_hz[..lanes]);
        Self {
            sample_rate: frame.sample_rate,
            center_hz: frame.center_hz,
            centers,
            lanes,
            tier: frame.tier,
        }
    }
}

pub(crate) enum BatchControl {
    Apply(Box<ProcessorParams>),
    Retune(RetuneFrame),
    Reset(ResetCause),
    Steer(Steer),
    Action(ProcessorAction),
    Sink { port: usize, sink: VirtualLaneSink },
}

struct WorkerEnds {
    free: Producer<Box<BatchJob>>,
    ready: Consumer<Box<BatchJob>>,
    control: Consumer<BatchControl>,
}

pub(crate) struct BatchRunner {
    free: Consumer<Box<BatchJob>>,
    ready: Producer<Box<BatchJob>>,
    control: Producer<BatchControl>,
    filling: Option<Box<BatchJob>>,
    batch: usize,
    pending_skip: u64,
    gap: bool,
    stats: Arc<ProcessorStats>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

pub(crate) struct BatchStart {
    pub(crate) node: String,
    pub(crate) processor: Box<dyn ArrayProcessor>,
    pub(crate) outputs: Box<Outputs>,
    pub(crate) shape: ArrayShape,
    pub(crate) batch: usize,
    pub(crate) stats: Arc<ProcessorStats>,
    pub(crate) steer_out: Arc<SteerMailbox>,
}

impl BatchRunner {
    pub(crate) fn start(start: BatchStart, frame: &LiveFrame) -> Result<Self, EngineError> {
        let BatchStart {
            node,
            processor,
            outputs,
            shape,
            batch,
            stats,
            steer_out,
        } = start;
        let batch = batch.max(1);
        let lanes = frame.lanes();
        let (mut free_tx, free_rx) = RingBuffer::new(BATCH_JOBS);
        for _ in 0..BATCH_JOBS {
            let _ = free_tx.push(Box::new(BatchJob::new(lanes, batch)));
        }
        let (ready_tx, ready_rx) = RingBuffer::new(BATCH_JOBS);
        let (control_tx, control_rx) = RingBuffer::new(CONTROL_SLOTS);
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            node,
            processor,
            outputs,
            shape,
            frame: frame.clone(),
            ends: WorkerEnds {
                free: free_tx,
                ready: ready_rx,
                control: control_rx,
            },
            stats: stats.clone(),
            base: stats.faults(),
            stop: stop.clone(),
            steer_out,
        };
        let handle = std::thread::Builder::new()
            .name("sdrmm-array-batch".to_owned())
            .spawn(move || worker.run())
            .map_err(|error| EngineError::Processor(format!("start batch worker: {error}")))?;
        Ok(Self {
            free: free_rx,
            ready: ready_tx,
            control: control_tx,
            filling: None,
            batch,
            pending_skip: 0,
            gap: false,
            stats,
            stop,
            worker: Some(handle),
        })
    }

    fn alive(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    pub(crate) fn watch(&mut self) {
        if !self.alive() {
            self.stats.alive.store(false, Ordering::Relaxed);
        }
    }

    pub(crate) fn push(
        &mut self,
        block: &ArrayBlock<'_>,
        correction: &CorrectionSet,
        sample_rate: f64,
    ) {
        let len = block.len();
        if !self.alive() {
            self.watch();
            self.drop_samples(len);
            return;
        }
        let mut at = 0;
        while at < len {
            let Some(mut job) = self.job_for(block) else {
                self.drop_samples(len - at);
                return;
            };
            if job.count == 0 {
                job.start(block, correction, std::mem::take(&mut self.gap));
                job.first_index = block.first_index + at as u64;
                job.sample_rate = sample_rate;
                job.skip_before = std::mem::take(&mut self.pending_skip);
            }
            let take = (self.batch - job.count).min(len - at);
            for (lane, samples) in job.lanes.iter_mut().zip(block.lanes) {
                lane.extend_from_slice(&samples[at..at + take]);
            }
            job.count += take;
            at += take;
            if job.count >= self.batch {
                self.submit(job);
            } else {
                self.filling = Some(job);
            }
        }
    }

    fn job_for(&mut self, block: &ArrayBlock<'_>) -> Option<Box<BatchJob>> {
        if let Some(job) = self.filling.take() {
            if job.count == 0 || job.follows(block) {
                return Some(job);
            }
            self.submit(job);
            if let Some(job) = self.filling.take() {
                return Some(job);
            }
        }
        let mut job = self.free.pop().ok()?;
        job.clear();
        Some(job)
    }

    fn submit(&mut self, job: Box<BatchJob>) {
        match self.ready.push(job) {
            Ok(()) => {
                if let Some(worker) = &self.worker {
                    worker.thread().unpark();
                }
            }
            Err(PushError::Full(mut job)) => {
                self.pending_skip += job.skip_before;
                self.drop_samples(job.count);
                job.clear();
                self.filling = Some(job);
            }
        }
    }

    fn drop_samples(&mut self, samples: usize) {
        self.stats
            .dropped_samples
            .fetch_add(samples as u64, Ordering::Relaxed);
        self.pending_skip += samples as u64;
        self.gap = true;
    }

    pub(crate) fn skip(&mut self, samples: u64) {
        if let Some(job) = self.filling.take() {
            if job.count > 0 {
                self.submit(job);
            } else {
                self.filling = Some(job);
            }
        }
        self.pending_skip += samples;
    }

    fn send(&mut self, control: BatchControl) -> Option<BatchControl> {
        match self.control.push(control) {
            Ok(()) => {
                if let Some(worker) = &self.worker {
                    worker.thread().unpark();
                }
                None
            }
            Err(PushError::Full(control)) => {
                self.stats.refused.fetch_add(1, Ordering::Relaxed);
                Some(control)
            }
        }
    }

    pub(crate) fn apply(&mut self, params: Box<ProcessorParams>) -> Option<Box<ProcessorParams>> {
        match self.send(BatchControl::Apply(params)) {
            Some(BatchControl::Apply(params)) => Some(params),
            _ => None,
        }
    }

    pub(crate) fn retune(&mut self, frame: &LiveFrame) {
        let _ = self.send(BatchControl::Retune(RetuneFrame::of(frame)));
    }

    pub(crate) fn reset(&mut self, cause: ResetCause) {
        let _ = self.send(BatchControl::Reset(cause));
    }

    pub(crate) fn steer(&mut self, steer: Steer) {
        let _ = self.send(BatchControl::Steer(steer));
    }

    pub(crate) fn action(&mut self, action: ProcessorAction) {
        let _ = self.send(BatchControl::Action(action));
    }

    pub(crate) fn set_sink(
        &mut self,
        port: usize,
        sink: VirtualLaneSink,
    ) -> Option<VirtualLaneSink> {
        match self.send(BatchControl::Sink { port, sink }) {
            Some(BatchControl::Sink { sink, .. }) => Some(sink),
            _ => None,
        }
    }
}

impl Drop for BatchRunner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if worker.join().is_err() {
                tracing::error!("array batch worker panicked");
            }
        }
    }
}

struct Worker {
    node: String,
    processor: Box<dyn ArrayProcessor>,
    outputs: Box<Outputs>,
    shape: ArrayShape,
    frame: LiveFrame,
    ends: WorkerEnds,
    stats: Arc<ProcessorStats>,
    base: ProcessorFaults,
    stop: Arc<AtomicBool>,
    steer_out: Arc<SteerMailbox>,
}

impl Worker {
    fn run(mut self) {
        while !self.stop.load(Ordering::Acquire) {
            while let Ok(control) = self.ends.control.pop() {
                self.control(control);
            }
            if let Ok(job) = self.ends.ready.pop() {
                self.job(&job);
                let _ = self.ends.free.push(job);
                continue;
            }
            let freq_hz = self.frame.center_hz;
            let processor = &mut self.processor;
            if let Some(steer) = self.outputs.run(|out| processor.poll(out), freq_hz) {
                self.steer_out.post(&steer);
            }
            std::thread::park_timeout(IDLE);
        }
    }

    fn control(&mut self, control: BatchControl) {
        let result = match control {
            BatchControl::Apply(params) => self.processor.apply(&params),
            BatchControl::Retune(retune) => {
                self.frame.sample_rate = retune.sample_rate;
                self.frame.center_hz = retune.center_hz;
                self.frame.tier = retune.tier;
                self.frame.lane_centers_hz.clear();
                self.frame
                    .lane_centers_hz
                    .extend_from_slice(&retune.centers[..retune.lanes]);
                let ctx = self.shape.ctx(&self.node, &self.frame);
                let result = self.processor.retune(&ctx);
                if result.is_err() {
                    self.stats.rebuild.store(true, Ordering::Relaxed);
                } else {
                    self.processor.reset(ResetCause::Retuned);
                }
                Ok(())
            }
            BatchControl::Reset(cause) => {
                self.processor.reset(cause);
                Ok(())
            }
            BatchControl::Steer(steer) => {
                self.processor.steer(&steer);
                Ok(())
            }
            BatchControl::Action(action) => self.processor.action(action),
            BatchControl::Sink { port, sink } => {
                let _ = self.outputs.set_sink(port, sink);
                Ok(())
            }
        };
        if result.is_err() {
            self.stats.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn job(&mut self, job: &BatchJob) {
        if job.skip_before > 0 {
            self.outputs.skip(job.skip_before);
        }
        let mut view: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        for (slot, lane) in view.iter_mut().zip(&job.lanes) {
            *slot = &lane[..job.count.min(lane.len())];
        }
        let correction = if job.corrected || job.correction_generation.is_none() {
            CorrectionView::identity()
        } else {
            CorrectionView::new(
                job.correction_generation.unwrap_or(0),
                job.sample_rate,
                &job.correction,
            )
        };
        let block = ArrayBlock {
            lanes: &view[..job.lane_count],
            corrected: job.corrected,
            correction,
            first_index: job.first_index,
            unix_ns: job.unix_ns,
            generation: job.generation,
            gap_before: job.gap_before,
            centers_hz: &job.centers[..job.lane_count],
            cal: job.cal,
            pose: job.pose,
        };
        let freq_hz = job.centers[0];
        let processor = &mut self.processor;
        let steer = self
            .outputs
            .run(|out| processor.process(&block, out), freq_hz);
        if let Some(steer) = steer {
            self.steer_out.post(&steer);
        }
        self.stats
            .record_faults(&self.base, self.processor.faults());
    }
}
