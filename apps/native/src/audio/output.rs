use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use anyhow::{Context, anyhow};
use cpal::{
    DeviceId, SampleFormat, Stream, StreamConfig, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, RingBuffer};

use super::{
    CHANNELS, MAX_FRAMES, SAMPLE_RATE,
    geiger::ClickState,
    mixer::{Command, MAX_VOICES, Mixer, Voice, VoiceStats},
};

const FEED_SAMPLES: usize = 2 * MAX_FRAMES * CHANNELS;

pub struct Output {
    stream: Stream,
    commands: Producer<Command>,
    retired: Consumer<Box<Voice>>,
    clicks: Arc<ClickState>,
    failed: Arc<AtomicBool>,
    latency_us: Arc<AtomicU32>,
    device: Option<DeviceId>,
    next_id: u64,
    seated: usize,
}

impl Output {
    pub fn open() -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no audio output device"))?;
        let supported = pick_config(&device)?;
        let (commands, command_feed) = RingBuffer::new(4 * MAX_VOICES);
        let (retire_feed, retired) = RingBuffer::new(2 * MAX_VOICES);
        let clicks = Arc::new(ClickState::default());
        let failed = Arc::new(AtomicBool::new(false));
        let latency_us = Arc::new(AtomicU32::new(0));
        let config: StreamConfig = supported.config();
        let mixer = Mixer::new(
            command_feed,
            retire_feed,
            clicks.clone(),
            config.sample_rate,
        );
        let wiring = Wiring {
            config,
            mixer,
            failed: failed.clone(),
            latency_us: latency_us.clone(),
        };
        let stream = match supported.sample_format() {
            SampleFormat::F32 => wiring.build::<f32>(&device),
            SampleFormat::I16 => wiring.build::<i16>(&device),
            SampleFormat::U16 => wiring.build::<u16>(&device),
            SampleFormat::I32 => wiring.build::<i32>(&device),
            other => Err(anyhow!("the audio device wants {other} samples")),
        }?;
        stream.play().context("cannot start the audio output")?;
        Ok(Self {
            stream,
            commands,
            retired,
            clicks,
            failed,
            latency_us,
            device: device.id().ok(),
            next_id: 0,
            seated: 0,
        })
    }

    pub fn add(&mut self, stats: Arc<VoiceStats>) -> anyhow::Result<(u64, Producer<f32>)> {
        self.collect();
        if self.seated >= MAX_VOICES {
            return Err(anyhow!("at most {MAX_VOICES} sources play at once"));
        }
        let (feed, consumer) = RingBuffer::new(FEED_SAMPLES);
        self.next_id += 1;
        let voice = Box::new(Voice::new(self.next_id, consumer, stats));
        self.commands
            .push(Command::Add(voice))
            .map_err(|_| anyhow!("the audio output is not keeping up"))?;
        self.seated += 1;
        Ok((self.next_id, feed))
    }

    pub fn remove(&mut self, id: u64) {
        if self.commands.push(Command::Remove(id)).is_err() {
            tracing::warn!(id, "audio output command queue is full");
        }
    }

    pub fn collect(&mut self) {
        while let Ok(voice) = self.retired.pop() {
            drop(voice);
            self.seated = self.seated.saturating_sub(1);
        }
    }

    #[must_use]
    pub fn clicks(&self) -> &ClickState {
        &self.clicks
    }

    #[must_use]
    pub fn latency_ms(&self) -> f64 {
        f64::from(self.latency_us.load(Ordering::Relaxed)) / 1_000.0
    }

    #[must_use]
    pub fn broken(&self) -> bool {
        self.failed.load(Ordering::Relaxed) || self.moved()
    }

    fn moved(&self) -> bool {
        let now = cpal::default_host()
            .default_output_device()
            .and_then(|device| device.id().ok());
        now.is_some() && now != self.device
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if let Err(error) = self.stream.pause() {
            tracing::debug!(%error, "cannot pause the audio output");
        }
    }
}

struct Wiring {
    config: StreamConfig,
    mixer: Mixer,
    failed: Arc<AtomicBool>,
    latency_us: Arc<AtomicU32>,
}

impl Wiring {
    fn build<T>(self, device: &cpal::Device) -> anyhow::Result<Stream>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let Self {
            config,
            mut mixer,
            failed,
            latency_us,
        } = self;
        let channels = usize::from(config.channels);
        device
            .build_output_stream::<T, _, _>(
                config,
                move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                    let stamp = info.timestamp();
                    let ahead = stamp.playback.duration_since(stamp.callback);
                    latency_us.store(
                        u32::try_from(ahead.as_micros()).unwrap_or(u32::MAX),
                        Ordering::Relaxed,
                    );
                    mixer.fill(data, channels);
                },
                move |error| {
                    tracing::warn!(%error, "audio output failed");
                    failed.store(true, Ordering::Relaxed);
                },
                None,
            )
            .context("cannot open the audio output")
    }
}

fn pick_config(device: &cpal::Device) -> anyhow::Result<SupportedStreamConfig> {
    let ranked = device
        .supported_output_configs()
        .map(|configs| {
            configs
                .filter(|range| {
                    range.min_sample_rate() <= SAMPLE_RATE && SAMPLE_RATE <= range.max_sample_rate()
                })
                .max_by_key(|range| {
                    (
                        range.channels() == 2,
                        range.sample_format() == SampleFormat::F32,
                    )
                })
        })
        .ok()
        .flatten();
    match ranked {
        Some(range) => Ok(range.with_sample_rate(SAMPLE_RATE)),
        None => device
            .default_output_config()
            .context("the audio device offers no output format"),
    }
}
