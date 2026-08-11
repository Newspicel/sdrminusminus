//! The phase-0 DMR chain's shared pieces — transmit-side framing constants, the receive front
//! end as production runs it, and the searched-alignment idiom — factored out of
//! `dmr_baseline.rs` verbatim so `dmr_soft_gain.rs` measures the *same* chain the committed
//! uncoded baselines were taken on (MODEM-PLAN §7 phase 1: the soft-BPTC gain is measured on
//! the phase-0 curves' chain, not a variant of it). Extraction only: nothing here may drift
//! from what the committed measurements were produced by.

// Each integration-test binary compiles its own copy of this module and uses a subset of it.
#![allow(dead_code)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_channels::{channel_filter, testgen::dv as tg};
use sdrmm_dsp::Fsk4Demod;
use sdrmm_modem::ber::rng::Rng;
use sdrmm_wire::{ChannelParams, DmrParams};

pub const RATE: f64 = 48_000.0;
pub const BAUD: f64 = 4_800.0;
pub const DEVIATION_HZ: f64 = 1_944.0;
pub const RRC_ALPHA: f64 = 0.2;

/// DMR BS-sourced voice sync (ETSI TS 102 361-1 §9.1.1) — the unique word both chains align
/// on and the burst chain anchors levels to, as the decoder itself does.
pub const UW: u64 = 0x755F_D7DF_75F7;
pub const UW_SYMBOLS: usize = 24;

/// Clock pull-in from a cold phase costs ~80 symbols (fsk4's own tests); the preamble covers
/// that before the sync so the payload is met by a locked loop.
pub const STEADY_PREAMBLE: usize = 88;
/// Trailing filler past the payload: the front end is a whole filter cascade late (~24
/// symbols), so the transmitter must keep shaping that long past the last payload symbol or
/// the demodulator never emits it.
pub const STEADY_TAIL: usize = 40;

pub fn dmr_params() -> ChannelParams {
    ChannelParams::Dmr(DmrParams::default())
}

pub fn uw_dibits() -> Vec<u8> {
    tg::dibits(&tg::bits(UW, 48))
}

/// Receiver noise at 40 dB below a unit carrier — what the steady chain's demodulator hears
/// before the transmission, exactly the `fsk4` tests' `listening` convention.
fn quiet(seed: u64, len: usize) -> Vec<Complex<f32>> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|_| {
            let re = (rng.uniform() * 2.0 - 1.0) * 0.01;
            let im = (rng.uniform() * 2.0 - 1.0) * 0.01;
            Complex::new(re as f32, im as f32)
        })
        .collect()
}

/// The receive front end under measurement, as production runs it: the DMR channel-selection
/// filter (the receiver's noise bandwidth — without it the discriminator eats the full 48 kHz
/// and the waterfall shifts ~6 dB right) into `Fsk4Demod`, fresh per trial so every trial is
/// independent and reproducible from its own seed.
pub fn recovered_symbols(wave: &[Complex<f32>], warm_up: bool) -> Vec<f32> {
    let mut filter = channel_filter(&dmr_params()).unwrap();
    let mut demod = Fsk4Demod::new(RATE, BAUD, DEVIATION_HZ, RRC_ALPHA);
    let mut filtered = Vec::new();
    if warm_up {
        let mut discard = Vec::new();
        filter.process(&quiet(0x1157, (RATE * 0.2) as usize), &mut filtered);
        demod.process(&filtered, &mut discard);
    }
    let mut symbols = Vec::new();
    filter.process(wave, &mut filtered);
    demod.process(&filtered, &mut symbols);
    symbols
}

fn uw_distance(sliced: &[u8], at: usize, uw: &[u8]) -> u32 {
    uw.iter()
        .enumerate()
        .map(|(i, &d)| (sliced[at + i] ^ d).count_ones())
        .sum()
}

/// Best sync position in `lo..=hi` by Hamming distance — the searched-alignment idiom. No
/// threshold: a chain too degraded to place its sync scores its garbage as bit errors.
pub fn find_uw(sliced: &[u8], lo: usize, hi: usize, uw: &[u8]) -> Option<usize> {
    let last = hi.min(sliced.len().checked_sub(uw.len())?);
    (lo..=last).min_by_key(|&at| uw_distance(sliced, at, uw))
}

pub fn alternating(len: usize) -> impl Iterator<Item = u8> {
    (0..len).map(|i| if i % 2 == 0 { 0b01 } else { 0b11 })
}

pub fn baseline_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/dmr/{name}"))
}
