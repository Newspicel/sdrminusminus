//! The M-PPM catalog entry (MODEM-PLAN §6, pulse-position row): M ∈ {2, 4} measured chains on
//! both detector tiers, shared by every consumer — the curve/limits/E2E tests, the perf
//! baselines, and `cargo xtask ber ppm`.
//!
//! Reference geometry: **1 Mslot/s at 8 Msps, 8 samples per slot**. The slot rate is Mode S's
//! (0.5 µs slots, so the M = 2 row is that waveform's alphabet at a bit rate of 1 Mbit/s), and
//! the oversampling is the one thing deliberately *unlike* the attachment: `channels::adsb`
//! runs the same tier at ~1 sample per slot because that is what a 2 Msps radio hands it, and a
//! curve taken there would measure the sampling, not the modulation. The fractional-rate and
//! sub-sample-phase behaviour that case needs is measured separately, as a property rather than
//! a curve (`crates/modem/tests/ppm.rs`).
//!
//! Two tiers, one chain (§5 item 2 — later tiers are measured against the first):
//!
//! - **Matched filter** (tier 1): the slot's samples integrated, then squared. M orthogonal
//!   equal-energy signals under envelope detection is exactly the closed form
//!   [`theory::mfsk_noncoherent_ber`] describes, so this tier is oracle-matched rather than
//!   commit-and-guard — the same acceptance the orthogonal M-FSK entry gets, from the same
//!   formula, which is the point: PPM and M-FSK are one signalling set wearing two waveforms.
//! - **Envelope** (tier 2): the slot's magnitudes summed, the statistic a receiver scanning a
//!   wideband stream for bursts already has. No closed form describes it — every sample brings
//!   its own rectified noise mean — so it is committed-and-guarded, and its measured distance
//!   behind tier 1 is the number that justifies Mode S paying it.
//!
//! Eb accounting: per information bit, with the 2-symbol lead-in and the 24-symbol unique word
//! charged to Eb exactly as every other entry charges its framing (0.05 dB at the committed
//! payload length), as the labels say.

use num_complex::Complex;

use super::{
    Measurement, Reference,
    mfsk::{bits_to_symbols, push_symbol_bits},
};
use crate::{
    ber::{sweep::Link, theory},
    ppm::{PpmDemod, PpmMod, SlotDetector},
};

/// Slots per second — Mode S's 0.5 µs half-chip.
pub const SLOT_RATE: f64 = 1_000_000.0;
/// Samples per slot in the reference configuration, and so the sample rate: 8 Msps.
pub const SLOT_SPS: f64 = 8.0;
pub const RATE: f64 = SLOT_RATE * SLOT_SPS;

/// Filler symbols ahead of the unique word: the burst lead-in the alignment search covers.
pub const LEAD: usize = 2;
/// Slots past the lead the word search also covers — the residual an impaired chain (clock
/// error, a static timing offset) shifts the frame by.
pub const SEARCH_SLOTS: usize = 4;
/// Payload symbols per trial, chosen against the Eb accounting exactly as the orthogonal M-FSK
/// entry's is: the 26 framing symbols are charged to Eb, and at 2048 payload symbols that
/// overhead is 0.05 dB — small enough that the oracle gate reads the detector rather than the
/// frame.
pub const PAYLOAD_SYMBOLS: usize = 2_048;

/// The unique word, over the same octal sequence the orthogonal M-FSK entry aligns on
/// ([`super::orthogonal::UW`]) reduced modulo M: one searched-alignment word for both
/// orthogonal-signalling entries, so a difference between their curves is never the framing.
#[must_use]
pub fn unique_word(m: usize) -> Vec<u8> {
    super::orthogonal::unique_word(m)
}

/// Lead filler at alphabet M — the M-FSK entry's stream, for the same reason.
#[must_use]
pub fn filler(m: usize, len: usize) -> Vec<u8> {
    super::orthogonal::filler(m, len)
}

#[must_use]
pub fn modulate(m: usize, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut modulator = PpmMod::new(m, SLOT_SPS, 0.0, 1.0);
    let mut out = Vec::new();
    modulator.modulate(symbols, &mut out);
    out
}

/// A receiver for one alphabet and tier, sized to a whole framed trial.
#[must_use]
pub fn demod(m: usize, payload_symbols: usize, detector: SlotDetector) -> PpmDemod {
    PpmDemod::new(
        m,
        SLOT_SPS,
        0,
        LEAD + unique_word(m).len() + payload_symbols,
        0.0,
        detector,
    )
}

