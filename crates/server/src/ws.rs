//! WebSocket hub (PLAN §5): one socket per client carrying JSON events + client commands and
//! binary spectrum frames. A single writer task owns the sink; event and per-subscription
//! spectrum tasks feed it through an mpsc channel. Spectrum uses drop-oldest backpressure so a
//! slow client never stalls the hub (PLAN §5).

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use sdrmm_dsp::{decimate_max, quantize_db};
use sdrmm_engine::{Engine, SpectrumSnapshot, adaptive_db_window};
use sdrmm_wire::{ClientCommand, ServerEvent, SpectrumFrame};
use tokio::sync::{broadcast, mpsc};

use crate::AppState;

const OUT_CHANNEL_CAP: usize = 256;
const MIN_BINS: usize = 16;
const MAX_BINS: usize = 4096;
const MAX_FPS: u16 = 60;

pub(crate) async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let engine = state.engine.clone();
    ws.on_upgrade(move |socket| handle_socket(socket, engine))
}

async fn handle_socket(socket: WebSocket, engine: Arc<Engine>) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(OUT_CHANNEL_CAP);

    // Sole owner of the sink; every producer routes through `out_tx`.
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Subscribe BEFORE snapshotting the revision so no event emitted after the snapshot can slip
    // through the gap — otherwise a mutation between snapshot and subscribe would be lost, and
    // StateChanged carries no revision for the client to detect it (PLAN §5).
    let mut event_rx = engine.subscribe_events();
    let hello = ServerEvent::Hello {
        revision: engine.snapshot().revision,
    };
    let _ = out_tx.send(text_event(&hello)).await;

    // Forward low-rate state events.
    let events = {
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(ev) => {
                        if out_tx.send(text_event(&ev)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
    };

    let mut spectra: HashMap<u32, tokio::task::JoinHandle<()>> = HashMap::new();

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                let parsed: &str = &text;
                match serde_json::from_str::<ClientCommand>(parsed) {
                    Ok(ClientCommand::SubscribeSpectrum {
                        device_set,
                        fps,
                        bins,
                    }) => {
                        // The binary frame's stream_id is u16 (PLAN §9); refuse ids that would
                        // silently alias rather than truncate them.
                        if device_set > u32::from(u16::MAX) {
                            let err = ServerEvent::Error {
                                message: format!("device set {device_set} exceeds stream id range"),
                            };
                            let _ = out_tx.send(text_event(&err)).await;
                            continue;
                        }
                        if let Some(old) = spectra.remove(&device_set) {
                            old.abort();
                        }
                        match engine.subscribe_spectrum(device_set) {
                            Ok(rx) => {
                                let started = ServerEvent::StreamStarted {
                                    stream_id: device_set as u16,
                                    device_set,
                                };
                                let _ = out_tx.send(text_event(&started)).await;
                                let task =
                                    spawn_spectrum(device_set, fps, bins, rx, out_tx.clone());
                                spectra.insert(device_set, task);
                            }
                            Err(e) => {
                                let err = ServerEvent::Error {
                                    message: e.to_string(),
                                };
                                let _ = out_tx.send(text_event(&err)).await;
                            }
                        }
                    }
                    Ok(ClientCommand::UnsubscribeSpectrum { device_set }) => {
                        if let Some(task) = spectra.remove(&device_set) {
                            task.abort();
                            let stopped = ServerEvent::StreamStopped {
                                stream_id: device_set as u16,
                            };
                            let _ = out_tx.send(text_event(&stopped)).await;
                        }
                    }
                    Err(_) => {
                        let err = ServerEvent::Error {
                            message: "invalid command".to_string(),
                        };
                        let _ = out_tx.send(text_event(&err)).await;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    for (_, task) in spectra {
        task.abort();
    }
    events.abort();
    writer.abort();
}

/// Per-subscription task: throttle to `fps`, decimate to `bins`, quantize over the adaptive dB
/// window, and emit a binary [`SpectrumFrame`] (PLAN §9).
fn spawn_spectrum(
    ds: u32,
    fps: u16,
    bins: u16,
    mut rx: broadcast::Receiver<SpectrumSnapshot>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    let fps = fps.clamp(1, MAX_FPS);
    let bins = (bins as usize).clamp(MIN_BINS, MAX_BINS);
    let min_interval = Duration::from_secs_f64(1.0 / f64::from(fps));

    tokio::spawn(async move {
        let mut dec = vec![0f32; bins];
        let mut quant = vec![0u8; bins];
        let mut last = Instant::now()
            .checked_sub(min_interval)
            .unwrap_or_else(Instant::now);

        loop {
            match rx.recv().await {
                Ok(snap) => {
                    let now = Instant::now();
                    if now.duration_since(last) < min_interval {
                        continue;
                    }
                    last = now;

                    decimate_max(&snap.db, &mut dec);
                    let (db_min, db_max) = adaptive_db_window(&snap.db);
                    quantize_db(&dec, db_min, db_max, &mut quant);

                    let frame = SpectrumFrame {
                        stream_id: ds as u16,
                        seq: snap.seq,
                        timestamp: snap.timestamp,
                        center_hz: snap.center_hz,
                        span_hz: snap.span_hz,
                        db_min,
                        db_max,
                        bins: &quant,
                    }
                    .encode();

                    // Drop-oldest backpressure: never block the hub on a slow client.
                    if out_tx.try_send(Message::Binary(frame.into())).is_err() {
                        // Frame dropped (channel full or closed); the next one will catch up.
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    // The device set was removed. Tell the client the stream ended (no silent
                    // termination, CLAUDE.md); this is a one-shot control message, so await it.
                    let stopped = ServerEvent::StreamStopped {
                        stream_id: ds as u16,
                    };
                    let _ = out_tx.send(text_event(&stopped)).await;
                    break;
                }
            }
        }
    })
}

fn text_event(ev: &ServerEvent) -> Message {
    match serde_json::to_string(ev) {
        Ok(json) => Message::Text(json.into()),
        // Serializing our own enum cannot realistically fail; emit a minimal error frame.
        Err(_) => Message::Text(
            r#"{"type":"Error","data":{"message":"event serialization failed"}}"#.into(),
        ),
    }
}
