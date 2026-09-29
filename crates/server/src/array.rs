use std::{
    collections::{HashMap, HashSet},
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use sdrmm_engine::{ArraySpec, EngineError, LaneRef, ProcessorSpec};
use sdrmm_wire::{
    ArrayCalRecord, ArrayCalSource, ArrayFailure, ArrayNode, ArrayOrientation,
    ArrayRecordingRequest, ArrayRecordingStarted, ArrayTune, ArrayTuneRequest, ChannelSettings,
    DeviceSet, HeadingSource, LaneKey, NodeBody, PatchApplyReport, PatchGraph, PatchNode,
    PatchRefusal, PositionFix, ServerEvent, StateSnapshot, WorkspaceArray, WorkspaceState,
    array::MAX_ARRAY_LANES,
};

use crate::{AppState, rest::AppError};

mod pump;

#[cfg(test)]
pub(crate) use pump::handle;

const POSE_LOST_EVERY: Duration = Duration::from_secs(1);
const ALREADY_RECORDING: &str = "Already recording";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArrayWiring {
    Unwired,
    Lanes(Vec<(String, u32)>),
    Gap { lane: u32 },
    Duplicate { lane: u32 },
    TooMany,
}

impl ArrayWiring {
    fn failure(&self) -> Option<ArrayFailure> {
        match *self {
            Self::Unwired | Self::Lanes(_) => None,
            Self::Gap { lane } => Some(ArrayFailure::LaneGap { lane }),
            Self::Duplicate { lane } => Some(ArrayFailure::DuplicateLane { lane }),
            Self::TooMany => Some(ArrayFailure::TooManyLanes),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ArrayBinding {
    pub(crate) position_node: Option<String>,
    pub(crate) spec: Option<ArraySpec>,
    pub(crate) problem: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessorBinding {
    pub(crate) array: String,
    pub(crate) lane_ports: Vec<String>,
}

#[derive(Default)]
pub(crate) struct ArrayHub {
    arrays: Mutex<HashMap<String, ArrayBinding>>,
    processors: Mutex<HashMap<String, ProcessorBinding>>,
    poses_lost: Mutex<HashMap<String, Instant>>,
    pumping: AtomicBool,
}

#[cfg_attr(not(test), expect(dead_code))]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArraySummary {
    pub(crate) device_sets: Vec<u32>,
    pub(crate) center_hz: Option<f64>,
    pub(crate) can_calibrate: bool,
    pub(crate) problem: Option<String>,
}

#[cfg_attr(not(test), expect(dead_code))]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArrayPose {
    pub(crate) lat: f64,
    pub(crate) lon: f64,
    pub(crate) altitude_m: Option<f64>,
    pub(crate) heading_deg: Option<f64>,
    pub(crate) heading_sigma_deg: Option<f64>,
    pub(crate) heading_source: Option<HeadingSource>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ArrayRefusal {
    #[error("No array {0}")]
    NoArray(String),
    #[error("Not recording")]
    NotRecording,
    #[error("{0}")]
    Refused(String),
}

impl ArrayHub {
    fn arrays(&self) -> MutexGuard<'_, HashMap<String, ArrayBinding>> {
        self.arrays.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn processors(&self) -> MutexGuard<'_, HashMap<String, ProcessorBinding>> {
        self.processors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn binding(&self, node: &str) -> Option<ArrayBinding> {
        self.arrays().get(node).cloned()
    }

    fn fed_by(&self, position: &str) -> Vec<String> {
        self.arrays()
            .iter()
            .filter(|(_, binding)| {
                binding.spec.is_some() && binding.position_node.as_deref() == Some(position)
            })
            .map(|(node, _)| node.clone())
            .collect()
    }

    fn positions(&self) -> Vec<String> {
        let wired: HashSet<String> = self
            .arrays()
            .values()
            .filter(|binding| binding.spec.is_some())
            .filter_map(|binding| binding.position_node.clone())
            .collect();
        wired.into_iter().collect()
    }

    fn pose_lost_is_news(&self, array: &str, now: Instant) -> bool {
        let mut lost = self
            .poses_lost
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let due = lost
            .get(array)
            .is_none_or(|at| now.saturating_duration_since(*at) >= POSE_LOST_EVERY);
        if due {
            lost.insert(array.to_owned(), now);
        }
        due
    }

    fn holds_processor(&self, node: &str) -> bool {
        self.processors().contains_key(node)
    }

    fn replace(
        &self,
        arrays: HashMap<String, ArrayBinding>,
        processors: HashMap<String, ProcessorBinding>,
    ) -> HashMap<String, ProcessorBinding> {
        self.poses_lost
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|node, _| arrays.contains_key(node));
        *self.arrays() = arrays;
        std::mem::replace(&mut *self.processors(), processors)
    }

    fn keep(&self, arrays: &[String], processors: &[String]) -> Vec<String> {
        let kept: HashMap<String, ArrayBinding> = self
            .arrays()
            .drain()
            .filter(|(node, _)| arrays.contains(node))
            .collect();
        let (held, gone): (HashMap<_, _>, HashMap<_, _>) = self
            .processors()
            .drain()
            .partition(|(node, _)| processors.contains(node));
        self.replace(kept, held);
        gone.into_keys().collect()
    }
}

pub(crate) fn wired_array(graph: &PatchGraph, node: &str) -> ArrayWiring {
    let lanes = graph.array_lanes(node);
    if lanes.is_empty() {
        return ArrayWiring::Unwired;
    }
    if lanes.len() > MAX_ARRAY_LANES as usize {
        return ArrayWiring::TooMany;
    }
    let mut wired = Vec::with_capacity(lanes.len());
    for (slot, lane) in lanes.into_iter().enumerate() {
        let slot = u32::try_from(slot).unwrap_or(MAX_ARRAY_LANES);
        let Some((device, stream)) = lane else {
            return ArrayWiring::Gap { lane: slot };
        };
        let lane = (device.to_owned(), stream);
        if wired.contains(&lane) {
            return ArrayWiring::Duplicate { lane: slot };
        }
        wired.push(lane);
    }
    ArrayWiring::Lanes(wired)
}

pub(crate) fn lane_port_listeners(graph: &PatchGraph, processor: &str, port: &str) -> Vec<String> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from.node == processor && edge.from.port == port)
        .filter(|edge| {
            graph
                .node(&edge.to.node)
                .is_some_and(|target| matches!(target.body, NodeBody::Channel(_)))
        })
        .map(|edge| edge.to.node.clone())
        .collect()
}

struct Bring<'a> {
    state: &'a AppState,
    graph: &'a PatchGraph,
    bound: &'a [(String, u32)],
    saved: &'a WorkspaceState,
    live: StateSnapshot,
    previous: HashMap<String, ArrayBinding>,
}