/// One (alphabet, tier) chain as a payload-to-payload [`Link`]: filler + unique word + payload
/// through [`PpmMod`], the frame's position *searched* rather than assumed — the engine's own
/// feedforward sub-slot estimate and its §3.4 known-symbol alignment — payload read off the
/// slot argmax.
#[must_use]
pub fn link_sized(m: usize, payload_symbols: usize, detector: SlotDetector) -> Link {
    let bits_per_symbol = m.trailing_zeros() as usize;
    let word = unique_word(m);
    let tx_word = word.clone();
    let receiver = demod(m, payload_symbols, detector);
    let tier = match detector {
        SlotDetector::MatchedFilter => "matched filter",
        SlotDetector::Envelope => "envelope",
    };
    Link {
        label: format!(
            "{m}-PPM, {SLOT_RATE} slots/s at {RATE} Hz ({SLOT_SPS} samples/slot) -> {tier} + \
             argmax, feedforward sub-slot timing and searched word alignment, {LEAD}+{} symbol \
             overhead in Eb, release",
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
            // Nothing has told the receiver where the frame starts: the sub-slot phase comes
            // from the concentration estimate over the lead-in and the word, and the slot
            // position from the word itself.
            let scan_slots = (LEAD + word.len()) * m;
            let offset = receiver.estimate_offset(wave, 0, scan_slots);
            let tail = wave.get(offset..).unwrap_or(&[]);
            let first_slot = receiver.align(tail, &word, LEAD * m + SEARCH_SLOTS);
            let mut symbols = Vec::with_capacity(payload_symbols);
            receiver.demodulate(
                tail,
                first_slot + word.len() * m,
                payload_symbols,
                &mut symbols,
            );
            let mut bits = Vec::with_capacity(payload_symbols * bits_per_symbol);
            for k in 0..payload_symbols {
                let symbol = symbols.get(k).copied().unwrap_or(0);
                push_symbol_bits(symbol, bits_per_symbol, &mut bits);
            }
            bits
        }),
    }
}

#[must_use]
pub fn ppm2_matched_link() -> Link {
    link_sized(2, PAYLOAD_SYMBOLS, SlotDetector::MatchedFilter)
}

#[must_use]
pub fn ppm2_envelope_link() -> Link {
    link_sized(2, PAYLOAD_SYMBOLS, SlotDetector::Envelope)
}

#[must_use]
pub fn ppm4_matched_link() -> Link {
    link_sized(4, PAYLOAD_SYMBOLS, SlotDetector::MatchedFilter)
}

// --- Committed sweep parameters ----------------------------------------------------------------

pub const M2_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0];
pub const M4_GRID: &[f64] = &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
/// The envelope tier's grid runs further right: it is the one that has to reach 1e-4 from
/// behind the matched tier.
pub const ENVELOPE_GRID: &[f64] = &[9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0];

pub const M2_SEED: u64 = 0x0bb2;
pub const M4_SEED: u64 = 0x0bb4;
pub const ENVELOPE_SEED: u64 = 0x0bbe;

pub const FULL_CAP: u64 = 3_000_000;

/// Worst horizontal distance from the exact noncoherent orthogonal closed form the matched tier
/// is held to. Measured: +0.099 dB at M = 2 and +0.098 at M = 4, of which 0.05 dB is the framing
/// overhead charged to Eb. The gate is set well above that and still an order tighter than the
/// tier margin it must not swallow.
pub const ORACLE_TOLERANCE_DB: f64 = 0.4;

pub const M2_MATCHED_AWGN: &str = "ppm/ppm2_matched_awgn";
pub const M2_ENVELOPE_AWGN: &str = "ppm/ppm2_envelope_awgn";
pub const M4_MATCHED_AWGN: &str = "ppm/ppm4_matched_awgn";
pub const MATCHED_LIMITS: &str = "ppm/ppm2_matched_limits";
pub const ENVELOPE_LIMITS: &str = "ppm/ppm2_envelope_limits";
pub const PERF: &str = "ppm/ppm_perf";

fn m2_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(2, ebn0_db)
}

fn m4_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(4, ebn0_db)
}

pub const MEASUREMENTS: &[Measurement] = &[
    Measurement {
        reference: Reference::Oracle {
            name: "exact noncoherent orthogonal 2-ary",
            ber: m2_ber,
            tolerance_db: ORACLE_TOLERANCE_DB,
        },
        ..Measurement::committed(
            M2_MATCHED_AWGN,
            ppm2_matched_link,
            M2_GRID,
            M2_SEED,
            FULL_CAP,
        )
    },
    Measurement {
        reference: Reference::Oracle {
            name: "exact noncoherent orthogonal 4-ary",
            ber: m4_ber,
            tolerance_db: ORACLE_TOLERANCE_DB,
        },
        ..Measurement::committed(
            M4_MATCHED_AWGN,
            ppm4_matched_link,
            M4_GRID,
            M4_SEED,
            FULL_CAP,
        )
    },
    Measurement::committed(
        M2_ENVELOPE_AWGN,
        ppm2_envelope_link,
        ENVELOPE_GRID,
        ENVELOPE_SEED,
        FULL_CAP,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tiers, both alphabets, no noise: a defect in framing, alignment or bit packing is
    /// loud before any statistics are involved.
    #[test]
    fn every_tier_round_trips_on_a_clean_channel() {
        for (m, detector) in [
            (2usize, SlotDetector::MatchedFilter),
            (2, SlotDetector::Envelope),
            (4, SlotDetector::MatchedFilter),
            (4, SlotDetector::Envelope),
        ] {
            let link = link_sized(m, 32, detector);
            let bits: Vec<bool> = (0..link.bits_per_trial).map(|i| i % 5 < 2).collect();
            let wave = (link.modulate)(&bits);
            assert_eq!((link.demodulate)(&wave), bits, "M = {m} {detector:?}");
        }
    }
}
