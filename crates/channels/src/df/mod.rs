mod heading;
mod report;
#[cfg(test)]
mod tests;

pub use heading::HeadingAverager;

use std::sync::Arc;

use num_complex::Complex;
use sdrmm_dsp::covariance::{CovarianceError, SampleCovariance};
use sdrmm_dsp::doa::{
    Doa, DoaConfig, DoaError, DoaReport, ELEVATION_NEEDS_2D, Estimator, FB_NEEDS_SYMMETRY,
    LIKELIHOOD_POINTS, NEEDS_LINE_OR_CIRCLE, OrderRule, Reconfigure, SourceCount, SquelchConfig,
    TOO_FEW_ELEMENTS, UlaSide as DoaSide,
};
use sdrmm_dsp::linalg::CMat;
use sdrmm_dsp::manifold::{AliasReport, ElevationSpan, Manifold, ManifoldError};
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::processor::df::DF_POINTS;
use sdrmm_wire::{DfAlgorithm, DfParams, ProcessorParams, SourceRule, UlaSide};

use crate::ChannelError;
use crate::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, CalView, Execution, MAX_LANES, Pose, ProcessorAction,
    ProcessorDescriptor, ProcessorFaults, ProcessorNeeds, ProcessorOutput, Registration,
    ResetCause, TuningNeed, boxed, check_tuning, geometry_of, no_lane_format,
};
use crate::band::LaneBand;

const SQUELCH_HYSTERESIS_DB: f32 = 1.0;

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
    create: Some(boxed::<DfProcessor>),
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

fn doa_config(df: &DfParams) -> DoaConfig {
    DoaConfig {
        estimator: match df.algorithm {
            DfAlgorithm::Bartlett => Estimator::Bartlett,
            DfAlgorithm::Capon => Estimator::Capon,
            DfAlgorithm::Music => Estimator::Music,
            DfAlgorithm::RootMusic => Estimator::RootMusic,
            DfAlgorithm::Esprit => Estimator::Esprit,
        },
        sources: df.sources.map_or(SourceCount::Auto, |count| {
            SourceCount::Fixed(u8::try_from(count).unwrap_or(u8::MAX))
        }),
        rule: match df.source_rule {
            SourceRule::Dominance => OrderRule::Dominance,
            SourceRule::Mdl => OrderRule::Mdl,
        },
        max_peaks: df.max_peaks,
        forward_backward: df.forward_backward,
        smoothing: df.smoothing,
        loading: df.loading,
        azimuth_step_deg: df.azimuth_step_deg,
        elevation: df.elevation.then(ElevationSpan::default),
        ula_side: match df.ula_side {
            UlaSide::Both => DoaSide::Both,
            UlaSide::Front => DoaSide::Front,
            UlaSide::Back => DoaSide::Back,
        },
        squelch: (df.squelch_db > 0.0).then_some(SquelchConfig {
            open_db: df.squelch_db,
            hysteresis_db: SQUELCH_HYSTERESIS_DB,
        }),
        ..DoaConfig::default()
    }
}

const fn refusal(error: DoaError) -> ChannelError {
    ChannelError::Refused(match error {
        DoaError::Unsupported(text) => text,
        DoaError::Covariance(CovarianceError::BesselNull) => "Bessel null here",
        DoaError::Covariance(CovarianceError::NotStructured) => NEEDS_LINE_OR_CIRCLE,
        DoaError::Covariance(CovarianceError::NotSymmetric) => FB_NEEDS_SYMMETRY,
        DoaError::Covariance(CovarianceError::TooFewForSmoothing) => TOO_FEW_ELEMENTS,
        DoaError::Manifold(ManifoldError::Frequency) => "Frequency out of range",
        DoaError::Manifold(ManifoldError::GridTooLarge(_) | ManifoldError::GridStep) => {
            "Step out of range"
        }
        DoaError::Manifold(ManifoldError::ElevationOnLine) => ELEVATION_NEEDS_2D,
        DoaError::Linalg(_) => "Too many elements",
        _ => "Needs array geometry",
    })
}

fn report_samples(rate: f64, report_ms: u32) -> u64 {
    ((rate * f64::from(report_ms) / 1_000.0).round() as u64).max(1)
}

