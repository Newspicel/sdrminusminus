use num_complex::Complex;

#[derive(Clone, Copy, Debug)]
struct Slot {
    start: usize,
    at: usize,
    taps: usize,
}

#[derive(Clone, Debug)]
pub struct SlotGrid {
    table: Vec<Slot>,
    weights: Vec<f32>,
    span: usize,
    samples_per_slot: f64,
    phase: f64,
}

impl SlotGrid {
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

    #[must_use]
    pub fn phases(samples_per_slot: f64, slots: usize, tables: usize) -> Vec<Self> {
        assert!(tables > 0, "at least one phase table");
        (0..tables)
            .map(|k| Self::new(samples_per_slot, slots, k as f64 / tables as f64))
            .collect()
    }

    #[must_use]
    pub fn start(&self, slot: usize) -> usize {
        self.table.get(slot).map_or(0, |s| s.start)
    }

    #[must_use]
    pub fn span(&self) -> usize {
        self.span
    }

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

    #[must_use]
    pub fn weight_energy(&self, slot: usize) -> f32 {
        let Some(&Slot { at, taps, .. }) = self.table.get(slot) else {
            return 0.0;
        };
        self.weights[at..at + taps].iter().map(|&w| w * w).sum()
    }

    #[must_use]
    pub fn energy(&self, window: &[f32], slot: usize) -> f32 {
        self.weighted(window, slot, 0.0, |acc, w, x| acc + w * x)
    }

    #[must_use]
    pub fn envelope(&self, window: &[Complex<f32>], slot: usize) -> f32 {
        self.weighted(window, slot, Complex::new(0.0, 0.0), |acc, w, x| {
            acc + Complex::new(w * x.norm(), 0.0)
        })
        .re
    }

    #[must_use]
    pub fn integrate(&self, window: &[Complex<f32>], slot: usize) -> Complex<f32> {
        self.weighted(window, slot, Complex::new(0.0, 0.0), |acc, w, x| {
            acc + x * w
        })
    }

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

    #[test]
    fn boundaries_are_computed_per_slot_not_stepped() {
        let grid = SlotGrid::new(1.024, 240, 0.0);
        assert_eq!(grid.start(40), 40);
        assert_eq!(grid.start(120), 122);
        assert_eq!(grid.start(240), 245);
    }

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

    #[test]
    fn phase_tables_cover_the_sample_evenly() {
        let grids = SlotGrid::phases(2.4, 16, 8);
        assert_eq!(grids.len(), 8);
        for (k, grid) in grids.iter().enumerate() {
            assert!((grid.phase() - k as f64 / 8.0).abs() < 1e-12);
            assert_eq!(grid.slots(), 16);
        }
    }

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
