//! Per-channel Opus encode (): a dedicated thread per channel turns the gated 48 kHz
//! PCM broadcast into 20 ms Opus packets on its own broadcast stream. The thread exits
//! when every PCM sender is gone (channel removed / set stopped); removal joins it.
//!
//! Channel layout travels with the PCM rather than being fixed at construction: WFM stereo is
//! a params patch, which reaches the live pipeline as a settings command and not as a rebuild,
//! so the encoder has to be able to change layout under a running stream.

use std::{sync::Arc, thread::JoinHandle};

use sdrmm_channels::AUDIO_RATE;
use tokio::sync::broadcast::{self, error::RecvError};

use crate::EngineError;

/// 20 ms at the fixed 48 kHz channel audio rate (), counted in sample frames — a
/// stereo frame holds twice as many `f32`s.
pub const OPUS_FRAME_SAMPLES: usize = 960;
const MONO_BITRATE_BPS: i32 = 64_000;
/// Stereo carries a second, largely correlated channel: half again the mono rate is what
/// libopus needs to keep the same quality once it couples them.
const STEREO_BITRATE_BPS: i32 = 96_000;
/// libopus's recommended packet buffer; any single 20 ms frame fits.
const MAX_PACKET_BYTES: usize = 4000;
/// PCM blocks arrive at drain cadence (~25 ms each); 32 buffers ~0.8 s of encoder stall
/// before the drop-oldest contract kicks in ().
pub(crate) const PCM_CHANNEL_CAP: usize = 32;
pub(crate) const AUDIO_CHANNEL_CAP: usize = 64;

/// One DSP-thread PCM hand-off, stamped with where it starts in the channel's 48 kHz frame
/// stream. The stamps are what keep packet timestamps honest: blocks dropped between DSP and
/// encoder surface as a stamp discontinuity instead of splicing seamlessly.
#[derive(Clone, Debug)]
pub(crate) struct PcmBlock {
    pub(crate) start_frame: u64,
    /// Interleave of `payload`; 1 = mono, 2 = stereo (see `sdrmm_channels::audio_channels`).
    pub(crate) channels: u8,
    pub(crate) payload: PcmPayload,
}

/// Squelched fill travels as a bare frame count — the DSP thread must not allocate for silence.
#[derive(Clone, Debug)]
pub(crate) enum PcmPayload {
    Samples(Arc<[f32]>),
    Silence(usize),
}

/// One encoded audio frame. `timestamp` is the position of the frame's first sample frame in
/// the channel's 48 kHz audio stream ( sample-count timestamps); a jump beyond the
/// frame size marks PCM lost upstream while `seq` stays a contiguous packet counter. The
/// clock is in frames, so a layout change does not disturb it.
#[derive(Clone, Debug)]
pub struct AudioPacket {
    pub seq: u32,
    pub timestamp: u64,
    /// Channel count this packet was encoded with; travels to the client in the frame header.
    pub channels: u8,
    pub opus: Arc<[u8]>,
}

/// The Opus encoder plus the layout it was built for, so a change of layout is one swap.
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

/// Build the encoder control-side so construction errors surface to the caller, then hand it
/// to a dedicated thread — Opus encode must never run on the DSP thread ().
/// `channels` is the layout the channel starts in; the thread follows it from there.
pub(crate) fn spawn_encoder(
    channels: u8,
    pcm_rx: broadcast::Receiver<PcmBlock>,
    audio_tx: broadcast::Sender<AudioPacket>,
) -> Result<JoinHandle<()>, EngineError> {
    let mut encoder = Encoder::new(channels)
        .map_err(|e| EngineError::Audio(format!("create opus encoder: {e}")))?;
    std::thread::Builder::new()
        .name("sdrmm-opus".to_string())
        .spawn(move || encode_loop(pcm_rx, &audio_tx, &mut encoder))
        .map_err(|e| EngineError::Audio(format!("spawn opus encoder thread: {e}")))
}

