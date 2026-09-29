use super::*;
use crate::{chdr::Header, rate};

fn timeline() -> Timeline {
    Timeline::new(rate::plan(2.048e6, 1).expect("plan"))
}

fn packet(assembly: &mut Assembly, sid: u32, time: u64, first: u32, samples: usize) -> usize {
    let header = Header {
        kind: Kind::Data,
        seq: 0,
        eob: false,
        sid,
        time: Some(time),
    };
    let start = header
        .write(samples * SAMPLE_BYTES, &mut assembly.datagram)
        .expect("fits");
    for slot in 0..samples {
        chdr::put(&mut assembly.datagram[start..], slot, first + slot as u32);
    }
    start + samples * SAMPLE_BYTES
}

fn overflow(assembly: &mut Assembly, sid: u32) -> usize {
    let header = Header::context(sid, 0);
    let start = header.write(8, &mut assembly.datagram).expect("fits");
    chdr::put(&mut assembly.datagram[start..], 0, 0x08);
    start + 8
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect()
}

#[test]
fn one_lane_takes_the_payload_as_it_came() {
    let mut assembly = Assembly::new(1, 0);
    let n = packet(&mut assembly, RX_STREAM_IDS[0], 64, 10, 3);
    assert_eq!(
        assembly.accept(n).expect("valid"),
        Accepted::Group {
            time: Some(64),
            frames: 3
        }
    );
    assert_eq!(words(&assembly.group), vec![10, 11, 12]);
}

#[test]
fn two_lanes_are_interleaved_sample_by_sample() {
    let mut assembly = Assembly::new(2, 0);
    let n = packet(&mut assembly, RX_STREAM_IDS[1], 64, 20, 2);
    assert_eq!(assembly.accept(n).expect("valid"), Accepted::Nothing);
    let n = packet(&mut assembly, RX_STREAM_IDS[0], 64, 10, 2);
    assert_eq!(
        assembly.accept(n).expect("valid"),
        Accepted::Group {
            time: Some(64),
            frames: 2
        }
    );
    assert_eq!(words(&assembly.group), vec![10, 20, 11, 21]);
}

#[test]
fn a_lane_that_lost_its_packet_drops_its_partner_until_they_meet_again() {
    let mut assembly = Assembly::new(2, 0);
    let n = packet(&mut assembly, RX_STREAM_IDS[0], 0, 1, 2);
    assembly.accept(n).expect("valid");
    let n = packet(&mut assembly, RX_STREAM_IDS[1], 32, 2, 2);
    assert_eq!(assembly.accept(n).expect("valid"), Accepted::Nothing);
    let n = packet(&mut assembly, RX_STREAM_IDS[0], 32, 3, 2);
    assert_eq!(
        assembly.accept(n).expect("valid"),
        Accepted::Group {
            time: Some(32),
            frames: 2
        }
    );
    assert_eq!(words(&assembly.group), vec![3, 2, 4, 3]);
}

#[test]
fn a_packet_for_a_lane_not_streaming_is_ignored() {
    let mut assembly = Assembly::new(1, 0);
    let n = packet(&mut assembly, RX_STREAM_IDS[1], 0, 1, 2);
    assert_eq!(assembly.accept(n).expect("valid"), Accepted::Nothing);
}

#[test]
fn a_timestamp_jump_is_counted_in_samples() {
    let timeline = timeline();
    let mut assembly = Assembly::new(1, timeline.generation());
    let per = timeline.ticks_per_sample();
    assert_eq!(assembly.lost_before(Some(0), 100, &timeline), 0);
    assert_eq!(assembly.lost_before(Some(100 * per), 100, &timeline), 0);
    assert_eq!(assembly.lost_before(Some(250 * per), 100, &timeline), 50);
    assert_eq!(
        assembly.lost_before(Some(10), 100, &timeline),
        0,
        "a reset clock"
    );
}

#[test]
fn a_new_rate_starts_the_count_afresh() {
    let timeline = timeline();
    let mut assembly = Assembly::new(1, timeline.generation());
    assembly.lost_before(Some(0), 100, &timeline);
    timeline.publish(rate::plan(4.096e6, 1).expect("plan"));
    assert_eq!(assembly.lost_before(Some(1_000_000), 100, &timeline), 0);
}

#[test]
fn an_overflow_asks_for_one_restart_per_guard_window() {
    let mut assembly = Assembly::new(2, 0);
    let n = overflow(&mut assembly, RX_STREAM_IDS[0]);
    assert_eq!(assembly.accept(n).expect("valid"), Accepted::Restart);
    let n = overflow(&mut assembly, RX_STREAM_IDS[1]);
    assert_eq!(assembly.accept(n).expect("valid"), Accepted::Nothing);
    assert_eq!(assembly.overflows, 1);
}

#[test]
fn blocks_hold_whole_packets() {
    for rate in [0.0, 1e5, 2.048e6, 25e6, 1e9] {
        let frames = block_frames(rate);
        assert_eq!(frames % PACKET_SAMPLES, 0, "{rate}");
        assert!(frames >= MIN_BLOCK_FRAMES);
    }
}

#[test]
fn the_converter_scales_by_the_stage_the_radio_runs() {
    let timeline = Arc::new(timeline());
    let mut converter = IqConverter::new(timeline.clone(), 4);
    let word: u32 = (16384u32 << 16) | 0xc000;
    let samples = converter.convert(&word.to_le_bytes());
    let expected = 16384.0 * timeline.rx_scale();
    assert_eq!(samples.len(), 1);
    assert!((samples[0].re - expected).abs() < 1e-6);
    assert!((samples[0].im + expected).abs() < 1e-6);
}
