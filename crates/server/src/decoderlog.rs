use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use sdrmm_engine::Engine;
use sdrmm_wire::{DecodedRecord, StateScope};
use tokio::{
    sync::broadcast::{Receiver, error::RecvError},
    task::JoinHandle,
    time::{Duration, MissedTickBehavior, interval},
};

use crate::store::{LogOrigin, Store};

/// Rows the log is capped at, enforced by [`prune`]. At roughly 300 B a row (the JSON event
/// dominates) that is a few hundred megabytes — an unattended receiver must not be able to
/// fill the disk — and about three hours of history at a busy ADS-B site.
const MAX_ROWS: u64 = 1_000_000;

/// Records coalesced into one transaction. ADS-B alone produces hundreds of frames a second;
/// a row-at-a-time write would make the commit the bottleneck.
const BATCH_MAX: usize = 256;

/// How long a partial batch waits before it is written anyway, so a quiet decoder still shows
/// up in the log promptly.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// Records carried across a failed insert. A store that keeps failing (disk full) must not
/// grow the retry queue without bound, so the oldest overflow is dropped and counted.
const RETRY_MAX: usize = 4 * BATCH_MAX;

/// Subscribe to the engine's decoded frames and persist them until the engine goes away.
///
/// Only a `Weak` reference is kept: holding the engine alive would mean the broadcast sender
/// never drops, and the task would never see [`RecvError::Closed`].
pub(crate) fn spawn_writer(
    engine: Arc<Engine>,
    store: Arc<Store>,
    dropped: Arc<AtomicU64>,
) -> JoinHandle<()> {
    let records = engine.subscribe_decoded();
    let weak = Arc::downgrade(&engine);
    drop(engine);
    spawn_writer_on(records, weak, store, dropped)
}

fn spawn_writer_on(
    mut records: Receiver<DecodedRecord>,
    engine: Weak<Engine>,
    store: Arc<Store>,
    dropped: Arc<AtomicU64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut batch: Vec<DecodedRecord> = Vec::with_capacity(BATCH_MAX);
        let mut nodes = NodeMap::default();
        let mut flush_tick = ticker(FLUSH_INTERVAL);
        let mut prune_tick = ticker(PRUNE_INTERVAL);
        loop {
            tokio::select! {
                received = records.recv() => match received {
                    Ok(record) => {
                        batch.push(record);
                        if batch.len() >= BATCH_MAX {
                            flush(&store, &mut batch, &dropped, &engine, &mut nodes).await;
                        }
                    }
                    Err(RecvError::Lagged(count)) => {
                        dropped.fetch_add(count, Ordering::Relaxed);
                        tracing::warn!(count, "decoder frames lost: log writer behind");
                    }
                    Err(RecvError::Closed) => {
                        flush(&store, &mut batch, &dropped, &engine, &mut nodes).await;
                        return;
                    }
                },
                _ = flush_tick.tick() => {
                    flush(&store, &mut batch, &dropped, &engine, &mut nodes).await;
                }
                _ = prune_tick.tick() => prune(&store, &engine, MAX_ROWS).await,
            }
        }
    })
}

/// Which patch node each live channel belongs to, so a stored row carries the durable half of
/// its origin (`DecoderLogEntry::node`) and not only an engine id that is reused between runs.
///
/// Rebuilt from the active workspace's binding, which is the same rule apply and capture use
/// (`crate::workspace::bind`) — there is no second notion of "which node is this channel"
/// anywhere. Held across flushes because rebuilding reads the workspace row: a decoder at full
/// rate flushes twice a second, and the answer changes only when a channel appears or disappears
/// or the patch itself is edited, which is what the cache key watches.
/// What a mapping was derived from: the engine's channel inventory, and the workspace (by id and
/// revision) it was bound against. Nothing else can change an answer, and both are cheap to read.
#[derive(PartialEq, Eq)]
struct NodeMapKey {
    channels: Vec<(u32, u32)>,
    workspace: i64,
    revision: u64,
}

#[derive(Default)]
struct NodeMap {
    key: Option<NodeMapKey>,
    /// The workspace `map` was bound against — stored beside every row, because a node id only
    /// names a decoder within the workspace that holds it.
    workspace: Option<i64>,
    map: HashMap<(u32, u32), String>,
}

impl NodeMap {
    /// The current mapping, rebuilding it only when what it is derived from has moved.
    ///
    /// Blocks (it reads the store), so this belongs on the blocking pool with the insert it
    /// feeds. With no engine or no active workspace the origin is empty, and every row of the
    /// batch is stored unattributed — honestly so, rather than attributed to a guess.
    fn resolve(&mut self, engine: Option<&Engine>, store: &Store) -> LogOrigin<'_> {
        let Some(engine) = engine else {
            self.forget();
            return self.origin();
        };
        let state = engine.snapshot();
        let channels: Vec<(u32, u32)> = state
            .device_sets
            .iter()
            .flat_map(|set| set.channels.iter().map(move |channel| (set.id, channel.id)))
            .collect();
        let active = match store.active_workspace() {
            Ok(active) => active,
            Err(err) => {
                // The binding is unavailable, not wrong: keep whatever was last resolved rather
                // than dropping the node off a batch because one read failed.
                tracing::warn!(%err, "could not read the active workspace for the decoder log");
                return self.origin();
            }
        };
        let Some(active) = active else {
            self.forget();
            return self.origin();
        };
        let key = NodeMapKey {
            channels,
            workspace: active.info.id,
            revision: active.info.revision,
        };
        if self.key.as_ref() == Some(&key) {
            return self.origin();
        }
        self.map = crate::workspace::bind(&active.snapshot.graph, &state)
            .into_iter()
            .flat_map(|binding| {
                binding
                    .channels
                    .into_iter()
                    .map(move |(node, channel)| ((binding.device_set, channel), node))
            })
            .collect();
        self.workspace = Some(active.info.id);
        self.key = Some(key);
        self.origin()
    }

    fn origin(&self) -> LogOrigin<'_> {
        LogOrigin {
            workspace: self.workspace,
            nodes: &self.map,
        }
    }

    /// Nothing can be attributed: the workspace goes with the map, or a later batch would be
    /// stamped with a workspace whose binding no longer backs it.
    fn forget(&mut self) {
        self.key = None;
        self.workspace = None;
        self.map.clear();
    }
}

