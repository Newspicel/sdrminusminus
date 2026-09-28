mod report;
mod solve;

use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::{BEAM_PORT, BeamMode, BeamformerParams, ProcessorParams, STEER_PORT};

use crate::array_processor::{
    ArrayCtx, Execution, LaneFormat, ProcessorDescriptor, ProcessorNeeds, Registration, TuningNeed,
    no_lane_format,
};
use crate::band::band_lane_format;

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "beamformer",
    name: "Beamformer",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: &[BEAM_PORT],
    steer_port: Some(STEER_PORT),
    surface: None,
    needs,
    band: |params| settings(params).and_then(band),
    tuning: |_| TuningNeed::Together,
    lane_format,
    execution,
    in_place,
};

pub const REGISTRATION: Registration = Registration {
    descriptor: &DESCRIPTOR,
    create: None,
};

const fn settings(params: &ProcessorParams) -> Option<&BeamformerParams> {
    match params {
        ProcessorParams::Beamformer(beamformer) => Some(beamformer),
        _ => None,
    }
}

const fn steered(mode: BeamMode) -> bool {
    matches!(
        mode,
        BeamMode::Das | BeamMode::Mvdr | BeamMode::Lcmv | BeamMode::Gsc
    )
}

fn band(beamformer: &BeamformerParams) -> Option<(f64, f64)> {
    beamformer
        .bandwidth_hz
        .map(|bandwidth| (beamformer.offset_hz, bandwidth))
}

fn needs(params: &ProcessorParams) -> ProcessorNeeds {
    ProcessorNeeds {
        geometry: settings(params).is_some_and(|beamformer| steered(beamformer.mode)),
        ..ProcessorNeeds::CALIBRATED
    }
}

fn lane_format(params: &ProcessorParams, ctx: &ArrayCtx<'_>, port: usize) -> LaneFormat {
    match settings(params) {
        Some(beamformer) if port == 0 => {
            band_lane_format(ctx, beamformer.offset_hz, beamformer.bandwidth_hz)
        }
        _ => no_lane_format(params, ctx, port),
    }
}

fn worker(beamformer: &BeamformerParams) -> bool {
    match beamformer.mode {
        BeamMode::Gsc => true,
        BeamMode::Canceller => beamformer.bandwidth_hz.is_none(),
        _ => false,
    }
}

fn execution(params: &ProcessorParams, ctx: &ArrayCtx<'_>) -> Execution {
    if settings(params).is_some_and(worker) {
        Execution::Worker {
            batch: ctx.max_block,
        }
    } else {
        Execution::Inline
    }
}

fn in_place(old: &ProcessorParams, new: &ProcessorParams) -> bool {
    let (Some(old), Some(new)) = (settings(old), settings(new)) else {
        return false;
    };
    band(old) == band(new)
        && steered(old.mode) == steered(new.mode)
        && worker(old) == worker(new)
        && old.taps == new.taps
        && old.adaptation == new.adaptation
        && old.main_lane == new.main_lane
        && old.reference_lanes == new.reference_lanes
}
