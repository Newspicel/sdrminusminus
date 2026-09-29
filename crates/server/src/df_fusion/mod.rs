use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        mpsc::{SyncSender, TrySendError},
    },
};

use sdrmm_wire::{
    DfBearing, DfEstimate, DfFusionState, DfStation, FusionGridOwned, NavTarget, NodeBody,
    PatchGraph, PositionFix, StreamKind, TriangulationParams,
    fusion::{FUSION_FIXED_HALF_LIFE_S, FUSION_MOVING_HALF_LIFE_S, FusionDecay},
    geo::{self, LatLon},
};

use crate::{AppState, fusion_routes, rest::AppError};

mod estimate;
mod grid;
mod nav;
mod observation;
mod votes;
mod worker;

#[cfg(test)]
mod tests;

use estimate::Located;
use grid::LogGrid;
use nav::{Nav, NavFlags};
pub(crate) use observation::Refusal;
use observation::{CORRELATION_S, Observation, prepare};
use votes::{Candidate, VoteKey, VoteMaps};

pub(crate) const QUEUE_DEPTH: usize = 256;
pub(crate) const MAX_EMITTER_CANDIDATES: usize = 8;

const MOVING_HOLD_S: f64 = 300.0;
const MOVED_M: f64 = 50.0;
const MOVED_WITHIN_S: f64 = 60.0;
const REFRESH_S: f64 = 1.0;
const RECENTRE_S: f64 = 5.0;
const MIN_RECENTRE_KEYS: usize = 2;
const FRAME_S: f64 = 1.0;
const FRAME_REFRESH_S: f64 = 5.0;
const REBUILD_AFTER_PAINTS: u32 = 1_000;

pub(crate) type SharedFusion = Arc<FusionHub>;

struct Sighting {
    node: String,
    device_set: u32,
    bearing: DfBearing,
    now_s: f64,
    at: String,
}

enum Job {
    Bearing(Box<Sighting>),
    Guide {
        node: String,
        fix: Option<Box<PositionFix>>,
        now_s: f64,
    },
}

pub(crate) struct NoTriangulation(pub(crate) String);

#[derive(Default)]
pub(crate) struct FusionHub {
    nodes: Mutex<HashMap<String, NodeFusion>>,
    queue: OnceLock<SyncSender<Job>>,
    pub(crate) routes: Mutex<Arc<fusion_routes::Table>>,
}

pub(crate) struct FusionOutcome {
    pub(crate) state: DfFusionState,
    pub(crate) first_fix: Option<DfEstimate>,
    pub(crate) grid_frame: Option<FusionGridOwned>,
}

#[derive(Default)]
struct DecayClock {
    at_s: Option<f64>,
    half_lives: f64,
}

impl DecayClock {
    fn advance(&mut self, now_s: f64, half_life_s: f64) {
        if let Some(at_s) = self.at_s
            && now_s > at_s
            && half_life_s > 0.0
        {
            self.half_lives += (now_s - at_s) / half_life_s;
        }
        self.at_s = Some(self.at_s.map_or(now_s, |at_s| at_s.max(now_s)));
    }
}

struct StationTrack {
    row: DfStation,
    at_s: f64,
    position: LatLon,
}

struct NodeFusion {
    params: TriangulationParams,
    grid: Option<LogGrid>,
    votes: VoteMaps,
    nav: Nav,
    stations: BTreeMap<String, StationTrack>,
    guided: Option<PositionFix>,
    clock: DecayClock,
    samples: u32,
    announced: bool,
    moving_until_s: f64,
    refreshed_s: Option<f64>,
    recentred_s: Option<f64>,
    frame_s: Option<f64>,
    frames: u32,
    paints: u32,
    dirty: bool,
    frame_dirty: bool,
    estimate: Option<DfEstimate>,
    emitters: Vec<DfEstimate>,
    nav_target: Option<NavTarget>,
    flags: NavFlags,
    dropped: u64,
    refused: u64,
}

