use std::fmt;
use std::ops::Range;

use sdrmm_dsp::manifold::Geometry;
use sdrmm_dsp::radar::RadarDspError;
use sdrmm_dsp::radar::batch::{BatchShape, DopplerTaper, MAX_SURVEILLANCE};
use sdrmm_dsp::radar::cfar::{AlphaTable, CfarSpec, RHO_TABLE};
use sdrmm_dsp::radar::threshold::CfarStatistic;
use sdrmm_dsp::radar::track::TrackerConfig;
use sdrmm_dsp::radar::wiener::GroupPlan;
use sdrmm_wire::ProcessorParams;
use sdrmm_wire::radar::{
    CfarKind, CfarParams, CfarWindow, ClutterMethod, DAB_SAMPLE_RATE_HZ, DopplerWindow,
    Illuminator, LIGHT_SPEED_M_S, MAX_ECA_ORDER, PassiveRadarParams, RADAR_MAX_BATCHES,
    RADAR_MAX_GATES, RADAR_MAX_WORKING_BYTES, RADAR_MIN_BATCHES, ReferenceCleaning,
    SurveillanceSet, TrackerParams,
};

use crate::ChannelError;
use crate::array_processor::{ArrayCtx, geometry_of, lanes_spread};

pub const MAX_LOOKS: usize = MAX_SURVEILLANCE;
pub const AOA_GRID_STEP_DEG: f32 = 0.5;
pub const DAB_FRAME_LATENCY: usize = 196_608 + 2 * 2_552;

const BAND_EDGE: f64 = 0.45;
const OCCUPANCY: f64 = 0.8;
const DOPPLER_OVERSAMPLING: f64 = 4.0;
const MIN_BATCH_LEN: usize = 16;
const NLMS_LOAD: f64 = 4.0e7;
const BLOCK_NLMS_LOAD: f64 = 2.0e9;
const BLOCK_NLMS_COST: f64 = 50.0;
const LINE_TOLERANCE_M: f64 = 1e-3;
const RHO_COLUMNS: usize = RHO_TABLE.len();

#[derive(Clone, Debug, PartialEq)]
pub struct RadarCtx {
    pub sample_rate: f64,
    pub center_hz: f64,
    pub elements: usize,
    pub positions_m: Vec<[f64; 3]>,
    pub tuned_together: bool,
}

impl RadarCtx {
    #[must_use]
    pub fn of_array(ctx: &ArrayCtx<'_>) -> Self {
        let positions_m = if ctx.positions_m.len() == ctx.lanes {
            ctx.positions_m.to_vec()
        } else {
            geometry_of(ctx.geometry, ctx.lanes)
                .map(|geometry| positions_of(&geometry))
                .unwrap_or_default()
        };
        Self {
            sample_rate: ctx.sample_rate,
            center_hz: ctx.center_hz,
            elements: ctx.lanes,
            positions_m,
            tuned_together: !lanes_spread(ctx),
        }
    }
}

