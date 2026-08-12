//! M-PPM detection: the slot statistics, the argmax, and the soft output.
//!
//! Two detectors, because the catalog's two consumers of this engine want different things and
//! the difference is a measured number rather than a preference:
//!
//! - [`SlotDetector::MatchedFilter`] integrates the *samples* of a slot and squares the result.
//!   That is the optimal statistic for an equal-energy orthogonal set with unknown carrier
//!   phase, and it is why this tier sits on the noncoherent orthogonal closed form: the slot
//!   statistic's noise power is one sample's, however many samples the slot spans.
//! - [`SlotDetector::Envelope`] sums the *magnitudes* a scanning receiver already computed.
//!   Every sample contributes its own rectified noise mean, so the tier pays a measured penalty
//!   — recorded in `CATALOG.md` — and buys back the thing Mode S needs: a statistic that costs
//!   one magnitude per sample for a receiver hunting bursts in a wideband stream, and that no
//!   carrier offset or phase drift inside a slot can cancel.
//!
//! Soft output follows the same split, and the type system carries it. The matched-filter
//! statistic is calibrated — normalised so a noise-only slot reads mean `N0` — so it goes
//! through the crate's one energy demapper as a true [`Llr`]. The envelope statistic is a
//! confidence on the receiver's own scale and comes back a [`SoftBit`]; calling it an LLR would
//! be the exact mistake `soft`'s two types exist to prevent.

use num_complex::Complex;

use super::grid::SlotGrid;
use crate::{
    constellation::demap::energy_llrs,
    soft::{Llr, SoftBit, argmax},
};

/// Which slot statistic the receiver forms. See the module docs for the measured trade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotDetector {
    /// `|Σ wₙ·xₙ|²`, normalised so a noise-only slot reads mean N0.
    MatchedFilter,
    /// `Σ wₙ·|xₙ|` over pre-computed magnitudes.
    Envelope,
}

/// One M-PPM receiver at one assumed sub-sample phase.
#[derive(Clone, Debug)]
pub struct PpmDemod {
    m: usize,
    grid: SlotGrid,
    detector: SlotDetector,
    /// Per slot, `1/√(Σ wₙ²)`: what turns the matched-filter sum into a statistic whose
    /// noise-only mean is N0 regardless of slot width or where the boundaries fell.
    norm: Vec<f32>,
}

impl PpmDemod {
    /// A receiver for `symbols` M-PPM symbols starting `phase` samples into its window,
    /// preceded by `lead_slots` slots the caller owns (a protocol preamble, a guard) so its
    /// slot arithmetic and this grid's are the same arithmetic.
    ///
    /// # Panics
    /// If `m` is not a power of two of at least two, `symbols` is zero, or as
    /// [`SlotGrid::new`].
    #[must_use]
    pub fn new(
        m: usize,
        samples_per_slot: f64,
        lead_slots: usize,
        symbols: usize,
        phase: f64,
        detector: SlotDetector,
    ) -> Self {
        assert!(
            m >= 2 && m.is_power_of_two(),
            "M must be a power of two of at least 2, got {m}"
        );
        assert!(symbols > 0, "a receiver needs at least one symbol to read");
        Self::over(
            m,
            SlotGrid::new(samples_per_slot, lead_slots + m * symbols, phase),
            detector,
        )
    }

    /// A receiver over a grid the caller built — the entry point for a protocol whose slot
    /// timeline is not a whole number of M-PPM symbols (Mode S prefixes a four-pulse preamble).
    ///
    /// # Panics
    /// If `m` is not a power of two of at least two.
    #[must_use]
    pub fn over(m: usize, grid: SlotGrid, detector: SlotDetector) -> Self {
        assert!(
            m >= 2 && m.is_power_of_two(),
            "M must be a power of two of at least 2, got {m}"
        );
        // Only the matched filter divides by its noise gain; the envelope tier reads a scale
        // of its own and would pay a square root per slot for a number it never uses.
        let norm = match detector {
            SlotDetector::MatchedFilter => (0..grid.slots())
                .map(|slot| grid.weight_energy(slot).sqrt().recip())
                .collect(),
            SlotDetector::Envelope => Vec::new(),
        };
        Self {
            m,
            grid,
            detector,
            norm,
        }
    }

