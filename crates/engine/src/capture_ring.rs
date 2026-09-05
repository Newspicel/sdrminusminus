use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};

const SPAN_CAPACITY: usize = 4096;

#[derive(Clone, Copy)]
struct Span {
    start: u64,
    len: usize,
}

pub(crate) struct CaptureProducer {
    samples: Producer<Complex<f32>>,
    spans: Producer<Span>,
}

pub(crate) struct CaptureConsumer {
    samples: Consumer<Complex<f32>>,
    spans: Consumer<Span>,
    pending: Option<Span>,
}

pub(crate) fn capture_ring(capacity: usize) -> (CaptureProducer, CaptureConsumer) {
    let (samples_tx, samples_rx) = RingBuffer::new(capacity);
    let (spans_tx, spans_rx) = RingBuffer::new(capacity.min(SPAN_CAPACITY));
    (
        CaptureProducer {
            samples: samples_tx,
            spans: spans_tx,
        },
        CaptureConsumer {
            samples: samples_rx,
            spans: spans_rx,
            pending: None,
        },
    )
}

impl CaptureProducer {
    pub(crate) fn push(&mut self, samples: &[Complex<f32>], start: u64) -> usize {
        let len = samples.len().min(self.samples.slots());
        if len == 0 {
            return 0;
        }
        let Ok(span) = self.spans.write_chunk_uninit(1) else {
            return 0;
        };
        let Ok(chunk) = self.samples.write_chunk_uninit(len) else {
            return 0;
        };
        chunk.fill_from_iter(samples[..len].iter().copied());
        span.fill_from_iter([Span { start, len }]);
        len
    }
}

impl CaptureConsumer {
    pub(crate) fn consume(
        &mut self,
        limit: usize,
        mut receive: impl FnMut(&[Complex<f32>], u64),
    ) -> usize {
        let Some(mut span) = self.pending.take().or_else(|| self.spans.pop().ok()) else {
            return 0;
        };
        let len = span.len.min(limit);
        let Ok(chunk) = self.samples.read_chunk(len) else {
            self.pending = Some(span);
            return 0;
        };
        let (a, b) = chunk.as_slices();
        if !a.is_empty() {
            receive(a, span.start);
        }
        if !b.is_empty() {
            receive(b, span.start + a.len() as u64);
        }
        chunk.commit_all();
        span.start += len as u64;
        span.len -= len;
        self.pending = (span.len > 0).then_some(span);
        len
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_test_support::{assert_no_alloc, measure_throughput};

    use super::*;

    fn ramp(start: u64, len: usize) -> Vec<Complex<f32>> {
        (start..start + len as u64)
            .map(|index| Complex::new(index as f32, 0.0))
            .collect()
    }

    fn drain(consumer: &mut CaptureConsumer, limit: usize) -> Vec<(u64, f32)> {
        let mut received = Vec::new();
        while consumer.consume(limit, |samples, start| {
            received.extend(
                samples
                    .iter()
                    .enumerate()
                    .map(|(offset, sample)| (start + offset as u64, sample.re)),
            );
        }) > 0
        {}
        received
    }

    fn expected(indices: impl Iterator<Item = u64>) -> Vec<(u64, f32)> {
        indices.map(|index| (index, index as f32)).collect()
    }

    #[test]
    fn an_overflow_keeps_the_old_prefix_and_marks_the_missing_tail() {
        let (mut producer, mut consumer) = capture_ring(8);
        assert_eq!(producer.push(&ramp(0, 6), 0), 6);
        assert_eq!(producer.push(&ramp(6, 6), 6), 2);
        assert_eq!(producer.push(&ramp(12, 4), 12), 0);
        assert_eq!(drain(&mut consumer, 3), expected(0..8));
        assert_eq!(producer.push(&ramp(16, 6), 16), 6);
        assert_eq!(drain(&mut consumer, 3), expected(16..22));
    }

    #[test]
    fn device_gaps_and_wrapped_chunks_keep_each_samples_capture_index() {
        let (mut producer, mut consumer) = capture_ring(8);
        assert_eq!(producer.push(&ramp(10, 6), 10), 6);
        assert_eq!(drain(&mut consumer, 4), expected(10..16));
        assert_eq!(producer.push(&ramp(25, 5), 25), 5);
        assert_eq!(producer.push(&ramp(40, 3), 40), 3);
        assert_eq!(drain(&mut consumer, 4), expected((25..30).chain(40..43)));
    }

    #[test]
    fn a_full_span_queue_cannot_enqueue_unindexed_samples() {
        let (mut producer, mut consumer) = capture_ring(8);
        (producer.spans, consumer.spans) = RingBuffer::new(1);
        assert_eq!(producer.push(&ramp(0, 2), 0), 2);
        assert_eq!(producer.push(&ramp(2, 2), 2), 0);
        assert_eq!(drain(&mut consumer, 1), expected(0..2));
        assert_eq!(producer.push(&ramp(4, 6), 4), 6);
        assert_eq!(drain(&mut consumer, 4), expected(4..10));
    }

    #[test]
    fn indexed_capture_reuses_storage_and_meets_radio_throughput() {
        let (mut producer, mut consumer) = capture_ring(8192);
        let samples = ramp(0, 2048);
        let mut position = 0;
        let mut transfer = || {
            assert_eq!(producer.push(&samples, position), samples.len());
            assert_eq!(
                consumer.consume(samples.len(), |block, start| {
                    assert_eq!(start, position);
                    std::hint::black_box(block);
                }),
                samples.len()
            );
            position += samples.len() as u64;
        };
        assert_no_alloc("indexed capture", &mut transfer);
        let msps = measure_throughput(2000, samples.len() as u64, transfer);
        assert!(
            msps > 10.0,
            "indexed capture fell below 10 Msamples/s: {msps}"
        );
    }
}
