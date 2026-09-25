use std::sync::Arc;

use anyhow::anyhow;
use rtrb::Producer;

use super::{
    CHANNELS, MAX_GAP_FRAMES, SAMPLE_RATE,
    loss::{LossAction, LossTracker},
    mixer::VoiceStats,
};

const MAX_PACKET_FRAMES: usize = 5_760;
const VOLUME_RANGE_DB: f32 = 60.0;

#[must_use]
pub fn gain_for_volume(volume: f32) -> f32 {
    let position = volume.clamp(0.0, 1.0);
    if position <= 0.0 {
        0.0
    } else {
        10f32.powf((position - 1.0) * VOLUME_RANGE_DB / 20.0)
    }
}

pub trait PacketDecoder {
    fn decode(&mut self, packet: &[u8], pcm: &mut [f32]) -> anyhow::Result<usize>;
    fn reset(&mut self);
}

pub struct Opus(opus::Decoder);

impl Opus {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self(opus::Decoder::new(
            SAMPLE_RATE,
            opus::Channels::Stereo,
        )?))
    }
}

impl PacketDecoder for Opus {
    fn decode(&mut self, packet: &[u8], pcm: &mut [f32]) -> anyhow::Result<usize> {
        Ok(self.0.decode_float(packet, pcm, false)?)
    }

    fn reset(&mut self) {
        if let Err(error) = self.0.reset_state() {
            tracing::debug!(%error, "cannot reset the opus decoder");
        }
    }
}

pub struct Sink<D> {
    decoder: D,
    loss: LossTracker,
    feed: Producer<f32>,
    stats: Arc<VoiceStats>,
    pcm: Vec<f32>,
    lost: u64,
}

impl<D: PacketDecoder> Sink<D> {
    pub fn new(decoder: D, feed: Producer<f32>, stats: Arc<VoiceStats>) -> Self {
        Self {
            decoder,
            loss: LossTracker::new(MAX_GAP_FRAMES),
            feed,
            stats,
            pcm: vec![0.0; MAX_PACKET_FRAMES * CHANNELS],
            lost: 0,
        }
    }

    #[must_use]
    pub const fn lost_frames(&self) -> u64 {
        self.lost
    }

    #[must_use]
    pub fn stats(&self) -> &VoiceStats {
        &self.stats
    }

    pub fn rebind(&mut self, feed: Producer<f32>, stats: Arc<VoiceStats>) {
        self.feed = feed;
        self.stats = stats;
        self.restart();
    }

    pub fn restart(&mut self) {
        self.loss.reset();
        self.decoder.reset();
        self.stats.restart();
    }

    pub fn push(
        &mut self,
        timestamp: u64,
        layout: u8,
        packet: &[u8],
        tap: impl FnOnce(&[f32]),
    ) -> anyhow::Result<()> {
        match self.loss.next(timestamp) {
            LossAction::Continuous => {}
            LossAction::Gap(frames) => {
                self.conceal(frames);
                self.lost += frames;
            }
            LossAction::Reset(frames) => {
                self.decoder.reset();
                self.stats.restart();
                self.lost += frames;
            }
        }
        if !matches!(layout, 1 | 2) {
            self.conceal_packet();
            return Err(anyhow!("unsupported audio channel count {layout}"));
        }
        match self.decoder.decode(packet, &mut self.pcm) {
            Ok(frames) => {
                let block = &self.pcm[..frames.min(MAX_PACKET_FRAMES) * CHANNELS];
                tap(block);
                if self.feed.slots() < block.len() {
                    self.lost += frames as u64;
                    return Ok(());
                }
                self.deliver(frames);
                Ok(())
            }
            Err(error) => {
                self.conceal_packet();
                Err(error)
            }
        }
    }

    fn deliver(&mut self, frames: usize) {
        let block = &self.pcm[..frames * CHANNELS];
        if let Ok(mut chunk) = self.feed.write_chunk(block.len()) {
            let (first, second) = chunk.as_mut_slices();
            let split = first.len();
            first.copy_from_slice(&block[..split]);
            second.copy_from_slice(&block[split..]);
            chunk.commit_all();
        }
    }

    fn conceal_packet(&mut self) {
        if let Some(frames) = self.loss.packet_frames() {
            self.conceal(frames);
            self.lost += frames;
        }
    }

