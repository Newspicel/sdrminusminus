use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sdrmm_engine::{Engine, PcmBlock, PcmPayload};
use sdrmm_wire::{
    DecodedRecord, DecoderEvent, DvFrame, DvFrameKind, EventAudio, NodeBody, ServerEvent,
    StateScope, VoiceCall,
};
use tokio::{
    sync::{broadcast::error::RecvError, mpsc},
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

use crate::{Store, trunking::Trunking};

const CALL_TIMEOUT: Duration = Duration::from_millis(900);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);
const MAX_CALL_SECONDS: usize = 600;
const MAX_STORED_CALLS: usize = 10_000;
const MAX_STORED_AUDIO_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Calls {
    inner: Mutex<StoredCalls>,
}

#[derive(Default)]
struct StoredCalls {
    next_id: u64,
    calls: VecDeque<StoredCall>,
}

struct StoredCall {
    call: VoiceCall,
    audio: Option<Arc<[u8]>>,
    expires: Instant,
}

impl Calls {
    pub(crate) fn list(&self) -> Vec<VoiceCall> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut inner);
        inner
            .calls
            .iter()
            .rev()
            .map(|item| item.call.clone())
            .collect()
    }

    pub(crate) fn audio(&self, id: u64) -> Option<Arc<[u8]>> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut inner);
        inner
            .calls
            .iter()
            .find(|item| item.call.id == id)
            .and_then(|item| item.audio.clone())
    }

    fn expire(&self) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = inner.calls.len();
        prune(&mut inner);
        inner.calls.len() != before
    }

    fn push(&self, mut call: VoiceCall, audio: Option<Arc<[u8]>>, retention: Duration) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        prune(&mut inner);
        inner.next_id = inner.next_id.wrapping_add(1).max(1);
        call.id = inner.next_id;
        call.audio = audio.as_ref().map(|_| EventAudio {
            url: format!("/api/calls/{}/audio", call.id),
            media_type: "audio/wav".to_owned(),
        });
        inner.calls.push_back(StoredCall {
            call,
            audio,
            expires: Instant::now() + retention,
        });
        while inner.calls.len() > MAX_STORED_CALLS {
            inner.calls.pop_front();
        }
        let mut audio_bytes = inner
            .calls
            .iter()
            .filter_map(|item| item.audio.as_ref())
            .map(|audio| audio.len())
            .sum::<usize>();
        for item in &mut inner.calls {
            if audio_bytes <= MAX_STORED_AUDIO_BYTES {
                break;
            }
            let Some(audio) = item.audio.take() else {
                continue;
            };
            audio_bytes = audio_bytes.saturating_sub(audio.len());
            item.call.audio = None;
            item.call
                .audio_error
                .get_or_insert_with(|| "audio evicted by the temporary buffer limit".to_owned());
        }
    }
}

fn prune(inner: &mut StoredCalls) {
    let now = Instant::now();
    inner.calls.retain(|item| item.expires > now);
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CallKey {
    node: String,
    device_set: u32,
    channel: u32,
}

#[derive(Clone)]
struct Binding {
    key: CallKey,
    source_node: String,
    retention: Duration,
}

struct ActiveCall {
    source_node: String,
    started_at: String,
    started: Instant,
    last_activity: Instant,
    freq_hz: f64,
    frame: DvFrame,
    retention: Duration,
    samples: Vec<i16>,
    audio_error: Option<String>,
}

enum Input {
    Pcm(u32, u32, PcmBlock),
    AudioError(u32, u32, String),
}

pub(crate) fn spawn(
    engine: Arc<Engine>,
    store: Arc<Store>,
    calls: Arc<Calls>,
    trunking: Arc<Trunking>,
) -> Option<JoinHandle<()>> {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("no runtime in context: completed calls will not be buffered");
        return None;
    };
    let _guard = handle.enter();
    Some(tokio::spawn(run(engine, store, calls, trunking)))
}

