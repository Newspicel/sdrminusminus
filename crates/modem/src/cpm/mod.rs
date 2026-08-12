//! The M-ary CPM/CPFSK engine (MODEM-PLAN §3.1 `cpm/`) — the one implementation thirteen of
//! the channels stand on (APRS is the fourteenth, waiting on a centre-tracking axis; see
//! `CATALOG.md`'s AFSK row). Everything that distinguishes one entry from another is
//! **data** ([`CpmParams`]): M and the symbol→level table ([`Mapping`]), the modulation index
//! in its one canonical form (h, converted from a deviation set at construction), the
//! frequency pulse (unit-area taps from [`crate::pulse`], full or partial response), and the
//! oversampling. A `match` on a specific standard anywhere inside this module is a defect
//! (§3.3, §8). The five phase-3 probes each stress one axis of that space and none needed a
//! special case:
//!
//! | Probe  | Axis | As data |
//! |---|---|---|
//! | POCSAG | 2-level base | M = 2, rect pulse, h from ±4.5 kHz deviation |
//! | DMR    | 4-level, RRC, TDMA | ETSI dibit table, RRC pulse, burst gate + [`KnownSymbols`] |
//! | D-STAR | Gaussian partial response | `gaussian_freq` pulse, `gaussian` receive filter, h = ½ |
//! | AIS    | parameterisation | the same Gaussian entry at BT 0.4, 9600 baud, 5 sps |
//! | APRS   | real-valued audio input | [`CpmDemod::real`] with a [`RealDetector`] |
//!
//! **Two detection tiers.** [`CpmDemod`] is the discriminator tier: one soft symbol per symbol
//! period, sliced where its pulse peaks. [`MlseDetector`] is the sequence-detection tier that
//! sits on those same soft symbols and decides over the whole span an entry's pulse touches —
//! the answer for *partial-response* entries, whose symbols overlap by construction. It is
//! derived entirely from the entry's data (the frequency pulse through the receive filter), so
//! an ISI-free entry's trellis collapses to one state and the tier reduces to the slicer,
//! which is the honest statement that such an entry had nothing to gain. Measured where there
//! is something to gain: GMSK BT = 0.3 improves 8.15 dB at BER 1e-3, BT = 0.5 by 1.92 dB
//! (see [`mlse`](self::MlseDetector) and `CATALOG.md`).
//!
//! **Chain** (discriminator tier): `carrier gate → detector → matched receive filter →
//! SymbolSync → M-level normalisation → soft symbols` — one `f32` per symbol period, gaps
//! included, levels on the mapping table's own scale. Slicing ([`Mapping::slice`]), per-bit
//! soft output ([`Mapping::soft_bits`], positive = 1) and the known-symbol correction
//! ([`KnownSymbols`]) sit on top of that stream. [`CpmMod`] is the matching reference
//! transmitter; `testgen`'s per-mode recipes migrate onto it so modulator and demodulator can
//! never drift apart (§1.2). Timing recovery is `sdrmm_dsp::SymbolSync` — the one timing
//! stack (§3.2), TDMA coast included; this module schedules it (per-gate-run
//! `process`/`process_held`) and never reimplements it.
//!
//! **The phase-0 finding this engine had to beat** (`crates/channels/tests/dmr_baseline.rs`):
//! on continuous random 4FSK the old `Fsk4Demod` — `sdrmm_dsp`'s four-level front end, deleted
//! at the end of this phase, which every later `fsk4` in these docs names — wandered into a
//! ~1e-2 dibit-BER floor past
//! ~2000 symbols — masked in TDMA operation, where the gate freezes the loop through every
//! gap. Measured here before designing the fix: the cause is Gardner self-noise — mid-point
//! ISI on the transitions the symmetry gate cannot reject — integrating into the timing loop
//! at `fsk4`'s hard-coded 0.015 cycles/symbol bandwidth, walking the rate estimate ±0.09 % on
//! a signal with *zero* clock error (six clean 20k-symbol runs: 879 symbol errors; the centre
//! estimate's data-driven wobble roughly doubles the count on top). At 0.003 the same runs
//! show one error total. But 0.015 is not a mistake to delete: the committed DMR limits row
//! (23 047 ppm sample-clock pull-in through an 88-symbol preamble) is only reachable at that
//! width — the two regimes genuinely want different loops. So the bandwidth is **per-entry
//! data** with two measured operating points, [`TIMING_BW_BURST`] and
//! [`TIMING_BW_CONTINUOUS`]; the continuous-stream test in `demod::tests` holds the engine to
//! ≤ 1e-3 over 20k symbols where the old chain floors at 1e-2 — measured, not hoped.
//!
//! **Real-valued domains** (§3.5): audio-carried FSK (APRS's AFSK-in-NFM, ACARS's MSK-in-AM)
//! enters as real samples through [`CpmDemod::real`]; the detector — analytic-signal
//! discriminator about a subcarrier, or a two-tone correlator filterbank — is a
//! [`RealDetector`] value on the entry, and everything downstream of it (matched filter,
//! timing, gate policy, levels, hook) is the same code the complex path runs.
//!
//! **The measured M = 8 boundary.** Blind, magnitude-only level normalisation — the peak
//! tracker `fsk4` introduced, twice corrected here (see `demod`'s `PEAK_SYMBOLS`) — holds the
//! level scale to about ×1.09 of true on an 8-level alphabet, because the estimator's
//! systematic errors are of the same order as the 14 % slicing margin it must defend; on
//! M ≤ 4's 33 % margins the same machinery is ample. So an 8-level entry carries its level
//! reference the way §3.4 prescribes and every burst standard already does: known symbols
//! through [`KnownSymbols`], whose per-anchor least-squares gain fit restores the scale
//! exactly — the committed 8-level loopback decodes error-free through the hook, and the
//! numbers behind the boundary are in the demod constants' docs. The MLSE tier (§7 phase-3
//! follow-on) is the designed path for arbitrary-payload high-M without embedded references.

mod demod;
mod levels;
mod mlse;
mod modulator;
mod params;

pub use demod::{CpmDemod, RealDetector, TIMING_BW_BURST, TIMING_BW_CONTINUOUS};
pub use levels::KnownSymbols;
pub use mlse::{MlseDetector, SymbolResponse};
pub use modulator::CpmMod;
pub use params::{CpmParams, Mapping};
