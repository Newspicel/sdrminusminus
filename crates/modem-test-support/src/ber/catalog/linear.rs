use num_complex::Complex;
use sdrmm_modem::{
    constellation::{Constellation, ConstellationError},
    linear::{
        CarrierLoop, EnvelopeDemod, EnvelopeTiming, LinearBurstDemod, LinearDemod, LinearMod,
        LinearParams, LinearTiming, PhaseAnchor, differential_detect, slice_amplitude,
    },
    pulse::{self, Norm},
    symbolcode::{DifferentialSymbolDecoder, DifferentialSymbolEncoder},
};

use crate::ber::sweep::Link;

pub const BAUD: f64 = 6_000.0;
pub const SPS: usize = 8;
pub const RATE: f64 = BAUD * SPS as f64;

pub const ALPHA: f64 = 0.35;
pub const SPAN: usize = 8;

pub const PREAMBLE: usize = 512;
pub const UW: usize = 32;
pub const TAIL: usize = 16;
pub const PAYLOAD_SYMBOLS: usize = 8_192;

pub const OVERHEAD: usize = PREAMBLE + UW + TAIL;

pub const POWER_SYMBOLS: f64 = f64::INFINITY;

pub const FULL_CAP: u64 = 4_000_000;

pub const FILLER_SEED: u32 = 0x9e37_79b9;

#[must_use]
pub fn rrc() -> Vec<f32> {
    pulse::root_raised_cosine(SPS as f64, ALPHA, SPAN, Norm::Energy)
}

#[must_use]
pub fn table(what: &str, built: Result<Constellation, ConstellationError>) -> Constellation {
    match built {
        Ok(t) => t,
        Err(why) => panic!("catalog entry `{what}`: {why}"),
    }
}

#[must_use]
pub fn params(
    table: Result<Constellation, ConstellationError>,
    rotation_rad: f64,
    offset: bool,
) -> LinearParams {
    let built = table
        .map_err(|e| e.to_string())
        .and_then(|t| LinearParams::new(t, rrc(), SPS).map_err(|e| e.to_string()))
        .and_then(|p| p.with_rotation(rotation_rad).map_err(|e| e.to_string()))
        .and_then(|p| p.with_offset(offset).map_err(|e| e.to_string()));
    match built {
        Ok(p) => p,
        Err(why) => panic!("catalog entry parameters: {why}"),
    }
}

#[must_use]
pub fn bits_to_labels(bits: &[bool], bits_per_symbol: usize) -> Vec<u32> {
    bits.chunks(bits_per_symbol)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u32, |acc, (i, &b)| acc | (u32::from(b) << i))
        })
        .collect()
}

#[must_use]
pub fn labels_to_bits(labels: &[u32], bits_per_symbol: usize) -> Vec<bool> {
    let mut bits = Vec::with_capacity(labels.len() * bits_per_symbol);
    for &label in labels {
        for i in 0..bits_per_symbol {
            bits.push((label >> i) & 1 == 1);
        }
    }
    bits
}

fn filler_from(state: &mut u32, len: usize, m: u32) -> Vec<u32> {
    (0..len)
        .map(|_| {
            *state ^= *state << 13;
            *state ^= *state >> 17;
            *state ^= *state << 5;
            *state % m
        })
        .collect()
}

#[must_use]
pub fn data_like(table: &Constellation, len: usize, seed: u32) -> Vec<u32> {
    filler_from(&mut { seed }, len, table.len() as u32)
}

#[must_use]
pub fn shell_labels(table: &Constellation) -> Vec<u32> {
    let radii: Vec<f64> = table.points().iter().map(|p| f64::from(p.norm())).collect();
    let mut best: Option<(usize, f64)> = None;
    for &r in &radii {
        let count = radii.iter().filter(|&&q| (q - r).abs() <= 1e-3 * r).count();
        if best.is_none_or(|(n, best_r)| count > n || (count == n && r > best_r)) {
            best = Some((count, r));
        }
    }
    let (_, radius) = best.unwrap_or((0, 0.0));
    table
        .labels()
        .iter()
        .zip(&radii)
        .filter(|&(_, &r)| (r - radius).abs() <= 1e-3 * radius)
        .map(|(&l, _)| l)
        .collect()
}

