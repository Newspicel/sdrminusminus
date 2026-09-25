use sdrmm_wire::{
    network::{
        ChannelNetworkExportRequest, NetworkExportAction, NetworkExportRequest, NetworkExportStatus,
    },
    patch::{NodeBody, PatchGraph},
    timemachine::{TimeMachineAction, TimeMachineRequest, TimeMachineStatus},
};

use crate::{binding, store::Store};

#[derive(Clone, Debug, PartialEq)]
pub enum Teardown {
    DeviceSet(u32),
    Channel {
        device_set: u32,
        channel: u32,
    },
    ChannelExport {
        device_set: u32,
        channel: u32,
        request: ChannelNetworkExportRequest,
    },
    DeviceExport {
        device_set: u32,
        request: NetworkExportRequest,
    },
    TimeMachine {
        device_set: u32,
        request: TimeMachineRequest,
    },
}

fn feeding_node(graph: &PatchGraph, node: &str) -> Option<String> {
    graph
        .edges
        .iter()
        .find(|edge| edge.to.node == node)
        .map(|edge| edge.from.node.clone())
}

#[must_use]
pub fn teardowns(store: Store, graph: &PatchGraph, ids: &[String]) -> Vec<Teardown> {
    ids.iter()
        .filter_map(|id| teardown_of(store, graph, id))
        .collect()
}

fn teardown_of(store: Store, graph: &PatchGraph, id: &str) -> Option<Teardown> {
    let node = graph.node(id)?;
    match &node.body {
        body if body.opens_device() && !matches!(body, NodeBody::Array(_)) => {
            store.device_set_of(id).map(Teardown::DeviceSet)
        }
        NodeBody::Channel(_) => {
            let channel = store.channel_of(id)?;
            let device_set = store.device_set_of(id)?;
            Some(Teardown::Channel {
                device_set,
                channel: channel.id,
            })
        }
        NodeBody::NetworkExport(export) => {
            let feeder = feeding_node(graph, id)?;
            if matches!(graph.node(&feeder)?.body, NodeBody::Channel(_)) {
                let channel = store.channel_of(&feeder)?;
                if channel
                    .network_export
                    .as_ref()
                    .is_none_or(|status: &NetworkExportStatus| status.node != id)
                {
                    return None;
                }
                return Some(Teardown::ChannelExport {
                    device_set: store.device_set_of(&feeder)?,
                    channel: channel.id,
                    request: ChannelNetworkExportRequest {
                        action: NetworkExportAction::Stop,
                        node: id.to_owned(),
                        settings: export.settings.clone(),
                    },
                });
            }
            let (source, stream) = binding::iq_source_of(graph, id)?;
            let device_set = store.device_set_of(&source)?;
            let running = store.set_of(device_set)?.network_export?;
            (running.node == id).then(|| Teardown::DeviceExport {
                device_set,
                request: NetworkExportRequest {
                    action: NetworkExportAction::Stop,
                    node: id.to_owned(),
                    stream,
                    settings: export.settings.clone(),
                },
            })
        }
        NodeBody::TimeMachine(machine) => {
            let (source, stream) = binding::iq_source_of(graph, id)?;
            let device_set = store.device_set_of(&source)?;
            let armed: TimeMachineStatus = store.set_of(device_set)?.time_machine?;
            (armed.node == id).then(|| Teardown::TimeMachine {
                device_set,
                request: TimeMachineRequest {
                    action: TimeMachineAction::Disarm,
                    node: id.to_owned(),
                    stream,
                    settings: machine.clone(),
                },
            })
        }
        _ => None,
    }
}

pub async fn close(store: Store, teardowns: Vec<Teardown>) {
    let api = store.api();
    for teardown in teardowns {
        let outcome = match &teardown {
            Teardown::DeviceSet(set) => api.delete(&format!("/api/devicesets/{set}")).await,
            Teardown::Channel {
                device_set,
                channel,
            } => {
                api.delete(&format!("/api/devicesets/{device_set}/channels/{channel}"))
                    .await
            }
            Teardown::ChannelExport {
                device_set,
                channel,
                request,
            } => api
                .post::<_, serde::de::IgnoredAny>(
                    &format!("/api/devicesets/{device_set}/channels/{channel}/network-export"),
                    request,
                )
                .await
                .map(|_| ()),
            Teardown::DeviceExport {
                device_set,
                request,
            } => api
                .post::<_, serde::de::IgnoredAny>(
                    &format!("/api/devicesets/{device_set}/network-export"),
                    request,
                )
                .await
                .map(|_| ()),
            Teardown::TimeMachine {
                device_set,
                request,
            } => api
                .post::<_, serde::de::IgnoredAny>(
                    &format!("/api/devicesets/{device_set}/time-machine"),
                    request,
                )
                .await
                .map(|_| ()),
        };
        if let Err(error) = outcome {
            store.say(format!("Cannot close the node: {error}"));
        }
    }
}
