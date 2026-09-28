mod report;
mod solve;

use std::sync::Arc;

use num_complex::Complex;
use sdrmm_dsp::beamform::{
    Adaptation as TdlAdaptation, BeamError, BeamMetrics, BlockSolver, Cma, Constraints, Gsc,
    LaneNoise, TDL_MAX_CMAC_PER_S, TdlCanceller, WeightRamp, WeightSet, tdl_cmac_per_sample,
};
use sdrmm_dsp::covariance::SampleCovariance;
use sdrmm_dsp::linalg::{CMat, Qr};
use sdrmm_dsp::manifold::{GridSpec, Manifold, SteeringGrid};
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::processor::beamformer::MAX_BEAM_NULLS;
use sdrmm_wire::{
    Adaptation, BEAM_PORT, BeamMode, BeamformerParams, NoiseModel, ProcessorParams, STEER_PORT,
    SteerSource,
};

use crate::ChannelError;
use crate::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, Execution, LaneFormat, MAX_LANES, ProcessorAction,
    ProcessorDescriptor, ProcessorFaults, ProcessorNeeds, ProcessorOutput, Registration,
    ResetCause, Steer, TuningNeed, boxed, check_tuning, geometry_of, no_lane_format,
};
use crate::band::{LaneBand, band_lane_format, check_offset};

use solve::Resolved;

const BEAM: usize = 0;
const PATTERN_POINTS: usize = 360;
const PATTERN_STEP_DEG: f64 = 1.0;

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
    create: Some(boxed::<BeamformerProcessor>),
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

fn band_of(offset_hz: f64, bandwidth_hz: Option<f64>) -> Option<(f64, f64)> {
    bandwidth_hz.map(|bandwidth| (offset_hz, bandwidth))
}

fn band(beamformer: &BeamformerParams) -> Option<(f64, f64)> {
    band_of(beamformer.offset_hz, beamformer.bandwidth_hz)
}

fn needs(params: &ProcessorParams) -> ProcessorNeeds {
    ProcessorNeeds {
        geometry: settings(params).is_some_and(|beamformer| steered(beamformer.mode)),
        ..ProcessorNeeds::CALIBRATED
    }
}

fn lane_format(params: &ProcessorParams, ctx: &ArrayCtx<'_>, port: usize) -> LaneFormat {
    match settings(params) {
        Some(beamformer) if port == BEAM => {
            band_lane_format(ctx, beamformer.offset_hz, beamformer.bandwidth_hz)
        }
        _ => no_lane_format(params, ctx, port),
    }
}

const fn runs_on_worker(mode: BeamMode, bandwidth_hz: Option<f64>) -> bool {
    match mode {
        BeamMode::Gsc => true,
        BeamMode::Canceller => bandwidth_hz.is_none(),
        _ => false,
    }
}

const fn worker(beamformer: &BeamformerParams) -> bool {
    runs_on_worker(beamformer.mode, beamformer.bandwidth_hz)
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    mode: BeamMode,
    steer: SteerSource,
    nulls: [f64; MAX_BEAM_NULLS],
    null_count: usize,
    auto_nulls: bool,
    main: usize,
    references: [usize; MAX_LANES],
    refs: usize,
    taps: usize,
    adaptation: Adaptation,
    step: f32,
    forget: f32,
    crossfade_ms: u32,
    update_ms: u32,
    carry_over: f32,
    loading: f32,
    noise: NoiseModel,
    offset_hz: f64,
    bandwidth_hz: Option<f64>,
    steer_timeout_ms: u32,
}

