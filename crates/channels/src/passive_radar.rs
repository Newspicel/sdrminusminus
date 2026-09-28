mod assemble;
mod caf;
mod cpi;
mod dab_remod;
mod front;
mod plan;
mod reference;
mod report;
mod surface;
#[cfg(test)]
mod tests;

use sdrmm_wire::StreamKind;
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};

use crate::array_processor::{
    Execution, ProcessorDescriptor, ProcessorNeeds, Registration, TuningNeed, never_in_place,
    no_lane_format,
};

pub use assemble::{CpiAssembler, CpiJob};
pub use caf::{
    CafBackend, CafError, CpuCaf, CubeOut, GateChunk, SpectraShard, doppler_stage, gather_stage,
    residual_stage, solve_stage, spectra_stage,
};
pub use cpi::{CpiOutcome, CpiStage};
pub use dab_remod::DabRemod;
pub use front::{FRONT_CHUNK, FrontStage, FrontStats, InLanes, JobSink};
pub use plan::{
    AOA_GRID_STEP_DEG, Alphas, AoaPlan, CancellerPlan, DAB_FRAME_LATENCY, FrontPlan, LiveParams,
    MAX_LOOKS, PlanChange, PlanError, RadarCtx, RadarPlan, change, line_axis, params_of, plan,
    plan_for, positions_of,
};
pub use reference::ReferenceCleaner;
pub use report::{
    AoaPod, DetectionPod, RadarCounters, RadarPod, TrackEventPod, TrackPod, axes_of, fill_update,
    track_events,
};
pub use surface::{RangeDopplerSurface, fill_surface, level};

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "passive_radar",
    name: "Passive radar",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: &[],
    steer_port: None,
    surface: Some(StreamKind::RangeDoppler),
    needs: |_| ProcessorNeeds::TIME,
    band: |_| None,
    tuning: |_| TuningNeed::Together,
    lane_format: no_lane_format,
    execution: |_, _| Execution::Dedicated,
    in_place: never_in_place,
};

pub const REGISTRATION: Registration = Registration {
    descriptor: &DESCRIPTOR,
    create: None,
};
