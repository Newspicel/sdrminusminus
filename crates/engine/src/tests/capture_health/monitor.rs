use tokio::sync::broadcast::{Receiver, error::TryRecvError};

use crate::{AudioPacket, Engine, PcmBlock, SpectrumSnapshot, audio::PcmPayload};

#[derive(Default, Debug)]
struct Timeline {
    next: Option<u64>,
    frames: u64,
    missing: u64,
    backwards: u64,
}

impl Timeline {
    fn push(&mut self, start: u64, frames: u64) {
        if let Some(next) = self.next {
            self.missing += start.saturating_sub(next);
            self.backwards += next.saturating_sub(start);
        }
        self.next = Some(start + frames);
        self.frames += frames;
    }
}

fn drain<T: Clone>(rx: &mut Receiver<T>, lagged: &mut u64, mut receive: impl FnMut(T)) {
    loop {
        match rx.try_recv() {
            Ok(value) => receive(value),
            Err(TryRecvError::Lagged(count)) => *lagged += count,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Closed) => panic!("observed stream closed during capture"),
        }
    }
}

pub(super) struct AudioMonitor {
    ds: u32,
    channel: u32,
    pcm: Receiver<PcmBlock>,
    opus: Receiver<AudioPacket>,
    pcm_time: Timeline,
    opus_time: Timeline,
    lagged: u64,
}

impl AudioMonitor {
    pub(super) fn new(engine: &Engine, ds: u32, channel: u32) -> Self {
        Self {
            ds,
            channel,
            pcm: engine.subscribe_pcm(ds, channel).expect("PCM subscription"),
            opus: engine
                .subscribe_audio(ds, channel)
                .expect("Opus subscription"),
            pcm_time: Timeline::default(),
            opus_time: Timeline::default(),
            lagged: 0,
        }
    }

    pub(super) fn poll(&mut self) {
        drain(&mut self.pcm, &mut self.lagged, |block| {
            let frames = match block.payload {
                PcmPayload::Silence(frames) => frames,
                PcmPayload::Samples(samples) => {
                    assert!(
                        samples.iter().all(|sample| sample.is_finite()),
                        "nonfinite PCM"
                    );
                    assert!(samples.len().is_multiple_of(usize::from(block.channels)));
                    samples.len() / usize::from(block.channels)
                }
            };
            self.pcm_time.push(block.start_frame, frames as u64);
        });
        drain(&mut self.opus, &mut self.lagged, |packet| {
            let frames = opus::packet::get_nb_samples(&packet.opus, 48_000).expect("valid Opus");
            assert_eq!(frames, crate::audio::OPUS_FRAME_SAMPLES);
            self.opus_time.push(packet.timestamp, frames as u64);
        });
    }

    pub(super) fn verify(&self, allow_drops: bool, seconds: u64) {
        eprintln!(
            "audio ds={} ch={} pcm={:?} opus={:?} observer_lag={}",
            self.ds, self.channel, self.pcm_time, self.opus_time, self.lagged
        );
        assert!(
            self.pcm_time.frames > 0 && self.opus_time.frames > 0,
            "audio never arrived"
        );
        assert_eq!(
            self.pcm_time.backwards + self.opus_time.backwards,
            0,
            "audio clock moved backwards"
        );
        assert_eq!(self.lagged, 0, "test observer could not keep up");
        if !allow_drops {
            let minimum_frames = seconds.saturating_sub(1) * 48_000;
            assert!(
                self.pcm_time.frames >= minimum_frames && self.opus_time.frames >= minimum_frames,
                "audio did not sustain the requested duration"
            );
            assert_eq!(
                self.pcm_time.missing + self.opus_time.missing,
                0,
                "audio samples were lost"
            );
        }
    }
}

pub(super) struct SpectrumMonitor {
    ds: u32,
    rx: Receiver<SpectrumSnapshot>,
    timeline: Timeline,
    lagged: u64,
    last_timestamp: Option<u64>,
}

impl SpectrumMonitor {
    pub(super) fn new(engine: &Engine, ds: u32) -> Self {
        Self {
            ds,
            rx: engine
                .subscribe_spectrum(ds, 0)
                .expect("spectrum subscription"),
            timeline: Timeline::default(),
            lagged: 0,
            last_timestamp: None,
        }
    }

    pub(super) fn poll(&mut self) {
        drain(&mut self.rx, &mut self.lagged, |frame| {
            assert!(
                frame.db.iter().all(|value| value.is_finite()),
                "nonfinite spectrum"
            );
            if let Some(previous) = self.last_timestamp {
                assert!(frame.timestamp > previous, "spectrum clock did not advance");
            }
            self.last_timestamp = Some(frame.timestamp);
            self.timeline.push(u64::from(frame.seq), 1);
        });
    }

    pub(super) fn verify(&self, allow_drops: bool) {
        eprintln!(
            "spectrum ds={} {:?} observer_lag={}",
            self.ds, self.timeline, self.lagged
        );
        assert!(self.timeline.frames > 0, "spectrum never arrived");
        assert_eq!(self.timeline.backwards, 0);
        assert_eq!(self.lagged, 0, "test observer could not keep up");
        if !allow_drops {
            assert_eq!(self.timeline.missing, 0, "spectrum frames were lost");
        }
    }
}

#[test]
fn timelines_distinguish_forward_gaps_from_replayed_samples() {
    let mut timeline = Timeline::default();
    timeline.push(100, 10);
    timeline.push(110, 10);
    timeline.push(125, 10);
    timeline.push(132, 10);
    assert_eq!(timeline.frames, 40);
    assert_eq!(timeline.missing, 5);
    assert_eq!(timeline.backwards, 3);
}
