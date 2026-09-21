use std::f32::consts::TAU;

use num_complex::Complex;
use rustfft::FftPlanner;

use super::{
    acquire::{Acquisition, Preamble},
    mapping::Mapping,
    p1_tables::*,
    signalling::Pre,
};

pub(super) fn p1(preamble: Preamble) -> Vec<Complex<f32>> {
    let mut spectrum = vec![Complex::default(); 1024];
    let mut differential = 1.0;
    let mut state = 0x4e46_u16;
    for (i, &carrier) in P1_ACTIVE_CARRIERS.iter().enumerate() {
        let bit = match i {
            0..64 => S1_PATTERNS[usize::from(preamble.s1)][i / 8] >> (7 - i % 8) & 1,
            64..320 => S2_PATTERNS[usize::from(preamble.s2)][(i - 64) / 8] >> (7 - i % 8) & 1,
            _ => S1_PATTERNS[usize::from(preamble.s1)][(i - 320) / 8] >> (7 - i % 8) & 1,
        };
        if bit != 0 {
            differential *= -1.0;
        }
        let prbs = (state ^ (state >> 1)) & 1;
        state = (state >> 1) | (prbs << 14);
        spectrum[(carrier + 1024 - 426) % 1024] =
            Complex::new(differential * if prbs != 0 { -1.0 } else { 1.0 }, 0.0);
    }
    FftPlanner::new()
        .plan_fft_inverse(1024)
        .process(&mut spectrum);
    (0..2048)
        .map(|i| {
            let (sample, shifted) = match i {
                0..542 => (spectrum[i], true),
                542..1566 => (spectrum[i - 542], false),
                _ => (spectrum[i - 1024], true),
            };
            sample / 384.0_f32.sqrt()
                * if shifted {
                    Complex::from_polar(1.0, TAU * i as f32 / 1024.0)
                } else {
                    Complex::new(1.0, 0.0)
                }
        })
        .collect()
}

pub(super) fn pre(fft: usize, pilots: u8, extended: bool) -> Pre {
    let s2 = match fft {
        1024 => 6,
        2048 => 0,
        4096 => 4,
        8192 => 2,
        16384 => 8,
        _ => 10,
    };
    Pre {
        preamble: Preamble { s1: 0, s2 },
        extended,
        repetition: false,
        guard_code: 0,
        papr: 0,
        modulation: 0,
        post_cells: 0,
        post_info: 0,
        pilots,
        frames: 2,
        data_symbols: 100,
        rf_count: 1,
        rf_index: 0,
        version: 2,
        scrambled: false,
        extension: false,
    }
}

#[test]
fn p1_acquires_all_preamble_codes_with_fractional_and_integer_offsets() {
    let mut acquisition = Acquisition::default();
    for s1 in 0..5 {
        for s2 in 0..16 {
            let preamble = Preamble { s1, s2 };
            let signal = p1(preamble);
            let offset = -3.7 * TAU / 1024.0;
            let mut samples = vec![Complex::default(); 71];
            samples.extend(
                signal
                    .iter()
                    .enumerate()
                    .map(|(i, &p)| p * Complex::from_polar(0.3, offset * i as f32 + 1.4)),
            );
            samples.extend_from_slice(&[Complex::default(); 64]);
            let detection = acquisition
                .find(&samples)
                .unwrap_or_else(|| panic!("{preamble:?}"));
            assert_eq!(detection.preamble, preamble);
            assert_eq!(detection.start, 71);
            assert!((detection.frequency - offset).abs() < 1e-5);
            assert!(detection.confidence > 0.98);
        }
    }
}

#[test]
fn p1_ignores_silence_and_nonfinite_samples() {
    let mut acquisition = Acquisition::default();
    assert!(acquisition.find(&vec![Complex::default(); 4096]).is_none());
    assert!(
        acquisition
            .find(&vec![Complex::new(f32::NAN, 0.0); 4096])
            .is_none()
    );
}

