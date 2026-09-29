mod crew;
mod gpu;
mod publish;
#[cfg(test)]
mod tests;
mod worker;

use std::{any::Any, num::NonZeroUsize, sync::Arc, thread::JoinHandle};

use sdrmm_channels::{
    ChannelError,
    array_processor::{ArrayBlock, ArrayCtx, ProcessorAction, ProcessorFaults, ProcessorOutput},
    passive_radar::{
        CafBackend, CpuCaf, LiveParams, PlanChange, RadarCtx, RadarPlan as StagePlan, change,
        plan_for,
    },
};
use sdrmm_wire::{GpuUse, PassiveRadarParams, ProcessorParams};

pub(crate) use worker::RadarWorker;
use worker::Shared;

const MAX_CREW: usize = 3;
const RESERVED_CORES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CafBackendKind {
    Cpu,
    Crew { threads: u32 },
    Gpu,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RadarPlan {
    pub(crate) stage: StagePlan,
    pub(crate) backend: CafBackendKind,
}

impl RadarPlan {
    pub(crate) const fn ctx(&self) -> &RadarCtx {
        &self.stage.ctx
    }

    pub(crate) const fn params(&self) -> &PassiveRadarParams {
        &self.stage.params
    }
}

pub(crate) enum Prepared {
    Same,
    Live(Box<LiveParams>),
    Rebuild(Box<dyn DedicatedRunner>),
}

pub(crate) trait DedicatedRunner: Send {
    fn push(&mut self, block: &ArrayBlock<'_>);
    fn poll(&mut self, out: &mut ProcessorOutput<'_>);
    fn commit(&mut self, prepared: Prepared) -> Option<Box<dyn DedicatedRunner>>;
    fn action(&mut self, action: ProcessorAction) -> Result<(), ChannelError>;
    fn faults(&self) -> ProcessorFaults;
    fn dropped_samples(&self) -> u64;
    fn running(&self) -> bool;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn retire(self: Box<Self>) -> Retired;
}

pub(crate) struct Retired {
    threads: Vec<JoinHandle<()>>,
}

impl Retired {
    pub(crate) const fn new(threads: Vec<JoinHandle<()>>) -> Self {
        Self { threads }
    }

    pub(crate) fn join(self) -> Result<(), String> {
        let panicked = self
            .threads
            .into_iter()
            .map(JoinHandle::join)
            .filter(Result::is_err)
            .count();
        if panicked == 0 {
            Ok(())
        } else {
            Err(format!("{panicked} radar threads panicked"))
        }
    }
}

pub(crate) fn build_dedicated(
    ctx: &ArrayCtx<'_>,
    params: &ProcessorParams,
    gpu: GpuUse,
) -> Result<(Box<dyn DedicatedRunner>, Arc<RadarPlan>), ChannelError> {
    let stage = plan_for(ctx, params)?;
    let (worker, backend) = launch(&stage, ctx.max_block, gpu)?;
    Ok((Box::new(worker), Arc::new(RadarPlan { stage, backend })))
}

pub(crate) fn prepare_dedicated(
    plan: &RadarPlan,
    ctx: &ArrayCtx<'_>,
    params: &ProcessorParams,
    gpu: GpuUse,
) -> Result<(Prepared, Arc<RadarPlan>), ChannelError> {
    let stage = plan_for(ctx, params)?;
    let gpu_withdrawn = plan.backend == CafBackendKind::Gpu && gpu == GpuUse::Off;
    let kept = |stage: StagePlan| {
        Arc::new(RadarPlan {
            stage,
            backend: plan.backend,
        })
    };
    match change(&plan.stage, &stage) {
        PlanChange::Same if !gpu_withdrawn => Ok((Prepared::Same, kept(stage))),
        PlanChange::Live if !gpu_withdrawn => {
            let live = stage.live();
            if !live.cfar.valid() {
                return Err(ChannelError::Refused("Guard out of range"));
            }
            if !live.tracker.valid() {
                return Err(ChannelError::Refused("Gate out of range"));
            }
            Ok((Prepared::Live(Box::new(live)), kept(stage)))
        }
        _ => {
            let (worker, backend) = launch(&stage, ctx.max_block, gpu)?;
            Ok((
                Prepared::Rebuild(Box::new(worker)),
                Arc::new(RadarPlan { stage, backend }),
            ))
        }
    }
}

fn launch(
    stage: &StagePlan,
    block: usize,
    gpu: GpuUse,
) -> Result<(RadarWorker, CafBackendKind), ChannelError> {
    let shared = Arc::new(Shared::default());
    let (backend, kind) = select_backend(stage, gpu, &shared)?;
    let worker = RadarWorker::start(stage, backend, block, shared)?;
    Ok((worker, kind))
}

fn select_backend(
    stage: &StagePlan,
    gpu: GpuUse,
    shared: &Arc<Shared>,
) -> Result<(Box<dyn CafBackend>, CafBackendKind), ChannelError> {
    if gpu == GpuUse::Auto && stage.params.gpu == GpuUse::Auto {
        match gpu::build(stage, shared) {
            Ok(backend) => return Ok((backend, CafBackendKind::Gpu)),
            Err(error) => tracing::debug!(%error, "radar CAF stays on the CPU"),
        }
    }
    let helpers = crew_helpers();
    if helpers > 0 {
        match crew::CrewCaf::new(stage, helpers, shared) {
            Ok(crew) => {
                let threads = crew.threads();
                return Ok((Box::new(crew), CafBackendKind::Crew { threads }));
            }
            Err(error) => {
                tracing::warn!(%error, "radar crew did not start, CAF runs on one thread")
            }
        }
    }
    Ok((Box::new(CpuCaf::new(stage)?), CafBackendKind::Cpu))
}

fn crew_helpers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .saturating_sub(RESERVED_CORES)
        .min(MAX_CREW)
}
