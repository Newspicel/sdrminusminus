use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::{BEAM_PORT, PolarimeterParams, ProcessorParams};

use crate::array_processor::{
    ArrayCtx, Execution, LaneFormat, ProcessorDescriptor, ProcessorNeeds, Registration, TuningNeed,
    no_lane_format,
};
use crate::band::band_lane_format;

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "polarimeter",
    name: "Polarimeter",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: &[BEAM_PORT],
    steer_port: None,
    surface: None,
    needs: |_| ProcessorNeeds::CALIBRATED,
    band: |params| settings(params).map(|polar| (polar.offset_hz, polar.bandwidth_hz)),
    tuning: |_| TuningNeed::Together,
    lane_format,
    execution: |_, _| Execution::Inline,
    in_place,
};

pub const REGISTRATION: Registration = Registration {
    descriptor: &DESCRIPTOR,
    create: None,
};

const fn settings(params: &ProcessorParams) -> Option<&PolarimeterParams> {
    match params {
        ProcessorParams::Polarimeter(polar) => Some(polar),
        _ => None,
    }
}

fn lane_format(params: &ProcessorParams, ctx: &ArrayCtx<'_>, port: usize) -> LaneFormat {
    match settings(params) {
        Some(polar) if port == 0 => {
            band_lane_format(ctx, polar.offset_hz, Some(polar.bandwidth_hz))
        }
        _ => no_lane_format(params, ctx, port),
    }
}

fn in_place(old: &ProcessorParams, new: &ProcessorParams) -> bool {
    let (Some(old), Some(new)) = (settings(old), settings(new)) else {
        return false;
    };
    old.h_lane == new.h_lane
        && old.v_lane == new.v_lane
        && old.offset_hz == new.offset_hz
        && old.bandwidth_hz == new.bandwidth_hz
}
