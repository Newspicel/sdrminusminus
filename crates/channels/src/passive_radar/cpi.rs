use std::f32::consts::LN_2;
use std::ops::Range;
use std::time::Instant;

use num_complex::Complex;
use sdrmm_dsp::manifold::Vec3;
use sdrmm_dsp::radar::aoa::Beamscan;
use sdrmm_dsp::radar::batch::{BatchShape, MAX_SURVEILLANCE};
use sdrmm_dsp::radar::cfar::{
    CORRELATION_LAGS, Cfar, CfarSpec, Hit, LaneCoherence, range_correlation,
};
use sdrmm_dsp::radar::cluster::{Cluster, Clusterer, MAX_SNAPSHOTS};
use sdrmm_dsp::radar::track::{MAX_MEASUREMENTS, Measurement, TrackAoa, TrackView, Tracker};
use sdrmm_wire::radar::{
    AoaState, MAX_RADAR_DETECTIONS, MAX_RADAR_TRACKS, RADAR_TRACK_EVENT_REPEAT_S, RadarAxes,
    TrackChange,
};

use super::assemble::CpiJob;
use super::caf::{CafBackend, CafError, CubeOut};
use super::plan::{LiveParams, RadarPlan};
use super::report::{
    AoaPod, DetectionPod, RadarCounters, RadarPod, TrackEventPod, TrackPod, axes_of,
};
use crate::ChannelError;

type C32 = Complex<f32>;

const NOISE_CELLS: usize = 65_536;
const HIT_CAPACITY: usize = 4 * MAX_RADAR_DETECTIONS;
const MIN_RIDGE_ROWS: usize = 3;
const CORRELATION_RUN: usize = 128;
const NANOS_PER_SECOND: f64 = 1e9;
const EVENT_REPEAT_NS: u64 = RADAR_TRACK_EVENT_REPEAT_S * 1_000_000_000;
const POWER_FLOOR: f32 = 1e-30;

pub struct CpiOutcome {
    pub compute_ns: u64,
    pub failed: Option<CafError>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Clock {
    id: u32,
    last_ns: u64,
    range_m: f32,
    range_rate_mps: f32,
    doppler_hz: f32,
    snr_db: f32,
}

struct Clocks {
    entries: [Clock; MAX_RADAR_TRACKS],
    len: usize,
}

impl Clocks {
    const fn new() -> Self {
        Self {
            entries: [Clock {
                id: 0,
                last_ns: 0,
                range_m: 0.0,
                range_rate_mps: 0.0,
                doppler_hz: 0.0,
                snr_db: 0.0,
            }; MAX_RADAR_TRACKS],
            len: 0,
        }
    }

    fn position(&self, id: u32) -> Option<usize> {
        self.entries[..self.len]
            .iter()
            .position(|clock| clock.id == id)
    }

    fn remove(&mut self, id: u32) -> Option<Clock> {
        let at = self.position(id)?;
        let clock = self.entries[at];
        self.entries.copy_within(at + 1..self.len, at);
        self.len -= 1;
        Some(clock)
    }

    const fn clear(&mut self) {
        self.len = 0;
    }

    fn insert(&mut self, clock: Clock) -> bool {
        match self.entries.get_mut(self.len) {
            Some(slot) => {
                *slot = clock;
                self.len += 1;
                true
            }
            None => false,
        }
    }
}

struct Geometry {
    shape: BatchShape,
    report_rows: Range<usize>,
    range_step_m: f64,
    cpi_s: f64,
    radar_rate: f64,
}

impl Geometry {
    fn cell(&self, lane: usize, gate: usize, row: usize) -> usize {
        (lane * self.shape.gates + gate) * self.shape.batches + row
    }

