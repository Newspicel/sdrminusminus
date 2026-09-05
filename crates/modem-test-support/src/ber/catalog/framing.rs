use num_complex::Complex;
use sdrmm_dsp::{Decimator, design_lowpass};
use sdrmm_modem::cpm::{CpmDemod, CpmMod, CpmParams, TIMING_BW_BURST};

use crate::ber::{rng::Rng, sweep::Link};

pub const BAUD: f64 = 4_800.0;
pub const SPS: f64 = 10.0;
pub const RATE: f64 = BAUD * SPS;

pub const NOISE_BW_HZ: f64 = 6_000.0;
pub const FRONT_TAPS: usize = 127;

pub const PREAMBLE: usize = 96;
pub const TAIL: usize = 24;
pub const STEADY_BITS: usize = 1024;

pub const UW24: [u8; 24] = [
    0, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 0,
];

pub const FULL_CAP: u64 = 4_000_000;

pub const WARMUP_SYMBOLS: usize = 500;

#[must_use]
pub fn alternating_symbols(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 2) as u8).collect()
}

pub const DATA_LIKE_SEED: u32 = 0x9e37_79b9;

fn data_like_from(state: &mut u32, len: usize) -> Vec<u8> {
    (0..len)
        .map(|_| {
            *state ^= *state << 13;
            *state ^= *state >> 17;
            *state ^= *state << 5;
            (*state & 1) as u8
        })
        .collect()
}

#[must_use]
pub fn data_like_symbols(len: usize, seed: u32) -> Vec<u8> {
    data_like_from(&mut { seed }, len)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acquisition {
    Alternating,
    DataLike,
}

#[must_use]
pub fn framed_symbols(
    acquisition: Acquisition,
    preamble: usize,
    uw: &[u8],
    bits: &[bool],
    tail: usize,
) -> Vec<u8> {
    let mut state = DATA_LIKE_SEED;
    let mut fill = |len: usize| match acquisition {
        Acquisition::Alternating => alternating_symbols(len),
        Acquisition::DataLike => data_like_from(&mut state, len),
    };
    let mut s = fill(preamble);
    s.extend_from_slice(uw);
    s.extend(bits.iter().map(|&b| u8::from(b)));
    s.extend(fill(tail));
    s
}

#[must_use]
pub fn cpm_wave(params: &CpmParams, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut m = CpmMod::new(params.clone());
    let mut out = Vec::new();
    m.modulate(symbols, &mut out);
    m.flush(&mut out);
    out
}

#[must_use]
pub fn quiet(seed: u64, len: usize) -> Vec<Complex<f32>> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|_| {
            let re = (rng.uniform() * 2.0 - 1.0) * 0.01;
            let im = (rng.uniform() * 2.0 - 1.0) * 0.01;
            Complex::new(re as f32, im as f32)
        })
        .collect()
}

#[must_use]
pub fn real_quiet(seed: u64, len: usize) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|_| ((rng.uniform() * 2.0 - 1.0) * 0.01) as f32)
        .collect()
}

#[must_use]
pub fn steady_soft(params: &CpmParams, rx: &[f32], wave: &[Complex<f32>]) -> Vec<f32> {
    let front = design_lowpass(FRONT_TAPS, NOISE_BW_HZ / RATE);
    let mut filter = Decimator::new(&front, 1);
    let mut demod = CpmDemod::new(params, rx, TIMING_BW_BURST);
    let mut filtered = Vec::new();
    let mut discard = Vec::new();
    filter.process(&quiet(0x1157, WARMUP_SYMBOLS * SPS as usize), &mut filtered);
    demod.process(&filtered, &mut discard);
    let mut soft = Vec::new();
    filter.process(wave, &mut filtered);
    demod.process(&filtered, &mut soft);
    soft
}

#[must_use]
pub fn uw_levels(params: &CpmParams, uw: &[u8]) -> Vec<f32> {
    uw.iter().map(|&s| params.mapping().level(s)).collect()
}

#[must_use]
pub fn find_uw(soft: &[f32], lo: usize, hi: usize, levels: &[f32]) -> Option<usize> {
    let last = hi.min(soft.len().checked_sub(levels.len())?);
    let misfit = |at: usize| -> f32 {
        levels
            .iter()
            .enumerate()
            .map(|(i, &l)| (soft[at + i] - l) * (soft[at + i] - l))
            .sum()
    };
    (lo..=last).min_by(|&a, &b| misfit(a).total_cmp(&misfit(b)))
}

#[must_use]
pub fn payload_bits(
    params: &CpmParams,
    soft: &[f32],
    at: usize,
    uw_len: usize,
    n: usize,
) -> Vec<bool> {
    (0..n)
        .map(|k| {
            soft.get(at + uw_len + k)
                .is_some_and(|&s| params.mapping().slice(s) == 1)
        })
        .collect()
}

#[must_use]
pub fn steady_link(label: &str, acquisition: Acquisition, params: CpmParams, rx: Vec<f32>) -> Link {
    let mod_params = params.clone();
    Link {
        label: label.to_string(),
        bits_per_trial: STEADY_BITS,
        modulate: Box::new(move |bits| {
            cpm_wave(
                &mod_params,
                &framed_symbols(acquisition, PREAMBLE, &UW24, bits, TAIL),
            )
        }),
        demodulate: Box::new(move |wave| {
            let soft = steady_soft(&params, &rx, wave);
            let levels = uw_levels(&params, &UW24);
            let Some(at) = find_uw(&soft, PREAMBLE, PREAMBLE + 48, &levels) else {
                return Vec::new();
            };
            payload_bits(&params, &soft, at, UW24.len(), STEADY_BITS)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_acquisition_frames_the_same_lengths() {
        let bits = [true, false, true, true];
        for acquisition in [Acquisition::Alternating, Acquisition::DataLike] {
            let s = framed_symbols(acquisition, PREAMBLE, &UW24, &bits, TAIL);
            assert_eq!(s.len(), PREAMBLE + UW24.len() + bits.len() + TAIL);
            assert_eq!(&s[PREAMBLE..PREAMBLE + UW24.len()], &UW24);
        }
    }

    #[test]
    fn alternating_filler_collapses_through_a_partial_response() {
        let response = [0.014f32, 0.220, 0.532, 0.220, 0.014];
        let level = |s: u8| if s == 1 { 1.0f32 } else { -1.0 };
        let convolve = |symbols: &[u8]| -> f32 {
            symbols
                .windows(response.len())
                .map(|w| {
                    w.iter()
                        .zip(&response)
                        .map(|(&s, &h)| level(s) * h)
                        .sum::<f32>()
                        .abs()
                })
                .fold(0.0, f32::max)
        };
        let alternating = convolve(&alternating_symbols(64));
        let data_like = convolve(&data_like_symbols(64, DATA_LIKE_SEED));
        assert!(
            alternating < 0.2 && data_like > 0.9,
            "alternating {alternating}, data-like {data_like}"
        );
    }
}
