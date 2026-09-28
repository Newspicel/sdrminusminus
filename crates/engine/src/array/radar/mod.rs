mod crew;
mod gpu;
mod publish;

use std::{sync::Arc, thread::JoinHandle};

use sdrmm_channels::{
    ChannelError,
    array_processor::{ArrayBlock, ArrayCtx, ProcessorAction, ProcessorFaults, ProcessorOutput},
};
use sdrmm_wire::{GpuUse, PassiveRadarParams, ProcessorParams};

const NOT_BUILT: &str = "passive radar needs its runner";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RadarPlan {
    pub(crate) params: PassiveRadarParams,
}

pub(crate) enum Prepared {
    Same,
    Live(Box<PassiveRadarParams>),
    Rebuild(Box<dyn DedicatedRunner>),
}

pub(crate) trait DedicatedRunner: Send {
    fn push(&mut self, block: &ArrayBlock<'_>);
    fn poll(&mut self, out: &mut ProcessorOutput<'_>);
    fn commit(&mut self, prepared: Prepared) -> Option<Box<dyn DedicatedRunner>>;
    fn action(&mut self, action: ProcessorAction) -> Result<(), ChannelError>;
    fn faults(&self) -> ProcessorFaults;
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
    _ctx: &ArrayCtx<'_>,
    _params: &ProcessorParams,
    _gpu: GpuUse,
) -> Result<(Box<dyn DedicatedRunner>, Arc<RadarPlan>), ChannelError> {
    Err(ChannelError::Unsupported(NOT_BUILT.to_owned()))
}

pub(crate) fn prepare_dedicated(
    _plan: &RadarPlan,
    _ctx: &ArrayCtx<'_>,
    _params: &ProcessorParams,
    _gpu: GpuUse,
) -> Result<(Prepared, Arc<RadarPlan>), ChannelError> {
    Err(ChannelError::Unsupported(NOT_BUILT.to_owned()))
}
