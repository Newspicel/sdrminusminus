//! The noncoherent orthogonal M-FSK catalog entry ( §6, orthogonal row): M ∈ {2, 4, 8}
//! measured chains, shared by every consumer of the entry — the curve/limits/E2E tests, the perf
//! baseline, and `cargo xtask ber mfsk-orthogonal` — so every committed artifact is taken on the
//! *same* chain.
//!
//! One geometry for all three alphabets, and deliberately the CPM entry's: 48 kHz, 4800 baud,
//! 10 samples per symbol. The two entries then differ by their *detector* alone, which is what
//! makes the cross-entry number in `CATALOG.md` — orthogonal M-FSK's filterbank against M-ary
//! CPFSK's discriminator at the same M — a measurement rather than a comparison of two
//! unrelated configurations.
//!
//! Three things this chain does *not* have, each because the detector removes the need:
//!
//! - **No channel-selection filter.** The bank's matched filters are the selectivity: a tone's
//!   correlator integrates one symbol and sits on every other tone's null, so noise outside the
//!   plan contributes only through those filters. The CPM rows need a front-end lowpass because
//!   a discriminator eats its whole input bandwidth as noise; this one measures the same
//!   waveform without one.
//! - **No timing loop.** Timing is the engine's feedforward burst estimate (see `orthogonal`'s
//!   demod docs) — one whole-sample offset per trial, maximising collected peak-tone energy.
//! - **No acquisition preamble.** A noncoherent receiver has nothing to converge: the estimator
//!   reads the payload's own tones. The only overhead is the 24-symbol unique word and the
//!   16 filler symbols that keep its position *searched* rather than assumed (§4.1: alignment
//!   is never assumed), both charged to Eb — 0.08 dB at the committed payload length, as the
//!   labels say.

use num_complex::Complex;

use super::{
    Measurement, Reference,
    mfsk::{bits_to_symbols, push_symbol_bits},
};
use crate::{
    ber::{sweep::Link, theory},
    orthogonal::{MfskDemod, MfskMod, MfskParams, TonePhase},
};

pub const RATE: f64 = 48_000.0;
pub const BAUD: f64 = 4_800.0;
pub const SPS: f64 = 10.0;

/// Filler symbols ahead of the unique word: enough that the word's position has to be found
/// rather than assumed, short enough to stay cheap in Eb.
pub const LEAD: usize = 16;
/// Symbols past the nominal word position the search covers — the residual an impaired chain
/// (sample-clock error, a timing offset the whole-sample estimate rounds) can shift the frame by.
pub const SEARCH: usize = 8;
/// Payload symbols per trial: 2048 symbols is 2048/4096/6144 information bits at M = 2/4/8. The
/// length is chosen against the Eb accounting rather than the runtime — the 40 framing symbols
/// are charged to Eb, and at 512 payload symbols that overhead alone was 0.32 dB of the measured
/// distance from the closed form. At 2048 it is 0.08 dB, so the oracle gate reads the detector
/// rather than the frame.
pub const PAYLOAD_SYMBOLS: usize = 2_048;

/// The 24-symbol unique word as octal digits, reduced modulo M per alphabet. Chosen by search
/// over base-8 sequences for the property that has to hold at *every* alphabet it reduces to:
/// worst shifted agreement against a data-like context 13 of 24 at M = 2 — where the reduction
/// is harshest and a random position already agrees on 12 — 7 of 24 at M = 4 and 4 of 24 at
/// M = 8, against a true position that agrees on 24. The failure this avoids is the one the
/// GMSK substrate recorded: a word whose halves repeat anchors whole trials one period early at
/// high Eb/N0, and the resulting floor reads as a detector bug.
pub const UW: [u8; 24] = [
    5, 3, 1, 0, 0, 6, 1, 2, 7, 2, 1, 5, 6, 3, 5, 4, 5, 7, 7, 3, 6, 2, 6, 4,
];

/// The tone plan of one alphabet: spacing 1 (the tightest orthogonal plan, and FT8's).
///
/// # Panics
/// As [`MfskParams::orthogonal`].
#[must_use]
pub fn params(m: usize) -> MfskParams {
    MfskParams::orthogonal(m, SPS)
}