const WORD_CANDIDATES: u32 = 256;

#[must_use]
pub fn unique_word(table: &Constellation, len: usize, seed: u32) -> Vec<u32> {
    let shell = shell_labels(table);
    let alphabet: Vec<u32> = if shell.len() >= 2 {
        shell
    } else {
        table.labels().to_vec()
    };
    let draw = |candidate: u32| -> Vec<u32> {
        let picks = filler_from(
            &mut seed.wrapping_add(candidate),
            len,
            alphabet.len() as u32,
        );
        picks.into_iter().map(|i| alphabet[i as usize]).collect()
    };
    let points_of_labels = |labels: &[u32]| -> Vec<Complex<f32>> {
        labels
            .iter()
            .map(|&l| {
                let i = table
                    .labels()
                    .iter()
                    .position(|&x| x == l)
                    .unwrap_or_default();
                table.points()[i]
            })
            .collect()
    };
    (0..WORD_CANDIDATES)
        .map(draw)
        .min_by(|a, b| {
            worst_sidelobe(&points_of_labels(a)).total_cmp(&worst_sidelobe(&points_of_labels(b)))
        })
        .unwrap_or_default()
}

#[must_use]
pub fn worst_sidelobe(word: &[Complex<f32>]) -> f64 {
    let n = word.len();
    let energy: f64 = word.iter().map(|x| f64::from(x.norm_sqr())).sum();
    if energy <= 0.0 || n < 2 {
        return f64::INFINITY;
    }
    let mut worst = 0.0f64;
    for shift in 1..n {
        let mut acc = Complex::new(0.0f64, 0.0);
        for k in 0..(n - shift) {
            let a = word[k + shift];
            let b = word[k];
            acc += Complex::new(f64::from(a.re), f64::from(a.im))
                * Complex::new(f64::from(b.re), -f64::from(b.im));
        }
        worst = worst.max(acc.norm() / energy);
    }
    worst
}

#[must_use]
pub fn table_points(table: &Constellation, labels: &[u32]) -> Vec<Complex<f32>> {
    labels
        .iter()
        .map(|&l| {
            let i = table
                .labels()
                .iter()
                .position(|&x| x == l)
                .unwrap_or_default();
            table.points()[i]
        })
        .collect()
}

#[must_use]
pub fn find_word(
    symbols: &[Complex<f32>],
    lo: usize,
    hi: usize,
    expected: &[Complex<f32>],
) -> Option<usize> {
    let last = hi.min(symbols.len().checked_sub(expected.len())?);
    let word_energy: f64 = expected.iter().map(|x| f64::from(x.norm_sqr())).sum();
    if word_energy <= 0.0 {
        return None;
    }
    let score = |at: usize| -> f64 {
        let mut acc = Complex::new(0.0f64, 0.0);
        let mut energy = 0.0f64;
        for (i, &x) in expected.iter().enumerate() {
            let y = symbols[at + i];
            acc += Complex::new(f64::from(y.re), f64::from(y.im))
                * Complex::new(f64::from(x.re), -f64::from(x.im));
            energy += f64::from(y.norm_sqr());
        }
        if energy <= 0.0 {
            return 0.0;
        }
        acc.norm_sqr() / (energy * word_energy)
    };
    (lo..=last).max_by(|&a, &b| score(a).total_cmp(&score(b)))
}

#[must_use]
pub fn find_word_amplitude(
    amplitudes: &[f32],
    lo: usize,
    hi: usize,
    expected: &[f32],
) -> Option<usize> {
    let last = hi.min(amplitudes.len().checked_sub(expected.len())?);
    let misfit = |at: usize| -> f64 {
        expected
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let d = f64::from(amplitudes[at + i]) - f64::from(x);
                d * d
            })
            .sum()
    };
    (lo..=last).min_by(|&a, &b| misfit(a).total_cmp(&misfit(b)))
}

