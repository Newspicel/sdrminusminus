use num_complex::Complex;
use sdrmm_dsp::covariance::{CovarianceBank, CovarianceError};
use sdrmm_dsp::doa::{bartlett, capon, music};
use sdrmm_dsp::linalg::{CMat, Cholesky, Eigen, HermitianEigen};
use sdrmm_dsp::manifold::{GridSpec, LIGHT_SPEED_M_S, Manifold, ManifoldError, SteeringGrid};
use sdrmm_dsp::special::norm_deg;
use sdrmm_wire::array::{MAX_ARRAY_LANES, MIN_ARRAY_LANES};
use sdrmm_wire::processor::spatial::MAX_SPATIAL_PEAKS;
use sdrmm_wire::{
    ProcessorParams, ProcessorReading, SpatialMethod, SpatialPeak, SpatialReading,
    SpatialSpectrumOwned, SpatialSpectrumParams, StreamKind, SurfaceFrame,
};

use crate::ChannelError;
use crate::array_processor::{
    ArrayBlock, ArrayCtx, ArrayProcessor, Execution, LaneFormat, MAX_LANES, ProcessorAction,
    ProcessorDescriptor, ProcessorFaults, ProcessorNeeds, ProcessorOutput, Registration,
    ResetCause, TuningNeed, boxed, check_tuning, stamp_at,
};
use crate::band::{LaneBand, band_rate};
use crate::df::{check_band, manifold_of, same_array};

const PHASE_ERROR_FACTOR: f64 = 90.0;
const CAPON_LOADING: f32 = 1e-3;
const MUSIC_SIGNALS: usize = 1;
const READING_MS: u32 = 1_000;
const MIN_POWER: f32 = 1e-30;
const NANOS_PER_MILLI: u64 = 1_000_000;

static DESCRIPTOR: ProcessorDescriptor = ProcessorDescriptor {
    type_id: "spatial_spectrum",
    name: "Spatial spectrum",
    min_lanes: MIN_ARRAY_LANES,
    max_lanes: MAX_ARRAY_LANES,
    lane_ports: &[],
    steer_port: None,
    surface: Some(StreamKind::SpatialSpectrum),
    needs: |_| ProcessorNeeds::ALL,
    band: |_| None,
    tuning: |_| TuningNeed::Together,
    lane_format,
    execution: |_, ctx| Execution::Worker {
        batch: ctx.max_block,
    },
    in_place,
};

pub const REGISTRATION: Registration = Registration {
    descriptor: &DESCRIPTOR,
    create: Some(boxed::<SpatialSpectrumProcessor>),
};

const fn settings(params: &ProcessorParams) -> Option<&SpatialSpectrumParams> {
    match params {
        ProcessorParams::SpatialSpectrum(spatial) => Some(spatial),
        _ => None,
    }
}

fn in_place(old: &ProcessorParams, new: &ProcessorParams) -> bool {
    let (Some(old), Some(new)) = (settings(old), settings(new)) else {
        return false;
    };
    old.bins == new.bins
        && old.columns == new.columns
        && old.azimuth_step_deg == new.azimuth_step_deg
        && old.offset_hz == new.offset_hz
        && old.bandwidth_hz == new.bandwidth_hz
}

fn bearings_of(spatial: &SpatialSpectrumParams) -> usize {
    (360.0 / spatial.azimuth_step_deg).round() as usize
}

#[must_use]
pub fn surface_cells(spatial: &SpatialSpectrumParams) -> usize {
    bearings_of(spatial) * spatial.columns as usize
}

fn lane_format(params: &ProcessorParams, ctx: &ArrayCtx<'_>, _port: usize) -> LaneFormat {
    let bandwidth = settings(params).and_then(|spatial| spatial.bandwidth_hz);
    let offset = settings(params).map_or(0.0, |spatial| spatial.offset_hz);
    LaneFormat {
        center_hz: ctx.center_hz + bandwidth.map_or(0.0, |_| offset),
        sample_rate: band_rate(ctx.sample_rate, bandwidth),
        capacity: settings(params).map_or(0, surface_cells),
    }
}

