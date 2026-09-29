use std::fmt::Write as _;

use num_complex::Complex;
use sdrmm_dsp::manifold::{Geometry, ManifoldError, ManifoldTable, Vec3, Winding as DspWinding};
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::{
    ArrayGeometry, ArrayTuningMode, CalPhase, CalSourceKind, Coherence, DecoderEvent,
    ProcessorParams, ProcessorReading, StreamKind, SurfaceFrame, Winding,
};

use crate::ChannelError;

#[cfg(feature = "probe")]
pub mod probe;

#[cfg(test)]
pub(crate) mod bench;
#[cfg(test)]
mod tests;

pub const MAX_LANES: usize = MAX_ARRAY_LANES as usize;
pub const MAX_LANE_PORTS: usize = 2;
pub const MAX_EVENTS_PER_BLOCK: usize = 8;
pub const TUNING_TOLERANCE_HZ: f64 = 1.0;
pub const INLINE_INPUT_LIMIT: f64 = 2e6;

const ONE: Complex<f32> = Complex::new(1.0, 0.0);
const NANOS_PER_MILLI: u64 = 1_000_000;
const MILLIS_PER_DAY: u64 = 86_400_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessorNeeds {
    pub time: bool,
    pub phase: bool,
    pub gain: bool,
    pub geometry: bool,
}

