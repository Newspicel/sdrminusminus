use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use axum::body::Bytes;
use sdrmm_engine::{Engine, PcmBlock};
use sdrmm_wire::{DvFrame, EventAudio, ServerEvent, StateScope, VoiceCall};
use tokio::{
    sync::{broadcast::error::RecvError, mpsc, watch},
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

use crate::trunking::{CallBinding, CallPolicy, Gate, Recording};

mod tracker;

use tracker::{Bindings, Tracker};

const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

const STORED_RATE_HZ: u32 = 8_000;
const DECIMATION: usize = 48_000 / STORED_RATE_HZ as usize;

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
    clips: VecDeque<(u64, Bytes, Instant)>,
}

struct StoredCall {
    call: VoiceCall,
    audio: Option<Bytes>,
    expires: Instant,
}

pub(crate) struct NewCall {
    pub node: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub device_set: u32,
    pub channel: u32,
    pub freq_hz: f64,
    pub mode: String,
    pub frame: Option<DvFrame>,
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
            .or_else(|| {
                inner
                    .clips
                    .iter()
                    .find(|(key, _, _)| *key == id)
                    .map(|(_, bytes, _)| bytes.clone())
            })
    }

    pub(crate) fn store_clip(&self, samples: &[i16]) -> (EventAudio, bool) {
        let mut inner = self.lock();
        prune(&mut inner);
        inner.next_id += 1;
        let id = inner.next_id;
        let audio = wav(samples);
        inner.audio_bytes += audio.len();
        inner
            .clips
            .push_back((id, audio, Instant::now() + RETENTION));
        let evicted = evict_audio(&mut inner);
        (
            EventAudio {
                url: crate::rest::call_audio_path(id),
                media_type: "audio/wav".to_owned(),
            },
            evicted,
        )
    }

    pub(crate) fn event_audio(&self, audio: &EventAudio) -> Option<Bytes> {
        let id = audio
            .url
            .strip_prefix("/api/calls/")?
            .strip_suffix("/audio")?
            .parse()
            .ok()?;
        self.audio(id)
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
        let frame = new.frame.unwrap_or_default();
        let call = VoiceCall {
            id: inner.next_id,
            node: new.node,
            started_at: new.started_at,
            ended_at: new.ended_at,
            duration_ms: new.duration_ms,
            device_set: new.device_set,
            channel: new.channel,
            freq_hz: new.freq_hz,
            mode: new.mode,
            slot: frame.slot,
            color_code: frame.color_code,
            source: frame.source,
            destination: frame.destination,
            group_call: frame.group_call,
            encrypted: frame.encrypted == Some(true),
            emergency: frame.emergency == Some(true),
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
    inner.clips.retain(|(_, audio, expires)| {
        if *expires > now {
            true
        } else {
            freed += audio.len();
            false
        }
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
    while inner.audio_bytes > MAX_STORED_AUDIO_BYTES || inner.clips.len() > MAX_STORED_CALLS {
        let Some((_, audio, _)) = inner.clips.pop_front() else {
            break;
        };
        inner.audio_bytes -= audio.len();
        evicted = true;
    }
    evicted
}

enum Input {
    Pcm(u32, u32, Box<PcmBlock>),
    AudioError(u32, u32, String),
}

pub(crate) async fn run(
    engine: Weak<Engine>,
    calls: Arc<Calls>,
    mut recording: watch::Receiver<Recording>,
) {
    let Some(strong) = engine.upgrade() else {
        return;
    };
    let mut decoded = strong.subscribe_decoded();
    let mut events = strong.subscribe_events();
    drop(strong);
    let (input_tx, mut input_rx) = mpsc::channel(1024);
    let mut audio_tasks: HashMap<(u32, u32), JoinHandle<()>> = HashMap::new();
    let mut tracker = Tracker::new(calls.clone(), engine.clone());
    let mut reconcile_tick = ticker(RECONCILE_INTERVAL);
    let mut idle_tick = ticker(Duration::from_millis(200));
    let mut rebind = true;
    loop {
        if rebind {
            rebind = false;
            let Some(strong) = engine.upgrade() else {
                break;
            };
            tracker.rebind(resolve_bindings(&strong, &recording.borrow()));
            reconcile_audio(&strong, &input_tx, &tracker.sources(), &mut audio_tasks);
        }
        tokio::select! {
            received = decoded.recv() => match received {
                Ok(record) => tracker.record(&record),
                Err(RecvError::Lagged(count)) => {
                    tracker.lost(&format!("decoder event stream lost {count} record(s)"));
                }
                Err(RecvError::Closed) => break,
            },
            received = events.recv() => match received {
                Ok(ServerEvent::StateChanged {
                    scope: StateScope::All | StateScope::DeviceSet(_) | StateScope::Workspaces,
                }) => rebind = true,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            changed = recording.changed() => match changed {
                Ok(()) => rebind = true,
                Err(_) => break,
            },
            input = input_rx.recv() => match input {
                Some(Input::Pcm(ds, channel, block)) => tracker.pcm(ds, channel, &block),
                Some(Input::AudioError(ds, channel, error)) => {
                    tracker.audio_error(ds, channel, &error);
                }
                None => break,
            },
            _ = reconcile_tick.tick() => {
                rebind = true;
                if calls.expire() && let Some(strong) = engine.upgrade() {
                    strong.emit_scope(StateScope::Calls);
                }
            }
            _ = idle_tick.tick() => tracker.expire_idle(),
        }
    }
    for (_, task) in audio_tasks {
        task.abort();
    }
    tracker.finish_all();
}

fn ticker(period: Duration) -> tokio::time::Interval {
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

fn resolve_bindings(engine: &Engine, policy: &CallPolicy) -> Bindings {
    let mut resolved = Bindings::new();
    for system in engine.trunk_systems() {
        if !policy.trunk_systems.contains(&system.node) {
            continue;
        }
        for follower in system.followers {
            resolved
                .entry((follower.device_set, follower.channel))
                .or_default()
                .push(CallBinding {
                    node: system.node.clone(),
                    device_set: follower.device_set,
                    channel: follower.channel,
                    gate: Gate::Frames,
                });
        }
    }
    for binding in &policy.channels {
        resolved
            .entry((binding.device_set, binding.channel))
            .or_default()
            .push(binding.clone());
    }
    resolved
}

fn reconcile_audio(
    engine: &Arc<Engine>,
    input_tx: &mpsc::Sender<Input>,
    wanted: &HashSet<(u32, u32)>,
    tasks: &mut HashMap<(u32, u32), JoinHandle<()>>,
) {
    tasks.retain(|source, task| {
        let keep = wanted.contains(source);
        if !keep {
            task.abort();
        }
        keep
    });
    for &source in wanted {
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
mod tests;
