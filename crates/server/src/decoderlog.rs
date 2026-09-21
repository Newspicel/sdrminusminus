use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
};

use sdrmm_engine::Engine;
use sdrmm_wire::StateScope;
use tokio::{
    sync::broadcast::{Receiver, error::RecvError},
    time::{Duration, MissedTickBehavior, interval},
};

use crate::{decoded::Decoded, events::Routed, store::Store};

const MAX_ROWS: u64 = 1_000_000;

const BATCH_MAX: usize = 256;

const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

const RETRY_MAX: usize = 4 * BATCH_MAX;

pub(crate) async fn run(
    records: Receiver<Decoded>,
    engine: Weak<Engine>,
    store: Arc<Store>,
    dropped: Arc<AtomicU64>,
) {
    let mut batch: Vec<Routed> = Vec::with_capacity(BATCH_MAX);
    let mut flush_tick = ticker(FLUSH_INTERVAL);
    let mut prune_tick = ticker(PRUNE_INTERVAL);
    let mut records = records;
    loop {
        tokio::select! {
            received = records.recv() => match received {
                Ok(Decoded::Record(routed)) => {
                    batch.push(*routed);
                    if batch.len() >= BATCH_MAX {
                        flush(&store, &mut batch, &dropped).await;
                    }
                }
                Ok(Decoded::Lost(count)) => {
                    dropped.fetch_add(count, Ordering::Relaxed);
                }
                Err(RecvError::Lagged(count)) => {
                    dropped.fetch_add(count, Ordering::Relaxed);
                    tracing::warn!(count, "decoder frames lost: log writer behind");
                }
                Err(RecvError::Closed) => {
                    flush(&store, &mut batch, &dropped).await;
                    return;
                }
            },
            _ = flush_tick.tick() => {
                flush(&store, &mut batch, &dropped).await;
            }
            _ = prune_tick.tick() => {
                expire(&store, &engine, RETENTION).await;
                prune(&store, &engine, MAX_ROWS).await;
            }
        }
    }
}