/// The entry's unique word at alphabet M.
#[must_use]
pub fn unique_word(m: usize) -> Vec<u8> {
    UW.iter().map(|&s| s % m as u8).collect()
}

/// Lead filler: data-like symbols from a fixed seed, so the estimator meets the same statistics
/// ahead of the word as inside the payload and a trial still reproduces from its own seed.
#[must_use]
pub fn filler(m: usize, len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state % m as u32) as u8
        })
        .collect()
}

#[must_use]
pub fn modulate(m: usize, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut modulator = MfskMod::new(params(m), TonePhase::Continuous);
    let mut out = Vec::new();
    modulator.modulate(symbols, &mut out);
    modulator.flush(&mut out);
    out
}

/// Best word position in `lo..=hi` by symbol Hamming distance — the crate's searched-alignment
/// idiom. No threshold: a chain too degraded to place its word scores its garbage as bit errors.
#[must_use]
pub fn find_word(symbols: &[u8], lo: usize, hi: usize, word: &[u8]) -> Option<usize> {
    let last = hi.min(symbols.len().checked_sub(word.len())?);
    (lo..=last).min_by_key(|&at| {
        word.iter()
            .enumerate()
            .filter(|&(i, &s)| symbols[at + i] != s)
            .count()
    })
}

/// One alphabet's payload-to-payload chain: filler + unique word + payload through [`MfskMod`],
/// feedforward timing, searched word alignment, payload read off the argmax.
///
/// `payload_symbols` is [`PAYLOAD_SYMBOLS`] for the committed curves; the level-1 E2E runs the
/// same chain with shorter payloads.
#[must_use]
pub fn link_sized(m: usize, payload_symbols: usize) -> Link {
    let bits_per_symbol = m.trailing_zeros() as usize;
    let word = unique_word(m);
    let tx_word = word.clone();
    let demod = MfskDemod::new(params(m));
    Link {
        label: format!(
            "orthogonal {m}-FSK, spacing 1 cycle/symbol, {BAUD} baud at {RATE} Hz ({SPS} sps), \
             continuous phase -> matched tone filterbank + feedforward burst timing, \
             {LEAD}+{} symbol overhead in Eb, release",
            word.len()
        ),
        bits_per_trial: payload_symbols * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut symbols = filler(m, LEAD);
            symbols.extend_from_slice(&tx_word);
            symbols.extend(bits_to_symbols(bits, bits_per_symbol));
            modulate(m, &symbols)
        }),
        demodulate: Box::new(move |wave| {
            // The estimator reads the frame's own tones — there is no preamble to read
            // instead, and none is needed: every symbol carries one dominant tone.
            let offset = demod.estimate_offset(wave, LEAD + word.len());
            let mut symbols = Vec::new();
            demod.demodulate(
                wave,
                offset,
                LEAD + SEARCH + word.len() + payload_symbols,
                &mut symbols,
            );
            let Some(at) = find_word(&symbols, 0, LEAD + SEARCH, &word) else {
                return Vec::new();
            };
            let mut bits = Vec::with_capacity(payload_symbols * bits_per_symbol);
            for k in 0..payload_symbols {
                let symbol = symbols.get(at + word.len() + k).copied().unwrap_or(0);
                push_symbol_bits(symbol, bits_per_symbol, &mut bits);
            }
            bits
        }),
    }
}

#[must_use]
pub fn mfsk2_link() -> Link {
    link_sized(2, PAYLOAD_SYMBOLS)
}

#[must_use]
pub fn mfsk4_link() -> Link {
    link_sized(4, PAYLOAD_SYMBOLS)
}

#[must_use]
pub fn mfsk8_link() -> Link {
    link_sized(8, PAYLOAD_SYMBOLS)
}

// --- Committed sweep parameters ----------------------------------------------------------------
//
// Grids bracket each waterfall from its shoulder to past the 1e-4 crossing. Orthogonal
// signalling's whole point is visible in them: the shoulder moves *left* as M grows, which is
// bandwidth being spent on energy efficiency.

