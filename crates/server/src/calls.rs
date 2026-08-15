use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use axum::body::Bytes;
use sdrmm_dsp::{decim::RealDecimator, fir::design_lowpass};
use sdrmm_engine::{Engine, PcmBlock, PcmPayload};
use sdrmm_wire::{
    DecodedRecord, DecoderEvent, DvFrame, DvFrameKind, EventAudio, ServerEvent, StateScope,
    VoiceCall,
};
use tokio::{
    sync::{broadcast::error::RecvError, mpsc, watch},
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

use crate::trunking::Retentions;

const CALL_TIMEOUT: Duration = Duration::from_millis(900);

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

const STORED_RATE_HZ: u32 = 8_000;
const DECIMATION: usize = 48_000 / STORED_RATE_HZ as usize;
const ANTIALIAS_TAPS: usize = 96;

const MAX_CALL_SECONDS: usize = 600;
const MAX_CALL_SAMPLES: usize = STORED_RATE_HZ as usize * MAX_CALL_SECONDS;
const MAX_STORED_CALLS: usize = 10_000;

const MAX_STORED_AUDIO_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Calls {
    inner: Mutex<StoredCalls>,
}

#[derive(Default)]
struct StoredCalls {
    next_id: u64,
    calls: VecDeque<StoredCall>,
    audio_bytes: usize,
}

struct StoredCall {
    call: VoiceCall,
    audio: Option<Bytes>,
    expires: Instant,
}

pub(crate) struct NewCall {
    pub node: String,
    pub source_node: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub device_set: u32,
    pub channel: u32,
    pub freq_hz: f64,
    pub frame: DvFrame,
    pub audio_error: Option<String>,
}

impl Calls {
    pub(crate) fn list(&self) -> Vec<VoiceCall> {
        let mut inner = self.lock();
        prune(&mut inner);
        inner.calls.iter().rev().map(|it| it.call.clone()).collect()
    }

    pub(crate) fn audio(&self, id: u64) -> Option<Bytes> {
        let mut inner = self.lock();
        prune(&mut inner);
        inner
            .calls
            .iter()
            .find(|item| item.call.id == id)
            .and_then(|item| item.audio.clone())
    }

    fn expire(&self) -> bool {
        let mut inner = self.lock();
        let before = inner.calls.len();
        prune(&mut inner);
        inner.calls.len() != before
    }

    fn push(&self, new: NewCall, audio: Option<Bytes>, retention: Duration) -> (VoiceCall, bool) {
        let mut inner = self.lock();
        prune(&mut inner);
        inner.next_id += 1;
        let encrypted = new.frame.encrypted == Some(true);
        let call = VoiceCall {
            id: inner.next_id,
            node: new.node,
            source_node: new.source_node,
            started_at: new.started_at,
            ended_at: new.ended_at,
            duration_ms: new.duration_ms,
            device_set: new.device_set,
            channel: new.channel,
            freq_hz: new.freq_hz,
            mode: new.frame.mode,
            slot: new.frame.slot,
            color_code: new.frame.color_code,
            source: new.frame.source,
            destination: new.frame.destination,
            group_call: new.frame.group_call,
            encrypted,
            emergency: new.frame.emergency == Some(true),
            audio: audio.as_ref().map(|_| EventAudio {
                url: crate::rest::call_audio_path(inner.next_id),
                media_type: "audio/wav".to_owned(),
            }),
            audio_error: new.audio_error,
        };
        inner.audio_bytes += audio.as_ref().map_or(0, Bytes::len);
        inner.calls.push_back(StoredCall {
            call: call.clone(),
            audio,
            expires: Instant::now() + retention,
        });
        while inner.calls.len() > MAX_STORED_CALLS {
            let dropped = inner.calls.pop_front();
            inner.audio_bytes -= dropped
                .and_then(|item| item.audio)
                .as_ref()
                .map_or(0, Bytes::len);
        }
        let evicted = evict_audio(&mut inner);
        (call, evicted)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoredCalls> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn prune(inner: &mut StoredCalls) {
    let now = Instant::now();
    let mut freed = 0;
    inner.calls.retain(|item| {
        let keep = item.expires > now;
        if !keep {
            freed += item.audio.as_ref().map_or(0, Bytes::len);
        }
        keep
    });
    inner.audio_bytes -= freed;
}

fn evict_audio(inner: &mut StoredCalls) -> bool {
    let mut evicted = false;
    for item in &mut inner.calls {
        if inner.audio_bytes <= MAX_STORED_AUDIO_BYTES {
            break;
        }
        let Some(audio) = item.audio.take() else {
            continue;
        };
        inner.audio_bytes -= audio.len();
        item.call.audio = None;
        item.call
            .audio_error
            .get_or_insert_with(|| "audio evicted by the temporary buffer limit".to_owned());
        evicted = true;
    }
    evicted
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CallKey {
    node: String,
    device_set: u32,
    channel: u32,
    slot: Option<u8>,
}

#[derive(Clone)]
struct Binding {
    node: String,
    device_set: u32,
    channel: u32,
    retention: Duration,
}

struct ActiveCall {
    node: String,
    started_at: String,
    started: Instant,
    last_activity: Instant,
    freq_hz: f64,
    frame: DvFrame,
    retention: Duration,
    audio: CallAudio,
    audio_error: Option<String>,
}

struct CallAudio {
    decimator: RealDecimator,
    scratch: Vec<f32>,
    samples: Vec<i16>,
}

impl CallAudio {
    fn new(taps: &[f32]) -> Self {
        Self {
            decimator: RealDecimator::new(taps, DECIMATION),
            scratch: Vec::new(),
            samples: Vec::new(),
        }
    }

    fn push(&mut self, input: &[f32]) {
        self.scratch.clear();
        self.decimator.process(input, &mut self.scratch);
        let room = MAX_CALL_SAMPLES - self.samples.len();
        self.samples.extend(
            self.scratch
                .iter()
                .take(room)
                .map(|sample| (sample.clamp(-1.0, 1.0) * 32_767.0) as i16),
        );
    }

    fn full(&self) -> bool {
        self.samples.len() >= MAX_CALL_SAMPLES
    }
}

enum Input {
    Pcm(u32, u32, Box<PcmBlock>),
    AudioError(u32, u32, String),
}

pub(crate) async fn run(
    engine: Weak<Engine>,
    calls: Arc<Calls>,
    mut retentions: watch::Receiver<Retentions>,
) {
    let Some(strong) = engine.upgrade() else {
        return;
    };
    let mut decoded = strong.subscribe_decoded();
    let mut events = strong.subscribe_events();
    drop(strong);
    let taps = design_lowpass(ANTIALIAS_TAPS, 0.5 / DECIMATION as f64);
    let (input_tx, mut input_rx) = mpsc::channel(1024);
    let mut audio_tasks: HashMap<(u32, u32), JoinHandle<()>> = HashMap::new();
    let mut bindings = HashMap::new();
    let mut active: HashMap<CallKey, ActiveCall> = HashMap::new();
    let mut reconcile_tick = ticker(RECONCILE_INTERVAL);
    let mut timeout_tick = ticker(Duration::from_millis(200));
    let mut rebind = true;
    loop {
        if rebind {
            rebind = false;
            let Some(strong) = engine.upgrade() else {
                break;
            };
            bindings = resolve_bindings(&strong, &retentions.borrow());
            reconcile_audio(&strong, &input_tx, &bindings, &mut audio_tasks);
            finish_unbound(&bindings, &mut active, &calls, &strong);
        }
        tokio::select! {
            received = decoded.recv() => match received {
                Ok(record) => handle_record(&record, &bindings, &mut active, &calls, &engine, &taps),
                Err(RecvError::Lagged(count)) => mark_all_audio_errors(
                    &mut active,
                    format!("decoder event stream lost {count} record(s)"),
                ),
                Err(RecvError::Closed) => break,
            },
            received = events.recv() => match received {
                Ok(ServerEvent::StateChanged {
                    scope: StateScope::All | StateScope::DeviceSet(_) | StateScope::Workspaces,
                }) => rebind = true,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            changed = retentions.changed() => match changed {
                Ok(()) => rebind = true,
                Err(_) => break,
            },
            input = input_rx.recv() => match input {
                Some(Input::Pcm(ds, channel, block)) => append_pcm(ds, channel, &block, &mut active),
                Some(Input::AudioError(ds, channel, error)) => {
                    mark_audio_error(ds, channel, error, &mut active);
                }
                None => break,
            },
            _ = reconcile_tick.tick() => {
                if calls.expire() && let Some(strong) = engine.upgrade() {
                    strong.emit_scope(StateScope::Calls);
                }
            }
            _ = timeout_tick.tick() => finish_timed_out(&mut active, &calls, &engine),
        }
    }
    for (_, task) in audio_tasks {
        task.abort();
    }
    finish_all(&mut active, &calls, &engine);
}

fn ticker(period: Duration) -> tokio::time::Interval {
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

fn resolve_bindings(engine: &Engine, retentions: &Retentions) -> HashMap<(u32, u32), Vec<Binding>> {
    let mut resolved: HashMap<(u32, u32), Vec<Binding>> = HashMap::new();
    for system in engine.trunk_systems() {
        let Some(&retention) = retentions.get(&system.node) else {
            continue;
        };
        for follower in system.followers {
            resolved
                .entry((follower.device_set, follower.channel))
                .or_default()
                .push(Binding {
                    node: system.node.clone(),
                    device_set: follower.device_set,
                    channel: follower.channel,
                    retention,
                });
        }
    }
    resolved
}

fn reconcile_audio(
    engine: &Arc<Engine>,
    input_tx: &mpsc::Sender<Input>,
    bindings: &HashMap<(u32, u32), Vec<Binding>>,
    tasks: &mut HashMap<(u32, u32), JoinHandle<()>>,
) {
    let wanted: HashSet<(u32, u32)> = bindings.keys().copied().collect();
    tasks.retain(|source, task| {
        let keep = wanted.contains(source);
        if !keep {
            task.abort();
        }
        keep
    });
    for source in wanted {
        if tasks.contains_key(&source) {
            continue;
        }
        let Ok(receiver) = engine.subscribe_pcm(source.0, source.1) else {
            continue;
        };
        tasks.insert(source, spawn_audio(source, receiver, input_tx.clone()));
    }
}

fn spawn_audio(
    source: (u32, u32),
    mut receiver: tokio::sync::broadcast::Receiver<PcmBlock>,
    input: mpsc::Sender<Input>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let message = match receiver.recv().await {
                Ok(block) => Input::Pcm(source.0, source.1, Box::new(block)),
                Err(RecvError::Lagged(count)) => Input::AudioError(
                    source.0,
                    source.1,
                    format!("audio stream lost {count} block(s)"),
                ),
                Err(RecvError::Closed) => return,
            };
            if input.send(message).await.is_err() {
                return;
            }
        }
    })
}

fn handle_record(
    record: &DecodedRecord,
    bindings: &HashMap<(u32, u32), Vec<Binding>>,
    active: &mut HashMap<CallKey, ActiveCall>,
    calls: &Calls,
    engine: &Weak<Engine>,
    taps: &[f32],
) {
    let DecoderEvent::Dv(frame) = &record.event else {
        return;
    };
    let Some(bound) = bindings.get(&(record.device_set, record.channel)) else {
        return;
    };
    for binding in bound {
        let key = CallKey {
            node: binding.node.clone(),
            device_set: binding.device_set,
            channel: binding.channel,
            slot: frame.slot,
        };
        match frame.kind {
            DvFrameKind::Header | DvFrameKind::Voice
                if frame.source.is_some() || frame.destination.is_some() =>
            {
                update_call(key, binding, record, frame, active, calls, engine, taps);
            }
            DvFrameKind::Terminator => finish_key(&key, active, calls, engine),
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update_call(
    key: CallKey,
    binding: &Binding,
    record: &DecodedRecord,
    frame: &DvFrame,
    active: &mut HashMap<CallKey, ActiveCall>,
    calls: &Calls,
    engine: &Weak<Engine>,
    taps: &[f32],
) {
    if active
        .get(&key)
        .is_some_and(|call| !same_call(&call.frame, frame))
    {
        finish_key(&key, active, calls, engine);
    }
    let call = active.entry(key).or_insert_with(|| ActiveCall {
        node: binding.node.clone(),
        started_at: record.at.clone(),
        started: Instant::now(),
        last_activity: Instant::now(),
        freq_hz: record.freq_hz,
        frame: frame.clone(),
        retention: binding.retention,
        audio: CallAudio::new(taps),
        audio_error: None,
    });
    merge_frame(&mut call.frame, frame);
    call.last_activity = Instant::now();
    call.retention = binding.retention;
    if call.frame.encrypted == Some(true) {
        call.audio.samples.clear();
    }
}

fn same_call(current: &DvFrame, incoming: &DvFrame) -> bool {
    fn agrees<T: PartialEq>(current: Option<T>, incoming: Option<T>) -> bool {
        match (current, incoming) {
            (Some(current), Some(incoming)) => current == incoming,
            _ => true,
        }
    }
    agrees(current.slot, incoming.slot)
        && agrees(current.source, incoming.source)
        && agrees(current.destination, incoming.destination)
}

fn merge_frame(current: &mut DvFrame, incoming: &DvFrame) {
    current.slot = incoming.slot.or(current.slot);
    current.color_code = incoming.color_code.or(current.color_code);
    current.source = incoming.source.or(current.source);
    current.destination = incoming.destination.or(current.destination);
    current.group_call = incoming.group_call.or(current.group_call);
    current.encrypted = incoming.encrypted.or(current.encrypted);
    current.emergency = incoming.emergency.or(current.emergency);
}

fn append_pcm(
    device_set: u32,
    channel: u32,
    block: &PcmBlock,
    active: &mut HashMap<CallKey, ActiveCall>,
) {
    for (key, call) in active.iter_mut() {
        if key.device_set != device_set || key.channel != channel {
            continue;
        }
        call.last_activity = Instant::now();
        if call.frame.encrypted == Some(true) {
            continue;
        }
        if call.audio.full() {
            call.audio_error
                .get_or_insert_with(|| format!("audio exceeded the {MAX_CALL_SECONDS} s limit"));
            continue;
        }
        match &block.payload {
            PcmPayload::Samples(samples) => {
                let channels = usize::from(block.channels.max(1));
                if channels == 1 {
                    call.audio.push(samples);
                } else {
                    let mono: Vec<f32> = samples.iter().step_by(channels).copied().collect();
                    call.audio.push(&mono);
                }
            }
            PcmPayload::Silence(frames) => {
                let silence = vec![0.0; *frames];
                call.audio.push(&silence);
            }
        }
    }
}

fn mark_audio_error(
    device_set: u32,
    channel: u32,
    error: String,
    active: &mut HashMap<CallKey, ActiveCall>,
) {
    for (key, call) in active.iter_mut() {
        if key.device_set == device_set && key.channel == channel {
            call.audio_error.get_or_insert_with(|| error.clone());
        }
    }
}

fn mark_all_audio_errors(active: &mut HashMap<CallKey, ActiveCall>, error: String) {
    for call in active.values_mut() {
        call.audio_error.get_or_insert_with(|| error.clone());
    }
}

fn finish_timed_out(
    active: &mut HashMap<CallKey, ActiveCall>,
    calls: &Calls,
    engine: &Weak<Engine>,
) {
    let expired: Vec<CallKey> = active
        .iter()
        .filter(|(_, call)| call.last_activity.elapsed() >= CALL_TIMEOUT)
        .map(|(key, _)| key.clone())
        .collect();
    for key in expired {
        finish_key(&key, active, calls, engine);
    }
}

fn finish_unbound(
    bindings: &HashMap<(u32, u32), Vec<Binding>>,
    active: &mut HashMap<CallKey, ActiveCall>,
    calls: &Calls,
    engine: &Engine,
) {
    let live: HashSet<(String, u32, u32)> = bindings
        .values()
        .flatten()
        .map(|binding| (binding.node.clone(), binding.device_set, binding.channel))
        .collect();
    let removed: Vec<CallKey> = active
        .keys()
        .filter(|key| !live.contains(&(key.node.clone(), key.device_set, key.channel)))
        .cloned()
        .collect();
    for key in removed {
        if let Some(call) = active.remove(&key) {
            complete(&key, call, calls, engine);
        }
    }
}

fn finish_all(active: &mut HashMap<CallKey, ActiveCall>, calls: &Calls, engine: &Weak<Engine>) {
    let keys: Vec<CallKey> = active.keys().cloned().collect();
    for key in keys {
        finish_key(&key, active, calls, engine);
    }
}

fn finish_key(
    key: &CallKey,
    active: &mut HashMap<CallKey, ActiveCall>,
    calls: &Calls,
    engine: &Weak<Engine>,
) {
    let Some(call) = active.remove(key) else {
        return;
    };
    let Some(engine) = engine.upgrade() else {
        return;
    };
    complete(key, call, calls, &engine);
}

fn complete(key: &CallKey, active: ActiveCall, calls: &Calls, engine: &Engine) {
    let encrypted = active.frame.encrypted == Some(true);
    let audio =
        (!encrypted && !active.audio.samples.is_empty()).then(|| wav(&active.audio.samples));
    let (call, evicted) = calls.push(
        NewCall {
            node: key.node.clone(),
            source_node: active.node,
            started_at: active.started_at,
            ended_at: format!("{:.9}", jiff::Timestamp::now()),
            duration_ms: active.started.elapsed().as_millis() as u64,
            device_set: key.device_set,
            channel: key.channel,
            freq_hz: active.freq_hz,
            frame: active.frame,
            audio_error: active.audio_error,
        },
        audio,
        active.retention,
    );
    engine.emit_event(ServerEvent::CallCompleted(Box::new(call)));
    if evicted {
        engine.emit_scope(StateScope::Calls);
    }
}

fn wav(samples: &[i16]) -> Bytes {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&STORED_RATE_HZ.to_le_bytes());
    out.extend_from_slice(&(STORED_RATE_HZ * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    Bytes::from(out)
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;
    use sdrmm_wire::DvMode;

    use super::*;

    fn new_call(encrypted: bool) -> NewCall {
        NewCall {
            node: "calls".to_owned(),
            source_node: "dmr".to_owned(),
            started_at: "2026-08-14T10:00:00Z".to_owned(),
            ended_at: "2026-08-14T10:00:01Z".to_owned(),
            duration_ms: 1_000,
            device_set: 1,
            channel: 2,
            freq_hz: 451_125_000.0,
            frame: DvFrame {
                encrypted: Some(encrypted),
                ..DvFrame::new(DvMode::Dmr, DvFrameKind::Header)
            },
            audio_error: None,
        }
    }

    #[test]
    fn wav_is_mono_8k_pcm() {
        let bytes = wav(&[0, i16::MAX, i16::MIN]);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 8_000);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
    }

    #[test]
    fn encrypted_completion_has_no_audio() {
        let calls = Calls::default();
        let (stored, _) = calls.push(new_call(true), None, Duration::from_secs(30));
        assert!(stored.encrypted);
        assert!(stored.audio.is_none());
        assert!(calls.audio(stored.id).is_none());
        assert_eq!(calls.list().len(), 1);
    }

    #[test]
    fn evicting_audio_keeps_the_call_and_says_why() {
        let calls = Calls::default();
        let big = Bytes::from(vec![0u8; MAX_STORED_AUDIO_BYTES / 2 + 1]);
        let mut evictions = 0;
        for _ in 0..3 {
            let (_, evicted) =
                calls.push(new_call(false), Some(big.clone()), Duration::from_secs(30));
            evictions += usize::from(evicted);
        }
        assert!(evictions >= 1, "eviction was never reported");
        let listed = calls.list();
        assert_eq!(listed.len(), 3);
        let evicted = listed.iter().filter(|call| call.audio.is_none()).count();
        assert!(evicted >= 1, "nothing was evicted over the byte limit");
        assert!(
            listed
                .iter()
                .filter(|call| call.audio.is_none())
                .all(|call| call.audio_error.is_some())
        );
        let inner = calls.lock();
        assert!(inner.audio_bytes <= MAX_STORED_AUDIO_BYTES);
    }

    #[test]
    fn a_partial_link_control_does_not_split_a_call() {
        let full = DvFrame {
            slot: Some(1),
            source: Some(1001),
            destination: Some(91),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Voice)
        };
        let partial = DvFrame {
            slot: Some(1),
            destination: Some(91),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Voice)
        };
        assert!(same_call(&full, &partial));
        let other = DvFrame {
            slot: Some(1),
            source: Some(2002),
            destination: Some(91),
            ..DvFrame::new(DvMode::Dmr, DvFrameKind::Voice)
        };
        assert!(!same_call(&full, &other));
    }

    #[test]
    fn the_two_slots_of_one_channel_are_two_calls() {
        let engine = Engine::with_registry(DeviceRegistry::new(), None);
        let calls = Calls::default();
        let taps = design_lowpass(ANTIALIAS_TAPS, 0.5 / DECIMATION as f64);
        let bindings = HashMap::from([(
            (1, 2),
            vec![Binding {
                node: "trunk".to_owned(),
                device_set: 1,
                channel: 2,
                retention: Duration::from_secs(30),
            }],
        )]);
        let mut active = HashMap::new();
        let weak = Arc::downgrade(&engine);
        for slot in [1, 2] {
            handle_record(
                &record(DvFrameKind::Header, slot),
                &bindings,
                &mut active,
                &calls,
                &weak,
                &taps,
            );
        }
        assert_eq!(active.len(), 2, "the slots merged into one call");
    }

    #[test]
    fn one_transmission_becomes_one_completed_call() {
        let engine = Engine::with_registry(DeviceRegistry::new(), None);
        let mut events = engine.subscribe_events();
        let calls = Calls::default();
        let taps = design_lowpass(ANTIALIAS_TAPS, 0.5 / DECIMATION as f64);
        let bindings = HashMap::from([(
            (1, 2),
            vec![Binding {
                node: "trunk".to_owned(),
                device_set: 1,
                channel: 2,
                retention: Duration::from_secs(30),
            }],
        )]);
        let mut active = HashMap::new();
        let weak = Arc::downgrade(&engine);
        for kind in [DvFrameKind::Header, DvFrameKind::Voice] {
            handle_record(
                &record(kind, 1),
                &bindings,
                &mut active,
                &calls,
                &weak,
                &taps,
            );
        }
        append_pcm(
            1,
            2,
            &PcmBlock {
                start_frame: 0,
                channels: 1,
                payload: PcmPayload::Samples(vec![0.25; 4_800].into()),
            },
            &mut active,
        );
        handle_record(
            &record(DvFrameKind::Terminator, 1),
            &bindings,
            &mut active,
            &calls,
            &weak,
            &taps,
        );

        assert!(active.is_empty());
        let listed = calls.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].node, "trunk");
        assert_eq!(listed[0].destination, Some(91));
        assert_eq!(
            listed[0].audio.as_ref().map(|audio| audio.url.as_str()),
            Some("/api/calls/1/audio")
        );
        let audio = calls.audio(listed[0].id).expect("audio");
        assert!(audio.len() > 44 && audio.len() <= 44 + 800 * 2);
        assert!(matches!(
            events.try_recv(),
            Ok(ServerEvent::CallCompleted(_))
        ));
    }

    fn record(kind: DvFrameKind, slot: u8) -> DecodedRecord {
        DecodedRecord {
            device_set: 1,
            channel: 2,
            at: "2026-08-14T10:00:00Z".to_owned(),
            freq_hz: 451_125_000.0,
            event: DecoderEvent::Dv(DvFrame {
                kind,
                slot: Some(slot),
                color_code: Some(3),
                source: Some(1001),
                destination: Some(91),
                group_call: Some(true),
                encrypted: Some(false),
                ..DvFrame::default()
            }),
        }
    }
}
