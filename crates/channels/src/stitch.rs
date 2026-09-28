use num_complex::Complex;
use sdrmm_dsp::stitch::{STITCH_FFT, StitchError, StitchOptions, Stitcher, output_rate};
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::{
    ProcessorParams, ProcessorReading, STITCH_WIDE_PORT, StitchBlend, StitchLane, StitchParams,
    StitchReading,
};

use crate::ChannelError;
use crate::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, Execution, LaneFormat, MAX_LANES, ProcessorAction,
    ProcessorDescriptor, ProcessorFaults, ProcessorNeeds, ProcessorOutput, Registration,
    ResetCause, TuningNeed, boxed, check_tuning, stamp_at,
};

const WIDE: usize = 0;

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "stitch",
    name: "Stitch",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: &[STITCH_WIDE_PORT],
    steer_port: None,
    surface: None,
    needs: |_| ProcessorNeeds::TIME,
    band: |_| None,
    tuning: |_| TuningNeed::Spread,
    lane_format,
    execution: |_, ctx| Execution::Worker {
        batch: ctx.max_block,
    },
    in_place: |old, new| settings(old).is_some() && settings(new).is_some(),
};

pub const REGISTRATION: Registration = Registration {
    descriptor: &DESCRIPTOR,
    create: Some(boxed::<StitchProcessor>),
};

const fn settings(params: &ProcessorParams) -> Option<&StitchParams> {
    match params {
        ProcessorParams::Stitch(stitch) => Some(stitch),
        _ => None,
    }
}

fn options(params: &StitchParams) -> StitchOptions {
    StitchOptions {
        equalise: params.noise_equalise,
        flatten: params.flatten,
        snr_blend: params.blend == StitchBlend::Snr,
        spur_reject: params.spur_reject,
        match_phase: params.match_phase,
    }
}

const fn wide_capacity(max_block: usize, lanes: usize) -> usize {
    (max_block + STITCH_FFT) * lanes
}

fn lane_offsets(ctx: &ArrayCtx<'_>) -> [f64; MAX_LANES] {
    let mut offsets = [0.0; MAX_LANES];
    for (offset, center) in offsets.iter_mut().zip(ctx.lane_centers_hz) {
        *offset = center - ctx.center_hz;
    }
    offsets
}

fn wide_offset_hz(ctx: &ArrayCtx<'_>) -> f64 {
    let lanes = ctx.lanes.min(MAX_LANES);
    let Ok(mut stitcher) = Stitcher::new(lanes, ctx.sample_rate) else {
        return 0.0;
    };
    match stitcher.retune(&lane_offsets(ctx)[..lanes]) {
        Ok(()) | Err(StitchError::NoOverlap(..)) => stitcher.output_center_offset_hz(),
        Err(_) => 0.0,
    }
}

fn lane_format(_: &ProcessorParams, ctx: &ArrayCtx<'_>, port: usize) -> LaneFormat {
    let wide = port == WIDE;
    LaneFormat {
        center_hz: ctx.center_hz + if wide { wide_offset_hz(ctx) } else { 0.0 },
        sample_rate: output_rate(ctx.lanes, ctx.sample_rate),
        capacity: if wide {
            wide_capacity(ctx.max_block, ctx.lanes)
        } else {
            0
        },
    }
}

const fn refused(error: &StitchError) -> ChannelError {
    match error {
        StitchError::Lanes(_) | StitchError::LaneCount { .. } => {
            ChannelError::Refused("Too few elements")
        }
        StitchError::Rate(_) => ChannelError::Refused("Rate out of range"),
        StitchError::LaneLength { .. } | StitchError::Offset(_) => {
            ChannelError::Refused("Lane out of range")
        }
        StitchError::NoOverlap(..) => ChannelError::Refused("Lanes do not overlap"),
    }
}

pub struct StitchProcessor {
    lanes: usize,
    rate: f64,
    center_hz: f64,
    centers: [f64; MAX_LANES],
    stitcher: Stitcher,
    wide: Vec<Complex<f32>>,
    no_overlap: bool,
    since_report: u64,
    report_samples: u64,
    faults: ProcessorFaults,
}

impl StitchProcessor {
    fn check_layout(&self, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
        if ctx.lanes != self.lanes || ctx.lane_centers_hz.len() != self.lanes {
            return Err(ChannelError::Refused("Lane out of range"));
        }
        if ctx.sample_rate != self.rate {
            return Err(ChannelError::Refused("Rate out of range"));
        }
        check_tuning(TuningNeed::Spread, ctx)
    }

