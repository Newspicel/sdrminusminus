//! WebSocket hub (PLAN §5): one socket per client carrying JSON events + client commands and
//! binary spectrum/audio frames. A single writer task owns the sink; event and
//! per-subscription stream tasks feed it through a bounded mpsc. Stream tasks *await* that
//! send: a full queue backpressures the task, its broadcast receiver lags, and tokio's
//! broadcast sheds the oldest entries — drop-oldest per connection (PLAN §5). Control events
//! stay lossless, with a full-state resync if their receiver ever lags.

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
use sdrmm_engine::{AudioPacket, Engine, SpectrumSnapshot, adaptive_db_window};
use sdrmm_wire::{AudioFrame, ClientCommand, ServerEvent, SpectrumFrame, StateScope, StreamKind};
use tokio::sync::{broadcast, mpsc};

use crate::AppState;

const OUT_CHANNEL_CAP: usize = 256;
const MIN_BINS: usize = 16;
const MAX_BINS: usize = 4096;
const MAX_FPS: u16 = 60;
/// `ch_layout` header byte for mono audio frames (PLAN §5 frame layout).
const CH_LAYOUT_MONO: u8 = 1;
/// Audio stream ids live in `AUDIO_ID_BASE..=u16::MAX`; spectrum stream ids are device-set
/// ids and must stay below it, so the two can never collide on one connection.
const AUDIO_ID_BASE: u16 = 0x8000;

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
    let event_rx = engine.subscribe_events();
    let hello = ServerEvent::Hello {
        revision: engine.snapshot().revision,
    };
    let _ = out_tx.send(text_event(&hello)).await;

    let events = spawn_events(event_rx, out_tx.clone());

    let mut spectra: HashMap<u32, tokio::task::JoinHandle<()>> = HashMap::new();
    // Audio streams keyed by (device_set, channel); resubscribing replaces the running task.
    let mut audio: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)> = HashMap::new();
    let mut next_audio_id: u16 = AUDIO_ID_BASE;

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
                        // The spectrum stream_id is the device-set id; refuse ids that would
                        // land in the reserved audio range rather than silently alias.
                        if device_set >= u32::from(AUDIO_ID_BASE) {
                            let err = ServerEvent::Error {
                                message: format!("device set {device_set} exceeds stream id range"),
                            };
                            let _ = out_tx.send(text_event(&err)).await;
                            continue;
                        }
                        // subscribe_spectrum takes the per-set runtime mutex, which
                        // patch/remove hold across device I/O and thread joins — blocking
                        // pool, like every hardware-reaching call in rest.rs.
                        let subscribe = {
                            let engine = engine.clone();
                            tokio::task::spawn_blocking(move || {
                                engine.subscribe_spectrum(device_set)
                            })
                            .await
                        };
                        match flatten_join(subscribe) {
                            Ok(rx) => {
                                if let Some(old) = spectra.remove(&device_set) {
                                    old.abort();
                                }
                                let started = ServerEvent::StreamStarted {
                                    stream_id: device_set as u16,
                                    device_set,
                                };
                                let _ = out_tx.send(text_event(&started)).await;
                                let task =
                                    spawn_spectrum(device_set, fps, bins, rx, out_tx.clone());
                                spectra.insert(device_set, task);
                            }
                            Err(message) => {
                                let _ = out_tx
                                    .send(text_event(&ServerEvent::Error { message }))
                                    .await;
                            }
                        }
                    }
                    Ok(ClientCommand::UnsubscribeSpectrum { device_set }) => {
                        if let Some(task) = spectra.remove(&device_set) {
                            task.abort();
                            let stopped = ServerEvent::StreamStopped {
                                stream_id: device_set as u16,
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
                                let live = |id: u16| audio.values().any(|(sid, _)| *sid == id);
                                match alloc_audio_id(&mut next_audio_id, live) {
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
                                            message: "no free audio stream ids on this connection"
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
    for (_, (_, task)) in audio {
        task.abort();
    }
    events.abort();
    writer.abort();
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

/// Allocate the next audio stream id from `AUDIO_ID_BASE..=u16::MAX`, wrapping within that
/// range and skipping ids still bound to a live stream on this connection, so a long-lived
/// socket can never hand a live id to a second stream. `None` only if all 32768 ids are live.
fn alloc_audio_id(next: &mut u16, in_use: impl Fn(u16) -> bool) -> Option<u16> {
    for _ in AUDIO_ID_BASE..=u16::MAX {
        let candidate = *next;
        *next = if candidate == u16::MAX {
            AUDIO_ID_BASE
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
/// (PLAN §10: these events are the only cache-invalidation path).
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

/// Paces admissions to `fps` with an accumulating deadline: each admitted frame advances the
/// deadline by exactly one period (clamped to `now` so no send-debt builds up while the
/// producer is slower than requested). A reset-to-now throttle would round the delivered
/// period up to a whole producer hop whenever arrivals are block-quantized, systematically
/// under-delivering (30 requested → ~20 delivered); this delivers min(producer, fps).
struct FrameThrottle {
    next_deadline: Instant,
    min_interval: Duration,
}

impl FrameThrottle {
    fn new(fps: u16, start: Instant) -> Self {
        Self {
            next_deadline: start,
            min_interval: Duration::from_secs_f64(1.0 / f64::from(fps)),
        }
    }

    fn admit(&mut self, now: Instant) -> bool {
        if now < self.next_deadline {
            return false;
        }
        self.next_deadline = self
            .next_deadline
            .checked_add(self.min_interval)
            .unwrap_or(now)
            .max(now);
        true
    }
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

    tokio::spawn(async move {
        let mut dec = vec![0f32; bins];
        let mut quant = vec![0u8; bins];
        let mut throttle = FrameThrottle::new(fps, Instant::now());

        loop {
            match rx.recv().await {
                Ok(snap) => {
                    if !throttle.admit(Instant::now()) {
                        continue;
                    }

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

                    // Awaited on purpose: a full out queue parks this task, the broadcast
                    // receiver lags, and the *oldest* snapshots are shed (drop-oldest,
                    // PLAN §5). A failed send means the connection is gone.
                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                // Oldest snapshots were shed while this task was backpressured — the
                // drop-oldest contract; resume with the newest retained.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    // The device set was removed. Tell the client the stream ended (no silent
                    // termination, CLAUDE.md); this is a one-shot control message, so await it.
                    let stopped = ServerEvent::StreamStopped {
                        stream_id: ds as u16,
                        kind: StreamKind::Spectrum,
                    };
                    let _ = out_tx.send(text_event(&stopped)).await;
                    break;
                }
            }
        }
    })
}

/// Per-subscription task: forward the channel's Opus packets as binary [`AudioFrame`]s
/// (mono, PLAN §9) with the same drop-oldest backpressure as [`spawn_spectrum`].
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
                        ch_layout: CH_LAYOUT_MONO,
                        opus: &packet.opus,
                    }
                    .encode();

                    // Awaited on purpose: backpressure lags the broadcast receiver and the
                    // oldest packets are shed (drop-oldest, PLAN §5).
                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                // Oldest packets were shed while this task was backpressured. The gap shows
                // up client-side as a jump in the 48 kHz sample-count timestamp; `seq` is a
                // plain packet counter, not a resync mechanism.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    // The channel or its device set was removed. Tell the client the stream
                    // ended (no silent termination, CLAUDE.md); one-shot control message, so
                    // await it.
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

fn text_event(ev: &ServerEvent) -> Message {
    match serde_json::to_string(ev) {
        Ok(json) => Message::Text(json.into()),
        // Serializing our own enum cannot realistically fail; emit a minimal error frame.
        Err(_) => Message::Text(
            r#"{"type":"Error","data":{"message":"event serialization failed"}}"#.into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::future::IntoFuture;

    use sdrmm_wire::{ChannelParams, ChannelSettings, NfmParams};
    use tokio::time::timeout;
    use tokio_tungstenite::tungstenite;

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    /// Hermetic engine: virtual driver only (PLAN §14: no hardware in CI, ever).
    fn test_engine() -> Arc<Engine> {
        let mut registry = sdrmm_device::DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        Engine::with_registry(registry)
    }

    type WsClient = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn connect(engine: Arc<Engine>) -> WsClient {
        let store = crate::Store::open(None).expect("in-memory store");
        let app = crate::router(engine, store, false);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(axum::serve(listener, app).into_future());
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
            },
        )
        .await;
        match next_event(&mut ws).await {
            ServerEvent::StreamStarted {
                stream_id,
                device_set,
            } => {
                assert_eq!(u32::from(stream_id), ds);
                assert_eq!(device_set, ds);
            }
            other => panic!("expected StreamStarted, got {other:?}"),
        }
        let (kind, stream_id) = next_frame_header(&mut ws).await;
        assert_eq!(kind, sdrmm_wire::FrameKind::Spectrum as u8);
        assert_eq!(u32::from(stream_id), ds);

        send(
            &mut ws,
            &ClientCommand::UnsubscribeSpectrum { device_set: ds },
        )
        .await;
        match next_event(&mut ws).await {
            ServerEvent::StreamStopped { stream_id, kind } => {
                assert_eq!(u32::from(stream_id), ds);
                assert_eq!(kind, StreamKind::Spectrum);
            }
            other => panic!("expected StreamStopped, got {other:?}"),
        }

        // A set id inside the reserved audio range must be refused, not aliased.
        send(
            &mut ws,
            &ClientCommand::SubscribeSpectrum {
                device_set: u32::from(AUDIO_ID_BASE),
                fps: 30,
                bins: 64,
            },
        )
        .await;
        assert!(matches!(
            next_event(&mut ws).await,
            ServerEvent::Error { .. }
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn audio_ids_disjoint_from_spectrum_and_duplicate_subscribe_stops_old() {
        let engine = test_engine();
        let ds = engine
            .create_device_set("virtual:siggen")
            .expect("device set");
        let ch = engine.add_channel(ds, nfm_channel(0.0)).expect("channel");
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
                    stream_id >= AUDIO_ID_BASE,
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
        assert!(second_id >= AUDIO_ID_BASE);

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
        let mut next = AUDIO_ID_BASE;
        let live = [AUDIO_ID_BASE, AUDIO_ID_BASE + 1];
        assert_eq!(
            alloc_audio_id(&mut next, |id| live.contains(&id)),
            Some(AUDIO_ID_BASE + 2)
        );

        let mut next = u16::MAX;
        assert_eq!(alloc_audio_id(&mut next, |_| false), Some(u16::MAX));
        assert_eq!(
            next, AUDIO_ID_BASE,
            "must wrap into the audio range, not to 0"
        );

        let mut next = AUDIO_ID_BASE;
        assert_eq!(alloc_audio_id(&mut next, |_| true), None);
    }

    /// A 30 Hz producer whose snapshots surface at 25 ms block boundaries arrives in a
    /// 25/25/50 ms pattern; the reset-to-now throttle this replaces delivered ~20 fps at a
    /// requested 30 because any arrival short of a full period was dropped.
    #[test]
    fn throttle_delivers_requested_fps_despite_block_quantized_arrivals() {
        let start = Instant::now();
        let pattern_ms = [25u64, 25, 50];
        for (fps, expected) in [(30u16, 300i64), (20, 200), (10, 100), (60, 300)] {
            let mut throttle = FrameThrottle::new(fps, start);
            let mut t = Duration::ZERO;
            let mut admitted: i64 = 0;
            let mut i = 0;
            while t < Duration::from_secs(10) {
                if throttle.admit(start + t) {
                    admitted += 1;
                }
                t += Duration::from_millis(pattern_ms[i % pattern_ms.len()]);
                i += 1;
            }
            assert!(
                (admitted - expected).abs() <= 3,
                "fps {fps}: delivered {admitted}, wanted ~{expected}"
            );
        }
    }

    /// Regression for drop-newest backpressure: with a stalled writer, the newest frame must
    /// survive (the old try_send path shed it) and the stale backlog must be bounded by the
    /// broadcast capacity, not the out-queue capacity.
    #[tokio::test(flavor = "multi_thread")]
    async fn audio_forwarder_sheds_oldest_and_delivers_newest() {
        let (tx, rx) = broadcast::channel::<AudioPacket>(4);
        let (out_tx, mut out_rx) = mpsc::channel::<Message>(1);
        let task = spawn_audio(AUDIO_ID_BASE, rx, out_tx);

        let last_seq = 20u32;
        for seq in 0..=last_seq {
            tx.send(AudioPacket {
                seq,
                timestamp: u64::from(seq) * 960,
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

        // The retained newest events still follow.
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