impl Settings {
    fn read(params: &ProcessorParams, lanes: usize) -> Result<Self, ChannelError> {
        let beamformer = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        if let Some(problem) = beamformer.problem() {
            return Err(ChannelError::Refused(problem));
        }
        let main = beamformer.main_lane as usize;
        let chosen = &beamformer.reference_lanes;
        if main >= lanes || chosen.iter().any(|&lane| lane as usize >= lanes) {
            return Err(ChannelError::Refused("Lane out of range"));
        }
        let mut references = [0; MAX_LANES];
        let refs = if chosen.is_empty() {
            for (slot, lane) in references.iter_mut().zip((0..lanes).filter(|&l| l != main)) {
                *slot = lane;
            }
            lanes - 1
        } else {
            for (slot, &lane) in references.iter_mut().zip(chosen) {
                *slot = lane as usize;
            }
            chosen.len().min(MAX_LANES)
        };
        let mut nulls = [0.0; MAX_BEAM_NULLS];
        for (slot, &null) in nulls.iter_mut().zip(&beamformer.nulls_deg) {
            *slot = null;
        }
        Ok(Self {
            mode: beamformer.mode,
            steer: beamformer.steer,
            nulls,
            null_count: beamformer.nulls_deg.len().min(MAX_BEAM_NULLS),
            auto_nulls: beamformer.auto_nulls,
            main,
            references,
            refs,
            taps: beamformer.taps as usize,
            adaptation: beamformer.adaptation,
            step: beamformer.step,
            forget: beamformer.forget,
            crossfade_ms: beamformer.crossfade_ms,
            update_ms: beamformer.update_ms,
            carry_over: beamformer.carry_over,
            loading: beamformer.loading,
            noise: beamformer.noise,
            offset_hz: beamformer.offset_hz,
            bandwidth_hz: beamformer.bandwidth_hz,
            steer_timeout_ms: beamformer.steer_timeout_ms,
        })
    }

    fn references(&self) -> &[usize] {
        &self.references[..self.refs]
    }

    const fn tdl_adaptation(&self) -> TdlAdaptation {
        match self.adaptation {
            Adaptation::Nlms => TdlAdaptation::Nlms { step: self.step },
            Adaptation::Rls => TdlAdaptation::Rls {
                forget: self.forget,
            },
        }
    }

    const fn wideband(&self) -> bool {
        matches!(self.mode, BeamMode::Canceller) && self.taps >= 2
    }

    fn rebuild_needed(&self, next: &Self) -> bool {
        band_of(self.offset_hz, self.bandwidth_hz) != band_of(next.offset_hz, next.bandwidth_hz)
            || steered(self.mode) != steered(next.mode)
            || runs_on_worker(self.mode, self.bandwidth_hz)
                != runs_on_worker(next.mode, next.bandwidth_hz)
            || self.taps != next.taps
            || self.adaptation != next.adaptation
            || self.main != next.main
            || self.references() != next.references()
    }

