use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    time::Duration,
};

use sdrmm_engine::{Engine, monitor::MonitorHandle};
use sdrmm_wire::{
    DecodedRecord, DecoderEvent, DeviceSetStatus, EventOrigin, NodeBody, SpectrumMonitorNode,
    Transmission, TransmissionState,
};

use crate::{Store, calls::Calls};

#[derive(Clone, PartialEq)]
struct Binding {
    node: String,
    device_set: u32,
    stream: u32,
    settings: SpectrumMonitorNode,
}

struct Active {
    binding: Binding,
    _handle: MonitorHandle,
}

pub(crate) async fn run(engine: Weak<Engine>, store: Arc<Store>, calls: Arc<Calls>) {
    let mut active = HashMap::<String, Active>::new();
    loop {
        let Some(strong) = engine.upgrade() else {
            return;
        };
        let store = store.clone();
        let calls = calls.clone();
        let reconciled = tokio::task::spawn_blocking(move || {
            reconcile(&strong, &store, &calls, &mut active);
            active
        })
        .await;
        match reconciled {
            Ok(next) => active = next,
            Err(error) => {
                tracing::error!(%error, "spectrum monitor reconciliation failed");
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn reconcile(
    engine: &Arc<Engine>,
    store: &Store,
    calls: &Arc<Calls>,
    active: &mut HashMap<String, Active>,
) {
    let workspace = match store.active_workspace() {
        Ok(workspace) => workspace,
        Err(error) => {
            tracing::error!(%error, "could not load spectrum monitor wiring");
            return;
        }
    };
    let Some(workspace) = workspace else {
        active.clear();
        return;
    };
    let graph = &workspace.snapshot.graph;
    let snapshot = engine.snapshot();
    let devices = crate::workspace::bind_devices(graph, &snapshot);
    let desired: Vec<Binding> = graph
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeBody::SpectrumMonitor(settings) = &node.body else {
                return None;
            };
            let edge = graph
                .edges
                .iter()
                .find(|edge| edge.to.node == node.id && edge.to.port == "iq")?;
            let (source, beam) = match graph.node(&edge.from.node).map(|node| &node.body) {
                Some(body) if body.lane_output() == Some(edge.from.port.as_str()) => {
                    let upstream = graph.edges.iter().find(|wire| {
                        wire.to.node == edge.from.node
                            && sdrmm_wire::port_stream("iq", &wire.to.port).is_some()
                    })?;
                    (upstream.from.node.as_str(), true)
                }
                _ => (edge.from.node.as_str(), false),
            };
            let (_, device_set) = devices.iter().find(|(node, _)| node == source)?;
            let set = snapshot
                .device_sets
                .iter()
                .find(|set| set.id == *device_set && set.status == DeviceSetStatus::Running)?;
            let stream = if beam {
                set.capabilities.rx_streams
            } else {
                sdrmm_wire::port_stream("iq", &edge.from.port)?
            };
            Some(Binding {
                node: node.id.clone(),
                device_set: set.id,
                stream,
                settings: settings.clone(),
            })
        })
        .collect();
    active.retain(|_, running| running._handle.is_active() && desired.contains(&running.binding));
    for binding in desired {
        if active.contains_key(&binding.node) {
            continue;
        }
        let weak = Arc::downgrade(engine);
        let calls = calls.clone();
        let origin = binding.node.clone();
        let device_set = binding.device_set;
        let result = engine.monitor(
            device_set,
            binding.stream,
            binding.settings.clone(),
            move |mut output| {
                let Some(engine) = weak.upgrade() else {
                    return;
                };
                if !output.audio.is_empty()
                    && let DecoderEvent::Transmission(transmission) = &mut output.event
                {
                    let (audio, evicted) = calls.store_clip(&output.audio);
                    transmission.audio = Some(audio);
                    if evicted {
                        transmission.error.get_or_insert_with(|| {
                            "older audio evicted by the temporary buffer limit".to_owned()
                        });
                    }
                }
                let at = match &output.event {
                    DecoderEvent::Transmission(t) => t.ended_at.clone(),
                    _ => None,
                }
                .unwrap_or_else(|| format!("{:.9}", jiff::Timestamp::now()));
                engine.publish_decoded(DecodedRecord {
                    origin: Some(EventOrigin {
                        node: origin.clone(),
                        transmission: output.transmission,
                    }),
                    device_set,
                    channel: u32::MAX,
                    at,
                    freq_hz: output.frequency_hz,
                    event: output.event,
                    sinks: Vec::new(),
                });
            },
        );
        match result {
            Ok(handle) => {
                active.insert(
                    binding.node.clone(),
                    Active {
                        binding,
                        _handle: handle,
                    },
                );
            }
            Err(error) => {
                engine.publish_decoded(DecodedRecord {
                    origin: Some(EventOrigin {
                        node: binding.node,
                        transmission: 0,
                    }),
                    device_set,
                    channel: u32::MAX,
                    at: format!("{:.9}", jiff::Timestamp::now()),
                    freq_hz: 0.0,
                    sinks: Vec::new(),
                    event: DecoderEvent::Transmission(Transmission {
                        id: 0,
                        state: TransmissionState::Problem,
                        signal: Default::default(),
                        start_sample: 0,
                        end_sample: 0,
                        sample_rate_hz: 0.0,
                        duration_ms: 0,
                        started_at: None,
                        ended_at: None,
                        decoder: None,
                        decoder_confirmed: false,
                        audio: None,
                        error: Some(error.to_string()),
                    }),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;
    use sdrmm_wire::{DeviceRef, PatchEdge, PatchNode, PortRef, Position, WorkspaceSnapshot};

    use super::*;

    #[test]
    fn wiring_creates_one_monitor_and_unwiring_releases_it_without_channels() {
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let set = engine.create_device_set("virtual:siggen").unwrap();
        let store = Store::open(None).unwrap();
        let calls = Arc::new(Calls::default());
        let mut snapshot = WorkspaceSnapshot::starter();
        let NodeBody::Device(device) = &mut snapshot
            .graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "device")
            .unwrap()
            .body
        else {
            panic!("device");
        };
        device.device = Some(DeviceRef {
            backend: "virtual".to_owned(),
            serial: None,
            key: Some("siggen".to_owned()),
        });
        snapshot.graph.nodes.push(PatchNode {
            id: "monitor".to_owned(),
            body: NodeBody::SpectrumMonitor(SpectrumMonitorNode::default()),
            position: Position { x: 100.0, y: 0.0 },
            size: None,
            label: None,
        });
        snapshot.graph.edges.push(PatchEdge {
            from: PortRef {
                node: "device".to_owned(),
                port: "iq".to_owned(),
            },
            to: PortRef {
                node: "monitor".to_owned(),
                port: "iq".to_owned(),
            },
        });
        let workspace = store.create_workspace("monitor", &snapshot).unwrap();
        store.activate_workspace(workspace).unwrap();
        let mut active = HashMap::new();
        reconcile(&engine, &store, &calls, &mut active);
        assert_eq!(active.len(), 1);
        let id = active["monitor"].binding.clone();
        reconcile(&engine, &store, &calls, &mut active);
        assert_eq!(active.len(), 1);
        assert!(active["monitor"].binding == id);
        assert!(
            engine
                .snapshot()
                .device_sets
                .iter()
                .find(|device| device.id == set)
                .unwrap()
                .channels
                .is_empty()
        );
        let empty = store
            .create_workspace("empty", &WorkspaceSnapshot::empty())
            .unwrap();
        store.activate_workspace(empty).unwrap();
        reconcile(&engine, &store, &calls, &mut active);
        assert!(active.is_empty());
    }
}