impl Bring<'_> {
    fn device_set(&self, device_node: &str) -> Option<u32> {
        self.bound
            .iter()
            .find(|(node, _)| node == device_node)
            .map(|(_, device_set)| *device_set)
    }

    fn set(&self, device_set: u32) -> Option<&DeviceSet> {
        self.live
            .device_sets
            .iter()
            .find(|set| set.id == device_set)
    }

    fn bind_all(&self) -> HashMap<String, ArrayBinding> {
        let mut arrays = HashMap::new();
        let mut waiting = Vec::new();
        for (node, settings) in array_nodes(self.graph) {
            let (binding, held) = self.bind(&node.id, settings);
            if held {
                waiting.push((node, settings));
            }
            arrays.insert(node.id.clone(), binding);
        }
        for (node, settings) in waiting {
            arrays.insert(node.id.clone(), self.bind(&node.id, settings).0);
        }
        arrays
    }

    fn bind(&self, node: &str, settings: &ArrayNode) -> (ArrayBinding, bool) {
        let mut binding = ArrayBinding {
            position_node: self.graph.position_source(node).map(str::to_owned),
            ..ArrayBinding::default()
        };
        let lanes = match wired_array(self.graph, node) {
            ArrayWiring::Lanes(lanes) => lanes,
            refused => {
                binding.problem = refused.failure().map(|failure| failure.to_string());
                return (binding, false);
            }
        };
        let spec = self.spec(node, settings, &lanes);
        match self.state.engine.apply_array(spec.clone()) {
            Ok(()) => {
                binding.spec = Some(spec);
                (binding, false)
            }
            Err(error) => {
                let held = matches!(error, EngineError::Array(ArrayFailure::LaneHeld { .. }));
                binding.problem = Some(error.to_string());
                (binding, held)
            }
        }
    }

    fn spec(&self, node: &str, settings: &ArrayNode, lanes: &[(String, u32)]) -> ArraySpec {
        let lanes: Vec<Option<LaneRef>> = lanes
            .iter()
            .map(|(device, stream)| {
                self.device_set(device).map(|device_set| LaneRef {
                    device_set,
                    stream: *stream,
                })
            })
            .collect();
        let running = self.live.arrays.iter().find(|status| status.node == node);
        let previous = self
            .previous
            .get(node)
            .and_then(|binding| binding.spec.as_ref());
        let (tune, warm) = match (running, previous) {
            (Some(_), Some(previous))
                if previous.lanes == lanes && previous.settings == *settings =>
            {
                (previous.tune, previous.warm.clone())
            }
            (Some(status), _) => {
                let live = ArrayTune {
                    center_hz: status.center_hz,
                    gain: status.gain,
                };
                (Some(live), self.warm(settings, &lanes, Some(live)))
            }
            (None, _) => {
                let tune = self.saved.array(node).map(|array| array.tune);
                (tune, self.warm(settings, &lanes, tune))
            }
        };
        ArraySpec {
            node: node.to_owned(),
            lanes,
            settings: settings.clone(),
            tune,
            warm,
        }
    }

    fn warm(
        &self,
        settings: &ArrayNode,
        lanes: &[Option<LaneRef>],
        tune: Option<ArrayTune>,
    ) -> Option<ArrayCalRecord> {
        if !settings.cal.warm_start {
            return None;
        }
        let keys = lanes
            .iter()
            .map(|lane| {
                let lane = (*lane)?;
                Some(LaneKey {
                    device: self.set(lane.device_set)?.device.id(),
                    stream: lane.stream,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let first = lanes.first().copied().flatten()?;
        let set = self.set(first.device_set)?;
        let rate = set.settings.sample_rate?;
        let center_hz = tune.map(|tune| tune.center_hz).or_else(|| {
            set.settings
                .for_stream(first.stream, &set.capabilities.per_stream)
                .center_hz
        })?;
        match self.state.store.array_calibration(&keys, rate, center_hz) {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(%error, "stored array calibrations unreadable, starting cold");
                None
            }
        }
    }

    fn bind_processors(
        &self,
        arrays: &HashMap<String, ArrayBinding>,
        refusals: &mut Vec<(String, String)>,
    ) -> HashMap<String, ProcessorBinding> {
        let mut processors = HashMap::new();
        for node in &self.graph.nodes {
            let Some(params) = node.body.processor_params() else {
                continue;
            };
            let Some(array) = self.graph.array_of_processor(&node.id) else {
                continue;
            };
            if !arrays.get(array).is_some_and(|array| array.spec.is_some()) {
                continue;
            }
            let lane_ports = wired_lane_ports(self.graph, node);
            let spec = ProcessorSpec {
                node: node.id.clone(),
                array: array.to_owned(),
                params,
                lane_ports: lane_ports.clone(),
                steer_from: self.graph.steer_source(&node.id).map(str::to_owned),
            };
            if let Err(error) = self.state.engine.apply_processor(spec) {
                refusals.push((node.id.clone(), error.to_string()));
            }
            processors.insert(
                node.id.clone(),
                ProcessorBinding {
                    array: array.to_owned(),
                    lane_ports,
                },
            );
        }
        processors
    }
}

fn array_nodes(graph: &PatchGraph) -> impl Iterator<Item = (&PatchNode, &ArrayNode)> {
    graph.nodes.iter().filter_map(|node| match &node.body {
        NodeBody::Array(settings) => Some((node, settings)),
        _ => None,
    })
}

fn wired_arrays(graph: &PatchGraph) -> Vec<String> {
    array_nodes(graph)
        .filter(|(node, _)| matches!(wired_array(graph, &node.id), ArrayWiring::Lanes(_)))
        .map(|(node, _)| node.id.clone())
        .collect()
}

fn processors_on(graph: &PatchGraph, arrays: &[String]) -> Vec<String> {
    graph
        .nodes
        .iter()
        .filter(|node| node.body.processor_params().is_some())
        .filter(|node| {
            graph
                .array_of_processor(&node.id)
                .is_some_and(|array| arrays.iter().any(|wired| wired == array))
        })
        .map(|node| node.id.clone())
        .collect()
}

pub(crate) fn release(state: &AppState, graph: &PatchGraph) {
    let arrays = wired_arrays(graph);
    let processors = processors_on(graph, &arrays);
    state.engine.retain_arrays(&arrays);
    state.engine.retain_processors(&processors);
    for node in state.arrays.keep(&arrays, &processors) {
        state.surfaces.forget(&node);
    }
}

fn wired_lane_ports(graph: &PatchGraph, node: &PatchNode) -> Vec<String> {
    node.body
        .lane_outputs()
        .iter()
        .filter(|port| {
            graph
                .edges
                .iter()
                .any(|edge| edge.from.node == node.id && edge.from.port == **port)
        })
        .map(|port| (*port).to_owned())
        .collect()
}

const fn draws_surface(body: &NodeBody) -> bool {
    matches!(
        body,
        NodeBody::PassiveRadar(_) | NodeBody::SpatialSpectrum(_) | NodeBody::Correlator(_)
    )
}

pub(crate) fn reconcile(
    state: &AppState,
    graph: &PatchGraph,
    bound: &[(String, u32)],
    saved: &WorkspaceState,
) -> Vec<(String, String)> {
    let bring = Bring {
        state,
        graph,
        bound,
        saved,
        live: state.engine.snapshot(),
        previous: state.arrays.arrays().clone(),
    };
    state.engine.retain_arrays(&wired_arrays(graph));
    let arrays = bring.bind_all();
    let mut refusals: Vec<(String, String)> = arrays
        .iter()
        .filter_map(|(node, binding)| Some((node.clone(), binding.problem.clone()?)))
        .collect();
    let drawn: Vec<String> = arrays
        .iter()
        .filter(|(_, binding)| binding.spec.is_some())
        .map(|(node, _)| node.clone())
        .collect();
    state.engine.retain_arrays(&drawn);
    let processors = bring.bind_processors(&arrays, &mut refusals);
    let listed: Vec<String> = processors.keys().cloned().collect();
    state.engine.retain_processors(&listed);
    open_surfaces(state, graph, &processors);
    let gone = state.arrays.replace(arrays, processors);
    for node in gone.keys().filter(|node| !listed.contains(node)) {
        state.surfaces.forget(node);
    }
    send_poses(state);
    refusals.sort();
    refusals
}

fn open_surfaces(
    state: &AppState,
    graph: &PatchGraph,
    processors: &HashMap<String, ProcessorBinding>,
) {
    for node in processors.keys() {
        if graph
            .node(node)
            .is_some_and(|patch| draws_surface(&patch.body))
        {
            state.surfaces.open(node);
        }
    }
}

fn send_poses(state: &AppState) {
    let fed: Vec<(String, Option<String>)> = state
        .arrays
        .arrays()
        .iter()
        .filter(|(_, binding)| binding.spec.is_some())
        .map(|(node, binding)| (node.clone(), binding.position_node.clone()))
        .collect();
    let now = received_ns();
    for (array, position) in fed {
        let fix = position.as_deref().and_then(|gps| state.gps.fix(gps));
        pose(state, &array, fix, now);
    }
}

fn received_ns() -> i64 {
    i64::try_from(sdrmm_device::now_ns()).unwrap_or(i64::MAX)
}

fn pose(state: &AppState, array: &str, fix: Option<PositionFix>, received_ns: i64) {
    if let Err(error) = state.engine.update_array_pose(array, fix, received_ns) {
        tracing::debug!(%error, array, "array pose refused");
        if state.arrays.pose_lost_is_news(array, Instant::now()) {
            state.engine.emit_event(ServerEvent::Error {
                message: format!("array pose lost: {array}"),
            });
        }
    }
}

pub(crate) fn virtual_lane_channels(
    state: &AppState,
    graph: &PatchGraph,
    live: &StateSnapshot,
) -> Vec<(String, u32, u32)> {
    let processors = state.arrays.processors();
    live.device_sets
        .iter()
        .flat_map(|set| {
            set.virtual_lanes
                .iter()
                .filter(|lane| {
                    processors
                        .get(&lane.node)
                        .is_some_and(|binding| binding.lane_ports.contains(&lane.port))
                })
                .flat_map(|lane| {
                    lane_port_listeners(graph, &lane.node, &lane.port)
                        .into_iter()
                        .map(|listener| (listener, set.id, lane.stream))
                })
        })
        .collect()
}

pub(crate) fn open_virtual_lane_channels(
    state: &AppState,
    graph: &PatchGraph,
    saved: &WorkspaceState,
    report: &mut PatchApplyReport,
) {
    let live = state.engine.snapshot();
    for (node, device_set, stream) in virtual_lane_channels(state, graph, &live) {
        let Some(NodeBody::Channel(channel)) = graph.node(&node).map(|patch| &patch.body) else {
            continue;
        };
        let Some(set) = live.device_sets.iter().find(|set| set.id == device_set) else {
            continue;
        };
        let open = set.channels.iter().any(|existing| {
            existing.stream == stream
                && existing.node.as_deref() == Some(node.as_str())
                && existing.settings.params.type_id() == channel.channel_type
        });
        if open {
            continue;
        }
        let center_hz = set
            .virtual_lanes
            .iter()
            .find(|lane| lane.stream == stream)
            .map(|lane| lane.center_hz);
        let Some(settings) = lane_channel(&node, &channel.channel_type, saved, center_hz) else {
            report.refused.push(PatchRefusal {
                node,
                reason: format!("this build has no channel type {:?}", channel.channel_type),
            });
            continue;
        };
        match state
            .engine
            .add_channel_for(device_set, stream, settings, Some(&node))
        {
            Ok(_) => report.created += 1,
            Err(error) => report.refused.push(PatchRefusal {
                node,
                reason: error.to_string(),
            }),
        }
    }
}

fn lane_channel(
    node: &str,
    channel_type: &str,
    saved: &WorkspaceState,
    center_hz: Option<f64>,
) -> Option<ChannelSettings> {
    if let Some(held) = saved
        .channel(node)
        .filter(|held| held.settings.params.type_id() == channel_type)
    {
        return Some(held.settings.clone());
    }
    let mut settings = ChannelSettings::default_for(channel_type)?;
    if let Some(center_hz) = center_hz {
        settings.frequency_hz = center_hz;
    }
    Some(settings)
}

pub(crate) fn capture_tunes(state: &AppState) -> Vec<WorkspaceArray> {
    state
        .engine
        .array_statuses()
        .into_iter()
        .filter(|status| status.center_hz.is_finite() && status.center_hz > 0.0)
        .map(|status| WorkspaceArray {
            node: status.node,
            tune: ArrayTune {
                center_hz: status.center_hz,
                gain: status.gain,
            },
        })
        .collect()
}

pub(crate) fn start_pump(state: &AppState) {
    if state.arrays.pumping.swap(true, Ordering::AcqRel) {
        return;
    }
    let events = state.engine.subscribe_arrays();
    let positions = state.gps.subscribe();
    let pumped = state.clone();
    let _detached = crate::spawn_task("sdrmm-arrays", move || pump::run(pumped, events, positions));
}

fn forward_pose(state: &AppState, position: &str, fix: Option<&PositionFix>) {
    let now = received_ns();
    for array in state.arrays.fed_by(position) {
        pose(state, &array, fix.cloned(), now);
    }
}

fn resend_poses(state: &AppState) {
    for position in state.arrays.positions() {
        let fix = state.gps.fix(&position);
        forward_pose(state, &position, fix.as_ref());
    }
}

#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn summary(state: &AppState, graph: &PatchGraph, node: &str) -> Option<ArraySummary> {
    let NodeBody::Array(settings) = &graph.node(node)?.body else {
        return None;
    };
    let binding = state.arrays.binding(node);
    let status = state
        .engine
        .array_statuses()
        .into_iter()
        .find(|status| status.node == node);
    let mut device_sets: Vec<u32> = Vec::new();
    let lanes = binding
        .as_ref()
        .and_then(|binding| binding.spec.as_ref())
        .map_or(&[][..], |spec| spec.lanes.as_slice());
    for lane in lanes.iter().flatten() {
        if !device_sets.contains(&lane.device_set) {
            device_sets.push(lane.device_set);
        }
    }
    let problem = wired_array(graph, node)
        .failure()
        .map(|failure| failure.to_string())
        .or_else(|| binding.as_ref().and_then(|binding| binding.problem.clone()))
        .or_else(|| {
            status
                .as_ref()
                .and_then(|status| status.failure.as_ref())
                .map(ToString::to_string)
        });
    Some(ArraySummary {
        device_sets,
        center_hz: status.as_ref().map(|status| status.center_hz),
        can_calibrate: status.is_some() && settings.cal.source != ArrayCalSource::Off,
        problem,
    })
}

fn refusal(state: &AppState, node: &str, error: EngineError) -> AppError {
    if !matches!(error, EngineError::ArrayNotFound(_)) {
        return error.into();
    }
    match state.arrays.binding(node) {
        Some(binding) => ArrayRefusal::Refused(
            binding
                .problem
                .unwrap_or_else(|| ArrayFailure::Unwired.to_string()),
        )
        .into(),
        None => ArrayRefusal::NoArray(node.to_owned()).into(),
    }
}

pub(crate) fn tune(
    state: &AppState,
    node: &str,
    request: ArrayTuneRequest,
) -> Result<(), AppError> {
    state
        .engine
        .tune_array(node, request)
        .map_err(|error| refusal(state, node, error))
}

pub(crate) fn calibrate(state: &AppState, node: &str) -> Result<(), AppError> {
    state
        .engine
        .calibrate_array(node)
        .map_err(|error| refusal(state, node, error))
}

fn recording(state: &AppState, node: &str) -> Result<bool, AppError> {
    state
        .engine
        .array_statuses()
        .into_iter()
        .find(|status| status.node == node)
        .map(|status| status.recording.is_some())
        .ok_or_else(|| refusal(state, node, EngineError::ArrayNotFound(node.to_owned())))
}

pub(crate) fn start_recording(
    state: &AppState,
    node: &str,
    request: ArrayRecordingRequest,
) -> Result<ArrayRecordingStarted, AppError> {
    if recording(state, node)? {
        return Err(ArrayRefusal::Refused(ALREADY_RECORDING.to_owned()).into());
    }
    let stem = state
        .engine
        .start_array_recording(node, request)
        .map_err(|error| refusal(state, node, error))?;
    Ok(ArrayRecordingStarted { stem })
}

pub(crate) fn stop_recording(state: &AppState, node: &str) -> Result<(), AppError> {
    if !recording(state, node)? {
        return Err(ArrayRefusal::NotRecording.into());
    }
    state
        .engine
        .stop_array_recording(node)
        .map_err(|error| refusal(state, node, error))
}

#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn array_pose(state: &AppState, node: &str) -> Option<ArrayPose> {
    let binding = state.arrays.binding(node)?;
    let orientation = binding.spec?.settings.orientation;
    let fix = state.gps.fix(binding.position_node.as_deref()?)?;
    let (heading_deg, heading_sigma_deg, heading_source) = match orientation {
        ArrayOrientation::Fixed { azimuth_deg } => {
            (Some(azimuth_deg.rem_euclid(360.0)), None, None)
        }
        ArrayOrientation::Heading { mount_offset_deg } => (
            fix.attitude
                .heading_deg
                .map(|heading| (heading + mount_offset_deg).rem_euclid(360.0)),
            fix.attitude.heading_accuracy_deg,
            fix.attitude.heading_source,
        ),
    };
    Some(ArrayPose {
        lat: fix.latitude,
        lon: fix.longitude,
        altitude_m: fix.altitude_m,
        heading_deg,
        heading_sigma_deg,
        heading_source,
    })
}

#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn processor_array(state: &AppState, processor: &str) -> Option<String> {
    state
        .arrays
        .processors()
        .get(processor)
        .map(|binding| binding.array.clone())
}