    fn check_cost(&self, out_rate: f64) -> Result<(), ChannelError> {
        if !self.wideband() {
            return Ok(());
        }
        let cost = tdl_cmac_per_sample(self.refs, self.taps, self.tdl_adaptation()) * out_rate;
        if cost > TDL_MAX_CMAC_PER_S {
            return Err(ChannelError::Refused("Too heavy for this band"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BeamFlags {
    no_steer: bool,
    steer_stale: bool,
    singular: bool,
    diverged: bool,
    band_full: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Path {
    Ramp,
    Gsc,
    Tdl,
    Cma,
}

fn unbuilt<E>(_: E) -> ChannelError {
    ChannelError::Refused("Too few elements")
}

fn manifold_of(ctx: &ArrayCtx<'_>) -> Option<Manifold> {
    let geometry = geometry_of(ctx.geometry, ctx.lanes).ok()?;
    match ctx.manifold {
        Some(table) => Manifold::measured(geometry, Arc::new(table.clone())).ok(),
        None => Some(Manifold::ideal(geometry)),
    }
}

pub struct BeamformerProcessor {
    band: LaneBand,
    core: Beamformer,
}

struct Beamformer {
    settings: Settings,
    lanes: usize,
    rate: f64,
    format: LaneFormat,
    freq_hz: f64,
    manifold: Option<Manifold>,
    ring: Option<SteeringGrid>,
    covariance: SampleCovariance,
    matrix: CMat,
    filled: bool,
    solver: BlockSolver,
    qr: Qr,
    constraints: Constraints,
    gsc: Gsc,
    gsc_for: Option<([f64; MAX_BEAM_NULLS + 1], usize)>,
    tdl: Option<TdlCanceller>,
    cma: Cma,
    cma_seeded: bool,
    ramp: WeightRamp,
    target: WeightSet,
    effective: WeightSet,
    noise: LaneNoise,
    pattern: [f32; PATTERN_POINTS],
    has_pattern: bool,
    metrics: BeamMetrics,
    beam_power: f32,
    steer: Option<Steer>,
    resolved: Resolved,
    heading: Option<f64>,
    since_update: u64,
    loading_used: f32,
    flags: BeamFlags,
    beam: Vec<Complex<f32>>,
    faults: ProcessorFaults,
}

impl Beamformer {
    fn build(
        ctx: &ArrayCtx<'_>,
        settings: Settings,
        band: &LaneBand,
    ) -> Result<Self, ChannelError> {
        let lanes = ctx.lanes;
        let format = band_lane_format(ctx, settings.offset_hz, settings.bandwidth_hz);
        settings.check_cost(format.sample_rate)?;
        let freq_hz = ctx.center_hz + band.center_offset_hz();
        let manifold = manifold_of(ctx);
        if steered(settings.mode) && manifold.is_none() {
            return Err(ChannelError::Refused("Needs array geometry"));
        }
        let ring = manifold.as_ref().and_then(|manifold| {
            SteeringGrid::new(manifold, GridSpec::ring(PATTERN_STEP_DEG), freq_hz).ok()
        });
        let tdl = if settings.taps >= 2 {
            let canceller = TdlCanceller::new(
                settings.main,
                settings.references(),
                settings.taps,
                settings.tdl_adaptation(),
            );
            Some(canceller.map_err(|_| ChannelError::Refused("Taps out of range"))?)
        } else {
            None
        };
        let target = WeightSet::unit(lanes, settings.main);
        let mut ramp = WeightRamp::new(lanes);
        ramp.set_target(&target, 0);
        let mut gsc = Gsc::new(lanes).map_err(unbuilt)?;
        gsc.set_step(settings.step)
            .map_err(|_| ChannelError::Refused("Step out of range"))?;
        Ok(Self {
            settings,
            lanes,
            rate: ctx.sample_rate,
            format,
            freq_hz,
            manifold,
            ring,
            covariance: SampleCovariance::new(lanes).map_err(unbuilt)?,
            matrix: CMat::zeros(lanes).map_err(unbuilt)?,
            filled: false,
            solver: BlockSolver::new(lanes).map_err(unbuilt)?,
            qr: Qr::new(lanes, 1).map_err(unbuilt)?,
            constraints: Constraints::new(lanes),
            gsc,
            gsc_for: None,
            tdl,
            cma: Cma::new(lanes, settings.step)
                .map_err(|_| ChannelError::Refused("Step out of range"))?,
            cma_seeded: false,
            ramp,
            effective: target.clone(),
            target,
            noise: LaneNoise::new(lanes).map_err(unbuilt)?,
            pattern: [0.0; PATTERN_POINTS],
            has_pattern: false,
            metrics: BeamMetrics::default(),
            beam_power: 0.0,
            steer: None,
            resolved: Resolved::default(),
            heading: None,
            since_update: 0,
            loading_used: 0.0,
            flags: BeamFlags::default(),
            beam: Vec::with_capacity(format.capacity),
            faults: ProcessorFaults::default(),
        })
    }

    fn update_samples(&self) -> u64 {
        ((self.rate * f64::from(self.settings.update_ms) / 1000.0).round() as u64).max(1)
    }

    fn crossfade_samples(&self) -> u32 {
        (self.format.sample_rate * f64::from(self.settings.crossfade_ms) / 1000.0).round() as u32
    }

    fn path(&self) -> Path {
        match self.settings.mode {
            BeamMode::Gsc if self.gsc_for.is_some() => Path::Gsc,
            BeamMode::Canceller if self.settings.wideband() && self.tdl.is_some() => Path::Tdl,
            BeamMode::Cma if self.cma_seeded => Path::Cma,
            _ => Path::Ramp,
        }
    }

    fn block(
        &mut self,
        lanes: &[&[Complex<f32>]],
        block: &ArrayBlock<'_>,
        out: &mut ProcessorOutput<'_>,
    ) {
        let n = lanes.first().map_or(0, |lane| lane.len());
        self.covariance.accumulate(lanes);
        self.beam.clear();
        let result = match self.path() {
            Path::Ramp => self.ramp.apply(lanes, &mut self.beam),
            Path::Gsc => self.gsc.process(lanes, &mut self.beam),
            Path::Cma => self.cma.process(lanes, &mut self.beam),
            Path::Tdl => match self.tdl.as_mut() {
                Some(tdl) => tdl.process(lanes, &mut self.beam),
                None => Err(BeamError::Lanes(0)),
            },
        };
        self.emit(result, n, out);
        self.since_update += block.len() as u64;
        let every = self.update_samples();
        if self.since_update >= every {
            self.since_update %= every;
            self.update(lanes, block);
            self.report(block.unix_ns, out);
        }
    }

    fn emit(&mut self, result: Result<(), BeamError>, n: usize, out: &mut ProcessorOutput<'_>) {
        match result {
            Ok(()) => {}
            Err(BeamError::Diverged) => {
                self.flags.diverged = true;
                self.faults.resets += 1;
            }
            Err(_) => {
                self.faults.dropped_blocks += 1;
                self.beam.clear();
            }
        }
        if self.beam.len() != n {
            out.skip_lane(BEAM, n as u64);
            return;
        }
        if let Some(mut lane) = out.lane(BEAM) {
            lane.extend(&self.beam);
        }
    }

    fn restart(&mut self) {
        self.covariance.reset();
        self.noise.reset();
        self.gsc.reset();
        self.gsc_for = None;
        if let Some(tdl) = self.tdl.as_mut() {
            tdl.reset();
        }
        self.cma_seeded = false;
        self.since_update = 0;
    }

    fn apply(&mut self, settings: Settings) -> Result<(), ChannelError> {
        if self.settings.rebuild_needed(&settings) {
            return Err(ChannelError::Refused("Needs a rebuild"));
        }
        settings.check_cost(self.format.sample_rate)?;
        let step = |_| ChannelError::Refused("Step out of range");
        self.gsc.set_step(settings.step).map_err(step)?;
        self.cma.set_step(settings.step).map_err(step)?;
        if let Some(tdl) = self.tdl.as_mut() {
            tdl.set_adaptation(settings.tdl_adaptation())
                .map_err(step)?;
        }
        if settings.mode != self.settings.mode {
            self.gsc_for = None;
            self.cma_seeded = false;
        }
        self.settings = settings;
        Ok(())
    }

    fn retune(&mut self, ctx: &ArrayCtx<'_>, band: &LaneBand) -> Result<(), ChannelError> {
        let settings = &self.settings;
        self.format = band_lane_format(ctx, settings.offset_hz, settings.bandwidth_hz);
        self.freq_hz = ctx.center_hz + band.center_offset_hz();
        if let (Some(manifold), Some(ring)) = (self.manifold.as_ref(), self.ring.as_mut()) {
            ring.rebuild(manifold, self.freq_hz)
                .map_err(|_| ChannelError::Refused("Rate out of range"))?;
        }
        self.restart();
        Ok(())
    }
}

impl BeamformerProcessor {
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

impl ArrayProcessor for BeamformerProcessor {
    fn descriptor() -> &'static ProcessorDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: &ArrayCtx<'_>, params: &ProcessorParams) -> Result<Self, ChannelError> {
        if ctx.lanes > MAX_LANES {
            return Err(ChannelError::Refused("Too many elements"));
        }
        let settings = Settings::read(params, ctx.lanes)?;
        check_tuning(TuningNeed::Together, ctx)?;
        if settings.bandwidth_hz.is_some() {
            check_offset(ctx.sample_rate, settings.offset_hz)?;
        }
        let band = LaneBand::new(
            ctx.lanes,
            ctx.sample_rate,
            settings.offset_hz,
            settings.bandwidth_hz,
            ctx.max_block,
        )?;
        let core = Beamformer::build(ctx, settings, &band)?;
        Ok(Self { band, core })
    }

    fn apply(&mut self, params: &ProcessorParams) -> Result<(), ChannelError> {
        let settings = Settings::read(params, self.core.lanes)?;
        self.core.apply(settings)
    }

    fn retune(&mut self, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
        self.check_layout(ctx)?;
        self.band.reset();
        self.core.retune(ctx, &self.band)
    }

    fn reset(&mut self, _cause: ResetCause) {
        self.band.reset();
        self.core.restart();
        self.core.faults.resets += 1;
    }

    fn steer(&mut self, steer: &Steer) {
        self.core.steer = Some(*steer);
    }

    fn action(&mut self, _action: ProcessorAction) -> Result<(), ChannelError> {
        Err(ChannelError::Refused("No tracks here"))
    }

    fn process(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        let lanes = self.core.lanes;
        if !self.core.faults.lanes_match(block, lanes) {
            let ratio = self.core.format.sample_rate / self.core.rate;
            out.skip_lane(BEAM, (block.len() as f64 * ratio).round() as u64);
            return;
        }
        let mut views: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let n = self.band.process(block, &mut views);
        for view in &mut views[..lanes] {
            *view = &view[..n];
        }
        self.core.block(&views[..lanes], block, out);
    }

    fn faults(&self) -> ProcessorFaults {
        self.core.faults
    }
}

#[cfg(test)]
mod tests;
