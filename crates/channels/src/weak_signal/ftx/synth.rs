use std::f64::consts::{PI, TAU};

use num_complex::Complex;

use super::protocol::{Protocol, SAMPLE_RATE};

const RENORMALISE_EVERY: usize = 1_024;

pub(crate) fn span(protocol: &Protocol) -> usize {
    (protocol.symbols + 2 * protocol.ramp_symbols) * protocol.symbol_samples
}

pub(crate) fn lead_samples(protocol: &Protocol) -> usize {
    protocol.ramp_symbols * protocol.symbol_samples
}

pub(crate) fn waveform(
    protocol: &Protocol,
    tones: &[u8],
    frequency_hz: f64,
    out: &mut Vec<Complex<f32>>,
) {
    let deviation = modulation(protocol, tones);
    let skip = if protocol.ramp_symbols == 0 {
        protocol.symbol_samples
    } else {
        0
    };
    let carrier = Complex::from_polar(1.0, TAU * frequency_hz / f64::from(SAMPLE_RATE));
    let mut phasor = Complex::new(1.0f64, 0.0);
    out.clear();
    out.reserve(span(protocol));
    for (index, &angle) in deviation[skip..skip + span(protocol)].iter().enumerate() {
        out.push(Complex::new(phasor.re as f32, phasor.im as f32));
        phasor *= carrier * small_rotation(angle);
        if index % RENORMALISE_EVERY == 0 {
            phasor /= phasor.norm();
        }
    }
    shape_edges(protocol, out);
}

fn modulation(protocol: &Protocol, tones: &[u8]) -> Vec<f64> {
    let samples = protocol.symbol_samples;
    let symbols = protocol.symbols;
    let pulse = pulse(protocol);
    let peak = TAU / samples as f64;
    let mut deviation = vec![0.0f64; (symbols + 2) * samples];
    for (symbol, &tone) in tones.iter().take(symbols).enumerate() {
        let start = symbol * samples;
        for (slot, &shape) in deviation[start..start + 3 * samples].iter_mut().zip(&pulse) {
            *slot += peak * shape * f64::from(tone);
        }
    }
    let first = f64::from(tones[0]);
    let last = f64::from(tones[symbols - 1]);
    for (slot, &shape) in deviation[..2 * samples].iter_mut().zip(&pulse[samples..]) {
        *slot += peak * shape * first;
    }
    for (slot, &shape) in deviation[symbols * samples..].iter_mut().zip(&pulse) {
        *slot += peak * shape * last;
    }
    deviation
}

fn pulse(protocol: &Protocol) -> Vec<f64> {
    let samples = protocol.symbol_samples as f64;
    let scale = PI * (2.0 / 2f64.ln()).sqrt() * f64::from(protocol.bt);
    (1..=3 * protocol.symbol_samples)
        .map(|index| {
            let t = (index as f64 - 1.5 * samples) / samples;
            0.5 * (erf(scale * (t + 0.5)) - erf(scale * (t - 0.5)))
        })
        .collect()
}

fn small_rotation(angle: f64) -> Complex<f64> {
    let square = angle * angle;
    Complex::new(
        1.0 - square / 2.0 + square * square / 24.0,
        angle * (1.0 - square / 6.0 + square * square / 120.0),
    )
}

fn shape_edges(protocol: &Protocol, out: &mut [Complex<f32>]) {
    let ramp = if protocol.ramp_symbols == 0 {
        protocol.symbol_samples / 8
    } else {
        protocol.symbol_samples
    };
    let length = out.len();
    for index in 0..ramp {
        let gain = (1.0 - (PI * index as f64 / ramp as f64).cos()) as f32 / 2.0;
        out[index] *= gain;
        out[length - 1 - index] *= gain;
    }
}

fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.5 * x.abs());
    let polynomial = -x * x - 1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_203_98
                                + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let value = 1.0 - t * polynomial.exp();
    if x >= 0.0 { value } else { -value }
}

#[cfg(test)]
mod tests {
    use super::{super::protocol::FT8, *};

    #[test]
    fn a_steady_tone_sits_at_its_frequency() {
        let tones = [2u8; 79];
        let mut wave = Vec::new();
        waveform(&FT8, &tones, 1_000.0, &mut wave);
        assert_eq!(wave.len(), 79 * 1_920);
        let middle = 40 * 1_920;
        let turn = (wave[middle + 1] * wave[middle].conj()).arg();
        let expected = TAU * (1_000.0 + 2.0 * 6.25) / 12_000.0;
        assert!(
            (f64::from(turn) - expected).abs() < 1e-5,
            "{turn} vs {expected}"
        );
        assert!((wave[middle].norm() - 1.0).abs() < 1e-4);
    }
}