pub(crate) fn check_band(
    rate: f64,
    offset_hz: f64,
    bandwidth_hz: Option<f64>,
) -> Result<(), ChannelError> {
    match bandwidth_hz {
        Some(bandwidth) if offset_hz.abs() + bandwidth / 2.0 > rate / 2.0 => {
            Err(ChannelError::Refused("Offset out of range"))
        }
        _ => Ok(()),
    }
}

pub(crate) fn manifold_of(ctx: &ArrayCtx<'_>) -> Result<Manifold, ChannelError> {
    let geometry = geometry_of(ctx.geometry, ctx.lanes)?;
    match ctx.manifold {
        Some(table) => Manifold::measured(geometry, Arc::new(table.clone()))
            .map_err(|_| ChannelError::Refused("Needs array geometry")),
        None => Ok(Manifold::ideal(geometry)),
    }
}

pub(crate) fn same_array(manifold: &Manifold, ctx: &ArrayCtx<'_>) -> bool {
    let same_table = match (ctx.manifold, manifold.table()) {
        (None, None) => true,
        (Some(given), Some(kept)) => given == kept,
        _ => false,
    };
    same_table
        && geometry_of(ctx.geometry, ctx.lanes)
            .is_ok_and(|geometry| &geometry == manifold.geometry())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    algorithm: DfAlgorithm,
    sources_auto: bool,
    elevation: bool,
    offset_hz: f64,
    carry_over: f32,
    yaw_gate_dps: f32,
}

impl Settings {
    const fn of(df: &DfParams) -> Self {
        Self {
            algorithm: df.algorithm,
            sources_auto: df.sources.is_none(),
            elevation: df.elevation,
            offset_hz: df.offset_hz,
            carry_over: df.carry_over,
            yaw_gate_dps: df.yaw_gate_dps,
        }
    }
}

struct Buffers {
    spectrum: [u8; DF_POINTS],
    likelihood_rel: [f32; LIKELIHOOD_POINTS],
    likelihood_out: [f32; LIKELIHOOD_POINTS],
    likelihood: [u8; DF_POINTS],
}

impl Buffers {
    const fn new() -> Self {
        Self {
            spectrum: [0; DF_POINTS],
            likelihood_rel: [0.0; LIKELIHOOD_POINTS],
            likelihood_out: [0.0; LIKELIHOOD_POINTS],
            likelihood: [0; DF_POINTS],
        }
    }
}

pub struct DfProcessor {
    node: String,
    station: String,
    settings: Settings,
    lanes: usize,
    rate: f64,
    center_hz: f64,
    manifold: Manifold,
    band: LaneBand,
    covariance: SampleCovariance,
    matrix: CMat,
    doa: Doa,
    estimate: DoaReport,
    heading: HeadingAverager,
    alias: AliasReport,
    buffers: Buffers,
    table_out_of_range: bool,
    since_report: u64,
    report_samples: u64,
    pose: Pose,
    cal: CalView,
    unix_ns: u64,
    gated: u32,
    fresh: bool,
    faults: ProcessorFaults,
}

impl DfProcessor {
    fn freq_hz(&self) -> f64 {
        self.center_hz + self.settings.offset_hz
    }

    fn refresh_table_state(&mut self) {
        self.table_out_of_range =
            self.manifold.table().is_some() && !self.manifold.uses_table_at(self.freq_hz());
    }

    fn clear(&mut self) {
        self.covariance.reset();
        self.heading.reset();
        self.band.reset();
        self.since_report = 0;
        self.gated = 0;
        self.fresh = false;
    }

    fn rotating(&self, pose: &Pose) -> bool {
        pose.yaw_rate_dps
            .is_some_and(|rate| rate.abs() > self.settings.yaw_gate_dps)
    }

    fn accumulate(&mut self, block: &ArrayBlock<'_>) {
        let rotating = self.rotating(&block.pose);
        let mut views: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let len = self.band.process(block, &mut views);
        if rotating {
            self.gated = self.gated.saturating_add(1);
            return;
        }
        for view in &mut views[..self.lanes] {
            *view = &view[..len];
        }
        self.covariance.accumulate(&views[..self.lanes]);
        self.heading.add(&block.pose, block.len());
        self.fresh = true;
    }
}

