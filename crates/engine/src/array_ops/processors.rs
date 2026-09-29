use std::sync::{Arc, TryLockError, atomic::Ordering};

use sdrmm_channels::{
    ChannelError,
    array_processor::{LaneFormat, ProcessorDescriptor, ProcessorNeeds, processor_descriptor},
};
use sdrmm_device::lock;
use sdrmm_wire::{GpuUse, ProcessorParams, ServerEvent, StateScope, array::MAX_VIRTUAL_LANES};

use super::{ArrayState, find, find_mut};
use crate::{
    DeviceSetState, Engine, EngineError, Inner, RebuildEntry, VirtualLaneState,
    array::{
        ArrayShape, Command, ControlConfig, HostPlan, HostSinks, LiveFrame, ProcessorAction,
        ProcessorHost, ProcessorSpec, ProcessorStats, RadarPlan, SteerInput, SteerMailbox,
        prepare_dedicated,
    },
    lock_runtime,
    runtime::{VirtualLaneSink, capture::RetiredLane},
};

const NO_TRACKS: &str = "No tracks here";
const NOT_RUNNING: &str = "Not running";
const NO_LANE_RADIO: &str = "No radio for lane outputs";
const TOO_MANY_LANES: &str = "Max 16 lane outputs";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LaneOut {
    pub(crate) port: usize,
    pub(crate) name: String,
    pub(crate) device_set: u32,
    pub(crate) stream: u32,
    pub(crate) format: LaneFormat,
}

#[derive(Clone)]
pub(crate) struct ProcessorRecord {
    pub(crate) spec: ProcessorSpec,
    pub(crate) kind: &'static str,
    pub(crate) lanes: Vec<LaneOut>,
    pub(crate) stats: Arc<ProcessorStats>,
    pub(crate) radar: Option<Arc<RadarPlan>>,
    pub(crate) needs: ProcessorNeeds,
    pub(crate) installed: bool,
    pub(crate) error: Option<String>,
}

impl ProcessorRecord {
    pub(crate) const fn needs_shape(&self) -> bool {
        self.needs.phase || self.needs.geometry
    }

    const fn takes_actions(&self) -> bool {
        matches!(self.spec.params, ProcessorParams::PassiveRadar(_)) || probe(&self.spec.params)
    }

    fn refused(spec: &ProcessorSpec, error: &EngineError) -> Self {
        Self {
            spec: spec.clone(),
            kind: spec.params.type_id(),
            lanes: Vec::new(),
            stats: Arc::new(ProcessorStats::default()),
            radar: None,
            needs: ProcessorNeeds::default(),
            installed: false,
            error: Some(error.to_string()),
        }
    }
}

#[cfg(feature = "probe")]
const fn probe(params: &ProcessorParams) -> bool {
    matches!(params, ProcessorParams::Probe(_))
}

#[cfg(not(feature = "probe"))]
const fn probe(_: &ProcessorParams) -> bool {
    false
}

fn gpu_of(params: &ProcessorParams) -> GpuUse {
    if let ProcessorParams::PassiveRadar(radar) = params {
        radar.gpu
    } else {
        GpuUse::Off
    }
}

struct View {
    frame: LiveFrame,
    shape: ArrayShape,
    anchor: Option<u32>,
    previous: Option<ProcessorRecord>,
    steer_in: SteerInput,
    steer_out: Arc<SteerMailbox>,
    sinks: HostSinks,
}

struct Opened {
    out: LaneOut,
    sink: VirtualLaneSink,
    reopened: bool,
}

fn wired_ports(
    descriptor: &ProcessorDescriptor,
    names: &[String],
) -> Result<Vec<(usize, String)>, EngineError> {
    let mut ports: Vec<(usize, String)> = Vec::with_capacity(names.len());
    for name in names {
        let port = descriptor
            .lane_ports
            .iter()
            .position(|port| port == name)
            .ok_or_else(|| EngineError::Processor(format!("No port {name}")))?;
        if !ports.iter().any(|(held, _)| *held == port) {
            ports.push((port, name.clone()));
        }
    }
    Ok(ports)
}