impl ProcessorNeeds {
    pub const TIME: Self = Self {
        time: true,
        phase: false,
        gain: false,
        geometry: false,
    };
    pub const CALIBRATED: Self = Self {
        time: true,
        phase: true,
        gain: true,
        geometry: false,
    };
    pub const ALL: Self = Self {
        geometry: true,
        ..Self::CALIBRATED
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuningNeed {
    Together,
    Spread,
    Any,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Execution {
    Inline,
    Worker { batch: usize },
    Dedicated,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneFormat {
    pub center_hz: f64,
    pub sample_rate: f64,
    pub capacity: usize,
}

pub struct ProcessorDescriptor {
    pub type_id: &'static str,
    pub name: &'static str,
    pub min_lanes: u32,
    pub max_lanes: u32,
    pub lane_ports: &'static [&'static str],
    pub steer_port: Option<&'static str>,
    pub surface: Option<StreamKind>,
    pub needs: fn(&ProcessorParams) -> ProcessorNeeds,
    pub band: fn(&ProcessorParams) -> Option<(f64, f64)>,
    pub tuning: fn(&ProcessorParams) -> TuningNeed,
    pub lane_format: fn(&ProcessorParams, &ArrayCtx<'_>, usize) -> LaneFormat,
    pub execution: fn(&ProcessorParams, &ArrayCtx<'_>) -> Execution,
    pub in_place: fn(&ProcessorParams, &ProcessorParams) -> bool,
}

pub struct ArrayCtx<'a> {
    pub node: &'a str,
    pub lanes: usize,
    pub sample_rate: f64,
    pub center_hz: f64,
    pub lane_centers_hz: &'a [f64],
    pub geometry: &'a ArrayGeometry,
    pub positions_m: &'a [[f64; 3]],
    pub manifold: Option<&'a ManifoldTable>,
    pub tier: Coherence,
    pub tuning: ArrayTuningMode,
    pub max_block: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GeoFix {
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: Option<f64>,
    pub accuracy_m: Option<f32>,
    pub speed_mps: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub heading_deg: Option<f64>,
    pub heading_sigma_deg: f32,
    pub yaw_rate_dps: Option<f32>,
    pub fix: Option<GeoFix>,
    pub moving: bool,
    pub follows: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CalView {
    pub phase: CalPhase,
    pub source: Option<CalSourceKind>,
    pub phase_ready: bool,
    pub gain_ready: bool,
    pub generation: u32,
    pub phase_sigma_deg: f32,
    pub gain_sigma_db: f32,
    pub valid_hz: Option<(f64, f64)>,
}

#[derive(Clone, Copy, Debug)]
pub struct CorrectionView<'a> {
    pub generation: u32,
    sample_rate: f64,
    spectra: &'a [Vec<Complex<f32>>],
}

impl<'a> CorrectionView<'a> {
    #[must_use]
    pub const fn new(generation: u32, sample_rate: f64, spectra: &'a [Vec<Complex<f32>>]) -> Self {
        Self {
            generation,
            sample_rate,
            spectra,
        }
    }

    #[must_use]
    pub const fn identity() -> CorrectionView<'static> {
        CorrectionView {
            generation: 0,
            sample_rate: 0.0,
            spectra: &[],
        }
    }

    #[must_use]
    pub fn response(&self, lane: usize, offset_hz: f64) -> Complex<f32> {
        let (Some(spectrum), Some(reference)) = (self.spectra.get(lane), self.spectra.first())
        else {
            return ONE;
        };
        let bins = spectrum.len();
        if bins == 0 || reference.len() != bins || !usable_rate(self.sample_rate) {
            return ONE;
        }
        let position = (offset_hz / self.sample_rate * bins as f64).rem_euclid(bins as f64);
        let low = (position.floor() as usize).min(bins - 1);
        let high = (low + 1) % bins;
        let fraction = (position - low as f64) as f32;
        let below = relative(spectrum[low], reference[low]);
        let above = relative(spectrum[high], reference[high]);
        below + (above - below) * fraction
    }
}

fn usable_rate(rate: f64) -> bool {
    rate.is_finite() && rate > 0.0
}

fn relative(lane: Complex<f32>, reference: Complex<f32>) -> Complex<f32> {
    let norm = reference.norm();
    if norm > f32::MIN_POSITIVE {
        lane * reference.conj() / norm
    } else {
        lane
    }
}

pub struct ArrayBlock<'a> {
    pub lanes: &'a [&'a [Complex<f32>]],
    pub corrected: bool,
    pub correction: CorrectionView<'a>,
    pub first_index: u64,
    pub unix_ns: u64,
    pub generation: u32,
    pub gap_before: bool,
    pub centers_hz: &'a [f64],
    pub cal: CalView,
    pub pose: Pose,
}

impl ArrayBlock<'_> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.lanes.iter().map(|lane| lane.len()).min().unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetCause {
    Realigned,
    Retuned,
    Calibrated,
    Gap,
    Resumed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Steer {
    pub same_array: bool,
    pub relative_deg: f64,
    pub true_deg: Option<f64>,
    pub elevation_deg: f64,
    pub sigma_deg: f32,
    pub others_relative_deg: [f64; 3],
    pub others_true_deg: [Option<f64>; 3],
    pub others: u8,
    pub wall_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessorAction {
    ClearTracks,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessorFaults {
    pub lane_mismatch: u64,
    pub dropped_blocks: u64,
    pub solver_failures: u64,
    pub resets: u64,
    pub truncated: u64,
}

impl ProcessorFaults {
    pub fn lanes_match(&mut self, block: &ArrayBlock<'_>, lanes: usize) -> bool {
        let matches = block.lanes.len() == lanes;
        if !matches {
            self.lane_mismatch += 1;
        }
        matches
    }

    pub fn push_capped<T>(&mut self, list: &mut Vec<T>, item: T) {
        if list.len() < list.capacity() {
            list.push(item);
        } else {
            self.truncated += 1;
        }
    }
}

pub struct LaneBuffer {
    samples: Vec<Complex<f32>>,
    capacity: usize,
    skipped: u64,
    overflowed: u64,
}

impl LaneBuffer {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
            capacity,
            skipped: 0,
            overflowed: 0,
        }
    }

    #[must_use]
    pub fn samples(&self) -> &[Complex<f32>] {
        &self.samples
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub const fn skipped(&self) -> u64 {
        self.skipped
    }

    #[must_use]
    pub const fn overflowed(&self) -> u64 {
        self.overflowed
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.skipped = 0;
        self.overflowed = 0;
    }
}

pub struct LaneWriter<'a> {
    buffer: &'a mut Vec<Complex<f32>>,
    capacity: usize,
    overflowed: &'a mut u64,
}

impl LaneWriter<'_> {
    pub fn extend(&mut self, samples: &[Complex<f32>]) {
        let take = samples.len().min(self.room());
        self.buffer.extend_from_slice(&samples[..take]);
        *self.overflowed += (samples.len() - take) as u64;
    }

    pub fn push(&mut self, sample: Complex<f32>) {
        if self.room() > 0 {
            self.buffer.push(sample);
        } else {
            *self.overflowed += 1;
        }
    }

    #[must_use]
    pub fn room(&self) -> usize {
        self.capacity
            .min(self.buffer.capacity())
            .saturating_sub(self.buffer.len())
    }
}

pub struct OutputSlots<'a> {
    pub report: Option<&'a mut ProcessorReading>,
    pub surface: Option<&'a mut SurfaceFrame>,
    pub events: &'a mut [DecoderEvent],
    pub lanes: &'a mut [LaneBuffer],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OutputTally {
    pub report: bool,
    pub surface: bool,
    pub events: usize,
    pub steer: Option<Steer>,
    pub dropped_reports: u64,
    pub dropped_events: u64,
}

pub struct ProcessorOutput<'a> {
    slots: OutputSlots<'a>,
    tally: OutputTally,
}

