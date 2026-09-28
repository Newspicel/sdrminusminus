use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::{ProcessorParams, SpatialSpectrumParams, StreamKind};

use crate::array_processor::{
    Execution, ProcessorDescriptor, ProcessorNeeds, Registration, TuningNeed, no_lane_format,
};

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "spatial_spectrum",
    name: "Spatial spectrum",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: &[],
    steer_port: None,
    surface: Some(StreamKind::SpatialSpectrum),
    needs: |_| ProcessorNeeds::ALL,
    band: |_| None,
    tuning: |_| TuningNeed::Together,
    lane_format: no_lane_format,
    execution: |_, ctx| Execution::Worker {
        batch: ctx.max_block,
    },
    in_place,
};

pub const REGISTRATION: Registration = Registration {
    descriptor: &DESCRIPTOR,
    create: None,
};

const fn settings(params: &ProcessorParams) -> Option<&SpatialSpectrumParams> {
    match params {
        ProcessorParams::SpatialSpectrum(spatial) => Some(spatial),
        _ => None,
    }
}

fn in_place(old: &ProcessorParams, new: &ProcessorParams) -> bool {
    let (Some(old), Some(new)) = (settings(old), settings(new)) else {
        return false;
    };
    old.bins == new.bins
        && old.columns == new.columns
        && old.azimuth_step_deg == new.azimuth_step_deg
        && old.offset_hz == new.offset_hz
        && old.bandwidth_hz == new.bandwidth_hz
}