impl NodeFusion {
    fn new(params: &TriangulationParams) -> Self {
        Self {
            params: *params,
            grid: None,
            votes: VoteMaps::default(),
            nav: Nav::new(params.nav, params.probe_km),
            stations: BTreeMap::new(),
            guided: None,
            clock: DecayClock::default(),
            samples: 0,
            announced: false,
            moving_until_s: f64::NEG_INFINITY,
            refreshed_s: None,
            recentred_s: None,
            frame_s: None,
            frames: 0,
            paints: 0,
            dirty: false,
            frame_dirty: false,
            estimate: None,
            emitters: Vec::new(),
            nav_target: None,
            flags: NavFlags {
                no_guide_position: true,
                no_bearings: false,
            },
            dropped: 0,
            refused: 0,
        }
    }

    fn configure(&mut self, params: &TriangulationParams) {
        let extent_changed = (params.extent_km - self.params.extent_km).abs() > f64::EPSILON;
        self.params = *params;
        self.nav.configure(params.nav, params.probe_km);
        if extent_changed && let Some(grid) = &self.grid {
            self.grid = Some(LogGrid::new(grid.enu().origin(), params.extent_km));
            self.rebuild();
        }
    }

    fn reset(&mut self, now_s: f64) -> FusionOutcome {
        let blank = self
            .grid
            .as_ref()
            .map(|grid| grid.blank_frame(self.frames, frame_millis(now_s)));
        let frames = self.frames.wrapping_add(u32::from(blank.is_some()));
        let mut fresh = Self::new(&self.params);
        std::mem::swap(&mut fresh.nav, &mut self.nav);
        fresh.nav.clear();
        fresh.guided = self.guided.take();
        fresh.frames = frames;
        *self = fresh;
        let mut outcome = self.settle(now_s, false);
        outcome.grid_frame = blank;
        outcome
    }

    fn half_life_s(&self, now_s: f64) -> u32 {
        match self.params.decay {
            FusionDecay::Fixed => FUSION_FIXED_HALF_LIFE_S,
            FusionDecay::Moving => FUSION_MOVING_HALF_LIFE_S,
            FusionDecay::HalfLife { seconds } => seconds,
            FusionDecay::Auto if now_s < self.moving_until_s => FUSION_MOVING_HALF_LIFE_S,
            FusionDecay::Auto => FUSION_FIXED_HALF_LIFE_S,
        }
    }

