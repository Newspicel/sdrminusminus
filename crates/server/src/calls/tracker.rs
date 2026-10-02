use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use sdrmm_dsp::{decim::RealDecimator, fir::design_lowpass};
use sdrmm_engine::{Engine, PcmBlock, PcmPayload};
use sdrmm_wire::{DecodedRecord, DecoderEvent, DvFrame, DvFrameKind, StateScope};

use super::{Calls, DECIMATION, MAX_CALL_SAMPLES, NewCall, RETENTION, wav};
use crate::trunking::{CallBinding, Gate};

const ANTIALIAS_TAPS: usize = 96;
const IDLE_TIMEOUT: Duration = Duration::from_millis(900);
pub(super) const SQUELCH_HOLD_FRAMES: usize = 48_000 * 3 / 2;

pub(super) type Bindings = HashMap<(u32, u32), Vec<CallBinding>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CallKey {
    node: String,
    device_set: u32,
    channel: u32,
    slot: Option<u8>,
}

impl CallKey {
    fn of(binding: &CallBinding, slot: Option<u8>) -> Self {
        Self {
            node: binding.node.clone(),
            device_set: binding.device_set,
            channel: binding.channel,
            slot,
        }
    }

    fn on(&self, device_set: u32, channel: u32) -> bool {
        self.device_set == device_set && self.channel == channel
    }
}

struct ActiveCall {
    mode: String,
    freq_hz: f64,
    frame: Option<DvFrame>,
    started_at: jiff::Timestamp,
    heard_at: jiff::Timestamp,
    last_activity: Instant,
    quiet_frames: usize,
    audio: CallAudio,
    audio_error: Option<String>,
}

impl ActiveCall {
    fn new(
        mode: String,
        freq_hz: f64,
        frame: Option<DvFrame>,
        started_at: jiff::Timestamp,
        taps: &[f32],
    ) -> Self {
        Self {
            mode,
            freq_hz,
            frame,
            started_at,
            heard_at: started_at,
            last_activity: Instant::now(),
            quiet_frames: 0,
            audio: CallAudio::new(taps),
            audio_error: None,
        }
    }

    fn continued(&self, taps: &[f32]) -> Self {
        Self::new(
            self.mode.clone(),
            self.freq_hz,
            self.frame.clone(),
            jiff::Timestamp::now(),
            taps,
        )
    }

    fn encrypted(&self) -> bool {
        self.frame
            .as_ref()
            .is_some_and(|frame| frame.encrypted == Some(true))
    }

    fn heard(&mut self) {
        self.heard_at = jiff::Timestamp::now();
        self.last_activity = Instant::now();
    }

    fn into_new_call(self, key: &CallKey) -> (NewCall, Option<axum::body::Bytes>) {
        let audio =
            (!self.encrypted() && !self.audio.samples.is_empty()).then(|| wav(&self.audio.samples));
        let duration_ms = self
            .heard_at
            .duration_since(self.started_at)
            .as_millis()
            .max(0) as u64;
        let call = NewCall {
            node: key.node.clone(),
            started_at: format!("{:.9}", self.started_at),
            ended_at: format!("{:.9}", self.heard_at),
            duration_ms,
            device_set: key.device_set,
            channel: key.channel,
            freq_hz: self.freq_hz,
            mode: self.mode,
            frame: self.frame,
            audio_error: self.audio_error,
        };
        (call, audio)
    }
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

    fn push_block(&mut self, block: &PcmBlock) {
        match &block.payload {
            PcmPayload::Samples(samples) => self.push_samples(block.channels, samples),
            PcmPayload::Silence(frames) => self.push_silence(*frames),
        }
    }

    fn push_samples(&mut self, channels: u8, samples: &[f32]) {
        let channels = usize::from(channels.max(1));
        if channels == 1 {
            self.push(samples);
        } else {
            let mono: Vec<f32> = samples.iter().step_by(channels).copied().collect();
            self.push(&mono);
        }
    }