    fn doppler_of_row(&self, row: f32) -> f32 {
        ((f64::from(row) - (self.shape.batches / 2) as f64) / self.cpi_s) as f32
    }
}

struct Detector {
    cfar: Cfar,
    hits: Vec<Hit>,
    clusterer: Clusterer,
    clusters: Vec<Cluster>,
    beamscan: Option<Beamscan>,
    mirror_axis_deg: Option<f32>,
}

struct Tracking {
    tracker: Tracker,
    measurements: Vec<Measurement>,
    sources: Vec<usize>,
    assigned: Vec<Option<u32>>,
    ended: Vec<u32>,
    clocks: Clocks,
    last_start: Option<u64>,
}

pub struct CpiStage {
    geometry: Geometry,
    axes: RadarAxes,
    hop: usize,
    backend: Box<dyn CafBackend>,
    cube: CubeOut,
    power: Vec<f32>,
    noise: [f32; MAX_SURVEILLANCE],
    scratch: Vec<f32>,
    coherence: LaneCoherence,
    detector: Detector,
    tracking: Tracking,
    live: LiveParams,
    aoa_possible: bool,
    generation: Option<u64>,
    counters: RadarCounters,
    last_seq: u64,
    last_end_ns: u64,
}

impl CpiStage {
    pub fn new(plan: &RadarPlan, backend: Box<dyn CafBackend>) -> Result<Self, ChannelError> {
        let shape = plan.shape;
        let refused = |_| ChannelError::Refused("CPI too large");
        let beamscan = plan
            .aoa
            .as_ref()
            .map(|aoa| {
                let positions: Vec<Vec3> = aoa
                    .positions_m
                    .iter()
                    .map(|p| Vec3::new(p[0], p[1], p[2]))
                    .collect();
                Beamscan::new(
                    &positions,
                    plan.wavelength_m,
                    aoa.grid_step_deg,
                    aoa.mirror_axis_deg,
                )
            })
            .transpose()
            .map_err(|_| ChannelError::Refused("Array geometry missing"))?;
        let tracker =
            Tracker::new(plan.tracker).map_err(|_| ChannelError::Refused("Gate out of range"))?;
        let cells = shape.gates * shape.batches;
        Ok(Self {
            geometry: Geometry {
                shape,
                report_rows: plan.report_rows.clone(),
                range_step_m: plan.range_step_m,
                cpi_s: plan.cpi_s,
                radar_rate: plan.front.radar_rate,
            },
            axes: axes_of(plan),
            hop: plan.hop,
            backend,
            cube: CubeOut::new(&shape),
            power: vec![0.0; cells],
            noise: [1.0; MAX_SURVEILLANCE],
            scratch: Vec::with_capacity(NOISE_CELLS + 1),
            coherence: LaneCoherence::new(shape.lanes).map_err(refused)?,
            detector: Detector {
                cfar: Cfar::new(plan.cfar, shape.gates, shape.batches).map_err(refused)?,
                hits: Vec::with_capacity(HIT_CAPACITY),
                clusterer: Clusterer::new(shape.gates, shape.batches).map_err(refused)?,
                clusters: Vec::with_capacity(MAX_RADAR_DETECTIONS),
                mirror_axis_deg: plan.aoa.as_ref().and_then(|aoa| aoa.mirror_axis_deg),
                beamscan,
            },
            tracking: Tracking {
                tracker,
                measurements: Vec::with_capacity(MAX_MEASUREMENTS),
                sources: Vec::with_capacity(MAX_MEASUREMENTS),
                assigned: Vec::with_capacity(MAX_MEASUREMENTS),
                ended: Vec::with_capacity(2 * MAX_RADAR_TRACKS),
                clocks: Clocks::new(),
                last_start: None,
            },
            live: plan.live(),
            aoa_possible: plan.aoa.is_some(),
            generation: None,
            counters: RadarCounters::default(),
            last_seq: 0,
            last_end_ns: 0,
        })
    }

    #[must_use]
    pub fn cube(&self) -> &CubeOut {
        &self.cube
    }

    #[must_use]
    pub fn power(&self) -> &[f32] {
        &self.power
    }

    #[must_use]
    pub const fn next_track_id(&self) -> u32 {
        self.tracking.tracker.next_id()
    }

    pub fn carry_ids(&mut self, next_id: u32) {
        self.tracking.tracker.resume_ids(next_id);
    }

    pub fn tune(&mut self, live: &LiveParams) -> Result<(), ChannelError> {
        if !live.cfar.valid() {
            return Err(ChannelError::Refused("Guard out of range"));
        }
        if !live.tracker.valid() {
            return Err(ChannelError::Refused("Gate out of range"));
        }
        self.detector
            .cfar
            .set_spec(live.cfar)
            .map_err(|_| ChannelError::Refused("Guard out of range"))?;
        self.tracking
            .tracker
            .set_config(live.tracker)
            .map_err(|_| ChannelError::Refused("Gate out of range"))?;
        self.live = *live;
        self.hop = live.hop;
        self.axes.hop_ms = (live.hop as f64 / self.geometry.radar_rate * 1_000.0) as f32;
        Ok(())
    }

