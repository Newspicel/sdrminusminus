use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_device::{
    Block, CaptureStream, Next, Sample, SampleConverter, StreamFailure,
    net::{Incoming, SocketStop, WebSocket},
};

use crate::{
    proto::{ADC_OVERLOAD, Command, Field, fields, snd},
    session,
};

const SCALE: f32 = 1.0 / 32_768.0;
const KEEPALIVE: Duration = Duration::from_secs(5);
const NO_SEQ: u64 = u64::MAX;

#[derive(Debug, Default)]
pub(crate) struct IqConverter {
    out: Vec<Sample>,
}

impl SampleConverter for IqConverter {
    fn convert(&mut self, bytes: &[u8]) -> &[Sample] {
        self.out.clear();
        let Some(frame) = snd(bytes) else {
            return &self.out;
        };
        let read = if frame.little_endian() {
            i16::from_le_bytes
        } else {
            i16::from_be_bytes
        };
        let (pairs, _) = frame.iq.as_chunks::<4>();
        self.out.extend(pairs.iter().map(|pair| {
            Complex::new(
                f32::from(read([pair[0], pair[1]])) * SCALE,
                f32::from(read([pair[2], pair[3]])) * SCALE,
            )
        }));
        &self.out
    }

    fn reset(&mut self) {}
}

fn missing_frames(last: u64, seq: u32) -> u64 {
    let Ok(last) = u32::try_from(last) else {
        return 0;
    };
    match seq.wrapping_sub(last) {
        step if step > u32::MAX / 2 => 0,
        step => u64::from(step.saturating_sub(1)),
    }
}

#[derive(Debug)]
pub(crate) struct KiwiStream {
    socket: Arc<WebSocket>,
    epoch: Instant,
    keepalive_ms: AtomicU64,
    last_seq: AtomicU64,
    overloaded: AtomicBool,
    ended: OnceLock<StreamFailure>,
}

impl KiwiStream {
    pub(crate) fn new(socket: Arc<WebSocket>) -> Self {
        Self {
            socket,
            epoch: Instant::now(),
            keepalive_ms: AtomicU64::new(0),
            last_seq: AtomicU64::new(NO_SEQ),
            overloaded: AtomicBool::new(false),
            ended: OnceLock::new(),
        }
    }

    fn end(&self, reason: String) -> Next<Block> {
        tracing::warn!("{reason}");
        let _ = self.ended.set(StreamFailure { reason, gone: true });
        Next::Ended
    }

    fn keep_alive(&self) -> bool {
        let now = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        let last = self.keepalive_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) < KEEPALIVE.as_millis() as u64 {
            return true;
        }
        self.keepalive_ms.store(now, Ordering::Relaxed);
        match session::send(&self.socket, &[Command::Keepalive]) {
            Ok(()) => true,
            Err(e) => {
                self.socket.fail(format!("the KiwiSDR keepalive: {e}"));
                false
            }
        }
    }

    fn note_overload(&self, flags: u8) {
        let overloaded = flags & ADC_OVERLOAD != 0;
        if self.overloaded.swap(overloaded, Ordering::Relaxed) != overloaded {
            if overloaded {
                tracing::warn!("the KiwiSDR ADC is overloading");
            } else {
                tracing::info!("the KiwiSDR ADC is no longer overloading");
            }
        }
    }

    fn samples(&self, block: Block) -> Next<Block> {
        let Some(frame) = snd(&block) else {
            return self.message(&block);
        };
        if !frame.is_iq() {
            return self.end(format!(
                "the KiwiSDR sent a non-IQ frame (flags {:#04x})",
                frame.flags
            ));
        }
        self.note_overload(frame.flags);
        Next::Block(block)
    }

    fn message(&self, frame: &[u8]) -> Next<Block> {
        for field in fields(frame) {
            if let Field::Refused(refusal) = field {
                return self.end(refusal.reason());
            }
        }
        Next::Idle
    }
}

impl CaptureStream for KiwiStream {
    type Block = Block;
    type Stop = SocketStop;

    fn stop_handle(&self) -> SocketStop {
        self.socket.stop_handle()
    }

    fn next_block(&self, timeout: Duration) -> Next<Block> {
        if self.ended.get().is_some() || !self.keep_alive() {
            return Next::Ended;
        }
        match self.socket.next(timeout) {
            Incoming::Binary(block) => self.samples(block),
            Incoming::Text(text) => self.message(text.as_bytes()),
            Incoming::Idle => Next::Idle,
            Incoming::Ended => Next::Ended,
        }
    }

    fn dropped(&self) -> u64 {
        0
    }

    fn block_gap(&self, block: &Block) -> Option<u64> {
        let frame = snd(block)?;
        let last = self.last_seq.swap(u64::from(frame.seq), Ordering::Relaxed);
        Some(missing_frames(last, frame.seq) * frame.pairs() as u64)
    }

    fn failure(&self) -> StreamFailure {
        self.ended
            .get()
            .cloned()
            .unwrap_or_else(|| self.socket.failure())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(flags: u8, samples: &[i16]) -> Vec<u8> {
        let mut out = b"SND".to_vec();
        out.push(flags);
        out.extend_from_slice(&[0; 16]);
        for sample in samples {
            out.extend_from_slice(&if flags & 0x80 == 0 {
                sample.to_be_bytes()
            } else {
                sample.to_le_bytes()
            });
        }
        out
    }

    #[test]
    fn big_endian_pairs_become_scaled_samples() {
        let mut converter = IqConverter::default();
        let got = converter
            .convert(&frame(0x08, &[16_384, -16_384, 32_767, 0]))
            .to_vec();
        assert_eq!(got.len(), 2);
        assert!((got[0].re - 0.5).abs() < 1e-6);
        assert!((got[0].im + 0.5).abs() < 1e-6);
        assert!((got[1].re - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_little_endian_flag_flips_byte_order() {
        let mut converter = IqConverter::default();
        let got = converter.convert(&frame(0x88, &[16_384, -16_384])).to_vec();
        assert!((got[0].re - 0.5).abs() < 1e-6);
        assert!((got[0].im + 0.5).abs() < 1e-6);
    }

    #[test]
    fn partial_pairs_and_foreign_frames_yield_nothing_extra() {
        let mut converter = IqConverter::default();
        assert_eq!(converter.convert(&frame(0x08, &[1, 2, 3])).len(), 1);
        assert!(converter.convert(b"MSG audio_rate=12000").is_empty());
    }

    #[test]
    fn the_output_buffer_is_reused() {
        let mut converter = IqConverter::default();
        let block = frame(0x08, &[0; 1024]);
        let first = converter.convert(&block).as_ptr();
        assert_eq!(converter.convert(&block).as_ptr(), first);
    }

    #[test]
    fn sequence_gaps_count_the_frames_between() {
        assert_eq!(missing_frames(NO_SEQ, 9), 0);
        assert_eq!(missing_frames(1, 2), 0);
        assert_eq!(missing_frames(1, 5), 3);
        assert_eq!(missing_frames(u64::from(u32::MAX), 1), 1);
        assert_eq!(missing_frames(5, 4), 0);
    }
}
