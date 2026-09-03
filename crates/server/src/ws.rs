use std::{
    collections::HashMap,
    sync::{Arc, atomic},
    time::Instant,
};

use axum::{
    extract::{
        State,
        ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use sdrmm_dsp::{adaptive_db_window, decimate_max, quantize_db};
use sdrmm_engine::{
    AudioPacket, Engine, IqBlock, SpectrumSnapshot, SymbolBlock, VideoPacket,
    coherent::SurfaceUpdate,
};
use sdrmm_wire::{
    AudioFrame, ClientCommand, IqFrame, PositionFix, RangeDopplerFrame, ServerEvent, SpectrumFrame,
    StateScope, StreamKind, SymbolFrame, VideoData, VideoFrame,
};
use tokio::sync::{broadcast, mpsc};

use crate::AppState;

const OUT_CHANNEL_CAP: usize = 256;
const MIN_BINS: usize = 16;
const MAX_BINS: usize = 4096;
const MAX_FPS: u16 = 60;
const POSITION_PUBLISH_RATE: f64 = 20.0;
const POSITION_PUBLISH_BURST: f64 = 16.0;
const MEDIA_ID_BASE: u16 = 0x8000;
const SPECTRUM_ID_BASE: u16 = 0;

pub(crate) async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

pub(crate) fn start_decoded_encoder(state: &AppState) {
    let mut decoded_rx = state.engine.subscribe_decoded();
    let out = state.decoded_text.clone();
    let tracks = state.tracks.clone();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("no runtime in context: decoder frames will not reach clients");
        return;
    };
    let _guard = handle.enter();
    tokio::spawn(async move {
        loop {
            match decoded_rx.recv().await {
                Ok(record) => {
                    tracks.observe(&record);
                    let _ = out.send(encode_event(&ServerEvent::Decoded(Box::new(record))));
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    let _ = out.send(encode_event(&ServerEvent::DecodedLost { count: missed }));
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

struct RateBudget {
    tokens: f64,
    capacity: f64,
    per_second: f64,
    refilled: Instant,
}

impl RateBudget {
    fn new(capacity: f64, per_second: f64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            per_second,
            refilled: Instant::now(),
        }
    }

    fn take(&mut self) -> bool {
        let now = Instant::now();
        let earned = now.duration_since(self.refilled).as_secs_f64() * self.per_second;
        self.tokens = (self.tokens + earned).min(self.capacity);
        self.refilled = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

struct Session {
    engine: Arc<Engine>,
    state: AppState,
    out: mpsc::Sender<Message>,
    spectra: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    audio: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    video: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    iq: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    symbols: HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    surfaces: HashMap<String, (u16, tokio::task::JoinHandle<()>)>,
    next_spectrum_id: u16,
    next_media_id: u16,
    position_budget: RateBudget,
}

impl Session {
    fn new(engine: Arc<Engine>, state: AppState, out: mpsc::Sender<Message>) -> Self {
        Self {
            engine,
            state,
            out,
            spectra: HashMap::new(),
            audio: HashMap::new(),
            video: HashMap::new(),
            iq: HashMap::new(),
            symbols: HashMap::new(),
            surfaces: HashMap::new(),
            next_spectrum_id: SPECTRUM_ID_BASE,
            next_media_id: MEDIA_ID_BASE,
            position_budget: RateBudget::new(POSITION_PUBLISH_BURST, POSITION_PUBLISH_RATE),
        }
    }

    async fn dispatch(&mut self, text: &str) {
        let Ok(command) = serde_json::from_str::<ClientCommand>(text) else {
            let err = ServerEvent::Error {
                message: "invalid command".to_string(),
            };
            let _ = self.out.send(text_event(&err)).await;
            return;
        };
        match command {
            ClientCommand::SubscribeSpectrum {
                device_set,
                fps,
                bins,
                stream,
            } => self.subscribe_spectrum(device_set, fps, bins, stream).await,
            ClientCommand::UnsubscribeSpectrum { device_set, stream } => {
                self.unsubscribe_spectrum(device_set, stream).await;
            }
            ClientCommand::SubscribeAudio {
                device_set,
                channel,
            } => self.subscribe_audio(device_set, channel).await,
            ClientCommand::UnsubscribeAudio {
                device_set,
                channel,
            } => self.unsubscribe_audio(device_set, channel).await,
            ClientCommand::SubscribeVideo {
                device_set,
                channel,
            } => self.subscribe_video(device_set, channel).await,
            ClientCommand::UnsubscribeVideo {
                device_set,
                channel,
            } => self.unsubscribe_video(device_set, channel).await,
            ClientCommand::SubscribeIq {
                device_set,
                channel,
            } => self.subscribe_iq(device_set, channel).await,
            ClientCommand::UnsubscribeIq {
                device_set,
                channel,
            } => self.unsubscribe_iq(device_set, channel).await,
            ClientCommand::SubscribeSymbols {
                device_set,
                channel,
            } => self.subscribe_symbols(device_set, channel).await,
            ClientCommand::UnsubscribeSymbols {
                device_set,
                channel,
            } => self.unsubscribe_symbols(device_set, channel).await,
            ClientCommand::PublishPosition { node, fix, error } => {
                self.publish_position(node, fix, error).await;
            }
            ClientCommand::SubscribeSurface { node } => self.subscribe_surface(node).await,
            ClientCommand::UnsubscribeSurface { node } => self.unsubscribe_surface(&node).await,
        }
    }

    async fn subscribe_surface(&mut self, node: String) {
        let Some(binding) = self.state.coherent.binding(&node) else {
            let err = ServerEvent::Error {
                message: format!("no coherent node {node} is running"),
            };
            let _ = self.out.send(text_event(&err)).await;
            return;
        };
        let Some(rx) = self.engine.subscribe_surfaces(binding.device_set) else {
            let err = ServerEvent::Error {
                message: format!("{node} produces no surface"),
            };
            let _ = self.out.send(text_event(&err)).await;
            return;
        };
        self.unsubscribe_surface(&node).await;
        let live = |id: u16| {
            media_id_live(&self.audio, &self.video, &self.iq, &self.symbols, id)
                || self.surfaces.values().any(|(sid, _)| *sid == id)
        };
        let Some(stream_id) =
            alloc_stream_id(&mut self.next_media_id, MEDIA_ID_BASE..=u16::MAX, live)
        else {
            let err = ServerEvent::Error {
                message: "no free media stream ids on this connection".to_string(),
            };
            let _ = self.out.send(text_event(&err)).await;
            return;
        };
        let started = ServerEvent::SurfaceStreamStarted {
            stream_id,
            device_set: binding.device_set,
            node: node.clone(),
        };
        let _ = self.out.send(text_event(&started)).await;
        let task = spawn_surface(stream_id, binding.id, rx, self.out.clone());
        self.surfaces.insert(node, (stream_id, task));
    }

    async fn unsubscribe_surface(&mut self, node: &str) {
        if let Some((stream_id, task)) = self.surfaces.remove(node) {
            task.abort();
            let stopped = ServerEvent::StreamStopped {
                stream_id,
                kind: StreamKind::RangeDoppler,
            };
            let _ = self.out.send(text_event(&stopped)).await;
        }
    }

    fn abort_streams(self) {
        for (_, (_, task)) in self.spectra {
            task.abort();
        }
        for (_, (_, task)) in self.audio {
            task.abort();
        }
        for (_, (_, task)) in self.iq {
            task.abort();
        }
        for (_, (_, task)) in self.video {
            task.abort();
        }
        for (_, (_, task)) in self.symbols {
            task.abort();
        }
        for (_, (_, task)) in self.surfaces {
            task.abort();
        }
    }

    async fn subscribe_spectrum(&mut self, device_set: u32, fps: u16, bins: u16, stream: u32) {
        let subscribe = {
            let engine = self.engine.clone();
            tokio::task::spawn_blocking(move || engine.subscribe_spectrum(device_set, stream)).await
        };
        match flatten_join(subscribe) {
            Ok(rx) => {
                if let Some((old_id, old)) = self.spectra.remove(&(device_set, stream)) {
                    old.abort();
                    let stopped = ServerEvent::StreamStopped {
                        stream_id: old_id,
                        kind: StreamKind::Spectrum,
                    };
                    let _ = self.out.send(text_event(&stopped)).await;
                }
                let live = |id: u16| self.spectra.values().any(|(sid, _)| *sid == id);
                match alloc_stream_id(
                    &mut self.next_spectrum_id,
                    SPECTRUM_ID_BASE..=MEDIA_ID_BASE - 1,
                    live,
                ) {
                    Some(stream_id) => {
                        let started = ServerEvent::StreamStarted {
                            stream_id,
                            device_set,
                            stream,
                        };
                        let _ = self.out.send(text_event(&started)).await;
                        let task = spawn_spectrum(
                            SpectrumLane {
                                stream_id,
                                device_set,
                                stream,
                            },
                            fps,
                            bins,
                            rx,
                            self.out.clone(),
                            self.engine.clone(),
                        );
                        self.spectra.insert((device_set, stream), (stream_id, task));
                    }
                    None => {
                        let err = ServerEvent::Error {
                            message: "no free spectrum stream ids on this \
                                      connection"
                                .to_string(),
                        };
                        let _ = self.out.send(text_event(&err)).await;
                    }
                }
            }
            Err(message) => {
                let _ = self
                    .out
                    .send(text_event(&ServerEvent::Error { message }))
                    .await;
            }
        }
    }

    async fn unsubscribe_spectrum(&mut self, device_set: u32, stream: u32) {
        if let Some((stream_id, task)) = self.spectra.remove(&(device_set, stream)) {
            task.abort();
            let stopped = ServerEvent::StreamStopped {
                stream_id,
                kind: StreamKind::Spectrum,
            };
            let _ = self.out.send(text_event(&stopped)).await;
        }
    }

    async fn subscribe_audio(&mut self, device_set: u32, channel: u32) {
        let subscribe = {
            let engine = self.engine.clone();
            tokio::task::spawn_blocking(move || engine.subscribe_audio(device_set, channel)).await
        };
        match flatten_join(subscribe) {
            Ok(rx) => {
                if let Some((old_id, old)) = self.audio.remove(&(device_set, channel)) {
                    old.abort();
                    let stopped = ServerEvent::StreamStopped {
                        stream_id: old_id,
                        kind: StreamKind::Audio,
                    };
                    let _ = self.out.send(text_event(&stopped)).await;
                }
                let live =
                    |id: u16| media_id_live(&self.audio, &self.video, &self.iq, &self.symbols, id);
                match alloc_stream_id(&mut self.next_media_id, MEDIA_ID_BASE..=u16::MAX, live) {
                    Some(stream_id) => {
                        let started = ServerEvent::AudioStreamStarted {
                            stream_id,
                            device_set,
                            channel,
                        };
                        let _ = self.out.send(text_event(&started)).await;
                        let task = spawn_audio(stream_id, rx, self.out.clone());
                        self.audio.insert((device_set, channel), (stream_id, task));
                    }
                    None => {
                        let err = ServerEvent::Error {
                            message: "no free media stream ids on this connection".to_string(),
                        };
                        let _ = self.out.send(text_event(&err)).await;
                    }
                }
            }
            Err(message) => {
                let _ = self
                    .out
                    .send(text_event(&ServerEvent::Error { message }))
                    .await;
            }
        }
    }

    async fn unsubscribe_audio(&mut self, device_set: u32, channel: u32) {
        if let Some((stream_id, task)) = self.audio.remove(&(device_set, channel)) {
            task.abort();
            let stopped = ServerEvent::StreamStopped {
                stream_id,
                kind: StreamKind::Audio,
            };
            let _ = self.out.send(text_event(&stopped)).await;
        }
    }

    async fn subscribe_video(&mut self, device_set: u32, channel: u32) {
        let subscribe = {
            let engine = self.engine.clone();
            tokio::task::spawn_blocking(move || engine.subscribe_video(device_set, channel)).await
        };
        match flatten_join(subscribe) {
            Ok(rx) => {
                if let Some((old_id, old)) = self.video.remove(&(device_set, channel)) {
                    old.abort();
                    let stopped = ServerEvent::StreamStopped {
                        stream_id: old_id,
                        kind: StreamKind::Video,
                    };
                    let _ = self.out.send(text_event(&stopped)).await;
                }
                let live =
                    |id: u16| media_id_live(&self.audio, &self.video, &self.iq, &self.symbols, id);
                match alloc_stream_id(&mut self.next_media_id, MEDIA_ID_BASE..=u16::MAX, live) {
                    Some(stream_id) => {
                        let started = ServerEvent::VideoStreamStarted {
                            stream_id,
                            device_set,
                            channel,
                        };
                        let _ = self.out.send(text_event(&started)).await;
                        let task = spawn_video(stream_id, rx, self.out.clone());
                        self.video.insert((device_set, channel), (stream_id, task));
                    }
                    None => {
                        let err = ServerEvent::Error {
                            message: "no free media stream ids on this connection".to_string(),
                        };
                        let _ = self.out.send(text_event(&err)).await;
                    }
                }
            }
            Err(message) => {
                let _ = self
                    .out
                    .send(text_event(&ServerEvent::Error { message }))
                    .await;
            }
        }
    }

    async fn unsubscribe_video(&mut self, device_set: u32, channel: u32) {
        if let Some((stream_id, task)) = self.video.remove(&(device_set, channel)) {
            task.abort();
            let stopped = ServerEvent::StreamStopped {
                stream_id,
                kind: StreamKind::Video,
            };
            let _ = self.out.send(text_event(&stopped)).await;
        }
    }

    async fn subscribe_iq(&mut self, device_set: u32, channel: u32) {
        let subscribe = {
            let engine = self.engine.clone();
            tokio::task::spawn_blocking(move || engine.subscribe_iq(device_set, channel)).await
        };
        match flatten_join(subscribe) {
            Ok(rx) => {
                if let Some((old_id, old)) = self.iq.remove(&(device_set, channel)) {
                    old.abort();
                    let stopped = ServerEvent::StreamStopped {
                        stream_id: old_id,
                        kind: StreamKind::Iq,
                    };
                    let _ = self.out.send(text_event(&stopped)).await;
                }
                let live =
                    |id: u16| media_id_live(&self.audio, &self.video, &self.iq, &self.symbols, id);
                match alloc_stream_id(&mut self.next_media_id, MEDIA_ID_BASE..=u16::MAX, live) {
                    Some(stream_id) => {
                        let started = ServerEvent::IqStreamStarted {
                            stream_id,
                            device_set,
                            channel,
                        };
                        let _ = self.out.send(text_event(&started)).await;
                        let task = spawn_iq(stream_id, rx, self.out.clone());
                        self.iq.insert((device_set, channel), (stream_id, task));
                    }
                    None => {
                        let err = ServerEvent::Error {
                            message: "no free media stream ids on this connection".to_string(),
                        };
                        let _ = self.out.send(text_event(&err)).await;
                    }
                }
            }
            Err(message) => {
                let _ = self
                    .out
                    .send(text_event(&ServerEvent::Error { message }))
                    .await;
            }
        }
    }

    async fn unsubscribe_iq(&mut self, device_set: u32, channel: u32) {
        if let Some((stream_id, task)) = self.iq.remove(&(device_set, channel)) {
            task.abort();
            let stopped = ServerEvent::StreamStopped {
                stream_id,
                kind: StreamKind::Iq,
            };
            let _ = self.out.send(text_event(&stopped)).await;
        }
    }

    async fn subscribe_symbols(&mut self, device_set: u32, channel: u32) {
        let subscribe = {
            let engine = self.engine.clone();
            tokio::task::spawn_blocking(move || engine.subscribe_symbols(device_set, channel)).await
        };
        match flatten_join(subscribe) {
            Ok(rx) => {
                if let Some((old_id, old)) = self.symbols.remove(&(device_set, channel)) {
                    old.abort();
                    let stopped = ServerEvent::StreamStopped {
                        stream_id: old_id,
                        kind: StreamKind::Symbols,
                    };
                    let _ = self.out.send(text_event(&stopped)).await;
                }
                let live =
                    |id: u16| media_id_live(&self.audio, &self.video, &self.iq, &self.symbols, id);
                match alloc_stream_id(&mut self.next_media_id, MEDIA_ID_BASE..=u16::MAX, live) {
                    Some(stream_id) => {
                        let started = ServerEvent::SymbolStreamStarted {
                            stream_id,
                            device_set,
                            channel,
                        };
                        let _ = self.out.send(text_event(&started)).await;
                        let task = spawn_symbols(stream_id, rx, self.out.clone());
                        self.symbols
                            .insert((device_set, channel), (stream_id, task));
                    }
                    None => {
                        let err = ServerEvent::Error {
                            message: "no free media stream ids on this connection".to_string(),
                        };
                        let _ = self.out.send(text_event(&err)).await;
                    }
                }
            }
            Err(message) => {
                let _ = self
                    .out
                    .send(text_event(&ServerEvent::Error { message }))
                    .await;
            }
        }
    }

    async fn unsubscribe_symbols(&mut self, device_set: u32, channel: u32) {
        if let Some((stream_id, task)) = self.symbols.remove(&(device_set, channel)) {
            task.abort();
            let stopped = ServerEvent::StreamStopped {
                stream_id,
                kind: StreamKind::Symbols,
            };
            let _ = self.out.send(text_event(&stopped)).await;
        }
    }

    async fn publish_position(
        &mut self,
        node: String,
        fix: Option<PositionFix>,
        error: Option<String>,
    ) {
        let too_fast = !self.position_budget.take();
        if node.is_empty() || node.len() > sdrmm_wire::patch::MAX_NODE_ID_LEN {
            let _ = self
                .out
                .send(text_event(&ServerEvent::Error {
                    message: "invalid position node id".to_owned(),
                }))
                .await;
        } else if too_fast {
            let _ = self
                .out
                .send(text_event(&ServerEvent::Error {
                    message: "position updates are limited to 20 Hz per connection".to_owned(),
                }))
                .await;
        } else {
            let app = self.state.clone();
            let publish_node = node.clone();
            let publish = tokio::task::spawn_blocking(move || {
                app.gps.publish_device(&app, &publish_node, fix, error)
            })
            .await;
            match publish {
                Ok(Ok(())) => {}
                Ok(Err(message)) => {
                    let _ = self
                        .out
                        .send(text_event(&ServerEvent::Error { message }))
                        .await;
                }
                Err(error) => {
                    tracing::error!(%error, "device GPS publish task failed");
                    let _ = self
                        .out
                        .send(text_event(&ServerEvent::Error {
                            message: "could not publish device position".to_owned(),
                        }))
                        .await;
                }
            }
        }
    }
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
    let decoded_rx = state.decoded_text.subscribe();
    let position_rx = state.gps.subscribe();
    let hello = ServerEvent::Hello {
        revision: engine.snapshot().revision,
    };
    let _ = out_tx.send(text_event(&hello)).await;
    for position in state.gps.snapshot() {
        let _ = out_tx.send(text_event(&position)).await;
    }

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

    let mut session = Session::new(engine.clone(), state.clone(), out_tx.clone());

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => session.dispatch(&text).await,
            Message::Close(_) => break,
            _ => {}
        }
    }

    session.abort_streams();

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

fn flatten_join<T>(
    joined: Result<Result<T, sdrmm_engine::EngineError>, tokio::task::JoinError>,
) -> Result<T, String> {
    match joined {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("engine task failed: {e}")),
    }
}

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

struct FrameThrottle {
    fps: f64,
    next: u64,
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

    fn admit(&mut self, timestamp: u64, sample_rate: f32) -> bool {
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
    stream_id: u16,
    device_set: u32,
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
        let mut window = Vec::with_capacity(bins);
        let mut throttle = FrameThrottle::new(fps);

        loop {
            match rx.recv().await {
                Ok(snap) => {
                    if !throttle.admit(snap.timestamp, snap.span_hz) {
                        continue;
                    }

                    decimate_max(&snap.db, &mut dec);
                    let (db_min, db_max) = adaptive_db_window(&dec, &mut window);
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
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
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

                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
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

fn media_id_live(
    audio: &HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    video: &HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    iq: &HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    symbols: &HashMap<(u32, u32), (u16, tokio::task::JoinHandle<()>)>,
    id: u16,
) -> bool {
    audio
        .values()
        .chain(symbols.values())
        .chain(video.values())
        .chain(iq.values())
        .any(|(sid, _)| *sid == id)
}

fn spawn_symbols(
    stream_id: u16,
    mut rx: broadcast::Receiver<SymbolBlock>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(block) => {
                    let frame = SymbolFrame {
                        stream_id,
                        seq: block.seq,
                        timestamp: block.timestamp,
                        plane: block.plane,
                        symbol_rate: block.symbol_rate,
                        evm: block.evm,
                        mer_db: block.mer_db,
                        margin: block.margin,
                        freq_error_hz: block.freq_error_hz,
                        reference: &block.reference,
                        symbols: &block.symbols,
                    }
                    .encode();

                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    let stopped = ServerEvent::StreamStopped {
                        stream_id,
                        kind: StreamKind::Symbols,
                    };
                    let _ = out_tx.send(text_event(&stopped)).await;
                    break;
                }
            }
        }
    })
}

fn spawn_iq(
    stream_id: u16,
    mut rx: broadcast::Receiver<IqBlock>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interleaved: Vec<f32> = Vec::new();
        loop {
            match rx.recv().await {
                Ok(block) => {
                    interleaved.clear();
                    interleaved.reserve(block.samples.len() * 2);
                    for sample in block.samples.iter() {
                        interleaved.push(sample.re);
                        interleaved.push(sample.im);
                    }
                    let frame = IqFrame {
                        stream_id,
                        seq: block.seq,
                        timestamp: block.timestamp,
                        sample_rate: block.sample_rate,
                        center_hz: block.center_hz,
                        samples: &interleaved,
                    }
                    .encode();

                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    let stopped = ServerEvent::StreamStopped {
                        stream_id,
                        kind: StreamKind::Iq,
                    };
                    let _ = out_tx.send(text_event(&stopped)).await;
                    break;
                }
            }
        }
    })
}

/// One coherent node's range-Doppler surface, quantised upstream and sent as it comes.
fn spawn_surface(
    stream_id: u16,
    node: u32,
    mut rx: broadcast::Receiver<SurfaceUpdate>,
    out_tx: mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(update) if update.node != node => continue,
                Ok(update) => {
                    let surface = &update.surface;
                    let frame = RangeDopplerFrame {
                        stream_id,
                        seq: update.seq,
                        timestamp: u64::from(update.seq),
                        ranges: surface.ranges.min(usize::from(u16::MAX)) as u16,
                        dopplers: surface.dopplers.min(usize::from(u16::MAX)) as u16,
                        range_step_us: surface.range_step_s * 1e6,
                        doppler_step_hz: surface.doppler_step_hz,
                        db_min: surface.db_min,
                        db_max: surface.db_max,
                        cells: &surface.cells,
                    }
                    .encode();
                    if out_tx.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    let stopped = ServerEvent::StreamStopped {
                        stream_id,
                        kind: StreamKind::RangeDoppler,
                    };
                    let _ = out_tx.send(text_event(&stopped)).await;
                    break;
                }
            }
        }
    })
}

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
                        data: if packet.picture.rgb.is_empty() {
                            VideoData::Gray(&packet.picture.luma)
                        } else {
                            VideoData::Rgb(&packet.picture.rgb)
                        },
                    }
                    .encode();

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
        Err(_) => r#"{"type":"Error","data":{"message":"event serialization failed"}}"#.into(),
    }
}

#[cfg(test)]
mod tests;