    pub fn clear_tracks(&mut self, pod: &mut RadarPod) {
        self.close_report(pod);
        self.end_tracks(pod);
        self.merge_counters(pod);
    }

    pub fn retuned(&mut self, pod: &mut RadarPod) {
        self.close_report(pod);
        self.end_tracks(pod);
        self.tracking.last_start = None;
        self.merge_counters(pod);
    }

    pub fn run(&mut self, job: &CpiJob, pod: &mut RadarPod) -> CpiOutcome {
        let started = Instant::now();
        self.open_report(job, pod);
        if self
            .generation
            .is_some_and(|generation| generation != job.generation)
        {
            self.end_tracks(pod);
            self.tracking.last_start = None;
        }
        self.generation = Some(job.generation);
        self.counters.unsuppressed_groups += job.canceller_resets;
        let result = self.backend.run(job, &mut self.cube).and_then(|()| {
            pod.suppression_db = self.cube.suppression_db;
            self.counters.unsuppressed_groups += u64::from(self.cube.unsuppressed_groups);
            self.detect(job, pod)
        });
        let failed = result.err();
        if failed.is_some() {
            pod.detection_count = 0;
            pod.track_count = 0;
            pod.surface.cells.fill(0);
        }
        let compute_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let compute_s = compute_ns as f64 / NANOS_PER_SECOND;
        pod.compute_ms = (compute_s * 1_000.0) as f32;
        pod.load = (compute_s * self.geometry.radar_rate / self.hop.max(1) as f64) as f32;
        self.merge_counters(pod);
        CpiOutcome { compute_ns, failed }
    }

    fn open_report(&mut self, job: &CpiJob, pod: &mut RadarPod) {
        let shape = self.geometry.shape;
        let duration_ns = shape.samples() as f64 / self.geometry.radar_rate * NANOS_PER_SECOND;
        pod.seq = job.seq;
        pod.cpi_end_unix_ns = job.start_unix_ns + duration_ns as u64;
        pod.axes = self.axes;
        pod.clear_lists();
        pod.lanes = shape.lanes;
        pod.gpu = self.backend.gpu();
        pod.threads = self.backend.threads();
        pod.reference = job.reference;
        pod.aoa = self.aoa_state(job.phase_ready);
        pod.surface.seq = job.seq;
        pod.surface.unix_ms = pod.cpi_end_unix_ns / 1_000_000;
        self.last_seq = pod.seq;
        self.last_end_ns = pod.cpi_end_unix_ns;
    }

    fn close_report(&self, pod: &mut RadarPod) {
        pod.seq = self.last_seq;
        pod.cpi_end_unix_ns = self.last_end_ns;
        pod.axes = self.axes;
        pod.clear_lists();
    }

    fn merge_counters(&self, pod: &mut RadarPod) {
        pod.counters.unsuppressed_groups = self.counters.unsuppressed_groups;
        pod.counters.truncated_detections = self.counters.truncated_detections;
        pod.counters.dropped_tracks = self.counters.dropped_tracks;
    }

    const fn aoa_state(&self, phase_ready: bool) -> AoaState {
        if !self.live.aoa {
            AoaState::Off
        } else if !self.aoa_possible {
            AoaState::OneLane
        } else if phase_ready {
            AoaState::Ready
        } else {
            AoaState::PhaseUnknown
        }
    }

    fn end_tracks(&mut self, pod: &mut RadarPod) {
        let tracking = &mut self.tracking;
        tracking.tracker.reset(&mut tracking.ended);
        for &id in &tracking.ended {
            if let Some(clock) = tracking.clocks.remove(id)
                && !pod.push_event(lost(&clock))
            {
                self.counters.dropped_tracks += 1;
            }
        }
        tracking.clocks.clear();
        pod.track_count = 0;
    }

