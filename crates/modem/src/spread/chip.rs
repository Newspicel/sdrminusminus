//! The chip-rate substrate the two direct-sequence entries share: one pulse shape, one matched
//! filter, one statement of where a chip sits on the sample grid.
//!
//! **Why this is a module and not two copies.** [`dsss`](super::dsss) and [`cck`](super::cck)
//! differ entirely in what a block of chips *means* — a PN period times one constellation point,
//! or one of a codebook's complex words — and not at all in how chips reach the air or come back.
//! 802.11b makes the same split: the Barker rate and the CCK rates are the same 11 Mchip/s
//! waveform carrying different block codes, and a receiver that has acquired one has acquired the
//! other. So the shaping, the matched filter, the grid convention and the burst search live here
//! once, and each engine contributes only its own block detector.
//!
//! **The grid convention, stated once because everything downstream indexes off it.** A burst's
//! *origin* is the sample index its first chip's impulse was placed at, before any filtering.
//! [`ChipShaper::render`] puts chip `k` of a burst rendered into an empty buffer at pre-filter
//! index `k·sps`; [`ChipShaper::matched`] removes the cascade's whole group delay, so after it
//! chip `k` of a burst at origin `o` is read at sample `o + k·sps` — the same index in both
//! directions. Nothing else in `spread/` has to know the filter's length.
//!
//! **Chips are complex.** DSSS spreads a real ±1 sequence and CCK's codewords are complex, so the
//! shared path carries the wider type; the narrowing is the caller's, at the point where it
//! knows.

use num_complex::Complex;

use crate::pulse::{self, Norm};

/// Chip pulse shaping and its matched filter — the same taps in both directions, designed once
/// (§1.2: a modulator and its demodulator can never drift apart if they read one pulse).
#[derive(Clone, Debug)]
pub struct ChipShaper {
    sps: usize,
    taps: Vec<f32>,
}

impl ChipShaper {
    /// A root-raised-cosine chip pulse at `sps` samples per chip, roll-off `alpha`, truncated to
    /// `span` chips each side. Unit energy (crate-root convention), so the transmit/receive
    /// cascade peaks at exactly 1 and a chip's energy is its own squared magnitude.
    ///
    /// # Panics
    /// If `sps` is less than two — a chip stream sampled at or below its own rate has no pulse
    /// left to shape and no matched filter to be matched to.
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

    /// Group delay of one filter, in samples. The cascade's is twice this, and it is what
    /// [`Self::matched`] removes.
    #[must_use]
    pub fn delay(&self) -> usize {
        (self.taps.len() - 1) / 2
    }

    /// Samples one burst of `chips` chips occupies once rendered, tail included.
    #[must_use]
    pub fn rendered_len(&self, chips: usize) -> usize {
        chips * self.sps + self.taps.len() - 1
    }

    /// Renders chips at `sps` samples per chip, appended to `out`. Chip `k` lands at pre-filter
    /// index `out.len() + k·sps` as measured when this was called — the module's grid convention.
    ///
    /// Cold path: this is a signal generator (`tx.rs`'s source and the harness's transmitter), so
    /// it reserves once and writes; the §4.2 zero-allocation gate binds the receive side.
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

    /// The matched filter, with the whole transmit/receive group delay removed: after it, chip
    /// `k` of a burst at origin `o` is `out[o + k·sps]`.
    ///
    /// `out` is resized rather than appended to, and the accumulation is `f64` — a chip-rate
    /// correlation sums hundreds of these and an `f32` accumulator would show it. Steady-state
    /// allocation-free once `out` holds its capacity, which is what the receive engines rely on.
    pub fn matched(&self, wave: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let shift = 2 * self.delay();
        out.clear();
        out.resize(wave.len(), Complex::new(0.0, 0.0));
        // Output index n reads wave[n + shift − j] for tap j, i.e. the cascade output advanced by
        // the delay the transmit filter already spent.
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

    /// The correlation of a known chip run against the stream at one origin, in groups of
    /// `group_chips`: coherent inside a group, magnitudes summed across groups.
    ///
    /// **The group length is the acquisition's carrier tolerance, stated as a parameter rather
    /// than discovered.** A coherent correlation over `G` chips loses its peak once a carrier
    /// offset turns the accumulator by half a cycle across the group, so the search survives
    /// offsets up to roughly `1/(2·G·sps)` cycles per sample; summing group magnitudes buys that
    /// back at the cost of the usual noncoherent combining loss. Every group is the same length
    /// (a partial trailing group is dropped), so the metric is comparable across origins by
    /// construction.
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

    /// One block's chips off a matched-filtered stream: `filtered[origin + (block·len + c)·sps]`
    /// for `c` in `0..out.len()`. Reads zero past the end rather than panicking, because a burst
    /// search probes origins whose blocks run off the buffer and a bounds check there is the
    /// honest answer, not a crash.
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

/// The burst search (§3.4, chip domain): the origin in `0..search` at which the known chip run
/// correlates best. `None` when there is nothing to search — no origins, no known chips, or a
/// group longer than the run — because an origin invented from an impossible search would be
/// indistinguishable from a found one.
///
/// The metric is [`ChipShaper::correlate`]'s, so what the search tolerates in carrier offset is
/// `group_chips`' statement and nothing else. Resolution is one *sample*, not one chip: a burst
/// whose chip clock sits between two samples is found at the nearer of them, and the residual is
/// what the §4.3 timing row measures.
///
/// Allocation-free, `O(search · known)`.
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

    /// The grid convention, which is the module's entire contract: render a burst after a lead,
    /// matched-filter it, and every chip must come back at `lead + k·sps` at its own amplitude.
    /// A Nyquist cascade has no inter-chip interference at those instants, so the recovered
    /// values are the transmitted ones and not merely close to them.
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

    /// The complex path is not a real path with a spare rail: an in-phase and a quadrature chip
    /// must survive independently, which is what CCK's codewords need.
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

    /// Unit energy in, unit energy out: the pulse carries the crate-root convention, so a burst
    /// of unit chips radiates its own chip count and nothing has to renormalise downstream.
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
        // Truncation puts a fraction of a percent of the pulse outside the span; the rest is the
        // chip count exactly.
        assert!(
            (energy - sent.len() as f64).abs() < 0.02 * sent.len() as f64,
            "radiated {energy} for {} unit chips",
            sent.len()
        );
    }

    /// The search finds the origin a burst was actually rendered at, at every lead — including a
    /// lead that is not a whole number of chips, which is the case a chip-quantised search would
    /// silently get wrong.
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

    /// An impossible search says so rather than answering zero — the difference between "not
    /// found" and "found at the start" is the whole point of the `Option`.
    #[test]
    fn an_impossible_search_finds_nothing() {
        let shaper = ChipShaper::root_raised_cosine(4, 0.35, 4);
        let filtered = vec![Complex::new(1.0, 0.0); 256];
        let known = chips(&[1.0, -1.0, 1.0, 1.0]);
        assert_eq!(find_burst(&shaper, &filtered, &known, 4, 0), None);
        assert_eq!(find_burst(&shaper, &filtered, &known, 0, 32), None);
        assert_eq!(find_burst(&shaper, &filtered, &known, 8, 32), None);
    }

    /// Grouping is what the search's carrier tolerance is made of, so the two regimes are
    /// measured rather than described: one coherent group over the whole word loses the burst at
    /// an offset that four groups still find.
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
        // One whole cycle across the 256-sample word: a single coherent correlation integrates
        // the carrier through a full turn and nulls, while four groups each see a quarter of one.
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

    /// A block reaching past the buffer reads zeros — the case a burst search hits at every
    /// origin near the end of its window.
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