impl ArrayProcessor for DfProcessor {
    fn descriptor() -> &'static ProcessorDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: &ArrayCtx<'_>, params: &ProcessorParams) -> Result<Self, ChannelError> {
        let df = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        check_tuning(TuningNeed::Together, ctx)?;
        check_band(ctx.sample_rate, df.offset_hz, Some(df.bandwidth_hz))?;
        let manifold = manifold_of(ctx)?;
        let band = LaneBand::new(
            ctx.lanes,
            ctx.sample_rate,
            df.offset_hz,
            Some(df.bandwidth_hz),
            ctx.max_block,
        )?;
        let doa =
            Doa::new(&manifold, &doa_config(df), ctx.center_hz + df.offset_hz).map_err(refusal)?;
        let covariance = SampleCovariance::new(ctx.lanes)
            .map_err(|_| ChannelError::Refused("Too many elements"))?;
        let matrix =
            CMat::identity(ctx.lanes).map_err(|_| ChannelError::Refused("Too many elements"))?;
        let mut processor = Self {
            node: ctx.node.to_owned(),
            station: df.station_id.clone().unwrap_or_else(|| ctx.node.to_owned()),
            settings: Settings::of(df),
            lanes: ctx.lanes,
            rate: ctx.sample_rate,
            center_hz: ctx.center_hz,
            alias: *doa.alias(),
            manifold,
            band,
            covariance,
            matrix,
            doa,
            estimate: DoaReport::default(),
            heading: HeadingAverager::default(),
            buffers: Buffers::new(),
            table_out_of_range: false,
            since_report: 0,
            report_samples: report_samples(ctx.sample_rate, df.report_ms),
            pose: Pose::default(),
            cal: CalView::default(),
            unix_ns: 0,
            gated: 0,
            fresh: false,
            faults: ProcessorFaults::default(),
        };
        processor.refresh_table_state();
        Ok(processor)
    }

    fn apply(&mut self, params: &ProcessorParams) -> Result<(), ChannelError> {
        let df = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        if let Some(problem) = df.problem() {
            return Err(ChannelError::Refused(problem));
        }
        match self.doa.configure(&self.manifold, &doa_config(df)) {
            Ok(Reconfigure::InPlace) => {}
            Ok(Reconfigure::NeedsNew) => return Err(ChannelError::Refused("Needs a rebuild")),
            Err(error) => return Err(refusal(error)),
        }
        self.settings = Settings {
            offset_hz: self.settings.offset_hz,
            elevation: self.settings.elevation,
            ..Settings::of(df)
        };
        self.report_samples = report_samples(self.rate, df.report_ms);
        Ok(())
    }

    fn retune(&mut self, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
        if ctx.lanes != self.lanes {
            return Err(ChannelError::Refused("Lane out of range"));
        }
        if ctx.sample_rate != self.rate {
            return Err(ChannelError::Refused("Rate out of range"));
        }
        check_tuning(TuningNeed::Together, ctx)?;
        if !same_array(&self.manifold, ctx) {
            return Err(ChannelError::Refused("Array changed"));
        }
        let freq_hz = ctx.center_hz + self.settings.offset_hz;
        self.alias = self.doa.retune(&self.manifold, freq_hz).map_err(refusal)?;
        self.center_hz = ctx.center_hz;
        self.refresh_table_state();
        self.clear();
        Ok(())
    }

    fn reset(&mut self, _cause: ResetCause) {
        self.clear();
        self.doa.reset();
        self.faults.resets += 1;
    }

    fn action(&mut self, _action: ProcessorAction) -> Result<(), ChannelError> {
        Err(ChannelError::Refused("No tracks here"))
    }

    fn process(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        if !self.faults.lanes_match(block, self.lanes) {
            return;
        }
        self.pose = block.pose;
        self.cal = block.cal;
        self.unix_ns = block.unix_ns;
        self.accumulate(block);
        self.since_report += block.len() as u64;
        if self.since_report >= self.report_samples {
            self.since_report %= self.report_samples;
            self.publish(out);
        }
    }

    fn faults(&self) -> ProcessorFaults {
        self.faults
    }
}