    fn detect(&mut self, job: &CpiJob, pod: &mut RadarPod) -> Result<(), CafError> {
        let looks = self.measure_noise();
        pod.noise_floor_db = self.noise_floor_db();
        pod.cfar_looks = looks;
        self.combine();
        let correlation = self.reference_correlation(job);
        pod.range_correlation = correlation as f32;
        let (alpha, alpha_edge) = self
            .live
            .alphas
            .pick(looks, correlation * self.live.doppler_correlation);
        let spec = CfarSpec {
            alpha,
            alpha_edge,
            ..self.live.cfar
        };
        let rows = self.geometry.report_rows.clone();
        let detector = &mut self.detector;
        let truncated = detector
            .cfar
            .set_spec(spec)
            .and_then(|()| {
                detector
                    .cfar
                    .detect(&self.power, rows.clone(), &mut detector.hits, HIT_CAPACITY)
            })
            .map_err(|_| CafError::Shape)?;
        let ridge_rows = MIN_RIDGE_ROWS.max(rows.len() / 4);
        let dropped = detector
            .clusterer
            .cluster(
                &detector.hits,
                &self.power,
                ridge_rows,
                &mut detector.clusters,
                MAX_RADAR_DETECTIONS,
            )
            .map_err(|_| CafError::Shape)?;
        self.counters.truncated_detections += (truncated + dropped) as u64;
        self.fill_detections(pod);
        self.track(job, pod);
        let first_row = self.geometry.report_rows.start;
        pod.surface.quantise(&self.power, first_row);
        Ok(())
    }

    fn measure_noise(&mut self) -> u32 {
        let geometry = &self.geometry;
        let shape = geometry.shape;
        let (rows, gates, lanes) = (shape.batches, shape.gates, shape.lanes);
        let half = self.live.cfar.clutter_half_rows;
        let centre = rows / 2;
        let clutter = centre.saturating_sub(half)..(centre + half + 1).min(rows);
        let min_gate = self.live.cfar.min_gate.min(gates.saturating_sub(1));
        let mut usable_rows = rows - clutter.len();
        let mut skip = clutter.clone();
        if usable_rows == 0 {
            usable_rows = rows;
            skip = 0..0;
        }
        let usable_gates = gates - min_gate;
        let total = usable_rows * usable_gates;
        let stride = total.div_ceil(NOISE_CELLS).max(1);
        let cell_of = |index: usize| {
            let (row, gate) = (index / usable_gates, min_gate + index % usable_gates);
            let row = if row >= skip.start {
                row + skip.len()
            } else {
                row
            };
            (row, gate)
        };
        for lane in 0..lanes {
            self.scratch.clear();
            for index in (0..total).step_by(stride) {
                let (row, gate) = cell_of(index);
                self.scratch
                    .push(self.cube.cube[geometry.cell(lane, gate, row)].norm_sqr());
            }
            let noise = median(&mut self.scratch) / LN_2;
            self.noise[lane] = if noise.is_finite() && noise > 0.0 {
                noise
            } else {
                POWER_FLOOR
            };
        }
        self.coherence.clear();
        let mut snapshot = [C32::default(); MAX_SURVEILLANCE];
        for index in (0..total).step_by(stride) {
            let (row, gate) = cell_of(index);
            for (lane, value) in snapshot.iter_mut().enumerate().take(lanes) {
                *value = self.cube.cube[geometry.cell(lane, gate, row)];
            }
            self.coherence.add(&snapshot[..lanes]);
        }
        self.coherence.looks()
    }

    fn noise_floor_db(&self) -> f32 {
        let lanes = self.geometry.shape.lanes;
        let mean = self.noise[..lanes].iter().sum::<f32>() / lanes as f32;
        10.0 * mean.max(POWER_FLOOR).log10()
    }

    fn combine(&mut self) {
        let shape = self.geometry.shape;
        let (rows, gates, lanes) = (shape.batches, shape.gates, shape.lanes);
        self.power.fill(0.0);
        let scale = 1.0 / lanes as f32;
        for lane in 0..lanes {
            let weight = scale / self.noise[lane];
            for gate in 0..gates {
                let start = self.geometry.cell(lane, gate, 0);
                let series = &self.cube.cube[start..start + rows];
                for (row, value) in series.iter().enumerate() {
                    self.power[row * gates + gate] += value.norm_sqr() * weight;
                }
            }
        }
    }

