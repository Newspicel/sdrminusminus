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
