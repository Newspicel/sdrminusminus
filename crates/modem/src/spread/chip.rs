use num_complex::Complex;

use crate::pulse::{self, Norm};

#[derive(Clone, Debug)]
pub struct ChipShaper {
    sps: usize,
    taps: Vec<f32>,
}

impl ChipShaper {
    #[must_use]
    pub fn root_raised_cosine(sps: usize, alpha: f64, span: usize) -> Self {
        assert!(
            sps >= 2,
            "a shaped chip needs at least two samples, got {sps}"
        );
        Self {
            sps,
            taps: pulse::root_raised_cosine(sps as f64, alpha, span, Norm::Energy),
        }
    }

    #[must_use]
    pub fn sps(&self) -> usize {
        self.sps
    }

    #[must_use]
    pub fn taps(&self) -> &[f32] {
        &self.taps
    }

    #[must_use]
    pub fn delay(&self) -> usize {
        (self.taps.len() - 1) / 2
    }

    #[must_use]
    pub fn rendered_len(&self, chips: usize) -> usize {
        chips * self.sps + self.taps.len() - 1
    }

    pub fn render(&self, chips: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let start = out.len();
        let total = self.rendered_len(chips.len());
        out.resize(start + total, Complex::new(0.0, 0.0));
        for (k, &chip) in chips.iter().enumerate() {
            let at = start + k * self.sps;
            for (j, &h) in self.taps.iter().enumerate() {
                let slot = &mut out[at + j];
                slot.re += chip.re * h;
                slot.im += chip.im * h;
            }
        }
    }

    pub fn matched(&self, wave: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let shift = 2 * self.delay();
        out.clear();
        out.resize(wave.len(), Complex::new(0.0, 0.0));
        for (n, slot) in out.iter_mut().enumerate() {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            let top = n + shift;
            for (j, &h) in self.taps.iter().enumerate() {
                let Some(&s) = top.checked_sub(j).and_then(|i| wave.get(i)) else {
                    continue;
                };
                re += f64::from(h) * f64::from(s.re);
                im += f64::from(h) * f64::from(s.im);
            }
            *slot = Complex::new(re as f32, im as f32);
        }
    }

    #[must_use]
    pub fn correlate(
        &self,
        filtered: &[Complex<f32>],
        known: &[Complex<f32>],
        origin: usize,
        group_chips: usize,
    ) -> f64 {
        let mut score = 0.0f64;
        for (g, group) in known.chunks_exact(group_chips).enumerate() {
            let first = g * group_chips;
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (c, &want) in group.iter().enumerate() {
                let Some(&y) = filtered.get(origin + (first + c) * self.sps) else {
                    continue;
                };
                re += f64::from(y.re) * f64::from(want.re) + f64::from(y.im) * f64::from(want.im);
                im += f64::from(y.im) * f64::from(want.re) - f64::from(y.re) * f64::from(want.im);
            }
            score += (re * re + im * im).sqrt();
        }
        score
    }

    pub fn block(
        &self,
        filtered: &[Complex<f32>],
        origin: usize,
        first_chip: usize,
        out: &mut [Complex<f32>],
    ) {
        for (c, slot) in out.iter_mut().enumerate() {
            let at = origin + (first_chip + c) * self.sps;
            *slot = filtered.get(at).copied().unwrap_or(Complex::new(0.0, 0.0));
        }
    }
}