    fn reference_correlation(&self, job: &CpiJob) -> f64 {
        let shape = self.geometry.shape;
        let reference = job.lane(0);
        let run = shape.batch_len.min(CORRELATION_RUN);
        if run <= CORRELATION_LAGS || reference.len() < shape.window() {
            return 1.0;
        }
        let mut lags = [C32::default(); CORRELATION_LAGS + 1];
        for batch in 0..shape.batches {
            let start = shape.pre() + batch * shape.batch_len;
            let samples = &reference[start..start + run];
            for n in 0..run - CORRELATION_LAGS {
                let base = samples[n].conj();
                for (lag, sum) in lags.iter_mut().enumerate() {
                    *sum += samples[n + lag] * base;
                }
            }
        }
        range_correlation(&lags, shape.batch_len)
    }

    fn fill_detections(&mut self, pod: &mut RadarPod) {
        let geometry = &self.geometry;
        let shape = geometry.shape;
        let aoa_ready = pod.aoa == AoaState::Ready;
        let detector = &mut self.detector;
        let tracking = &mut self.tracking;
        tracking.measurements.clear();
        tracking.sources.clear();
        pod.detection_count = 0;
        for cluster in &detector.clusters {
            let range_m = f64::from(cluster.gate) * geometry.range_step_m;
            if range_m < self.live.min_range_m {
                continue;
            }
            let Some(slot) = pod.detections.get_mut(pod.detection_count) else {
                self.counters.truncated_detections += 1;
                continue;
            };
            let snr = if cluster.noise > 0.0 {
                cluster.power / cluster.noise
            } else {
                0.0
            };
            let doppler_hz = geometry.doppler_of_row(cluster.row);
            let aoa = if aoa_ready && !cluster.ridge {
                detector.beamscan.as_mut().and_then(|beamscan| {
                    estimate(
                        beamscan,
                        &self.cube,
                        &shape,
                        cluster,
                        snr,
                        detector.mirror_axis_deg,
                    )
                })
            } else {
                None
            };
            *slot = DetectionPod {
                range_m: range_m as f32,
                doppler_hz,
                snr_db: 10.0 * snr.max(POWER_FLOOR).log10(),
                cells: cluster.cells,
                ridge: cluster.ridge,
                aoa,
                track_id: None,
            };
            if !cluster.ridge && tracking.measurements.len() < MAX_MEASUREMENTS {
                tracking.measurements.push(Measurement {
                    range_m,
                    doppler_hz: f64::from(doppler_hz),
                    snr: f64::from(snr),
                    aoa: aoa.map(|aoa| TrackAoa {
                        azimuth_deg: aoa.azimuth_deg,
                        quality: aoa.quality,
                        sigma_deg: aoa.sigma_deg,
                    }),
                });
                tracking.sources.push(pod.detection_count);
            }
            pod.detection_count += 1;
        }
    }

