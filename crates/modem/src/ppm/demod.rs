use num_complex::Complex;

use super::grid::SlotGrid;
use crate::{
    constellation::demap::energy_llrs,
    soft::{Llr, SoftBit, argmax},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotDetector {
    MatchedFilter,
    Envelope,
}

#[derive(Clone, Debug)]
pub struct PpmDemod {
    m: usize,
    grid: SlotGrid,
    detector: SlotDetector,
    norm: Vec<f32>,
}

impl PpmDemod {
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

    #[must_use]
    pub fn over(m: usize, grid: SlotGrid, detector: SlotDetector) -> Self {
        assert!(
            m >= 2 && m.is_power_of_two(),
            "M must be a power of two of at least 2, got {m}"
        );
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

    fn statistic(&self, window: &[Complex<f32>], slot: usize) -> f32 {
        match self.detector {
            SlotDetector::MatchedFilter => {
                (self.grid.integrate(window, slot) * self.slot_norm(slot)).norm_sqr()
            }
            SlotDetector::Envelope => self.grid.envelope(window, slot),
        }
    }

    pub fn statistics_at(&self, window: &[Complex<f32>], first_slot: usize, out: &mut [f32]) {
        assert_eq!(out.len(), self.m, "one statistic per slot");
        for (k, slot) in out.iter_mut().enumerate() {
            *slot = self.statistic(window, first_slot + k);
        }
    }

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

    fn slot_norm(&self, slot: usize) -> f32 {
        self.norm.get(slot).copied().unwrap_or(1.0)
    }

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

pub const MAX_SLOTS: usize = 256;

pub const MAX_SLOT_BITS: usize = MAX_SLOTS.trailing_zeros() as usize;

pub fn llrs(statistics: &[f32], noise_var: f64, out: &mut [Llr]) {
    energy_llrs(statistics, noise_var, out);
}

pub fn soft_bits(statistics: &[f32], out: &mut [SoftBit]) {
    let peak = statistics.iter().copied().fold(0.0f32, f32::max);
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

    #[test]
    fn a_noise_only_slot_reads_mean_n0() {
        use crate::ber::rng::Rng;
        for &sps in &[2.0, 7.0, 16.0] {
            let mut rng = Rng::new(0x9711);
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

    #[test]
    fn a_dead_window_decodes_to_the_last_slot() {
        let d = demod(4, 3.0, 4, SlotDetector::MatchedFilter);
        let mut decoded = Vec::new();
        d.demodulate(&vec![Complex::new(0.0, 0.0); 48], 0, 4, &mut decoded);
        assert_eq!(decoded, vec![3, 3, 3, 3]);
        assert_eq!(argmax(&[1.0, 1.0]), 1);
        assert_eq!(argmax(&[1.0, 0.5]), 0);
    }

    #[test]
    fn soft_bits_agree_with_the_hard_decision_and_abstain_on_silence() {
        let stats = [0.1f32, 4.0, 0.2, 0.05];
        let mut soft = [SoftBit(0.0); 2];
        soft_bits(&stats, &mut soft);
        assert!(soft[0].0 > 0.0 && soft[1].0 < 0.0, "{soft:?}");
        assert!(soft.iter().all(|s| s.0.abs() <= 1.0), "{soft:?}");

        let mut soft = [SoftBit(0.0); 2];
        soft_bits(&[0.0; 4], &mut soft);
        assert!(soft.iter().all(|s| s.is_erasure()), "{soft:?}");
    }

    #[test]
    fn llr_magnitude_grows_with_slot_separation() {
        let mut weak = [Llr(0.0); 1];
        let mut strong = [Llr(0.0); 1];
        llrs(&[1.0, 1.2], 1.0, &mut weak);
        llrs(&[1.0, 9.0], 1.0, &mut strong);
        assert!(strong[0].0 > weak[0].0 && weak[0].0 > 0.0);
    }

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
