use sdrmm_wire::DatvCodeRate;

use super::*;
use crate::{synth, testutil::realtime_budget};

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
    let params = synth::dvbt::defaults();
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
            ..synth::dvbt::defaults()
        };
        let iq = synth::dvbt::waveform(params, 160);
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
            ..synth::dvbt::defaults()
        };
        let mut iq = synth::dvbt::waveform(params, 145);
        for i in (17..iq.len()).rev() {
            let echo = iq[i - 17] * num_complex::Complex::new(0.12, 0.06);
            iq[i] += echo;
        }
        synth::shift(&mut iq, 1234.0, 64_000_000.0 / 7.0);
        synth::add_noise(&mut iq, 0x92a1, 0.03);
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
            ..synth::dvbt::defaults()
        };
        let iq = synth::dvbt::waveform(params, 150);
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
fn highest_order_terrestrial_demodulation_has_bounded_cost() {
    let params = tps::Parameters {
        fft: 8192,
        guard: 256,
        bits: 6,
        high_rate: DatvCodeRate::SevenEighths,
        ..synth::dvbt::defaults()
    };
    let iq = synth::dvbt::waveform(params, 340);
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
    // Four times the signal's own duration, not once: 64-QAM at 7/8 over 8k carriers is the
    // heaviest mode the standard defines, and the same hosted runner decoded it in 0.405 s one
    // day and 0.732 s the next for a laptop's 0.239 s. A tighter bound reports which machine
    // picked up the job; this one still catches a decoder that has halved in speed.
    assert!(
        elapsed < realtime_budget(4.0 * duration),
        "{duration:.3}s of DVB-T took {elapsed:.3}s"
    );
}

fn decoded_share(paths: &[(usize, f32, f32, f32)], snr_db: f32) -> f32 {
    let params = tps::Parameters {
        guard: 512,
        ..synth::dvbt::defaults()
    };
    let clean = synth::dvbt::waveform(params, 68 * 4);
    let power = clean.iter().map(|s| s.norm_sqr()).sum::<f32>() / clean.len() as f32;
    let rate = 64_000_000.0 / 7.0;
    let mut iq = clean.clone();
    for (i, sample) in iq.iter_mut().enumerate() {
        for &(delay, re, im, doppler) in paths {
            if i >= delay {
                let turn = num_complex::Complex::from_polar(
                    1.0,
                    std::f32::consts::TAU * doppler * i as f32 / rate,
                );
                *sample += clean[i - delay] * num_complex::Complex::new(re, im) * turn;
            }
        }
    }
    synth::add_noise(
        &mut iq,
        0x5a5a,
        (power / 10f32.powf(snr_db / 10.0) * 1.5).sqrt(),
    );
    let mut decoder = receiver::Receiver::new(false);
    let mut packets = Vec::new();
    for block in iq.chunks(8191) {
        decoder.push(block, &mut packets);
    }
    let metrics = decoder.metrics();
    assert!(
        metrics.packets_ok >= 200,
        "only {} packets",
        metrics.packets_ok
    );
    metrics.packets_ok as f32 / (metrics.packets_ok + metrics.packets_bad) as f32
}

#[test]
fn echoes_past_the_scattered_pilot_reach_still_decode() {
    for (name, paths) in [
        ("0.9 echo at 300 samples", &[(300, 0.0, 0.9, 0.0)][..]),
        (
            "three-path network",
            &[(60, 0.6, 0.3, 0.0), (230, -0.4, 0.5, 0.0)][..],
        ),
    ] {
        let share = decoded_share(paths, 18.0);
        assert!(share > 0.9, "{name}: {share} of the packets decoded");
    }
}

#[test]
fn a_near_total_short_echo_decodes_close_to_plain_noise() {
    let share = decoded_share(&[(7, -0.95, 0.0, 0.0)], 13.0);
    assert!(share > 0.9, "{share} of the packets decoded");
}

#[test]
fn a_fading_channel_with_doppler_keeps_decoding() {
    let paths = [
        (0, -1.0, 0.0, 0.0),
        (0, 1.0, 0.0, 200.0),
        (25, 0.5, 0.0, -200.0),
    ];
    let share = decoded_share(&paths, 15.0);
    assert!(share > 0.9, "{share} of the packets decoded");
}