    fn track(&mut self, job: &CpiJob, pod: &mut RadarPod) {
        let dt = self.tracking.last_start.map_or(0.0, |last| {
            job.start_index.saturating_sub(last) as f64 / self.geometry.radar_rate
        });
        let tracking = &mut self.tracking;
        tracking.last_start = Some(job.start_index);
        let stats = tracking.tracker.update(
            dt,
            &tracking.measurements,
            &mut tracking.assigned,
            &mut tracking.ended,
        );
        self.counters.dropped_tracks += u64::from(stats.dropped);
        self.counters.truncated_detections += u64::from(stats.ignored);
        for (&source, id) in tracking.sources.iter().zip(&tracking.assigned) {
            if let Some(detection) = pod.detections.get_mut(source) {
                detection.track_id = *id;
            }
        }
        for &id in &tracking.ended {
            if let Some(clock) = tracking.clocks.remove(id)
                && !pod.push_event(lost(&clock))
            {
                self.counters.dropped_tracks += 1;
            }
        }
        let now = pod.cpi_end_unix_ns;
        let mirror = self.detector.mirror_axis_deg;
        let mut dropped = 0u64;
        let clocks = &mut tracking.clocks;
        tracking.tracker.for_each_confirmed(|view| {
            let track = track_pod(view, mirror);
            match pod.tracks.get_mut(pod.track_count) {
                Some(slot) => {
                    *slot = track;
                    pod.track_count += 1;
                }
                None => dropped += 1,
            }
            if !note_track(clocks, &track, now, pod) {
                dropped += 1;
            }
        });
        self.counters.dropped_tracks += dropped;
    }
}

fn note_track(clocks: &mut Clocks, track: &TrackPod, now: u64, pod: &mut RadarPod) -> bool {
    let current = Clock {
        id: track.id,
        last_ns: now,
        range_m: track.range_m,
        range_rate_mps: track.range_rate_mps,
        doppler_hz: track.doppler_hz,
        snr_db: track.snr_db,
    };
    match clocks.position(track.id) {
        None => {
            let noted = clocks.insert(current);
            noted && pod.push_event(event(&current, TrackChange::Confirmed))
        }
        Some(at) => {
            let clock = &mut clocks.entries[at];
            let last_ns = clock.last_ns;
            *clock = Clock { last_ns, ..current };
            if now.saturating_sub(last_ns) >= EVENT_REPEAT_NS
                && pod.push_event(event(&current, TrackChange::Update))
            {
                clock.last_ns = now;
            }
            true
        }
    }
}

const fn event(clock: &Clock, change: TrackChange) -> TrackEventPod {
    TrackEventPod {
        track_id: clock.id,
        change,
        range_m: clock.range_m,
        range_rate_mps: clock.range_rate_mps,
        doppler_hz: clock.doppler_hz,
        snr_db: clock.snr_db,
    }
}

const fn lost(clock: &Clock) -> TrackEventPod {
    event(clock, TrackChange::Lost)
}

fn mirror_of(azimuth_deg: f32, axis: Option<f32>) -> Option<f32> {
    axis.map(|axis| (2.0 * axis - azimuth_deg).rem_euclid(360.0))
}

fn track_pod(view: &TrackView, mirror: Option<f32>) -> TrackPod {
    TrackPod {
        id: view.id,
        coasting: view.coasting,
        range_m: view.range_m as f32,
        range_rate_mps: view.range_rate_mps as f32,
        doppler_hz: view.doppler_hz as f32,
        accel_mps2: view.accel_mps2 as f32,
        range_sigma_m: view.range_sigma_m as f32,
        rate_sigma_mps: view.rate_sigma_mps as f32,
        snr_db: (10.0 * view.snr.max(f64::from(POWER_FLOOR)).log10()) as f32,
        looks: view.looks,
        misses: view.misses,
        aoa: view.aoa.map(|aoa| AoaPod {
            azimuth_deg: aoa.azimuth_deg,
            quality: aoa.quality,
            sigma_deg: aoa.sigma_deg,
            mirror_deg: mirror_of(aoa.azimuth_deg, mirror),
        }),
        trail: view.trail,
        trail_len: usize::from(view.trail_len),
    }
}

fn estimate(
    beamscan: &mut Beamscan,
    cube: &CubeOut,
    shape: &BatchShape,
    cluster: &Cluster,
    snr: f32,
    mirror: Option<f32>,
) -> Option<AoaPod> {
    let lanes = shape.lanes;
    let mut values = [[C32::default(); MAX_SURVEILLANCE]; MAX_SNAPSHOTS];
    let count = usize::from(cluster.snapshot_count).min(MAX_SNAPSHOTS);
    for (snapshot, &(row, gate)) in values.iter_mut().zip(&cluster.snapshots[..count]) {
        for (lane, value) in snapshot.iter_mut().enumerate().take(lanes) {
            *value = cube.cell(shape, lane, gate as usize, row as usize);
        }
    }
    let empty: &[C32] = &[];
    let mut slices = [empty; MAX_SNAPSHOTS];
    for (slice, snapshot) in slices.iter_mut().zip(&values).take(count) {
        *slice = &snapshot[..lanes];
    }
    let found = beamscan.estimate(&slices[..count], snr)?;
    Some(AoaPod {
        azimuth_deg: found.azimuth_deg,
        quality: found.quality,
        sigma_deg: found.sigma_deg,
        mirror_deg: found
            .mirror_deg
            .or_else(|| mirror_of(found.azimuth_deg, mirror)),
    })
}

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mid = values.len() / 2;
    let (_, value, _) = values.select_nth_unstable_by(mid, f32::total_cmp);
    *value
}
