use std::sync::{Arc, Mutex, mpsc};

use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

use super::*;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

#[test]
fn saturation_is_nonblocking_and_recycled_buffers_preserve_order_without_allocating() {
    let (entered, waiting) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let received = observed.clone();
    let mut first = true;
    let mut publisher = Publisher::new(
        "test-publish",
        4,
        || vec![0u64; 16],
        move |packet| {
            if first {
                first = false;
                entered.send(()).expect("signal worker entered");
                resume
                    .recv_timeout(Duration::from_secs(5))
                    .expect("release worker");
            }
            received.lock().expect("results").push(packet[0]);
        },
        || {},
    )
    .expect("publisher");
    assert!(publisher.submit(|packet| packet[0] = 0));
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("worker started");
    let mut accepted = [false; 4];
    assert_no_alloc("publication while consumer is blocked", || {
        for (index, result) in accepted.iter_mut().enumerate() {
            *result = publisher.submit(|packet| packet[0] = index as u64 + 1);
        }
    });
    release.send(()).expect("release");
    assert_eq!(accepted, [true, true, true, false]);
    publisher.flush();
    for sequence in 4..20 {
        assert_no_alloc("recycled publication", || {
            assert!(publisher.submit(|packet| packet[0] = sequence));
        });
        publisher.flush();
    }
    drop(publisher);
    assert_eq!(
        *observed.lock().expect("results"),
        (0..20).collect::<Vec<_>>()
    );
}

#[test]
fn shutdown_drains_pending_publications() {
    let values = Arc::new(Mutex::new(Vec::new()));
    let received = values.clone();
    let mut publisher = Publisher::new(
        "test-drain",
        8,
        || 0,
        move |value| {
            received.lock().expect("values").push(*value);
        },
        || {},
    )
    .expect("publisher");
    for value in 0..8 {
        assert!(publisher.submit(|packet| *packet = value));
    }
    drop(publisher);
    assert_eq!(*values.lock().expect("values"), (0..8).collect::<Vec<_>>());
}

#[test]
fn recording_publication_failures_are_reported_without_allocating() {
    let (audio, _audio_blocks, audio_state) = crate::audio_recording::create_tap();
    let (iq, _position, _iq_blocks, iq_state) = crate::recording::create_tap();
    assert_no_alloc("recording failure reporting", || {
        audio.publication_failed();
        iq.publication_failed();
    });
    assert!(
        audio_state
            .error()
            .expect("audio fault")
            .contains("publication")
    );
    assert!(iq_state.error().expect("IQ fault").contains("publication"));
    assert!(!iq.healthy());
}