    fn track_station(&mut self, observation: &mut Observation, at: &str) -> f32 {
        let position = LatLon {
            lat: observation.lat,
            lon: observation.lon,
        };
        let track = self
            .stations
            .entry(observation.station.clone())
            .or_insert_with(|| StationTrack {
                row: DfStation {
                    station_id: observation.station.clone(),
                    ..DfStation::default()
                },
                at_s: f64::NEG_INFINITY,
                position,
            });
        let elapsed = observation.at_s - track.at_s;
        let weight = if track.at_s.is_finite() {
            (elapsed / CORRELATION_S).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        if track.at_s.is_finite()
            && elapsed <= MOVED_WITHIN_S
            && geo::distance_m(track.position, position) > MOVED_M
        {
            observation.moving = true;
        }
        track.at_s = track.at_s.max(observation.at_s);
        track.position = position;
        track.row.lat = observation.lat;
        track.row.lon = observation.lon;
        track.row.bearings = track.row.bearings.saturating_add(1);
        at.clone_into(&mut track.row.last_seen);
        track.row.last_bearing_deg = observation.bearing_deg;
        track.row.sigma_deg = observation.sigma_deg;
        track.row.source = observation.source;
        track.row.moving = observation.moving;
        weight
    }

    fn observe(
        &mut self,
        bearing: &DfBearing,
        now_s: f64,
        at: &str,
    ) -> Result<FusionOutcome, Refusal> {
        let mut observation = match prepare(bearing, now_s, self.params.min_confidence) {
            Ok(observation) => observation,
            Err(refusal) => {
                self.refused = self.refused.saturating_add(1);
                return Err(refusal);
            }
        };
        let weight = self.track_station(&mut observation, at);
        if observation.moving {
            self.moving_until_s = now_s + MOVING_HOLD_S;
        }
        self.clock
            .advance(now_s, f64::from(self.half_life_s(now_s)));
        let place = LatLon {
            lat: observation.lat,
            lon: observation.lon,
        };
        let extent_km = self.params.extent_km;
        let at_enu = self
            .grid
            .get_or_insert_with(|| LogGrid::new(place, extent_km))
            .enu()
            .to_enu(place);
        let added = self
            .votes
            .add(&observation, at_enu, weight, self.clock.half_lives);
        if let Some(evicted) = added.evicted {
            self.unpaint(&evicted);
        }
        self.refresh_key(added.index, false);
        self.samples = self.samples.saturating_add(1);
        self.nav.remember(&observation);
        self.refresh_powers(now_s);
        Ok(self.settle(now_s, true))
    }

    fn guide(&mut self, fix: Option<&PositionFix>, now_s: f64) -> FusionOutcome {
        self.guided = fix.cloned();
        self.settle(now_s, false)
    }

    fn unpaint(&mut self, key: &VoteKey) {
        if let (Some(grid), Some(painted)) = (self.grid.as_mut(), key.painted.as_ref()) {
            grid.paint(Some(&painted.wedge), None);
            self.touched();
        }
    }

    fn touched(&mut self) {
        self.paints = self.paints.saturating_add(1);
        self.dirty = true;
        self.frame_dirty = true;
    }

    fn refresh_key(&mut self, index: usize, force: bool) {
        let clock = self.clock.half_lives;
        let (Some(grid), Some(key)) = (self.grid.as_mut(), self.votes.keys.get_mut(index)) else {
            return;
        };
        let Some(old) = key.refresh(clock, force) else {
            return;
        };
        grid.paint(
            old.as_ref().map(|painted| &painted.wedge),
            key.painted.as_ref().map(|painted| &painted.wedge),
        );
        self.touched();
    }

    fn refresh_powers(&mut self, now_s: f64) {
        if self
            .refreshed_s
            .is_some_and(|refreshed| now_s - refreshed < REFRESH_S)
        {
            return;
        }
        self.refreshed_s = Some(now_s);
        for retired in self.votes.retire(self.clock.half_lives) {
            self.unpaint(&retired);
        }
        if self.paints >= REBUILD_AFTER_PAINTS {
            self.rebuild();
        }
        for index in 0..self.votes.keys.len() {
            self.refresh_key(index, false);
        }
    }

    fn rebuild(&mut self) {
        let Some(grid) = self.grid.as_mut() else {
            return;
        };
        grid.clear();
        for key in &self.votes.keys {
            if let Some(painted) = &key.painted {
                grid.paint(None, Some(&painted.wedge));
            }
        }
        self.paints = 0;
        self.dirty = true;
        self.frame_dirty = true;
    }

    fn recentre(&mut self, now_s: f64) {
        if self
            .recentred_s
            .is_some_and(|recentred| now_s - recentred < RECENTRE_S)
        {
            return;
        }
        let Some(grid) = self.grid.as_mut() else {
            return;
        };
        let peak_at =
            (self.votes.keys.len() >= MIN_RECENTRE_KEYS).then(|| grid.index_centre(grid.max().0));
        let guided_at = self.guided.as_ref().map(|fix| {
            grid.enu().to_enu(LatLon {
                lat: fix.latitude,
                lon: fix.longitude,
            })
        });
        let target = match peak_at {
            Some(peak) if grid.in_edge_band(peak) => Some(peak),
            _ => guided_at.filter(|guided| {
                grid.in_edge_band(*guided)
                    && peak_at.is_none_or(|peak| {
                        !grid.in_edge_band_around(grid.snapped_centre(guided.0, guided.1), peak)
                    })
            }),
        };
        if let Some((east, north)) = target
            && grid.recentre(east, north)
        {
            self.recentred_s = Some(now_s);
            self.rebuild();
        }
    }

    fn settle(&mut self, now_s: f64, announce: bool) -> FusionOutcome {
        self.recentre(now_s);
        if self.dirty {
            self.estimate_now();
        } else {
            self.recount();
        }
        let (nav, flags) = self
            .nav
            .update(self.guided.as_ref(), self.estimate.as_ref(), now_s);
        self.nav_target = nav;
        self.flags = flags;
        let first_fix = if announce { self.announce() } else { None };
        let grid_frame = self.grid_frame(now_s);
        FusionOutcome {
            state: self.state(now_s),
            first_fix,
            grid_frame,
        }
    }

    fn estimate_now(&mut self) {
        self.dirty = false;
        let Some(grid) = self.grid.as_ref().filter(|_| !self.votes.keys.is_empty()) else {
            self.estimate = None;
            self.emitters.clear();
            return;
        };
        let global: Option<Located> = estimate::global(grid, self.samples);
        let mut candidates: Vec<Candidate> = Vec::new();
        self.votes
            .candidates(grid, MAX_EMITTER_CANDIDATES, &mut candidates);
        self.emitters = estimate::emitters(
            grid,
            &candidates,
            global.as_ref(),
            usize::from(self.params.max_emitters),
            self.samples,
        );
        self.estimate = global.map(|located| located.estimate);
    }

    fn recount(&mut self) {
        let samples = self.samples;
        for estimate in self.estimate.iter_mut().chain(self.emitters.iter_mut()) {
            estimate.samples = samples;
            estimate.converged = estimate::converged(estimate);
        }
        if self.estimate.is_none()
            && samples >= estimate::MIN_ESTIMATE_SAMPLES
            && self.grid.is_some()
            && !self.votes.keys.is_empty()
        {
            self.estimate_now();
        }
    }

    fn announce(&mut self) -> Option<DfEstimate> {
        match self.estimate {
            Some(estimate) if estimate.converged && !self.announced => {
                self.announced = true;
                Some(estimate)
            }
            Some(estimate) if estimate.converged => None,
            _ => {
                self.announced = false;
                None
            }
        }
    }

    fn grid_frame(&mut self, now_s: f64) -> Option<FusionGridOwned> {
        let wait_s = if self.frame_dirty {
            FRAME_S
        } else {
            FRAME_REFRESH_S
        };
        if self.frame_s.is_some_and(|sent| now_s - sent < wait_s) {
            return None;
        }
        let grid = self.grid.as_ref()?;
        self.frame_dirty = false;
        self.frame_s = Some(now_s);
        let seq = self.frames;
        self.frames = self.frames.wrapping_add(1);
        Some(grid.frame(seq, frame_millis(now_s)))
    }

    fn state(&self, now_s: f64) -> DfFusionState {
        DfFusionState {
            estimate: self.estimate,
            emitters: self.emitters.clone(),
            nav: self.nav_target,
            stations: self
                .stations
                .values()
                .map(|track| track.row.clone())
                .collect(),
            samples: self.samples,
            half_life_s: self.half_life_s(now_s),
            no_guide_position: self.flags.no_guide_position,
            no_bearings: self.flags.no_bearings,
            dropped: self.dropped,
            refused: self.refused,
        }
    }
}

impl FusionHub {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, NodeFusion>> {
        self.nodes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn configure(&self, node: &str, params: &TriangulationParams) {
        self.lock()
            .entry(node.to_owned())
            .and_modify(|fusion| fusion.configure(params))
            .or_insert_with(|| NodeFusion::new(params));
    }

