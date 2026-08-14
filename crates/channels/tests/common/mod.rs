//! The DMR measurement chain's shared pieces — transmit-side framing constants, the receive
//! front end as production runs it, and the searched-alignment idiom — shared so
//! `dmr_soft_gain.rs` measures the *same* chain `dmr_baseline.rs` commits curves for.
#![allow(dead_code)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_channels::{channel_filter, testgen::dv as tg};
use sdrmm_modem::{
    ber::rng::Rng,
    cpm::{CpmDemod, CpmParams, Mapping},
    pulse::{self, Norm},
};
use sdrmm_wire::{ChannelParams, DmrParams};

pub const RATE: f64 = 48_000.0;
pub const BAUD: f64 = 4_800.0;
pub const SPS: f64 = 10.0;
pub const DEVIATION_HZ: f64 = 1_944.0;
pub const RRC_ALPHA: f64 = 0.2;
pub const RRC_SPAN: usize = 8;

pub const UW: u64 = 0x755F_D7DF_75F7;
pub const UW_SYMBOLS: usize = 24;

/// Clock pull-in from a cold phase costs ~80 symbols at the burst timing bandwidth; the
/// preamble covers that before the sync so the payload is met by a locked loop.
pub const STEADY_PREAMBLE: usize = 88;
/// Trailing filler past the payload: the front end is a whole filter cascade late (~24
/// symbols), so the transmitter must keep shaping that long past the last payload symbol or
/// the demodulator never emits it.
pub const STEADY_TAIL: usize = 40;

pub fn dmr_params() -> ChannelParams {
    ChannelParams::Dmr(DmrParams::default())
}

/// The DMR C4FM entry exactly as the production channel parameterises it (`dv::c4fm_params`):
/// the ETSI dibit table (symbol index == dibit), h converted from the ±1944 Hz outer
/// deviation, the RRC α=0.2 frequency pulse — which, being a root pair, is also the receive
/// matched filter.
pub fn dmr_entry() -> CpmParams {
    CpmParams::from_deviation(
        Mapping::new(vec![1.0, 3.0, -1.0, -3.0]),
        DEVIATION_HZ,
        BAUD,
        pulse::root_raised_cosine(SPS, RRC_ALPHA, RRC_SPAN, Norm::Area),
        SPS,
    )
}

/// The sync as transmitted, oldest symbol first — what the modulate side appends.
pub fn uw_dibits() -> Vec<u8> {
    tg::dibits(&tg::bits(UW, 48))
}

/// The same sync in `KnownSymbols::anchor` pairing order: element `i` is the dibit `i`
/// symbols back from the pattern's end, matching a measured window read backwards.
pub fn uw_recent_first() -> Vec<u8> {
    (0..UW_SYMBOLS)
        .map(|i| (UW >> (2 * i)) as u8 & 0b11)
        .collect()
}

/// Receiver noise at 40 dB below a unit carrier — what the steady chain's demodulator hears
/// before the transmission, the front-end tests' `listening` convention.
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
/// and the waterfall shifts ~6 dB right) into `CpmDemod`, fresh per trial so every trial is
/// independent and reproducible from its own seed. `timing_bw` is the entry's timing operating
/// point — `TIMING_BW_BURST` as the TDMA channel runs, `TIMING_BW_CONTINUOUS` for the
/// continuous-stream measurements.
pub fn recovered_symbols(wave: &[Complex<f32>], warm_up: bool, timing_bw: f64) -> Vec<f32> {
    let entry = dmr_entry();
    let mut filter = channel_filter(&dmr_params()).unwrap();
    let mut demod = CpmDemod::new(&entry, entry.freq_pulse(), timing_bw);
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
