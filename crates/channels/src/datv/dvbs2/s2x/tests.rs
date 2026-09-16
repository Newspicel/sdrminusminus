use super::*;
use crate::datv::{
    dvbs2::{
        pl,
        receiver::{Dvbs2Decoder, Dvbs2Encoder, Dvbs2Output},
    },
    ts,
};

#[test]
fn extended_modes_have_valid_constellations_codes_and_inverse_interleavers() {
    for mode in points::MODES {
        let mut header = Vec::new();
        let signal = pl::Signalling {
            modcod: mode.code,
            short: mode.short,
            pilots: true,
        };
        pl::header(signal, &mut header);
        assert_eq!(pl::read_signalling(&header), Some(signal));
        let count = mode.points.len();
        assert_eq!(count, 1 << mode.modulation.bits());
        let power = mode.points.iter().map(|&(i, q)| i * i + q * q).sum::<f32>() / count as f32;
        assert!((power - 1.0).abs() < 0.01, "{}: power {power}", mode.code);
        let length = Frame::of(mode.short).length();
        let coded: Vec<bool> = (0..length).map(|i| ((i * 931 + i / 13) % 17) < 8).collect();
        let ordered = mode.interleave(&coded);
        assert_eq!(
            ordered.len(),
            mode.modcod().slots(mode.short) * 90 * mode.modulation.bits()
        );
        let soft: Vec<f32> = ordered
            .iter()
            .map(|&bit| if bit { -1.0 } else { 1.0 })
            .collect();
        assert_eq!(
            mode.deinterleave(&soft)
                .iter()
                .map(|&value| value < 0.0)
                .collect::<Vec<_>>(),
            coded
        );
        assert!(mode.rate.information(Frame::of(mode.short)) > 0);
    }
}

#[test]
fn every_extended_mode_delivers_crc_checked_transport_packets() {
    for mode in points::MODES {
        let mut encoder = Dvbs2Encoder::new(mode.modcod(), mode.short, true).expect("S2X encoder");
        let packet = ts::null_packet();
        let packets = vec![packet; encoder.capacity()];
        let mut symbols = Vec::new();
        assert!(encoder.frame(&packets, &mut symbols));
        let mut receiver = Dvbs2Decoder::new();
        let mut output = Dvbs2Output::default();
        for chunk in symbols.chunks(1301) {
            receiver.push(chunk, &mut output);
        }
        assert_eq!(
            receiver.metrics.frames_ok, 1,
            "PLS {}: {:?}",
            mode.code, receiver.metrics
        );
        assert!(!output.packets.is_empty(), "PLS {}", mode.code);
        assert!(
            output
                .packets
                .iter()
                .all(|packet| packet == &ts::null_packet()),
            "PLS {}",
            mode.code
        );
    }
}

#[test]
fn independent_recordings_decode_the_expected_transport_bytes() {
    for (code, data) in [
        (
            132,
            include_bytes!("../../../../../../fixtures/dvbs2x/pls132.sigmf-data").as_slice(),
        ),
        (
            138,
            include_bytes!("../../../../../../fixtures/dvbs2x/pls138.sigmf-data").as_slice(),
        ),
        (
            184,
            include_bytes!("../../../../../../fixtures/dvbs2x/pls184.sigmf-data").as_slice(),
        ),
        (
            200,
            include_bytes!("../../../../../../fixtures/dvbs2x/pls200.sigmf-data").as_slice(),
        ),
        (
            214,
            include_bytes!("../../../../../../fixtures/dvbs2x/pls214.sigmf-data").as_slice(),
        ),
        (
            248,
            include_bytes!("../../../../../../fixtures/dvbs2x/pls248.sigmf-data").as_slice(),
        ),
    ] {
        let symbols: Vec<_> = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|sample| {
                num_complex::Complex::new(
                    f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 16000.0,
                    f32::from(i16::from_le_bytes([sample[2], sample[3]])) / 16000.0,
                )
            })
            .collect();
        let mut receiver = Dvbs2Decoder::new();
        let mut output = Dvbs2Output::default();
        for chunk in symbols.chunks(719) {
            receiver.push(chunk, &mut output);
        }
        assert_eq!(
            receiver.metrics.frames_ok, 1,
            "PLS {code}: {:?}",
            receiver.metrics
        );
        assert!(output.packets.len() >= 6);
        for (index, packet) in output.packets.iter().enumerate() {
            assert_eq!(&packet[..4], &[0x47, 1, 0x23, 0x10 | (index & 15) as u8]);
            for (byte, &value) in packet[4..].iter().enumerate() {
                assert_eq!(
                    value,
                    (code + index + byte) as u8,
                    "PLS {code}, packet {index}, byte {byte}"
                );
            }
        }
    }
}

#[test]
fn reserved_headers_skip_their_announced_lengths_and_keep_the_next_frame() {
    let mode = mode(248).unwrap();
    let mut encoder = Dvbs2Encoder::new(mode.modcod(), true, false).unwrap();
    let packets = vec![ts::null_packet(); encoder.capacity()];
    let mut valid = Vec::new();
    assert!(encoder.frame(&packets, &mut valid));
    for code in [
        128, 130, 176, 177, 188, 189, 192, 193, 196, 197, 250, 251, 252, 253, 254, 255,
    ] {
        let mut symbols = Vec::new();
        pl::header(pl::Signalling::from_code(code), &mut symbols);
        symbols.extend_from_slice(&valid);
        symbols.resize(
            reserved_symbols(code).unwrap(),
            num_complex::Complex::new(0.0, 0.0),
        );
        symbols.extend_from_slice(&valid);
        let mut receiver = Dvbs2Decoder::new();
        let mut output = Dvbs2Output::default();
        for chunk in symbols.chunks(509) {
            receiver.push(chunk, &mut output);
        }
        assert_eq!(receiver.metrics.frames_skipped, 1, "PLS {code}");
        assert_eq!(receiver.metrics.frames_ok, 1, "PLS {code}");
        assert_eq!(output.packets.len(), packets.len() - 1);
    }
}

#[test]
fn grouped_soft_decisions_match_the_exhaustive_bit_metric() {
    for mode in points::MODES {
        let constellation = mode.constellation();
        let samples: Vec<_> = mode
            .points
            .iter()
            .map(|&(i, q)| num_complex::Complex::new(i + 0.023, q - 0.019))
            .collect();
        let mut soft = Vec::new();
        crate::datv::dvbs2::frame::demodulate(&samples, &constellation, 0.3, &mut soft);
        for (sample, actual) in samples
            .iter()
            .zip(soft.chunks_exact(mode.modulation.bits()))
        {
            for (bit, &value) in actual.iter().enumerate() {
                let mut maxima = [f32::NEG_INFINITY; 2];
                for (label, &(i, q)) in mode.points.iter().enumerate() {
                    let index = (label >> (mode.modulation.bits() - 1 - bit)) & 1;
                    maxima[index] = maxima[index]
                        .max(-(*sample - num_complex::Complex::new(i, q)).norm_sqr() / 0.3);
                }
                assert!((value - (maxima[0] - maxima[1])).abs() < 1e-5);
            }
        }
    }
}
