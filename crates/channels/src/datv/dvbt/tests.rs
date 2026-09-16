use sdrmm_wire::DatvCodeRate;

use super::*;
use crate::{testgen, testutil::realtime_budget};

#[test]
fn carrier_grid_and_permutations_match_the_standard() {
    for n in [2048, 8192] {
        let map = mapping::Mapping::new(n);
        assert_eq!(map.tps.len(), 17 * n / 2048);
        assert_eq!(map.continual.len(), 44 * n / 2048 + 1);
        for phase in 0..4 {
            assert_eq!(map.data[phase].len(), 1512 * n / 2048);
        }
        let mut permutation = map.permutation.clone();
        permutation.sort_unstable();
        assert_eq!(permutation, (0..1512 * n / 2048).collect::<Vec<_>>());
    }
    assert_eq!(&mapping::permutation(2048)[..5], &[0, 1024, 16, 1025, 128]);
    assert_eq!(
        mapping::point(0, 4, 1),
        num_complex::Complex::new(3.0, 3.0) / 10.0f32.sqrt()
    );
    assert_eq!(
        mapping::point(0b0011, 4, 1),
        num_complex::Complex::new(1.0, 1.0) / 10.0f32.sqrt()
    );
}

#[test]
fn tps_repairs_two_errors_and_rejects_invalid_modes() {
    let params = testgen::dvbt::defaults();
    let mut bits = tps::encode(params);
    bits[28] = !bits[28];
    bits[53] = !bits[53];
    let mut decoder = tps::Tps::default();
    let mut decoded = None;
    for bit in bits {
        decoded = decoder.push(bit).or(decoded);
    }
    assert_eq!(decoded, Some(params));
}

#[test]
fn every_constellation_and_rate_delivers_transport_packets() {
    for (bits, rate) in [
        (2, DatvCodeRate::Half),
        (4, DatvCodeRate::TwoThirds),
        (6, DatvCodeRate::ThreeQuarters),
        (4, DatvCodeRate::FiveSixths),
        (6, DatvCodeRate::SevenEighths),
    ] {
        let params = tps::Parameters {
            bits,
            high_rate: rate,
            ..testgen::dvbt::defaults()
        };
        let iq = testgen::dvbt::waveform(params, 160);
        let mut decoder = receiver::Receiver::new(false);
        let mut packets = Vec::new();
        for block in iq.chunks(1009) {
            decoder.push(block, &mut packets);
        }
        assert!(
            decoder.locked(),
            "{bits} {rate:?}: {:?} good {} bad {} symbols {}",
            decoder.parameters,
            decoder.metrics().packets_ok,
            decoder.metrics().packets_bad,
            decoder.bad_symbols
        );
        assert!(packets.len() > 20, "{bits} {rate:?}: {}", packets.len());
        let mut demux = crate::datv::ts::TsDemux::new();
        let mut units = Vec::new();
        for packet in packets {
            demux.push(&packet, &mut units);
        }
        assert_eq!(
            demux.program().and_then(|p| p.name.as_deref()),
            Some("Rust TV")
        );
        assert!(units.iter().any(|u| u.pid == 0x101));
    }
}

#[test]
fn eight_kilocarriers_and_all_guards_survive_carrier_offset_and_echoes() {
    for denominator in [4, 8, 16, 32] {
        let params = tps::Parameters {
            fft: 8192,
            guard: 8192 / denominator,
            bits: 4,
            ..testgen::dvbt::defaults()
        };
        let mut iq = testgen::dvbt::waveform(params, 145);
        for i in (17..iq.len()).rev() {
            let echo = iq[i - 17] * num_complex::Complex::new(0.12, 0.06);
            iq[i] += echo;
        }
        testgen::shift(&mut iq, 1234.0, 64_000_000.0 / 7.0);
        testgen::add_noise(&mut iq, 0x92a1, 0.03);
        let mut decoder = receiver::Receiver::new(false);
        let mut packets = Vec::new();
        for block in iq.chunks(8191) {
            decoder.push(block, &mut packets);
        }
        assert!(
            decoder.locked(),
            "guard 1/{denominator} params {:?} packets {} errors {} symbols {}",
            decoder.parameters,
            packets.len(),
            decoder.metrics().packets_bad,
            decoder.bad_symbols
        );
        assert!(packets.len() > 20);
    }
}

#[test]
fn hierarchical_streams_select_high_and_low_priority() {
    for alpha in [1, 2, 4] {
        let params = tps::Parameters {
            bits: 6,
            alpha,
            hierarchical: true,
            ..testgen::dvbt::defaults()
        };
        let iq = testgen::dvbt::waveform(params, 150);
        for low in [false, true] {
            let mut decoder = receiver::Receiver::new(low);
            let mut packets = Vec::new();
            decoder.push(&iq, &mut packets);
            assert!(decoder.locked(), "alpha {alpha} low {low}");
            assert!(packets.len() > 20);
        }
    }
}

#[test]
fn independent_recorded_iq_recovers_exact_transport_payloads() {
    let bytes = include_bytes!("../../../../../fixtures/dvbt/qpsk_2k_reference.sigmf-data");
    let iq: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|s| {
            num_complex::Complex::new(
                i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                i16::from_le_bytes([s[2], s[3]]) as f32 / 32768.0,
            )
        })
        .collect();
    let mut receiver = receiver::Receiver::new(false);
    let mut packets = Vec::new();
    for block in iq.chunks(997) {
        receiver.push(block, &mut packets);
    }
    assert!(
        receiver.locked(),
        "{:?}, packets {}, metrics {:?}, symbols {}",
        receiver.parameters,
        packets.len(),
        receiver.metrics(),
        receiver.bad_symbols
    );
    assert_eq!(receiver.parameters.map(|p| p.cell), Some(0x5a));
    assert!(packets.len() > 30);
    for packet in packets {
        assert_eq!(&packet[..3], &[0x47, 1, 0x23]);
        assert_eq!(packet[3], 0x10 | (packet[4] & 15));
        for (i, &byte) in packet[4..].iter().enumerate() {
            assert_eq!(byte, packet[4].wrapping_add(i as u8));
        }
    }
}

#[test]
fn highest_order_terrestrial_demodulation_keeps_ahead_of_realtime() {
    let params = tps::Parameters {
        fft: 8192,
        guard: 256,
        bits: 6,
        high_rate: DatvCodeRate::SevenEighths,
        ..testgen::dvbt::defaults()
    };
    let iq = testgen::dvbt::waveform(params, 340);
    let mut receiver = receiver::Receiver::new(false);
    let mut packets = Vec::new();
    let start = std::time::Instant::now();
    for block in iq.chunks(8192) {
        packets.clear();
        receiver.push(block, &mut packets);
    }
    let elapsed = start.elapsed().as_secs_f64();
    let duration = iq.len() as f64 / (64_000_000.0 / 7.0);
    assert!(receiver.locked());
    assert!(
        elapsed < realtime_budget(duration),
        "{duration:.3}s of DVB-T took {elapsed:.3}s"
    );
}