    fn conceal(&mut self, frames: u64) {
        let samples = usize::try_from(frames).unwrap_or(usize::MAX) * CHANNELS;
        if self.feed.slots() < samples {
            return;
        }
        if let Ok(mut chunk) = self.feed.write_chunk(samples) {
            let (first, second) = chunk.as_mut_slices();
            first.fill(0.0);
            second.fill(0.0);
            chunk.commit_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use rtrb::{Consumer, RingBuffer};

    use super::*;

    struct Fake {
        frames: usize,
        resets: usize,
        fail: bool,
    }

    impl PacketDecoder for Fake {
        fn decode(&mut self, _packet: &[u8], pcm: &mut [f32]) -> anyhow::Result<usize> {
            if self.fail {
                return Err(anyhow!("corrupt"));
            }
            pcm[..self.frames * CHANNELS].fill(0.25);
            Ok(self.frames)
        }

        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    fn sink(capacity: usize) -> (Sink<Fake>, Consumer<f32>) {
        let (feed, consumer) = RingBuffer::new(capacity);
        let decoder = Fake {
            frames: 960,
            resets: 0,
            fail: false,
        };
        (
            Sink::new(decoder, feed, Arc::new(VoiceStats::default())),
            consumer,
        )
    }

    #[test]
    fn a_decoded_packet_is_tapped_and_queued_as_stereo() {
        let (mut sink, consumer) = sink(8_192);
        let mut tapped = 0;
        assert!(sink.push(0, 1, &[1], |block| tapped = block.len()).is_ok());
        assert_eq!(tapped, 960 * CHANNELS);
        assert_eq!(consumer.slots(), 960 * CHANNELS);
    }

    #[test]
    fn a_gap_is_filled_with_exactly_its_silence_and_counted() {
        let (mut sink, consumer) = sink(16_384);
        for timestamp in [0, 960, 2_880] {
            assert!(sink.push(timestamp, 2, &[1], |_| {}).is_ok());
        }
        assert_eq!(sink.lost_frames(), 960);
        assert_eq!(consumer.slots(), 4 * 960 * CHANNELS);
    }

    #[test]
    fn a_full_queue_drops_the_packet_and_says_so() {
        let (mut sink, consumer) = sink(1_000);
        assert!(sink.push(0, 2, &[1], |_| {}).is_ok());
        assert_eq!(consumer.slots(), 0);
        assert_eq!(sink.lost_frames(), 960);
    }

    #[test]
    fn a_bad_packet_or_layout_is_concealed_and_reported() {
        let (mut sink, _consumer) = sink(16_384);
        assert!(sink.push(0, 2, &[1], |_| {}).is_ok());
        assert!(sink.push(960, 2, &[1], |_| {}).is_ok());
        assert!(sink.push(1_920, 3, &[1], |_| {}).is_err());
        assert_eq!(sink.lost_frames(), 960);
        sink.decoder.fail = true;
        assert!(sink.push(2_880, 2, &[1], |_| {}).is_err());
        assert_eq!(sink.lost_frames(), 1_920);
    }

    #[test]
    fn a_restart_resets_the_decoder_and_the_loss_history() {
        let (mut sink, _consumer) = sink(16_384);
        assert!(sink.push(0, 2, &[1], |_| {}).is_ok());
        sink.restart();
        assert_eq!(sink.decoder.resets, 1);
        assert!(sink.push(0, 2, &[1], |_| {}).is_ok());
        assert_eq!(sink.lost_frames(), 0);
    }

    #[test]
    fn volume_spans_sixty_decibels_and_zero_is_silent() {
        assert!(gain_for_volume(0.0).abs() < f32::EPSILON);
        assert!(gain_for_volume(-1.0).abs() < f32::EPSILON);
        assert!((gain_for_volume(1.0) - 1.0).abs() < 1e-6);
        assert!((gain_for_volume(2.0) - 1.0).abs() < 1e-6);
        assert!((gain_for_volume(0.5) - 10f32.powf(-1.5)).abs() < 1e-6);
    }

    #[test]
    fn the_real_decoder_decodes_an_encoded_packet() {
        let mut encoder =
            opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Audio)
                .expect("an encoder");
        let packet = encoder
            .encode_vec_float(&[0.1f32; 960], 4_000)
            .expect("a packet");
        let mut decoder = Opus::new().expect("a decoder");
        let mut pcm = vec![0.0f32; MAX_PACKET_FRAMES * CHANNELS];
        assert_eq!(decoder.decode(&packet, &mut pcm).ok(), Some(960));
    }
}
