use std::sync::Arc;

use sdrmm_channels::{
    ChannelError,
    passive_radar::{CafBackend, RadarPlan as StagePlan},
};

use super::worker::Shared;

pub(super) const NOT_BUILT: &str = "GPU radar not built yet";

pub(super) fn build(
    _stage: &StagePlan,
    _shared: &Arc<Shared>,
) -> Result<Box<dyn CafBackend>, ChannelError> {
    Err(ChannelError::Unsupported(NOT_BUILT.to_owned()))
}
