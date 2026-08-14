//! Where a symbol's slots fall on the sample grid, when the two grids owe each other nothing.
use num_complex::Complex;

/// One slot's window: where it starts and how many samples it touches.
#[derive(Clone, Copy, Debug)]
struct Slot {
    start: usize,
    /// Offset of this slot's run inside [`SlotGrid::weights`].
    at: usize,
    taps: usize,
}

/// The sample-domain boundaries of a run of slots at one assumed sub-sample phase.
///
/// Built once per (rate, phase) pair and then read per candidate burst — construction allocates,
/// reading does not.
#[derive(Clone, Debug)]
pub struct SlotGrid {
    /// `slots + 1` entries: the extra one is the boundary past the last slot, which is where a
    /// consumer that accepted a burst resumes.
    table: Vec<Slot>,
    weights: Vec<f32>,
    span: usize,
    samples_per_slot: f64,
    phase: f64,
}

impl SlotGrid {
    /// The boundaries of `slots` slots of `samples_per_slot` samples each, the first starting
    /// `phase` samples into the window.
    ///
    /// # Panics
    /// If `samples_per_slot` is not positive and finite, `phase` is outside `[0, 1)`, or `slots`
    /// is zero — a grid with no slots is a receiver with nothing to compare.
    #[must_use]
    pub fn new(samples_per_slot: f64, slots: usize, phase: f64) -> Self {
        assert!(
            samples_per_slot.is_finite() && samples_per_slot > 0.0,
            "samples per slot must be positive, got {samples_per_slot}"
        );
        assert!(
            phase.is_finite() && (0.0..1.0).contains(&phase),
            "phase is a sub-sample offset in [0, 1), got {phase}"
        );
        assert!(slots > 0, "a grid needs at least one slot");
        let mut weights = Vec::with_capacity((slots + 1) * (samples_per_slot as usize + 2));
        let mut table = Vec::with_capacity(slots + 1);
        for j in 0..=slots {
            let from = j as f64 * samples_per_slot + phase;
            let to = from + samples_per_slot;
            let start = from.floor() as usize;
            let taps = (to.ceil() as usize).saturating_sub(start).max(1);
            let at = weights.len();
            for i in 0..taps {
                let k = (start + i) as f64;
                weights.push((to.min(k + 1.0) - from.max(k)).max(0.0) as f32);
            }
            table.push(Slot { start, at, taps });
        }
        Self {
            table,
            weights,
            span: (slots as f64 * samples_per_slot + phase).ceil() as usize,
            samples_per_slot,
            phase,
        }
    }

    /// One grid per assumed sub-sample phase, `k / tables` of a sample apart — the set a
    /// receiver tries when the transmitter's clock phase is unknown. Eight tables bound the
    /// residual mismatch to a sixteenth of a sample; four was measured (in `channels::adsb`) to
    /// be too few at ~1 sample per slot, where some bit patterns' slot margins invert.
    ///
    /// # Panics
    /// As [`Self::new`], plus if `tables` is zero.
    #[must_use]
    pub fn phases(samples_per_slot: f64, slots: usize, tables: usize) -> Vec<Self> {
        assert!(tables > 0, "at least one phase table");
        (0..tables)
            .map(|k| Self::new(samples_per_slot, slots, k as f64 / tables as f64))
            .collect()
    }

    /// First sample of slot `slot`; 0 for a slot past the grid, so a caller reading the
    /// boundary past the end of a shorter burst gets a defined answer rather than a panic.
    #[must_use]
    pub fn start(&self, slot: usize) -> usize {
        self.table.get(slot).map_or(0, |s| s.start)
    }

    /// Samples the whole run of slots spans — the window a scan must have in hand.
    #[must_use]
    pub fn span(&self) -> usize {
        self.span
    }

    /// Slots the grid carries (the boundary entry past the last one is not one).
    #[must_use]
    pub fn slots(&self) -> usize {
        self.table.len() - 1
    }

    #[must_use]
    pub fn samples_per_slot(&self) -> f64 {
        self.samples_per_slot
    }

