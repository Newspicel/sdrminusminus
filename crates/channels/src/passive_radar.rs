mod assemble;
mod caf;
mod cpi;
mod dab_remod;
mod front;
mod plan;
mod reference;
mod report;
mod surface;

use sdrmm_wire::StreamKind;
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};

use crate::array_processor::{
    Execution, ProcessorDescriptor, ProcessorNeeds, Registration, TuningNeed, never_in_place,
    no_lane_format,
};

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