    fn stitch(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) -> bool {
        self.wide.clear();
        if self.stitcher.process(block.lanes, &mut self.wide).is_err() {
            self.faults.lane_mismatch += 1;
            return false;
        }
        if let Some(mut lane) = out.lane(WIDE) {
            lane.extend(&self.wide);
        }
        true
    }

    fn report(&mut self, unix_ns: u64, out: &mut ProcessorOutput<'_>) {
        let Some(slot) = out.report() else {
            return;
        };
        if !matches!(slot, ProcessorReading::Stitch(_)) {
            *slot = ProcessorReading::Stitch(StitchReading::reserved());
        }
        if let ProcessorReading::Stitch(reading) = slot {
            self.fill(reading, unix_ns);
        }
        out.publish_report();
    }

    fn fill(&mut self, reading: &mut StitchReading, unix_ns: u64) {
        stamp_at(&mut reading.at, unix_ns);
        reading.center_hz = self.center_hz + self.stitcher.output_center_offset_hz();
        reading.span_hz = self.stitcher.output_rate();
        reading.dropped_blocks = self.faults.lane_mismatch + self.faults.dropped_blocks;
        reading.no_overlap = self.no_overlap;
        reading.lanes.clear();
        for lane in 0..self.lanes {
            let state = self.stitcher.lane_state(lane);
            let entry = StitchLane {
                lane: u32::try_from(lane).unwrap_or(u32::MAX),
                center_hz: self.centers[lane],
                noise_eq_db: state.gain_db,
                coherence: state.coherence,
                phase_deg: state.phase_deg,
                spur_bins: state.spur_bins,
            };
            self.faults.push_capped(&mut reading.lanes, entry);
        }
    }
}

impl ArrayProcessor for StitchProcessor {
    fn descriptor() -> &'static ProcessorDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: &ArrayCtx<'_>, params: &ProcessorParams) -> Result<Self, ChannelError> {
        let stitch = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        if ctx.lanes > MAX_LANES {
            return Err(ChannelError::Refused("Too many elements"));
        }
        let mut stitcher =
            Stitcher::new(ctx.lanes, ctx.sample_rate).map_err(|error| refused(&error))?;
        stitcher.set_options(options(stitch));
        let mut processor = Self {
            lanes: ctx.lanes,
            rate: ctx.sample_rate,
            center_hz: ctx.center_hz,
            centers: [0.0; MAX_LANES],
            stitcher,
            wide: Vec::with_capacity(wide_capacity(ctx.max_block, ctx.lanes)),
            no_overlap: false,
            since_report: 0,
            report_samples: (ctx.sample_rate.round() as u64).max(1),
            faults: ProcessorFaults::default(),
        };
        processor.retune(ctx)?;
        Ok(processor)
    }

    fn apply(&mut self, params: &ProcessorParams) -> Result<(), ChannelError> {
        let stitch = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        self.stitcher.set_options(options(stitch));
        Ok(())
    }

    fn retune(&mut self, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
        self.check_layout(ctx)?;
        self.no_overlap = match self.stitcher.retune(&lane_offsets(ctx)[..self.lanes]) {
            Ok(()) => false,
            Err(StitchError::NoOverlap(..)) => true,
            Err(error) => return Err(refused(&error)),
        };
        self.center_hz = ctx.center_hz;
        self.centers[..self.lanes].copy_from_slice(ctx.lane_centers_hz);
        Ok(())
    }

    fn reset(&mut self, _cause: ResetCause) {
        self.stitcher.reset();
        self.faults.resets += 1;
    }

    fn action(&mut self, _action: ProcessorAction) -> Result<(), ChannelError> {
        Err(ChannelError::Refused("No tracks here"))
    }

    fn process(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        let skipped = (block.len() * self.lanes) as u64;
        if !self.faults.lanes_match(block, self.lanes) || !self.stitch(block, out) {
            out.skip_lane(WIDE, skipped);
            return;
        }
        self.since_report += block.len() as u64;
        if self.since_report >= self.report_samples {
            self.since_report %= self.report_samples;
            self.report(block.unix_ns, out);
        }
    }

    fn faults(&self) -> ProcessorFaults {
        self.faults
    }
}

#[cfg(test)]
mod tests;
