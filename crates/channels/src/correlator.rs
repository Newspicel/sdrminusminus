use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::{CorrelatorParams, ProcessorParams, StreamKind};

use crate::array_processor::{
    Execution, ProcessorDescriptor, ProcessorNeeds, Registration, TuningNeed, no_lane_format,
};

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "correlator",
    name: "Correlator",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: &[],
    steer_port: None,
    surface: Some(StreamKind::Visibility),
    needs: |_| ProcessorNeeds::CALIBRATED,
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

const fn settings(params: &ProcessorParams) -> Option<&CorrelatorParams> {
    match params {
        ProcessorParams::Correlator(correlator) => Some(correlator),
        _ => None,
    }
}

fn in_place(old: &ProcessorParams, new: &ProcessorParams) -> bool {
    let (Some(old), Some(new)) = (settings(old), settings(new)) else {
        return false;
    };
    old.bins == new.bins && old.channels == new.channels && old.overlap == new.overlap
}