impl<'a> ProcessorOutput<'a> {
    #[must_use]
    pub fn new(slots: OutputSlots<'a>) -> Self {
        Self {
            slots,
            tally: OutputTally::default(),
        }
    }

    #[must_use]
    pub const fn tally(&self) -> OutputTally {
        self.tally
    }

    pub fn report(&mut self) -> Option<&mut ProcessorReading> {
        match self.slots.report.as_deref_mut() {
            Some(slot) if !self.tally.report => Some(slot),
            _ => {
                self.tally.dropped_reports += 1;
                None
            }
        }
    }

    pub fn publish_report(&mut self) {
        self.tally.report |= self.slots.report.is_some();
    }

    pub fn surface(&mut self) -> Option<&mut SurfaceFrame> {
        match self.slots.surface.as_deref_mut() {
            Some(slot) if !self.tally.surface => Some(slot),
            _ => {
                self.tally.dropped_reports += 1;
                None
            }
        }
    }

    pub fn publish_surface(&mut self) {
        self.tally.surface |= self.slots.surface.is_some();
    }

    pub fn event(&mut self) -> Option<&mut DecoderEvent> {
        let limit = self.slots.events.len().min(MAX_EVENTS_PER_BLOCK);
        if self.tally.events < limit {
            self.slots.events.get_mut(self.tally.events)
        } else {
            self.tally.dropped_events += 1;
            None
        }
    }

    pub fn publish_event(&mut self) {
        let limit = self.slots.events.len().min(MAX_EVENTS_PER_BLOCK);
        if self.tally.events < limit {
            self.tally.events += 1;
        }
    }

    pub fn steer(&mut self, steer: Steer) {
        self.tally.steer = Some(steer);
    }

    pub fn lane(&mut self, port: usize) -> Option<LaneWriter<'_>> {
        self.slots.lanes.get_mut(port).map(|lane| LaneWriter {
            buffer: &mut lane.samples,
            capacity: lane.capacity,
            overflowed: &mut lane.overflowed,
        })
    }

    pub fn skip_lane(&mut self, port: usize, samples: u64) {
        if let Some(lane) = self.slots.lanes.get_mut(port) {
            lane.skipped += samples;
        }
    }
}

pub trait ArrayProcessor: Send {
    fn descriptor() -> &'static ProcessorDescriptor
    where
        Self: Sized;
    fn new(ctx: &ArrayCtx<'_>, params: &ProcessorParams) -> Result<Self, ChannelError>
    where
        Self: Sized;
    fn apply(&mut self, params: &ProcessorParams) -> Result<(), ChannelError>;
    fn retune(&mut self, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError>;
    fn reset(&mut self, cause: ResetCause);
    fn steer(&mut self, _steer: &Steer) {}
    fn action(&mut self, action: ProcessorAction) -> Result<(), ChannelError>;
    fn process(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>);
    fn poll(&mut self, _out: &mut ProcessorOutput<'_>) {}
    fn faults(&self) -> ProcessorFaults;
}

pub type CreateFn =
    fn(&ArrayCtx<'_>, &ProcessorParams) -> Result<Box<dyn ArrayProcessor>, ChannelError>;

#[derive(Clone, Copy)]
pub struct Registration {
    pub descriptor: &'static ProcessorDescriptor,
    pub create: Option<CreateFn>,
}

macro_rules! registry {
    ($($registration:path),* $(,)?) => {
        #[cfg(not(feature = "probe"))]
        static REGISTRY: &[Registration] = &[$($registration),*];
        #[cfg(feature = "probe")]
        static REGISTRY: &[Registration] = &[$($registration,)* probe::REGISTRATION];
    };
}

registry![
    crate::df::REGISTRATION,
    crate::beamformer::REGISTRATION,
    crate::spatial_spectrum::REGISTRATION,
    crate::stitch::REGISTRATION,
    crate::correlator::REGISTRATION,
    crate::polarimeter::REGISTRATION,
    crate::passive_radar::REGISTRATION,
];

#[must_use]
pub fn registrations() -> &'static [Registration] {
    REGISTRY
}

fn registration(type_id: &str) -> Option<&'static Registration> {
    REGISTRY
        .iter()
        .find(|entry| entry.descriptor.type_id == type_id)
}

#[must_use]
pub fn processor_descriptor(type_id: &str) -> Option<&'static ProcessorDescriptor> {
    registration(type_id).map(|entry| entry.descriptor)
}

pub fn create_processor(
    ctx: &ArrayCtx<'_>,
    params: &ProcessorParams,
) -> Result<Box<dyn ArrayProcessor>, ChannelError> {
    let type_id = params.type_id();
    let entry =
        registration(type_id).ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
    let descriptor = entry.descriptor;
    let lanes = u32::try_from(ctx.lanes).unwrap_or(u32::MAX);
    if lanes < descriptor.min_lanes {
        return Err(ChannelError::Refused("Too few elements"));
    }
    if lanes > descriptor.max_lanes {
        return Err(ChannelError::Refused("Too many elements"));
    }
    if let Some(problem) = params.problem() {
        return Err(ChannelError::Refused(problem));
    }
    if (descriptor.execution)(params, ctx) == Execution::Dedicated {
        return Err(ChannelError::Unsupported(format!(
            "{} is built by the engine",
            descriptor.type_id
        )));
    }
    let create = entry.create.ok_or_else(|| {
        ChannelError::Unsupported(format!("{} is not built yet", descriptor.name))
    })?;
    create(ctx, params)
}

pub fn boxed<P: ArrayProcessor + 'static>(
    ctx: &ArrayCtx<'_>,
    params: &ProcessorParams,
) -> Result<Box<dyn ArrayProcessor>, ChannelError> {
    Ok(Box::new(P::new(ctx, params)?))
}

