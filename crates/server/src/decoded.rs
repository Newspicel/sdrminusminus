use std::sync::{Arc, Weak};

use sdrmm_engine::Engine;
use sdrmm_wire::{ServerEvent, StateScope, StateSnapshot};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::{
    Store,
    events::{Routed, Routes},
};

pub(crate) const FEED_CAP: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) enum Decoded {
    Record(Box<Routed>),
    Lost(u64),
}

pub(crate) type Feed = broadcast::Sender<Decoded>;

pub(crate) fn feed() -> Feed {
    broadcast::channel(FEED_CAP).0
}

pub(crate) async fn run(engine: Weak<Engine>, store: Arc<Store>, feed: Feed) {
    let Some(strong) = engine.upgrade() else {
        return;
    };
    let mut events = strong.subscribe_events();
    let mut records = strong.subscribe_decoded();
    drop(strong);
    let mut routes = load_routes(store.clone(), engine.clone()).await;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(ServerEvent::StateChanged {
                    scope: StateScope::All
                        | StateScope::Devices
                        | StateScope::DeviceSet(_)
                        | StateScope::Workspaces,
                }) => routes = load_routes(store.clone(), engine.clone()).await,
                Ok(_) => {}
                Err(RecvError::Lagged(count)) => {
                    tracing::error!(count, "event routing missed server events");
                    routes = load_routes(store.clone(), engine.clone()).await;
                }
                Err(RecvError::Closed) => break,
            },
            record = records.recv() => match record {
                Ok(record) => {
                    let _ = feed.send(Decoded::Record(Box::new(routes.route(record))));
                }
                Err(RecvError::Lagged(count)) => {
                    tracing::error!(count, "decoded events lost before routing");
                    let _ = feed.send(Decoded::Lost(count));
                }
                Err(RecvError::Closed) => break,
            },
        }
    }
}

pub(crate) fn resolve_routes(
    store: &Store,
    state: &StateSnapshot,
) -> Result<Routes, crate::StoreError> {
    let Some(workspace) = store.active_workspace()? else {
        return Ok(Routes::default());
    };
    Ok(Routes::resolve(
        workspace.info.id,
        &workspace.snapshot.graph,
        state,
    ))
}

async fn load_routes(store: Arc<Store>, engine: Weak<Engine>) -> Routes {
    let loaded = tokio::task::spawn_blocking(move || {
        let state = engine
            .upgrade()
            .map_or_else(StateSnapshot::default, |engine| engine.snapshot());
        resolve_routes(&store, &state)
    })
    .await;
    match loaded {
        Ok(Ok(routes)) => routes,
        Ok(Err(error)) => {
            tracing::error!(%error, "could not resolve event routes");
            Routes::default()
        }
        Err(error) => {
            tracing::error!(%error, "event route resolution panicked");
            Routes::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sdrmm_wire::{
        AdsbMessage, ChannelNode, ChannelParams, ChannelSettings, DecodedRecord, DecoderEvent,
        DeviceRef, EventFilterNode, FilterMode, NodeBody, PatchEdge, PatchNode, PortRef, Position,
        WorkspaceSnapshot,
    };
    use tokio::time::{Duration, timeout};

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

    fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
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

    fn adsb(icao: &str, callsign: &str) -> DecoderEvent {
        DecoderEvent::Adsb(AdsbMessage {
            icao: icao.to_owned(),
            df: 17,
            callsign: Some(callsign.to_owned()),
            ..AdsbMessage::default()
        })
    }

    fn bench() -> (Arc<Engine>, Arc<Store>, u32, u32) {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let set = engine
            .create_device_set("virtual:siggen")
            .expect("open the virtual radio");
        let channel = engine
            .add_channel(
                set,
                0,
                ChannelSettings {
                    frequency_hz: 100_000_000.0,
                    squelch: sdrmm_wire::Squelch::Off,
                    params: ChannelParams::default_for("adsb").expect("adsb is a channel type"),
                    audio: Default::default(),
                },
            )
            .expect("add channel");
        let store = Arc::new(Store::open(None).expect("store"));
        let mut snapshot = WorkspaceSnapshot::starter();
        snapshot.graph.nodes.push(node(
            "channel:adsb",
            NodeBody::Channel(ChannelNode {
                channel_type: "adsb".to_owned(),
                record_calls: false,
                tuning_locked: false,
            }),
        ));
        snapshot.graph.nodes.push(node(
            "filter",
            NodeBody::EventFilter(EventFilterNode {
                mode: FilterMode::Drop,
                contains: Some("TEST".to_owned()),
                ..EventFilterNode::default()
            }),
        ));
        snapshot.graph.nodes.push(node("log", NodeBody::DecoderLog));
        snapshot.graph.nodes.push(node("export", NodeBody::Export));
        snapshot
            .graph
            .edges
            .push(edge(("device", "iq"), ("channel:adsb", "iq")));
        snapshot
            .graph
            .edges
            .push(edge(("channel:adsb", "events"), ("filter", "events")));
        snapshot
            .graph
            .edges
            .push(edge(("filter", "events"), ("log", "events")));
        snapshot
            .graph
            .edges
            .push(edge(("channel:adsb", "events"), ("export", "events")));
        let NodeBody::Device(device) = &mut snapshot
            .graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "device")
            .expect("the starter draws a radio")
            .body
        else {
            panic!("the starter's radio is a device node");
        };
        device.device = Some(DeviceRef {
            backend: "virtual".to_owned(),
            serial: None,
            key: Some("siggen".to_owned()),
        });
        let id = store.create_workspace("bench", &snapshot).expect("create");
        store.activate_workspace(id).expect("activate");
        (engine, store, set, channel)
    }

    async fn next(rx: &mut broadcast::Receiver<Decoded>) -> Routed {
        match timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(Decoded::Record(routed))) => *routed,
            other => panic!("expected a routed record, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn records_leave_the_router_with_their_source_and_sinks_marked() {
        let (engine, store, set, channel) = bench();
        let feed = feed();
        let mut out = feed.subscribe();
        let router = tokio::spawn(run(Arc::downgrade(&engine), store.clone(), feed));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let record = |callsign: &str| DecodedRecord {
            device_set: set,
            channel,
            at: "2026-09-21T10:00:00Z".to_owned(),
            freq_hz: 1_090_000_000.0,
            event: adsb("3C6444", callsign),
            sinks: Vec::new(),
        };
        engine.publish_decoded(record("DLH123"));
        let routed = next(&mut out).await;
        assert_eq!(routed.source.as_deref(), Some("channel:adsb"));
        assert!(routed.workspace.is_some());
        let mut sinks = routed.record.sinks.clone();
        sinks.sort();
        assert_eq!(sinks, vec!["export".to_owned(), "log".to_owned()]);

        engine.publish_decoded(record("TEST01"));
        let dropped = next(&mut out).await;
        assert_eq!(
            dropped.record.sinks,
            vec!["export".to_owned()],
            "the drop filter keeps the test message away from the log only"
        );

        engine.publish_decoded(DecodedRecord {
            channel: channel + 99,
            ..record("DLH123")
        });
        let unbound = next(&mut out).await;
        assert_eq!(unbound.source, None);
        assert!(unbound.record.sinks.is_empty());

        router.abort();
    }
}
