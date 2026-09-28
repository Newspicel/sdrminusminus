use num_complex::Complex;
use sdrmm_dsp::beamform::{LaneNoise, WeightRamp, WeightSet};
use sdrmm_dsp::covariance::SampleCovariance;
use sdrmm_dsp::linalg::{CMat, Eigen, HermitianEigen};
use sdrmm_dsp::polar::{Hand as PolarHand, Stokes, matched_weights};
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::{
    BEAM_PORT, Hand, PolarimeterParams, PolarimeterReading, ProcessorParams, ProcessorReading,
};

use crate::ChannelError;
use crate::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, Execution, LaneFormat, MAX_LANES, ProcessorAction,
    ProcessorDescriptor, ProcessorFaults, ProcessorNeeds, ProcessorOutput, Registration,
    ResetCause, TuningNeed, boxed, check_tuning, no_lane_format, stamp_at,
};
use crate::band::{LaneBand, band_lane_format, check_offset};

const BEAM: usize = 0;
const PAIR: usize = 2;
const POWER_FLOOR_DB: f32 = -150.0;
const SNR_FLOOR: f32 = 1e-6;

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
    create: Some(boxed::<PolarimeterProcessor>),
};

const fn settings(params: &ProcessorParams) -> Option<&PolarimeterParams> {
    match params {
        ProcessorParams::Polarimeter(polar) => Some(polar),
        _ => None,
    }
}

fn lane_format(params: &ProcessorParams, ctx: &ArrayCtx<'_>, port: usize) -> LaneFormat {
    match settings(params) {
        Some(polar) if port == BEAM => {
            band_lane_format(ctx, polar.offset_hz, Some(polar.bandwidth_hz))
        }
        _ => no_lane_format(params, ctx, port),
    }
}

fn same_band(old: &PolarimeterParams, new: &PolarimeterParams) -> bool {
    old.h_lane == new.h_lane
        && old.v_lane == new.v_lane
        && old.offset_hz == new.offset_hz
        && old.bandwidth_hz == new.bandwidth_hz
}

fn in_place(old: &ProcessorParams, new: &ProcessorParams) -> bool {
    let (Some(old), Some(new)) = (settings(old), settings(new)) else {
        return false;
    };
    same_band(old, new)
}

fn checked(params: &ProcessorParams) -> Result<PolarimeterParams, ChannelError> {
    let polar = *settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
    match polar.problem() {
        Some(problem) => Err(ChannelError::Refused(problem)),
        None => Ok(polar),
    }
}

const fn wire_hand(hand: PolarHand) -> Hand {
    match hand {
        PolarHand::Right => Hand::Right,
        PolarHand::Left => Hand::Left,
        PolarHand::Linear => Hand::Linear,
    }
}

fn db(power: f32) -> f32 {
    if power > 0.0 {
        (10.0 * power.log10()).max(POWER_FLOOR_DB)
    } else {
        POWER_FLOOR_DB
    }
}

fn unsolved<E>(_: E) -> ChannelError {
    ChannelError::Refused("Too few elements")
}

fn samples(rate: f64, ms: u32) -> f64 {
    rate * f64::from(ms) / 1000.0
}

pub struct PolarimeterProcessor {
    band: LaneBand,
    core: Polarimeter,
}

struct Polarimeter {
    params: PolarimeterParams,
    lanes: usize,
    rate: f64,
    format: LaneFormat,
    covariance: SampleCovariance,
    matrix: CMat,
    eigen: HermitianEigen,
    values: Eigen,
    weights: WeightSet,
    ramp: WeightRamp,
    noise: LaneNoise,
    stokes: Stokes,
    snr_db: Option<f32>,
    beam: Vec<Complex<f32>>,
    since_report: u64,
    faults: ProcessorFaults,
}

impl Polarimeter {
    fn block(
        &mut self,
        pair: &[&[Complex<f32>]; PAIR],
        block: &ArrayBlock<'_>,
        out: &mut ProcessorOutput<'_>,
    ) {
        let n = pair[0].len();
        let average = samples(self.format.sample_rate, self.params.average_ms).max(1.0);
        self.covariance.decay((-(n as f64) / average).exp() as f32);
        self.covariance.accumulate(pair);
        self.beam.clear();
        if self.ramp.apply(pair, &mut self.beam).is_err() {
            self.faults.dropped_blocks += 1;
            out.skip_lane(BEAM, n as u64);
        } else if let Some(mut lane) = out.lane(BEAM) {
            lane.extend(&self.beam);
        }
        self.since_report += block.len() as u64;
        let report = (samples(self.rate, self.params.report_ms) as u64).max(1);
        if self.since_report >= report {
            self.since_report %= report;
            self.update(pair);
            self.report(block.unix_ns, out);
        }
    }

    fn update(&mut self, pair: &[&[Complex<f32>]; PAIR]) {
        if self.noise.push(pair).is_err() {
            self.faults.solver_failures += 1;
        }
        if !self.covariance.matrix(&mut self.matrix) {
            return;
        }
        self.stokes = Stokes::from_covariance(
            self.matrix.get(0, 0).re,
            self.matrix.get(1, 1).re,
            self.matrix.get(0, 1),
        );
        self.snr_db = self.noise.noise().map(|noise| {
            let floor: f32 = noise.iter().sum();
            db(((self.stokes.i - floor) / floor).max(SNR_FLOOR))
        });
        let orthogonal = !self.params.matched;
        match matched_weights(
            &self.matrix,
            orthogonal,
            &mut self.eigen,
            &mut self.values,
            &mut self.weights,
        ) {
            Ok(()) => {
                let ramp = samples(self.format.sample_rate, self.params.crossfade_ms) as u32;
                self.ramp.set_target(&self.weights, ramp);
            }
            Err(_) => self.faults.solver_failures += 1,
        }
    }

