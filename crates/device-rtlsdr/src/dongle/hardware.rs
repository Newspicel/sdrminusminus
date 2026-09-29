use std::time::{Duration, Instant};

use sdrmm_usb_stream::RxStream;

use super::{
    catalog::{Catalog, Listing},
    radio::{DirectSampling, Dongle},
};

fn env_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(default)
}

fn chosen(listings: &[Listing]) -> Option<&Listing> {
    let wanted = std::env::var("SDRMM_RTL_TEST_SERIAL").ok();
    let banked = crate::kraken::claimed(listings);
    listings.iter().find(|listing| match &wanted {
        Some(serial) => listing.serial.as_ref() == Some(serial),
        None => !banked.contains(&listing.index),
    })
}

fn first_dongle() -> Dongle {
    let catalog = Catalog::scan().expect("scan USB");
    let listings: Vec<Listing> = catalog.listings().cloned().collect();
    let listing = chosen(&listings).expect("a standalone RTL-SDR is attached");
    println!(
        "dongle {:?} {:?} serial {:?} board {:?}",
        listing.manufacturer, listing.product, listing.serial, listing.board
    );
    catalog.open(listing.index).expect("open")
}

#[derive(Default)]
struct Continuity {
    next: Option<u8>,
    breaks: u64,
}

impl Continuity {
    fn feed(&mut self, bytes: &[u8]) {
        let Some(&first) = bytes.first() else {
            return;
        };
        if self.next.is_some_and(|expected| expected != first) {
            self.breaks += 1;
        }
        let mut expected = first;
        for &byte in bytes {
            if byte != expected {
                self.breaks += 1;
            }
            expected = byte.wrapping_add(1);
        }
        self.next = Some(expected);
    }
}

#[test]
fn continuity_counts_every_break_including_across_blocks() {
    let mut check = Continuity::default();
    check.feed(&[254, 255]);
    check.feed(&[0, 1]);
    check.feed(&[]);
    assert_eq!(check.breaks, 0);
    check.feed(&[5, 6, 9]);
    assert_eq!(check.breaks, 2);
}

struct Tally {
    bytes: u64,
    missing: u64,
    since: Option<Instant>,
}

fn drain_counter(stream: &RxStream, seconds: u64, check: &mut Continuity) -> Tally {
    let mut tally = Tally {
        bytes: 0,
        missing: 0,
        since: None,
    };
    let stop = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < stop {
        let Ok(block) = stream.recv_timeout(Duration::from_millis(500)) else {
            assert!(stream.error().is_none(), "{:?}", stream.error());
            continue;
        };
        tally.missing += block.missing_bytes();
        check.feed(&block);
        match tally.since {
            None => tally.since = Some(Instant::now()),
            Some(_) => tally.bytes += block.len() as u64,
        }
    }
    tally
}

#[test]
#[ignore = "needs an idle RTL-SDR; streams its byte counter"]
fn hardware_counter_stream_is_continuous() {
    let rate = env_or("SDRMM_RTL_TEST_RATE", 2_400_000) as u32;
    let seconds = env_or("SDRMM_RTL_TEST_SECONDS", 30);
    let _awake = sdrmm_device::schedule::stay_awake("RTL counter test");
    sdrmm_device::schedule::claim(sdrmm_device::Latency::Critical);
    let mut dongle = first_dongle();
    dongle.set_sample_rate(rate).expect("rate");
    dongle.set_center(100_000_000).expect("center");
    dongle.set_counter(true).expect("counter mode");
    let mut stream = dongle.start_stream().expect("stream");
    let mut check = Continuity::default();
    let tally = drain_counter(&stream, seconds, &mut check);
    let elapsed = tally
        .since
        .map_or(0.0, |since| since.elapsed().as_secs_f64());
    let stats = stream.stop();
    dongle.set_counter(false).expect("sample mode");
    let measured = tally.bytes as f64 / 2.0 / elapsed.max(f64::EPSILON);
    println!(
        "rate={rate} measured={measured:.1}, discontinuities={}, dropped={}, missing_bytes={}",
        check.breaks, stats.dropped, tally.missing
    );
    assert_eq!(check.breaks, 0);
    assert_eq!(stats.dropped, 0);
    assert_eq!(tally.missing, 0);
    assert!((measured / f64::from(rate) - 1.0).abs() < 0.01);
}

fn collect(stream: &RxStream, wanted: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(wanted);
    let deadline = Instant::now() + Duration::from_secs(5);
    while bytes.len() < wanted && Instant::now() < deadline {
        if let Ok(block) = stream.recv_timeout(Duration::from_millis(200)) {
            bytes.extend_from_slice(&block);
        }
    }
    bytes
}

struct Spread {
    distinct: usize,
    i_std: f64,
    q_std: f64,
    counting: bool,
    iq_same: f64,
}

fn spread(bytes: &[u8]) -> Spread {
    let mut seen = [false; 256];
    for &byte in bytes {
        seen[usize::from(byte)] = true;
    }
    let std = |offset: usize| {
        let lane: Vec<f64> = bytes
            .iter()
            .skip(offset)
            .step_by(2)
            .map(|b| f64::from(*b))
            .collect();
        let mean = lane.iter().sum::<f64>() / lane.len().max(1) as f64;
        (lane.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / lane.len().max(1) as f64).sqrt()
    };
    let counting = bytes
        .windows(2)
        .all(|pair| pair[1] == pair[0].wrapping_add(1));
    let (pairs, _) = bytes.as_chunks::<2>();
    let same = pairs.iter().filter(|[i, q]| i == q).count();
    let iq_same = same as f64 / pairs.len().max(1) as f64;
    Spread {
        distinct: seen.iter().filter(|hit| **hit).count(),
        i_std: std(0),
        q_std: std(1),
        counting,
        iq_same,
    }
}