async fn run(engine: Arc<Engine>, store: Arc<Store>, calls: Arc<Calls>, trunking: Arc<Trunking>) {
    let mut decoded = engine.subscribe_decoded();
    let mut events = engine.subscribe_events();
    let (input_tx, mut input_rx) = mpsc::channel(1024);
    let mut audio_tasks: HashMap<(u32, u32), JoinHandle<()>> = HashMap::new();
    let mut bindings = resolve_bindings(&store, &trunking);
    let mut active: HashMap<CallKey, ActiveCall> = HashMap::new();
    reconcile_audio(&engine, &input_tx, &bindings, &mut audio_tasks);
    let mut reconcile_tick = interval(RECONCILE_INTERVAL);
    reconcile_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut timeout_tick = interval(Duration::from_millis(200));
    timeout_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            received = decoded.recv() => match received {
                Ok(record) => handle_record(&record, &bindings, &mut active, &calls, &engine),
                Err(RecvError::Lagged(count)) => mark_all_audio_errors(
                    &mut active,
                    format!("decoder event stream lost {count} record(s)"),
                ),
                Err(RecvError::Closed) => break,
            },
            received = events.recv() => match received {
                Ok(ServerEvent::StateChanged { scope: StateScope::All | StateScope::DeviceSet(_) | StateScope::Workspaces }) => {
                    let fresh = resolve_bindings(&store, &trunking);
                    reconcile_audio(&engine, &input_tx, &fresh, &mut audio_tasks);
                    finish_unbound(&fresh, &mut active, &calls, &engine);
                    bindings = fresh;
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            input = input_rx.recv() => match input {
                Some(Input::Pcm(ds, channel, block)) => append_pcm(ds, channel, block, &mut active),
                Some(Input::AudioError(ds, channel, error)) => {
                    mark_audio_error(ds, channel, error, &mut active);
                }
                None => break,
            },
            _ = reconcile_tick.tick() => {
                if calls.expire() {
                    engine.emit_scope(StateScope::Calls);
                }
                let fresh = resolve_bindings(&store, &trunking);
                reconcile_audio(&engine, &input_tx, &fresh, &mut audio_tasks);
                finish_unbound(&fresh, &mut active, &calls, &engine);
                bindings = fresh;
            }
            _ = timeout_tick.tick() => finish_timed_out(&mut active, &calls, &engine),
        }
    }
    for (_, task) in audio_tasks {
        task.abort();
    }
    finish_all(&mut active, &calls, &engine);
}

fn resolve_bindings(store: &Store, trunking: &Trunking) -> HashMap<(u32, u32), Vec<Binding>> {
    let Ok(Some(workspace)) = store.active_workspace() else {
        return HashMap::new();
    };
    let graph = &workspace.snapshot.graph;
    let mut resolved: HashMap<(u32, u32), Vec<Binding>> = HashMap::new();
    for ((device_set, channel), system_node) in trunking.followers() {
        let Some(node) = graph.node(&system_node) else {
            continue;
        };
        let NodeBody::DmrTrunk(settings) = &node.body else {
            continue;
        };
        if let Some(binding) =
            retained_binding(system_node, device_set, channel, settings.retention_seconds)
        {
            resolved
                .entry((device_set, channel))
                .or_default()
                .push(binding);
        }
    }
    resolved
}

fn retained_binding(
    system_node: String,
    device_set: u32,
    channel: u32,
    retention_seconds: u32,
) -> Option<Binding> {
    (retention_seconds > 0).then(|| Binding {
        key: CallKey {
            node: system_node.clone(),
            device_set,
            channel,
        },
        source_node: system_node,
        retention: Duration::from_secs(u64::from(retention_seconds)),
    })
}

