use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle, Thread},
    time::{Duration, Instant},
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_channels::{
    ChannelError, PassiveRadarProcessor,
    coherent::{CoherentCtx, CoherentDescriptor, CoherentOutputs, CoherentRx},
};
use sdrmm_wire::CoherentParams;

const JOBS: usize = 3;
const DROP_LOG_INTERVAL: Duration = Duration::from_secs(5);

struct Job {
    reference: Vec<Complex<f32>>,
    surveillance: Vec<Complex<f32>>,
    output: CoherentOutputs,
    generation: u64,
    segment: u64,
    center_hz: f64,
}

pub(super) struct RadarWorker {
    ctx: CoherentCtx,
    requests: Producer<Job>,
    completions: Consumer<Job>,
    available: Vec<Job>,
    pending: Option<Job>,
    filled: usize,
    cpi: usize,
    generation: u64,
    segment: u64,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    wake: Thread,
    dropped_samples: u64,
    last_drop: Option<Instant>,
    failed: bool,
}

fn processor(
    ctx: CoherentCtx,
    params: &CoherentParams,
) -> Result<(PassiveRadarProcessor, usize), ChannelError> {
    let mut cpi = 0;
    let processor = PassiveRadarProcessor::with_correlation(ctx, params, |cpu| {
        cpi = cpu.cpi();
        #[cfg(feature = "gpu-fft")]
        {
            Box::new(acceleration::Correlation::new(cpu))
        }
        #[cfg(not(feature = "gpu-fft"))]
        {
            Box::new(cpu)
        }
    })?;
    Ok((processor, cpi))
}

impl RadarWorker {
    fn start(
        ctx: CoherentCtx,
        processor: Box<dyn CoherentRx>,
        cpi: usize,
    ) -> Result<Self, ChannelError> {
        let (requests, request_rx) = RingBuffer::new(JOBS);
        let (completion_tx, completions) = RingBuffer::new(JOBS);
        let stop = Arc::new(AtomicBool::new(false));
        let halt = stop.clone();
        let worker = thread::Builder::new()
            .name("sdrmm-radar".to_owned())
            .spawn(move || work(processor, request_rx, completion_tx, &halt))
            .map_err(|error| {
                ChannelError::InvalidSettings(format!("start radar worker: {error}"))
            })?;
        let wake = worker.thread().clone();
        let available = (0..JOBS)
            .map(|_| Job {
                reference: vec![Complex::default(); cpi],
                surveillance: vec![Complex::default(); cpi],
                output: CoherentOutputs::default(),
                generation: 0,
                segment: 0,
                center_hz: ctx.center_hz,
            })
            .collect();
        Ok(Self {
            ctx,
            requests,
            completions,
            available,
            pending: None,
            filled: 0,
            cpi,
            generation: 0,
            segment: 0,
            stop,
            worker: Some(worker),
            wake,
            dropped_samples: 0,
            last_drop: None,
            failed: false,
        })
    }

    fn receive(&mut self, output: &mut CoherentOutputs) {
        while let Ok(mut job) = self.completions.pop() {
            let current = job.generation == self.generation;
            if current {
                std::mem::swap(output, &mut job.output);
            }
            self.available.push(job);
            if current {
                break;
            }
        }
    }

    fn drop_samples(&mut self, count: usize) {
        self.dropped_samples = self.dropped_samples.saturating_add(count as u64);
        self.segment = self.segment.wrapping_add(1);
        let now = Instant::now();
        if self
            .last_drop
            .is_none_or(|last| now.duration_since(last) >= DROP_LOG_INTERVAL)
        {
            tracing::warn!(
                dropped_samples = self.dropped_samples,
                "radar worker busy; dropping samples"
            );
            self.last_drop = Some(now);
        }
    }

    fn submit(&mut self, job: Job) {
        match self.requests.push(job) {
            Ok(()) => self.wake.unpark(),
            Err(PushError::Full(job)) => {
                self.available.push(job);
                self.drop_samples(self.cpi);
            }
        }
    }
}

impl CoherentRx for RadarWorker {
    fn descriptor() -> &'static CoherentDescriptor {
        PassiveRadarProcessor::descriptor()
    }

    fn new(ctx: CoherentCtx, params: &CoherentParams) -> Result<Self, ChannelError> {
        if ctx.lanes < 2 {
            return Err(ChannelError::InvalidSettings(
                "passive radar needs two lanes".to_owned(),
            ));
        }
        let (processor, cpi) = processor(ctx, params)?;
        Self::start(ctx, Box::new(processor), cpi)
    }

    fn apply(&mut self, params: &CoherentParams) -> Result<(), ChannelError> {
        *self = Self::new(self.ctx, params)?;
        Ok(())
    }

    fn retuned(&mut self, center_hz: f64) {
        self.ctx.center_hz = center_hz;
        self.generation = self.generation.wrapping_add(1);
        self.segment = self.segment.wrapping_add(1);
        self.filled = 0;
        if let Some(job) = self.pending.take() {
            self.available.push(job);
        }
    }

    fn poll(&mut self, output: &mut CoherentOutputs) {
        self.receive(output);
    }

    fn process(&mut self, lanes: &[&[Complex<f32>]], output: &mut CoherentOutputs) {
        self.receive(output);
        let (Some(reference), Some(surveillance)) = (lanes.first(), lanes.get(1)) else {
            return;
        };
        let count = reference.len().min(surveillance.len());
        if reference.len() != surveillance.len() {
            self.drop_samples(reference.len().max(surveillance.len()));
            self.retuned(self.ctx.center_hz);
            return;
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
            && !self.failed
        {
            self.failed = true;
            tracing::error!("radar worker stopped unexpectedly");
        }
        let mut at = 0;
        while at < count {
            let Some(mut job) = self.pending.take().or_else(|| self.available.pop()) else {
                self.drop_samples(count - at);
                return;
            };
            let take = (self.cpi - self.filled).min(count - at);
            job.reference[self.filled..self.filled + take]
                .copy_from_slice(&reference[at..at + take]);
            job.surveillance[self.filled..self.filled + take]
                .copy_from_slice(&surveillance[at..at + take]);
            self.filled += take;
            at += take;
            if self.filled == self.cpi {
                self.filled = 0;
                job.generation = self.generation;
                job.segment = self.segment;
                job.center_hz = self.ctx.center_hz;
                self.submit(job);
            } else {
                self.pending = Some(job);
            }
        }
    }
}

fn work(
    mut processor: Box<dyn CoherentRx>,
    mut requests: Consumer<Job>,
    mut completions: Producer<Job>,
    stop: &AtomicBool,
) {
    let mut segment = None;
    while !stop.load(Ordering::Acquire) {
        let Ok(mut job) = requests.pop() else {
            thread::park_timeout(Duration::from_millis(10));
            continue;
        };
        if segment != Some(job.segment) {
            processor.retuned(job.center_hz);
            segment = Some(job.segment);
        }
        job.output.reset();
        processor.process(&[&job.reference, &job.surveillance], &mut job.output);
        if let Err(PushError::Full(_)) = completions.push(job) {
            tracing::error!("radar completion queue overflow");
            return;
        }
    }
}

impl Drop for RadarWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake.unpark();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("radar worker panicked");
        }
    }
}

#[cfg(feature = "gpu-fft")]
mod acceleration;
#[cfg(test)]
mod tests;
