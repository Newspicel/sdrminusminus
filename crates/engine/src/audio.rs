use std::{sync::Arc, thread::JoinHandle};

use sdrmm_channels::AUDIO_RATE;
use tokio::sync::broadcast::{self, error::RecvError};

use crate::EngineError;

pub const OPUS_FRAME_SAMPLES: usize = 960;
const MONO_BITRATE_BPS: i32 = 64_000;
const STEREO_BITRATE_BPS: i32 = 96_000;
const MAX_PACKET_BYTES: usize = 4000;
pub(crate) const PCM_CHANNEL_CAP: usize = 32;
pub(crate) const AUDIO_CHANNEL_CAP: usize = 64;

#[derive(Clone, Debug)]
pub struct PcmBlock {
    pub start_frame: u64,
    pub channels: u8,
    pub payload: PcmPayload,
}

#[derive(Clone, Debug)]
pub enum PcmPayload {
    Samples(Arc<[f32]>),
    Silence(usize),
}

#[derive(Clone, Debug)]
pub struct AudioPacket {
    pub seq: u32,
    pub timestamp: u64,
    pub channels: u8,
    pub opus: Arc<[u8]>,
}

struct Encoder {
    opus: opus::Encoder,
    channels: u8,
}

impl Encoder {
    fn new(channels: u8) -> Result<Self, opus::Error> {
        let (layout, bitrate) = match channels {
            2 => (opus::Channels::Stereo, STEREO_BITRATE_BPS),
            _ => (opus::Channels::Mono, MONO_BITRATE_BPS),
        };
        let mut opus = opus::Encoder::new(AUDIO_RATE, layout, opus::Application::Audio)?;
        opus.set_bitrate(opus::Bitrate::Bits(bitrate))?;
        Ok(Self { opus, channels })
    }
}

pub(crate) fn spawn_encoder(
    channels: u8,
    pcm_rx: broadcast::Receiver<PcmBlock>,
    audio_tx: broadcast::Sender<AudioPacket>,
) -> Result<JoinHandle<()>, EngineError> {
    let mut encoder = Encoder::new(channels)
        .map_err(|e| EngineError::Audio(format!("create opus encoder: {e}")))?;
    std::thread::Builder::new()
        .name("sdrmm-opus".to_string())
        .spawn(move || {
            sdrmm_device::schedule::claim(sdrmm_device::Latency::Interactive);
            encode_loop(pcm_rx, &audio_tx, &mut encoder);
        })
        .map_err(|e| EngineError::Audio(format!("spawn opus encoder thread: {e}")))
}