    #[must_use]
    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// `Σ wₙ²` over slot `slot` — the noise gain of its weighted sum, and so the normalisation
    /// a matched-filter statistic divides by to mean the same thing at every slot width and
    /// every phase. Zero past the grid.
    #[must_use]
    pub fn weight_energy(&self, slot: usize) -> f32 {
        let Some(&Slot { at, taps, .. }) = self.table.get(slot) else {
            return 0.0;
        };
        self.weights[at..at + taps].iter().map(|&w| w * w).sum()
    }

    /// Overlap-weighted sum of slot `slot` over pre-computed sample magnitudes — the
    /// **envelope** statistic: energy that has already lost its phase, which is what a receiver
    /// scanning for bursts has anyway (it computed magnitudes to find them). Samples past the
    /// end of `window` read as zero, so a truncated burst scores low instead of panicking.
    #[must_use]
    pub fn energy(&self, window: &[f32], slot: usize) -> f32 {
        self.weighted(window, slot, 0.0, |acc, w, x| acc + w * x)
    }

    /// [`energy`](Self::energy) straight off complex baseband — the same envelope statistic, for
    /// a receiver that has samples rather than a magnitude stream in hand. Identical arithmetic,
    /// one magnitude per touched sample.
    #[must_use]
    pub fn envelope(&self, window: &[Complex<f32>], slot: usize) -> f32 {
        self.weighted(window, slot, Complex::new(0.0, 0.0), |acc, w, x| {
            acc + Complex::new(w * x.norm(), 0.0)
        })
        .re
    }

    /// Overlap-weighted **coherent** sum of slot `slot` — the matched filter proper. Its squared
    /// magnitude is the optimal noncoherent statistic for an unknown carrier phase, and it beats
    /// [`energy`](Self::energy) by the margin the catalog's two PPM tiers record: summing
    /// magnitudes adds the noise's rectified mean to every slot, summing samples does not.
    #[must_use]
    pub fn integrate(&self, window: &[Complex<f32>], slot: usize) -> Complex<f32> {
        self.weighted(window, slot, Complex::new(0.0, 0.0), |acc, w, x| {
            acc + x * w
        })
    }

    /// The one weighted-sum loop both statistics are. Accumulation runs in ascending sample
    /// order and skips nothing inside the slot's run: the arithmetic is load-bearing for
    /// `channels::adsb`, whose committed behaviour is bit-identical to this sum.
    fn weighted<T: Copy>(
        &self,
        window: &[T],
        slot: usize,
        zero: T,
        add: impl Fn(T, f32, T) -> T,
    ) -> T {
        let Some(&Slot { start, at, taps }) = self.table.get(slot) else {
            return zero;
        };
        let mut acc = zero;
        for i in 0..taps {
            let w = self.weights[at + i];
            let x = window.get(start + i).copied().unwrap_or(zero);
            acc = add(acc, w, x);
        }
        acc
    }
}