#[must_use]
pub fn frequency_groups(span_hz: f64, aperture_m: f64, columns: usize) -> usize {
    let groups = (PHASE_ERROR_FACTOR * span_hz * aperture_m / LIGHT_SPEED_M_S).ceil();
    if groups.is_finite() {
        (groups.max(1.0) as usize).min(columns.max(1))
    } else {
        columns.max(1)
    }
}

fn column_offset_hz(column: usize, bins: usize, columns: usize, rate: f64) -> f64 {
    let per = (bins / columns.max(1)) as f64;
    let mean_bin = column as f64 * per + (per - 1.0) / 2.0;
    (mean_bin - bins as f64 / 2.0) * rate / bins as f64
}

fn group_columns(group: usize, groups: usize, columns: usize) -> (usize, usize) {
    let first = (group * columns).div_ceil(groups);
    let end = ((group + 1) * columns).div_ceil(groups);
    (first, end.max(first + 1) - 1)
}

const fn refusal(error: ManifoldError) -> ChannelError {
    ChannelError::Refused(match error {
        ManifoldError::Frequency => "Frequency out of range",
        ManifoldError::GridTooLarge(_) | ManifoldError::GridStep => "Step out of range",
        _ => "Needs array geometry",
    })
}

const fn bank_refusal(error: CovarianceError) -> ChannelError {
    ChannelError::Refused(match error {
        CovarianceError::FftSize(_) | CovarianceError::Hop(_) => "Bins out of range",
        _ => "Too many elements",
    })
}

fn decibels(power: f32) -> f32 {
    10.0 * power.max(MIN_POWER).log10()
}

fn samples_in(rate: f64, ms: u32) -> u64 {
    ((rate * f64::from(ms) / 1_000.0).round() as u64).max(1)
}

fn frame_decay(bins: usize, band_rate: f64, average_ms: u32) -> f32 {
    let frames = band_rate * f64::from(average_ms) / 1_000.0 / bins as f64;
    (-1.0 / frames.max(f64::MIN_POSITIVE)).exp() as f32
}