fn reconcile_audio(
    engine: &Arc<Engine>,
    input_tx: &mpsc::Sender<Input>,
    bindings: &HashMap<(u32, u32), Vec<Binding>>,
    tasks: &mut HashMap<(u32, u32), JoinHandle<()>>,
) {
    let wanted: HashSet<(u32, u32)> = bindings.keys().copied().collect();
    tasks.retain(|source, task| {
        if wanted.contains(source) {
            true
        } else {
            task.abort();
            false
        }
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
            match receiver.recv().await {
                Ok(block) => {
                    if input
                        .send(Input::Pcm(source.0, source.1, block))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(RecvError::Lagged(count)) => {
                    let error = format!("audio stream lost {count} block(s)");
                    if input
                        .send(Input::AudioError(source.0, source.1, error))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(RecvError::Closed) => return,
            }
        }
    })
}

fn handle_record(
    record: &DecodedRecord,
    bindings: &HashMap<(u32, u32), Vec<Binding>>,
    active: &mut HashMap<CallKey, ActiveCall>,
    calls: &Calls,
    engine: &Engine,
) {
    let DecoderEvent::Dv(frame) = &record.event else {
        return;
    };
    let Some(bound) = bindings.get(&(record.device_set, record.channel)) else {
        return;
    };
    for binding in bound {
        match frame.kind {
            DvFrameKind::Header | DvFrameKind::Voice
                if frame.source.is_some() || frame.destination.is_some() =>
            {
                update_call(binding, record, frame, active, calls, engine);
            }
            DvFrameKind::Terminator => finish_key(&binding.key, active, calls, engine),
            _ => {}
        }
    }
}

fn update_call(
    binding: &Binding,
    record: &DecodedRecord,
    frame: &DvFrame,
    active: &mut HashMap<CallKey, ActiveCall>,
    calls: &Calls,
    engine: &Engine,
) {
    let replace = active
        .get(&binding.key)
        .is_some_and(|call| !same_call(&call.frame, frame));
    if replace {
        finish_key(&binding.key, active, calls, engine);
    }
    let call = active
        .entry(binding.key.clone())
        .or_insert_with(|| ActiveCall {
            source_node: binding.source_node.clone(),
            started_at: record.at.clone(),
            started: Instant::now(),
            last_activity: Instant::now(),
            freq_hz: record.freq_hz,
            frame: frame.clone(),
            retention: binding.retention,
            samples: Vec::new(),
            audio_error: None,
        });
    merge_frame(&mut call.frame, frame);
    call.last_activity = Instant::now();
    call.retention = binding.retention;
    if call.frame.encrypted == Some(true) {
        call.samples.clear();
    }
}

fn same_call(current: &DvFrame, incoming: &DvFrame) -> bool {
    current.slot == incoming.slot
        && current.source == incoming.source
        && current.destination == incoming.destination
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
    block: PcmBlock,
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
        let remaining = 48_000 * MAX_CALL_SECONDS - call.samples.len();
        if remaining == 0 {
            call.audio_error = Some("audio exceeded the ten minute call limit".to_owned());
            continue;
        }
        match &block.payload {
            PcmPayload::Samples(samples) => {
                let channels = usize::from(block.channels.max(1));
                call.samples.extend(
                    samples
                        .iter()
                        .step_by(channels)
                        .take(remaining)
                        .map(|sample| (sample.clamp(-1.0, 1.0) * 32_767.0) as i16),
                );
            }
            PcmPayload::Silence(frames) => {
                call.samples
                    .resize(call.samples.len() + (*frames).min(remaining), 0);
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

fn finish_timed_out(active: &mut HashMap<CallKey, ActiveCall>, calls: &Calls, engine: &Engine) {
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
    let valid: HashSet<&CallKey> = bindings
        .values()
        .flat_map(|values| values.iter().map(|binding| &binding.key))
        .collect();
    let removed: Vec<CallKey> = active
        .keys()
        .filter(|key| !valid.contains(key))
        .cloned()
        .collect();
    for key in removed {
        finish_key(&key, active, calls, engine);
    }
}

fn finish_all(active: &mut HashMap<CallKey, ActiveCall>, calls: &Calls, engine: &Engine) {
    let keys: Vec<CallKey> = active.keys().cloned().collect();
    for key in keys {
        finish_key(&key, active, calls, engine);
    }
}

fn finish_key(
    key: &CallKey,
    active: &mut HashMap<CallKey, ActiveCall>,
    calls: &Calls,
    engine: &Engine,
) {
    let Some(active) = active.remove(key) else {
        return;
    };
    let encrypted = active.frame.encrypted == Some(true);
    let audio = (!encrypted && !active.samples.is_empty()).then(|| wav(&active.samples));
    let call = VoiceCall {
        id: 0,
        node: key.node.clone(),
        source_node: active.source_node,
        started_at: active.started_at,
        ended_at: format!("{:.9}", jiff::Timestamp::now()),
        duration_ms: active.started.elapsed().as_millis() as u64,
        device_set: key.device_set,
        channel: key.channel,
        freq_hz: active.freq_hz,
        mode: active.frame.mode,
        slot: active.frame.slot,
        color_code: active.frame.color_code,
        source: active.frame.source,
        destination: active.frame.destination,
        group_call: active.frame.group_call,
        encrypted,
        emergency: active.frame.emergency == Some(true),
        audio: None,
        audio_error: active.audio_error,
    };
    calls.push(call, audio, active.retention);
    engine.emit_scope(StateScope::Calls);
}

fn wav(samples: &[i16]) -> Arc<[u8]> {
    let data_len = samples.len().saturating_mul(2).min(u32::MAX as usize) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&96_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples.iter().take(data_len as usize / 2) {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out.into()
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;
    use sdrmm_wire::{DvMode, VoiceCall};

    use super::*;

    fn call(id: u64) -> VoiceCall {
        VoiceCall {
            id,
            node: "calls".to_owned(),
            source_node: "dmr".to_owned(),
            started_at: "2026-08-14T10:00:00Z".to_owned(),
            ended_at: "2026-08-14T10:00:01Z".to_owned(),
            duration_ms: 1_000,
            device_set: 1,
            channel: 2,
            freq_hz: 451_125_000.0,
            mode: DvMode::Dmr,
            slot: Some(1),
            color_code: Some(3),
            source: Some(1001),
            destination: Some(91),
            group_call: Some(true),
            encrypted: false,
            emergency: false,
            audio: None,
            audio_error: None,
        }
    }

    #[test]
    fn wav_is_mono_48k_pcm() {
        let bytes = wav(&[0, i16::MAX, i16::MIN]);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
    }

    #[test]
    fn encrypted_completion_has_no_audio() {
        let calls = Calls::default();
        let mut item = call(0);
        item.encrypted = true;
        calls.push(item, None, Duration::from_secs(30));
        let listed = calls.list();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].encrypted);
        assert!(listed[0].audio.is_none());
        assert!(calls.audio(listed[0].id).is_none());
    }

    #[test]
    fn zero_retention_disables_call_assembly() {
        assert!(retained_binding("system".to_owned(), 1, 2, 0).is_none());
        let binding = retained_binding("system".to_owned(), 1, 2, 60).expect("retained");
        assert_eq!(binding.retention, Duration::from_secs(60));
    }

    #[test]
    fn one_transmission_becomes_one_completed_call() {
        let engine = Engine::with_registry(DeviceRegistry::new(), None);
        let calls = Calls::default();
        let key = CallKey {
            node: "dmr-system".to_owned(),
            device_set: 1,
            channel: 2,
        };
        let bindings = HashMap::from([(
            (1, 2),
            vec![Binding {
                key: key.clone(),
                source_node: "dmr-system".to_owned(),
                retention: Duration::from_secs(30),
            }],
        )]);
        let mut active = HashMap::new();
        for kind in [DvFrameKind::Header, DvFrameKind::Voice] {
            handle_record(&record(kind), &bindings, &mut active, &calls, &engine);
        }
        append_pcm(
            1,
            2,
            PcmBlock {
                start_frame: 0,
                channels: 1,
                payload: PcmPayload::Samples(vec![0.25; 960].into()),
            },
            &mut active,
        );
        handle_record(
            &record(DvFrameKind::Terminator),
            &bindings,
            &mut active,
            &calls,
            &engine,
        );

        assert!(active.is_empty());
        let listed = calls.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].node, "dmr-system");
        assert_eq!(listed[0].source_node, "dmr-system");
        assert_eq!(listed[0].destination, Some(91));
        assert_eq!(
            listed[0].audio.as_ref().map(|audio| audio.url.as_str()),
            Some("/api/calls/1/audio")
        );
        assert!(calls.audio(listed[0].id).is_some());
    }

    fn record(kind: DvFrameKind) -> DecodedRecord {
        DecodedRecord {
            device_set: 1,
            channel: 2,
            at: "2026-08-14T10:00:00Z".to_owned(),
            freq_hz: 451_125_000.0,
            event: DecoderEvent::Dv(DvFrame {
                kind,
                slot: Some(1),
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
