//! The decoder-log writer (PLAN §11): the task that turns the engine's decoded-frame
//! broadcast into rows the log endpoints can query and export.
//!
//! Loss is surfaced through a counter, not an event (PLAN §5). The engine already pushes
//! `ServerEvent::DecodedLost` for the frames *it* drops, and the WS hub does the same for a
//! slow connection — but a client that opens the log page after a burst would never see those
//! events. A counter is reported on every `GET /api/decoderlog` as `dropped`, so the loss
//! stays visible for as long as the server runs.

use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
};

use sdrmm_engine::Engine;
use sdrmm_wire::{DecodedRecord, StateScope};
use tokio::{
    sync::broadcast::{Receiver, error::RecvError},
    task::JoinHandle,
    time::{Duration, MissedTickBehavior, interval},
};

use crate::store::Store;

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
        let mut flush_tick = ticker(FLUSH_INTERVAL);
        let mut prune_tick = ticker(PRUNE_INTERVAL);
        loop {
            tokio::select! {
                received = records.recv() => match received {
                    Ok(record) => {
                        batch.push(record);
                        if batch.len() >= BATCH_MAX {
                            flush(&store, &mut batch, &dropped).await;
                        }
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
                _ = flush_tick.tick() => flush(&store, &mut batch, &dropped).await,
                _ = prune_tick.tick() => prune(&store, &engine, MAX_ROWS).await,
            }
        }
    })
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
async fn flush(store: &Arc<Store>, batch: &mut Vec<DecodedRecord>, dropped: &AtomicU64) {
    if batch.is_empty() {
        return;
    }
    let records = std::mem::take(batch);
    let owned = store.clone();
    let written =
        tokio::task::spawn_blocking(move || (owned.insert_decoder_events(&records), records)).await;
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
/// must refetch — hence the scope emit (individual decodes never invalidate, PLAN §10).
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
    use sdrmm_wire::{AdsbMessage, DecoderEvent, DecoderLogQuery, ServerEvent};
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

        // Dropping the sender closes the broadcast, which is the writer's exit signal.
        drop(tx);
        writer.await.expect("writer exits cleanly");
    }

    /// A batch larger than the channel capacity must be counted as lost, never dropped
    /// silently (PLAN §5).
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
