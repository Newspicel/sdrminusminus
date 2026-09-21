use num_complex::Complex;

use super::*;
use crate::datv::dvbs2::{
    bb,
    bch::Bch,
    ldpc::{Frame, Ldpc, Rate},
};

fn coding(frame: Frame, rate: Rate, constellation: Constellation, rotated: bool) -> Coding {
    Coding {
        frame,
        rate,
        constellation,
        rotated,
        lite: matches!(rate, Rate::R1_3 | Rate::R2_5),
    }
}

pub(super) fn encode(coding: Coding, message: &[bool], block: usize) -> Vec<Complex<f32>> {
    let mut scrambled = message.to_vec();
    bb::scramble(&mut scrambled);
    let mut bch = Vec::new();
    Bch::new(coding.frame, coding.correct(), coding.message()).encode(&scrambled, &mut bch);
    let mut code = Vec::new();
    Ldpc::with_addresses(coding.frame, coding.addresses().unwrap())
        .unwrap()
        .encode(&bch, &mut code);
    let map = interleave::bit_permutation(coding).unwrap();
    let bits = coding.constellation.bits();
    let points: Vec<_> = map
        .chunks_exact(bits)
        .map(|indices| {
            let word = indices
                .iter()
                .fold(0, |word, &i| word << 1 | usize::from(code[i]));
            let point = bicm::point(word, coding.constellation);
            if coding.rotated {
                point * Complex::from_polar(1.0, coding.constellation.rotation())
            } else {
                point
            }
        })
        .collect();
    let mut cells = vec![Complex::new(0.0, 0.0); points.len()];
    let map = interleave::cell_permutation(points.len()).unwrap();
    let shift = interleave::cell_shift(points.len(), block).unwrap();
    for (i, &point) in points.iter().enumerate() {
        cells[(map[i] + shift) % points.len()] = if coding.rotated {
            Complex::new(point.re, points[(i + points.len() - 1) % points.len()].im)
        } else {
            point
        };
    }
    cells
}

#[test]
fn every_supported_bicm_profile_decodes_a_nonzero_frame() {
    for frame in [Frame::Short, Frame::Normal] {
        for rate in [
            Rate::R1_3,
            Rate::R2_5,
            Rate::R1_2,
            Rate::R3_5,
            Rate::R2_3,
            Rate::R3_4,
            Rate::R4_5,
            Rate::R5_6,
        ] {
            if frame == Frame::Normal && matches!(rate, Rate::R1_3 | Rate::R2_5) {
                continue;
            }
            for constellation in [
                Constellation::Qpsk,
                Constellation::Qam16,
                Constellation::Qam64,
                Constellation::Qam256,
            ] {
                let coding = coding(
                    frame,
                    rate,
                    constellation,
                    constellation != Constellation::Qam256 || frame == Frame::Normal,
                );
                let message: Vec<_> = (0..coding.message())
                    .map(|i| (i * 173 + i / 7) % 31 < 15)
                    .collect();
                let mut signal = encode(coding, &message, 3);
                for (i, point) in signal.iter_mut().enumerate() {
                    *point += Complex::new((i as f32 * 1.7).sin(), (i as f32 * 0.3).cos()) * 0.001;
                }
                let mut decoder = bicm::Decoder::new(coding).unwrap();
                let mut output = vec![false; coding.message()];
                let decoded = decoder
                    .decode(&signal, 3, 0.001, &mut output)
                    .unwrap_or_else(|error| panic!("{coding:?}: {error}"));
                assert_eq!(decoded.bits, message.len());
                assert_eq!(output, message, "{coding:?}");
            }
        }
    }
}

#[test]
fn constellation_mapping_matches_etsi_table_14() {
    for (constellation, words, amplitudes, scale) in [
        (
            Constellation::Qam16,
            vec![0b1000, 0b1010, 0b0010, 0],
            vec![-3., -1., 1., 3.],
            10_f32.sqrt(),
        ),
        (
            Constellation::Qam64,
            vec![
                0b100000, 0b100010, 0b101010, 0b101000, 0b001000, 0b001010, 0b000010, 0,
            ],
            vec![-7., -5., -3., -1., 1., 3., 5., 7.],
            42_f32.sqrt(),
        ),
    ] {
        for (word, amplitude) in words.into_iter().zip(amplitudes) {
            assert!((bicm::point(word, constellation).re * scale - amplitude).abs() < 1e-5);
        }
    }
    for constellation in [
        Constellation::Qpsk,
        Constellation::Qam16,
        Constellation::Qam64,
        Constellation::Qam256,
    ] {
        let count = 1 << constellation.bits();
        let power: f32 = (0..count)
            .map(|i| bicm::point(i, constellation).norm_sqr())
            .sum();
        assert!((power / count as f32 - 1.0).abs() < 1e-5);
    }
}