fn ticker(period: Duration) -> tokio::time::Interval {
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

async fn flush(store: &Arc<Store>, batch: &mut Vec<Routed>, dropped: &AtomicU64) {
    if batch.is_empty() {
        return;
    }
    let records = std::mem::take(batch);
    let owned = store.clone();
    let written = tokio::task::spawn_blocking(move || {
        let result = owned.insert_decoder_events(&records);
        (result, records)
    })
    .await;
    match written {
        Ok((Ok(_), _)) => {}
        Ok((Err(err), mut records)) => {
            tracing::error!(error = %err, rows = records.len(), "decoder log insert failed");
            if records.len() > RETRY_MAX {
                let overflow = records.len() - RETRY_MAX;
                dropped.fetch_add(overflow as u64, Ordering::Relaxed);
                tracing::warn!(count = overflow, "decoder frames dropped: retry queue full");
                records.drain(..overflow);
            }
            *batch = records;
        }
        Err(err) => tracing::error!(error = %err, "decoder log writer task failed"),
    }
}

async fn expire(store: &Arc<Store>, engine: &Weak<Engine>, keep: Duration) {
    let Ok(keep) = jiff::SignedDuration::try_from(keep) else {
        return;
    };
    let cutoff = (jiff::Timestamp::now() - keep).to_string();
    let owned = store.clone();
    match tokio::task::spawn_blocking(move || owned.prune_decoder_log_before(&cutoff)).await {
        Ok(Ok(0)) => {}
        Ok(Ok(count)) => {
            tracing::info!(count, "decoder log rows aged out");
            if let Some(engine) = engine.upgrade() {
                engine.emit_scope(StateScope::DecoderLog);
            }
        }
        Ok(Err(err)) => tracing::error!(error = %err, "decoder log expiry failed"),
        Err(err) => tracing::error!(error = %err, "decoder log expiry task failed"),
    }
}

async fn prune(store: &Arc<Store>, engine: &Weak<Engine>, max_rows: u64) {
    let owned = store.clone();
    match tokio::task::spawn_blocking(move || owned.prune_decoder_log(max_rows)).await {
        Ok(Ok(0)) => {}
        Ok(Ok(count)) => {
            tracing::info!(count, "decoder log pruned to its row budget");
            if let Some(engine) = engine.upgrade() {
                engine.emit_scope(StateScope::DecoderLog);
            }
        }
        Ok(Err(err)) => tracing::error!(error = %err, "decoder log prune failed"),
        Err(err) => tracing::error!(error = %err, "decoder log prune task failed"),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        AdsbMessage, ChannelNode, ChannelParams, ChannelSettings, DecodedRecord, DecoderEvent,
        DecoderLogQuery, DeviceRef, NodeBody, PatchEdge, PatchNode, PortRef, Position, ServerEvent,
        WorkspaceSnapshot,
    };
    use tokio::sync::broadcast;

    use super::*;

    fn spawn_writer_on(
        records: Receiver<Decoded>,
        engine: Weak<Engine>,
        store: Arc<Store>,
        dropped: Arc<AtomicU64>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run(records, engine, store, dropped))
    }

    fn loose(record: DecodedRecord) -> Decoded {
        Decoded::Record(Box::new(Routed::unattributed(record)))
    }

    fn record(icao: &str) -> DecodedRecord {
        DecodedRecord {
            sinks: Vec::new(),
            device_set: 0,
            channel: 0,
            at: jiff::Timestamp::now().to_string(),
            freq_hz: 1_090_000_000.0,
            event: DecoderEvent::Adsb(AdsbMessage {
                icao: icao.to_string(),
                df: 17,
                raw: "8D3C6444".to_string(),
                ..AdsbMessage::default()
            }),
        }
    }

    fn total(store: &Store) -> u64 {
        store
            .query_decoder_log(&DecoderLogQuery::default())
            .expect("query")
            .1
    }

    async fn wait_for_rows(store: &Store, want: u64) {
        for _ in 0..200 {
            if total(store) >= want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "decoder log never reached {want} rows (has {})",
            total(store)
        );
    }

    #[tokio::test]
    async fn broadcast_records_reach_the_store() {
        let store = Arc::new(Store::open(None).expect("store"));
        let (tx, rx) = broadcast::channel(64);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer = spawn_writer_on(rx, Weak::new(), store.clone(), dropped.clone());

        for icao in ["3C6444", "4CA2D4", "AB1234"] {
            tx.send(loose(record(icao))).expect("send");
        }
        wait_for_rows(&store, 3).await;
        assert_eq!(dropped.load(Ordering::Relaxed), 0);

        let entries = store
            .query_decoder_log(&DecoderLogQuery::default())
            .expect("query")
            .0;
        assert_eq!(entries[0].station.as_deref(), Some("AB1234"));
        assert_eq!(entries[0].kind, "adsb");

        drop(tx);
        writer.await.expect("writer exits cleanly");
    }

    #[tokio::test]
    async fn a_written_row_carries_the_patch_node_behind_its_channel() {
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
        snapshot.graph.nodes.push(PatchNode {
            id: "channel:adsb".to_owned(),
            body: NodeBody::Channel(ChannelNode {
                channel_type: "adsb".to_owned(),
                record_calls: false,
                tuning_locked: false,
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        });
        snapshot.graph.edges.push(PatchEdge {
            from: PortRef {
                node: "device".to_owned(),
                port: "iq".to_owned(),
            },
            to: PortRef {
                node: "channel:adsb".to_owned(),
                port: "iq".to_owned(),
            },
        });
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

        let routes =
            crate::decoded::resolve_routes(&store, &engine.snapshot()).expect("routes resolve");
        let mut batch = vec![routes.route(DecodedRecord {
            sinks: Vec::new(),
            device_set: set,
            channel,
            ..record("3C6444")
        })];
        flush(&store, &mut batch, &AtomicU64::new(0)).await;

        let entries = store
            .query_decoder_log(&DecoderLogQuery::default())
            .expect("query")
            .0;
        assert_eq!(entries[0].node.as_deref(), Some("channel:adsb"));

        let mut orphan = vec![routes.route(DecodedRecord {
            sinks: Vec::new(),
            device_set: set,
            channel: channel + 99,
            ..record("4CA2D4")
        })];
        flush(&store, &mut orphan, &AtomicU64::new(0)).await;
        let entries = store
            .query_decoder_log(&DecoderLogQuery::default())
            .expect("query")
            .0;
        let orphaned = entries
            .iter()
            .find(|entry| entry.station.as_deref() == Some("4CA2D4"))
            .expect("the orphan was written");
        assert_eq!(orphaned.node, None);
    }

    #[tokio::test]
    async fn overrunning_the_broadcast_counts_the_loss() {
        let store = Arc::new(Store::open(None).expect("store"));
        let (tx, rx) = broadcast::channel(4);
        let dropped = Arc::new(AtomicU64::new(0));
        for i in 0..20 {
            tx.send(loose(record(&format!("00000{i}")))).expect("send");
        }
        let writer = spawn_writer_on(rx, Weak::new(), store.clone(), dropped.clone());
        drop(tx);
        writer.await.expect("writer exits cleanly");

        assert_eq!(dropped.load(Ordering::Relaxed), 16);
        assert_eq!(total(&store), 4);
    }

    #[tokio::test]
    async fn rows_older_than_the_window_age_out() {
        let store = Arc::new(Store::open(None).expect("store"));
        let engine = Engine::with_registry(sdrmm_device::DeviceRegistry::new(), None);
        let weak = Arc::downgrade(&engine);
        store
            .insert_decoder_events(
                &[
                    DecodedRecord {
                        sinks: Vec::new(),
                        at: "2020-01-01T00:00:00Z".to_owned(),
                        ..record("3C6444")
                    },
                    record("4CA2D4"),
                ]
                .map(Routed::unattributed),
            )
            .expect("insert");
        assert_eq!(total(&store), 2);

        expire(&store, &weak, Duration::from_secs(24 * 60 * 60)).await;

        assert_eq!(total(&store), 1, "only the stale row goes");
        let (entries, _) = store
            .query_decoder_log(&DecoderLogQuery::default())
            .expect("query");
        assert_eq!(entries[0].station.as_deref(), Some("4CA2D4"));
    }

    #[tokio::test]
    async fn prune_over_budget_emits_the_decoder_log_scope() {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let mut events = engine.subscribe_events();

        let store = Arc::new(Store::open(None).expect("store"));
        let records: Vec<Routed> = (0..3)
            .map(|i| Routed::unattributed(record(&format!("00000{i}"))))
            .collect();
        store.insert_decoder_events(&records).expect("insert");

        let weak = Arc::downgrade(&engine);
        prune(&store, &weak, 3).await;
        assert_eq!(total(&store), 3, "under budget: nothing pruned");
        assert!(events.try_recv().is_err(), "a no-op prune must not emit");

        prune(&store, &weak, 1).await;
        assert_eq!(total(&store), 1);
        assert!(matches!(
            events.try_recv().expect("scope emitted"),
            ServerEvent::StateChanged {
                scope: StateScope::DecoderLog
            }
        ));
    }
}