pub const M2_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0];
pub const M4_GRID: &[f64] = &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
pub const M8_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0];

pub const M2_SEED: u64 = 0x0f52;
pub const M4_SEED: u64 = 0x0f54;
pub const M8_SEED: u64 = 0x0f58;

/// Trial-bit cap per committed point. A trial is 512–1536 bits behind a 40-symbol frame, so
/// this clears several thousand whole trials at the steep high-SNR points.
pub const FULL_CAP: u64 = 4_000_000;

/// Worst horizontal distance from the exact noncoherent orthogonal closed form the committed
/// curves are held to, across the whole grid. Measured: +0.148 dB at M = 2, +0.219 at M = 4,
/// +0.168 at M = 8 — of which 0.08 dB is the framing overhead charged to Eb per the labels. The
/// gate is set at twice the worst measured value, so it fails on a detector regression rather
/// than on the counting noise of a high-SNR point.
pub const ORACLE_TOLERANCE_DB: f64 = 0.4;

pub const M2_AWGN: &str = "orthogonal/mfsk2_noncoherent_awgn";
pub const M4_AWGN: &str = "orthogonal/mfsk4_noncoherent_awgn";
pub const M8_AWGN: &str = "orthogonal/mfsk8_noncoherent_awgn";
pub const M4_LIMITS: &str = "orthogonal/mfsk4_limits";
pub const PERF: &str = "orthogonal/mfsk_perf";

fn m2_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(2, ebn0_db)
}

fn m4_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(4, ebn0_db)
}

fn m8_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(8, ebn0_db)
}

const fn oracle(name: &'static str, ber: fn(f64) -> f64) -> Reference {
    Reference::Oracle {
        name,
        ber,
        tolerance_db: ORACLE_TOLERANCE_DB,
    }
}

pub const MEASUREMENTS: &[Measurement] = &[
    Measurement {
        reference: oracle("exact noncoherent orthogonal 2-FSK", m2_ber),
        ..Measurement::committed(M2_AWGN, mfsk2_link, M2_GRID, M2_SEED, FULL_CAP)
    },
    Measurement {
        reference: oracle("exact noncoherent orthogonal 4-FSK", m4_ber),
        ..Measurement::committed(M4_AWGN, mfsk4_link, M4_GRID, M4_SEED, FULL_CAP)
    },
    Measurement {
        reference: oracle("exact noncoherent orthogonal 8-FSK", m8_ber),
        ..Measurement::committed(M8_AWGN, mfsk8_link, M8_GRID, M8_SEED, FULL_CAP)
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The unique word's reduction property, at every alphabet it is reduced to: no shifted
    /// overlap may agree well enough to out-score the true position under noise. Measured as
    /// the best shifted agreement against a data-like context, the way the search sees it.
    #[test]
    fn the_unique_word_stays_aperiodic_at_every_alphabet() {
        for (m, worst_allowed) in [(2usize, 14), (4, 9), (8, 6)] {
            let word = unique_word(m);
            let mut context = filler(m, LEAD);
            context.extend_from_slice(&word);
            context.extend(filler(m, 32));
            let worst = (0..=LEAD + 8)
                .filter(|&at| at != LEAD)
                .map(|at| {
                    word.iter()
                        .enumerate()
                        .filter(|&(i, &s)| context[at + i] == s)
                        .count()
                })
                .max()
                .unwrap_or(0);
            assert!(
                worst <= worst_allowed,
                "M = {m}: a shifted position agrees on {worst} of {} symbols",
                word.len()
            );
        }
    }

    /// The chain round-trips with no noise at all — a defect in framing, alignment or bit
    /// packing is loud before any statistics are involved.
    #[test]
    fn every_alphabet_round_trips_on_a_clean_channel() {
        for m in [2usize, 4, 8] {
            let link = link_sized(m, 64);
            let bits: Vec<bool> = (0..link.bits_per_trial).map(|i| i % 3 == 0).collect();
            let wave = (link.modulate)(&bits);
            assert_eq!((link.demodulate)(&wave), bits, "M = {m}");
        }
    }
}