pub fn geometry_of(geometry: &ArrayGeometry, lanes: usize) -> Result<Geometry, ChannelError> {
    let built = match geometry {
        ArrayGeometry::Uca {
            radius_m,
            first_deg,
            winding,
        } => Geometry::uca(*radius_m, lanes, *first_deg, dsp_winding(*winding)),
        ArrayGeometry::Ula {
            spacing_m,
            axis_deg,
        } => Geometry::ula(*spacing_m, lanes, *axis_deg),
        ArrayGeometry::Explicit { positions } => {
            if positions.len() != lanes || lanes > MAX_LANES {
                return Err(ChannelError::Refused("Needs array geometry"));
            }
            let mut points = [Vec3::default(); MAX_LANES];
            for (point, element) in points.iter_mut().zip(positions) {
                *point = Vec3::new(element.x_m, element.y_m, element.z_m);
            }
            Geometry::explicit(&points[..lanes])
        }
    };
    built.map_err(|error| ChannelError::Refused(geometry_problem(error)))
}

const fn geometry_problem(error: ManifoldError) -> &'static str {
    match error {
        ManifoldError::Count(count) if count < MIN_ARRAY_LANES as usize => "Too few elements",
        ManifoldError::Count(_) => "Too many elements",
        _ => "Needs array geometry",
    }
}

const fn dsp_winding(winding: Winding) -> DspWinding {
    match winding {
        Winding::Clockwise => DspWinding::Clockwise,
        Winding::CounterClockwise => DspWinding::Counterclockwise,
    }
}

#[must_use]
pub fn lanes_spread(ctx: &ArrayCtx<'_>) -> bool {
    ctx.lane_centers_hz
        .iter()
        .any(|hz| (hz - ctx.center_hz).abs() > TUNING_TOLERANCE_HZ)
}

pub fn check_tuning(need: TuningNeed, ctx: &ArrayCtx<'_>) -> Result<(), ChannelError> {
    match need {
        TuningNeed::Together if lanes_spread(ctx) => {
            Err(ChannelError::Refused("Needs lanes tuned together"))
        }
        TuningNeed::Spread if !lanes_spread(ctx) => {
            Err(ChannelError::Refused("Needs spread tuning"))
        }
        _ => Ok(()),
    }
}

#[must_use]
pub fn no_lane_format(_: &ProcessorParams, ctx: &ArrayCtx<'_>, _: usize) -> LaneFormat {
    LaneFormat {
        center_hz: ctx.center_hz,
        sample_rate: ctx.sample_rate,
        capacity: 0,
    }
}

#[must_use]
pub fn banded_execution(ctx: &ArrayCtx<'_>) -> Execution {
    if ctx.lanes as f64 * ctx.sample_rate <= INLINE_INPUT_LIMIT {
        Execution::Inline
    } else {
        Execution::Worker {
            batch: ctx.max_block,
        }
    }
}

#[must_use]
pub const fn never_in_place(_: &ProcessorParams, _: &ProcessorParams) -> bool {
    false
}

pub fn stamp_at(at: &mut String, unix_ns: u64) {
    let millis = unix_ns / NANOS_PER_MILLI;
    let (year, month, day) = civil_from_days(millis / MILLIS_PER_DAY);
    let of_day = millis % MILLIS_PER_DAY;
    at.clear();
    let written = write!(
        at,
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1_000 % 60,
        of_day % 1_000,
    );
    if written.is_err() {
        at.clear();
    }
}

const fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}
