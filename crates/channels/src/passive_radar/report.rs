use sdrmm_dsp::radar::batch::MAX_SURVEILLANCE;
use sdrmm_dsp::radar::track::TRAIL_LEN;
use sdrmm_wire::radar::{
    AoaState, MAX_RADAR_DETECTIONS, MAX_RADAR_TRACKS, MAX_TRACK_TRAIL, RadarAoa, RadarAxes,
    RadarDetection, RadarHealth, RadarTrack, RadarTrackEvent, RadarTrailPoint, RadarUpdate,
    ReferenceHealth, TrackChange, TrackState,
};

use super::plan::RadarPlan;
use super::surface::RangeDopplerSurface;
use crate::array_processor::stamp_at;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AoaPod {
    pub azimuth_deg: f32,
    pub quality: f32,
    pub sigma_deg: f32,
    pub mirror_deg: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DetectionPod {
    pub range_m: f32,
    pub doppler_hz: f32,
    pub snr_db: f32,
    pub cells: u32,
    pub ridge: bool,
    pub aoa: Option<AoaPod>,
    pub track_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackPod {
    pub id: u32,
    pub coasting: bool,
    pub range_m: f32,
    pub range_rate_mps: f32,
    pub doppler_hz: f32,
    pub accel_mps2: f32,
    pub range_sigma_m: f32,
    pub rate_sigma_mps: f32,
    pub snr_db: f32,
    pub looks: u32,
    pub misses: u32,
    pub aoa: Option<AoaPod>,
    pub trail: [(f32, f32); TRAIL_LEN],
    pub trail_len: usize,
}

impl TrackPod {
    pub const EMPTY: Self = Self {
        id: 0,
        coasting: false,
        range_m: 0.0,
        range_rate_mps: 0.0,
        doppler_hz: 0.0,
        accel_mps2: 0.0,
        range_sigma_m: 0.0,
        rate_sigma_mps: 0.0,
        snr_db: 0.0,
        looks: 0,
        misses: 0,
        aoa: None,
        trail: [(0.0, 0.0); TRAIL_LEN],
        trail_len: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackEventPod {
    pub track_id: u32,
    pub change: TrackChange,
    pub range_m: f32,
    pub range_rate_mps: f32,
    pub doppler_hz: f32,
    pub snr_db: f32,
}

impl TrackEventPod {
    pub const EMPTY: Self = Self {
        track_id: 0,
        change: TrackChange::Update,
        range_m: 0.0,
        range_rate_mps: 0.0,
        doppler_hz: 0.0,
        snr_db: 0.0,
    };
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RadarCounters {
    pub unsuppressed_groups: u64,
    pub dropped_samples: u64,
    pub dropped_cpis: u64,
    pub discarded_cpis: u64,
    pub dropped_reports: u64,
    pub truncated_detections: u64,
    pub dropped_tracks: u64,
    pub gpu_failures: u64,
}

pub struct RadarPod {
    pub seq: u64,
    pub cpi_end_unix_ns: u64,
    pub axes: RadarAxes,
    pub wavelength_m: f32,
    pub detections: [DetectionPod; MAX_RADAR_DETECTIONS],
    pub detection_count: usize,
    pub tracks: [TrackPod; MAX_RADAR_TRACKS],
    pub track_count: usize,
    pub events: [TrackEventPod; MAX_RADAR_TRACKS],
    pub event_count: usize,
    pub counters: RadarCounters,
    pub suppression_db: [f32; MAX_SURVEILLANCE],
    pub lanes: usize,
    pub load: f32,
    pub front_load: f32,
    pub compute_ms: f32,
    pub noise_floor_db: f32,
    pub cfar_looks: u32,
    pub range_correlation: f32,
    pub gpu: bool,
    pub threads: u32,
    pub aoa: AoaState,
    pub reference: ReferenceHealth,
    pub surface: RangeDopplerSurface,
}

impl RadarPod {
    #[must_use]
    pub fn new(plan: &RadarPlan) -> Box<Self> {
        Box::new(Self {
            seq: 0,
            cpi_end_unix_ns: 0,
            axes: axes_of(plan),
            wavelength_m: plan.wavelength_m as f32,
            detections: [DetectionPod::default(); MAX_RADAR_DETECTIONS],
            detection_count: 0,
            tracks: [TrackPod::EMPTY; MAX_RADAR_TRACKS],
            track_count: 0,
            events: [TrackEventPod::EMPTY; MAX_RADAR_TRACKS],
            event_count: 0,
            counters: RadarCounters::default(),
            suppression_db: [0.0; MAX_SURVEILLANCE],
            lanes: plan.shape.lanes,
            load: 0.0,
            front_load: 0.0,
            compute_ms: 0.0,
            noise_floor_db: 0.0,
            cfar_looks: 0,
            range_correlation: 1.0,
            gpu: false,
            threads: 0,
            aoa: AoaState::Off,
            reference: ReferenceHealth::default(),
            surface: RangeDopplerSurface::new(plan),
        })
    }

    #[must_use]
    pub fn detections(&self) -> &[DetectionPod] {
        &self.detections[..self.detection_count.min(MAX_RADAR_DETECTIONS)]
    }

    #[must_use]
    pub fn tracks(&self) -> &[TrackPod] {
        &self.tracks[..self.track_count.min(MAX_RADAR_TRACKS)]
    }

    #[must_use]
    pub fn events(&self) -> &[TrackEventPod] {
        &self.events[..self.event_count.min(MAX_RADAR_TRACKS)]
    }

    pub fn push_event(&mut self, event: TrackEventPod) -> bool {
        match self.events.get_mut(self.event_count) {
            Some(slot) => {
                *slot = event;
                self.event_count += 1;
                true
            }
            None => false,
        }
    }

    pub const fn clear_lists(&mut self) {
        self.detection_count = 0;
        self.track_count = 0;
        self.event_count = 0;
    }
}

#[must_use]
pub fn axes_of(plan: &RadarPlan) -> RadarAxes {
    RadarAxes {
        sample_rate_hz: plan.front.radar_rate,
        carrier_hz: plan.carrier_hz,
        range_step_m: plan.range_step_m as f32,
        gates: plan.shape.gates as u32,
        doppler_step_hz: plan.doppler_step_hz() as f32,
        doppler_rows: plan.report_rows.len() as u32,
        batches: plan.shape.batches as u32,
        cpi_ms: (plan.cpi_s * 1_000.0) as f32,
        hop_ms: (plan.hop_s() * 1_000.0) as f32,
        lanes: plan.shape.lanes as u32,
    }
}

pub fn fill_update(pod: &RadarPod, update: &mut RadarUpdate) {
    update.seq = pod.seq;
    stamp_at(&mut update.at, pod.cpi_end_unix_ns);
    update.axes = pod.axes;
    update.detections.clear();
    update.detections.extend(
        pod.detections()
            .iter()
            .map(|detection| wire_detection(pod, detection)),
    );
    fill_tracks(pod, &mut update.tracks);
    update.truth.clear();
    fill_health(pod, &mut update.health);
    update.geometry = None;
    update.problems.clear();
    track_events(pod, &mut update.events);
}

pub fn track_events(pod: &RadarPod, out: &mut Vec<RadarTrackEvent>) {
    out.clear();
    out.extend(pod.events().iter().map(|event| RadarTrackEvent {
        track_id: event.track_id,
        change: event.change,
        range_km: event.range_m / 1_000.0,
        range_rate_mps: event.range_rate_mps,
        doppler_hz: event.doppler_hz,
        snr_db: event.snr_db,
        bearing_deg: None,
        lat: None,
        lon: None,
        icao: None,
    }));
}

fn wire_aoa(aoa: &AoaPod) -> RadarAoa {
    RadarAoa {
        azimuth_deg: aoa.azimuth_deg,
        bearing_deg: None,
        sigma_deg: aoa.sigma_deg,
        quality: aoa.quality,
        mirror_deg: aoa.mirror_deg,
    }
}

fn wire_detection(pod: &RadarPod, detection: &DetectionPod) -> RadarDetection {
    RadarDetection {
        range_km: detection.range_m / 1_000.0,
        doppler_hz: detection.doppler_hz,
        range_rate_mps: -pod.wavelength_m * detection.doppler_hz,
        snr_db: detection.snr_db,
        cells: detection.cells,
        aoa: detection.aoa.as_ref().map(wire_aoa),
        track_id: detection.track_id,
    }
}

fn fill_tracks(pod: &RadarPod, tracks: &mut Vec<RadarTrack>) {
    let source = pod.tracks();
    tracks.truncate(source.len());
    for (slot, track) in tracks.iter_mut().zip(source) {
        write_track(slot, track);
    }
    for track in &source[tracks.len()..] {
        let mut slot = RadarTrack {
            id: 0,
            state: TrackState::Confirmed,
            range_km: 0.0,
            range_rate_mps: 0.0,
            doppler_hz: 0.0,
            accel_mps2: 0.0,
            range_sigma_m: 0.0,
            rate_sigma_mps: 0.0,
            snr_db: 0.0,
            looks: 0,
            misses: 0,
            aoa: None,
            fix: None,
            adsb: None,
            trail: Vec::with_capacity(MAX_TRACK_TRAIL),
        };
        write_track(&mut slot, track);
        tracks.push(slot);
    }
}

fn write_track(slot: &mut RadarTrack, track: &TrackPod) {
    slot.id = track.id;
    slot.state = if track.coasting {
        TrackState::Coasting
    } else {
        TrackState::Confirmed
    };
    slot.range_km = track.range_m / 1_000.0;
    slot.range_rate_mps = track.range_rate_mps;
    slot.doppler_hz = track.doppler_hz;
    slot.accel_mps2 = track.accel_mps2;
    slot.range_sigma_m = track.range_sigma_m;
    slot.rate_sigma_mps = track.rate_sigma_mps;
    slot.snr_db = track.snr_db;
    slot.looks = track.looks;
    slot.misses = track.misses;
    slot.aoa = track.aoa.as_ref().map(wire_aoa);
    slot.fix = None;
    slot.adsb = None;
    slot.trail.clear();
    slot.trail
        .extend(track.trail[..track.trail_len.min(TRAIL_LEN)].iter().map(
            |&(range_km, doppler_hz)| RadarTrailPoint {
                range_km,
                doppler_hz,
            },
        ));
}

fn fill_health(pod: &RadarPod, health: &mut RadarHealth) {
    health.suppression_db.clear();
    health
        .suppression_db
        .extend_from_slice(&pod.suppression_db[..pod.lanes.min(MAX_SURVEILLANCE)]);
    let counters = pod.counters;
    health.unsuppressed_groups = counters.unsuppressed_groups;
    health.dropped_samples = counters.dropped_samples;
    health.dropped_cpis = counters.dropped_cpis;
    health.discarded_cpis = counters.discarded_cpis;
    health.dropped_reports = counters.dropped_reports;
    health.lagged_updates = 0;
    health.truncated_detections = counters.truncated_detections;
    health.dropped_tracks = counters.dropped_tracks;
    health.gpu_failures = counters.gpu_failures;
    health.load = pod.load;
    health.front_load = pod.front_load;
    health.compute_ms = pod.compute_ms;
    health.noise_floor_db = pod.noise_floor_db;
    health.cfar_looks = pod.cfar_looks;
    health.range_correlation = pod.range_correlation;
    health.gpu = pod.gpu;
    health.threads = pod.threads;
    health.aoa = pod.aoa;
    health.reference = pod.reference;
}
