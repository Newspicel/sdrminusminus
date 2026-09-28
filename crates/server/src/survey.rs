use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use sdrmm_engine::SpectrumSnapshot;
use sdrmm_wire::{
    NodeBody, PatchGraph, PositionFix, ServerEvent, SignalMapNode, StateSnapshot, SurveyAction,
    SurveyCell, SurveyGrid, SurveyStop, SurveyUpdate, port_stream,
    survey::SURVEY_LEVEL_INTERVAL_MS,
};
use tokio::{sync::broadcast::error::RecvError, task::JoinHandle};

use crate::{AppState, rest::AppError, workspace};

mod measure;

#[cfg(test)]
mod tests;

use measure::{Merged, measure_dbfs, merge};

pub(crate) const LEVEL_INTERVAL: Duration = Duration::from_millis(SURVEY_LEVEL_INTERVAL_MS);

const IQ_PORT: &str = "iq";

pub(crate) enum SurveyRefusal {
    Missing(String),
    NoRadio,
    NoPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Radio {
    device_set: u32,
    stream: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Wiring {
    settings: SignalMapNode,
    iq_wired: bool,
    radio: Option<Radio>,
    position_node: Option<String>,
}

#[derive(Default)]
struct SurveySession {
    wiring: Wiring,
    recording: bool,
    frequency_hz: Option<f64>,
    cells: Vec<SurveyCell>,
    dropped: u64,
    last_fix: Option<String>,
    level_at: Option<Instant>,
    task: Option<JoinHandle<()>>,
    generation: u64,
}

impl SurveySession {
    fn grid(&self, node: &str) -> SurveyGrid {
        SurveyGrid {
            node: node.to_owned(),
            frequency_hz: self.frequency_hz,
            offset_hz: self.wiring.settings.offset_hz,
            bandwidth_hz: self.wiring.settings.bandwidth_hz,
            recording: self.recording,
            cells: self.cells.clone(),
            dropped: self.dropped,
        }
    }

    fn update(
        &self,
        level_dbfs: Option<f32>,
        target_hz: Option<f64>,
        cell: Option<SurveyCell>,
        stopped: Option<SurveyStop>,
    ) -> SurveyUpdate {
        SurveyUpdate {
            level_dbfs,
            target_hz,
            recording: self.recording,
            cells: u32::try_from(self.cells.len()).unwrap_or(u32::MAX),
            dropped: self.dropped,
            cell,
            stopped,
        }
    }

    fn listening(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }

    fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation = self.generation.wrapping_add(1);
    }

    fn listen(&mut self, state: &AppState, node: &str) {
        self.stop();
        let Some(radio) = self.wiring.radio else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(node, "no runtime: the survey cannot listen to its radio");
            return;
        };
        self.task = Some(runtime.spawn(watch(
            state.clone(),
            node.to_owned(),
            radio,
            self.generation,
        )));
    }

    fn step(
        &mut self,
        snapshot: &SpectrumSnapshot,
        fix: Option<&PositionFix>,
        now: Instant,
    ) -> Option<SurveyUpdate> {
        let settings = self.wiring.settings;
        let target_hz = snapshot.center_hz + settings.offset_hz as f64;
        let level = measure_dbfs(
            snapshot.center_hz,
            f64::from(snapshot.span_hz),
            &snapshot.db,
            target_hz,
            settings.bandwidth_hz as f64,
        );
        if self.recording
            && self
                .frequency_hz
                .is_some_and(|frequency| frequency.round() != target_hz.round())
        {
            self.recording = false;
            return Some(self.update(level, Some(target_hz), None, Some(SurveyStop::Retuned)));
        }
        if self.recording
            && let (Some(level_dbfs), Some(fix)) = (level, fix)
            && self.last_fix.as_deref() != Some(fix.time.as_str())
        {
            let merged = merge(
                &mut self.cells,
                SurveyCell {
                    latitude: fix.latitude,
                    longitude: fix.longitude,
                    frequency_hz: target_hz,
                    level_dbfs,
                    measured_at: fix.time.clone(),
                    observations: 1,
                    accuracy_m: fix.accuracy_m,
                },
            );
            if matches!(merged, Merged::Evicted(_)) {
                self.dropped = self.dropped.saturating_add(1);
            }
            self.frequency_hz.get_or_insert(target_hz);
            self.last_fix = Some(fix.time.clone());
            self.level_at = Some(now);
            let cell = self.cells.get(merged.index()).cloned();
            return Some(self.update(level, Some(target_hz), cell, None));
        }
        if self
            .level_at
            .is_some_and(|at| now.duration_since(at) < LEVEL_INTERVAL)
        {
            return None;
        }
        self.level_at = Some(now);
        Some(self.update(level, Some(target_hz), None, None))
    }
}

