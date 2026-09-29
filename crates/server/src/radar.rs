use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use jiff::Timestamp;
use sdrmm_dsp::radar::assign::Assignment;
use sdrmm_engine::{EngineError, ProcessorAction};
use sdrmm_wire::{
    AdsbMessage, AoaState, ArrayFailure, ArrayStatus, DecodedRecord, DecoderEvent, EventOrigin,
    NO_CHANNEL, NodeBody, PassiveRadarParams, PatchGraph, ProcessorReading, RADAR_TX_PORT,
    RadarProblem, RadarTrack, RadarTrackEvent, RadarUpdate, ReferenceMode, ServerEvent,
};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::{AppState, array, decoded::Decoded};

mod geometry;
mod truth;

#[cfg(test)]
mod tests;

use geometry::Sites;
use truth::AircraftTable;

const OVERLOAD_CPIS: u32 = 3;
const NOT_RUNNING: &str = "Not running";

#[derive(Debug, thiserror::Error)]
pub(crate) enum RadarRefusal {
    #[error("no radar {0}")]
    NoRadar(String),
    #[error("{0}")]
    Refused(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RadarBinding {
    pub(crate) array: Option<String>,
    pub(crate) tx: Option<String>,
    pub(crate) params: PassiveRadarParams,
}

impl RadarBinding {
    fn of(graph: &PatchGraph, node: &str, params: PassiveRadarParams) -> Self {
        Self {
            array: graph.array_of_processor(node).map(str::to_owned),
            tx: graph
                .sources_of(node, RADAR_TX_PORT)
                .next()
                .map(str::to_owned),
            params,
        }
    }

    fn wired_like(&self, other: &Self) -> bool {
        self.array == other.array && self.tx == other.tx
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Enrichment {
    bearing_deg: Option<f32>,
    lat: Option<f64>,
    lon: Option<f64>,
    icao: Option<String>,
}

impl Enrichment {
    fn of(track: &RadarTrack) -> Self {
        Self {
            bearing_deg: track.aoa.and_then(|aoa| aoa.bearing_deg),
            lat: track.fix.map(|fix| fix.lat),
            lon: track.fix.map(|fix| fix.lon),
            icao: track.adsb.as_ref().map(|adsb| adsb.icao.clone()),
        }
    }

    fn fill(&self, event: &mut RadarTrackEvent) {
        event.bearing_deg = self.bearing_deg;
        event.lat = self.lat;
        event.lon = self.lon;
        event.icao.clone_from(&self.icao);
    }
}

struct RadarNode {
    binding: RadarBinding,
    latest: Option<RadarUpdate>,
    refused: Option<RadarProblem>,
    aircraft: AircraftTable,
    solver: Assignment,
    overloaded: u32,
    lagged: u64,
    remembered: HashMap<u32, Enrichment>,
}

fn cpi_centre(update: &RadarUpdate) -> Timestamp {
    let end = update
        .at
        .parse::<Timestamp>()
        .unwrap_or_else(|_| Timestamp::now());
    let half_ms = (f64::from(update.axes.cpi_ms) / 2.0).round() as i64;
    Timestamp::from_millisecond(end.as_millisecond().saturating_sub(half_ms)).unwrap_or(end)
}

fn now_at() -> String {
    format!("{:.3}", Timestamp::now())
}

impl RadarNode {
    fn new(binding: RadarBinding) -> Self {
        Self {
            binding,
            latest: None,
            refused: None,
            aircraft: AircraftTable::default(),
            solver: truth::solver(),
            overloaded: 0,
            lagged: 0,
            remembered: HashMap::new(),
        }
    }

    fn complete(&mut self, update: &mut RadarUpdate, sites: &Sites) {
        update.problems.clear();
        sites.problems(&mut update.problems);
        update.geometry = sites.geometry();
        if let Some(heading_deg) = sites.heading_deg() {
            geometry::true_bearings(update, heading_deg);
        }
        self.place(update, sites);
        self.pod_problems(update);
        update.health.lagged_updates = update.health.lagged_updates.saturating_add(self.lagged);
    }

    fn place(&mut self, update: &mut RadarUpdate, sites: &Sites) {
        let now = cpi_centre(update);
        self.aircraft.prune(now);
        update.truth.clear();
        for track in &mut update.tracks {
            track.fix = None;
            track.adsb = None;
        }
        let Some(baseline) = sites.both() else {
            return;
        };
        self.aircraft
            .truth(now, baseline, &update.axes, &mut update.truth);
        truth::associate(
            &mut self.solver,
            &mut update.truth,
            &mut update.tracks,
            &update.axes,
        );
        if sites.heading_deg().is_some() {
            geometry::fix_tracks(update, baseline, self.binding.params.assumed_altitude_m);
        }
    }

    fn pod_problems(&mut self, update: &mut RadarUpdate) {
        let health = &update.health;
        if self.binding.params.aoa && health.aoa == AoaState::PhaseUnknown {
            update.problems.push(RadarProblem::PhaseUnknown);
        }
        self.overloaded = if health.load > 1.0 {
            self.overloaded.saturating_add(1)
        } else {
            0
        };
        if self.overloaded >= OVERLOAD_CPIS {
            update.problems.push(RadarProblem::Overloaded);
        }
        if health.reference.mode != ReferenceMode::Raw && !health.reference.locked {
            update.problems.push(RadarProblem::ReferenceLost);
        }
    }

    fn enrich(
        &mut self,
        events: Vec<RadarTrackEvent>,
        tracks: &[RadarTrack],
    ) -> Vec<RadarTrackEvent> {
        for track in tracks {
            self.remembered.insert(track.id, Enrichment::of(track));
        }
        let enriched: Vec<RadarTrackEvent> = events
            .into_iter()
            .map(|mut event| {
                if let Some(known) = self.remembered.get(&event.track_id) {
                    known.fill(&mut event);
                }
                event
            })
            .collect();
        self.remembered
            .retain(|id, _| tracks.iter().any(|track| track.id == *id));
        enriched
    }

    fn clear_tracks(&mut self) {
        self.remembered.clear();
        if let Some(latest) = &mut self.latest {
            latest.tracks.clear();
            for detection in &mut latest.detections {
                detection.track_id = None;
            }
            for sighting in &mut latest.truth {
                sighting.track_id = None;
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct RadarHub {
    nodes: Mutex<HashMap<String, RadarNode>>,
}

impl RadarHub {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, RadarNode>> {
        self.nodes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn binding(&self, node: &str) -> Option<RadarBinding> {
        self.lock().get(node).map(|entry| entry.binding.clone())
    }

    fn bind(&self, node: &str, binding: RadarBinding) -> bool {
        let mut nodes = self.lock();
        let Some(entry) = nodes.get_mut(node) else {
            nodes.insert(node.to_owned(), RadarNode::new(binding));
            return true;
        };
        let rewired = !entry.binding.wired_like(&binding);
        entry.binding = binding;
        rewired || entry.refused.is_some()
    }

    fn nodes(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    fn knows(&self, node: &str) -> bool {
        self.lock().contains_key(node)
    }

    pub(crate) fn on_report(
        &self,
        state: &AppState,
        node: &str,
        mut update: RadarUpdate,
    ) -> RadarUpdate {
        let Some(binding) = self.binding(node).or_else(|| {
            array::processor_array(state, node).map(|array| RadarBinding {
                array: Some(array),
                ..RadarBinding::default()
            })
        }) else {
            let dropped = std::mem::take(&mut update.events).len();
            tracing::warn!(node, dropped, "a report from a radar that is gone");
            return update;
        };
        let sites = sites(state, &binding);
        let events = std::mem::take(&mut update.events);
        let enriched = {
            let mut nodes = self.lock();
            let entry = nodes
                .entry(node.to_owned())
                .or_insert_with(|| RadarNode::new(binding.clone()));
            entry.complete(&mut update, &sites);
            let enriched = entry.enrich(events, &update.tracks);
            entry.refused = None;
            entry.latest = Some(update.clone());
            enriched
        };
        if !enriched.is_empty() {
            let anchor = anchor(state, binding.array.as_deref());
            publish(state, node, anchor, &update, enriched);
        }
        update
    }

    pub(crate) fn refuse(&self, state: &AppState, node: &str, problem: RadarProblem) {
        let update = RadarUpdate {
            at: now_at(),
            problems: vec![problem.clone()],
            ..RadarUpdate::default()
        };
        {
            let mut nodes = self.lock();
            let entry = nodes
                .entry(node.to_owned())
                .or_insert_with(|| RadarNode::new(RadarBinding::default()));
            if entry.refused.as_ref() == Some(&problem) {
                return;
            }
            entry.refused = Some(problem);
            entry.remembered.clear();
            entry.latest = Some(update.clone());
        }
        emit(state, node, update);
    }

    fn wait(&self, state: &AppState, node: &str) {
        let Some(binding) = self.binding(node) else {
            return;
        };
        let sites = sites(state, &binding);
        let mut update = RadarUpdate {
            at: now_at(),
            ..RadarUpdate::default()
        };
        {
            let mut nodes = self.lock();
            let Some(entry) = nodes.get_mut(node) else {
                return;
            };
            entry.complete(&mut update, &sites);
            entry.refused = None;
            entry.remembered.clear();
            entry.latest = Some(update.clone());
        }
        emit(state, node, update);
    }

    pub(crate) fn latest(&self, node: &str) -> Option<RadarUpdate> {
        self.lock()
            .get(node)
            .map(|entry| entry.latest.clone().unwrap_or_default())
    }

    pub(crate) fn forget(&self, node: &str) {
        self.lock().remove(node);
    }

    pub(crate) fn lagged(&self, count: u64) {
        for entry in self.lock().values_mut() {
            entry.lagged = entry.lagged.saturating_add(count);
        }
    }

    fn observe(&self, node: &str, at: Timestamp, message: &AdsbMessage) {
        if let Some(entry) = self.lock().get_mut(node) {
            entry.aircraft.observe(at, message);
        }
    }

    fn clear_tracks(&self, node: &str) {
        if let Some(entry) = self.lock().get_mut(node) {
            entry.clear_tracks();
        }
    }
}

fn sites(state: &AppState, binding: &RadarBinding) -> Sites {
    let pose = binding
        .array
        .as_deref()
        .and_then(|array| array::array_pose(state, array));
    let transmitter = binding.tx.as_deref().and_then(|tx| state.gps.fix(tx));
    Sites::of(pose.as_ref(), transmitter.as_ref())
}

fn anchor(state: &AppState, array: Option<&str>) -> u32 {
    let Some(array) = array else {
        return 0;
    };
    state
        .engine
        .array_statuses()
        .into_iter()
        .find(|status| status.node == array)
        .and_then(|status| status.anchor)
        .or_else(|| {
            let spec = state.arrays.binding(array)?.spec?;
            spec.lanes
                .into_iter()
                .flatten()
                .next()
                .map(|lane| lane.device_set)
        })
        .unwrap_or(0)
}

fn publish(
    state: &AppState,
    node: &str,
    anchor: u32,
    update: &RadarUpdate,
    events: Vec<RadarTrackEvent>,
) {
    let at = if update.at.is_empty() {
        now_at()
    } else {
        update.at.clone()
    };
    for event in events {
        state.engine.publish_decoded(DecodedRecord {
            origin: Some(EventOrigin {
                node: node.to_owned(),
                transmission: 0,
            }),
            device_set: anchor,
            channel: NO_CHANNEL,
            at: at.clone(),
            freq_hz: update.axes.carrier_hz,
            event: DecoderEvent::Radar(event),
            sinks: Vec::new(),
        });
    }
}

fn emit(state: &AppState, node: &str, update: RadarUpdate) {
    state.engine.emit_event(ServerEvent::ProcessorUpdate {
        node: node.to_owned(),
        reading: Box::new(ProcessorReading::PassiveRadar(update)),
    });
}

fn refusal(
    state: &AppState,
    statuses: &[ArrayStatus],
    node: &str,
    binding: &RadarBinding,
) -> Option<RadarProblem> {
    let Some(array) = binding.array.as_deref() else {
        return Some(RadarProblem::NoArray);
    };
    let refused = statuses
        .iter()
        .flat_map(|status| &status.processors)
        .find(|processor| processor.node == node)
        .and_then(|processor| processor.error.clone());
    if let Some(refused) = refused {
        return Some(RadarProblem::Refused(refused));
    }
    match state.arrays.binding(array) {
        Some(held) if held.spec.is_some() => None,
        held => Some(RadarProblem::Refused(
            held.and_then(|held| held.problem)
                .unwrap_or_else(|| ArrayFailure::Unwired.to_string()),
        )),
    }
}

pub(crate) fn reconcile(state: &AppState, graph: &PatchGraph) -> Vec<(String, String)> {
    release(state, graph);
    let statuses = state.engine.array_statuses();
    for node in &graph.nodes {
        let NodeBody::PassiveRadar(radar) = &node.body else {
            continue;
        };
        let binding = RadarBinding::of(graph, &node.id, radar.settings);
        let fresh = state.radar.bind(&node.id, binding.clone());
        match refusal(state, &statuses, &node.id, &binding) {
            Some(problem) => state.radar.refuse(state, &node.id, problem),
            None if fresh => state.radar.wait(state, &node.id),
            None => {}
        }
    }
    Vec::new()
}

pub(crate) fn release(state: &AppState, graph: &PatchGraph) {
    for node in state.radar.nodes() {
        let drawn = graph
            .node(&node)
            .is_some_and(|found| matches!(found.body, NodeBody::PassiveRadar(_)));
        if !drawn {
            state.radar.forget(&node);
        }
    }
}

pub(crate) fn clear(state: &AppState, node: &str) -> Result<(), RadarRefusal> {
    if !state.radar.knows(node) {
        return Err(RadarRefusal::NoRadar(node.to_owned()));
    }
    state
        .engine
        .processor_action(node, ProcessorAction::ClearTracks)
        .map_err(|error| match error {
            EngineError::ProcessorNotFound(_) => RadarRefusal::Refused(NOT_RUNNING.to_owned()),
            other => RadarRefusal::Refused(other.to_string()),
        })?;
    state.radar.clear_tracks(node);
    Ok(())
}

pub(crate) fn start(state: &AppState) {
    let records = state.decoded.subscribe();
    let fed = state.clone();
    let _detached = crate::spawn_task("sdrmm-radar-truth", move || feed_truth(fed, records));
}

async fn feed_truth(state: AppState, mut records: broadcast::Receiver<Decoded>) {
    loop {
        match records.recv().await {
            Ok(Decoded::Record(routed)) => observe(&state, &routed.record),
            Ok(Decoded::Lost(count)) | Err(RecvError::Lagged(count)) => {
                tracing::warn!(count, "radar truth missed decoded records");
            }
            Err(RecvError::Closed) => break,
        }
    }
}

fn observe(state: &AppState, record: &DecodedRecord) {
    let DecoderEvent::Adsb(message) = &record.event else {
        return;
    };
    let at = record
        .at
        .parse::<Timestamp>()
        .unwrap_or_else(|_| Timestamp::now());
    for sink in &record.sinks {
        state.radar.observe(sink, at, message);
    }
}
