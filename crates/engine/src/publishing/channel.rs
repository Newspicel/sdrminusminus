use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_channels::{ChannelCtx, ChannelOutputs, ChannelRx};
use sdrmm_wire::ChannelSettings;

use super::Publisher;
use crate::{
    audio::{PcmBlock, PcmPayload},
    audio_recording::AudioRecorderTap,
    iq::IqTap,
    recording::RecorderTap,
    runtime::{ChannelSinks, DecodedSink},
    symbols::SymbolBatcher,
    video::VideoPacket,
};

pub(crate) const IQ_WANTED: u8 = 1;
pub(crate) const SYMBOLS_WANTED: u8 = 2;

pub(crate) struct ChannelPacket {
    pub(crate) outputs: ChannelOutputs,
    pub(crate) iq: Vec<Complex<f32>>,
    pub(crate) iq_start: u64,
    pub(crate) input_len: usize,
    pub(crate) iq_wanted: bool,
    pub(crate) symbols_wanted: bool,
    pub(crate) audio_start: u64,
    pub(crate) silence: usize,
    pub(crate) channels: u8,
    pub(crate) frequency: f64,
    pub(crate) video_position: u64,
    pub(crate) recorder: Option<AudioRecorderTap>,
    pub(crate) baseband_recorder: Option<RecorderTap>,
}

pub(crate) struct ChannelPublisher {
    pub(crate) queue: Publisher<ChannelPacket>,
    wanted: Arc<AtomicU8>,
    retired_rx: Producer<Box<dyn ChannelRx>>,
    fresh_rx: Consumer<Box<dyn ChannelRx>>,
}

impl ChannelPublisher {
    pub(crate) fn new(
        rate: f64,
        settings: &ChannelSettings,
        iq_capacity: usize,
        sinks: ChannelSinks,
        decoded: DecodedSink,
    ) -> std::io::Result<Self> {
        let settings = settings.clone();
        let make_rx = move || sdrmm_channels::create(ChannelCtx { input_rate: rate }, &settings);
        let (retired_rx, mut old_rx) = RingBuffer::<Box<dyn ChannelRx>>::new(1);
        let (mut ready_rx, fresh_rx) = RingBuffer::new(1);
        let fresh = make_rx().map_err(std::io::Error::other)?;
        if ready_rx.push(fresh).is_err() {
            return Err(std::io::Error::other("receiver pool initialization failed"));
        }
        let mut rebuild_rx = false;
        let wanted = Arc::new(AtomicU8::new(subscriptions(&sinks)));
        let poll_wanted = wanted.clone();
        let poll_sinks = sinks.clone();
        let mut iq = IqTap::new(rate);
        let mut symbols = SymbolBatcher::new();
        let mut video_seq = 0u32;
        let mut previous_end = 0;
        let metrics = sinks.publication.clone();
        let queue = Publisher::with_metrics(
            "sdrmm-publish",
            64,
            || ChannelPacket {
                outputs: ChannelOutputs {
                    audio_pcm: Vec::with_capacity(8192),
                    events: Vec::with_capacity(16),
                    video: Vec::with_capacity(2),
                    images: Vec::with_capacity(2),
                    ..Default::default()
                },
                iq: Vec::with_capacity(iq_capacity),
                iq_start: 0,
                input_len: 0,
                iq_wanted: false,
                symbols_wanted: false,
                audio_start: 0,
                silence: 0,
                channels: 1,
                frequency: 0.0,
                video_position: 0,
                recorder: None,
                baseband_recorder: None,
            },
            move |packet| {
                if packet.iq_start != previous_end {
                    iq.reset();
                    symbols.reset();
                }
                previous_end = packet.iq_start + packet.input_len as u64;
                if packet.iq_wanted {
                    iq.push_at(
                        &packet.iq,
                        packet.iq_start,
                        rate as f32,
                        packet.frequency,
                        |block| {
                            let _ = sinks.iq_tx.send(block);
                        },
                    );
                } else {
                    iq.reset();
                }
                if packet.symbols_wanted {
                    symbols.push(&packet.outputs.symbols, |block| {
                        let _ = sinks.symbol_tx.send(block);
                    });
                } else {
                    symbols.reset();
                }
                publish_audio(packet, &sinks);
                if let Some(recorder) = packet.baseband_recorder.take() {
                    let _ = recorder.push(&packet.iq, packet.iq_start, packet.frequency);
                }
                for event in packet.outputs.events.drain(..) {
                    decoded.publish(packet.frequency, event);
                }
                for image in packet.outputs.images.drain(..) {
                    decoded.publish_image(packet.frequency, image);
                }
                for picture in packet.outputs.video.drain(..) {
                    let _ = sinks.video_tx.send(VideoPacket {
                        seq: video_seq,
                        timestamp: packet.video_position,
                        picture: Arc::new(picture),
                    });
                    video_seq = video_seq.wrapping_add(1);
                }
                packet.recorder = None;
                packet.outputs.reset();
                packet.iq.clear();
            },
            move || {
                poll_wanted.store(subscriptions(&poll_sinks), Ordering::Relaxed);
                if let Ok(old) = old_rx.pop() {
                    drop(old);
                    rebuild_rx = true;
                }
                if rebuild_rx {
                    match make_rx() {
                        Ok(rx) => {
                            rebuild_rx = ready_rx.push(rx).is_err();
                        }
                        Err(error) => {
                            tracing::error!(%error, "receiver recovery preparation failed")
                        }
                    }
                }
            },
            metrics,
        )?;
        Ok(Self {
            queue,
            wanted,
            retired_rx,
            fresh_rx,
        })
    }

    pub(crate) fn recover(&mut self, receiver: &mut Box<dyn ChannelRx>) -> bool {
        if self.retired_rx.slots() == 0 {
            return false;
        }
        let Ok(mut fresh) = self.fresh_rx.pop() else {
            return false;
        };
        std::mem::swap(receiver, &mut fresh);
        let result = self.retired_rx.push(fresh);
        debug_assert!(result.is_ok());
        true
    }

    pub(crate) fn wanted(&self) -> u8 {
        self.wanted.load(Ordering::Relaxed)
    }
}

fn subscriptions(sinks: &ChannelSinks) -> u8 {
    (u8::from(sinks.iq_tx.receiver_count() > 0) * IQ_WANTED)
        | (u8::from(sinks.symbol_tx.receiver_count() > 0) * SYMBOLS_WANTED)
}

fn publish_audio(packet: &ChannelPacket, sinks: &ChannelSinks) {
    let payload = if !packet.outputs.audio_pcm.is_empty() {
        PcmPayload::Samples(Arc::from(packet.outputs.audio_pcm.as_slice()))
    } else if packet.silence > 0 {
        PcmPayload::Silence(packet.silence)
    } else {
        return;
    };
    let block = PcmBlock {
        start_frame: packet.audio_start,
        channels: packet.channels,
        payload,
    };
    if let Some(recorder) = &packet.recorder {
        let _ = recorder.push(block.clone());
    }
    let _ = sinks.pcm_tx.send(block);
}