pub fn magnitudes(iq: &[Complex<f32>], out: &mut Vec<f32>) {
    out.extend(iq.iter().map(|s| s.re.mul_add(s.re, s.im * s.im).sqrt()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The degenerate case the general form must contain: whole samples per slot, no phase, so
    /// every slot is a plain rectangular window of unit weights and nothing overlaps.
    #[test]
    fn an_integer_rate_at_phase_zero_is_the_plain_rectangular_window() {
        let grid = SlotGrid::new(4.0, 6, 0.0);
        assert_eq!(grid.span(), 24);
        assert_eq!(grid.slots(), 6);
        for slot in 0..6 {
            assert_eq!(grid.start(slot), slot * 4);
        }
        let window: Vec<f32> = (0..24).map(|i| i as f32).collect();
        assert_eq!(grid.energy(&window, 2), 8.0 + 9.0 + 10.0 + 11.0);
    }

    /// Weights are aperture overlaps, so each slot's must sum to its width however the
    /// boundaries fall — the property that keeps two slots' energies comparable at any phase.
    #[test]
    fn every_slot_weighs_exactly_one_slot_width() {
        for &sps in &[1.024, 1.2, 2.5, 8.0, 0.75] {
            for phase in [0.0, 0.13, 0.5, 0.87] {
                let grid = SlotGrid::new(sps, 40, phase);
                let ones = vec![1.0f32; grid.span() + 4];
                for slot in 0..grid.slots() {
                    let w = grid.energy(&ones, slot);
                    assert!(
                        (f64::from(w) - sps).abs() < 1e-5,
                        "sps {sps} phase {phase} slot {slot}: {w}"
                    );
                }
            }
        }
    }

    /// The fractional-rate claim, stated as arithmetic: at 1.024 samples per slot the fortieth
    /// slot starts a whole sample later than a constant stride of one would put it.
    #[test]
    fn boundaries_are_computed_per_slot_not_stepped() {
        let grid = SlotGrid::new(1.024, 240, 0.0);
        assert_eq!(grid.start(40), 40); // 40.96 → 40
        assert_eq!(grid.start(120), 122); // 122.88 → 122
        assert_eq!(grid.start(240), 245); // 245.76 → 245
    }

    /// Why a grid is built *per assumed phase*, as arithmetic rather than as prose: one pulse
    /// half a sample off the grid at ~1 sample per slot is read correctly by the matching
    /// phase table and attributed to the *next* slot by the phase-0 one. One table is not
    /// enough — this is `channels::adsb`'s field failure in eight lines.
    #[test]
    fn a_pulse_off_the_sample_grid_needs_the_matching_phase_table() {
        let (from, to): (f64, f64) = (3.0 * 1.024 + 0.5, 4.0 * 1.024 + 0.5);
        let window: Vec<f32> = (0..12)
            .map(|k| (to.min(k as f64 + 1.0) - from.max(k as f64)).max(0.0) as f32)
            .collect();
        let peak = |phase: f64| {
            let grid = SlotGrid::new(1.024, 8, phase);
            let energies: Vec<f32> = (0..8).map(|slot| grid.energy(&window, slot)).collect();
            (0..8)
                .max_by(|&a, &b| energies[a].total_cmp(&energies[b]))
                .unwrap()
        };
        assert_eq!(peak(0.5), 3, "the matching phase table must read slot 3");
        assert_eq!(
            peak(0.0),
            4,
            "the phase-0 table reads the pulse one slot late"
        );
    }

    /// Phase tables are what a receiver tries when the clock phase is unknown, so they must be
    /// distinct grids covering the sample evenly, all spanning the same window.
    #[test]
    fn phase_tables_cover_the_sample_evenly() {
        let grids = SlotGrid::phases(2.4, 16, 8);
        assert_eq!(grids.len(), 8);
        for (k, grid) in grids.iter().enumerate() {
            assert!((grid.phase() - k as f64 / 8.0).abs() < 1e-12);
            assert_eq!(grid.slots(), 16);
        }
    }

    /// The coherent sum keeps the phase the envelope sum throws away: a slot of constant
    /// samples integrates to their vector sum, and a slot whose samples cancel integrates to
    /// nothing while its envelope reads full scale.
    #[test]
    fn integrate_keeps_the_phase_energy_discards() {
        let grid = SlotGrid::new(4.0, 2, 0.0);
        let steady = vec![Complex::new(0.5f32, 0.0); 8];
        assert!((grid.integrate(&steady, 0) - Complex::new(2.0, 0.0)).norm() < 1e-6);
        let alternating: Vec<Complex<f32>> = (0..8)
            .map(|i| Complex::new(if i % 2 == 0 { 0.5 } else { -0.5 }, 0.0))
            .collect();
        assert!(grid.integrate(&alternating, 0).norm() < 1e-6);
        let magnitudes: Vec<f32> = alternating.iter().map(|s| s.norm()).collect();
        assert!((grid.energy(&magnitudes, 0) - 2.0).abs() < 1e-6);
    }

    /// A window that ends inside a burst must score the truncated slots low, never read past
    /// its end — the block-boundary case every streaming consumer meets.
    #[test]
    fn a_short_window_reads_zero_past_its_end() {
        let grid = SlotGrid::new(3.0, 4, 0.0);
        let window = [1.0f32; 5];
        assert_eq!(grid.energy(&window, 0), 3.0);
        assert_eq!(grid.energy(&window, 1), 2.0);
        assert_eq!(grid.energy(&window, 2), 0.0);
        assert_eq!(grid.energy(&window, 9), 0.0);
    }

    #[test]
    fn magnitudes_are_the_envelope() {
        let iq = [Complex::new(3.0f32, 4.0), Complex::new(0.0, -2.0)];
        let mut mag = Vec::new();
        magnitudes(&iq, &mut mag);
        assert_eq!(mag, vec![5.0, 2.0]);
    }
}