#[must_use]
pub fn positions_of(geometry: &Geometry) -> Vec<[f64; 3]> {
    geometry
        .positions()
        .iter()
        .map(|p| [p.x, p.y, p.z])
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CancellerPlan {
    pub block: bool,
    pub taps: usize,
    pub lead: usize,
    pub step: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrontPlan {
    pub input_rate: f64,
    pub radar_rate: f64,
    pub offset_hz: f64,
    pub bandwidth_hz: f64,
    pub lanes: Vec<usize>,
    pub cleaning: ReferenceCleaning,
    pub canceller: Option<CancellerPlan>,
    pub latency: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AoaPlan {
    pub positions_m: Vec<[f64; 3]>,
    pub mirror_axis_deg: Option<f32>,
    pub grid_step_deg: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alphas {
    table: [[[f32; 2]; RHO_COLUMNS]; MAX_LOOKS],
}

impl Alphas {
    fn of(table: &AlphaTable) -> Self {
        let mut out = [[[0.0; 2]; RHO_COLUMNS]; MAX_LOOKS];
        for (look, row) in (1u32..).zip(out.iter_mut()) {
            for (cell, rho) in row.iter_mut().zip(RHO_TABLE) {
                let (alpha, edge) = table.pick(look, rho);
                *cell = [alpha, edge];
            }
        }
        Self { table: out }
    }

    #[must_use]
    pub fn pick(&self, looks: u32, correlation: f64) -> (f32, f32) {
        let look = (looks.max(1) as usize).min(MAX_LOOKS) - 1;
        let column = RHO_TABLE
            .iter()
            .position(|&rho| rho >= correlation)
            .unwrap_or(RHO_COLUMNS - 1);
        let [alpha, edge] = self.table[look][column];
        (alpha, edge)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveParams {
    pub cfar: CfarSpec,
    pub tracker: TrackerConfig,
    pub aoa: bool,
    pub hop: usize,
    pub alphas: Alphas,
    pub doppler_correlation: f64,
    pub min_range_m: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RadarPlan {
    pub ctx: RadarCtx,
    pub params: PassiveRadarParams,
    pub front: FrontPlan,
    pub carrier_hz: f64,
    pub wavelength_m: f64,
    pub range_step_m: f64,
    pub shape: BatchShape,
    pub groups: Option<GroupPlan>,
    pub taper: DopplerTaper,
    pub window: Vec<f32>,
    pub hop: usize,
    pub cpi_s: f64,
    pub report_rows: Range<usize>,
    pub clutter_half_rows: usize,
    pub min_gate: usize,
    pub looks: u32,
    pub correlation_doppler: f64,
    pub alphas: Alphas,
    pub cfar: CfarSpec,
    pub tracker: TrackerConfig,
    pub range_resolution_m: f64,
    pub doppler_resolution_hz: f64,
    pub aoa: Option<AoaPlan>,
    pub working_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanChange {
    Same,
    Live,
    Rebuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    Settings(&'static str),
    ReferenceMissing,
    SurveillanceMissing,
    DabRate,
    Band,
    Spread,
    Gates,
    Batches,
    Order,
    NlmsLoad,
    Memory,
    Geometry,
    Threshold,
}

impl PlanError {
    #[must_use]
    pub const fn text(&self) -> &'static str {
        match self {
            Self::Settings(text) => text,
            Self::ReferenceMissing => "Reference not in the array",
            Self::SurveillanceMissing => "Surveillance not in the array",
            Self::DabRate => "DAB needs 2.048 MS/s",
            Self::Band => "Band outside the capture",
            Self::Spread => "Array must be tuned together",
            Self::Gates => "Too many range gates",
            Self::Batches => "Too many batches",
            Self::Order => "Clutter order over 512",
            Self::NlmsLoad => "NLMS too slow here",
            Self::Memory => "CPI too large",
            Self::Geometry => "Array geometry missing",
            Self::Threshold => "No CFAR threshold for this Pfa",
        }
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.text())
    }
}

impl std::error::Error for PlanError {}

impl From<PlanError> for ChannelError {
    fn from(error: PlanError) -> Self {
        Self::Refused(error.text())
    }
}

impl From<RadarDspError> for PlanError {
    fn from(error: RadarDspError) -> Self {
        match error {
            RadarDspError::Order => Self::Order,
            RadarDspError::Size => Self::Memory,
            RadarDspError::Shape | RadarDspError::Singular | RadarDspError::Setting => {
                Self::Batches
            }
        }
    }
}

struct Doppler {
    batches: usize,
    batch_len: usize,
    cpi_s: f64,
    report_rows: Range<usize>,
}

struct Clutter {
    lead: usize,
    taps: usize,
    eca: bool,
    canceller: Option<CancellerPlan>,
}

struct Detection {
    alphas: Alphas,
    cfar: CfarSpec,
    tracker: TrackerConfig,
    range_resolution_m: f64,
    doppler_resolution_hz: f64,
    clutter_half_rows: usize,
    min_gate: usize,
}

pub fn plan(ctx: &RadarCtx, params: &PassiveRadarParams) -> Result<RadarPlan, PlanError> {
    if let Some(problem) = params.problem() {
        return Err(PlanError::Settings(problem));
    }
    let lanes = element_lanes(ctx, params)?;
    let surveillance = lanes.len() - 1;
    if !ctx.tuned_together {
        return Err(PlanError::Spread);
    }
    let bandwidth = occupied_bandwidth(ctx, params)?;
    let radar_rate = radar_rate(ctx.sample_rate, bandwidth, params.illuminator)?;
    let carrier_hz = ctx.center_hz + params.offset_hz;
    if !(carrier_hz.is_finite() && carrier_hz > 0.0) {
        return Err(PlanError::Band);
    }
    let wavelength_m = LIGHT_SPEED_M_S / carrier_hz;
    let range_step_m = LIGHT_SPEED_M_S / radar_rate;
    let gates = (f64::from(params.max_range_km) * 1_000.0 / range_step_m).ceil() as usize + 1;
    if gates > RADAR_MAX_GATES as usize {
        return Err(PlanError::Gates);
    }
    let doppler = doppler_grid(
        params,
        f64::from(params.max_speed_mps) / wavelength_m,
        radar_rate,
    )?;
    let clutter = clutter_taps(params, range_step_m, radar_rate, surveillance)?;
    let shape = BatchShape::new(
        doppler.batches,
        doppler.batch_len,
        gates,
        clutter.lead,
        clutter.taps,
        surveillance,
    )?;
    let groups = clutter
        .eca
        .then(|| group_plan(params, &shape, radar_rate))
        .transpose()?;
    let taper = taper_of(params.window);
    let mut window = vec![0.0f32; shape.batches];
    taper.fill(&mut window);
    let hop =
        ((shape.samples() as f64 * (1.0 - f64::from(params.overlap))).round() as usize).max(1);
    let detection = detection(
        params,
        &shape,
        &doppler,
        range_step_m,
        wavelength_m,
        bandwidth,
    )?;
    let aoa = aoa_plan(ctx, params, &lanes[1..])?;
    let working_bytes = working_bytes(&shape);
    if working_bytes > RADAR_MAX_WORKING_BYTES {
        return Err(PlanError::Memory);
    }
    let front = FrontPlan {
        input_rate: ctx.sample_rate,
        radar_rate,
        offset_hz: params.offset_hz,
        bandwidth_hz: bandwidth,
        lanes,
        cleaning: params.reference,
        canceller: clutter.canceller,
        latency: front_latency(params.reference),
    };
    Ok(RadarPlan {
        ctx: ctx.clone(),
        params: *params,
        front,
        carrier_hz,
        wavelength_m,
        range_step_m,
        shape,
        groups,
        taper,
        window,
        hop,
        cpi_s: doppler.cpi_s,
        report_rows: doppler.report_rows,
        clutter_half_rows: detection.clutter_half_rows,
        min_gate: detection.min_gate,
        looks: surveillance as u32,
        correlation_doppler: taper.enbw(),
        alphas: detection.alphas,
        cfar: detection.cfar,
        tracker: detection.tracker,
        range_resolution_m: detection.range_resolution_m,
        doppler_resolution_hz: detection.doppler_resolution_hz,
        aoa,
        working_bytes,
    })
}

pub fn plan_for(ctx: &ArrayCtx<'_>, params: &ProcessorParams) -> Result<RadarPlan, ChannelError> {
    Ok(plan(&RadarCtx::of_array(ctx), params_of(params)?)?)
}

pub fn params_of(params: &ProcessorParams) -> Result<&PassiveRadarParams, ChannelError> {
    match params {
        ProcessorParams::PassiveRadar(radar) => Ok(radar),
        _ => Err(ChannelError::Refused("Wrong settings")),
    }
}

#[must_use]
pub fn change(old: &RadarPlan, new: &RadarPlan) -> PlanChange {
    if old.ctx != new.ctx {
        return PlanChange::Rebuild;
    }
    if old.params == new.params {
        return PlanChange::Same;
    }
    let structural = |params: &PassiveRadarParams| PassiveRadarParams {
        cfar: CfarParams::default(),
        tracker: TrackerParams::default(),
        aoa: false,
        overlap: 0.0,
        assumed_altitude_m: 0.0,
        ..*params
    };
    if structural(&old.params) == structural(&new.params) {
        PlanChange::Live
    } else {
        PlanChange::Rebuild
    }
}

impl RadarPlan {
    #[must_use]
    pub fn live(&self) -> LiveParams {
        LiveParams {
            cfar: self.cfar,
            tracker: self.tracker,
            aoa: self.params.aoa,
            hop: self.hop,
            alphas: self.alphas,
            doppler_correlation: if self.cfar.plane {
                self.correlation_doppler
            } else {
                1.0
            },
            min_range_m: f64::from(self.params.cfar.min_range_km) * 1_000.0,
        }
    }

    #[must_use]
    pub const fn lanes(&self) -> usize {
        self.shape.lanes
    }

    #[must_use]
    pub fn hop_s(&self) -> f64 {
        self.hop as f64 / self.front.radar_rate
    }

    #[must_use]
    pub fn doppler_step_hz(&self) -> f64 {
        1.0 / self.cpi_s
    }

    #[must_use]
    pub fn doppler_of_row(&self, row: f64) -> f64 {
        (row - (self.shape.batches / 2) as f64) / self.cpi_s
    }
}

fn element_lanes(ctx: &RadarCtx, params: &PassiveRadarParams) -> Result<Vec<usize>, PlanError> {
    let elements = u32::try_from(ctx.elements).map_err(|_| PlanError::ReferenceMissing)?;
    let reference = params.reference_element;
    if reference >= elements {
        return Err(PlanError::ReferenceMissing);
    }
    if let SurveillanceSet::Mask { mask } = params.surveillance
        && elements < u32::BITS
        && mask >> elements != 0
    {
        return Err(PlanError::SurveillanceMissing);
    }
    let mut lanes = vec![reference as usize];
    lanes.extend(
        params
            .surveillance
            .elements(reference, elements)
            .map(|element| element as usize),
    );
    if !(2..=MAX_SURVEILLANCE + 1).contains(&lanes.len()) {
        return Err(PlanError::SurveillanceMissing);
    }
    Ok(lanes)
}

fn occupied_bandwidth(ctx: &RadarCtx, params: &PassiveRadarParams) -> Result<f64, PlanError> {
    let input = ctx.sample_rate;
    if !(input.is_finite() && input > 0.0) {
        return Err(PlanError::Band);
    }
    let bandwidth = match params.illuminator {
        Illuminator::DvbtPartial { bandwidth_hz } | Illuminator::Custom { bandwidth_hz } => {
            bandwidth_hz.min(OCCUPANCY * input)
        }
        Illuminator::Fm | Illuminator::Dab => params.illuminator.bandwidth_hz(),
    };
    if params.offset_hz.abs() + bandwidth / 2.0 > BAND_EDGE * input {
        return Err(PlanError::Band);
    }
    Ok(bandwidth)
}

fn radar_rate(input: f64, bandwidth: f64, illuminator: Illuminator) -> Result<f64, PlanError> {
    if matches!(illuminator, Illuminator::Dab) {
        if input < DAB_SAMPLE_RATE_HZ {
            return Err(PlanError::DabRate);
        }
        return Ok(DAB_SAMPLE_RATE_HZ);
    }
    let decimation = (input / (bandwidth / OCCUPANCY)).floor().max(1.0);
    Ok(input / decimation)
}

fn doppler_grid(
    params: &PassiveRadarParams,
    max_doppler: f64,
    radar_rate: f64,
) -> Result<Doppler, PlanError> {
    let requested_s = f64::from(params.cpi_ms) / 1_000.0;
    let needed = (DOPPLER_OVERSAMPLING * max_doppler * requested_s)
        .ceil()
        .max(1.0) as usize;
    let batches = needed.next_power_of_two();
    if batches > RADAR_MAX_BATCHES as usize {
        return Err(PlanError::Batches);
    }
    let batches = batches.max(RADAR_MIN_BATCHES as usize);
    let batch_len = (requested_s * radar_rate / batches as f64).round() as usize;
    if batch_len < MIN_BATCH_LEN {
        return Err(PlanError::Batches);
    }
    let cpi_s = (batches * batch_len) as f64 / radar_rate;
    let shown = 2 * (max_doppler * cpi_s).floor() as usize + 1;
    let half = (shown - 1) / 2;
    let centre = batches / 2;
    let report_rows = centre.saturating_sub(half)..(centre + half + 1).min(batches);
    Ok(Doppler {
        batches,
        batch_len,
        cpi_s,
        report_rows,
    })
}

fn clutter_taps(
    params: &PassiveRadarParams,
    range_step_m: f64,
    radar_rate: f64,
    surveillance: usize,
) -> Result<Clutter, PlanError> {
    let clutter = params.clutter;
    let taps = (f64::from(clutter.reach_km) * 1_000.0 / range_step_m).ceil() as usize;
    let lead = clutter.lead as usize;
    let order = lead + taps;
    let none = Clutter {
        lead: 0,
        taps: 0,
        eca: false,
        canceller: None,
    };
    match clutter.method {
        ClutterMethod::Off => Ok(none),
        ClutterMethod::EcaBatch | ClutterMethod::EcaSliding => {
            let unknowns = order * (2 * clutter.doppler_taps as usize + 1);
            if unknowns > MAX_ECA_ORDER as usize {
                return Err(PlanError::Order);
            }
            Ok(Clutter {
                lead,
                taps,
                eca: true,
                canceller: None,
            })
        }
        ClutterMethod::Nlms | ClutterMethod::BlockNlms => {
            let block = clutter.method == ClutterMethod::BlockNlms;
            let lanes = surveillance as f64;
            let load = if block {
                let fft = (2 * order.next_power_of_two()) as f64;
                lanes * radar_rate * BLOCK_NLMS_COST * fft.log2() / BLOCK_NLMS_LOAD
            } else {
                lanes * order as f64 * radar_rate / NLMS_LOAD
            };
            if load > 1.0 {
                return Err(PlanError::NlmsLoad);
            }
            Ok(Clutter {
                canceller: Some(CancellerPlan {
                    block,
                    taps,
                    lead,
                    step: clutter.step,
                }),
                ..none
            })
        }
    }
}

fn group_plan(
    params: &PassiveRadarParams,
    shape: &BatchShape,
    radar_rate: f64,
) -> Result<GroupPlan, PlanError> {
    let clutter = params.clutter;
    let per_batch_s = shape.batch_len as f64 / radar_rate;
    let group_batches =
        ((f64::from(clutter.batch_ms) / 1_000.0 / per_batch_s).round() as usize).max(1);
    let extension = (f64::from(clutter.extension_ms) / 1_000.0 / per_batch_s).round() as usize;
    Ok(GroupPlan::split(
        shape.batches,
        group_batches,
        extension,
        clutter.doppler_taps as usize,
        clutter.taper,
        clutter.method == ClutterMethod::EcaSliding,
    )?)
}

const fn taper_of(window: DopplerWindow) -> DopplerTaper {
    match window {
        DopplerWindow::Hann => DopplerTaper::Hann,
        DopplerWindow::BlackmanHarris => DopplerTaper::BlackmanHarris,
        DopplerWindow::Rectangular => DopplerTaper::Rectangular,
    }
}

const fn statistic(kind: CfarKind) -> CfarStatistic {
    match kind {
        CfarKind::Ca => CfarStatistic::Ca,
        CfarKind::Os { rank } => CfarStatistic::Os { rank },
        CfarKind::Go => CfarStatistic::Go,
    }
}

fn detection(
    params: &PassiveRadarParams,
    shape: &BatchShape,
    doppler: &Doppler,
    range_step_m: f64,
    wavelength_m: f64,
    bandwidth: f64,
) -> Result<Detection, PlanError> {
    let cfar = params.cfar;
    let clutter_half_rows = (f64::from(cfar.min_doppler_hz) * doppler.cpi_s).floor() as usize;
    let min_gate = (f64::from(cfar.min_range_km) * 1_000.0 / range_step_m).ceil() as usize;
    let shaped = CfarSpec {
        stat: statistic(cfar.kind),
        plane: cfar.window == CfarWindow::Plane,
        guard_range: cfar.guard_range as usize,
        train_range: cfar.train_range as usize,
        guard_doppler: cfar.guard_doppler as usize,
        train_doppler: cfar.train_doppler as usize,
        alpha: 1.0,
        alpha_edge: 1.0,
        min_snr: 10f32.powf(cfar.min_snr_db / 10.0),
        min_gate,
        clutter_half_rows,
    };
    let table = AlphaTable::new(
        shaped.stat,
        shaped.statistic_cells(),
        shaped.edge_cells(),
        cfar.pfa,
        shape.lanes as u32,
    )
    .map_err(|_| PlanError::Threshold)?;
    let alphas = Alphas::of(&table);
    let (alpha, alpha_edge) = alphas.pick(1, 1.0);
    let range_resolution_m = range_step_m.max(LIGHT_SPEED_M_S / bandwidth);
    let doppler_resolution_hz = taper_of(params.window).enbw() / doppler.cpi_s;
    let tracker = params.tracker;
    Ok(Detection {
        alphas,
        cfar: CfarSpec {
            alpha,
            alpha_edge,
            ..shaped
        },
        tracker: TrackerConfig {
            wavelength_m,
            confirm_hits: tracker.confirm_hits,
            confirm_window: tracker.confirm_window,
            coast_looks: tracker.coast_looks,
            max_accel: f64::from(tracker.max_accel_mps2),
            gate: f64::from(tracker.gate),
            jerk: f64::from(tracker.jerk),
            range_resolution_m,
            doppler_resolution_hz,
        },
        range_resolution_m,
        doppler_resolution_hz,
        clutter_half_rows,
        min_gate,
    })
}

fn aoa_plan(
    ctx: &RadarCtx,
    params: &PassiveRadarParams,
    surveillance: &[usize],
) -> Result<Option<AoaPlan>, PlanError> {
    if surveillance.len() < 2 {
        return Ok(None);
    }
    let positions: Option<Vec<[f64; 3]>> = surveillance
        .iter()
        .map(|&element| ctx.positions_m.get(element).copied())
        .collect();
    match positions {
        Some(positions_m) => Ok(Some(AoaPlan {
            mirror_axis_deg: line_axis(&positions_m),
            positions_m,
            grid_step_deg: AOA_GRID_STEP_DEG,
        })),
        None if params.aoa => Err(PlanError::Geometry),
        None => Ok(None),
    }
}

#[must_use]
pub fn line_axis(positions: &[[f64; 3]]) -> Option<f32> {
    if positions.len() < 2 {
        return None;
    }
    let count = positions.len() as f64;
    let mean_x = positions.iter().map(|p| p[0]).sum::<f64>() / count;
    let mean_y = positions.iter().map(|p| p[1]).sum::<f64>() / count;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in positions {
        let (dx, dy) = (p[0] - mean_x, p[1] - mean_y);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (ux, uy) = (angle.cos(), angle.sin());
    let straight = positions
        .iter()
        .all(|p| ((p[0] - mean_x) * uy - (p[1] - mean_y) * ux).abs() <= LINE_TOLERANCE_M);
    straight.then(|| ux.atan2(uy).to_degrees().rem_euclid(180.0) as f32)
}

fn working_bytes(shape: &BatchShape) -> u64 {
    let lanes = shape.lanes as u64;
    let window = shape.window() as u64;
    let batches = shape.batches as u64;
    let fft = shape.fft_len as u64;
    let gates = shape.gates as u64;
    8 * ((lanes + 1) * window * 2 + (lanes + 1) * batches * fft + 2 * lanes * batches * gates)
        + 4 * batches * gates
}

const fn front_latency(cleaning: ReferenceCleaning) -> usize {
    match cleaning {
        ReferenceCleaning::DabRemod => DAB_FRAME_LATENCY,
        ReferenceCleaning::Off | ReferenceCleaning::Cma { .. } => 0,
    }
}
