use std::{
    collections::HashMap,
    sync::{Arc, atomic},
    time::{Duration, Instant},
};

use axum::{
    extract::{
        State,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use sdrmm_dsp::{decimate_max, quantize_db};
use sdrmm_engine::{AudioPacket, Engine, SpectrumSnapshot, VideoPacket, adaptive_db_window};
use sdrmm_wire::{
    AudioFrame, ClientCommand, ServerEvent, SpectrumFrame, StateScope, StreamKind, VideoFrame,
};
use tokio::sync::{broadcast, mpsc};

use crate::AppState;

const OUT_CHANNEL_CAP: usize = 256;
const MIN_BINS: usize = 16;
const MAX_BINS: usize = 4096;
const MAX_FPS: u16 = 60;
/// Browser geolocation can be configured for high accuracy and may produce bursts. Keep one
/// socket from turning those into unbounded store lookups and routing work.
const MIN_POSITION_PUBLISH_INTERVAL: Duration = Duration::from_millis(50);
/// Per-channel media ids — audio and video alike — live in `MEDIA_ID_BASE..=u16::MAX` and
/// spectrum ids in `0..MEDIA_ID_BASE`, so no two streams on one connection can collide.
///
/// A spectrum id used to *be* the device-set id, which a multi-stream radio broke: several lanes
/// of one set can be watched at once, and they need ids of their own to be told apart. Every kind
/// is now allocated per connection and reported in the `…StreamStarted` event that answers the
/// subscribe.
///
/// Audio and video share one range and one allocator rather than splitting it: a client keys a
/// sink on `(kind, id)`, and one space is what lets a channel's picture and its sound be told
/// apart from every other channel's without reasoning about which half of the range they fell in.
const MEDIA_ID_BASE: u16 = 0x8000;
/// First spectrum id. Ids run `SPECTRUM_ID_BASE..MEDIA_ID_BASE`.
const SPECTRUM_ID_BASE: u16 = 0;

pub(crate) async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Serialize each decoder frame once for the whole server, not once per connection (
/// M5 multi-client). Under ADS-B traffic the per-connection cost was N× the same JSON; the
/// per-connection tasks now clone a `Utf8Bytes`, which is a refcount bump. Runs until the
/// engine is dropped (the decoded broadcast closes).
pub(crate) fn start_decoded_encoder(state: &AppState) {
    let mut decoded_rx = state.engine.subscribe_decoded();
    let out = state.decoded_text.clone();
    let tracks = state.tracks.clone();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // Building a router with no runtime in context would panic on the `spawn` below. Every
        // caller in the tree enters one first (`serve` builds inside its own; the desktop shell
        // enters Tauri's), so this is the guard on a caller that has not been written yet — and
        // it is loud, because a server whose decoders never reach a client looks like broken
        // radio hardware from the outside.
        tracing::warn!("no runtime in context: decoder frames will not reach clients");
        return;
    };
    let _guard = handle.enter();
    tokio::spawn(async move {
        loop {
            match decoded_rx.recv().await {
                Ok(record) => {
                    // Buffered here rather than in a second subscriber: this task already sees
                    // every record exactly once, and another receiver on the engine's broadcast
                    // would be another thing that can lag and lose frames.
                    tracks.observe(&record);
                    let _ = out.send(encode_event(&ServerEvent::Decoded(Box::new(record))));
                }
                // The encoder is the only consumer of the engine's broadcast now, so its own
                // lag is server-wide loss: report it once to everyone rather than per socket.
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    let _ = out.send(encode_event(&ServerEvent::DecodedLost { count: missed }));
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let engine = state.engine.clone();
    let live = state.clients.fetch_add(1, atomic::Ordering::Relaxed) + 1;
    tracing::debug!(clients = live, "client connected");
    engine.emit_scope(StateScope::Clients);
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(OUT_CHANNEL_CAP);

    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    let event_rx = engine.subscribe_events();
    // Subscribed alongside the control stream, before the snapshot, for the same reason: a
    // decode landing in the gap would otherwise never reach this client's live view.
    let decoded_rx = state.decoded_text.subscribe();
    let position_rx = state.gps.subscribe();
    let hello = ServerEvent::Hello {
        revision: engine.snapshot().revision,
    };
    let _ = out_tx.send(text_event(&hello)).await;
    for position in state.gps.snapshot() {
        let _ = out_tx.send(text_event(&position)).await;
    }

    // Snapshotted after the live subscription above, so the seam repeats a record rather than
    // dropping one. A repeat is free — merging the same decode onto a station it is already part
    // of changes nothing — while a gap would leave a contact off the map until it transmits again.
    let backlog = state.tracks.backlog();
    if !backlog.is_empty() {
        let _ = out_tx
            .send(text_event(&ServerEvent::DecodedBacklog {
                records: backlog,
            }))
            .await;
    }

    let events = spawn_events(event_rx, out_tx.clone());
    let decoded = spawn_decoded(decoded_rx, out_tx.clone());
    let positions = spawn_positions(position_rx, out_tx.clone());

    // Keyed by (device set, receive stream): two scopes on two lanes of one radio are the point
    // of a multi-stream device, so a subscribe on one lane must not replace another's.
    let mut spectra: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)> = HashMap::new();
    let mut next_spectrum_id: u16 = SPECTRUM_ID_BASE;
    let mut audio: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)> = HashMap::new();
    let mut video: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)> = HashMap::new();
    let mut last_position_publish: Option<Instant> = None;
    let mut next_media_id: u16 = MEDIA_ID_BASE;

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                let parsed: &str = &text;
                match serde_json::from_str::<ClientCommand>(parsed) {
                    Ok(ClientCommand::SubscribeSpectrum {
                        device_set,
                        fps,
                        bins,
                        stream,
                    }) => {
                        // subscribe_spectrum takes the per-set runtime mutex, which
                        // patch/remove hold across device I/O and thread joins — blocking
                        // pool, like every hardware-reaching call in rest.rs.
                        let subscribe = {
                            let engine = engine.clone();
                            tokio::task::spawn_blocking(move || {
                                engine.subscribe_spectrum(device_set, stream)
                            })
                            .await
                        };
                        match flatten_join(subscribe) {
                            Ok(rx) => {
                                // Re-subscribing this same lane replaces it, and the old id stops
                                // loudly first so the client can drop its sink — the audio arm's
                                // no-silent-termination rule. Other lanes of this set are
                                // untouched.
                                if let Some((old_id, old)) = spectra.remove(&(device_set, stream)) {
                                    old.abort();
                                    let stopped = ServerEvent::StreamStopped {
                                        stream_id: old_id,
                                        kind: StreamKind::Spectrum,
                                    };
                                    let _ = out_tx.send(text_event(&stopped)).await;
                                }
                                let live = |id: u16| spectra.values().any(|(sid, _)| *sid == id);
                                match alloc_stream_id(
                                    &mut next_spectrum_id,
                                    SPECTRUM_ID_BASE..=MEDIA_ID_BASE - 1,
                                    live,
                                ) {
                                    Some(stream_id) => {
                                        let started = ServerEvent::StreamStarted {
                                            stream_id,
                                            device_set,
                                            stream,
                                        };
                                        let _ = out_tx.send(text_event(&started)).await;
                                        let task = spawn_spectrum(
                                            SpectrumLane {
                                                stream_id,
                                                device_set,
                                                stream,
                                            },
                                            fps,
                                            bins,
                                            rx,
                                            out_tx.clone(),
                                            engine.clone(),
                                        );
                                        spectra.insert((device_set, stream), (stream_id, task));
                                    }
                                    None => {
                                        let err = ServerEvent::Error {
                                            message: "no free spectrum stream ids on this \
                                                      connection"
                                                .to_string(),
                                        };
                                        let _ = out_tx.send(text_event(&err)).await;
                                    }
                                }
                            }
                            Err(message) => {
                                let _ = out_tx
                                    .send(text_event(&ServerEvent::Error { message }))
                                    .await;
                            }
                        }
                    }
                    Ok(ClientCommand::UnsubscribeSpectrum { device_set, stream }) => {
                        if let Some((stream_id, task)) = spectra.remove(&(device_set, stream)) {
                            task.abort();
                            let stopped = ServerEvent::StreamStopped {
                                stream_id,
                                kind: StreamKind::Spectrum,
                            };
                            let _ = out_tx.send(text_event(&stopped)).await;
                        }
                    }
                    Ok(ClientCommand::SubscribeAudio {
                        device_set,
                        channel,
                    }) => {
                        let subscribe = {
                            let engine = engine.clone();
                            tokio::task::spawn_blocking(move || {
                                engine.subscribe_audio(device_set, channel)
                            })
                            .await
                        };
                        match flatten_join(subscribe) {
                            Ok(rx) => {
                                // Replacing a running stream for this (ds, ch): stop the old
                                // id loudly first, so the client can tear its sink down
                                // before the new id starts (no silent termination).
                                if let Some((old_id, old)) = audio.remove(&(device_set, channel)) {
                                    old.abort();
                                    let stopped = ServerEvent::StreamStopped {
                                        stream_id: old_id,
                                        kind: StreamKind::Audio,
                                    };
                                    let _ = out_tx.send(text_event(&stopped)).await;
                                }
                                let live = |id: u16| media_id_live(&audio, &video, id);
                                match alloc_stream_id(
                                    &mut next_media_id,
                                    MEDIA_ID_BASE..=u16::MAX,
                                    live,
                                ) {
                                    Some(stream_id) => {
                                        let started = ServerEvent::AudioStreamStarted {
                                            stream_id,
                                            device_set,
                                            channel,
                                        };
                                        let _ = out_tx.send(text_event(&started)).await;
                                        let task = spawn_audio(stream_id, rx, out_tx.clone());
                                        audio.insert((device_set, channel), (stream_id, task));
                                    }
                                    None => {
                                        let err = ServerEvent::Error {
                                            message: "no free media stream ids on this connection"
                                                .to_string(),
                                        };
                                        let _ = out_tx.send(text_event(&err)).await;
                                    }
                                }
                            }
                            Err(message) => {
                                let _ = out_tx
                                    .send(text_event(&ServerEvent::Error { message }))
                                    .await;
                            }
                        }
                    }
                    Ok(ClientCommand::UnsubscribeAudio {
                        device_set,
                        channel,
                    }) => {
                        if let Some((stream_id, task)) = audio.remove(&(device_set, channel)) {
                            task.abort();
                            let stopped = ServerEvent::StreamStopped {
                                stream_id,
                                kind: StreamKind::Audio,
                            };
                            let _ = out_tx.send(text_event(&stopped)).await;
                        }
                    }
                    Ok(ClientCommand::SubscribeVideo {
                        device_set,
                        channel,
                    }) => {
                        let subscribe = {
                            let engine = engine.clone();
                            tokio::task::spawn_blocking(move || {
                                engine.subscribe_video(device_set, channel)
                            })
                            .await
                        };
                        match flatten_join(subscribe) {
                            Ok(rx) => {
                                if let Some((old_id, old)) = video.remove(&(device_set, channel)) {
                                    old.abort();
                                    let stopped = ServerEvent::StreamStopped {
                                        stream_id: old_id,
                                        kind: StreamKind::Video,
                                    };
                                    let _ = out_tx.send(text_event(&stopped)).await;
                                }
                                let live = |id: u16| media_id_live(&audio, &video, id);
                                match alloc_stream_id(
                                    &mut next_media_id,
                                    MEDIA_ID_BASE..=u16::MAX,
                                    live,
                                ) {
                                    Some(stream_id) => {
                                        let started = ServerEvent::VideoStreamStarted {
                                            stream_id,
                                            device_set,
                                            channel,
                                        };
                                        let _ = out_tx.send(text_event(&started)).await;
                                        let task = spawn_video(stream_id, rx, out_tx.clone());
                                        video.insert((device_set, channel), (stream_id, task));
                                    }
                                    None => {
                                        let err = ServerEvent::Error {
                                            message: "no free media stream ids on this connection"
                                                .to_string(),
                                        };
                                        let _ = out_tx.send(text_event(&err)).await;
                                    }
                                }
                            }
                            Err(message) => {
                                let _ = out_tx
                                    .send(text_event(&ServerEvent::Error { message }))
                                    .await;
                            }
                        }
                    }
                    Ok(ClientCommand::UnsubscribeVideo {
                        device_set,
                        channel,
                    }) => {
                        if let Some((stream_id, task)) = video.remove(&(device_set, channel)) {
                            task.abort();
                            let stopped = ServerEvent::StreamStopped {
                                stream_id,
                                kind: StreamKind::Video,
                            };
                            let _ = out_tx.send(text_event(&stopped)).await;
                        }
                    }
                    Ok(ClientCommand::PublishPosition { node, fix, error }) => {
                        let too_fast = last_position_publish
                            .is_some_and(|last| last.elapsed() < MIN_POSITION_PUBLISH_INTERVAL);
                        last_position_publish = Some(Instant::now());
                        if node.is_empty() || node.len() > sdrmm_wire::patch::MAX_NODE_ID_LEN {
                            let _ = out_tx
                                .send(text_event(&ServerEvent::Error {
                                    message: "invalid position node id".to_owned(),
                                }))
                                .await;
                        } else if too_fast {
                            let _ = out_tx
                                .send(text_event(&ServerEvent::Error {
                                    message: "position updates are limited to 20 Hz per connection"
                                        .to_owned(),
                                }))
                                .await;
                        } else {
                            // Device GPS validation reads the active workspace from SQLite. Keep
                            // that blocking lookup off the WebSocket runtime worker just like the
                            // engine control operations above. Charge the rate limit per attempt:
                            // rejected updates cost the same database lookup as accepted ones.
                            let app = state.clone();
                            let publish_node = node.clone();
                            let publish = tokio::task::spawn_blocking(move || {
                                app.gps.publish_device(&app, &publish_node, fix, error)
                            })
                            .await;
                            match publish {
                                Ok(Ok(())) => {}
                                Ok(Err(message)) => {
                                    let _ = out_tx
                                        .send(text_event(&ServerEvent::Error { message }))
                                        .await;
                                }
                                Err(error) => {
                                    tracing::error!(%error, "device GPS publish task failed");
                                    let _ = out_tx
                                        .send(text_event(&ServerEvent::Error {
                                            message: "could not publish device position".to_owned(),
                                        }))
                                        .await;
                                }
                            }
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

    for (_, (_, task)) in spectra {
        task.abort();
    }
    for (_, (_, task)) in audio {
        task.abort();
    }
    for (_, (_, task)) in video {
        task.abort();
    }
    events.abort();
    decoded.abort();
    positions.abort();
    writer.abort();
    let live = state
        .clients
        .fetch_sub(1, atomic::Ordering::Relaxed)
        .saturating_sub(1);
    tracing::debug!(clients = live, "client disconnected");
    engine.emit_scope(StateScope::Clients);
}

/// Collapse a `spawn_blocking` result into the message for a `ServerEvent::Error`, mirroring
/// rest.rs's `JoinError` mapping so both surfaces report engine failures the same way.
fn flatten_join<T>(
    joined: Result<Result<T, sdrmm_engine::EngineError>, tokio::task::JoinError>,
) -> Result<T, String> {
    match joined {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("engine task failed: {e}")),
    }
}

/// Allocate the next stream id from `range`, wrapping within it and skipping ids still bound to a
/// live stream on this connection, so a long-lived socket can never hand a live id to a second
/// stream. `None` only once every id in the range is live.
///
/// One allocator for every kind: spectrum and the media streams draw from disjoint ranges (see
/// [`MEDIA_ID_BASE`]) but the rule is identical, and two copies of it would be two places for the
/// wrap to be wrong.
fn alloc_stream_id(
    next: &mut u16,
    range: std::ops::RangeInclusive<u16>,
    in_use: impl Fn(u16) -> bool,
) -> Option<u16> {
    let (first, last) = (*range.start(), *range.end());
    for _ in first..=last {
        let candidate = *next;
        *next = if candidate == last {
            first
        } else {
            candidate + 1
        };
        if !in_use(candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Forward engine control events losslessly. If this receiver ever lags (writer stalled long
/// enough for the engine's event buffer to wrap), the dropped invalidations are gone for good
/// and StateChanged carries no revision to detect that — so synthesize a full-scope
/// invalidation and the client refetches everything instead of rendering stale state forever
/// (: these events are the only cache-invalidation path).
fn spawn_events(
    mut event_rx: broadcast::Receiver<ServerEvent>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(ev) => {
                    if out_tx.send(text_event(&ev)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "event stream lagged; forcing full state refetch");
                    let resync = ServerEvent::StateChanged {
                        scope: StateScope::All,
                    };
                    if out_tx.send(text_event(&resync)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

fn spawn_decoded(
    mut decoded_rx: broadcast::Receiver<Utf8Bytes>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match decoded_rx.recv().await {
                Ok(text) => {
                    if out_tx.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    let lost = ServerEvent::DecodedLost { count: missed };
                    if out_tx.send(text_event(&lost)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

fn spawn_positions(
    mut position_rx: broadcast::Receiver<ServerEvent>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match position_rx.recv().await {
                Ok(event) => {
                    if out_tx.send(text_event(&event)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "GPS event stream lagged");
                    let error = ServerEvent::Error {
                        message: "GPS updates were lost; waiting for the next fix".to_owned(),
                    };
                    if out_tx.send(text_event(&error)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// Paces admissions to `fps` on the *producer's* sample clock, never on arrival time.
///
/// Snapshots do not reach this task evenly: the DSP thread drains a whole USB block at once, so
/// several frames are broadcast within the same millisecond and then nothing arrives for tens of
/// milliseconds. Any wall-clock deadline admits whatever lands first in a burst and drops the
/// rest of it, delivering the *burst* rate rather than the frame rate — measured at 3.5 fps out
/// of a ~10 fps producer with 30 requested. Each snapshot carries the sample position it was
/// taken at, which is the cadence the operator actually sees, and is immune to how the frames
/// were delivered.
///
/// The deadline accumulates by exactly one period per admission and is clamped forward to the
/// frame just admitted, so no send-debt builds up while the producer is slower than requested:
/// this delivers min(producer, fps).
struct FrameThrottle {
    fps: f64,
    /// Sample position the next admitted frame must reach.
    next: u64,
    /// Last position seen, so a capture restart can be told from a late frame.
    last: u64,
}

impl FrameThrottle {
    fn new(fps: u16) -> Self {
        Self {
            fps: f64::from(fps),
            next: 0,
            last: 0,
        }
    }

    /// `timestamp` is the snapshot's sample position, `sample_rate` its span in Hz.
    fn admit(&mut self, timestamp: u64, sample_rate: f32) -> bool {
        // A restarted capture rewinds the sample clock to zero; without re-anchoring, the
        // deadline would sit in the old stream's future and the display would never resume.
        if timestamp < self.last {
            self.next = timestamp;
        }
        self.last = timestamp;
        if timestamp < self.next {
            return false;
        }
        let period = (f64::from(sample_rate) / self.fps).max(1.0) as u64;
        self.next = self.next.saturating_add(period).max(timestamp);
        true
    }
}

#[derive(Clone, Copy)]
struct SpectrumLane {
    /// Per-connection id the client demuxes frames on.
    stream_id: u16,
    device_set: u32,
    /// Receive stream within the device set.
    stream: u32,
}

fn spawn_spectrum(
    lane: SpectrumLane,
    fps: u16,
    bins: u16,
    mut rx: broadcast::Receiver<SpectrumSnapshot>,
    out_tx: mpsc::Sender<Message>,
    engine: Arc<Engine>,
) -> tokio::task::JoinHandle<()> {
    let SpectrumLane {
        stream_id,
        device_set: ds,
        stream,
    } = lane;
    let fps = fps.clamp(1, MAX_FPS);
    let bins = (bins as usize).clamp(MIN_BINS, MAX_BINS);

    tokio::spawn(async move {
        let mut dec = vec![0f32; bins];
        let mut quant = vec![0u8; bins];
        let mut throttle = FrameThrottle::new(fps);

        loop {
            match rx.recv().await {
                Ok(snap) => {
                    if !throttle.admit(snap.timestamp, snap.span_hz) {
                        continue;
                    }

                    decimate_max(&snap.db, &mut dec);
                    let (db_min, db_max) = adaptive_db_window(&snap.db);
                    quantize_db(&dec, db_min, db_max, &mut quant);

                    let frame = SpectrumFrame {
                        stream_id,
                        seq: snap.seq,
                        timestamp: snap.timestamp,
                        center_hz: snap.center_hz,
                        span_hz: snap.span_hz,
                        db_min,
                        db_max,
                        bins: &quant,
                    }
                    .encode();

                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                // Oldest snapshots were shed while this task was backpressured — the
                // drop-oldest contract; resume with the newest retained.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    // The broadcast closes for two very different reasons: the device set was
                    // removed, or auto-reconnect ( M5) replaced its whole runtime
                    // after a replug. Re-subscribing distinguishes them — a set that came
                    // back hands out a receiver on the new runtime and the stream resumes,
                    // and only a set that is really gone gets the stop event. On the *same*
                    // lane: falling back to stream 0 here would silently switch every
                    // non-zero-stream waterfall on a replug.
                    let engine = engine.clone();
                    let resubscribed =
                        tokio::task::spawn_blocking(move || engine.subscribe_spectrum(ds, stream))
                            .await;
                    if let Ok(Ok(fresh)) = resubscribed {
                        rx = fresh;
                        continue;
                    }
                    let stopped = ServerEvent::StreamStopped {
                        stream_id,
                        kind: StreamKind::Spectrum,
                    };
                    let _ = out_tx.send(text_event(&stopped)).await;
                    break;
                }
            }
        }
    })
}

fn spawn_audio(
    stream_id: u16,
    mut rx: broadcast::Receiver<AudioPacket>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(packet) => {
                    let frame = AudioFrame {
                        stream_id,
                        seq: packet.seq,
                        timestamp: packet.timestamp,
                        ch_layout: packet.channels,
                        opus: &packet.opus,
                    }
                    .encode();

                    // Awaited on purpose: backpressure lags the broadcast receiver and the
                    // oldest packets are shed (drop-oldest).
                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                // Oldest packets were shed while this task was backpressured. The gap shows
                // up client-side as a jump in the 48 kHz sample-count timestamp; `seq` is a
                // plain packet counter, not a resync mechanism.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    let stopped = ServerEvent::StreamStopped {
                        stream_id,
                        kind: StreamKind::Audio,
                    };
                    let _ = out_tx.send(text_event(&stopped)).await;
                    break;
                }
            }
        }
    })
}

/// Whether `id` is bound to a live media stream of either kind on this connection. One check
/// across both maps, because both draw from [`MEDIA_ID_BASE`].
fn media_id_live(
    audio: &HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    video: &HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    id: u16,
) -> bool {
    audio
        .values()
        .chain(video.values())
        .any(|(sid, _)| *sid == id)
}

/// Per-subscription task: forward the channel's pictures as binary [`VideoFrame`]s with the same
/// drop-oldest backpressure as [`spawn_audio`]. A shed picture is simply a frame the client never
/// draws — unlike audio there is nothing to conceal a gap from, so the next one is the whole
/// recovery.
fn spawn_video(
    stream_id: u16,
    mut rx: broadcast::Receiver<VideoPacket>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(packet) => {
                    let frame = VideoFrame {
                        stream_id,
                        seq: packet.seq,
                        timestamp: packet.timestamp,
                        width: packet.picture.width,
                        height: packet.picture.height,
                        luma: &packet.picture.luma,
                    }
                    .encode();

                    // Awaited on purpose: backpressure lags the broadcast receiver and the
                    // oldest pictures are shed (drop-oldest).
                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    let stopped = ServerEvent::StreamStopped {
                        stream_id,
                        kind: StreamKind::Video,
                    };
                    let _ = out_tx.send(text_event(&stopped)).await;
                    break;
                }
            }
        }
    })
}

fn text_event(ev: &ServerEvent) -> Message {
    Message::Text(encode_event(ev))
}

fn encode_event(ev: &ServerEvent) -> Utf8Bytes {
    match serde_json::to_string(ev) {
        Ok(json) => json.into(),
        // Serializing our own enum cannot realistically fail; emit a minimal error frame.
        Err(_) => r#"{"type":"Error","data":{"message":"event serialization failed"}}"#.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::IntoFuture,
        time::{Duration, Instant},
    };

    use sdrmm_wire::{
        ChannelParams, ChannelSettings, GpsNode, NfmParams, PatchNode, Position, PositionFix,
        PositionSource, WorkspaceSnapshot,
    };
    use tokio::time::timeout;
    use tokio_tungstenite::tungstenite;

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    /// Hermetic engine: virtual driver only (: no hardware in CI, ever).
    fn test_engine() -> Arc<Engine> {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        Engine::with_registry(registry, None)
    }

    type WsClient = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn connect(engine: Arc<Engine>) -> WsClient {
        let (addr, _state) = serve_ws(engine).await;
        dial(addr).await
    }

    /// The same server, but handing back its address and state so a test can observe what the
    /// hub does to shared state (client count) or open a second connection to it.
    async fn serve_ws(engine: Arc<Engine>) -> (std::net::SocketAddr, AppState) {
        let store = Arc::new(crate::Store::open(None).expect("in-memory store"));
        let state = AppState::new(engine, store);
        let (app, writer) =
            crate::router_with_state(state.clone(), &crate::ServerOptions::default());
        writer.detach();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(axum::serve(listener, app).into_future());
        (addr, state)
    }

    async fn dial(addr: std::net::SocketAddr) -> WsClient {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws"))
            .await
            .expect("connect");
        ws
    }

    async fn send(ws: &mut WsClient, cmd: &ClientCommand) {
        let json = serde_json::to_string(cmd).expect("serialize");
        ws.send(tungstenite::Message::text(json))
            .await
            .expect("send");
    }

    /// Next JSON event, skipping interleaved binary frames.
    async fn next_event(ws: &mut WsClient) -> ServerEvent {
        loop {
            let msg = timeout(WAIT, ws.next())
                .await
                .expect("timed out waiting for event")
                .expect("socket ended")
                .expect("socket error");
            if let tungstenite::Message::Text(text) = msg {
                return serde_json::from_str(text.as_str()).expect("event json");
            }
        }
    }

    fn position_fix(latitude: f64) -> PositionFix {
        PositionFix {
            latitude,
            longitude: 13.405,
            altitude_m: None,
            accuracy_m: Some(4.0),
            speed_mps: None,
            track_deg: None,
            time: "2026-08-14T12:00:00Z".to_owned(),
        }
    }

    fn activate_device_gps(state: &AppState) {
        state
            .gps
            .set_device_publish_interval(Duration::from_secs(5));
        let mut snapshot = WorkspaceSnapshot::empty();
        snapshot.graph.nodes.push(PatchNode {
            id: "position".to_owned(),
            body: sdrmm_wire::NodeBody::Gps(GpsNode {
                source: PositionSource::Device,
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        });
        let workspace = state
            .store
            .create_workspace("mobile", &snapshot)
            .expect("workspace");
        state
            .store
            .activate_workspace(workspace)
            .expect("activate workspace");
        state.gps.reconcile(state);
    }

    /// Next binary frame's (kind, stream_id) header fields, skipping text events.
    async fn next_frame_header(ws: &mut WsClient) -> (u8, u16) {
        loop {
            let msg = timeout(WAIT, ws.next())
                .await
                .expect("timed out waiting for frame")
                .expect("socket ended")
                .expect("socket error");
            if let tungstenite::Message::Binary(buf) = msg {
                let id = u16::from_le_bytes([buf[2], buf[3]]);
                return (buf[1], id);
            }
        }
    }

    fn nfm_channel(offset_hz: f64) -> ChannelSettings {
        ChannelSettings {
            offset_hz,
            squelch_db: None,
            params: ChannelParams::Nfm(NfmParams::default()),
        }
    }

    fn atv_channel() -> ChannelSettings {
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            params: ChannelParams::Atv(sdrmm_wire::AtvParams::default()),
        }
    }

    /// The video subscription's lifecycle, and the rule that keeps a panel honest: a channel that
    /// scans out pictures gets an id from the same media range audio does — never one already in
    /// use by an audio stream on this socket — and a channel that scans out nothing is refused
    /// rather than handed a stream that would stay empty.
    #[tokio::test(flavor = "multi_thread")]
    async fn video_lifecycle_shares_the_media_id_space_and_refuses_silent_channels() {
        let engine = test_engine();
        let ds = engine
            .create_device_set("virtual:siggen")
            .expect("device set");
        let voice = engine
            .add_channel(ds, 0, nfm_channel(0.0))
            .expect("nfm channel");
        let picture = engine
            .add_channel(ds, 0, atv_channel())
            .expect("atv channel");
        let mut ws = connect(engine).await;

        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Hello { .. }
        ));

        send(
            &mut ws,
            &ClientCommand::SubscribeAudio {
                device_set: ds,
                channel: voice,
            },
        )
        .await;
        let audio_id = match next_event(&mut ws).await {
            ServerEvent::AudioStreamStarted { stream_id, .. } => stream_id,
            other => panic!("expected AudioStreamStarted, got {other:?}"),
        };

        send(
            &mut ws,
            &ClientCommand::SubscribeVideo {
                device_set: ds,
                channel: picture,
            },
        )
        .await;
        let video_id = match next_event(&mut ws).await {
            ServerEvent::VideoStreamStarted {
                stream_id,
                device_set,
                channel,
            } => {
                assert!(
                    stream_id >= MEDIA_ID_BASE,
                    "video id {stream_id:#x} collides with the spectrum range"
                );
                assert_eq!((device_set, channel), (ds, picture));
                stream_id
            }
            other => panic!("expected VideoStreamStarted, got {other:?}"),
        };
        assert_ne!(
            video_id, audio_id,
            "a live audio id must not be handed to a video stream"
        );

        send(
            &mut ws,
            &ClientCommand::UnsubscribeVideo {
                device_set: ds,
                channel: picture,
            },
        )
        .await;
        match next_event(&mut ws).await {
            ServerEvent::StreamStopped { stream_id, kind } => {
                assert_eq!(stream_id, video_id);
                assert_eq!(kind, StreamKind::Video);
            }
            other => panic!("expected StreamStopped, got {other:?}"),
        }

        // A mode with no picture: the refusal has to name the mode, not just say no.
        send(
            &mut ws,
            &ClientCommand::SubscribeVideo {
                device_set: ds,
                channel: voice,
            },
        )
        .await;
        match next_event(&mut ws).await {
            ServerEvent::Error { message } => {
                assert!(message.contains("no video"), "unhelpful: {message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// Two scopes on two lanes of one radio is the whole point of a multi-stream device, so the
    /// two subscriptions must coexist: distinct ids, independent frames, and unsubscribing one
    /// leaving the other running. A spectrum id used to *be* the device-set id, which made the
    /// second subscribe replace the first and made unsubscribe ambiguous.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_lanes_of_one_radio_stream_independently() {
        let engine = test_engine();
        let ds = engine
            .create_device_set("virtual:array4")
            .expect("the four-stream virtual array");
        let mut ws = connect(engine).await;
        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Hello { .. }
        ));

        let mut subscribe = async |stream: u32| {
            send(
                &mut ws,
                &ClientCommand::SubscribeSpectrum {
                    device_set: ds,
                    fps: 30,
                    bins: 64,
                    stream,
                },
            )
            .await;
            loop {
                if let ServerEvent::StreamStarted {
                    stream_id,
                    device_set,
                    stream: lane,
                } = next_event(&mut ws).await
                {
                    assert_eq!(device_set, ds);
                    assert_eq!(lane, stream);
                    return stream_id;
                }
            }
        };
        let lane0 = subscribe(0).await;
        let lane2 = subscribe(2).await;
        assert_ne!(lane0, lane2, "each lane needs an id of its own to demux on");

        // Both lanes deliver, which they cannot do if one replaced the other.
        let mut seen = std::collections::HashSet::new();
        while seen.len() < 2 {
            let (kind, stream_id) = next_frame_header(&mut ws).await;
            assert_eq!(kind, sdrmm_wire::FrameKind::Spectrum as u8);
            assert!(stream_id == lane0 || stream_id == lane2, "{stream_id}");
            seen.insert(stream_id);
        }

        send(
            &mut ws,
            &ClientCommand::UnsubscribeSpectrum {
                device_set: ds,
                stream: 0,
            },
        )
        .await;
        loop {
            if let ServerEvent::StreamStopped { stream_id, kind } = next_event(&mut ws).await {
                assert_eq!(stream_id, lane0, "only the named lane stops");
                assert_eq!(kind, StreamKind::Spectrum);
                break;
            }
        }

        // The surviving lane is still delivering: unsubscribing one scope must not silence the
        // other, which keying by device set alone did.
        for _ in 0..8 {
            let (_, stream_id) = next_frame_header(&mut ws).await;
            if stream_id == lane2 {
                return;
            }
        }
        panic!("the other lane stopped when its neighbour unsubscribed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn spectrum_lifecycle_reports_kind_and_streams_frames() {
        let engine = test_engine();
        let ds = engine
            .create_device_set("virtual:siggen")
            .expect("device set");
        let mut ws = connect(engine).await;

        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Hello { .. }
        ));

        send(
            &mut ws,
            &ClientCommand::SubscribeSpectrum {
                device_set: ds,
                fps: 30,
                bins: 64,
                stream: 0,
            },
        )
        .await;
        // The id is allocated per connection, so the frames and the stop event are matched
        // against what StreamStarted handed out — not against the device-set id, which stopped
        // being the stream id when one radio could have several lanes watched at once.
        let allocated = match next_event(&mut ws).await {
            ServerEvent::StreamStarted {
                stream_id,
                device_set,
                stream,
            } => {
                assert_eq!(device_set, ds);
                assert_eq!(stream, 0);
                stream_id
            }
            other => panic!("expected StreamStarted, got {other:?}"),
        };
        let (kind, stream_id) = next_frame_header(&mut ws).await;
        assert_eq!(kind, sdrmm_wire::FrameKind::Spectrum as u8);
        assert_eq!(stream_id, allocated);

        send(
            &mut ws,
            &ClientCommand::UnsubscribeSpectrum {
                device_set: ds,
                stream: 0,
            },
        )
        .await;
        match next_event(&mut ws).await {
            ServerEvent::StreamStopped { stream_id, kind } => {
                assert_eq!(stream_id, allocated);
                assert_eq!(kind, StreamKind::Spectrum);
            }
            other => panic!("expected StreamStopped, got {other:?}"),
        }

        // A set id inside the reserved audio range must be refused, not aliased.
        send(
            &mut ws,
            &ClientCommand::SubscribeSpectrum {
                device_set: u32::from(MEDIA_ID_BASE),
                fps: 30,
                bins: 64,
                stream: 0,
            },
        )
        .await;
        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Error { .. }
        ));
    }

    /// The lane is the client's to name: a stream this radio does not have must come back as
    /// an engine refusal, which only happens if the subscribe carries the stream it asked for
    /// instead of quietly landing on lane 0.
    #[tokio::test(flavor = "multi_thread")]
    async fn spectrum_subscribe_routes_the_named_stream_to_the_engine() {
        let engine = test_engine();
        let ds = engine
            .create_device_set("virtual:siggen")
            .expect("device set");
        let mut ws = connect(engine).await;
        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Hello { .. }
        ));

        send(
            &mut ws,
            &ClientCommand::SubscribeSpectrum {
                device_set: ds,
                fps: 30,
                bins: 64,
                stream: 7,
            },
        )
        .await;
        match next_event(&mut ws).await {
            ServerEvent::Error { message } => {
                assert!(message.contains("rx streams"), "unhelpful: {message}");
            }
            other => panic!("expected an engine refusal, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn audio_ids_disjoint_from_spectrum_and_duplicate_subscribe_stops_old() {
        let engine = test_engine();
        let ds = engine
            .create_device_set("virtual:siggen")
            .expect("device set");
        let ch = engine
            .add_channel(ds, 0, nfm_channel(0.0))
            .expect("channel");
        let mut ws = connect(engine).await;

        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Hello { .. }
        ));

        let subscribe = ClientCommand::SubscribeAudio {
            device_set: ds,
            channel: ch,
        };
        send(&mut ws, &subscribe).await;
        let first_id = match next_event(&mut ws).await {
            ServerEvent::AudioStreamStarted {
                stream_id,
                device_set,
                channel,
            } => {
                assert!(
                    stream_id >= MEDIA_ID_BASE,
                    "audio id {stream_id:#x} collides with spectrum range"
                );
                assert_eq!(device_set, ds);
                assert_eq!(channel, ch);
                stream_id
            }
            other => panic!("expected AudioStreamStarted, got {other:?}"),
        };

        // Duplicate subscribe: the old stream must stop loudly before the new one starts.
        send(&mut ws, &subscribe).await;
        match next_event(&mut ws).await {
            ServerEvent::StreamStopped { stream_id, kind } => {
                assert_eq!(stream_id, first_id);
                assert_eq!(kind, StreamKind::Audio);
            }
            other => panic!("expected StreamStopped for the replaced stream, got {other:?}"),
        }
        let second_id = match next_event(&mut ws).await {
            ServerEvent::AudioStreamStarted { stream_id, .. } => stream_id,
            other => panic!("expected AudioStreamStarted, got {other:?}"),
        };
        assert_ne!(second_id, first_id);
        assert!(second_id >= MEDIA_ID_BASE);

        send(
            &mut ws,
            &ClientCommand::UnsubscribeAudio {
                device_set: ds,
                channel: ch,
            },
        )
        .await;
        match next_event(&mut ws).await {
            ServerEvent::StreamStopped { stream_id, kind } => {
                assert_eq!(stream_id, second_id);
                assert_eq!(kind, StreamKind::Audio);
            }
            other => panic!("expected StreamStopped, got {other:?}"),
        }
    }

    #[test]
    fn audio_id_allocator_wraps_within_range_and_skips_live_ids() {
        let mut next = MEDIA_ID_BASE;
        let live = [MEDIA_ID_BASE, MEDIA_ID_BASE + 1];
        assert_eq!(
            alloc_stream_id(&mut next, MEDIA_ID_BASE..=u16::MAX, |id| live.contains(&id)),
            Some(MEDIA_ID_BASE + 2)
        );

        let mut next = u16::MAX;
        assert_eq!(
            alloc_stream_id(&mut next, MEDIA_ID_BASE..=u16::MAX, |_| false),
            Some(u16::MAX)
        );
        assert_eq!(
            next, MEDIA_ID_BASE,
            "must wrap into the audio range, not to 0"
        );

        let mut next = MEDIA_ID_BASE;
        assert_eq!(
            alloc_stream_id(&mut next, MEDIA_ID_BASE..=u16::MAX, |_| true),
            None
        );
    }

    /// The producer emits every `rate / 30` samples whatever the wall clock does. Requesting at
    /// or above that must deliver every frame, and below it must decimate exactly — this is the
    /// whole point of pacing on the sample clock, and the arrival pattern never enters into it.
    #[test]
    fn throttle_paces_on_the_sample_clock_not_on_arrival() {
        const RATE: f32 = 2_000_000.0;
        let hop = (RATE / 30.0) as u64;
        for (fps, expected) in [(30u16, 300i64), (20, 200), (10, 100), (60, 300)] {
            let mut throttle = FrameThrottle::new(fps);
            let mut admitted: i64 = 0;
            for i in 0..300 {
                if throttle.admit(i * hop, RATE) {
                    admitted += 1;
                }
            }
            assert!(
                (admitted - expected).abs() <= 3,
                "fps {fps}: delivered {admitted}, wanted ~{expected}"
            );
        }
    }

    /// The regression this replaced a wall-clock throttle for: a USB block carries several hops,
    /// so the frames it produces are broadcast back to back. Every one of them must still be
    /// admitted at the requested rate.
    #[test]
    fn throttle_admits_a_whole_burst_of_frames() {
        const RATE: f32 = 2_000_000.0;
        let hop = (RATE / 30.0) as u64;
        let mut throttle = FrameThrottle::new(30);
        let mut admitted = 0;
        // Eight hops per block, ten blocks: 80 frames arriving in ten bursts.
        for i in 0..80u64 {
            if throttle.admit(i * hop, RATE) {
                admitted += 1;
            }
        }
        assert_eq!(admitted, 80, "a burst must not be collapsed to one frame");
    }

    /// A replugged device restarts its sample clock at zero; the stream has to resume rather
    /// than wait out the old capture's deadline.
    #[test]
    fn throttle_reanchors_when_the_capture_restarts() {
        const RATE: f32 = 2_000_000.0;
        let hop = (RATE / 30.0) as u64;
        let mut throttle = FrameThrottle::new(30);
        for i in 0..100u64 {
            throttle.admit(i * hop, RATE);
        }
        assert!(throttle.admit(0, RATE), "the restarted stream must resume");
        assert!(throttle.admit(hop, RATE));
    }

    /// Regression for drop-newest backpressure: with a stalled writer, the newest frame must
    /// survive (the old try_send path shed it) and the stale backlog must be bounded by the
    /// broadcast capacity, not the out-queue capacity.
    #[tokio::test(flavor = "multi_thread")]
    async fn audio_forwarder_sheds_oldest_and_delivers_newest() {
        let (tx, rx) = broadcast::channel::<AudioPacket>(4);
        let (out_tx, mut out_rx) = mpsc::channel::<Message>(1);
        let task = spawn_audio(MEDIA_ID_BASE, rx, out_tx);

        let last_seq = 20u32;
        for seq in 0..=last_seq {
            tx.send(AudioPacket {
                seq,
                timestamp: u64::from(seq) * 960,
                channels: 1,
                opus: Arc::from(&[0u8; 4][..]),
            })
            .expect("send");
        }
        drop(tx);

        let mut seqs = Vec::new();
        let mut stopped = false;
        loop {
            let Ok(msg) = timeout(WAIT, out_rx.recv()).await else {
                panic!("timed out draining forwarder");
            };
            let Some(msg) = msg else { break };
            match msg {
                Message::Binary(buf) => {
                    seqs.push(u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]));
                }
                Message::Text(text) => {
                    let ev: ServerEvent = serde_json::from_str(&text).expect("event json");
                    assert!(
                        matches!(
                            ev,
                            ServerEvent::StreamStopped {
                                kind: StreamKind::Audio,
                                ..
                            }
                        ),
                        "unexpected event {ev:?}"
                    );
                    stopped = true;
                }
                other => panic!("unexpected message {other:?}"),
            }
        }
        task.await.expect("forwarder task");

        assert!(stopped, "closed stream must end with StreamStopped");
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "out of order: {seqs:?}"
        );
        assert_eq!(
            seqs.last(),
            Some(&last_seq),
            "newest frame was shed: {seqs:?}"
        );
        assert!(
            seqs.len() < last_seq as usize + 1,
            "stale backlog was not shed: {seqs:?}"
        );
    }

    /// The layout byte is the packet's own, not a constant: a channel that switches to stereo
    /// mid-stream (WFM's stereo toggle) has to be announced frame by frame, or the client
    /// keeps decoding the new packets with the old channel count.
    #[tokio::test(flavor = "multi_thread")]
    async fn audio_frames_carry_each_packets_channel_layout() {
        let (tx, rx) = broadcast::channel::<AudioPacket>(4);
        let (out_tx, mut out_rx) = mpsc::channel::<Message>(4);
        let task = spawn_audio(MEDIA_ID_BASE, rx, out_tx);

        for (seq, channels) in [(0u32, 1u8), (1, 2)] {
            tx.send(AudioPacket {
                seq,
                timestamp: u64::from(seq) * 960,
                channels,
                opus: Arc::from(&[0u8; 4][..]),
            })
            .expect("send");
            let Ok(Some(Message::Binary(buf))) = timeout(WAIT, out_rx.recv()).await else {
                panic!("no audio frame for seq {seq}");
            };
            assert_eq!(buf[16], channels, "layout of packet {seq}");
        }

        drop(tx);
        // Drain the StreamStopped the closed broadcast produces so the task can finish.
        let _ = timeout(WAIT, out_rx.recv()).await;
        task.await.expect("forwarder task");
    }

    /// Connections are counted for every client and released on disconnect, and each change
    /// invalidates the `clients` scope — a count that only ever grew would be worse than none.
    #[tokio::test]
    async fn client_count_tracks_connections_and_invalidates() {
        let engine = test_engine();
        let mut events = engine.subscribe_events();
        let (addr, state) = serve_ws(engine).await;
        let count = || state.clients.load(atomic::Ordering::Relaxed);

        let first = dial(addr).await;
        wait_for(&mut events, StateScope::Clients).await;
        assert_eq!(count(), 1);

        let second = dial(addr).await;
        wait_for(&mut events, StateScope::Clients).await;
        assert_eq!(count(), 2);

        drop(second);
        wait_for(&mut events, StateScope::Clients).await;
        assert_eq!(count(), 1);
        drop(first);
        wait_for(&mut events, StateScope::Clients).await;
        assert_eq!(count(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rotating_position_node_ids_share_one_connection_budget() {
        let mut ws = connect(test_engine()).await;
        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Hello { .. }
        ));

        send(
            &mut ws,
            &ClientCommand::PublishPosition {
                node: "first".to_owned(),
                fix: Some(position_fix(52.52)),
                error: None,
            },
        )
        .await;
        send(
            &mut ws,
            &ClientCommand::PublishPosition {
                node: "second".to_owned(),
                fix: Some(position_fix(52.53)),
                error: None,
            },
        )
        .await;
        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Error { message }
                if message.contains("not a device GPS source")
        ));
        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Error { message }
                if message.contains("per connection")
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_sockets_share_the_device_source_budget() {
        let (addr, state) = serve_ws(test_engine()).await;
        activate_device_gps(&state);
        let mut first = dial(addr).await;
        let mut second = dial(addr).await;
        assert!(matches!(
            next_event(&mut first).await,
            ServerEvent::Hello { .. }
        ));
        assert!(matches!(
            next_event(&mut second).await,
            ServerEvent::Hello { .. }
        ));
        let _ = next_event(&mut first).await;
        let _ = next_event(&mut second).await;

        send(
            &mut first,
            &ClientCommand::PublishPosition {
                node: "position".to_owned(),
                fix: Some(position_fix(52.52)),
                error: None,
            },
        )
        .await;
        send(
            &mut second,
            &ClientCommand::PublishPosition {
                node: "position".to_owned(),
                fix: Some(position_fix(52.53)),
                error: None,
            },
        )
        .await;

        let mut saw_fix = false;
        let mut saw_limit = false;
        let deadline = Instant::now() + WAIT;
        while !(saw_fix && saw_limit) {
            assert!(
                Instant::now() < deadline,
                "shared GPS budget events timed out"
            );
            let event = tokio::select! {
                event = next_event(&mut first) => event,
                event = next_event(&mut second) => event,
            };
            match event {
                ServerEvent::PositionChanged { fix: Some(_), .. } => saw_fix = true,
                ServerEvent::Error { message } if message.contains("per node") => {
                    saw_limit = true;
                }
                _ => {}
            }
        }
    }

    async fn wait_for(events: &mut broadcast::Receiver<ServerEvent>, scope: StateScope) {
        let deadline = Instant::now() + WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let ev = timeout(remaining, events.recv())
                .await
                .expect("scope within timeout")
                .expect("event");
            if matches!(ev, ServerEvent::StateChanged { scope: got } if got == scope) {
                return;
            }
        }
    }

    /// Auto-reconnect replaces a device set's whole runtime, which closes the spectrum
    /// broadcast every subscriber is holding. The stream must survive that: before this, a
    /// replug left every client's waterfall frozen until it re-opened the page.
    #[tokio::test(flavor = "multi_thread")]
    async fn spectrum_survives_a_reconnect_instead_of_stopping() {
        let die = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(FaultingDriver { die: die.clone() }));
        let engine = Engine::with_registry(registry, None);
        let (addr, state) = serve_ws(engine).await;
        let ds = state
            .engine
            .create_device_set("mock:faulting")
            .expect("device set");

        let mut ws = dial(addr).await;
        send(
            &mut ws,
            &ClientCommand::SubscribeSpectrum {
                device_set: ds,
                fps: 30,
                bins: 64,
                stream: 0,
            },
        )
        .await;
        assert_eq!(next_frame_header(&mut ws).await.1, ds as u16);

        die.store(true, std::sync::atomic::Ordering::SeqCst);
        let deadline = Instant::now() + WAIT;
        while state.engine.snapshot().device_sets[0].status != sdrmm_wire::DeviceSetStatus::Error {
            assert!(Instant::now() < deadline, "the device never faulted");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        die.store(false, std::sync::atomic::Ordering::SeqCst);
        state
            .engine
            .hotplug_tick_for_test(&mut None, &mut std::collections::HashSet::new());
        assert_eq!(
            state.engine.snapshot().device_sets[0].status,
            sdrmm_wire::DeviceSetStatus::Running
        );

        let deadline = Instant::now() + WAIT;
        loop {
            let msg = timeout(WAIT, ws.next())
                .await
                .expect("message within timeout")
                .expect("stream open")
                .expect("message");
            match msg {
                tungstenite::Message::Binary(bytes) => {
                    if bytes.first() == Some(&sdrmm_wire::PROTOCOL_VERSION) {
                        break;
                    }
                }
                tungstenite::Message::Text(text) => {
                    let ev: ServerEvent = serde_json::from_str(&text).expect("event");
                    assert!(
                        !matches!(ev, ServerEvent::StreamStopped { .. }),
                        "the stream was torn down instead of following the reconnect"
                    );
                }
                _ => {}
            }
            assert!(Instant::now() < deadline, "no spectrum after the reconnect");
        }
    }

    /// Streams paced blocks until told to die, and probes present throughout — an unplug the
    /// hotplug tick can recover from.
    struct FaultingDriver {
        die: Arc<std::sync::atomic::AtomicBool>,
    }

    impl sdrmm_device::DeviceDriver for FaultingDriver {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn probe(&self) -> Vec<sdrmm_wire::DeviceInfo> {
            vec![sdrmm_wire::DeviceInfo {
                driver: "mock".to_string(),
                key: "faulting".to_string(),
                label: "Faulting mock".to_string(),
                serial: None,
                profile: None,
            }]
        }

        fn open(
            &self,
            _info: &sdrmm_wire::DeviceInfo,
        ) -> Result<Box<dyn sdrmm_device::SdrDevice>, sdrmm_device::DeviceError> {
            Ok(Box::new(FaultingDevice {
                capabilities: sdrmm_wire::Capabilities {
                    freq_ranges: Vec::new(),
                    sample_rates: Vec::new(),
                    sample_rate_range: None,
                    gains: Vec::new(),
                    antennas: Vec::new(),
                    bandwidths: Vec::new(),
                    extra: Vec::new(),
                    ppm: false,
                    duplex: sdrmm_wire::Duplex::RxOnly,
                    rx_streams: 1,
                    tx_streams: 0,
                    per_stream: sdrmm_wire::StreamScope::default(),
                    directional: None,
                },
                settings: sdrmm_wire::DeviceSettings {
                    sample_rate: Some(2_048_000.0),
                    ..sdrmm_wire::DeviceSettings::default()
                },
                die: self.die.clone(),
                stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                worker: None,
            }))
        }
    }

    struct FaultingDevice {
        capabilities: sdrmm_wire::Capabilities,
        settings: sdrmm_wire::DeviceSettings,
        die: Arc<std::sync::atomic::AtomicBool>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl sdrmm_device::SdrDevice for FaultingDevice {
        fn capabilities(&self) -> &sdrmm_wire::Capabilities {
            &self.capabilities
        }

        fn settings(&self) -> &sdrmm_wire::DeviceSettings {
            &self.settings
        }

        fn apply(
            &mut self,
            settings: &sdrmm_wire::DeviceSettings,
        ) -> Result<(), sdrmm_device::DeviceError> {
            self.settings.merge_from(settings);
            Ok(())
        }

        fn rx_start(
            &mut self,
            sinks: Vec<sdrmm_device::RxSink>,
        ) -> Result<(), sdrmm_device::DeviceError> {
            let mut sink = sdrmm_device::single_rx_sink(sinks)?;
            let die = self.die.clone();
            let stop = self.stop.clone();
            self.worker = Some(std::thread::spawn(move || {
                let block = [num_complex::Complex::new(0.1f32, 0.0); 4_096];
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    if die.load(std::sync::atomic::Ordering::SeqCst) {
                        sink.fail(sdrmm_device::DeviceError::Io("mock died".to_string()));
                        return;
                    }
                    sink.push(&block);
                    std::thread::sleep(Duration::from_millis(2));
                }
            }));
            Ok(())
        }

        fn rx_stop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    /// Decoder frames are serialized once for the whole server; every connection must still
    /// receive the identical frame (the shared-encoder path is the only one that runs now).
    #[tokio::test]
    async fn decoded_frames_reach_every_connection_from_one_encoding() {
        let engine = test_engine();
        let (addr, state) = serve_ws(engine).await;
        let mut a = dial(addr).await;
        let mut b = dial(addr).await;

        let record = sdrmm_wire::DecodedRecord {
            device_set: 1,
            channel: 2,
            at: "2026-08-09T12:00:00.000000000Z".to_string(),
            freq_hz: 1_090_000_000.0,
            event: sdrmm_wire::DecoderEvent::Adsb(sdrmm_wire::AdsbMessage {
                icao: "3C6444".to_string(),
                df: 17,
                raw: "8D3C6444".to_string(),
                ..sdrmm_wire::AdsbMessage::default()
            }),
        };
        let encoded = encode_event(&ServerEvent::Decoded(Box::new(record.clone())));
        state.decoded_text.send(encoded).expect("subscribers");

        // A fresh connection also gets a Hello and the connect/disconnect scope events; skip
        // to the frame under test rather than asserting on the ordering of unrelated ones.
        for ws in [&mut a, &mut b] {
            loop {
                if let ServerEvent::Decoded(got) = next_event(ws).await {
                    assert_eq!(*got, record);
                    break;
                }
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_late_client_is_handed_the_recent_past() {
        let engine = test_engine();
        let (addr, state) = serve_ws(engine).await;

        let record = sdrmm_wire::DecodedRecord {
            device_set: 1,
            channel: 2,
            at: "2026-08-09T12:00:00.000000000Z".to_string(),
            freq_hz: 1_090_000_000.0,
            event: sdrmm_wire::DecoderEvent::Adsb(sdrmm_wire::AdsbMessage {
                icao: "3C6444".to_string(),
                df: 17,
                raw: "8D3C6444".to_string(),
                ..sdrmm_wire::AdsbMessage::default()
            }),
        };
        state.tracks.observe(&record);

        let mut ws = dial(addr).await;
        assert!(
            matches!(next_event(&mut ws).await, ServerEvent::Hello { .. }),
            "the backlog follows Hello, never precedes it"
        );
        loop {
            if let ServerEvent::DecodedBacklog { records } = next_event(&mut ws).await {
                assert_eq!(records, vec![record]);
                break;
            }
        }
    }

    /// Nothing heard yet means no message at all, not an empty one.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_silent_server_sends_no_backlog() {
        let engine = test_engine();
        let (addr, state) = serve_ws(engine).await;
        let mut ws = dial(addr).await;
        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Hello { .. }
        ));

        // A frame published after connect proves the socket is live and that nothing came before
        // it: a backlog message would have had to arrive in between.
        let record = sdrmm_wire::DecodedRecord {
            device_set: 0,
            channel: 0,
            at: "2026-08-09T12:00:01.000000000Z".to_string(),
            freq_hz: 1_090_000_000.0,
            event: sdrmm_wire::DecoderEvent::Adsb(sdrmm_wire::AdsbMessage {
                icao: "4CA1FA".to_string(),
                df: 17,
                raw: "8D4CA1FA".to_string(),
                ..sdrmm_wire::AdsbMessage::default()
            }),
        };
        state
            .decoded_text
            .send(encode_event(&ServerEvent::Decoded(Box::new(
                record.clone(),
            ))))
            .expect("subscribers");

        loop {
            match next_event(&mut ws).await {
                ServerEvent::DecodedBacklog { .. } => panic!("nothing had been decoded yet"),
                ServerEvent::Decoded(got) => {
                    assert_eq!(*got, record);
                    break;
                }
                _ => {}
            }
        }
    }

    /// A lagged event receiver has lost invalidations for good; the forwarder must synthesize
    /// a full-scope StateChanged so the client refetches instead of going permanently stale.
    #[tokio::test(flavor = "multi_thread")]
    async fn event_forwarder_lag_synthesizes_full_invalidation() {
        let (tx, rx) = broadcast::channel::<ServerEvent>(2);
        // Overflow the buffer before the forwarder first polls: guaranteed Lagged on recv.
        for i in 0..5 {
            tx.send(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(i),
            })
            .expect("send");
        }
        let (out_tx, mut out_rx) = mpsc::channel::<Message>(16);
        let task = spawn_events(rx, out_tx);

        let first = timeout(WAIT, out_rx.recv())
            .await
            .expect("timed out")
            .expect("closed");
        let Message::Text(text) = first else {
            panic!("expected text event");
        };
        let ev: ServerEvent = serde_json::from_str(&text).expect("event json");
        assert_eq!(
            ev,
            ServerEvent::StateChanged {
                scope: StateScope::All
            }
        );

        let mut followed = Vec::new();
        drop(tx);
        while let Some(Message::Text(text)) = out_rx.recv().await {
            followed.push(serde_json::from_str::<ServerEvent>(&text).expect("event json"));
        }
        task.await.expect("events task");
        assert_eq!(
            followed,
            vec![
                ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(3)
                },
                ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(4)
                },
            ]
        );
    }
}
