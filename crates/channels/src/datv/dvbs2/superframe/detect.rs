use num_complex::Complex;

use super::{
    codes::{self, Gold, Search, Sequence},
    layout::{HEADER, SOSF},
};

const GATE: f32 = 0.5;
const COHERENCE: f32 = 0.5;
const COHERENCE_FIT: f32 = 0.65;
const SHORT_GATE: usize = 64;
const WALSH: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    pub sosf: u8,
    pub phase: f32,
    pub frequency: f32,
    pub format: u8,
}

pub fn fit(symbols: &[Complex<f32>], reference: &[Complex<f32>], frequency: f32) -> Option<f32> {
    let mut power = 0.0;
    let mut sum = Complex::new(0.0, 0.0);
    for (i, (&symbol, &known)) in symbols.iter().zip(reference).enumerate() {
        sum += symbol * known.conj() * Complex::from_polar(1.0, -frequency * i as f32);
        power += symbol.norm_sqr();
    }
    let coherence = sum.norm_sqr() / (power * symbols.len() as f32).max(1e-12);
    (coherence > COHERENCE_FIT).then(|| sum.arg())
}

pub fn format(
    symbols: &[Complex<f32>],
    reference: &[Complex<f32>],
    phase: f32,
    frequency: f32,
) -> Option<u8> {
    let mut bits = [0.0f32; 15];
    for (i, (&symbol, &known)) in symbols
        .iter()
        .zip(reference)
        .enumerate()
        .skip(SOSF)
        .take(HEADER - SOSF)
    {
        bits[(i - SOSF) / 30] +=
            (symbol * known.conj() * Complex::from_polar(1.0, -phase - frequency * i as f32)).re;
    }
    let total: f32 = bits.iter().map(|v| v.abs()).sum();
    let mut best = (0, f32::NEG_INFINITY);
    for code in 0..16u8 {
        let score: f32 = bits
            .iter()
            .enumerate()
            .map(|(i, bit)| {
                if (code & (i + 1) as u8).count_ones().is_multiple_of(2) {
                    *bit
                } else {
                    -*bit
                }
            })
            .sum();
        if score > best.1 {
            best = (code, score);
        }
    }
    (best.1 > total * 0.8 && total > 1e-6).then_some(best.0)
}

fn pairs(stripped: &impl Fn(usize) -> Complex<f32>, count: usize) -> (Complex<f32>, f32) {
    let mut sum = Complex::new(0.0, 0.0);
    let mut power = 0.0;
    for pair in 0..count / 2 {
        let first = stripped(2 * pair);
        let second = stripped(2 * pair + 1);
        sum += second * first.conj();
        power += first.norm_sqr() + second.norm_sqr();
    }
    (sum, power)
}

fn rotation(stripped: &impl Fn(usize) -> Complex<f32>) -> f32 {
    let mut estimate = 0.0f32;
    let mut lag = 1;
    while lag < WALSH {
        let sum: Complex<f32> = (0..WALSH)
            .filter(|k| k & lag == 0)
            .map(|k| stripped(k + lag) * stripped(k).conj())
            .sum();
        let span = 2.0 * lag as f32;
        let ambiguity = std::f32::consts::TAU / span;
        let raw = (sum * sum).arg() / span;
        estimate = raw + ((estimate - raw) / ambiguity).round() * ambiguity;
        lag *= 2;
    }
    estimate
}

#[must_use]
pub fn detect(samples: &[Complex<f32>], reference: &Sequence, payload: &Sequence) -> Option<Found> {
    if samples.len() < HEADER {
        return None;
    }
    let stripped = |k: usize| samples[k] * reference.known(k, false).conj();
    let (sum, power) = pairs(&stripped, SHORT_GATE);
    if 2.0 * sum.norm() < GATE * power {
        return None;
    }
    let (sum, power) = pairs(&stripped, WALSH);
    if 2.0 * sum.norm() < GATE * power {
        return None;
    }
    let turn = rotation(&stripped);
    let mut values = [Complex::new(0.0f32, 0.0); WALSH];
    let mut energy = 0.0;
    for (k, value) in values.iter_mut().enumerate() {
        *value = stripped(k) * Complex::from_polar(1.0, -turn * k as f32);
        energy += value.norm_sqr();
    }
    codes::hadamard(&mut values);
    let (row, peak) = codes::strongest(&values);
    if peak.norm_sqr() < COHERENCE * WALSH as f32 * energy {
        return None;
    }
    let sosf = row as u8;
    let mut known = [Complex::new(0.0f32, 0.0); HEADER];
    for (k, slot) in known.iter_mut().enumerate() {
        *slot = if k < SOSF {
            reference.known(k, codes::sosf(sosf, k))
        } else {
            payload.known(k, false)
        };
    }
    let phase = fit(&samples[..SOSF], &known[..SOSF], turn)?;
    let (phase, frequency) = refine(&samples[..SOSF], &known[..SOSF], phase, turn);
    let format = format(&samples[..HEADER], &known, phase, frequency)?;
    for (k, slot) in known.iter_mut().enumerate().skip(SOSF) {
        let column = (k - SOSF) / 30 + 1;
        if (format & column as u8).count_ones() % 2 == 1 {
            *slot = -*slot;
        }
    }
    let (phase, frequency) = refine(&samples[..HEADER], &known, phase, frequency);
    Some(Found {
        sosf,
        phase,
        frequency,
        format,
    })
}

fn refine(
    symbols: &[Complex<f32>],
    known: &[Complex<f32>],
    phase: f32,
    frequency: f32,
) -> (f32, f32) {
    let half = symbols.len() / 2;
    let stripped = |k: usize| {
        symbols[k] * known[k].conj() * Complex::from_polar(1.0, -phase - frequency * k as f32)
    };
    let first: Complex<f32> = (0..half).map(stripped).sum();
    let second: Complex<f32> = (half..2 * half).map(stripped).sum();
    let frequency = frequency + (second * first.conj()).arg() / half as f32;
    let sum: Complex<f32> = (0..symbols.len())
        .map(|k| symbols[k] * known[k].conj() * Complex::from_polar(1.0, -frequency * k as f32))
        .sum();
    (sum.arg(), frequency)
}

#[must_use]
pub fn identify(gold: &Gold, search: &Search, samples: &[Complex<f32>]) -> Option<(u32, u32)> {
    if samples.len() < HEADER {
        return None;
    }
    let turn = |k: usize| codes::turn(samples[k], samples[k - 1]);
    let reference = search.identify(gold, 1, SOSF - 1, |k| turn(k + 1))?;
    let payload = search.identify(gold, SOSF + 1, HEADER - SOSF - 1, |k| turn(SOSF + 1 + k))?;
    Some((reference, payload))
}

#[must_use]
pub fn pilot_row(field: &[Complex<f32>], expected: impl Fn(usize) -> Complex<f32>) -> (u8, f32) {
    let mut values = [Complex::new(0.0f32, 0.0); 32];
    let mut energy = 0.0;
    for (k, value) in values.iter_mut().enumerate() {
        *value = field[k] * expected(k).conj();
        energy += value.norm_sqr();
    }
    codes::hadamard(&mut values);
    let (row, peak) = codes::strongest(&values);
    (row as u8, peak.norm_sqr() / (32.0 * energy).max(1e-12))
}