fn steer_input(inner: &mut Inner, array: &str, source: Option<&str>) -> SteerInput {
    let Some(source) = source else {
        return SteerInput::None;
    };
    if inner
        .processor_index
        .get(source)
        .is_some_and(|held| held == array)
    {
        SteerInput::Local(source.to_owned())
    } else {
        SteerInput::Remote(
            inner
                .steer_boxes
                .entry(source.to_owned())
                .or_default()
                .clone(),
        )
    }
}

pub(super) fn prune_steer_boxes(inner: &mut Inner) {
    let referenced: Vec<String> = inner
        .arrays
        .values()
        .flat_map(|state| state.processors.values())
        .filter_map(|record| record.spec.steer_from.clone())
        .collect();
    let Inner {
        steer_boxes,
        processor_index,
        ..
    } = inner;
    steer_boxes.retain(|node, _| processor_index.contains_key(node) || referenced.contains(node));
}

fn update_needs(state: &mut ArrayState) {
    let (time, phase) = state
        .processors
        .values()
        .filter(|record| record.installed)
        .fold((false, false), |(time, phase), record| {
            (time || record.needs.time, phase || record.needs.phase)
        });
    let config = ControlConfig {
        needs_time: time,
        needs_phase: phase,
        ..state.config.clone()
    };
    state.configure(config);
    if state.frame.needs_time != time {
        let frame = LiveFrame {
            needs_time: time,
            ..state.frame.clone()
        };
        if let Err(error) = state.reframe(frame) {
            tracing::warn!(array = %state.spec.node, %error, "array missed a new frame");
        }
    }
}

fn free_stream(state: &DeviceSetState) -> Result<u32, EngineError> {
    if state.virtual_lanes.len() >= MAX_VIRTUAL_LANES as usize {
        return Err(ChannelError::Refused(TOO_MANY_LANES).into());
    }
    (state.physical_streams()..u32::MAX)
        .find(|stream| !state.virtual_lanes.contains_key(stream))
        .ok_or_else(|| ChannelError::Refused(TOO_MANY_LANES).into())
}

fn processor<'a>(
    inner: &'a Inner,
    node: &str,
) -> Result<(&'a ArrayState, &'a ProcessorRecord), EngineError> {
    let missing = || EngineError::ProcessorNotFound(node.to_owned());
    let array = inner.processor_index.get(node).ok_or_else(missing)?;
    let state = inner.arrays.get(array).ok_or_else(missing)?;
    let record = state.processors.get(node).ok_or_else(missing)?;
    Ok((state, record))
}

