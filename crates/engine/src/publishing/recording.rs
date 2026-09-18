use num_complex::Complex;

use super::Publisher;
use crate::{
    recording::{RecorderTap, queue_depth},
    runtime::DSP_BLOCK,
};

struct RecordingPacket {
    samples: Vec<Complex<f32>>,
    start: u64,
    center: f64,
    recorder: Option<RecorderTap>,
}

pub(crate) struct RecordingPublisher(Publisher<RecordingPacket>);

impl RecordingPublisher {
    pub(crate) fn new(sample_rate: f64) -> std::io::Result<Self> {
        Publisher::new(
            "sdrmm-iq-publish",
            queue_depth(sample_rate),
            || RecordingPacket {
                samples: Vec::with_capacity(DSP_BLOCK),
                start: 0,
                center: 0.0,
                recorder: None,
            },
            |packet| {
                if let Some(recorder) = packet.recorder.take() {
                    let _ = recorder.push_waiting(&packet.samples, packet.start, packet.center);
                }
                packet.samples.clear();
            },
            || {},
        )
        .map(Self)
    }

    pub(crate) fn publish(
        &mut self,
        recorder: &RecorderTap,
        samples: &[Complex<f32>],
        start: u64,
        center: f64,
    ) -> bool {
        if !recorder.healthy() {
            return false;
        }
        let sent = self.0.submit(|packet| {
            packet.samples.extend_from_slice(samples);
            packet.start = start;
            packet.center = center;
            packet.recorder = Some(recorder.clone());
        });
        if !sent {
            recorder.publication_failed();
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use sdrmm_recorder::{SigmfReader, SigmfWriter};
    use sdrmm_test_support::assert_no_alloc;

    use super::*;
    use crate::recording::{create_tap, spawn_writer};

    #[test]
    fn a_paused_writer_uses_both_queues_and_preserves_every_sample() {
        let rate = 48_000.0;
        let count = queue_depth(rate);
        let directory = tempfile::tempdir().unwrap();
        let stem = directory.path().join("stalled");
        let writer = SigmfWriter::create(&stem, rate, 100_000_000.0, "test").unwrap();
        let (tap, position, messages, shared) = create_tap(rate);
        let mut samples = vec![Complex::new(0.0, 0.0); DSP_BLOCK];
        for index in 0..count {
            samples.fill(Complex::new(index as f32, -(index as f32)));
            assert!(tap.push(&samples, (index * DSP_BLOCK) as u64, 100_000_000.0));
        }
        let mut publisher = RecordingPublisher::new(rate).unwrap();
        assert_no_alloc("recording while writer is paused", || {
            for index in count..count + 16 {
                samples.fill(Complex::new(index as f32, -(index as f32)));
                assert!(publisher.publish(
                    &tap,
                    &samples,
                    (index * DSP_BLOCK) as u64,
                    if index < count + 8 {
                        100_000_000.0
                    } else {
                        101_000_000.0
                    }
                ));
            }
        });
        let (completed, retired) = mpsc::channel();
        let retirement = std::thread::spawn(move || {
            drop(publisher);
            completed.send(()).unwrap();
        });
        let premature = retired.recv_timeout(Duration::from_millis(50)).is_ok();
        let error_while_paused = shared.error();
        let handle = spawn_writer(writer, messages, shared.clone()).unwrap();
        retirement.join().unwrap();
        drop(tap);
        drop(position);
        handle.join().unwrap();
        assert!(!premature, "publication stopped before the writer resumed");
        assert_eq!(error_while_paused, None);
        assert_eq!(shared.error(), None);
        let total = ((count + 16) * DSP_BLOCK) as u64;
        assert_eq!(shared.samples(), total);
        let mut reader = SigmfReader::open(&stem).unwrap();
        assert_eq!(reader.total_samples(), total);
        assert_eq!(reader.meta().captures.len(), 2);
        assert_eq!(
            reader.meta().captures[1].sample_start,
            ((count + 8) * DSP_BLOCK) as u64
        );
        assert_eq!(reader.meta().captures[1].frequency, Some(101_000_000.0));
        let mut actual = vec![Complex::new(0.0, 0.0); DSP_BLOCK];
        for index in 0..count + 16 {
            samples.fill(Complex::new(index as f32, -(index as f32)));
            assert_eq!(reader.read_block(&mut actual).unwrap(), DSP_BLOCK);
            assert_eq!(actual, samples);
        }
    }

    #[test]
    fn exhausted_recording_capacity_fails_without_blocking_or_allocating_on_dsp() {
        let rate = 48_000.0;
        let count = queue_depth(rate);
        let (tap, _position, messages, shared) = create_tap(rate);
        let samples = vec![Complex::new(0.25, -0.5); DSP_BLOCK];
        for index in 0..count {
            assert!(tap.push(&samples, (index * DSP_BLOCK) as u64, 100_000_000.0));
        }
        let mut publisher = RecordingPublisher::new(rate).unwrap();
        let mut accepted = 0;
        assert_no_alloc("exhausted recording queues", || {
            for index in count..2 * count + 1 {
                accepted += usize::from(publisher.publish(
                    &tap,
                    &samples,
                    (index * DSP_BLOCK) as u64,
                    100_000_000.0,
                ));
            }
        });
        let error = shared.error();
        drop(messages);
        drop(publisher);
        assert_eq!(accepted, count);
        assert!(error.unwrap().contains("publication queue overflow"));
        assert!(!tap.healthy());
    }
}
