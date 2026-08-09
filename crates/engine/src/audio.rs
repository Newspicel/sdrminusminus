//! Per-channel Opus encode (PLAN §9): a dedicated thread per channel turns the gated 48 kHz
//! mono PCM broadcast into 20 ms Opus packets on its own broadcast stream. The thread exits
//! when every PCM sender is gone (channel removed / set stopped); removal joins it.

use std::{sync::Arc, thread::JoinHandle};

use sdrmm_channels::AUDIO_RATE;
use tokio::sync::broadcast::{self, error::RecvError};

use crate::EngineError;

/// 20 ms at the fixed 48 kHz channel audio rate (PLAN §9).
pub const OPUS_FRAME_SAMPLES: usize = 960;
const OPUS_BITRATE_BPS: i32 = 64_000;
/// libopus's recommended packet buffer; any single 20 ms frame fits.
const MAX_PACKET_BYTES: usize = 4000;
/// PCM blocks arrive at drain cadence (~25 ms each); 32 buffers ~0.8 s of encoder stall
/// before the drop-oldest contract kicks in (PLAN §5).
pub(crate) const PCM_CHANNEL_CAP: usize = 32;
pub(crate) const AUDIO_CHANNEL_CAP: usize = 64;

/// One DSP-thread PCM hand-off, stamped with where it starts in the channel's 48 kHz sample
/// stream. The stamps are what keep packet timestamps honest: blocks dropped between DSP and
/// encoder surface as a stamp discontinuity instead of splicing seamlessly.
#[derive(Clone, Debug)]
pub(crate) struct PcmBlock {
    pub(crate) start_sample: u64,
    pub(crate) payload: PcmPayload,
}

/// Squelched fill travels as a bare length — the DSP thread must not allocate for silence.
#[derive(Clone, Debug)]
pub(crate) enum PcmPayload {
    Samples(Arc<[f32]>),
    Silence(usize),
}

/// One encoded audio frame. `timestamp` is the position of the frame's first sample in the
/// channel's 48 kHz audio stream (PLAN §5 sample-count timestamps); a jump beyond the frame
/// size marks PCM lost upstream while `seq` stays a contiguous packet counter.
#[derive(Clone, Debug)]
pub struct AudioPacket {
    pub seq: u32,
    pub timestamp: u64,
    pub opus: Arc<[u8]>,
}

/// Build the encoder control-side so construction errors surface to the caller, then hand it
/// to a dedicated thread — Opus encode must never run on the DSP thread (PLAN §7).
pub(crate) fn spawn_encoder(
    pcm_rx: broadcast::Receiver<PcmBlock>,
    audio_tx: broadcast::Sender<AudioPacket>,
) -> Result<JoinHandle<()>, EngineError> {
    let mut encoder =
        opus::Encoder::new(AUDIO_RATE, opus::Channels::Mono, opus::Application::Audio)
            .map_err(|e| EngineError::Audio(format!("create opus encoder: {e}")))?;
    encoder
        .set_bitrate(opus::Bitrate::Bits(OPUS_BITRATE_BPS))
        .map_err(|e| EngineError::Audio(format!("set opus bitrate: {e}")))?;
    std::thread::Builder::new()
        .name("sdrmm-opus".to_string())
        .spawn(move || encode_loop(pcm_rx, &audio_tx, &mut encoder))
        .map_err(|e| EngineError::Audio(format!("spawn opus encoder thread: {e}")))
}

fn encode_loop(
    mut pcm_rx: broadcast::Receiver<PcmBlock>,
    audio_tx: &broadcast::Sender<AudioPacket>,
    encoder: &mut opus::Encoder,
) {
    let mut pending: Vec<f32> = Vec::new();
    // Stream position of `pending[0]`; every packet timestamp derives from it.
    let mut pending_start: u64 = 0;
    let mut packet = [0u8; MAX_PACKET_BYTES];
    let mut seq: u32 = 0;
    loop {
        match pcm_rx.blocking_recv() {
            Ok(block) => {
                // A stamp that is not where `pending` ends means PCM was dropped upstream:
                // discard the stale partial frame instead of splicing across the gap, and
                // resync — the loss then shows downstream as a timestamp jump.
                if block.start_sample != pending_start + pending.len() as u64 {
                    pending.clear();
                    pending_start = block.start_sample;
                }
                match block.payload {
                    PcmPayload::Samples(samples) => pending.extend_from_slice(&samples),
                    PcmPayload::Silence(n) => pending.resize(pending.len() + n, 0.0),
                }
                while pending.len() >= OPUS_FRAME_SAMPLES {
                    match encoder.encode_float(&pending[..OPUS_FRAME_SAMPLES], &mut packet) {
                        Ok(len) => {
                            // send() only errors with no subscribers — nobody listening is fine.
                            let _ = audio_tx.send(AudioPacket {
                                seq,
                                timestamp: pending_start,
                                opus: Arc::from(&packet[..len]),
                            });
                            seq = seq.wrapping_add(1);
                        }
                        // The frame's samples are consumed below either way, so the failure
                        // surfaces downstream as a timestamp gap, never a silent splice.
                        Err(e) => tracing::error!(error = %e, "opus encode failed; frame dropped"),
                    }
                    pending.drain(..OPUS_FRAME_SAMPLES);
                    pending_start += OPUS_FRAME_SAMPLES as u64;
                }
            }
            // Drop-oldest is the UI-stream contract (PLAN §5): a stalled encoder skips PCM
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

    /// A cap-1 PCM channel with blocks queued before the encoder starts makes the broadcast
    /// lag deterministic: the receiver sees `Lagged`, then only the newest block. The stale
    /// partial frame from before the gap must not be spliced into the first packet, and
    /// timestamps must resync to the surviving block's stamp while seq stays contiguous.
    #[test]
    fn lag_drops_stale_pending_and_resyncs_timestamps() {
        let (pcm_tx, pcm_rx) = broadcast::channel(1);
        let (audio_tx, mut audio_rx) = broadcast::channel(8);
        for (start, n) in [(0u64, 480usize), (480, 480), (960, 960)] {
            pcm_tx
                .send(PcmBlock {
                    start_sample: start,
                    payload: PcmPayload::Samples(vec![0.25; n].into()),
                })
                .unwrap();
        }
        let encoder = spawn_encoder(pcm_rx, audio_tx).unwrap();

        let first = audio_rx.blocking_recv().unwrap();
        assert_eq!(first.seq, 0);
        assert_eq!(
            first.timestamp, 960,
            "first packet must start at the post-gap stamp, not splice the stale 480 samples"
        );

        // The first packet proves the 960-block was consumed, so this send cannot lag.
        pcm_tx
            .send(PcmBlock {
                start_sample: 1_920,
                payload: PcmPayload::Silence(960),
            })
            .unwrap();
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
        pcm_tx
            .send(PcmBlock {
                start_sample: 0,
                payload: PcmPayload::Samples(vec![0.1; 1_440].into()),
            })
            .unwrap();
        pcm_tx
            .send(PcmBlock {
                start_sample: 1_440,
                payload: PcmPayload::Silence(480),
            })
            .unwrap();
        let encoder = spawn_encoder(pcm_rx, audio_tx).unwrap();
        drop(pcm_tx);

        let first = audio_rx.blocking_recv().unwrap();
        let second = audio_rx.blocking_recv().unwrap();
        assert_eq!((first.seq, first.timestamp), (0, 0));
        assert_eq!((second.seq, second.timestamp), (1, 960));
        encoder.join().unwrap();
    }
}