#[must_use]
pub fn find_burst(
    shaper: &ChipShaper,
    filtered: &[Complex<f32>],
    known: &[Complex<f32>],
    group_chips: usize,
    search: usize,
) -> Option<usize> {
    if search == 0 || group_chips == 0 || known.len() < group_chips {
        return None;
    }
    let mut best = (0usize, f64::NEG_INFINITY);
    for origin in 0..search {
        let score = shaper.correlate(filtered, known, origin, group_chips);
        if score > best.1 {
            best = (origin, score);
        }
    }
    best.1.is_finite().then_some(best.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chips(values: &[f32]) -> Vec<Complex<f32>> {
        values.iter().map(|&v| Complex::new(v, 0.0)).collect()
    }

    #[test]
    fn a_rendered_chip_comes_back_at_its_own_grid_index() {
        let shaper = ChipShaper::root_raised_cosine(4, 0.35, 8);
        let sent = chips(&[1.0, -1.0, -1.0, 1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0]);
        let lead = 37;
        let mut wave = vec![Complex::new(0.0, 0.0); lead];
        shaper.render(&sent, &mut wave);
        wave.resize(wave.len() + 64, Complex::new(0.0, 0.0));

        let mut filtered = Vec::new();
        shaper.matched(&wave, &mut filtered);
        let mut got = vec![Complex::new(0.0, 0.0); sent.len()];
        shaper.block(&filtered, lead, 0, &mut got);
        for (k, (&g, &s)) in got.iter().zip(&sent).enumerate() {
            assert!((g - s).norm() < 2e-3, "chip {k}: got {g}, sent {s}");
        }
    }

    #[test]
    fn complex_chips_survive_the_cascade_independently() {
        let shaper = ChipShaper::root_raised_cosine(4, 0.5, 6);
        let sent = vec![
            Complex::new(1.0, 0.0),
            Complex::new(0.0, 1.0),
            Complex::new(-1.0, 0.0),
            Complex::new(0.0, -1.0),
            Complex::new(1.0, 0.0),
            Complex::new(0.0, 1.0),
            Complex::new(-1.0, 0.0),
            Complex::new(1.0, 0.0),
        ];
        let mut wave = vec![Complex::new(0.0, 0.0); 24];
        shaper.render(&sent, &mut wave);
        wave.resize(wave.len() + 48, Complex::new(0.0, 0.0));
        let mut filtered = Vec::new();
        shaper.matched(&wave, &mut filtered);
        let mut got = vec![Complex::new(0.0, 0.0); sent.len()];
        shaper.block(&filtered, 24, 0, &mut got);
        for (k, (&g, &s)) in got.iter().zip(&sent).enumerate() {
            assert!((g - s).norm() < 5e-3, "chip {k}: got {g}, sent {s}");
        }
    }

    #[test]
    fn the_shaping_preserves_the_chip_energy_it_was_handed() {
        let shaper = ChipShaper::root_raised_cosine(8, 0.35, 10);
        let sent = chips(&[1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, -1.0]);
        let mut wave = Vec::new();
        shaper.render(&sent, &mut wave);
        let energy: f64 = wave
            .iter()
            .map(|s| f64::from(s.re) * f64::from(s.re) + f64::from(s.im) * f64::from(s.im))
            .sum();
        assert!(
            (energy - sent.len() as f64).abs() < 0.02 * sent.len() as f64,
            "radiated {energy} for {} unit chips",
            sent.len()
        );
    }

    #[test]
    fn the_burst_search_finds_the_origin_the_burst_was_rendered_at() {
        let shaper = ChipShaper::root_raised_cosine(4, 0.35, 8);
        let known = chips(&[1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0]);
        for lead in [0usize, 1, 7, 16, 33, 50] {
            let mut wave = vec![Complex::new(0.0, 0.0); lead];
            shaper.render(&known, &mut wave);
            wave.resize(wave.len() + 128, Complex::new(0.0, 0.0));
            let mut filtered = Vec::new();
            shaper.matched(&wave, &mut filtered);
            assert_eq!(
                find_burst(&shaper, &filtered, &known, known.len(), 64),
                Some(lead),
                "lead {lead}"
            );
        }
    }

    #[test]
    fn an_impossible_search_finds_nothing() {
        let shaper = ChipShaper::root_raised_cosine(4, 0.35, 4);
        let filtered = vec![Complex::new(1.0, 0.0); 256];
        let known = chips(&[1.0, -1.0, 1.0, 1.0]);
        assert_eq!(find_burst(&shaper, &filtered, &known, 4, 0), None);
        assert_eq!(find_burst(&shaper, &filtered, &known, 0, 32), None);
        assert_eq!(find_burst(&shaper, &filtered, &known, 8, 32), None);
    }

    #[test]
    fn grouping_buys_the_search_its_carrier_tolerance() {
        let shaper = ChipShaper::root_raised_cosine(4, 0.35, 8);
        let known: Vec<Complex<f32>> = (0..64)
            .map(|k: usize| Complex::new(if k.is_multiple_of(3) { 1.0 } else { -1.0 }, 0.0))
            .collect();
        let lead = 40usize;
        let mut wave = vec![Complex::new(0.0, 0.0); lead];
        shaper.render(&known, &mut wave);
        wave.resize(wave.len() + 128, Complex::new(0.0, 0.0));
        let offset = 1.0 / (known.len() * shaper.sps()) as f64;
        for (index, s) in wave.iter_mut().enumerate() {
            let phase = std::f64::consts::TAU * offset * index as f64;
            let (sin, cos) = phase.sin_cos();
            *s = Complex::new(
                (f64::from(s.re) * cos - f64::from(s.im) * sin) as f32,
                (f64::from(s.re) * sin + f64::from(s.im) * cos) as f32,
            );
        }
        let mut filtered = Vec::new();
        shaper.matched(&wave, &mut filtered);
        assert_ne!(
            find_burst(&shaper, &filtered, &known, known.len(), 96),
            Some(lead),
            "one coherent group should have lost this burst"
        );
        assert_eq!(
            find_burst(&shaper, &filtered, &known, known.len() / 4, 96),
            Some(lead)
        );
    }

    #[test]
    fn a_block_past_the_end_reads_zeros_rather_than_panicking() {
        let shaper = ChipShaper::root_raised_cosine(4, 0.35, 4);
        let filtered = vec![Complex::new(1.0, 0.0); 16];
        let mut got = vec![Complex::new(9.0, 9.0); 8];
        shaper.block(&filtered, 12, 0, &mut got);
        assert_eq!(got[0], Complex::new(1.0, 0.0));
        assert!(got[1..].iter().all(|s| *s == Complex::new(0.0, 0.0)));
    }
}