fn refine(ring: &[f32], peak: usize) -> f64 {
    let count = ring.len();
    if count < 3 {
        return peak as f64;
    }
    let left = ring[(peak + count - 1) % count];
    let centre = ring[peak];
    let right = ring[(peak + 1) % count];
    let bend = left - 2.0 * centre + right;
    let shift = if bend < 0.0 {
        (0.5 * (left - right) / bend).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    peak as f64 + f64::from(shift)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    method: SpatialMethod,
    span_db: f32,
    average_ms: u32,
    report_ms: u32,
}

impl Settings {
    const fn of(spatial: &SpatialSpectrumParams) -> Self {
        Self {
            method: spatial.method,
            span_db: spatial.span_db,
            average_ms: spatial.average_ms,
            report_ms: spatial.report_ms,
        }
    }
}

struct Solvers {
    matrix: CMat,
    chol: Cholesky,
    solver: HermitianEigen,
    eigen: Eigen,
    scratch: [Complex<f32>; MAX_LANES],
}

impl Solvers {
    fn new(lanes: usize) -> Result<Self, ChannelError> {
        let too_many = |_| ChannelError::Refused("Too many elements");
        Ok(Self {
            matrix: CMat::identity(lanes).map_err(too_many)?,
            chol: Cholesky::new(lanes).map_err(too_many)?,
            solver: HermitianEigen::new(lanes).map_err(too_many)?,
            eigen: Eigen::new(),
            scratch: [Complex::new(0.0, 0.0); MAX_LANES],
        })
    }

    fn ring(&mut self, method: SpatialMethod, ring: &SteeringGrid, out: &mut [f32]) -> bool {
        let prepared = match method {
            SpatialMethod::Bartlett => true,
            SpatialMethod::Capon => self.chol.factor_loaded(&self.matrix, CAPON_LOADING).is_ok(),
            SpatialMethod::Music => self.solver.solve(&self.matrix, &mut self.eigen).is_ok(),
        };
        if !prepared {
            out.fill(MIN_POWER);
            return false;
        }
        for (point, value) in out.iter_mut().enumerate().take(ring.points()) {
            let a = ring.vector(point);
            *value = match method {
                SpatialMethod::Bartlett => bartlett(&self.matrix, a),
                SpatialMethod::Capon => capon(&self.chol, a, &mut self.scratch),
                SpatialMethod::Music => music(&self.eigen, MUSIC_SIGNALS, a),
            };
        }
        true
    }
}

pub struct SpatialSpectrumProcessor {
    lanes: usize,
    rate: f64,
    center_hz: f64,
    offset_hz: f64,
    bandwidth_hz: Option<f64>,
    settings: Settings,
    manifold: Manifold,
    band: LaneBand,
    bank: CovarianceBank,
    columns: usize,
    bearings: usize,
    step_deg: f64,
    rings: Vec<SteeringGrid>,
    solvers: Solvers,
    ring: Vec<f32>,
    levels: Vec<f32>,
    column_power: Vec<f32>,
    column_bearing: Vec<f32>,
    peaks: [SpatialPeak; MAX_SPATIAL_PEAKS],
    peak_count: usize,
    db_min: f32,
    db_max: f32,
    decay: f32,
    since_report: u64,
    report_samples: u64,
    since_reading: u64,
    reading_samples: u64,
    frames: u64,
    dropped_frames: u64,
    seq: u32,
    heading_deg: Option<f64>,
    unix_ns: u64,
    faults: ProcessorFaults,
}

impl SpatialSpectrumProcessor {
    #[must_use]
    pub fn groups(&self) -> usize {
        self.rings.len()
    }

    fn span_hz(&self) -> f64 {
        self.band.output_rate()
    }

    fn surface_center_hz(&self) -> f64 {
        self.center_hz + self.band.center_offset_hz()
    }

    fn group_freq_hz(&self, group: usize) -> f64 {
        let (first, last) = group_columns(group, self.rings.len(), self.columns);
        let bins = self.bank.bins();
        let low = column_offset_hz(first, bins, self.columns, self.span_hz());
        let high = column_offset_hz(last, bins, self.columns, self.span_hz());
        self.surface_center_hz() + (low + high) / 2.0
    }

    fn column_freq_hz(&self, column: usize) -> f64 {
        self.surface_center_hz()
            + column_offset_hz(column, self.bank.bins(), self.columns, self.span_hz())
    }

    fn rebuild_rings(&mut self) -> Result<(), ChannelError> {
        for group in 0..self.rings.len() {
            let freq_hz = self.group_freq_hz(group);
            self.rings[group]
                .rebuild(&self.manifold, freq_hz)
                .map_err(refusal)?;
        }
        Ok(())
    }

    fn clear(&mut self) {
        self.bank.reset();
        self.band.reset();
        self.since_report = 0;
        self.peak_count = 0;
    }

    fn accumulate(&mut self, block: &ArrayBlock<'_>) {
        let mut views: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let len = self.band.process(block, &mut views);
        for view in &mut views[..self.lanes] {
            *view = &view[..len];
        }
        if self.bank.push(&views[..self.lanes], self.decay).is_err() {
            self.faults.lane_mismatch += 1;
        }
    }

    fn analyse(&mut self) {
        let per = self.bank.bins() / self.columns;
        let groups = self.rings.len();
        let mut db_max = f32::NEG_INFINITY;
        for column in 0..self.columns {
            let power = match self
                .bank
                .group_matrix(column * per, per, &mut self.solvers.matrix)
            {
                Ok(true) => self.solvers.matrix.trace_re() / self.lanes as f32,
                Ok(false) | Err(_) => 0.0,
            };
            self.column_power[column] = power;
            let ring = &self.rings[column * groups / self.columns];
            if !self
                .solvers
                .ring(self.settings.method, ring, &mut self.ring)
            {
                self.faults.solver_failures += 1;
            }
            let mut best = 0;
            for bearing in 0..self.ring.len() {
                let db = decibels(self.ring[bearing]);
                self.ring[bearing] = db;
                self.levels[bearing * self.columns + column] = db;
                if db > self.ring[best] {
                    best = bearing;
                }
            }
            db_max = db_max.max(self.ring[best]);
            self.column_bearing[column] = norm_deg(refine(&self.ring, best) * self.step_deg) as f32;
        }
        self.db_max = if db_max.is_finite() { db_max } else { 0.0 };
        self.db_min = self.db_max - self.settings.span_db;
        self.rank_peaks();
    }

    fn rank_peaks(&mut self) {
        let mut chosen = [usize::MAX; MAX_SPATIAL_PEAKS];
        let mut count = 0;
        for column in 0..self.columns {
            let power = self.column_power[column];
            if power <= 0.0 {
                continue;
            }
            let at = chosen[..count]
                .iter()
                .position(|&kept| power > self.column_power[kept])
                .unwrap_or(count);
            if at < MAX_SPATIAL_PEAKS {
                let last = count.min(MAX_SPATIAL_PEAKS - 1);
                chosen.copy_within(at..last, at + 1);
                chosen[at] = column;
                count = (count + 1).min(MAX_SPATIAL_PEAKS);
            }
        }
        for (slot, &column) in chosen[..count].iter().enumerate() {
            let bearing = f64::from(self.column_bearing[column]);
            self.peaks[slot] = SpatialPeak {
                freq_hz: self.column_freq_hz(column),
                bearing_deg: bearing as f32,
                db: decibels(self.column_power[column]),
                true_deg: self
                    .heading_deg
                    .map(|heading| norm_deg(bearing + heading) as f32),
            };
        }
        self.peak_count = count;
    }

    fn write_surface(&mut self, out: &mut ProcessorOutput<'_>) {
        let cells = self.levels.len();
        let Some(slot) = out.surface() else {
            self.dropped_frames += 1;
            return;
        };
        if !matches!(slot, SurfaceFrame::SpatialSpectrum(_)) {
            *slot = SurfaceFrame::SpatialSpectrum(SpatialSpectrumOwned {
                cells: Vec::with_capacity(cells),
                ..SpatialSpectrumOwned::default()
            });
        }
        let SurfaceFrame::SpatialSpectrum(frame) = slot else {
            return;
        };
        if frame.cells.capacity() < cells {
            self.faults.truncated += 1;
            self.dropped_frames += 1;
            return;
        }
        self.fill_frame(frame);
        out.publish_surface();
        self.frames += 1;
    }

    fn fill_frame(&mut self, frame: &mut SpatialSpectrumOwned) {
        self.seq = self.seq.wrapping_add(1);
        frame.seq = self.seq;
        frame.timestamp = self.unix_ns / NANOS_PER_MILLI;
        frame.center_hz = self.surface_center_hz();
        frame.span_hz = self.span_hz() as f32;
        frame.bearings = u16::try_from(self.bearings).unwrap_or(u16::MAX);
        frame.bins = u16::try_from(self.columns).unwrap_or(u16::MAX);
        frame.db_min = self.db_min;
        frame.db_max = self.db_max;
        frame.cells.clear();
        let span = (self.db_max - self.db_min).max(f32::MIN_POSITIVE);
        let floor = self.db_min;
        frame.cells.extend(self.levels.iter().map(|&db| {
            let level = (255.0 * (db - floor) / span).round();
            if level.is_nan() {
                0
            } else {
                level.clamp(0.0, 255.0) as u8
            }
        }));
    }

    fn write_reading(&mut self, out: &mut ProcessorOutput<'_>) {
        let Some(slot) = out.report() else {
            return;
        };
        if !matches!(slot, ProcessorReading::SpatialSpectrum(_)) {
            *slot = ProcessorReading::SpatialSpectrum(SpatialReading::reserved());
        }
        if let ProcessorReading::SpatialSpectrum(reading) = slot {
            stamp_at(&mut reading.at, self.unix_ns);
            reading.peaks.clear();
            for peak in &self.peaks[..self.peak_count] {
                self.faults.push_capped(&mut reading.peaks, *peak);
            }
            reading.azimuth_deg = self.heading_deg;
            reading.frames = self.frames;
            reading.dropped_frames = self.dropped_frames;
        }
        out.publish_report();
    }
}

impl ArrayProcessor for SpatialSpectrumProcessor {
    fn descriptor() -> &'static ProcessorDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: &ArrayCtx<'_>, params: &ProcessorParams) -> Result<Self, ChannelError> {
        let spatial = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        check_tuning(TuningNeed::Together, ctx)?;
        check_band(ctx.sample_rate, spatial.offset_hz, spatial.bandwidth_hz)?;
        let manifold = manifold_of(ctx)?;
        let band = LaneBand::new(
            ctx.lanes,
            ctx.sample_rate,
            spatial.offset_hz,
            spatial.bandwidth_hz,
            ctx.max_block,
        )?;
        let bins = spatial.bins as usize;
        let columns = spatial.columns as usize;
        let bearings = bearings_of(spatial);
        let bank = CovarianceBank::new(ctx.lanes, bins, bins).map_err(bank_refusal)?;
        let span_hz = band.output_rate();
        let groups = frequency_groups(span_hz, manifold.geometry().aperture_m(), columns);
        let spec = GridSpec::ring(spatial.azimuth_step_deg);
        let center_hz = ctx.center_hz + band.center_offset_hz();
        let rings = (0..groups)
            .map(|_| SteeringGrid::new(&manifold, spec, center_hz).map_err(refusal))
            .collect::<Result<Vec<_>, _>>()?;
        let mut processor = Self {
            lanes: ctx.lanes,
            rate: ctx.sample_rate,
            center_hz: ctx.center_hz,
            offset_hz: spatial.offset_hz,
            bandwidth_hz: spatial.bandwidth_hz,
            settings: Settings::of(spatial),
            manifold,
            decay: frame_decay(bins, span_hz, spatial.average_ms),
            band,
            bank,
            columns,
            bearings,
            step_deg: spatial.azimuth_step_deg,
            rings,
            solvers: Solvers::new(ctx.lanes)?,
            ring: vec![0.0; bearings],
            levels: vec![0.0; bearings * columns],
            column_power: vec![0.0; columns],
            column_bearing: vec![0.0; columns],
            peaks: [SpatialPeak::default(); MAX_SPATIAL_PEAKS],
            peak_count: 0,
            db_min: 0.0,
            db_max: 0.0,
            since_report: 0,
            report_samples: samples_in(ctx.sample_rate, spatial.report_ms),
            reading_samples: samples_in(ctx.sample_rate, READING_MS),
            since_reading: samples_in(ctx.sample_rate, READING_MS),
            frames: 0,
            dropped_frames: 0,
            seq: 0,
            heading_deg: None,
            unix_ns: 0,
            faults: ProcessorFaults::default(),
        };
        processor.rebuild_rings()?;
        Ok(processor)
    }

    fn apply(&mut self, params: &ProcessorParams) -> Result<(), ChannelError> {
        let spatial = settings(params).ok_or(ChannelError::Refused("Wrong settings"))?;
        if let Some(problem) = spatial.problem() {
            return Err(ChannelError::Refused(problem));
        }
        let same_layout = spatial.bins as usize == self.bank.bins()
            && spatial.columns as usize == self.columns
            && spatial.azimuth_step_deg == self.step_deg
            && spatial.offset_hz == self.offset_hz
            && spatial.bandwidth_hz == self.bandwidth_hz;
        if !same_layout {
            return Err(ChannelError::Refused("Needs a rebuild"));
        }
        self.settings = Settings::of(spatial);
        self.decay = frame_decay(self.bank.bins(), self.span_hz(), spatial.average_ms);
        self.report_samples = samples_in(self.rate, spatial.report_ms);
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
        self.center_hz = ctx.center_hz;
        self.rebuild_rings()?;
        self.clear();
        Ok(())
    }

    fn reset(&mut self, _cause: ResetCause) {
        self.clear();
        self.faults.resets += 1;
    }

    fn action(&mut self, _action: ProcessorAction) -> Result<(), ChannelError> {
        Err(ChannelError::Refused("No tracks here"))
    }

    fn process(&mut self, block: &ArrayBlock<'_>, out: &mut ProcessorOutput<'_>) {
        if !self.faults.lanes_match(block, self.lanes) {
            return;
        }
        self.heading_deg = block.pose.heading_deg;
        self.unix_ns = block.unix_ns;
        self.accumulate(block);
        let samples = block.len() as u64;
        self.since_report += samples;
        self.since_reading = self.since_reading.saturating_add(samples);
        if self.since_report < self.report_samples {
            return;
        }
        self.since_report %= self.report_samples;
        if self.bank.weight() <= 0.0 {
            return;
        }
        self.analyse();
        self.write_surface(out);
        if self.since_reading >= self.reading_samples {
            self.since_reading = 0;
            self.write_reading(out);
        }
    }

    fn faults(&self) -> ProcessorFaults {
        self.faults
    }
}

#[cfg(test)]
mod tests;
