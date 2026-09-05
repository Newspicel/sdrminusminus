use num_complex::Complex;

use super::Publisher;
use crate::recording::RecorderTap;

struct RecordingPacket {
    samples: Vec<Complex<f32>>,
    start: u64,
    center: f64,
    recorder: Option<RecorderTap>,
}

pub(crate) struct RecordingPublisher(Publisher<RecordingPacket>);

impl RecordingPublisher {
    pub(crate) fn new(capacity: usize) -> std::io::Result<Self> {
        Publisher::new(
            "sdrmm-iq-publish",
            64,
            || RecordingPacket {
                samples: Vec::with_capacity(capacity),
                start: 0,
                center: 0.0,
                recorder: None,
            },
            |packet| {
                if let Some(recorder) = packet.recorder.take() {
                    let _ = recorder.push(&packet.samples, packet.start, packet.center);
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
