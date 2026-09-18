use std::{sync::Arc, time::Duration};

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_device::SinkRoom;

use crate::metrics::QueueMetrics;

const SPAN_CAPACITY: usize = 4096;

#[derive(Clone, Copy)]
struct Span {
    start: u64,
    len: usize,
    queued: u64,
}

pub(crate) struct CaptureProducer {
    samples: Producer<Complex<f32>>,
    spans: Producer<Span>,
    metrics: Arc<QueueMetrics>,
    room: Arc<SinkRoom>,
    next: Option<u64>,
}

pub(crate) struct CaptureConsumer {
    samples: Consumer<Complex<f32>>,
    spans: Consumer<Span>,
    pending: Option<Span>,
    recovering: bool,
    last_stale: Option<u64>,
    fresh_since_stale: bool,
    room: Arc<SinkRoom>,
    pub(crate) metrics: Arc<QueueMetrics>,
}

pub(crate) fn capture_ring(capacity: usize) -> (CaptureProducer, CaptureConsumer) {
    let metrics = Arc::new(QueueMetrics::default());
    metrics.capacity(capacity);
    let room = Arc::new(SinkRoom::new(capacity));
    let (samples_tx, samples_rx) = RingBuffer::new(capacity);
    let (spans_tx, spans_rx) = RingBuffer::new(capacity.min(SPAN_CAPACITY));
    (
        CaptureProducer {
            samples: samples_tx,
            spans: spans_tx,
            metrics: metrics.clone(),
            room: room.clone(),
            next: None,
        },
        CaptureConsumer {
            samples: samples_rx,
            spans: spans_rx,
            pending: None,
            recovering: false,
            last_stale: None,
            fresh_since_stale: false,
            room,
            metrics,
        },
    )
}

impl CaptureProducer {
    pub(crate) fn room(&self) -> Arc<SinkRoom> {
        self.room.clone()
    }

    pub(crate) fn push(&mut self, samples: &[Complex<f32>], start: u64) -> usize {
        if let Some(next) = self.next {
            self.metrics.dropped(start.saturating_sub(next) as usize);
        }
        self.next = Some(start + samples.len() as u64);
        let len = samples.len().min(self.samples.slots());
        if len == 0 {
            self.metrics.dropped(samples.len());
            return 0;
        }
        let Ok(span) = self.spans.write_chunk_uninit(1) else {
            self.metrics.dropped(samples.len());
            return 0;
        };
        let Ok(chunk) = self.samples.write_chunk_uninit(len) else {
            self.metrics.dropped(samples.len());
            return 0;
        };
        self.metrics.push(len);
        self.room.took(len);
        self.metrics.dropped(samples.len() - len);
        chunk.fill_from_iter(samples[..len].iter().copied());
        span.fill_from_iter([Span {
            start,
            len,
            queued: self.metrics.now(),
        }]);
        len
    }
}

impl CaptureConsumer {
    #[cfg(test)]
    pub(crate) fn consume(
        &mut self,
        limit: usize,
        receive: impl FnMut(&[Complex<f32>], u64),
    ) -> usize {
        self.consume_fresh(limit, Duration::MAX, receive)
    }

    pub(crate) fn consume_fresh(
        &mut self,
        limit: usize,
        max_age: Duration,
        receive: impl FnMut(&[Complex<f32>], u64),
    ) -> usize {
        self.consume_fresh_at(limit, max_age, self.metrics.now(), receive)
    }