fn frame(table: &Constellation, uw: &[u32], payload: &[u32]) -> Vec<u32> {
    let mut state = FILLER_SEED;
    let m = table.len() as u32;
    let mut s = filler_from(&mut state, PREAMBLE, m);
    s.extend_from_slice(uw);
    s.extend_from_slice(payload);
    s.extend(filler_from(&mut state, TAIL, m));
    s
}

#[must_use]
pub fn coherent_link(
    label: &str,
    params: LinearParams,
    carrier: impl Fn() -> Option<CarrierLoop> + 'static,
) -> Link {
    coherent_link_with_power(label, params, carrier, POWER_SYMBOLS)
}

#[must_use]
pub fn coherent_link_with_power(
    label: &str,
    params: LinearParams,
    carrier: impl Fn() -> Option<CarrierLoop> + 'static,
    power_symbols: f64,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let payload = bits_to_labels(bits, bits_per_symbol);
            let uw = unique_word(table, UW, FILLER_SEED);
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let table = params.constellation();
            let mut demod = LinearBurstDemod::new(&params, &rx, power_symbols, carrier());
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            decode_coherent(table, &symbols, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn coherent_differential_link(
    label: &str,
    params: LinearParams,
    phase_positions: u32,
    carrier: impl Fn() -> Option<CarrierLoop> + 'static,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let uw = unique_word(table, UW, FILLER_SEED);
            let payload = differential_encode(
                table,
                phase_positions,
                *uw.last().unwrap_or(&0),
                &bits_to_labels(bits, bits_per_symbol),
            );
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let table = params.constellation();
            let mut demod = LinearBurstDemod::new(&params, &rx, POWER_SYMBOLS, carrier());
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            let Some(sliced) = slice_from_word(table, &symbols, PAYLOAD_SYMBOLS + 1, 1) else {
                return Vec::new();
            };
            let labels = differential_decode(table, phase_positions, &sliced);
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

fn differential_encode(
    table: &Constellation,
    phase_positions: u32,
    reference: u32,
    data: &[u32],
) -> Vec<u32> {
    let index_of = |label: u32| {
        table
            .labels()
            .iter()
            .position(|&x| x == label)
            .unwrap_or_default() as u32
    };
    let mut encoder = DifferentialSymbolEncoder::new(phase_positions);
    let _ = encoder.encode(index_of(reference) % phase_positions);
    data.iter()
        .map(|&label| {
            let index = index_of(label);
            let ring = index / phase_positions;
            let phase = encoder.encode(index % phase_positions);
            table.labels()[(ring * phase_positions + phase) as usize]
        })
        .collect()
}

fn differential_decode(table: &Constellation, phase_positions: u32, sliced: &[u32]) -> Vec<u32> {
    let index_of = |label: u32| {
        table
            .labels()
            .iter()
            .position(|&x| x == label)
            .unwrap_or_default() as u32
    };
    let mut decoder = DifferentialSymbolDecoder::new(phase_positions);
    let mut out = Vec::with_capacity(sliced.len().saturating_sub(1));
    for (k, &label) in sliced.iter().enumerate() {
        let index = index_of(label);
        let phase = decoder.decode(index % phase_positions);
        if k == 0 {
            continue;
        }
        let ring = index / phase_positions;
        out.push(table.labels()[(ring * phase_positions + phase) as usize]);
    }
    out
}

fn slice_from_word(
    table: &Constellation,
    symbols: &[Complex<f32>],
    count: usize,
    before: usize,
) -> Option<Vec<u32>> {
    let uw = unique_word(table, UW, FILLER_SEED);
    let expected = table_points(table, &uw);
    let at = find_word(symbols, 0, PREAMBLE * 2, &expected)?;
    let anchor = PhaseAnchor::fit_gain_only(&symbols[at..at + UW], &expected).ok()?;
    let start = at + UW - before;
    Some(
        (0..count)
            .filter_map(|k| symbols.get(start + k))
            .map(|&y| table.hard_slice(anchor.correct(0, y)))
            .collect(),
    )
}

#[must_use]
pub fn coherent_tracked_link(
    label: &str,
    params: LinearParams,
    carrier: impl Fn() -> Option<CarrierLoop> + 'static,
    timing: LinearTiming,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let payload = bits_to_labels(bits, bits_per_symbol);
            let uw = unique_word(table, UW, FILLER_SEED);
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let table = params.constellation();
            let mut demod = LinearDemod::new(&params, &rx, timing, carrier());
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            decode_coherent(table, &symbols, bits_per_symbol)
        }),
    }
}