#[derive(Default)]
pub(crate) struct SurveyHub {
    sessions: Mutex<HashMap<String, SurveySession>>,
}

impl SurveyHub {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, SurveySession>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn grid(&self, node: &str) -> Option<SurveyGrid> {
        self.lock().get(node).map(|session| session.grid(node))
    }

    #[expect(dead_code)]
    pub(crate) fn cells(&self, node: &str) -> u32 {
        self.lock().get(node).map_or(0, |session| {
            u32::try_from(session.cells.len()).unwrap_or(u32::MAX)
        })
    }

    #[expect(dead_code)]
    pub(crate) fn recording(&self, node: &str) -> bool {
        self.lock()
            .get(node)
            .is_some_and(|session| session.recording)
    }

    pub(crate) fn act(
        &self,
        state: &AppState,
        node: &str,
        action: SurveyAction,
    ) -> Result<SurveyGrid, AppError> {
        let (grid, update) = {
            let mut sessions = self.lock();
            let session = sessions
                .get_mut(node)
                .ok_or_else(|| SurveyRefusal::Missing(node.to_owned()))?;
            match action {
                SurveyAction::Start => {
                    if session.wiring.radio.is_none() {
                        return Err(SurveyRefusal::NoRadio.into());
                    }
                    if session.wiring.position_node.is_none() {
                        return Err(SurveyRefusal::NoPosition.into());
                    }
                    session.recording = true;
                    if !session.listening() {
                        session.listen(state, node);
                    }
                }
                SurveyAction::Stop => session.recording = false,
                SurveyAction::Clear => {
                    session.cells.clear();
                    session.dropped = 0;
                    session.frequency_hz = None;
                    session.last_fix = None;
                }
            }
            (session.grid(node), session.update(None, None, None, None))
        };
        emit(state, node, update);
        Ok(grid)
    }

    fn apply(&self, state: &AppState, wanted: HashMap<String, Wiring>) {
        let mut stopped = Vec::new();
        {
            let mut sessions = self.lock();
            sessions.retain(|node, session| {
                let keep = wanted.contains_key(node);
                if !keep {
                    session.stop();
                }
                keep
            });
            for (node, wiring) in wanted {
                let session = sessions.entry(node.clone()).or_default();
                let reason = if !wiring.iq_wired || wiring.position_node.is_none() {
                    Some(SurveyStop::Unwired)
                } else if wiring.radio.is_none() {
                    Some(SurveyStop::RadioGone)
                } else {
                    None
                };
                let retuned = session.wiring.radio != wiring.radio;
                session.wiring = wiring;
                if let Some(reason) = reason
                    && session.recording
                {
                    session.recording = false;
                    stopped.push((node.clone(), session.update(None, None, None, Some(reason))));
                }
                if retuned || !session.listening() {
                    session.listen(state, &node);
                }
            }
        }
        for (node, update) in stopped {
            emit(state, &node, update);
        }
    }

    fn sample(
        &self,
        node: &str,
        generation: u64,
        snapshot: &SpectrumSnapshot,
        fix_of: impl FnOnce(&str) -> Option<PositionFix>,
        now: Instant,
    ) -> Option<SurveyUpdate> {
        let mut sessions = self.lock();
        let session = sessions
            .get_mut(node)
            .filter(|session| session.generation == generation)?;
        let fix = session.wiring.position_node.as_deref().and_then(fix_of);
        session.step(snapshot, fix.as_ref(), now)
    }

