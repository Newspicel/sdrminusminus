use std::collections::VecDeque;

use crate::{AUDIO_RATE, ChannelOutputs};

const CHANNELS: usize = 2;
const TARGET_FRAMES: usize = AUDIO_RATE as usize * 3 / 10;
const CAPACITY_FRAMES: usize = 2 * AUDIO_RATE as usize;
const MAX_TRIM: f64 = 0.002;

pub struct Pacer {
    frames_per_input: f64,
    fifo: VecDeque<f32>,
    carry: f64,
    playing: bool,
    pub dropped_frames: u32,
}

impl Pacer {
    pub fn new(input_rate: f64) -> Self {
        Self {
            frames_per_input: f64::from(AUDIO_RATE) / input_rate,
            fifo: VecDeque::with_capacity(CAPACITY_FRAMES * CHANNELS),
            carry: 0.0,
            playing: false,
            dropped_frames: 0,
        }
    }

    pub fn reset(&mut self) {
        self.fifo.clear();
        self.carry = 0.0;
        self.playing = false;
        self.dropped_frames = 0;
    }

    fn buffered(&self) -> usize {
        self.fifo.len() / CHANNELS
    }

    pub fn take(&mut self, out: &mut ChannelOutputs, from: usize) {
        let Some(fresh) = out.audio_pcm.get(from..) else {
            return;
        };
        let queued = self.buffered();
        let excess = (queued + fresh.len() / CHANNELS).saturating_sub(CAPACITY_FRAMES);
        let from_queue = excess.min(queued);
        self.fifo.drain(..from_queue * CHANNELS);
        self.fifo.extend(&fresh[(excess - from_queue) * CHANNELS..]);
        self.dropped_frames = self.dropped_frames.saturating_add(excess as u32);
        out.audio_pcm.truncate(from);
    }

    pub fn release(&mut self, input_samples: usize, out: &mut ChannelOutputs) {
        if !self.playing {
            if self.buffered() < TARGET_FRAMES {
                return;
            }
            self.playing = true;
            self.carry = 0.0;
        }
        let level = (self.buffered() as f64 - TARGET_FRAMES as f64) / TARGET_FRAMES as f64;
        let trim = 1.0 + level.clamp(-1.0, 1.0) * MAX_TRIM;
        self.carry += input_samples as f64 * self.frames_per_input * trim;
        let due = self.carry as usize;
        self.carry -= due as f64;
        let ready = due.min(self.buffered());
        if ready > 0 {
            out.audio_rate = AUDIO_RATE;
            out.audio_pcm.extend(self.fifo.drain(..ready * CHANNELS));
        }
        self.playing = ready == due;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT_RATE: f64 = 2_048_000.0;
    const BLOCK: usize = 16_384;
    const BLOCK_FRAMES: usize = BLOCK * AUDIO_RATE as usize / INPUT_RATE as usize;
    const SUPERFRAME: usize = 5_760;

    fn offer(pacer: &mut Pacer, frames: usize) {
        let mut out = ChannelOutputs::default();
        out.audio_pcm.resize(frames * CHANNELS, 0.5);
        pacer.take(&mut out, 0);
        assert!(out.audio_pcm.is_empty());
    }

    fn released(pacer: &mut Pacer) -> usize {
        let mut out = ChannelOutputs::default();
        pacer.release(BLOCK, &mut out);
        assert_eq!(out.audio_pcm.len() % CHANNELS, 0);
        out.audio_pcm.len() / CHANNELS
    }

    #[test]
    fn superframe_bursts_leave_as_a_steady_stream() {
        let mut pacer = Pacer::new(INPUT_RATE);
        let mut sent = Vec::new();
        for block in 0..2_000 {
            if block % 12 == 0 {
                let frame = block / 12;
                for _ in 4 * frame / 5..4 * (frame + 1) / 5 {
                    offer(&mut pacer, SUPERFRAME);
                }
            }
            sent.push(released(&mut pacer));
        }
        let playing = sent
            .iter()
            .position(|&frames| frames > 0)
            .expect("playback");
        for frames in &sent[playing..] {
            assert!(frames.abs_diff(BLOCK_FRAMES) <= 1, "{frames}");
        }
        assert_eq!(pacer.dropped_frames, 0);
    }

    #[test]
    fn nothing_leaves_until_the_target_is_buffered() {
        let mut pacer = Pacer::new(INPUT_RATE);
        offer(&mut pacer, TARGET_FRAMES - 1);
        assert_eq!(released(&mut pacer), 0);
        offer(&mut pacer, 1);
        assert!(released(&mut pacer) > 0);
    }

    #[test]
    fn a_starved_stream_buffers_again_before_resuming() {
        let mut pacer = Pacer::new(INPUT_RATE);
        offer(&mut pacer, TARGET_FRAMES);
        released(&mut pacer);
        while pacer.playing {
            released(&mut pacer);
        }
        offer(&mut pacer, BLOCK_FRAMES);
        assert_eq!(released(&mut pacer), 0);
    }

    #[test]
    fn overflow_drops_the_oldest_audio_and_counts_it() {
        let mut pacer = Pacer::new(INPUT_RATE);
        offer(&mut pacer, CAPACITY_FRAMES);
        let mut out = ChannelOutputs::default();
        out.audio_pcm.extend_from_slice(&[0.25; 20]);
        pacer.take(&mut out, 0);
        assert_eq!(pacer.dropped_frames, 10);
        assert_eq!(pacer.buffered(), CAPACITY_FRAMES);
        assert_eq!(pacer.fifo.back(), Some(&0.25));
        pacer.reset();
        assert_eq!(pacer.dropped_frames, 0);
    }

    #[test]
    fn a_full_buffer_drains_slightly_faster_than_real_time() {
        let mut pacer = Pacer::new(INPUT_RATE);
        offer(&mut pacer, 2 * TARGET_FRAMES);
        let sent: usize = (0..50).map(|_| released(&mut pacer)).sum();
        assert!(sent > 50 * BLOCK_FRAMES);
    }
}