impl Engine {
    pub fn apply_processor(&self, spec: ProcessorSpec) -> Result<(), EngineError> {
        let _edits = lock(&self.array_edits);
        let moved = self
            .lock()
            .processor_index
            .get(&spec.node)
            .is_some_and(|array| *array != spec.array);
        if moved {
            self.drop_processor(&spec.node)?;
        }
        let installed = self.install_processor(&spec, None, false);
        if let Err(error) = &installed {
            self.note_processor_refusal(&spec, error);
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Arrays,
        });
        installed
    }

    pub fn remove_processor(&self, node: &str) -> Result<(), EngineError> {
        let _edits = lock(&self.array_edits);
        self.drop_processor(node)
    }

    pub fn retain_processors(&self, drawn: &[String]) {
        let _edits = lock(&self.array_edits);
        let gone: Vec<String> = self
            .lock()
            .processor_index
            .keys()
            .filter(|node| !drawn.contains(node))
            .cloned()
            .collect();
        for node in gone {
            if let Err(error) = self.drop_processor(&node) {
                tracing::warn!(processor = %node, %error, "a processor left the patch but did not stop cleanly");
            }
        }
    }

    pub fn processor_action(&self, node: &str, action: ProcessorAction) -> Result<(), EngineError> {
        let inner = self.lock();
        let (state, record) = processor(&inner, node)?;
        if !record.takes_actions() {
            return Err(EngineError::Processor(NO_TRACKS.to_owned()));
        }
        if !record.installed {
            return Err(EngineError::Processor(
                record
                    .error
                    .clone()
                    .unwrap_or_else(|| NOT_RUNNING.to_owned()),
            ));
        }
        state.send(Command::Action {
            node: node.to_owned(),
            action,
        })
    }

    fn note_processor_refusal(&self, spec: &ProcessorSpec, error: &EngineError) {
        let mut inner = self.lock();
        let Some(state) = inner.arrays.get_mut(&spec.array) else {
            return;
        };
        state
            .processors
            .entry(spec.node.clone())
            .and_modify(|record| record.error = Some(error.to_string()))
            .or_insert_with(|| ProcessorRecord::refused(spec, error));
        inner
            .processor_index
            .insert(spec.node.clone(), spec.array.clone());
        inner.revision += 1;
    }

    fn drop_processor(&self, node: &str) -> Result<(), EngineError> {
        let (record, sent) = {
            let mut inner = self.lock();
            let missing = || EngineError::ProcessorNotFound(node.to_owned());
            let array = inner.processor_index.remove(node).ok_or_else(missing)?;
            let state = inner.arrays.get_mut(&array).ok_or_else(missing)?;
            let record = state.processors.remove(node).ok_or_else(missing)?;
            let sent = if record.installed {
                state.send(Command::RemoveHost {
                    node: node.to_owned(),
                })
            } else {
                Ok(())
            };
            update_needs(state);
            prune_steer_boxes(&mut inner);
            inner.revision += 1;
            (record, sent)
        };
        for lane in &record.lanes {
            self.close_lane(lane.device_set, lane.stream);
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Arrays,
        });
        sent
    }

    pub(super) fn forget_processors(&self, records: &[ProcessorRecord]) {
        {
            let mut inner = self.lock();
            for record in records {
                inner.processor_index.remove(&record.spec.node);
            }
            prune_steer_boxes(&mut inner);
        }
        for lane in records.iter().flat_map(|record| &record.lanes) {
            self.close_lane(lane.device_set, lane.stream);
        }
    }

    pub(super) fn reinstall(&self, array: &str, record: ProcessorRecord) {
        let spec = ProcessorSpec {
            array: array.to_owned(),
            ..record.spec.clone()
        };
        if let Err(error) = self.install_processor(&spec, Some(record), true) {
            tracing::warn!(processor = %spec.node, %error, "a processor did not come back after its array restarted");
            self.note_processor_refusal(&spec, &error);
        }
    }

    pub(crate) fn rebuild_processors(&self, array: &str) -> Result<(), EngineError> {
        let _edits = match self.array_edits.try_lock() {
            Ok(edits) => edits,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Ok(()),
        };
        let due: Vec<ProcessorSpec> =
            self.lock()
                .arrays
                .get(array)
                .map_or_else(Vec::new, |state| {
                    state
                        .processors
                        .values()
                        .filter(|record| record.installed && record.stats.rebuild_due())
                        .map(|record| record.spec.clone())
                        .collect()
                });
        if due.is_empty() {
            return Ok(());
        }
        let mut failed = None;
        for spec in &due {
            if let Err(error) = self.install_processor(spec, None, true) {
                self.note_processor_refusal(spec, &error);
                failed.get_or_insert(error);
            }
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Arrays,
        });
        failed.map_or(Ok(()), Err)
    }

    pub(super) fn reinstall_all(&self, array: &str, wanted: impl Fn(&ProcessorRecord) -> bool) {
        let specs: Vec<ProcessorSpec> =
            self.lock()
                .arrays
                .get(array)
                .map_or_else(Vec::new, |state| {
                    state
                        .processors
                        .values()
                        .filter(|record| record.installed && wanted(record))
                        .map(|record| record.spec.clone())
                        .collect()
                });
        for spec in specs {
            if let Err(error) = self.install_processor(&spec, None, true) {
                tracing::warn!(processor = %spec.node, %error, "a processor could not be rebuilt");
                self.note_processor_refusal(&spec, &error);
            }
        }
    }

    fn install_processor(
        &self,
        spec: &ProcessorSpec,
        carried: Option<ProcessorRecord>,
        force: bool,
    ) -> Result<(), EngineError> {
        let type_id = spec.params.type_id();
        let descriptor = processor_descriptor(type_id)
            .ok_or_else(|| ChannelError::UnknownType(type_id.to_owned()))?;
        if let Some(problem) = spec.params.problem() {
            return Err(ChannelError::Refused(problem).into());
        }
        let ports = wired_ports(descriptor, &spec.lane_ports)?;
        let view = self.view(spec, carried)?;
        if !force && self.apply_in_place(spec, &view, descriptor, &ports)? {
            return Ok(());
        }
        self.build_processor(spec, view, descriptor, &ports)
    }

    fn view(
        &self,
        spec: &ProcessorSpec,
        carried: Option<ProcessorRecord>,
    ) -> Result<View, EngineError> {
        let mut inner = self.lock();
        let (frame, shape, anchor, previous) = {
            let state = find(&inner, &spec.array)?;
            (
                state.frame.clone(),
                state.shape.clone(),
                state.anchor,
                carried.or_else(|| state.processors.get(&spec.node).cloned()),
            )
        };
        let steer_in = steer_input(&mut inner, &spec.array, spec.steer_from.as_deref());
        let steer_out = inner
            .steer_boxes
            .entry(spec.node.clone())
            .or_default()
            .clone();
        Ok(View {
            sinks: HostSinks {
                events: self.array_tx.clone(),
                decoded: self.decoded_tx_out.clone(),
                decoded_lost: self.decoded_dropped.clone(),
                anchor: anchor.unwrap_or_default(),
            },
            frame,
            shape,
            anchor,
            previous,
            steer_in,
            steer_out,
        })
    }

    fn apply_in_place(
        &self,
        spec: &ProcessorSpec,
        view: &View,
        descriptor: &ProcessorDescriptor,
        ports: &[(usize, String)],
    ) -> Result<bool, EngineError> {
        let Some(previous) = view.previous.as_ref().filter(|record| record.installed) else {
            return Ok(false);
        };
        let ctx = view.shape.ctx(&spec.node, &view.frame);
        let same_lanes = previous.lanes.len() == ports.len()
            && previous.lanes.iter().all(|lane| {
                Some(lane.device_set) == view.anchor
                    && ports.iter().any(|(port, _)| {
                        *port == lane.port
                            && (descriptor.lane_format)(&spec.params, &ctx, *port) == lane.format
                    })
            });
        if previous.kind != descriptor.type_id
            || previous.spec.steer_from != spec.steer_from
            || !same_lanes
        {
            return Ok(false);
        }
        let (command, radar) = if previous.spec.params == spec.params {
            (None, None)
        } else if let Some(radar) = &previous.radar {
            let (prepared, plan) =
                prepare_dedicated(radar, &ctx, &spec.params, gpu_of(&spec.params))?;
            let commit = Command::CommitDedicated {
                node: spec.node.clone(),
                prepared,
            };
            (Some(commit), Some(plan))
        } else if (descriptor.in_place)(&previous.spec.params, &spec.params) {
            let apply = Command::ApplyParams {
                node: spec.node.clone(),
                params: Box::new(spec.params.clone()),
            };
            (Some(apply), None)
        } else {
            return Ok(false);
        };
        let mut inner = self.lock();
        let state = find_mut(&mut inner, &spec.array)?;
        if let Some(command) = command {
            state.send(command)?;
        }
        if let Some(record) = state.processors.get_mut(&spec.node) {
            record.spec = spec.clone();
            record.needs = (descriptor.needs)(&spec.params);
            record.error = None;
            if radar.is_some() {
                record.radar = radar;
            }
        }
        update_needs(state);
        inner.revision += 1;
        Ok(true)
    }

    fn build_processor(
        &self,
        spec: &ProcessorSpec,
        view: View,
        descriptor: &'static ProcessorDescriptor,
        ports: &[(usize, String)],
    ) -> Result<(), EngineError> {
        let stats = view.previous.as_ref().map_or_else(
            || Arc::new(ProcessorStats::default()),
            |record| record.stats.clone(),
        );
        let plan = HostPlan {
            node: spec.node.clone(),
            params: spec.params.clone(),
            shape: view.shape.clone(),
            sinks: Vec::new(),
            outputs: view.sinks.clone(),
            steer_in: view.steer_in,
            steer_out: view.steer_out.clone(),
            stats: stats.clone(),
            gpu: gpu_of(&spec.params),
        };
        let built = ProcessorHost::build(plan, &view.frame)?;
        let ctx = view.shape.ctx(&spec.node, &view.frame);
        let formats: Vec<(usize, String, LaneFormat)> = ports
            .iter()
            .map(|(port, name)| {
                (
                    *port,
                    name.clone(),
                    (descriptor.lane_format)(&spec.params, &ctx, *port),
                )
            })
            .collect();
        let opened = self.open_lanes(spec, view.anchor, view.previous.as_ref(), &formats)?;
        let record = ProcessorRecord {
            spec: spec.clone(),
            kind: descriptor.type_id,
            lanes: opened.iter().map(|lane| lane.out.clone()).collect(),
            stats,
            radar: built.radar,
            needs: (descriptor.needs)(&spec.params),
            installed: true,
            error: None,
        };
        let rebuild: Vec<(u32, u32)> = opened
            .iter()
            .filter(|lane| lane.reopened)
            .map(|lane| (lane.out.device_set, lane.out.stream))
            .collect();
        let kept = record.lanes.clone();
        let committed = self.commit_host(spec, built.host, opened, record);
        let stale: Vec<(u32, u32)> = view
            .previous
            .iter()
            .flat_map(|previous| &previous.lanes)
            .filter(|lane| {
                !kept
                    .iter()
                    .any(|held| held.device_set == lane.device_set && held.stream == lane.stream)
            })
            .map(|lane| (lane.device_set, lane.stream))
            .collect();
        match committed {
            Ok(()) => {
                for (ds, stream) in rebuild {
                    self.rebuild_lane_channels(ds, stream);
                }
                for (ds, stream) in stale {
                    self.close_lane(ds, stream);
                }
                Ok(())
            }
            Err(error) => {
                for lane in kept
                    .iter()
                    .filter(|lane| !rebuild.contains(&(lane.device_set, lane.stream)))
                {
                    self.close_lane(lane.device_set, lane.stream);
                }
                Err(error)
            }
        }
    }

    fn commit_host(
        &self,
        spec: &ProcessorSpec,
        host: Box<ProcessorHost>,
        opened: Vec<Opened>,
        record: ProcessorRecord,
    ) -> Result<(), EngineError> {
        let mut inner = self.lock();
        let state = find_mut(&mut inner, &spec.array)?;
        record.stats.replacing.store(true, Ordering::Release);
        if let Err(error) = state.send(Command::ReplaceHost { host }) {
            record.stats.replacing.store(false, Ordering::Release);
            return Err(error);
        }
        let mut lost = None;
        for lane in opened {
            let swapped = state.send(Command::SwapVirtual {
                node: spec.node.clone(),
                port: lane.out.port,
                sink: lane.sink,
            });
            if let Err(error) = swapped {
                lost = Some(error.to_string());
            }
        }
        state.processors.insert(
            spec.node.clone(),
            ProcessorRecord {
                error: lost,
                ..record
            },
        );
        update_needs(state);
        inner
            .processor_index
            .insert(spec.node.clone(), spec.array.clone());
        inner.revision += 1;
        Ok(())
    }

    fn open_lanes(
        &self,
        spec: &ProcessorSpec,
        anchor: Option<u32>,
        previous: Option<&ProcessorRecord>,
        formats: &[(usize, String, LaneFormat)],
    ) -> Result<Vec<Opened>, EngineError> {
        if formats.is_empty() {
            return Ok(Vec::new());
        }
        let anchor = anchor.ok_or_else(|| EngineError::Processor(NO_LANE_RADIO.to_owned()))?;
        let mut opened: Vec<Opened> = Vec::with_capacity(formats.len());
        for (port, name, format) in formats {
            let reuse = previous
                .and_then(|record| {
                    record
                        .lanes
                        .iter()
                        .find(|lane| lane.name == *name && lane.device_set == anchor)
                })
                .map(|lane| lane.stream);
            match self.open_lane(anchor, reuse, &spec.node, name, *format) {
                Ok((stream, sink)) => opened.push(Opened {
                    out: LaneOut {
                        port: *port,
                        name: name.clone(),
                        device_set: anchor,
                        stream,
                        format: *format,
                    },
                    sink,
                    reopened: reuse.is_some(),
                }),
                Err(error) => {
                    for lane in opened.iter().filter(|lane| !lane.reopened) {
                        self.close_lane(anchor, lane.out.stream);
                    }
                    return Err(error);
                }
            }
        }
        Ok(opened)
    }

    pub(super) fn open_lane(
        &self,
        ds: u32,
        reuse: Option<u32>,
        node: &str,
        port: &str,
        format: LaneFormat,
    ) -> Result<(u32, VirtualLaneSink), EngineError> {
        let (opened, retired) = self.swap_lane(ds, reuse, node, port, format);
        drop(retired);
        opened
    }

    fn swap_lane(
        &self,
        ds: u32,
        reuse: Option<u32>,
        node: &str,
        port: &str,
        format: LaneFormat,
    ) -> (
        Result<(u32, VirtualLaneSink), EngineError>,
        Option<RetiredLane>,
    ) {
        let mut inner = self.lock();
        let Some(state) = inner.device_sets.get_mut(&ds) else {
            return (Err(EngineError::DeviceSetNotFound(ds)), None);
        };
        let stream = match reuse.map_or_else(|| free_stream(state), Ok) {
            Ok(stream) => stream,
            Err(error) => return (Err(error), None),
        };
        let (retired, added) = {
            let mut runtime = lock_runtime(&state.runtime);
            let retired = runtime.remove_virtual_lane(stream);
            let added =
                runtime.add_virtual_lane(stream, format.center_hz, format.sample_rate, false);
            (retired, added)
        };
        let opened = match added {
            Ok((sink, cmd_tx)) => {
                state.virtual_lanes.insert(
                    stream,
                    VirtualLaneState {
                        node: node.to_owned(),
                        port: port.to_owned(),
                        center_hz: format.center_hz,
                        sample_rate: format.sample_rate,
                        cmd_tx,
                    },
                );
                Ok((stream, sink))
            }
            Err(error) => {
                state.virtual_lanes.remove(&stream);
                Err(error.into())
            }
        };
        inner.revision += 1;
        (opened, retired)
    }

    pub(super) fn close_lane(&self, ds: u32, stream: u32) {
        let channels: Vec<u32> = self
            .lock()
            .device_sets
            .get(&ds)
            .map_or_else(Vec::new, |state| {
                state
                    .channels
                    .iter()
                    .filter(|channel| channel.stream == stream)
                    .map(|channel| channel.id)
                    .collect()
            });
        for channel in channels {
            if let Err(error) = self.remove_channel(ds, channel) {
                tracing::warn!(ds, channel, %error, "a channel on a lane output did not close");
            }
        }
        let retired = {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            if state.virtual_lanes.remove(&stream).is_none() {
                return;
            }
            let retired = lock_runtime(&state.runtime).remove_virtual_lane(stream);
            inner.revision += 1;
            retired
        };
        drop(retired);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
    }

    fn rebuild_lane_channels(&self, ds: u32, stream: u32) {
        let rebuilds: Vec<RebuildEntry> =
            self.lock()
                .device_sets
                .get(&ds)
                .map_or_else(Vec::new, |state| {
                    state
                        .channels
                        .iter()
                        .filter(|channel| channel.stream == stream)
                        .filter_map(|channel| {
                            state.media.get(&channel.id).map(|media| RebuildEntry {
                                id: channel.id,
                                stream,
                                settings: channel.settings.clone(),
                                sinks: media.sinks.clone(),
                            })
                        })
                        .collect()
                });
        let mut dead = Vec::new();
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, &mut dead);
        }
        for media in dead {
            media.shutdown();
        }
    }

    pub(super) fn refresh_lanes(&self, array: &str) {
        let (stale, moved) = {
            let mut inner = self.lock();
            let Some(state) = inner.arrays.get_mut(array) else {
                return;
            };
            let frame = state.frame.clone();
            let shape = state.shape.clone();
            let mut stale: Vec<String> = Vec::new();
            let mut moved: Vec<(u32, u32, LaneFormat)> = Vec::new();
            for record in state.processors.values_mut() {
                let Some(descriptor) = processor_descriptor(record.kind) else {
                    continue;
                };
                let ctx = shape.ctx(&record.spec.node, &frame);
                for lane in &mut record.lanes {
                    let format = (descriptor.lane_format)(&record.spec.params, &ctx, lane.port);
                    if format.sample_rate != lane.format.sample_rate {
                        stale.push(record.spec.node.clone());
                        break;
                    }
                    if format.center_hz != lane.format.center_hz {
                        lane.format = format;
                        moved.push((lane.device_set, lane.stream, format));
                    }
                }
            }
            (stale, moved)
        };
        for (ds, stream, format) in moved {
            self.move_lane(ds, stream, format);
        }
        if !stale.is_empty() {
            self.reinstall_all(array, |record| stale.contains(&record.spec.node));
        }
    }

    fn move_lane(&self, ds: u32, stream: u32, format: LaneFormat) {
        let mut inner = self.lock();
        let Some(state) = inner.device_sets.get_mut(&ds) else {
            return;
        };
        let Some(lane) = state.virtual_lanes.get_mut(&stream) else {
            return;
        };
        lane.center_hz = format.center_hz;
        lane.sample_rate = format.sample_rate;
        lock_runtime(&state.runtime).set_virtual_meta(stream, format.center_hz, format.sample_rate);
        inner.revision += 1;
    }

    pub(super) fn reattach_lanes(&self, array: &str, ds: u32) {
        let lanes: Vec<(String, LaneOut)> =
            self.lock()
                .arrays
                .get(array)
                .map_or_else(Vec::new, |state| {
                    state
                        .processors
                        .values()
                        .filter(|record| record.installed)
                        .flat_map(|record| {
                            record
                                .lanes
                                .iter()
                                .filter(|lane| lane.device_set == ds)
                                .map(|lane| (record.spec.node.clone(), lane.clone()))
                        })
                        .collect()
                });
        for (node, lane) in lanes {
            let reattached = self
                .open_lane(ds, Some(lane.stream), &node, &lane.name, lane.format)
                .and_then(|(_, sink)| {
                    find(&self.lock(), array)?.send(Command::SwapVirtual {
                        node: node.clone(),
                        port: lane.port,
                        sink,
                    })
                });
            if let Err(error) = reattached {
                tracing::warn!(processor = %node, %error, "a lane output did not come back after a replug");
                self.note_lane_loss(array, &node, &error);
            }
        }
    }

    fn note_lane_loss(&self, array: &str, node: &str, error: &EngineError) {
        let mut inner = self.lock();
        let Some(record) = inner
            .arrays
            .get_mut(array)
            .and_then(|state| state.processors.get_mut(node))
        else {
            return;
        };
        record.error = Some(error.to_string());
        inner.revision += 1;
    }
}