/// A timer whose missed ticks are dropped rather than replayed: a long insert must not be
/// followed by a burst of catch-up flushes.
fn ticker(period: Duration) -> tokio::time::Interval {
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

/// Write the pending records in one transaction. A store failure is logged and the batch is
/// carried into the next flush; nothing here may panic or exit the task.
async fn flush(
    store: &Arc<Store>,
    batch: &mut Vec<DecodedRecord>,
    dropped: &AtomicU64,
    engine: &Weak<Engine>,
    nodes: &mut NodeMap,
) {
    if batch.is_empty() {
        return;
    }
    let records = std::mem::take(batch);
    let owned = store.clone();
    // Both halves run on the blocking pool: resolving reads the workspace row, and it must see
    // the binding as of this batch rather than one flush later.
    let engine = engine.upgrade();
    let mut resolver = std::mem::take(nodes);
    let written = tokio::task::spawn_blocking(move || {
        let result = {
            let origin = resolver.resolve(engine.as_deref(), &owned);
            owned.insert_decoder_events(&records, &origin)
        };
        (result, records, resolver)
    })
    .await;
    let written = match written {
        Ok((result, records, resolver)) => {
            *nodes = resolver;
            Ok((result, records))
        }
        Err(err) => Err(err),
    };
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
        // The blocking task was cancelled or panicked, taking its records with it.
        Err(err) => tracing::error!(error = %err, "decoder log writer task failed"),
    }
}

/// Enforce the row budget. A prune changes the log structurally, so clients holding a page
/// must refetch — hence the scope emit (individual decodes never invalidate, ).
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
        AdsbMessage, ChannelNode, ChannelParams, ChannelSettings, DecoderEvent, DecoderLogQuery,
        DeviceRef, NodeBody, PatchEdge, PatchNode, PortRef, Position, ServerEvent,
        WorkspaceSnapshot,
    };
    use tokio::sync::broadcast;

    use super::*;

    fn record(icao: &str) -> DecodedRecord {
        DecodedRecord {
            device_set: 0,
            channel: 0,
            at: "2026-08-09T12:00:00Z".to_string(),
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

    /// Poll instead of sleeping a fixed span: the writer flushes on its own timer.
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
            tx.send(record(icao)).expect("send");
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

    /// The row's durable origin. An engine channel id names this frame's decoder only for as
    /// long as this run lasts, so the writer resolves it against the active workspace's binding
    /// and stores the patch node beside it — and stores nothing where it cannot, rather than
    /// attributing a frame to a decoder that never heard it.
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
                    offset_hz: 0.0,
                    squelch_db: None,
                    params: ChannelParams::default_for("adsb").expect("adsb is a channel type"),
                },
            )
            .expect("add channel");

        let store = Arc::new(Store::open(None).expect("store"));
        let mut snapshot = WorkspaceSnapshot::starter();
        snapshot.graph.nodes.push(PatchNode {
            id: "channel:adsb".to_owned(),
            body: NodeBody::Channel(ChannelNode {
                channel_type: "adsb".to_owned(),
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

        let mut nodes = NodeMap::default();
        let mut batch = vec![DecodedRecord {
            device_set: set,
            channel,
            ..record("3C6444")
        }];
        flush(
            &store,
            &mut batch,
            &AtomicU64::new(0),
            &Arc::downgrade(&engine),
            &mut nodes,
        )
        .await;

        let entries = store
            .query_decoder_log(&DecoderLogQuery::default())
            .expect("query")
            .0;
        assert_eq!(entries[0].node.as_deref(), Some("channel:adsb"));

        // A channel no node claims is stored unattributed, and the scope's fallback is what
        // reaches it — never a node id the binding did not actually produce.
        let mut orphan = vec![DecodedRecord {
            device_set: set,
            channel: channel + 99,
            ..record("4CA2D4")
        }];
        flush(
            &store,
            &mut orphan,
            &AtomicU64::new(0),
            &Arc::downgrade(&engine),
            &mut nodes,
        )
        .await;
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
        // Overflow before the writer ever polls, so the lag is deterministic.
        for i in 0..20 {
            tx.send(record(&format!("00000{i}"))).expect("send");
        }
        let writer = spawn_writer_on(rx, Weak::new(), store.clone(), dropped.clone());
        drop(tx);
        writer.await.expect("writer exits cleanly");

        assert_eq!(dropped.load(Ordering::Relaxed), 16);
        assert_eq!(total(&store), 4);
    }

    /// Pruning is a structural change: the log page must be invalidated, not silently
    /// truncated under the client.
    #[tokio::test]
    async fn prune_over_budget_emits_the_decoder_log_scope() {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let mut events = engine.subscribe_events();

        let store = Arc::new(Store::open(None).expect("store"));
        let records: Vec<DecodedRecord> = (0..3).map(|i| record(&format!("00000{i}"))).collect();
        store
            .insert_decoder_events(&records, &LogOrigin::unattributed())
            .expect("insert");

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