#[test]
fn p2_carrier_counts_match_standard_in_siso_and_miso() {
    for (fft, siso, miso) in [
        (1024, 558, 546),
        (2048, 1118, 1098),
        (4096, 2236, 2198),
        (8192, 4472, 4398),
        (16384, 8944, 8814),
        (32768, 22432, 17612),
    ] {
        let mut mapping = Mapping::new(fft).unwrap();
        for (s1, count) in [(0, siso), (1, miso)] {
            let mut p = pre(fft, 1, false).preamble;
            p.s1 = s1;
            mapping.p2(p, 0).unwrap();
            assert_eq!(mapping.data, count, "{fft}, {s1}");
            for symbol in 0..2 {
                let input: Vec<_> = (0..count).collect();
                let mut output = vec![0; count];
                mapping.deinterleave(&input, symbol, &mut output).unwrap();
                output.sort_unstable();
                assert_eq!(output, input);
            }
        }
    }
}

#[test]
fn data_pilot_counts_match_standard_for_every_symbol_phase() {
    for (fft, extended, counts) in [
        (1024, false, [764, 768, 798, 804, 818, 0, 0, 0]),
        (2048, false, [1522, 1532, 1596, 1602, 1632, 0, 1646, 0]),
        (4096, false, [3084, 3092, 3228, 3234, 3298, 0, 3328, 0]),
        (8192, false, [6208, 6214, 6494, 6498, 6634, 0, 6698, 6698]),
        (8192, true, [6296, 6298, 6584, 6588, 6728, 0, 6788, 6788]),
        (
            16384,
            false,
            [12418, 12436, 12988, 13002, 13272, 13288, 13416, 13406],
        ),
        (
            16384,
            true,
            [12678, 12698, 13262, 13276, 13552, 13568, 13698, 13688],
        ),
        (32768, false, [0, 24886, 0, 26022, 0, 26592, 26836, 26812]),
        (32768, true, [0, 25412, 0, 26572, 0, 27152, 27404, 27376]),
    ] {
        let mut mapping = Mapping::new(fft).unwrap();
        for (pattern, &count) in counts.iter().enumerate() {
            if count == 0 {
                continue;
            }
            let mut p = pre(fft, pattern as u8 + 1, extended);
            for papr in [0, 2] {
                p.papr = papr;
                for symbol in 16..32 {
                    mapping.data(p, symbol).unwrap();
                    let reserved = if papr == 0 {
                        0
                    } else {
                        super::pilot_tables::P2_TONES[fft.ilog2() as usize - 10].len()
                    };
                    assert_eq!(
                        mapping.data,
                        count - reserved,
                        "fft={fft} extended={extended} pp={} symbol={symbol} papr={papr}",
                        pattern + 1
                    );
                }
            }
        }
    }
}

#[test]
fn equalizer_recovers_siso_and_miso_through_two_complex_channels() {
    use super::{equalize::Equalizer, mapping::Carrier};
    for fft in [1024, 2048, 4096, 8192, 16384, 32768] {
        let mut map = Mapping::new(fft).unwrap();
        let mut equalizer = Equalizer::default();
        for miso in [false, true] {
            let mut preamble = pre(fft, 1, true).preamble;
            preamble.s1 = u8::from(miso);
            map.p2(preamble, 0).unwrap();
            equalizer.reset();
            let mut spectrum = vec![Complex::default(); fft];
            let mut expected = Vec::new();
            let h1 = Complex::new(0.7, 0.3);
            let h2 = if miso {
                Complex::new(-0.2, 0.4)
            } else {
                Complex::default()
            };
            let mut index = 0;
            for k in 0..map.carriers {
                let value = match map.map[k] {
                    Carrier::Pilot { inverted, .. } => {
                        map.pilots[k] * (h1 + if inverted { -h2 } else { h2 })
                    }
                    Carrier::Data => {
                        let point = |i: usize| {
                            Complex::new((i % 7) as f32 / 3.0 - 1.0, (i % 11) as f32 / 5.0 - 1.0)
                        };
                        let p = point(index);
                        expected.push(p);
                        let other = if index % 2 == 0 {
                            -point(index + 1).conj()
                        } else {
                            point(index - 1).conj()
                        };
                        index += 1;
                        h1 * p + h2 * other
                    }
                    Carrier::Reserved => Complex::default(),
                };
                spectrum[(k + fft - map.carriers / 2) % fft] = value;
            }
            let mut actual = vec![Complex::default(); map.data];
            equalizer
                .decode(&spectrum, &map, miso, 0, &mut actual)
                .unwrap();
            for (a, b) in actual.iter().zip(&expected) {
                assert!((*a - *b).norm() < 1e-5, "fft={fft} miso={miso} {a} {b}");
            }
        }
    }
}