fn check_live_samples(dongle: &mut Dongle, label: &str, antenna_path: bool) {
    let (mut stream, release) = dongle.prime_stream().expect("hold stream");
    release.go().expect("release");
    let bytes = collect(&stream, 1 << 20);
    let stats = stream.stop();
    let skipped = &bytes[bytes.len().min(1 << 16)..];
    let found = spread(skipped);
    println!(
        "{label}: bytes={} distinct={} i_std={:.2} q_std={:.2} i_eq_q={:.3} dropped={}",
        bytes.len(),
        found.distinct,
        found.i_std,
        found.q_std,
        found.iq_same,
        stats.dropped
    );
    assert!(bytes.len() >= 1 << 20, "{label}: stream stalled");
    assert!(!found.counting, "{label}: still in counter mode");
    if !antenna_path {
        return;
    }
    assert!(found.distinct > 2, "{label}: samples look stuck");
    assert!(found.iq_same < 0.9, "{label}: Q copies I");
    assert!(
        found.i_std > 0.1 && found.q_std > 0.1,
        "{label}: samples look constant"
    );
}

fn check_tuning(dongle: &mut Dongle) {
    for hz in [
        100_000_000,
        433_920_000,
        1_090_000_000,
        30_000_000,
        1_700_000_000,
    ] {
        dongle.set_center(hz).expect("tune");
        let locked = dongle.pll_locked().expect("status");
        println!("center={hz} locked={locked}");
        assert!(locked, "{hz}");
        assert_eq!(dongle.center_hz(), Some(hz));
    }
}

fn check_rates(dongle: &mut Dongle) {
    for rate in [
        250_000, 1_024_000, 2_048_000, 2_560_000, 3_200_000, 2_400_000,
    ] {
        dongle.set_sample_rate(rate).expect("rate");
        println!("rate={rate} actual={}", dongle.sample_rate());
        assert_eq!(dongle.sample_rate(), rate);
        assert!(dongle.pll_locked().expect("status"), "retuned at {rate}");
    }
}

fn level(dongle: &mut Dongle) -> f64 {
    let mut stream = dongle.start_stream().expect("stream");
    let bytes = collect(&stream, 1 << 18);
    stream.stop();
    let found = spread(&bytes[bytes.len().min(1 << 15)..]);
    found.i_std.hypot(found.q_std)
}

fn check_gains(dongle: &mut Dongle) {
    let mut levels = Vec::new();
    for tenths in [0, 297, 496] {
        dongle.set_manual_gain(tenths).expect("manual gain");
        let read = dongle.measured_gain().expect("gain");
        let level = level(dongle);
        println!("manual gain={tenths} level={level:.2} agc_status={read}");
        levels.push(level);
    }
    assert!(
        levels[2] > levels[0] * 1.5,
        "gain does not reach the samples: {levels:?}"
    );
    dongle.set_auto_gain().expect("auto gain");
    std::thread::sleep(Duration::from_millis(100));
    let read = dongle.measured_gain().expect("gain");
    println!("auto gain read={read}");
    assert!((0..=496).contains(&read));
}

fn check_bandwidth_and_ppm(dongle: &mut Dongle) {
    for (bandwidth, if_hz) in [
        (1_000_000, 1_700_000),
        (6_000_000, 3_570_000),
        (0, 1_815_000),
    ] {
        let got = dongle.set_bandwidth(bandwidth).expect("bandwidth");
        println!("bandwidth={bandwidth} if={got}");
        assert_eq!(got, if_hz);
        assert!(dongle.pll_locked().expect("status"));
    }
    for ppm in [50, -50, 0] {
        dongle.set_ppm(ppm).expect("ppm");
        println!("ppm={ppm} locked={}", dongle.pll_locked().expect("status"));
        assert_eq!(dongle.ppm(), ppm);
    }
}

fn check_direct_sampling(dongle: &mut Dongle) {
    if dongle.board().has_upconverter() {
        return;
    }
    dongle
        .set_direct_sampling(DirectSampling::Q)
        .expect("direct q");
    dongle.set_center(7_100_000).expect("direct center");
    check_live_samples(dongle, "direct q 7.1 MHz", false);
    dongle
        .set_direct_sampling(DirectSampling::Off)
        .expect("tuner again");
    dongle.set_center(100_000_000).expect("retune");
    dongle.set_auto_gain().expect("gain");
    dongle.set_bandwidth(0).expect("bandwidth");
    assert!(dongle.pll_locked().expect("status"));
}

#[test]
#[ignore = "needs an idle RTL-SDR; tunes it and reads live samples"]
fn hardware_tunes_and_reads_live_samples() {
    let mut dongle = first_dongle();
    println!("tuner={:?} board={:?}", dongle.tuner_kind(), dongle.board());
    dongle.set_sample_rate(2_048_000).expect("rate");
    dongle.set_auto_gain().expect("auto");
    check_tuning(&mut dongle);
    check_rates(&mut dongle);
    dongle.set_center(100_000_000).expect("tune");
    check_gains(&mut dongle);
    check_bandwidth_and_ppm(&mut dongle);
    dongle.set_bias_tee(false).expect("bias tee");
    check_live_samples(&mut dongle, "100 MHz auto gain", true);
    dongle.set_manual_gain(496).expect("manual");
    dongle.set_center(433_920_000).expect("tune");
    check_live_samples(&mut dongle, "433.92 MHz 49.6 dB", true);
    check_direct_sampling(&mut dongle);
    check_live_samples(&mut dongle, "100 MHz after direct sampling", true);
}