fn encode_loop(
    mut pcm_rx: broadcast::Receiver<PcmBlock>,
    audio_tx: &broadcast::Sender<AudioPacket>,
    encoder: &mut Encoder,
) {
    let mut pending: Vec<f32> = Vec::new();
    // Stream position of `pending[0]`, in sample frames; every packet timestamp derives from it.
    let mut pending_start: u64 = 0;
    let mut packet = [0u8; MAX_PACKET_BYTES];
    let mut seq: u32 = 0;
    // The layout an encoder rebuild last failed on, so a layout nothing can encode is
    // reported once instead of at block cadence.
    let mut rejected: Option<u8> = None;
    loop {
        match pcm_rx.blocking_recv() {
            Ok(block) => {
                let mut channels = usize::from(encoder.channels);
                // A stamp that is not where `pending` ends means PCM was dropped upstream:
                // discard the stale partial frame instead of splicing across the gap, and
                // resync — the loss then shows downstream as a timestamp jump.
                if block.start_frame != pending_start + (pending.len() / channels) as u64 {
                    pending.clear();
                    pending_start = block.start_frame;
                }
                if block.channels != encoder.channels {
                    // The pending tail is in the old interleave and cannot be spliced onto
                    // the new one; the client re-reads the layout from the frame header.
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
                            // send() only errors with no subscribers — nobody listening is fine.
                            let _ = audio_tx.send(AudioPacket {
                                seq,
                                timestamp: pending_start,
                                channels: encoder.channels,
                                opus: Arc::from(&packet[..len]),
                            });
                            seq = seq.wrapping_add(1);
                        }
                        // The frame's samples are consumed below either way, so the failure
                        // surfaces downstream as a timestamp gap, never a silent splice.
                        Err(e) => tracing::error!(error = %e, "opus encode failed; frame dropped"),
                    }
                    pending.drain(..frame_samples);
                    pending_start += OPUS_FRAME_SAMPLES as u64;
                }
            }
            // Drop-oldest is the UI-stream contract (): a stalled encoder skips PCM
            // rather than stalling the DSP thread. The next block's stamp resyncs the
            // timeline; the stale partial frame goes now.
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

    /// A cap-1 PCM channel with blocks queued before the encoder starts makes the broadcast
    /// lag deterministic: the receiver sees `Lagged`, then only the newest block. The stale
    /// partial frame from before the gap must not be spliced into the first packet, and
    /// timestamps must resync to the surviving block's stamp while seq stays contiguous.
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

        // The first packet proves the 960-block was consumed, so this send cannot lag.
        pcm_tx.send(silence(1_920, 1, 960)).unwrap();
        let second = audio_rx.blocking_recv().unwrap();
        assert_eq!(second.seq, 1);
        assert_eq!(second.timestamp, 1_920);

        drop(pcm_tx);
        encoder.join().unwrap();
    }

    /// Contiguously stamped blocks (samples and silence alike) must produce back-to-back
    /// timestamps — the resync path must never fire on an intact stream.
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

    /// A stereo channel's blocks hold two samples per frame: the packet cadence must follow
    /// the *frame* clock, not the buffer length, or every timestamp doubles.
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

    /// Toggling WFM stereo reaches a live channel as a settings command, so the layout changes
    /// under the running encoder: the packets after it must be stereo, keep the frame clock,
    /// and carry no spliced tail from the mono side of the switch.
    #[test]
    fn a_layout_change_swaps_the_encoder_mid_stream() {
        let (pcm_tx, pcm_rx) = broadcast::channel(PCM_CHANNEL_CAP);
        let (audio_tx, mut audio_rx) = broadcast::channel(8);
        let encoder = spawn_encoder(1, pcm_rx, audio_tx).unwrap();

        pcm_tx.send(samples(0, 1, 960)).unwrap();
        let mono = audio_rx.blocking_recv().unwrap();
        assert_eq!((mono.timestamp, mono.channels), (0, 1));

        // Half a frame of mono, then the switch: the partial mono tail must not be encoded
        // as the head of the first stereo packet.
        pcm_tx.send(samples(960, 1, 480)).unwrap();
        pcm_tx.send(samples(1_440, 2, 960)).unwrap();
        let stereo = audio_rx.blocking_recv().unwrap();
        assert_eq!((stereo.timestamp, stereo.channels), (1_440, 2));
        assert_eq!(stereo.seq, 1, "seq stays a contiguous packet counter");

        drop(pcm_tx);
        encoder.join().unwrap();
    }
}