    fn radio_gone(&self, node: &str, generation: u64) -> Option<SurveyUpdate> {
        let mut sessions = self.lock();
        let session = sessions
            .get_mut(node)
            .filter(|session| session.generation == generation)?;
        session.recording = false;
        Some(session.update(None, None, None, Some(SurveyStop::RadioGone)))
    }
}

fn emit(state: &AppState, node: &str, update: SurveyUpdate) {
    state.engine.emit_event(ServerEvent::SurveyUpdate {
        node: node.to_owned(),
        update: Box::new(update),
    });
}

async fn watch(state: AppState, node: String, radio: Radio, generation: u64) {
    let engine = state.engine.clone();
    let subscribed = tokio::task::spawn_blocking(move || {
        engine.subscribe_spectrum(radio.device_set, radio.stream)
    })
    .await;
    let mut spectrum = match subscribed {
        Ok(Ok(spectrum)) => spectrum,
        Ok(Err(error)) => {
            tracing::warn!(%error, node, "the survey cannot hear its radio");
            gone(&state, &node, generation);
            return;
        }
        Err(error) => {
            tracing::error!(%error, node, "the survey listener failed to start");
            gone(&state, &node, generation);
            return;
        }
    };
    loop {
        match spectrum.recv().await {
            Ok(snapshot) => {
                let update = state.survey.sample(
                    &node,
                    generation,
                    &snapshot,
                    |position| state.gps.fix(position),
                    Instant::now(),
                );
                if let Some(update) = update {
                    emit(&state, &node, update);
                }
            }
            Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => {
                gone(&state, &node, generation);
                return;
            }
        }
    }
}

fn gone(state: &AppState, node: &str, generation: u64) {
    if let Some(update) = state.survey.radio_gone(node, generation) {
        emit(state, node, update);
    }
}

fn radio_of(
    devices: &HashMap<String, u32>,
    snapshot: &StateSnapshot,
    source: &str,
    port: &str,
) -> Option<Radio> {
    if let Some(&device_set) = devices.get(source) {
        return port_stream(IQ_PORT, port).map(|stream| Radio { device_set, stream });
    }
    snapshot.device_sets.iter().find_map(|set| {
        set.virtual_lanes
            .iter()
            .find(|lane| lane.node == source && lane.port == port)
            .map(|lane| Radio {
                device_set: set.id,
                stream: lane.stream,
            })
    })
}

fn wirings(graph: &PatchGraph, snapshot: &StateSnapshot) -> HashMap<String, Wiring> {
    let devices: HashMap<String, u32> = workspace::bind_devices(graph, snapshot)
        .into_iter()
        .collect();
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeBody::SignalMap(settings) = &node.body else {
                return None;
            };
            let iq = graph
                .edges
                .iter()
                .find(|edge| edge.to.node == node.id && edge.to.port == IQ_PORT);
            let wiring = Wiring {
                settings: *settings,
                iq_wired: iq.is_some(),
                radio: iq.and_then(|edge| {
                    radio_of(&devices, snapshot, &edge.from.node, &edge.from.port)
                }),
                position_node: graph.position_source(&node.id).map(str::to_owned),
            };
            Some((node.id.clone(), wiring))
        })
        .collect()
}

pub(crate) fn reconcile(state: &AppState, graph: &PatchGraph) -> Vec<(String, String)> {
    let snapshot = state.engine.snapshot();
    state.survey.apply(state, wirings(graph, &snapshot));
    Vec::new()
}

pub(crate) fn start(state: &AppState) {
    match state.store.active_workspace() {
        Ok(Some(active)) => {
            reconcile(state, &active.snapshot.graph);
        }
        Ok(None) => {}
        Err(error) => tracing::error!(%error, "could not read the active workspace for surveys"),
    }
}