    pub(crate) fn observe(
        &self,
        node: &str,
        bearing: &DfBearing,
        now_s: f64,
        at: &str,
    ) -> Result<FusionOutcome, Refusal> {
        self.lock()
            .get_mut(node)
            .ok_or(Refusal::Unknown)?
            .observe(bearing, now_s, at)
    }

    pub(crate) fn guide(
        &self,
        node: &str,
        fix: Option<&PositionFix>,
        now_s: f64,
    ) -> Option<FusionOutcome> {
        self.lock()
            .get_mut(node)
            .map(|fusion| fusion.guide(fix, now_s))
    }

    pub(crate) fn state(&self, node: &str) -> Option<DfFusionState> {
        let now_s = now_s();
        self.lock().get(node).map(|fusion| fusion.state(now_s))
    }

    pub(crate) fn reset(&self, node: &str) -> Option<FusionOutcome> {
        let now_s = now_s();
        self.lock().get_mut(node).map(|fusion| fusion.reset(now_s))
    }

    fn due_frames(&self, now_s: f64) -> Vec<(String, FusionGridOwned)> {
        self.lock()
            .iter_mut()
            .filter_map(|(node, fusion)| Some((node.clone(), fusion.grid_frame(now_s)?)))
            .collect()
    }

    pub(crate) fn forget(&self, node: &str) {
        self.lock().remove(node);
    }