fn decode_coherent(
    table: &Constellation,
    symbols: &[Complex<f32>],
    bits_per_symbol: usize,
) -> Vec<bool> {
    let uw = unique_word(table, UW, FILLER_SEED);
    let expected = table_points(table, &uw);
    let Some(at) = find_word(symbols, 0, PREAMBLE * 2, &expected) else {
        return Vec::new();
    };
    let Ok(anchor) = PhaseAnchor::fit_gain_only(&symbols[at..at + UW], &expected) else {
        return Vec::new();
    };
    let payload = at + UW;
    let labels: Vec<u32> = (0..PAYLOAD_SYMBOLS)
        .filter_map(|k| symbols.get(payload + k))
        .map(|&y| table.hard_slice(anchor.correct(0, y)))
        .collect();
    labels_to_bits(&labels, bits_per_symbol)
}

#[must_use]
pub fn differential_link(
    label: &str,
    params: LinearParams,
    difference_table: Constellation,
    phase_positions: u32,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let uw = unique_word(table, UW, FILLER_SEED);
            let payload = differential_encode(
                table,
                phase_positions,
                *uw.last().unwrap_or(&0),
                &bits_to_labels(bits, bits_per_symbol),
            );
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let mut demod = LinearBurstDemod::new(&params, &rx, POWER_SYMBOLS, None);
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            let mut products = Vec::new();
            differential_detect(&symbols, &mut products);
            let uw = unique_word(params.constellation(), UW, FILLER_SEED);
            let word_points = table_points(params.constellation(), &uw);
            let mut word_products = Vec::new();
            differential_detect(&word_points, &mut word_products);
            let Some(at) = find_word(&products, 0, PREAMBLE * 2, &word_products) else {
                return Vec::new();
            };
            let payload = at + UW - 1;
            let labels: Vec<u32> = (0..PAYLOAD_SYMBOLS)
                .filter_map(|k| products.get(payload + k))
                .map(|&z| difference_table.hard_slice(z))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn envelope_link(
    label: &str,
    params: LinearParams,
    timing_bw: f64,
    timing: EnvelopeTiming,
) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let table = tx.constellation();
            let payload = bits_to_labels(bits, bits_per_symbol);
            let uw = unique_word(table, UW, FILLER_SEED);
            LinearMod::transmission(&tx, &frame(table, &uw, &payload))
        }),
        demodulate: Box::new(move |wave| {
            let table = params.constellation();
            let mut demod = EnvelopeDemod::new(&params, &rx, timing_bw, timing);
            let mut amplitudes = Vec::new();
            demod.process(wave, &mut amplitudes);
            let uw = unique_word(table, UW, FILLER_SEED);
            let expected: Vec<f32> = table_points(table, &uw).iter().map(|p| p.norm()).collect();
            let Some(at) = find_word_amplitude(&amplitudes, 0, PREAMBLE * 2, &expected) else {
                return Vec::new();
            };
            let payload = at + UW;
            let labels: Vec<u32> = (0..PAYLOAD_SYMBOLS)
                .filter_map(|k| amplitudes.get(payload + k))
                .map(|&a| slice_amplitude(table, a))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_modem::constellation::tables;

    use super::*;

    #[test]
    fn bits_and_labels_round_trip_least_significant_first() {
        let bits = [true, false, true, true, false, false, true, false];
        for k in [1usize, 2, 4, 8] {
            let labels = bits_to_labels(&bits, k);
            assert_eq!(labels.len(), bits.len() / k);
            assert_eq!(labels_to_bits(&labels, k), bits);
        }
        assert_eq!(bits_to_labels(&[true, false, true, false], 2), [0b01, 0b01]);
    }

    #[test]
    fn the_unique_word_shell_is_the_most_populated_radius() {
        for (name, table, want) in [
            ("psk8", tables::psk(8).unwrap(), 8usize),
            ("qam16", tables::qam_square(16).unwrap(), 8),
            ("qam64", tables::qam_square(64).unwrap(), 12),
            ("apsk32", tables::apsk32_dvbs2(2.84, 5.27).unwrap(), 16),
            ("star16", tables::qam_star(&[1.0, 2.0], 8).unwrap(), 8),
            ("ook", tables::ook().unwrap(), 1),
        ] {
            let shell = shell_labels(&table);
            assert_eq!(shell.len(), want, "{name}: shell {shell:?}");
            let radius_of = |label: u32| {
                let i = table.labels().iter().position(|&x| x == label).unwrap();
                f64::from(table.points()[i].norm())
            };
            let first = radius_of(shell[0]);
            for &l in &shell {
                assert!(
                    (radius_of(l) - first).abs() <= 1e-3 * first.max(f64::MIN_POSITIVE),
                    "{name}: label {l} sits at {}, not {first}",
                    radius_of(l)
                );
            }
            let biggest = table
                .points()
                .iter()
                .map(|p| {
                    let r = f64::from(p.norm());
                    table
                        .points()
                        .iter()
                        .filter(|q| {
                            (f64::from(q.norm()) - r).abs() <= 1e-3 * r.max(f64::MIN_POSITIVE)
                        })
                        .count()
                })
                .max()
                .unwrap();
            assert_eq!(shell.len(), biggest, "{name}");
        }
    }

    #[test]
    fn the_word_is_located_through_any_rotation() {
        let table = tables::qam_square(16).unwrap();
        let uw = unique_word(&table, UW, FILLER_SEED);
        let expected = table_points(&table, &uw);
        let filler = data_like(&table, PREAMBLE, FILLER_SEED);
        let mut stream: Vec<Complex<f32>> = table_points(&table, &filler);
        stream.extend_from_slice(&expected);
        stream.extend(table_points(&table, &data_like(&table, 200, 0x1234)));
        for turns in [0.0f64, 0.25, 0.5, 0.75, 0.13] {
            let theta = std::f64::consts::TAU * turns;
            let rot = Complex::new(theta.cos() as f32, theta.sin() as f32);
            let rotated: Vec<Complex<f32>> = stream.iter().map(|&s| s * rot).collect();
            assert_eq!(
                find_word(&rotated, 0, PREAMBLE * 2, &expected),
                Some(PREAMBLE),
                "rotation {turns} turns"
            );
        }
    }

    #[test]
    fn the_frame_is_the_documented_geometry() {
        let table = tables::psk(4).unwrap();
        let uw = unique_word(&table, UW, FILLER_SEED);
        let payload = vec![0u32; PAYLOAD_SYMBOLS];
        let s = frame(&table, &uw, &payload);
        assert_eq!(s.len(), OVERHEAD + PAYLOAD_SYMBOLS);
        assert_eq!(&s[PREAMBLE..PREAMBLE + UW], &uw[..]);
        let charged = 10.0 * ((OVERHEAD + PAYLOAD_SYMBOLS) as f64 / PAYLOAD_SYMBOLS as f64).log10();
        assert!(
            (charged - 0.287).abs() < 0.005,
            "overhead charges {charged} dB"
        );
    }
}