    fn consume_fresh_at(
        &mut self,
        limit: usize,
        max_age: Duration,
        now: u64,
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
        self.metrics.oldest(span.queued);
        let age = u128::from(now.saturating_sub(span.queued));
        let max_age = max_age.as_micros();
        if age > max_age {
            self.recovering |= self.fresh_since_stale
                && self
                    .last_stale
                    .is_some_and(|last| u128::from(now.saturating_sub(last)) <= max_age);
            self.last_stale = Some(now);
            self.fresh_since_stale = false;
        }
        let fresh = age
            <= if self.recovering {
                max_age / 2
            } else {
                max_age
            };
        if fresh {
            self.recovering = false;
            self.fresh_since_stale = true;
        }
        if !fresh {
            self.metrics.dropped(len);
        }
        let (a, b) = chunk.as_slices();
        if fresh && !a.is_empty() {
            receive(a, span.start);
        }
        if fresh && !b.is_empty() {
            receive(b, span.start + a.len() as u64);
        }
        self.metrics.pop(len);
        chunk.commit_all();
        self.room.freed(len);
        span.start += len as u64;
        span.len -= len;
        self.pending = (span.len > 0).then_some(span);
        len
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

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
    fn sustained_overload_keeps_contiguous_audio_windows_available() {
        let (mut producer, mut consumer) = capture_ring(100);
        let mut now = 7_000;
        let mut produced = 0;
        let mut received = 0;
        let mut next = 0;
        let mut contiguous = 0;
        let mut last_frame = 0;
        while now < 2_000_000 {
            while (produced + 7) * 1_000 <= now {
                producer.push(&ramp(produced, 7), produced);
                produced += 7;
            }
            if let Some(mut span) = consumer
                .pending
                .take()
                .or_else(|| consumer.spans.pop().ok())
            {
                span.queued = (span.start / 7 + 1) * 7_000;
                consumer.pending = Some(span);
            }
            let mut processed = 0;
            consumer.consume_fresh_at(1, Duration::from_millis(100), now, |samples, start| {
                if start != next {
                    contiguous = 0;
                }
                next = start + samples.len() as u64;
                contiguous += samples.len();
                if contiguous >= 20 {
                    last_frame = now;
                    contiguous -= 20;
                }
                processed += samples.len();
            });
            received += processed as u64;
            now += if processed > 0 { 2_000 } else { 10 };
        }
        let health = consumer.metrics.snapshot();
        assert_eq!(received + health.dropped + health.queued, produced);
        assert!(
            health.dropped > 0,
            "the simulated consumer must be overloaded"
        );
        assert!(last_frame >= 1_800_000, "audio stopped at {last_frame} us");
    }

    #[test]
    fn repeated_stalls_recover_headroom_then_restore_the_full_age_limit() {
        let (mut producer, mut consumer) = capture_ring(24);
        producer.push(&ramp(0, 4), 0);
        producer.push(&ramp(4, 4), 4);
        producer.push(&ramp(8, 4), 8);
        producer.push(&ramp(12, 4), 12);
        producer.push(&ramp(16, 8), 16);
        for queued in [0, 20_000, 30_000, 50_000, 90_000] {
            let mut span = consumer.spans.pop().expect("span");
            span.queued = queued;
            producer.spans.push(span).expect("restamp");
        }
        let max_age = Duration::from_millis(100);
        assert_eq!(
            consumer.consume_fresh_at(4, max_age, 100_001, |_, _| panic!("stale")),
            4
        );
        assert_eq!(
            consumer.consume_fresh_at(4, max_age, 100_001, |samples, start| {
                assert_eq!(start, 4);
                assert_eq!(samples, ramp(4, 4));
            }),
            4
        );
        for _ in 0..2 {
            assert_eq!(
                consumer.consume_fresh_at(4, max_age, 130_001, |_, _| panic!("stale")),
                4
            );
        }
        let mut received = Vec::new();
        for now in [130_001, 180_000] {
            assert_eq!(
                consumer.consume_fresh_at(4, max_age, now, |samples, start| {
                    received.extend(
                        samples
                            .iter()
                            .enumerate()
                            .map(|(offset, sample)| (start + offset as u64, sample.re)),
                    );
                }),
                4
            );
        }
        assert_eq!(received, expected(16..24));
        assert_eq!(consumer.metrics.snapshot().dropped, 12);
        assert_eq!(consumer.metrics.snapshot().queued, 0);
        assert_eq!(producer.room.free(), 24);
    }

    #[test]
    fn an_isolated_stall_preserves_every_sample_within_the_age_limit() {
        let (mut producer, mut consumer) = capture_ring(8);
        for start in [0, 4] {
            producer.push(&ramp(start, 4), start);
        }
        for queued in [0, 30_000] {
            let mut span = consumer.spans.pop().expect("span");
            span.queued = queued;
            producer.spans.push(span).expect("restamp");
        }
        let max_age = Duration::from_millis(100);
        assert_eq!(
            consumer.consume_fresh_at(4, max_age, 110_000, |_, _| panic!("stale")),
            4
        );
        assert_eq!(
            consumer.consume_fresh_at(4, max_age, 110_000, |samples, start| {
                assert_eq!(start, 4);
                assert_eq!(samples, ramp(4, 4));
            }),
            4
        );
        assert_eq!(consumer.metrics.snapshot().dropped, 4);
        assert_eq!(producer.room.free(), 8);
    }

    #[test]
    fn stale_capture_is_counted_and_discarded_before_dsp() {
        let (mut producer, mut consumer) = capture_ring(8);
        producer.push(&ramp(0, 4), 0);
        let span = consumer.spans.pop().expect("span");
        consumer.pending = Some(span);
        while consumer.metrics.now() == span.queued {
            std::hint::spin_loop();
        }
        let mut delivered = 0;
        assert_eq!(
            consumer.consume_fresh(8, Duration::ZERO, |samples, _| delivered += samples.len()),
            4
        );
        assert_eq!(delivered, 0);
        let metrics = consumer.metrics.snapshot();
        assert_eq!(metrics.queued, 0);
        assert_eq!(metrics.dropped, 4);
        producer.push(&ramp(4, 2), 4);
        assert_eq!(drain(&mut consumer, 8), expected(4..6));
    }

    #[test]
    fn shared_drop_counter_counts_overflow_source_gaps_and_stale_samples_once() {
        let (mut producer, mut consumer) = capture_ring(8);
        let counter = consumer.metrics.dropped_counter();
        assert_eq!(producer.push(&ramp(0, 10), 0), 8);
        assert_eq!(counter.load(Ordering::Relaxed), 2);
        assert_eq!(producer.push(&ramp(14, 2), 14), 0);
        assert_eq!(counter.load(Ordering::Relaxed), 8);
        let span = consumer.spans.pop().expect("span");
        consumer.pending = Some(span);
        while consumer.metrics.now() == span.queued {
            std::hint::spin_loop();
        }
        assert_eq!(
            consumer.consume_fresh(8, Duration::ZERO, |_, _| panic!("stale")),
            8
        );
        assert_eq!(counter.load(Ordering::Relaxed), 16);
        assert_eq!(producer.push(&ramp(16, 4), 16), 4);
        assert_eq!(drain(&mut consumer, 8), expected(16..20));
        assert_eq!(counter.load(Ordering::Relaxed), 16);
        assert_eq!(consumer.metrics.snapshot().dropped, 16);
    }

    #[test]
    fn room_tracks_what_the_ring_still_takes_across_a_stale_discard() {
        let (mut producer, mut consumer) = capture_ring(8);
        let room = producer.room();
        assert_eq!(room.free(), 8);
        producer.push(&ramp(0, 6), 0);
        assert_eq!(room.free(), 2);
        assert_eq!(consumer.consume(4, |_, _| {}), 4);
        assert_eq!(room.free(), 6);

        let span = consumer
            .pending
            .or_else(|| consumer.spans.pop().ok())
            .expect("span");
        consumer.pending = Some(span);
        while consumer.metrics.now() == span.queued {
            std::hint::spin_loop();
        }
        assert_eq!(consumer.consume_fresh(8, Duration::ZERO, |_, _| {}), 2);
        assert_eq!(room.free(), 8, "a discarded span gives its slots back");
    }

    #[test]
    fn a_backpressured_source_never_loses_samples_when_the_consumer_releases_room() {
        use std::{sync::atomic::AtomicBool, time::Instant};

        let (mut producer, mut consumer) = capture_ring(1);
        let room = producer.room();
        let done = Arc::new(AtomicBool::new(false));
        let consumer_done = done.clone();
        let worker = std::thread::spawn(move || {
            let mut received = 0;
            let mut valid = true;
            loop {
                let finished = consumer_done.load(Ordering::Acquire);
                let consumed = consumer.consume(1, |samples, start| {
                    valid &= start == received && samples[0].re == received as f32;
                    received += samples.len() as u64;
                });
                if consumed == 0 {
                    if finished {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
            (received, valid)
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut sent = 0;
        while sent < 250_000 && Instant::now() < deadline {
            if room.free() == 0 {
                std::hint::spin_loop();
                continue;
            }
            if producer.push(&[Complex::new(sent as f32, 0.0)], sent) != 1 {
                break;
            }
            sent += 1;
        }
        done.store(true, Ordering::Release);
        let (received, valid) = worker.join().expect("consumer");
        assert!(valid, "sample contents or positions changed");
        assert_eq!(received, sent);
        assert_eq!(sent, 250_000, "advertised room must accept the next sample");
        assert_eq!(room.free(), 1);
        assert_eq!(producer.metrics.snapshot().dropped, 0);
    }

    #[test]
    fn live_capture_can_reuse_committed_storage_before_room_is_published() {
        let (mut producer, mut consumer) = capture_ring(8);
        let room = producer.room();
        assert_eq!(producer.push(&ramp(0, 8), 0), 8);
        consumer.spans.pop().expect("span");
        consumer.metrics.pop(8);
        consumer
            .samples
            .read_chunk(8)
            .expect("samples")
            .commit_all();
        assert_eq!(room.free(), 0);
        assert_eq!(producer.push(&ramp(8, 3), 8), 3);
        assert_eq!(room.free(), 0);
        consumer.room.freed(8);
        assert_eq!(room.free(), 5);
        assert_eq!(drain(&mut consumer, 8), expected(8..11));
        assert_eq!(room.free(), 8);
        assert_eq!(consumer.metrics.snapshot().dropped, 0);
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
        assert!(msps > 10.0, "indexed capture fell below 10 MS/s: {msps}");
    }
}
