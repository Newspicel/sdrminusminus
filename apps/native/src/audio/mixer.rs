use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

use rtrb::{Consumer, Producer};

use super::{
    CHANNELS, MAX_FRAMES, TARGET_FRAMES,
    geiger::{ClickState, ClickSynth},
    jitter::JitterBuffer,
};

pub const MAX_VOICES: usize = 32;
const BLOCK_FRAMES: usize = 2_048;
const GAIN_RAMP_SECONDS: f32 = 0.02;

#[derive(Default)]
pub struct VoiceStats {
    buffered: AtomicU32,
    trimmed: AtomicU64,
    underruns: AtomicU32,
    gain: AtomicU32,
    epoch: AtomicU32,
}

impl VoiceStats {
    #[must_use]
    pub fn with_gain(gain: f32) -> Self {
        let stats = Self::default();
        stats.set_gain(gain);
        stats
    }

    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    pub fn restart(&self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn buffered_frames(&self) -> u32 {
        self.buffered.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn trimmed_frames(&self) -> u64 {
        self.trimmed.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn underruns(&self) -> u32 {
        self.underruns.load(Ordering::Relaxed)
    }

    fn report(&self, jitter: &JitterBuffer) {
        self.buffered
            .store(jitter.buffered() as u32, Ordering::Relaxed);
        self.trimmed.store(jitter.trimmed(), Ordering::Relaxed);
        self.underruns.store(jitter.underruns(), Ordering::Relaxed);
    }
}

pub struct Voice {
    id: u64,
    feed: Consumer<f32>,
    jitter: JitterBuffer,
    stats: Arc<VoiceStats>,
    gain: f32,
    epoch: u32,
    retiring: bool,
}

impl Voice {
    #[must_use]
    pub fn new(id: u64, feed: Consumer<f32>, stats: Arc<VoiceStats>) -> Self {
        let gain = stats.gain();
        Self {
            id,
            feed,
            jitter: JitterBuffer::new(TARGET_FRAMES, MAX_FRAMES, CHANNELS),
            stats,
            gain,
            epoch: 0,
            retiring: false,
        }
    }

    fn drain(&mut self) {
        let epoch = self.stats.epoch.load(Ordering::Relaxed);
        let waiting = self.feed.slots() - self.feed.slots() % CHANNELS;
        let Ok(chunk) = self.feed.read_chunk(waiting) else {
            return;
        };
        if epoch == self.epoch {
            let (first, second) = chunk.as_slices();
            self.jitter.push(first);
            self.jitter.push(second);
        } else {
            self.epoch = epoch;
            self.jitter.clear();
        }
        chunk.commit_all();
    }
}

pub enum Command {
    Add(Box<Voice>),
    Remove(u64),
}

pub struct Mixer {
    voices: Vec<Option<Box<Voice>>>,
    commands: Consumer<Command>,
    retired: Producer<Box<Voice>>,
    clicks: Arc<ClickState>,
    synth: ClickSynth,
    base_rate: f64,
    ramp: f32,
    left: Vec<f32>,
    right: Vec<f32>,
    voice_left: Vec<f32>,
    voice_right: Vec<f32>,
}

impl Mixer {
    #[must_use]
    pub fn new(
        commands: Consumer<Command>,
        retired: Producer<Box<Voice>>,
        clicks: Arc<ClickState>,
        device_rate: u32,
    ) -> Self {
        let rate = device_rate.max(1) as f32;
        Self {
            voices: (0..MAX_VOICES).map(|_| None).collect(),
            commands,
            retired,
            clicks,
            synth: ClickSynth::new(rate),
            base_rate: f64::from(super::SAMPLE_RATE) / f64::from(rate),
            ramp: 1.0 - (-1.0 / (GAIN_RAMP_SECONDS * rate)).exp(),
            left: vec![0.0; BLOCK_FRAMES],
            right: vec![0.0; BLOCK_FRAMES],
            voice_left: vec![0.0; BLOCK_FRAMES],
            voice_right: vec![0.0; BLOCK_FRAMES],
        }
    }

    pub fn fill<T>(&mut self, out: &mut [T], channels: usize)
    where
        T: cpal::Sample + cpal::FromSample<f32>,
    {
        self.obey();
        let channels = channels.max(1);
        for block in out.chunks_mut(BLOCK_FRAMES * channels) {
            let frames = block.len() / channels;
            self.mix(frames);
            for (frame, samples) in block.chunks_mut(channels).enumerate() {
                let (l, r) = (self.left[frame], self.right[frame]);
                for (lane, sample) in samples.iter_mut().enumerate() {
                    let value = match (channels, lane) {
                        (1, _) => 0.5 * (l + r),
                        (_, 0) => l,
                        (_, 1) => r,
                        _ => 0.0,
                    };
                    *sample = T::from_sample(value);
                }
            }
        }
        self.retire();
    }

    fn obey(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Add(voice) => self.seat(voice),
                Command::Remove(id) => {
                    for voice in self.voices.iter_mut().flatten() {
                        if voice.id == id {
                            voice.retiring = true;
                        }
                    }
                }
            }
        }
    }

    fn seat(&mut self, voice: Box<Voice>) {
        match self.voices.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(voice),
            None => {
                let mut voice = voice;
                voice.retiring = true;
                if let Err(rtrb::PushError::Full(voice)) = self.retired.push(voice) {
                    std::mem::forget(voice);
                }
            }
        }
    }

    fn retire(&mut self) {
        for slot in &mut self.voices {
            if slot.as_ref().is_none_or(|voice| !voice.retiring) || self.retired.is_full() {
                continue;
            }
            if let Some(voice) = slot.take()
                && let Err(rtrb::PushError::Full(voice)) = self.retired.push(voice)
            {
                *slot = Some(voice);
            }
        }
    }