#[test]
fn permutations_match_standard_addresses_and_are_bijective() {
    assert_eq!(
        (0..8)
            .map(|i| interleave::cell_shift(10800, i).unwrap())
            .collect::<Vec<_>>(),
        [0, 8192, 4096, 2048, 10240, 6144, 1024, 9216]
    );
    for cells in [2025, 2700, 4050, 8100, 10800, 16200, 32400] {
        let mut map = interleave::cell_permutation(cells).unwrap();
        assert_eq!(&map[..3], &[0, cells.next_power_of_two() / 2, 1]);
        map.sort_unstable();
        assert_eq!(map, (0..cells).collect::<Vec<_>>());
    }
    let config = coding(Frame::Normal, Rate::R1_2, Constellation::Qam64, false);
    let map = interleave::bit_permutation(config).unwrap();
    assert_eq!(map[11], 0);
    assert_eq!(map[7], 5400);
    assert_eq!(map[3], 16198);
    assert_eq!(map[10], 21598);
    let mut sorted = map;
    sorted.sort_unstable();
    assert_eq!(sorted, (0..64800).collect::<Vec<_>>());
    assert!(interleave::cell_permutation(10).is_err());
    assert!(interleave::cell_shift(8100, 1023).is_err());
}

#[test]
fn time_deinterleaving_preserves_all_blocks_and_uneven_groups() {
    assert_eq!(
        (0..3)
            .map(|i| interleave::time_block_size(8, 3, i).unwrap())
            .collect::<Vec<_>>(),
        [2, 3, 3]
    );
    let cells = 2025;
    let blocks = 3;
    let rows = cells / 5;
    let columns = blocks * 5;
    let source: Vec<_> = (0..cells * blocks)
        .map(|i| Complex::new(i as f32, -(i as f32)))
        .collect();
    let transmitted: Vec<_> = (0..rows)
        .flat_map(|row| (0..columns).map(move |col| col * rows + row))
        .map(|i| source[i])
        .collect();
    let mut output = vec![Complex::default(); source.len()];
    interleave::time_deinterleave(&transmitted, &mut output, cells).unwrap();
    assert_eq!(output, source);
    assert_eq!(
        interleave::time_deinterleave(&transmitted[..1], &mut output, cells),
        Err(DecodeError::Length)
    );
}

#[test]
fn invalid_profiles_and_buffers_are_rejected() {
    let mut config = coding(Frame::Normal, Rate::R1_2, Constellation::Qpsk, false);
    config.lite = true;
    assert_eq!(config.validate(), Err(DecodeError::Parameters));
    config.frame = Frame::Short;
    config.constellation = Constellation::Qam256;
    config.rotated = true;
    assert!(config.validate().is_err());
    config.rotated = false;
    config.rate = Rate::R2_3;
    assert!(config.validate().is_err());
    config.constellation = Constellation::Qpsk;
    let mut decoder = bicm::Decoder::new(config).unwrap();
    let mut output = vec![false; config.message()];
    assert_eq!(
        decoder.decode(&[], 0, 1.0, &mut output),
        Err(DecodeError::Length)
    );
    let invalid = vec![Complex::new(f32::NAN, 0.0); config.cells()];
    assert_eq!(
        decoder.decode(&invalid, 0, 1.0, &mut output),
        Err(DecodeError::NonFinite)
    );
}

#[test]
fn terrestrial_tables_are_distinct_from_satellite_codes() {
    assert_eq!(&tables::NORMAL_R2_3[0][..4], &[317, 2255, 2324, 2723]);
    assert_eq!(&tables::SHORT_R3_5[0][..4], &[71, 1478, 1901, 2240]);
    assert_eq!(tables::NORMAL_R3_5.len(), 108);
    assert_eq!(tables::NORMAL_R2_3.len(), 120);
    assert_eq!(tables::SHORT_R3_5.len(), 27);
    for config in [
        coding(Frame::Normal, Rate::R3_5, Constellation::Qpsk, false),
        coding(Frame::Normal, Rate::R2_3, Constellation::Qpsk, false),
        coding(Frame::Short, Rate::R3_5, Constellation::Qpsk, false),
    ] {
        assert_ne!(
            config.addresses().unwrap(),
            config.rate.addresses(config.frame).unwrap()
        );
    }
}

mod transport_tests;

#[test]
fn axis_demapping_matches_exhaustive_maxlog_distances() {
    for constellation in [
        Constellation::Qpsk,
        Constellation::Qam16,
        Constellation::Qam64,
        Constellation::Qam256,
    ] {
        let bits = constellation.bits();
        let points: Vec<_> = (0..1 << bits)
            .map(|word| bicm::point(word, constellation))
            .collect();
        for i in 0..128 {
            let sample = Complex::new(
                (i as f32 * 0.137).sin() * 1.8,
                (i as f32 * 0.281).cos() * 1.8,
            );
            let actual = bicm::soften(sample, &points, bits, 0.15);
            for (bit, &llr) in actual[..bits].iter().enumerate() {
                let mut minimum = [f32::INFINITY; 2];
                for (word, &point) in points.iter().enumerate() {
                    let value = word >> (bits - 1 - bit) & 1;
                    minimum[value] = minimum[value].min((sample - point).norm_sqr());
                }
                let expected = ((minimum[1] - minimum[0]) / 0.15).clamp(-32.0, 32.0);
                assert!(
                    (llr - expected).abs() < 0.0001,
                    "{constellation:?} {sample}: {llr} {expected}"
                );
            }
        }
    }
}
