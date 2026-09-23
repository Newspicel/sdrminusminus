use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use sdrmm_engine::Engine;
use sdrmm_wire::{
    AudioRoute, MAX_AUDIO_FX_CHAIN, NodeBody, PatchGraph, RecorderNode, StateScope, StateSnapshot,
    port_stream,
};

use crate::{
    gps::GpsHub,
    rest::{lock_gate, reconcile_recordings},
    store::{Store, StoreError},
    workspace::{bind, bind_devices},
};

const SWITCH_ATTEMPTS: usize = 3;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SwitchError {
    #[error("no workspace is open")]
    NoWorkspace,
    #[error("no recorder `{0}` in the open workspace")]
    NoRecorder(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn switch_of(body: &mut NodeBody) -> Option<&mut RecorderNode> {
    match body {
        NodeBody::Recorder(recorder)
        | NodeBody::AudioRecorder(recorder)
        | NodeBody::BasebandRecorder(recorder) => Some(recorder),
        _ => None,
    }
}

pub(crate) fn switch(store: &Store, node: &str, recording: bool) -> Result<(), SwitchError> {
    let mut attempt = 0;
    loop {
        let mut workspace = store.active_workspace()?.ok_or(SwitchError::NoWorkspace)?;
        let recorder = workspace
            .snapshot
            .graph
            .nodes
            .iter_mut()
            .filter(|found| found.id == node)
            .find_map(|found| switch_of(&mut found.body))
            .ok_or_else(|| SwitchError::NoRecorder(node.to_owned()))?;
        if recorder.recording == recording {
            return Ok(());
        }
        recorder.recording = recording;
        let update = sdrmm_wire::UpdateWorkspaceRequest {
            revision: workspace.info.revision,
            name: None,
            snapshot: Some(workspace.snapshot),
        };
        match store.update_workspace(workspace.info.id, &update) {
            Ok(_) => return Ok(()),
            Err(StoreError::WorkspaceConflict { .. }) if attempt + 1 < SWITCH_ATTEMPTS => {
                attempt += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(crate) struct Hooks {
    pub(crate) store: Arc<Store>,
    pub(crate) gate: Arc<Mutex<()>>,
    pub(crate) gps: Arc<GpsHub>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Target {
    Audio(AudioRoute),
    Iq { device_set: u32 },
    Baseband { device_set: u32, channel: u32 },
}

#[derive(Default)]
pub(crate) struct Recorders {
    failures: HashMap<Target, String>,
}

struct Tick<'a> {
    engine: &'a Arc<Engine>,
    hooks: &'a Hooks,
    finished: bool,
    moved_iq: bool,
}

impl Recorders {
    pub(crate) fn reconcile(
        &mut self,
        engine: &Arc<Engine>,
        graph: Option<&PatchGraph>,
        hooks: &Hooks,
    ) {
        let snapshot = engine.snapshot();
        let empty = PatchGraph::default();
        let graph = graph.unwrap_or(&empty);
        let mut tick = Tick {
            engine,
            hooks,
            finished: false,
            moved_iq: false,
        };
        let mut wanted_targets = HashSet::new();
        self.audio(&mut tick, graph, &snapshot, &mut wanted_targets);
        self.baseband(&mut tick, graph, &snapshot, &mut wanted_targets);
        self.iq(&mut tick, graph, &snapshot, &mut wanted_targets);
        self.failures
            .retain(|target, _| wanted_targets.contains(target));
        tick.finish();
    }

    fn audio(
        &mut self,
        tick: &mut Tick<'_>,
        graph: &PatchGraph,
        snapshot: &StateSnapshot,
        targets: &mut HashSet<Target>,
    ) {
        let wanted = wanted_audio(graph, snapshot);
        let current = running_audio(snapshot);
        for route in current.difference(&wanted) {
            match tick.engine.stop_route_recording(route) {
                Ok(_) => tick.finished = true,
                Err(error) => tracing::warn!(?route, %error, "could not stop an audio recording"),
            }
        }
        for route in wanted.difference(&current) {
            let result = tick.engine.start_route_recording(route).map(|_| ());
            self.outcome(Target::Audio(route.clone()), result);
        }
        targets.extend(wanted.into_iter().map(Target::Audio));
    }

    fn baseband(
        &mut self,
        tick: &mut Tick<'_>,
        graph: &PatchGraph,
        snapshot: &StateSnapshot,
        targets: &mut HashSet<Target>,
    ) {
        let wanted = wanted_baseband(graph, snapshot);
        let current = running_baseband(snapshot);
        for &(device_set, channel) in current.difference(&wanted) {
            match tick
                .engine
                .stop_channel_baseband_recording(device_set, channel)
            {
                Ok(_) => tick.finished = true,
                Err(error) => {
                    tracing::warn!(device_set, channel, %error, "could not stop a baseband recording");
                }
            }
        }
        for &(device_set, channel) in wanted.difference(&current) {
            let result = tick
                .engine
                .start_channel_baseband_recording(device_set, channel)
                .map(|_| ());
            self.outcome(
                Target::Baseband {
                    device_set,
                    channel,
                },
                result,
            );
        }
        targets.extend(
            wanted
                .into_iter()
                .map(|(device_set, channel)| Target::Baseband {
                    device_set,
                    channel,
                }),
        );
    }

    fn iq(
        &mut self,
        tick: &mut Tick<'_>,
        graph: &PatchGraph,
        snapshot: &StateSnapshot,
        targets: &mut HashSet<Target>,
    ) {
        let wanted = wanted_iq(graph, snapshot);
        let current = running_iq(snapshot);
        for (&device_set, &stream) in &current {
            if wanted.get(&device_set) == Some(&stream) {
                continue;
            }
            match tick.engine.stop_recording(device_set) {
                Ok(_) => {
                    tick.finished = true;
                    tick.moved_iq = true;
                }
                Err(error) => tracing::warn!(device_set, %error, "could not stop an IQ recording"),
            }
        }
        for (&device_set, &stream) in &wanted {
            if current.get(&device_set) == Some(&stream) {
                continue;
            }
            let result = tick.engine.start_recording(device_set, stream);
            if result.is_ok() {
                tick.moved_iq = true;
            }
            self.outcome(Target::Iq { device_set }, result);
        }
        targets.extend(
            wanted
                .into_keys()
                .map(|device_set| Target::Iq { device_set }),
        );
    }

    fn outcome(&mut self, target: Target, result: Result<(), sdrmm_engine::EngineError>) {
        match result {
            Ok(()) => {
                self.failures.remove(&target);
            }
            Err(error) => {
                let error = error.to_string();
                if self.failures.get(&target) != Some(&error) {
                    tracing::warn!(?target, %error, "recorder cannot record");
                    self.failures.insert(target, error);
                }
            }
        }
    }
}

impl Tick<'_> {
    fn finish(self) {
        if self.moved_iq {
            self.hooks.gps.route_for(self.engine, &self.hooks.store);
        }
        if !self.finished {
            return;
        }
        if let Some(dir) = self.engine.recordings_dir() {
            let _gate = lock_gate(&self.hooks.gate);
            if let Err(error) = reconcile_recordings(dir, &self.hooks.store) {
                tracing::warn!(?error, "could not index a finished recording");
            }
        }
        self.engine.emit_scope(StateScope::Recordings);
    }
}

fn recording(body: &NodeBody) -> bool {
    match body {
        NodeBody::Recorder(recorder)
        | NodeBody::AudioRecorder(recorder)
        | NodeBody::BasebandRecorder(recorder) => recorder.recording,
        _ => false,
    }
}

fn switched_on<'a>(
    graph: &'a PatchGraph,
    kind: &'a str,
) -> impl Iterator<Item = &'a sdrmm_wire::PatchNode> {
    graph
        .nodes
        .iter()
        .filter(move |node| node.body.kind() == kind && recording(&node.body))
}

fn channel_bindings(graph: &PatchGraph, snapshot: &StateSnapshot) -> Vec<(String, u32, u32)> {
    bind(graph, snapshot)
        .into_iter()
        .flat_map(|binding| {
            binding
                .channels
                .into_iter()
                .map(move |(node, channel)| (node, binding.device_set, channel))
        })
        .collect()
}

fn running_audio(snapshot: &StateSnapshot) -> HashSet<AudioRoute> {
    snapshot
        .device_sets
        .iter()
        .flat_map(|set| {
            set.channels.iter().flat_map(move |channel| {
                channel
                    .audio_recordings
                    .iter()
                    .map(move |status| AudioRoute {
                        device_set: set.id,
                        channel: channel.id,
                        fx: status.fx.clone(),
                    })
            })
        })
        .collect()
}

fn wanted_audio(graph: &PatchGraph, snapshot: &StateSnapshot) -> HashSet<AudioRoute> {
    let channels = channel_bindings(graph, snapshot);
    switched_on(graph, "audio_recorder")
        .flat_map(|node| audio_paths_into(graph, &node.id))
        .flat_map(|(channel_node, fx)| {
            channels
                .iter()
                .filter(|(node, _, _)| *node == channel_node)
                .map(|&(_, device_set, channel)| AudioRoute {
                    device_set,
                    channel,
                    fx: fx.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn running_baseband(snapshot: &StateSnapshot) -> HashSet<(u32, u32)> {
    snapshot
        .device_sets
        .iter()
        .flat_map(|set| {
            set.channels
                .iter()
                .filter(|channel| channel.baseband_recording.is_some())
                .map(move |channel| (set.id, channel.id))
        })
        .collect()
}

fn wanted_baseband(graph: &PatchGraph, snapshot: &StateSnapshot) -> HashSet<(u32, u32)> {
    let channels = channel_bindings(graph, snapshot);
    switched_on(graph, "baseband_recorder")
        .flat_map(|node| graph.sources_of(&node.id, "baseband"))
        .flat_map(|source| {
            channels
                .iter()
                .filter(move |(node, _, _)| node == source)
                .map(|&(_, device_set, channel)| (device_set, channel))
        })
        .collect()
}

fn running_iq(snapshot: &StateSnapshot) -> HashMap<u32, u32> {
    snapshot
        .device_sets
        .iter()
        .filter_map(|set| set.recording.as_ref().map(|status| (set.id, status.stream)))
        .collect()
}

fn wanted_iq(graph: &PatchGraph, snapshot: &StateSnapshot) -> HashMap<u32, u32> {
    let devices: HashMap<String, u32> = bind_devices(graph, snapshot).into_iter().collect();
    let mut wanted = HashMap::new();
    for node in switched_on(graph, "recorder") {
        for edge in graph
            .edges
            .iter()
            .filter(|edge| edge.to.node == node.id && edge.to.port == "iq")
        {
            let (Some(&device_set), Some(stream)) = (
                devices.get(&edge.from.node),
                port_stream("iq", &edge.from.port),
            ) else {
                continue;
            };
            wanted.entry(device_set).or_insert(stream);
        }
    }
    wanted
}

pub(crate) fn audio_paths_into(graph: &PatchGraph, sink: &str) -> Vec<(String, Vec<String>)> {
    let mut paths = Vec::new();
    walk(graph, sink, &mut Vec::new(), &mut paths);
    paths
}

fn walk(
    graph: &PatchGraph,
    node: &str,
    fx: &mut Vec<String>,
    paths: &mut Vec<(String, Vec<String>)>,
) {
    if fx.len() > MAX_AUDIO_FX_CHAIN {
        return;
    }
    for source in graph.sources_of(node, "audio") {
        match graph.node(source).map(|found| &found.body) {
            Some(NodeBody::AudioFx(_)) => {
                fx.push(source.to_owned());
                walk(graph, source, fx, paths);
                fx.pop();
            }
            Some(NodeBody::Channel(_)) => {
                paths.push((source.to_owned(), fx.iter().rev().cloned().collect()));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        AudioFxNode, ChannelNode, DeviceNode, DeviceRef, PatchEdge, PatchNode, PortRef, Position,
    };

    use super::*;

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn wire(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
        PatchEdge {
            from: PortRef {
                node: from.0.to_owned(),
                port: from.1.to_owned(),
            },
            to: PortRef {
                node: to.0.to_owned(),
                port: to.1.to_owned(),
            },
        }
    }

    fn audio(from: &str, to: &str) -> PatchEdge {
        wire((from, "audio"), (to, "audio"))
    }

    fn channel(id: &str) -> PatchNode {
        node(
            id,
            NodeBody::Channel(ChannelNode {
                channel_type: "nfm".to_owned(),
                record_calls: false,
                tuning_locked: false,
            }),
        )
    }

    fn fx(id: &str) -> PatchNode {
        node(id, NodeBody::AudioFx(AudioFxNode::default()))
    }

    fn on(recording: bool) -> RecorderNode {
        RecorderNode { recording }
    }

    #[test]
    fn a_recorder_sees_each_channel_through_the_fx_chain_in_front_of_it() {
        let graph = PatchGraph {
            nodes: vec![
                channel("a"),
                channel("b"),
                fx("near"),
                fx("far"),
                node("rec", NodeBody::AudioRecorder(on(true))),
            ],
            edges: vec![
                audio("a", "near"),
                audio("near", "far"),
                audio("far", "rec"),
                audio("b", "rec"),
            ],
        };
        let mut paths = audio_paths_into(&graph, "rec");
        paths.sort();
        assert_eq!(
            paths,
            vec![
                ("a".to_owned(), vec!["near".to_owned(), "far".to_owned()]),
                ("b".to_owned(), Vec::new()),
            ]
        );
    }

    #[test]
    fn switching_any_recorder_is_stored_in_the_workspace() {
        let store = Store::open(None).expect("store");
        let mut snapshot = sdrmm_wire::WorkspaceSnapshot::empty();
        snapshot.graph.nodes.extend([
            node("iq", NodeBody::Recorder(on(false))),
            node("base", NodeBody::BasebandRecorder(on(false))),
            node("audio", NodeBody::AudioRecorder(on(false))),
        ]);
        let id = store.create_workspace("w", &snapshot).expect("workspace");
        store.activate_workspace(id).expect("activate");
        for recorder in ["iq", "base", "audio"] {
            switch(&store, recorder, true).expect("switch on");
        }
        let stored = store.workspace(id).expect("read");
        assert!(
            stored
                .snapshot
                .graph
                .nodes
                .iter()
                .all(|found| recording(&found.body))
        );
        assert!(matches!(
            switch(&store, "nope", true),
            Err(SwitchError::NoRecorder(_))
        ));
    }

    fn patch(recording: bool) -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node(
                    "dev",
                    NodeBody::Device(DeviceNode {
                        device: Some(DeviceRef {
                            backend: "virtual".to_owned(),
                            serial: None,
                            key: None,
                        }),
                        ..DeviceNode::default()
                    }),
                ),
                channel("ch"),
                node("iq", NodeBody::Recorder(on(recording))),
                node("base", NodeBody::BasebandRecorder(on(recording))),
                node("audio", NodeBody::AudioRecorder(on(recording))),
            ],
            edges: vec![
                wire(("dev", "iq"), ("ch", "iq")),
                wire(("dev", "iq"), ("iq", "iq")),
                wire(("ch", "baseband"), ("base", "baseband")),
                audio("ch", "audio"),
            ],
        }
    }

    struct Running {
        iq: bool,
        baseband: bool,
        audio: usize,
    }

    fn running(engine: &Engine, ch: u32) -> Running {
        let snapshot = engine.snapshot();
        let set = &snapshot.device_sets[0];
        let channel = set
            .channels
            .iter()
            .find(|channel| channel.id == ch)
            .expect("channel");
        Running {
            iq: set.recording.is_some(),
            baseband: channel.baseband_recording.is_some(),
            audio: channel.audio_recordings.len(),
        }
    }

    #[test]
    fn the_switches_on_the_patch_start_and_stop_every_kind_of_recording() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, Some(dir.path().to_path_buf()));
        let store = Arc::new(Store::open(None).expect("store"));
        let hooks = Hooks {
            store,
            gate: Arc::new(Mutex::new(())),
            gps: Arc::new(GpsHub::default()),
        };
        let ds = engine.create_device_set("virtual:siggen").expect("radio");
        let settings = sdrmm_wire::ChannelSettings::default_for("nfm").expect("nfm");
        let ch = engine
            .add_channel_for(ds, 0, settings, Some("ch"))
            .expect("channel");
        let mut recorders = Recorders::default();

        recorders.reconcile(&engine, Some(&patch(true)), &hooks);
        let started = running(&engine, ch);
        assert!(started.iq && started.baseband && started.audio == 1);
        recorders.reconcile(&engine, Some(&patch(true)), &hooks);
        assert_eq!(
            running(&engine, ch).audio,
            1,
            "a second tick doubled a file"
        );

        recorders.reconcile(&engine, Some(&patch(false)), &hooks);
        let stopped = running(&engine, ch);
        assert!(!stopped.iq && !stopped.baseband && stopped.audio == 0);

        recorders.reconcile(&engine, Some(&patch(true)), &hooks);
        recorders.reconcile(&engine, None, &hooks);
        let closed = running(&engine, ch);
        assert!(!closed.iq && !closed.baseband && closed.audio == 0);
        engine.remove_device_set(ds).expect("close");
    }
}