    fn report(&mut self, unix_ns: u64, out: &mut ProcessorOutput<'_>) {
        let Some(slot) = out.report() else {
            return;
        };
        if !matches!(slot, ProcessorReading::Polarimeter(_)) {
            *slot = ProcessorReading::Polarimeter(PolarimeterReading::reserved());
        }
        if let ProcessorReading::Polarimeter(reading) = slot {
            self.fill(reading, unix_ns);
        }
        out.publish_report();
    }

    fn fill(&self, reading: &mut PolarimeterReading, unix_ns: u64) {
        let stokes = self.stokes;
        let scale = if stokes.i > 0.0 {
            stokes.i.recip()
        } else {
            0.0
        };
        stamp_at(&mut reading.at, unix_ns);
        reading.i_db = db(stokes.i);
        reading.q = stokes.q * scale;
        reading.u = stokes.u * scale;
        reading.v = stokes.v * scale;
        reading.degree = stokes.degree();
        reading.angle_deg = stokes.angle_deg();
        reading.ellipticity_deg = stokes.ellipticity_deg();
        reading.hand = wire_hand(stokes.hand(self.params.flip_hand));
        reading.snr_db = self.snr_db;
        reading.out_center_hz = self.format.center_hz;
        reading.out_rate = self.format.sample_rate;
    }

    fn restart(&mut self) {
        self.covariance.reset();
        self.noise.reset();
        self.since_report = 0;
    }
}

impl PolarimeterProcessor {
    fn check_layout(&self, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
        if ctx.lanes != self.core.lanes {
            return Err(ChannelError::Refused("Lane out of range"));
        }
        if ctx.sample_rate != self.core.rate {
            return Err(ChannelError::Refused("Rate out of range"));
        }
        check_tuning(TuningNeed::Together, ctx)
    }
}

impl ArrayProcessor for PolarimeterProcessor {
    fn descriptor() -> &'static ProcessorDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: &ArrayCtx<'_>, params: &ProcessorParams) -> Result<Self, ChannelError> {
        let polar = checked(params)?;
        if ctx.lanes > MAX_LANES {
            return Err(ChannelError::Refused("Too many elements"));
        }
        let (h, v) = (polar.h_lane as usize, polar.v_lane as usize);
        if h >= ctx.lanes || v >= ctx.lanes {
            return Err(ChannelError::Refused("Lane out of range"));
        }
        check_tuning(TuningNeed::Together, ctx)?;
        check_offset(ctx.sample_rate, polar.offset_hz)?;
        let band = LaneBand::picked(
            ctx.lanes,
            &[h, v],
            ctx.sample_rate,
            polar.offset_hz,
            Some(polar.bandwidth_hz),
            ctx.max_block,
        )?;
        let format = band_lane_format(ctx, polar.offset_hz, Some(polar.bandwidth_hz));
        let mut ramp = WeightRamp::new(PAIR);
        let weights = WeightSet::unit(PAIR, 0);
        ramp.set_target(&weights, 0);
        Ok(Self {
            band,
            core: Polarimeter {
                params: polar,
                lanes: ctx.lanes,
                rate: ctx.sample_rate,
                format,
                covariance: SampleCovariance::new(PAIR).map_err(unsolved)?,
                matrix: CMat::zeros(PAIR).map_err(unsolved)?,
                eigen: HermitianEigen::new(PAIR).map_err(unsolved)?,
                values: Eigen::new(),
                weights,
                ramp,
                noise: LaneNoise::new(PAIR).map_err(unsolved)?,
                stokes: Stokes::default(),
                snr_db: None,
                beam: Vec::with_capacity(format.capacity),
                since_report: 0,
                faults: ProcessorFaults::default(),
            },
        })
    }

    fn apply(&mut self, params: &ProcessorParams) -> Result<(), ChannelError> {
        let polar = checked(params)?;
        if !same_band(&self.core.params, &polar) {
            return Err(ChannelError::Refused("Needs a rebuild"));
        }
        self.core.params = polar;
        Ok(())
    }

    fn retune(&mut self, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
        self.check_layout(ctx)?;
        let polar = &self.core.params;
        self.core.format = band_lane_format(ctx, polar.offset_hz, Some(polar.bandwidth_hz));
        self.band.reset();
        self.core.restart();
        Ok(())
    }

    fn reset(&mut self, _cause: ResetCause) {
        self.band.reset();
        self.core.restart();
        self.core.faults.resets += 1;
    }

    fn action(&mut self, _action: ProcessorAction) -> Result<(), ChannelError> {
        Err(ChannelError::Refused("No tracks here"))
    }

    fn process(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        if !self.core.faults.lanes_match(block, self.core.lanes) {
            let ratio = self.core.format.sample_rate / self.core.rate;
            out.skip_lane(BEAM, (block.len() as f64 * ratio).round() as u64);
            return;
        }
        let mut views: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let n = self.band.process(block, &mut views);
        let pair = [&views[0][..n], &views[1][..n]];
        self.core.block(&pair, block, out);
    }

    fn faults(&self) -> ProcessorFaults {
        self.core.faults
    }
}

#[cfg(test)]
mod tests;