    /// Every assumed sub-sample phase of one configuration — the set a receiver tries when the
    /// transmitter's clock phase is unknown, and the CRC (or the sync correlation) arbitrates.
    ///
    /// # Panics
    /// As [`Self::new`], plus if `tables` is zero.
    #[must_use]
    pub fn phases(
        m: usize,
        samples_per_slot: f64,
        lead_slots: usize,
        symbols: usize,
        tables: usize,
        detector: SlotDetector,
    ) -> Vec<Self> {
        SlotGrid::phases(samples_per_slot, lead_slots + m * symbols, tables)
            .into_iter()
            .map(|grid| Self::over(m, grid, detector))
            .collect()
    }

    #[must_use]
    pub fn m(&self) -> usize {
        self.m
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> u32 {
        self.m.trailing_zeros()
    }

    #[must_use]
    pub fn grid(&self) -> &SlotGrid {
        &self.grid
    }

    #[must_use]
    pub fn detector(&self) -> SlotDetector {
        self.detector
    }

    /// One slot's statistic, through whichever tier this receiver carries.
    fn statistic(&self, window: &[Complex<f32>], slot: usize) -> f32 {
        match self.detector {
            SlotDetector::MatchedFilter => {
                (self.grid.integrate(window, slot) * self.slot_norm(slot)).norm_sqr()
            }
            SlotDetector::Envelope => self.grid.envelope(window, slot),
        }
    }

    /// The M slot statistics of the symbol whose first slot is `first_slot`, written into `out`.
    /// `window` starts at the burst's first sample. Both tiers read complex baseband here, so a
    /// chain that changes detector changes one constructor argument and nothing else. Zero
    /// allocation — the hot path of every consumer.
    ///
    /// # Panics
    /// If `out.len() != m`.
    pub fn statistics_at(&self, window: &[Complex<f32>], first_slot: usize, out: &mut [f32]) {
        assert_eq!(out.len(), self.m, "one statistic per slot");
        for (k, slot) in out.iter_mut().enumerate() {
            *slot = self.statistic(window, first_slot + k);
        }
    }

    /// [`Self::statistics_at`] over *pre-computed* sample magnitudes — the entry point for a
    /// receiver that scans a wideband stream and therefore already holds a magnitude buffer
    /// (`channels::adsb`), where recomputing one per candidate window would double its
    /// per-sample cost.
    ///
    /// # Panics
    /// If `out.len() != m`, or if this receiver's detector is [`SlotDetector::MatchedFilter`],
    /// which cannot work from magnitudes at all — the phase is what it integrates.
    pub fn envelope_at(&self, magnitudes: &[f32], first_slot: usize, out: &mut [f32]) {
        assert_eq!(out.len(), self.m, "one statistic per slot");
        assert_eq!(
            self.detector,
            SlotDetector::Envelope,
            "the matched filter integrates samples, not magnitudes; call statistics_at"
        );
        for (k, slot) in out.iter_mut().enumerate() {
            *slot = self.grid.energy(magnitudes, first_slot + k);
        }
    }

    /// Normalisation of one slot's matched-filter sum; 1.0 past the grid, where the sum is zero
    /// anyway and the statistic is "nothing here" either way.
    fn slot_norm(&self, slot: usize) -> f32 {
        self.norm.get(slot).copied().unwrap_or(1.0)
    }

    /// `symbols` consecutive M-PPM symbols from `first_slot`, hard-decided, appended to `out`.
    pub fn demodulate(
        &self,
        window: &[Complex<f32>],
        first_slot: usize,
        symbols: usize,
        out: &mut Vec<u8>,
    ) {
        let mut stats = [0.0f32; MAX_SLOTS];
        let stats = &mut stats[..self.m];
        for symbol in 0..symbols {
            self.statistics_at(window, first_slot + symbol * self.m, stats);
            out.push(argmax(stats));
        }
    }

    /// Feedforward burst timing: the whole-sample offset in `0..ceil(samples_per_slot)` whose
    /// grid reads `slots` slots from `first_slot` as the most *concentrated* energy. Ties keep
    /// the earliest offset, so a burst already on the grid estimates 0.
    ///
    /// The metric is `Σ stat²` over the slots, not the peak per symbol, and the difference is
    /// load-bearing: a whole-slot shift moves a pulse from one symbol's window into its
    /// neighbour's, so a per-symbol peak silently rewards or punishes offsets according to the
    /// *data* — measured as an estimator that preferred a 1-sample error on a burst whose first
    /// two symbols were 1 then 0. A sum over slots cannot see the pairing at all, and a pulse
    /// split across two slots always scores less than one inside a single slot.
    ///
    /// The search is deliberately *shorter than one slot*, because past that the maximisation
    /// is blind: a burst read one whole slot late still puts every pulse entirely inside some
    /// slot, so the concentration is identical and only a known sequence can tell the two apart
    /// — which is [`Self::align`]'s job. Below one sample it is blind for the opposite reason: a
    /// whole-sample search cannot express a sub-sample phase, and the answer to that is a grid
    /// per phase ([`Self::phases`]), not a finer loop.
    #[must_use]
    pub fn estimate_offset(
        &self,
        window: &[Complex<f32>],
        first_slot: usize,
        slots: usize,
    ) -> usize {
        let span = (self.grid.samples_per_slot().ceil() as usize).max(1);
        let mut best = (0usize, f64::NEG_INFINITY);
        for offset in 0..span {
            let tail = window.get(offset..).unwrap_or(&[]);
            let score: f64 = (0..slots)
                .map(|k| {
                    let stat = f64::from(self.statistic(tail, first_slot + k));
                    stat * stat
                })
                .sum();
            if score > best.1 {
                best = (offset, score);
            }
        }
        best.0
    }

    /// The §3.4 known-symbol hook for this engine: the first slot in `0..=search` at which
    /// `known`'s symbols best explain what the receiver sees, scored as collected evidence
    /// rather than as agreement.
    ///
    /// Soft rather than hard for the reason the CPM substrate's `find_uw` records: a hard-sliced
    /// match throws away exactly the confidence that separates the true position from a
    /// neighbour, and a slot grid has neighbours that still slice perfectly — at 8 samples per
    /// slot, a burst read three samples early decodes every symbol of the word and loses 4 dB
    /// doing it. The score is the expected slot's statistic minus the mean of the others, summed
    /// over the word, which peaks only where the pulses are centred.
    ///
    /// # Panics
    /// If `known` is empty.
    #[must_use]
    pub fn align(&self, window: &[Complex<f32>], known: &[u8], search: usize) -> usize {
        assert!(!known.is_empty(), "no known symbols, no alignment");
        let mut stats = [0.0f32; MAX_SLOTS];
        let stats = &mut stats[..self.m];
        let others = (self.m - 1) as f64;
        let mut best = (0usize, f64::NEG_INFINITY);
        for first_slot in 0..=search {
            let mut score = 0.0f64;
            for (k, &symbol) in known.iter().enumerate() {
                self.statistics_at(window, first_slot + k * self.m, stats);
                let expect = f64::from(stats[symbol as usize & (self.m - 1)]);
                let total: f64 = stats.iter().map(|&s| f64::from(s)).sum();
                score += expect - (total - expect) / others;
            }
            if score > best.1 {
                best = (first_slot, score);
            }
        }
        best.0
    }
}

/// Largest M any single-symbol scratch buffer here holds. 256 slots is four orders past the
/// alphabets PPM is used at (Mode S is 2, optical links 4–16) and keeps the stack scratch that
/// makes [`PpmDemod::demodulate`] allocation-free honest rather than generous.
pub const MAX_SLOTS: usize = 256;

/// Bits one symbol of [`MAX_SLOTS`] slots labels — the soft-output scratch's size.
pub const MAX_SLOT_BITS: usize = MAX_SLOTS.trailing_zeros() as usize;

/// Per-bit LLRs of one symbol's matched-filter statistics, through the crate's one energy
/// demapper (`constellation::demap::energy_llrs`): the slot index is the bit label, and
/// `noise_var` is N0 — the same normalisation [`PpmDemod::statistics_at`] delivers, which is
/// what makes these true LLRs rather than confidences.
///
/// # Panics
/// As [`energy_llrs`].
pub fn llrs(statistics: &[f32], noise_var: f64, out: &mut [Llr]) {
    energy_llrs(statistics, noise_var, out);
}

/// Per-bit confidences of one symbol's *envelope* statistics: the same max-log difference, but
/// scaled by the winning slot instead of a measured variance, so the result is a [`SoftBit`] on
/// the receiver's own scale — ±1 for a clean symbol, per the crate's convention. An envelope
/// sum's noise is neither zero-mean nor of known variance; calling this an LLR would be a
/// calibration claim nothing here can back.
///
/// # Panics
/// As [`energy_llrs`], plus if `out.len()` is not log₂ of the slot count.
pub fn soft_bits(statistics: &[f32], out: &mut [SoftBit]) {
    let peak = statistics.iter().copied().fold(0.0f32, f32::max);
    // A dead window votes for nothing rather than voting confidently for the tie rule's winner.
    let scale = if peak > 0.0 { f64::from(peak) } else { 1.0 };
    let mut llrs = [Llr(0.0); MAX_SLOT_BITS];
    let llrs = &mut llrs[..out.len()];
    energy_llrs(statistics, scale, llrs);
    for (slot, &llr) in out.iter_mut().zip(llrs.iter()) {
        *slot = SoftBit(llr.0.clamp(-1.0, 1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{grid::magnitudes, modulator::PpmMod},
        *,
    };

    fn modulated(m: usize, sps: f64, symbols: &[u8]) -> Vec<Complex<f32>> {
        let mut modulator = PpmMod::new(m, sps, 0.0, 1.0);
        let mut out = Vec::new();
        modulator.modulate(symbols, &mut out);
        out
    }

    fn demod(m: usize, sps: f64, symbols: usize, detector: SlotDetector) -> PpmDemod {
        PpmDemod::new(m, sps, 0, symbols, 0.0, detector)
    }

    /// The round trip, on both detectors: what the modulator keyed is what the argmax reads.
    #[test]
    fn every_symbol_round_trips_through_both_detectors() {
        let symbols: Vec<u8> = (0..64).map(|i| (i * 5 % 8) as u8).collect();
        let wave = modulated(8, 4.0, &symbols);
        let mut mag = Vec::new();
        magnitudes(&wave, &mut mag);

        let matched = demod(8, 4.0, symbols.len(), SlotDetector::MatchedFilter);
        let mut decoded = Vec::new();
        matched.demodulate(&wave, 0, symbols.len(), &mut decoded);
        assert_eq!(decoded, symbols);

        let envelope = demod(8, 4.0, symbols.len(), SlotDetector::Envelope);
        let mut stats = [0.0f32; 8];
        let envelope_decoded: Vec<u8> = (0..symbols.len())
            .map(|s| {
                envelope.envelope_at(&mag, s * 8, &mut stats);
                argmax(&stats)
            })
            .collect();
        assert_eq!(envelope_decoded, symbols);
    }

    /// The calibration claim behind the matched tier's LLRs: a noise-only slot's statistic has
    /// mean N0, whatever the slot width. Without it the demapper's `noise_var` would be a
    /// fudge factor and the LLRs confidences wearing the wrong type.
    #[test]
    fn a_noise_only_slot_reads_mean_n0() {
        use crate::ber::rng::Rng;
        for &sps in &[2.0, 7.0, 16.0] {
            let mut rng = Rng::new(0x9711);
            // Complex noise of total variance 1: each component σ² = ½.
            let sigma = (0.5f64).sqrt();
            let noise: Vec<Complex<f32>> = (0..(sps as usize) * 4_000)
                .map(|_| Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32))
                .collect();
            let d = demod(2, sps, 2_000, SlotDetector::MatchedFilter);
            let mut stats = [0.0f32; 2];
            let mut sum = 0.0f64;
            for symbol in 0..2_000 {
                d.statistics_at(&noise, symbol * 2, &mut stats);
                sum += f64::from(stats[0]) + f64::from(stats[1]);
            }
            let mean = sum / 4_000.0;
            assert!(
                (mean - 1.0).abs() < 0.05,
                "sps {sps}: mean {mean} vs N0 = 1"
            );
        }
    }

    /// The tie rule, stated as behaviour: silence decodes to the last slot, deterministically.
    #[test]
    fn a_dead_window_decodes_to_the_last_slot() {
        let d = demod(4, 3.0, 4, SlotDetector::MatchedFilter);
        let mut decoded = Vec::new();
        d.demodulate(&vec![Complex::new(0.0, 0.0); 48], 0, 4, &mut decoded);
        assert_eq!(decoded, vec![3, 3, 3, 3]);
        assert_eq!(argmax(&[1.0, 1.0]), 1);
        assert_eq!(argmax(&[1.0, 0.5]), 0);
    }

    /// Soft output carries the right sign for every bit of the detected symbol, and a dead
    /// window abstains rather than voting confidently for the tie winner.
    #[test]
    fn soft_bits_agree_with_the_hard_decision_and_abstain_on_silence() {
        let stats = [0.1f32, 4.0, 0.2, 0.05];
        let mut soft = [SoftBit(0.0); 2];
        soft_bits(&stats, &mut soft);
        // Slot 1 = bits (b0, b1) = (1, 0), positive means 1 (crate-root convention).
        assert!(soft[0].0 > 0.0 && soft[1].0 < 0.0, "{soft:?}");
        assert!(soft.iter().all(|s| s.0.abs() <= 1.0), "{soft:?}");

        let mut soft = [SoftBit(0.0); 2];
        soft_bits(&[0.0; 4], &mut soft);
        assert!(soft.iter().all(|s| s.is_erasure()), "{soft:?}");
    }

    /// LLR magnitudes must grow with separation — the property a FEC downstream actually
    /// consumes, and the one a mis-normalised statistic would silently invert.
    #[test]
    fn llr_magnitude_grows_with_slot_separation() {
        let mut weak = [Llr(0.0); 1];
        let mut strong = [Llr(0.0); 1];
        llrs(&[1.0, 1.2], 1.0, &mut weak);
        llrs(&[1.0, 9.0], 1.0, &mut strong);
        assert!(strong[0].0 > weak[0].0 && weak[0].0 > 0.0);
    }

    /// The two timing primitives together, on the burst they are written for: a frame that
    /// starts at an arbitrary sample is found by the sub-slot estimate and the known word, and
    /// what is found is the *centred* alignment — not merely one that happens to slice
    /// correctly, which is the failure mode a hard-agreement search walks into.
    #[test]
    fn the_timing_estimate_and_the_known_word_find_the_frame() {
        let word = [1u8, 0, 0, 1, 1, 1, 0, 1, 0, 0, 1, 0];
        let payload: Vec<u8> = (0..64).map(|i| (i * 7 % 2) as u8).collect();
        let mut sent = word.to_vec();
        sent.extend_from_slice(&payload);
        for lead_samples in [0usize, 3, 8, 11, 16, 27] {
            let mut wave = vec![Complex::new(0.0, 0.0); lead_samples];
            modulated(2, 8.0, &sent)
                .into_iter()
                .for_each(|s| wave.push(s));
            let receiver = demod(2, 8.0, sent.len() + 4, SlotDetector::MatchedFilter);
            let offset = receiver.estimate_offset(&wave, 0, 16);
            assert_eq!(offset, lead_samples % 8, "lead {lead_samples}");
            let tail = &wave[offset..];
            let first_slot = receiver.align(tail, &word, 8);
            assert_eq!(first_slot, lead_samples / 8, "lead {lead_samples}");
            let mut decoded = Vec::new();
            receiver.demodulate(
                tail,
                first_slot + word.len() * 2,
                payload.len(),
                &mut decoded,
            );
            assert_eq!(decoded, payload, "lead {lead_samples}");
        }
    }

    /// A receiver at the wrong phase must still be *a* receiver: the phase set exists so that
    /// something downstream can pick, and every member has to be able to read a burst that
    /// happens to sit on its grid.
    #[test]
    fn every_phase_table_reads_a_burst_aligned_to_it() {
        let symbols: Vec<u8> = (0..32).map(|i| (i % 4) as u8).collect();
        for (k, receiver) in
            PpmDemod::phases(4, 2.4, 0, symbols.len(), 8, SlotDetector::MatchedFilter)
                .iter()
                .enumerate()
        {
            let phase = k as f64 / 8.0;
            let mut modulator = PpmMod::new(4, 2.4, phase, 1.0);
            let mut wave = Vec::new();
            modulator.modulate(&symbols, &mut wave);
            let mut decoded = Vec::new();
            receiver.demodulate(&wave, 0, symbols.len(), &mut decoded);
            assert_eq!(decoded, symbols, "phase {phase}");
        }
    }
}