    pub(crate) fn nodes(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    pub(crate) fn count_dropped(&self, node: &str, count: u64) {
        if let Some(fusion) = self.lock().get_mut(node) {
            fusion.dropped = fusion.dropped.saturating_add(count);
        }
    }

    pub(crate) fn count_lost(&self, count: u64) -> usize {
        let mut nodes = self.lock();
        for fusion in nodes.values_mut() {
            fusion.dropped = fusion.dropped.saturating_add(count);
        }
        nodes.len()
    }

    fn install(&self, sender: SyncSender<Job>) -> bool {
        self.queue.set(sender).is_ok()
    }

    fn enqueue(&self, job: Job) -> bool {
        let Some(queue) = self.queue.get() else {
            self.lost(&job, "the fusion thread is not running");
            return false;
        };
        match queue.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(job)) => {
                self.lost(&job, "the fusion queue is full");
                false
            }
            Err(TrySendError::Disconnected(job)) => {
                self.lost(&job, "the fusion thread stopped");
                false
            }
        }
    }

    fn lost(&self, job: &Job, reason: &str) {
        match job {
            Job::Bearing(sighting) => {
                tracing::debug!(node = sighting.node, reason, "bearing dropped");
                self.count_dropped(&sighting.node, 1);
            }
            Job::Guide { node, .. } => {
                tracing::debug!(
                    node,
                    reason,
                    "guide position dropped, the next fix stands in"
                );
            }
        }
    }
}

pub(crate) fn now_s() -> f64 {
    jiff::Timestamp::now().as_microsecond() as f64 / 1e6
}

fn frame_millis(now_s: f64) -> u64 {
    (now_s * 1_000.0).max(0.0) as u64
}

pub(crate) fn start(state: &AppState) {
    let (sender, jobs) = std::sync::mpsc::sync_channel(QUEUE_DEPTH);
    if !state.fusion.install(sender) {
        tracing::error!("the fusion thread runs already");
        return;
    }
    let worker = worker::Worker::new(
        Arc::downgrade(&state.fusion),
        Arc::downgrade(&state.engine),
        Arc::downgrade(&state.surfaces),
    );
    let spawned = std::thread::Builder::new()
        .name("sdrmm-fusion".to_owned())
        .spawn(move || worker.run(&jobs));
    if let Err(error) = spawned {
        tracing::error!(%error, "could not start the fusion thread");
    }
    match state.store.active_workspace() {
        Ok(Some(active)) => {
            for (node, reason) in reconcile(state, &active.snapshot.graph) {
                tracing::warn!(node, reason, "triangulation refused at startup");
            }
        }
        Ok(None) => {}
        Err(error) => tracing::error!(%error, "could not read the active workspace for fusion"),
    }
}

pub(crate) fn reconcile(state: &AppState, graph: &PatchGraph) -> Vec<(String, String)> {
    let mut refusals = Vec::new();
    let mut wanted = HashSet::new();
    for node in &graph.nodes {
        let NodeBody::Triangulation(triangulation) = &node.body else {
            continue;
        };
        if let Some(problem) = triangulation.settings.problem() {
            refusals.push((node.id.clone(), problem.to_owned()));
            continue;
        }
        wanted.insert(node.id.as_str());
        state.fusion.configure(&node.id, &triangulation.settings);
        state.surfaces.open(&node.id, StreamKind::FusionGrid);
    }
    for node in state.fusion.nodes() {
        if !wanted.contains(node.as_str()) {
            state.fusion.forget(&node);
            state.surfaces.forget(&node);
        }
    }
    refusals
}

pub(crate) fn clear(state: &AppState, node: &str) -> Result<(), AppError> {
    let fresh = state
        .fusion
        .reset(node)
        .ok_or_else(|| NoTriangulation(node.to_owned()))?;
    worker::deliver(&state.engine, Some(&state.surfaces), node, fresh);
    Ok(())
}

pub(crate) fn submit(
    state: &AppState,
    node: &str,
    device_set: u32,
    bearing: DfBearing,
    now_s: f64,
    at: String,
) {
    state.fusion.enqueue(Job::Bearing(Box::new(Sighting {
        node: node.to_owned(),
        device_set,
        bearing,
        now_s,
        at,
    })));
}

pub(crate) fn guide(state: &AppState, node: &str, fix: Option<PositionFix>, now_s: f64) {
    state.fusion.enqueue(Job::Guide {
        node: node.to_owned(),
        fix: fix.map(Box::new),
        now_s,
    });
}