    fn push_silence(&mut self, frames: usize) {
        if frames > 0 {
            self.push(&vec![0.0; frames]);
        }
    }

    fn push(&mut self, input: &[f32]) {
        self.scratch.clear();
        self.decimator.process(input, &mut self.scratch);
        let room = MAX_CALL_SAMPLES.saturating_sub(self.samples.len());
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

pub(super) struct Tracker {
    calls: Arc<Calls>,
    engine: Weak<Engine>,
    taps: Vec<f32>,
    bindings: Bindings,
    active: HashMap<CallKey, ActiveCall>,
}

impl Tracker {
    pub(super) fn new(calls: Arc<Calls>, engine: Weak<Engine>) -> Self {
        Self {
            calls,
            engine,
            taps: design_lowpass(ANTIALIAS_TAPS, 0.5 / DECIMATION as f64),
            bindings: HashMap::new(),
            active: HashMap::new(),
        }
    }

    pub(super) fn sources(&self) -> HashSet<(u32, u32)> {
        self.bindings.keys().copied().collect()
    }

    pub(super) fn rebind(&mut self, bindings: Bindings) {
        self.bindings = bindings;
        let unbound: Vec<CallKey> = self
            .active
            .keys()
            .filter(|key| {
                !self
                    .bindings
                    .get(&(key.device_set, key.channel))
                    .is_some_and(|bound| bound.iter().any(|binding| binding.node == key.node))
            })
            .cloned()
            .collect();
        for key in unbound {
            self.finish(&key);
        }
    }

    pub(super) fn record(&mut self, record: &DecodedRecord) {
        if record.origin.is_some() {
            return;
        }
        let DecoderEvent::Dv(frame) = &record.event else {
            return;
        };
        let Some(bound) = self.bindings.get(&(record.device_set, record.channel)) else {
            return;
        };
        let keys: Vec<CallKey> = bound
            .iter()
            .filter(|binding| binding.gate == Gate::Frames)
            .map(|binding| CallKey::of(binding, frame.slot))
            .collect();
        for key in keys {
            match frame.kind {
                DvFrameKind::Header | DvFrameKind::Voice
                    if frame.source.is_some() || frame.destination.is_some() =>
                {
                    self.frame(key, record, frame);
                }
                DvFrameKind::Terminator => self.finish(&key),
                _ => {}
            }
        }
    }

    fn frame(&mut self, key: CallKey, record: &DecodedRecord, frame: &DvFrame) {
        if self.active.get(&key).is_some_and(|call| {
            call.frame
                .as_ref()
                .is_some_and(|current| !same_call(current, frame))
        }) {
            self.finish(&key);
        }
        let taps = &self.taps;
        let call = self.active.entry(key).or_insert_with(|| {
            ActiveCall::new(
                frame.mode.type_id().to_owned(),
                record.freq_hz,
                Some(frame.clone()),
                record.at.parse().unwrap_or_else(|_| jiff::Timestamp::now()),
                taps,
            )
        });
        if let Some(current) = &mut call.frame {
            merge_frame(current, frame);
        }
        call.heard();
        if call.encrypted() {
            call.audio.samples.clear();
        }
    }

    pub(super) fn pcm(&mut self, device_set: u32, channel: u32, block: &PcmBlock) {
        let Some(bound) = self.bindings.get(&(device_set, channel)) else {
            return;
        };
        let squelched: Vec<CallKey> = bound
            .iter()
            .filter(|binding| binding.gate == Gate::Squelch)
            .map(|binding| CallKey::of(binding, None))
            .collect();
        let framed = bound.iter().any(|binding| binding.gate == Gate::Frames);
        for key in squelched {
            self.squelched_pcm(key, block);
        }
        if framed {
            self.framed_pcm(device_set, channel, block);
        }
    }

    fn framed_pcm(&mut self, device_set: u32, channel: u32, block: &PcmBlock) {
        let mut full = Vec::new();
        for (key, call) in &mut self.active {
            if !key.on(device_set, channel) || call.frame.is_none() {
                continue;
            }
            call.last_activity = Instant::now();
            if call.encrypted() {
                continue;
            }
            call.audio.push_block(block);
            if call.audio.full() {
                full.push(key.clone());
            }
        }
        for key in full {
            self.roll_over(&key);
        }
    }

    fn squelched_pcm(&mut self, key: CallKey, block: &PcmBlock) {
        match &block.payload {
            PcmPayload::Samples(samples) => {
                if !self.active.contains_key(&key) {
                    let Some((mode, freq_hz)) = self.tuning(key.device_set, key.channel) else {
                        return;
                    };
                    let call =
                        ActiveCall::new(mode, freq_hz, None, jiff::Timestamp::now(), &self.taps);
                    self.active.insert(key.clone(), call);
                }
                let Some(call) = self.active.get_mut(&key) else {
                    return;
                };
                call.audio
                    .push_silence(std::mem::take(&mut call.quiet_frames));
                call.audio.push_samples(block.channels, samples);
                call.heard();
                if call.audio.full() {
                    self.roll_over(&key);
                }
            }
            PcmPayload::Silence(frames) => {
                let Some(call) = self.active.get_mut(&key) else {
                    return;
                };
                call.last_activity = Instant::now();
                call.quiet_frames += frames;
                if call.quiet_frames >= SQUELCH_HOLD_FRAMES {
                    self.finish(&key);
                }
            }
        }
    }

    fn tuning(&self, device_set: u32, channel: u32) -> Option<(String, f64)> {
        let engine = self.engine.upgrade()?;
        let snapshot = engine.snapshot();
        let info = snapshot
            .device_sets
            .iter()
            .find(|set| set.id == device_set)?
            .channels
            .iter()
            .find(|info| info.id == channel)?;
        Some((
            info.settings.params.type_id().to_owned(),
            info.settings.frequency_hz,
        ))
    }

    pub(super) fn audio_error(&mut self, device_set: u32, channel: u32, error: &str) {
        for (key, call) in &mut self.active {
            if key.on(device_set, channel) {
                call.audio_error.get_or_insert_with(|| error.to_owned());
            }
        }
    }

    pub(super) fn lost(&mut self, error: &str) {
        for call in self.active.values_mut() {
            call.audio_error.get_or_insert_with(|| error.to_owned());
        }
    }

    pub(super) fn expire_idle(&mut self) {
        let idle: Vec<CallKey> = self
            .active
            .iter()
            .filter(|(_, call)| call.last_activity.elapsed() >= IDLE_TIMEOUT)
            .map(|(key, _)| key.clone())
            .collect();
        for key in idle {
            self.finish(&key);
        }
    }

    pub(super) fn finish_all(&mut self) {
        let keys: Vec<CallKey> = self.active.keys().cloned().collect();
        for key in keys {
            self.finish(&key);
        }
    }

    fn roll_over(&mut self, key: &CallKey) {
        let Some(call) = self.active.get(key) else {
            return;
        };
        let next = call.continued(&self.taps);
        self.finish(key);
        self.active.insert(key.clone(), next);
    }

    fn finish(&mut self, key: &CallKey) {
        let Some(call) = self.active.remove(key) else {
            return;
        };
        let Some(engine) = self.engine.upgrade() else {
            return;
        };
        let (new, audio) = call.into_new_call(key);
        let (call, evicted) = self.calls.push(new, audio, RETENTION);
        engine.publish_decoded(DecodedRecord {
            origin: None,
            sinks: Vec::new(),
            device_set: key.device_set,
            channel: key.channel,
            at: call.ended_at.clone(),
            freq_hz: call.freq_hz,
            event: DecoderEvent::Call(call),
        });
        if evicted {
            engine.emit_scope(StateScope::Calls);
        }
    }

    #[cfg(test)]
    fn active(&self) -> usize {
        self.active.len()
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

#[cfg(test)]
mod tests;