    fn mix(&mut self, frames: usize) {
        let left = &mut self.left[..frames];
        let right = &mut self.right[..frames];
        left.fill(0.0);
        right.fill(0.0);
        for voice in self.voices.iter_mut().flatten() {
            if voice.retiring {
                continue;
            }
            voice.drain();
            let voice_left = &mut self.voice_left[..frames];
            let voice_right = &mut self.voice_right[..frames];
            let played = voice
                .jitter
                .read_at(&mut [&mut *voice_left, &mut *voice_right], self.base_rate);
            voice.stats.report(&voice.jitter);
            let target = voice.stats.gain();
            let mut gain = voice.gain;
            for frame in 0..frames {
                gain += (target - gain) * self.ramp;
                if played {
                    left[frame] += voice_left[frame] * gain;
                    right[frame] += voice_right[frame] * gain;
                }
            }
            voice.gain = gain;
        }
        self.synth.add(self.clicks.read(), left, right);
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;

    struct Rig {
        mixer: Mixer,
        commands: Producer<Command>,
        retired: Consumer<Box<Voice>>,
    }

    fn rig() -> Rig {
        let (commands, command_feed) = RingBuffer::new(8);
        let (retire_feed, retired) = RingBuffer::new(8);
        Rig {
            mixer: Mixer::new(
                command_feed,
                retire_feed,
                Arc::new(ClickState::default()),
                48_000,
            ),
            commands,
            retired,
        }
    }

    fn voice(rig: &mut Rig, id: u64, gain: f32) -> (Producer<f32>, Arc<VoiceStats>) {
        let (feed, consumer) = RingBuffer::new(4 * MAX_FRAMES * CHANNELS);
        let stats = Arc::new(VoiceStats::with_gain(gain));
        assert!(
            rig.commands
                .push(Command::Add(Box::new(Voice::new(
                    id,
                    consumer,
                    stats.clone()
                ))))
                .is_ok()
        );
        (feed, stats)
    }

    fn feed(producer: &mut Producer<f32>, frames: usize, value: f32) {
        for _ in 0..frames * CHANNELS {
            assert!(producer.push(value).is_ok());
        }
    }

    #[test]
    fn plays_silence_until_a_voice_has_its_target_then_its_audio() {
        let mut rig = rig();
        let (mut producer, stats) = voice(&mut rig, 1, 1.0);
        let mut out = vec![1.0f32; 256];
        rig.mixer.fill(&mut out, 2);
        assert!(out.iter().all(|sample| *sample == 0.0));
        feed(&mut producer, TARGET_FRAMES, 0.5);
        rig.mixer.fill(&mut out, 2);
        assert!(out.iter().all(|sample| (sample - 0.5).abs() < 1e-3));
        assert_eq!(stats.buffered_frames() as usize, TARGET_FRAMES - 128);
    }

    #[test]
    fn mixes_voices_by_their_own_gain() {
        let mut rig = rig();
        let (mut loud, _) = voice(&mut rig, 1, 1.0);
        let (mut quiet, quiet_stats) = voice(&mut rig, 2, 0.0);
        feed(&mut loud, 2 * TARGET_FRAMES, 0.25);
        feed(&mut quiet, 2 * TARGET_FRAMES, 0.25);
        let mut out = vec![0.0f32; 64];
        rig.mixer.fill(&mut out, 2);
        assert!(out.iter().all(|sample| (sample - 0.25).abs() < 1e-3));
        quiet_stats.set_gain(1.0);
        let mut later = vec![0.0f32; 2 * 4_800];
        rig.mixer.fill(&mut later, 2);
        assert!((later[later.len() - 1] - 0.5).abs() < 0.01);
    }

    #[test]
    fn folds_to_mono_and_leaves_extra_lanes_silent() {
        let mut rig = rig();
        let (mut producer, _) = voice(&mut rig, 1, 1.0);
        feed(&mut producer, TARGET_FRAMES, 0.5);
        let mut mono = vec![0.0f32; 16];
        rig.mixer.fill(&mut mono, 1);
        assert!(mono.iter().all(|sample| (sample - 0.5).abs() < 1e-3));
        let mut quad = vec![1.0f32; 16];
        rig.mixer.fill(&mut quad, 4);
        assert!((quad[0] - 0.5).abs() < 1e-3 && quad[2] == 0.0 && quad[3] == 0.0);
    }

    #[test]
    fn a_restart_discards_what_was_queued() {
        let mut rig = rig();
        let (mut producer, stats) = voice(&mut rig, 1, 1.0);
        feed(&mut producer, TARGET_FRAMES, 0.5);
        stats.restart();
        let mut out = vec![1.0f32; 64];
        rig.mixer.fill(&mut out, 2);
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(stats.buffered_frames(), 0);
    }

    #[test]
    fn a_removed_voice_goes_back_to_be_freed_off_the_audio_thread() {
        let mut rig = rig();
        let (_producer, _) = voice(&mut rig, 7, 1.0);
        let mut out = vec![0.0f32; 16];
        rig.mixer.fill(&mut out, 2);
        assert!(rig.commands.push(Command::Remove(7)).is_ok());
        rig.mixer.fill(&mut out, 2);
        assert_eq!(rig.retired.pop().map(|voice| voice.id).ok(), Some(7));
    }

    #[test]
    fn converts_to_integer_output_formats() {
        let mut rig = rig();
        let (mut producer, _) = voice(&mut rig, 1, 1.0);
        feed(&mut producer, TARGET_FRAMES, 0.5);
        let mut out = vec![0i16; 8];
        rig.mixer.fill(&mut out, 2);
        assert!(out.iter().all(|sample| (*sample - 16_384).abs() < 64));
    }
}