fn encode_loop(
    mut pcm_rx: broadcast::Receiver<PcmBlock>,
    audio_tx: &broadcast::Sender<AudioPacket>,
    encoder: &mut Encoder,
) {
    let mut pending: Vec<f32> = Vec::new();
    let mut pending_start: u64 = 0;
    let mut packet = [0u8; MAX_PACKET_BYTES];
    let mut seq: u32 = 0;
    let mut rejected: Option<u8> = None;
    loop {
        match pcm_rx.blocking_recv() {
            Ok(block) => {
                let mut channels = usize::from(encoder.channels);
                if block.start_frame != pending_start + (pending.len() / channels) as u64 {
                    pending.clear();
                    pending_start = block.start_frame;
                }
                if block.channels != encoder.channels {
                    pending.clear();
                    pending_start = block.start_frame;
                    match Encoder::new(block.channels) {
                        Ok(fresh) => {
                            *encoder = fresh;
                            channels = usize::from(encoder.channels);
                            rejected = None;
                        }
                        Err(e) => {
                            if rejected.replace(block.channels) != Some(block.channels) {
                                tracing::error!(
                                    error = %e,
                                    channels = block.channels,
                                    "opus encoder rebuild failed; audio dropped until the layout changes"
                                );
                            }
                            continue;
                        }
                    }
                }
                let frame_samples = OPUS_FRAME_SAMPLES * channels;
                match block.payload {
                    PcmPayload::Samples(samples) => pending.extend_from_slice(&samples),
                    PcmPayload::Silence(frames) => {
                        pending.resize(pending.len() + frames * channels, 0.0);
                    }
                }
                while pending.len() >= frame_samples {
                    match encoder
                        .opus
                        .encode_float(&pending[..frame_samples], &mut packet)
                    {
                        Ok(len) => {
                            let _ = audio_tx.send(AudioPacket {
                                seq,
                                timestamp: pending_start,
                                channels: encoder.channels,
                                opus: Arc::from(&packet[..len]),
                            });
                            seq = seq.wrapping_add(1);
                        }
                        Err(e) => tracing::error!(error = %e, "opus encode failed; frame dropped"),
                    }
                    pending.drain(..frame_samples);
                    pending_start += OPUS_FRAME_SAMPLES as u64;
                }
            }
            Err(RecvError::Lagged(skipped)) => {
                pending.clear();
                tracing::debug!(skipped, "audio encoder lagged; oldest pcm dropped");
            }
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(start_frame: u64, channels: u8, frames: usize) -> PcmBlock {
        PcmBlock {
            start_frame,
            channels,
            payload: PcmPayload::Samples(vec![0.25; frames * usize::from(channels)].into()),
        }
    }

    fn silence(start_frame: u64, channels: u8, frames: usize) -> PcmBlock {
        PcmBlock {
            start_frame,
            channels,
            payload: PcmPayload::Silence(frames),
        }
    }

    #[test]
    fn lag_drops_stale_pending_and_resyncs_timestamps() {
        let (pcm_tx, pcm_rx) = broadcast::channel(1);
        let (audio_tx, mut audio_rx) = broadcast::channel(8);
        for (start, frames) in [(0u64, 480usize), (480, 480), (960, 960)] {
            pcm_tx.send(samples(start, 1, frames)).unwrap();
        }
        let encoder = spawn_encoder(1, pcm_rx, audio_tx).unwrap();

        let first = audio_rx.blocking_recv().unwrap();
        assert_eq!(first.seq, 0);
        assert_eq!(
            first.timestamp, 960,
            "first packet must start at the post-gap stamp, not splice the stale 480 samples"
        );

        pcm_tx.send(silence(1_920, 1, 960)).unwrap();
        let second = audio_rx.blocking_recv().unwrap();
        assert_eq!(second.seq, 1);
        assert_eq!(second.timestamp, 1_920);

        drop(pcm_tx);
        encoder.join().unwrap();
    }

    #[test]
    fn contiguous_stamps_yield_contiguous_timestamps() {
        let (pcm_tx, pcm_rx) = broadcast::channel(PCM_CHANNEL_CAP);
        let (audio_tx, mut audio_rx) = broadcast::channel(8);
        pcm_tx.send(samples(0, 1, 1_440)).unwrap();
        pcm_tx.send(silence(1_440, 1, 480)).unwrap();
        let encoder = spawn_encoder(1, pcm_rx, audio_tx).unwrap();
        drop(pcm_tx);

        let first = audio_rx.blocking_recv().unwrap();
        let second = audio_rx.blocking_recv().unwrap();
        assert_eq!((first.seq, first.timestamp), (0, 0));
        assert_eq!((second.seq, second.timestamp), (1, 960));
        encoder.join().unwrap();
    }

    #[test]
    fn stereo_blocks_are_timestamped_in_frames() {
        let (pcm_tx, pcm_rx) = broadcast::channel(PCM_CHANNEL_CAP);
        let (audio_tx, mut audio_rx) = broadcast::channel(8);
        pcm_tx.send(samples(0, 2, 1_440)).unwrap();
        pcm_tx.send(silence(1_440, 2, 480)).unwrap();
        let encoder = spawn_encoder(2, pcm_rx, audio_tx).unwrap();
        drop(pcm_tx);

        let first = audio_rx.blocking_recv().unwrap();
        let second = audio_rx.blocking_recv().unwrap();
        assert_eq!((first.seq, first.timestamp, first.channels), (0, 0, 2));
        assert_eq!((second.seq, second.timestamp, second.channels), (1, 960, 2));
        encoder.join().unwrap();
    }

    #[test]
    fn a_layout_change_swaps_the_encoder_mid_stream() {
        let (pcm_tx, pcm_rx) = broadcast::channel(PCM_CHANNEL_CAP);
        let (audio_tx, mut audio_rx) = broadcast::channel(8);
        let encoder = spawn_encoder(1, pcm_rx, audio_tx).unwrap();

        pcm_tx.send(samples(0, 1, 960)).unwrap();
        let mono = audio_rx.blocking_recv().unwrap();
        assert_eq!((mono.timestamp, mono.channels), (0, 1));

        pcm_tx.send(samples(960, 1, 480)).unwrap();
        pcm_tx.send(samples(1_440, 2, 960)).unwrap();
        let stereo = audio_rx.blocking_recv().unwrap();
        assert_eq!((stereo.timestamp, stereo.channels), (1_440, 2));
        assert_eq!(stereo.seq, 1, "seq stays a contiguous packet counter");

        drop(pcm_tx);
        encoder.join().unwrap();
    }
}
