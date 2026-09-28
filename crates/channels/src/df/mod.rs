mod heading;
mod report;

use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::{DfParams, ProcessorParams, UlaSide};

use crate::array_processor::{
    Execution, ProcessorDescriptor, ProcessorNeeds, Registration, TuningNeed, no_lane_format,
};

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "df",
    name: "Direction finder",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: &[],
    steer_port: None,
    surface: None,
    needs: |_| ProcessorNeeds::ALL,
    band: |params| settings(params).map(|df| (df.offset_hz, df.bandwidth_hz)),
    tuning: |_| TuningNeed::Together,
    lane_format: no_lane_format,
    execution: |_, _| Execution::Inline,
    in_place,
};

pub const REGISTRATION: Registration = Registration {
    descriptor: &DESCRIPTOR,
    create: None,
};

const fn settings(params: &ProcessorParams) -> Option<&DfParams> {
    match params {
        ProcessorParams::Df(df) => Some(df),
        _ => None,
    }
}

const fn full_span(side: UlaSide) -> bool {
    matches!(side, UlaSide::Both)
}

fn in_place(old: &ProcessorParams, new: &ProcessorParams) -> bool {
    let (Some(old), Some(new)) = (settings(old), settings(new)) else {
        return false;
    };
    old.offset_hz == new.offset_hz
        && old.bandwidth_hz == new.bandwidth_hz
        && old.azimuth_step_deg == new.azimuth_step_deg
        && old.elevation == new.elevation
        && old.smoothing == new.smoothing
        && old.forward_backward == new.forward_backward
        && full_span(old.ula_side) == full_span(new.ula_side)
        && old.station_id == new.station_id
}
