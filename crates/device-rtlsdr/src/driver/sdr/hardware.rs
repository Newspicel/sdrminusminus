use std::{
    sync::mpsc::RecvTimeoutError,
    time::{Duration, Instant},
};

use super::*;

#[derive(Default)]
struct Counter {
    next: Option<u8>,
    bytes: u64,
    gaps: u64,
    missing_minimum: u64,
}

impl Counter {
    fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if let Some(expected) = self.next
                && byte != expected
            {
                self.gaps += 1;
                self.missing_minimum += u64::from(byte.wrapping_sub(expected));
            }
            self.next = Some(byte.wrapping_add(1));
        }
        self.bytes += bytes.len() as u64;
    }
}

#[test]
fn counter_tracks_wraps_and_gaps_across_transfer_boundaries() {
    let mut counter = Counter::default();
    counter.push(&[253, 254]);
    counter.push(&[]);
    counter.push(&[255, 0, 1]);
    assert_eq!(counter.gaps, 0);
    counter.push(&[5, 6]);
    counter.push(&[255, 0]);
    assert_eq!(counter.gaps, 2);
    assert_eq!(counter.missing_minimum, 3 + 248);
    assert_eq!(counter.bytes, 9);
}

#[test]
#[ignore = "requires an idle RTL-SDR; checks its hardware counter over native USB"]
fn connected_rtl_counter_continuity() {
    let rate = std::env::var("SDRMM_RTL_TEST_RATE")
        .map(|value| value.parse().expect("sample rate"))
        .unwrap_or(2_400_000);
    let seconds = std::env::var("SDRMM_RTL_TEST_SECONDS")
        .map(|value| value.parse().expect("duration"))
        .unwrap_or(30);
    assert!(seconds > 0);
    let _awake = sdrmm_device::schedule::stay_awake("RTL counter test");
    sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
    let mut radio = RtlSdr::open(0).expect("open RTL-SDR");
    radio.set_sample_rate(rate).expect("sample rate");
    radio.set_center_freq(100_000_000).expect("frequency");
    radio
        .dev
        .demod_write_reg(0, 0x19, 0x03, 1)
        .expect("enable hardware counter");
    let mut stream = radio.start_streaming().expect("start native USB");
    let mut counter = Counter::default();
    let started = Instant::now();
    let mut first = None;
    let mut last = started;
    let mut measured_bytes = 0;
    while started.elapsed() < Duration::from_secs(seconds) {
        match stream.recv_timeout(Duration::from_millis(100)) {
            Ok(block) => {
                last = Instant::now();
                if first.is_some() {
                    measured_bytes += block.len();
                } else {
                    first = Some(last);
                }
                counter.push(&block);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("USB ended: {:?}", stream.error()),
        }
    }
    let stats = stream.stop();
    radio
        .dev
        .demod_write_reg(0, 0x19, 0x05, 1)
        .expect("disable hardware counter");
    let elapsed = last
        .duration_since(first.expect("received data"))
        .as_secs_f64();
    let measured_rate = measured_bytes as f64 / 2.0 / elapsed;
    eprintln!(
        "RTL counter rate={rate} measured={measured_rate:.1} bytes={} discontinuities={} missing_bytes_minimum={} usb={stats:?}",
        counter.bytes, counter.gaps, counter.missing_minimum
    );
    assert_eq!(counter.gaps, 0, "hardware byte sequence has gaps");
    assert_eq!(stats.dropped, 0, "USB dropped transfers");
    assert!(
        (measured_rate / f64::from(rate) - 1.0).abs() < 0.01,
        "sample rate mismatch"
    );
}
