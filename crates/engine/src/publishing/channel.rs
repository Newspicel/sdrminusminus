use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_channels::{AUDIO_RATE, ChannelCtx, ChannelOutputs, ChannelRx};
use sdrmm_wire::ChannelSettings;

use super::Publisher;
use crate::{
    audio::{PcmBlock, PcmPayload},
    audio_recording::AudioRecorderTap,
    iq::IqTap,
    recording::RecorderTap,
    runtime::{ChannelSinks, DSP_BLOCK, DecodedSink},
    symbols::SymbolBatcher,
    video::VideoPacket,
};

pub(crate) const IQ_WANTED: u8 = 1;
pub(crate) const SYMBOLS_WANTED: u8 = 2;

const PUBLICATION_SLACK_S: f64 = 0.25;
const MIN_PUBLICATION_DEPTH: usize = 64;
const PUBLICATION_BUDGET_BYTES: usize = 8 << 20;
const BLOCK_SLACK: usize = 64;
const OVERFLOW_REPORT_EVERY: Duration = Duration::from_secs(1);

fn iq_capacity(input_rate: f64, device_rate: f64) -> usize {
    (DSP_BLOCK as f64 * input_rate / device_rate.max(1.0)).ceil() as usize + BLOCK_SLACK
}

fn audio_capacity(device_rate: f64, channels: u8) -> usize {
    let frames = (DSP_BLOCK as f64 * f64::from(AUDIO_RATE) / device_rate.max(1.0)).ceil() as usize;
    (frames + BLOCK_SLACK) * usize::from(channels.max(1))
}

fn publication_depth(device_rate: f64, packet_bytes: usize) -> usize {
    let blocks_per_second = device_rate.max(1.0) / DSP_BLOCK as f64;
    let slack = (blocks_per_second * PUBLICATION_SLACK_S).ceil() as usize;
    let affordable = (PUBLICATION_BUDGET_BYTES / packet_bytes.max(1)).max(MIN_PUBLICATION_DEPTH);
    slack.clamp(MIN_PUBLICATION_DEPTH, affordable)
}

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
        device_rate: f64,
        settings: &ChannelSettings,
        sinks: ChannelSinks,
        decoded: DecodedSink,
    ) -> std::io::Result<Self> {
        let iq_capacity = iq_capacity(rate, device_rate);
        let pcm_capacity = audio_capacity(
            device_rate,
            sdrmm_channels::audio_channels(&settings.params),
        );
        let depth = publication_depth(
            device_rate,
            iq_capacity * size_of::<Complex<f32>>() + pcm_capacity * size_of::<f32>(),
        );
        let device_set = decoded.device_set();
        let channel = decoded.channel();
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
        let overflow = metrics.clone();
        let mut overflow_seen = 0u64;
        let mut overflow_at = Instant::now();
        let queue = Publisher::with_metrics(
            "sdrmm-publish",
            depth,
            || ChannelPacket {
                outputs: ChannelOutputs {
                    audio_pcm: Vec::with_capacity(pcm_capacity),
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
                let now = overflow.dropped_total();
                if now != overflow_seen && overflow_at.elapsed() >= OVERFLOW_REPORT_EVERY {
                    overflow_at = Instant::now();
                    tracing::warn!(
                        dropped = now - overflow_seen,
                        total = now,
                        device_set,
                        channel,
                        "channel publication queue overflow: the publisher is behind"
                    );
                    overflow_seen = now;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn slack_ms(device_rate: f64, depth: usize) -> f64 {
        depth as f64 * DSP_BLOCK as f64 / device_rate * 1000.0
    }

    fn depth_for(device_rate: f64, input_rate: f64, channels: u8) -> usize {
        let iq = iq_capacity(input_rate, device_rate);
        let pcm = audio_capacity(device_rate, channels);
        publication_depth(
            device_rate,
            iq * size_of::<Complex<f32>>() + pcm * size_of::<f32>(),
        )
    }

    #[test]
    fn a_fast_radio_buys_the_same_stall_slack_as_a_slow_one() {
        for (device_rate, input_rate) in [
            (10_000_000.0, 400_000.0),
            (2_400_000.0, 48_000.0),
            (960_000.0, 48_000.0),
        ] {
            let depth = depth_for(device_rate, input_rate, 2);
            assert!(
                slack_ms(device_rate, depth) >= 200.0,
                "{device_rate} Hz holds only {:.1} ms",
                slack_ms(device_rate, depth)
            );
        }
    }

    #[test]
    fn a_slow_radio_keeps_the_floor_rather_than_a_shallower_queue() {
        let depth = depth_for(250_000.0, 48_000.0, 1);
        assert_eq!(depth, MIN_PUBLICATION_DEPTH);
        assert!(slack_ms(250_000.0, depth) > 200.0);
    }

    #[test]
    fn the_pool_stays_inside_its_memory_budget() {
        for (device_rate, input_rate, channels) in [
            (10_000_000.0, 10_000_000.0, 2u8),
            (10_000_000.0, 400_000.0, 2),
            (2_048_000.0, 2_048_000.0, 2),
            (250_000.0, 48_000.0, 1),
        ] {
            let iq = iq_capacity(input_rate, device_rate);
            let pcm = audio_capacity(device_rate, channels);
            let packet_bytes = iq * size_of::<Complex<f32>>() + pcm * size_of::<f32>();
            let bytes = depth_for(device_rate, input_rate, channels) * packet_bytes;
            assert!(
                bytes <= PUBLICATION_BUDGET_BYTES.max(MIN_PUBLICATION_DEPTH * packet_bytes),
                "{device_rate} Hz pool is {bytes} bytes"
            );
        }
    }

    #[test]
    fn a_block_of_audio_fits_the_packet_it_is_swapped_into() {
        for (device_rate, channels) in [(10_000_000.0, 2u8), (2_400_000.0, 2), (250_000.0, 1)] {
            let produced = (DSP_BLOCK as f64 * f64::from(AUDIO_RATE) / device_rate).ceil() as usize
                * usize::from(channels);
            assert!(audio_capacity(device_rate, channels) >= produced);
        }
    }

    #[test]
    fn nonsense_rates_do_not_ask_for_an_impossible_pool() {
        assert_eq!(publication_depth(0.0, 1024), MIN_PUBLICATION_DEPTH);
        assert!(iq_capacity(48_000.0, 0.0) > 0);
        assert!(audio_capacity(0.0, 0) > 0);
    }
}
