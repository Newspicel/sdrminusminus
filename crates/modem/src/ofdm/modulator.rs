//! The OFDM transmitter: constellation points onto subcarriers, an inverse transform, a cyclic
//! prefix, and the preamble the receiver finds the burst with.
//!
//! Two conventions are locked here because everything downstream reads them.
//!
//! **The transform is unitary.** Both directions carry `1/√N`, so Parseval holds sample for
//! symbol: a symbol whose 52 occupied bins each carry unit energy radiates exactly 52 units over
//! its 64 transform samples, and the per-subcarrier Eb/N0 a curve is plotted against is the
//! *same quantity* as the time-domain Eb/N0 the sweep runner sets. Without it every OFDM curve
//! would sit an arbitrary N-dependent distance from its closed form.
//!
//! **The prefix is cyclic, not a pad.** The last `cp` samples of the symbol are prepended, which
//! is what turns the channel's linear convolution into a circular one over the transform window
//! — and therefore what makes a one-tap equaliser correct at all. The engine's tolerance to
//! delay spread is exactly this length, and the limits table measures it as such.
//!
//! Rendering is cold path (a test-signal generator, `tx.rs`'s source): it allocates its planner
//! and buffers once at construction and nothing per frame, but the §4.2 zero-allocation gate
//! binds the demodulator.

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::params::{Domain, OfdmParams};

/// One OFDM transmitter over one parameter set.
///
/// `Clone` is part of the contract, not an afterthought: a measurement chain builds one
/// transmitter and clones it per trial, so every trial starts from the same designed state
/// without re-planning a transform (the harness's rule that a trial reproduces from its own seed
/// alone, at the cost of one buffer copy instead of one planner).
#[derive(Clone)]
pub struct OfdmMod {
    params: OfdmParams,
    ifft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    grid: Vec<Complex<f32>>,
    scale: f32,
}

impl OfdmMod {
    #[must_use]
    pub fn new(params: OfdmParams) -> Self {
        let fft = params.fft();
        let ifft = FftPlanner::<f32>::new().plan_fft_inverse(fft);
        let scratch = vec![Complex::new(0.0, 0.0); ifft.get_inplace_scratch_len()];
        Self {
            params,
            ifft,
            scratch,
            grid: vec![Complex::new(0.0, 0.0); fft],
            scale: (fft as f64).sqrt().recip() as f32,
        }
    }

    #[must_use]
    pub fn params(&self) -> &OfdmParams {
        &self.params
    }

    /// Points one frame of `symbols` data symbols consumes.
    #[must_use]
    pub fn points_per_frame(&self, symbols: usize) -> usize {
        symbols * self.params.data_subcarriers()
    }

    /// A whole frame: preamble, then the data symbols carrying `points`.
    ///
    /// # Panics
    /// As [`Self::modulate`].
    pub fn frame(&mut self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.preamble(out);
        self.modulate(points, out);
    }

    /// The preamble: the short training's repeated sub-symbol period, then the long training's
    /// guard and whole-symbol repeats.
    pub fn preamble(&mut self, out: &mut Vec<Complex<f32>>) {
        let fft = self.params.fft();
        let preamble = self.params.preamble();

        self.clear();
        for index in 0..self.params.short_bins().len() {
            let bin = self.params.short_bins()[index].bin;
            let value = self.params.short_training(index);
            self.place(bin, value);
        }
        self.transform();
        // Read modulo the transform length rather than modulo the period: the periodicity is a
        // *consequence* of the stride (asserted in `the_short_training_repeats_its_period`), and
        // a renderer that assumed it would hide a map whose stride does not actually produce it.
        let short_samples = preamble.short_repeats * preamble.short_period(fft);
        out.reserve(short_samples + preamble.long_guard + preamble.long_repeats * fft);
        for n in 0..short_samples {
            out.push(self.grid[n % fft]);
        }

        self.clear();
        for index in 0..self.params.map().occupied().len() {
            let bin = self.params.map().occupied()[index].bin;
            let value = self.params.long_training(index);
            self.place(bin, value);
        }
        self.transform();
        out.extend_from_slice(&self.grid[fft - preamble.long_guard..]);
        for _ in 0..preamble.long_repeats {
            out.extend_from_slice(&self.grid);
        }
    }

    /// Data symbols alone, symbol 0 first — the pilot polarity indexes from the frame's start,
    /// so this is one frame's payload and not a continuation.
    ///
    /// # Panics
    /// If `points` is not a whole number of symbols' worth.
    pub fn modulate(&mut self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let data = self.params.data_subcarriers();
        assert_eq!(
            points.len() % data,
            0,
            "an OFDM symbol carries {data} points; {} is not a whole number of symbols",
            points.len()
        );
        let fft = self.params.fft();
        let cp = self.params.cp();
        out.reserve(points.len() / data * (fft + cp));
        for (symbol, chunk) in points.chunks_exact(data).enumerate() {
            self.clear();
            for (index, &point) in chunk.iter().enumerate() {
                let bin = self.params.map().data()[index].bin;
                self.place(bin, point);
            }
            for index in 0..self.params.map().pilots().len() {
                let bin = self.params.map().pilots()[index].bin;
                let value = self.params.pilot_pattern().value(index, symbol);
                self.place(bin, value);
            }
            self.transform();
            out.extend_from_slice(&self.grid[fft - cp..]);
            out.extend_from_slice(&self.grid);
        }
    }

    fn clear(&mut self) {
        self.grid.fill(Complex::new(0.0, 0.0));
    }

    /// Writes one subcarrier, mirroring it onto its conjugate partner under the Hermitian flag —
    /// the one place in the engine the DMT domain exists at all.
    fn place(&mut self, bin: usize, value: Complex<f32>) {
        self.grid[bin] = value;
        if self.params.domain() == Domain::RealHermitian {
            let mirror = (self.params.fft() - bin) % self.params.fft();
            self.grid[mirror] = value.conj();
        }
    }

    fn transform(&mut self) {
        self.ifft
            .process_with_scratch(&mut self.grid, &mut self.scratch);
        for sample in &mut self.grid {
            *sample *= self.scale;
        }
    }
}

impl std::fmt::Debug for OfdmMod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfdmMod")
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

/// The long training symbol in the time domain, one transform length — what a receiver
/// correlates against to place a frame, built here so transmitter and receiver can never train
/// against different sequences.
#[must_use]
pub fn long_training_time(params: &OfdmParams) -> Vec<Complex<f32>> {
    let mut modulator = OfdmMod::new(params.clone());
    let mut preamble = Vec::new();
    modulator.preamble(&mut preamble);
    let start = params.preamble().short_repeats * params.preamble().short_period(params.fft())
        + params.preamble().long_guard;
    preamble[start..start + params.fft()].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ofdm::params::Domain;

    fn points(n: usize, seed: u32) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let re = if state & 1 == 0 { 1.0 } else { -1.0 };
                let im = if state & 2 == 0 { 1.0 } else { -1.0 };
                Complex::new(re, im) * std::f32::consts::FRAC_1_SQRT_2
            })
            .collect()
    }

    #[test]
    fn a_frame_is_exactly_as_long_as_the_geometry_says() {
        let params = OfdmParams::wifi_like();
        let mut m = OfdmMod::new(params.clone());
        let mut wave = Vec::new();
        m.frame(&points(48 * 4, 0x51), &mut wave);
        assert_eq!(wave.len(), params.frame_samples(4));
        assert_eq!(wave.len(), 320 + 4 * 80);
    }

    /// The prefix is a copy of the symbol's tail — the property that makes a one-tap equaliser
    /// legitimate, and the one a "pad with zeros" mistake would silently break.
    #[test]
    fn the_cyclic_prefix_repeats_the_symbols_tail() {
        let params = OfdmParams::wifi_like();
        let mut m = OfdmMod::new(params.clone());
        let mut wave = Vec::new();
        m.modulate(&points(48, 0x9c), &mut wave);
        let (cp, fft) = (params.cp(), params.fft());
        for n in 0..cp {
            let tail = wave[cp + fft - cp + n];
            assert!((wave[n] - tail).norm() < 1e-6, "prefix sample {n}");
        }
    }

    /// The short half repeats at the period its stride implies; the long half repeats whole
    /// symbols. Both are what the receiver's two autocorrelations assume.
    #[test]
    fn the_short_training_repeats_its_period_and_the_long_its_symbol() {
        let params = OfdmParams::wifi_like();
        let mut m = OfdmMod::new(params.clone());
        let mut wave = Vec::new();
        m.preamble(&mut wave);
        let period = params.preamble().short_period(params.fft());
        let short = params.preamble().short_repeats * period;
        for n in 0..short - period {
            assert!(
                (wave[n] - wave[n + period]).norm() < 1e-6,
                "short training sample {n}"
            );
        }
        let long = short + params.preamble().long_guard;
        for n in 0..params.fft() {
            assert!(
                (wave[long + n] - wave[long + params.fft() + n]).norm() < 1e-6,
                "long training sample {n}"
            );
        }
        // …and the guard is that symbol's own tail.
        for n in 0..params.preamble().long_guard {
            let tail = wave[long + params.fft() - params.preamble().long_guard + n];
            assert!((wave[short + n] - tail).norm() < 1e-6, "long guard {n}");
        }
    }

    /// Parseval, which is the transform's normalisation contract: the frame's measured energy is
    /// the geometry's closed form, so `framing_overhead_db` describes the waveform rather than an
    /// intention.
    ///
    /// Averaged over payloads, not per payload, and the difference is the closed form's one piece
    /// of honesty: a cyclic prefix copies the symbol's *last* `cp` samples, whose energy is
    /// `cp/fft` of the symbol's only in expectation, and the long training's guard copies a fixed
    /// 32 samples of a fixed symbol, which lands 0.5 % from half of it and stays there. The
    /// residual is 0.02 dB — an order inside the tolerance any oracle gate here is stated at, and
    /// the sweep runner charges each trial its own *measured* energy in any case.
    #[test]
    fn the_radiated_energy_is_the_closed_form_on_average() {
        for params in [OfdmParams::wifi_like(), OfdmParams::dmt_like()] {
            let symbols = 8;
            let mut m = OfdmMod::new(params.clone());
            let mut total = 0.0f64;
            let trials = 64u32;
            for trial in 0..trials {
                let mut wave = Vec::new();
                m.frame(
                    &points(params.data_subcarriers() * symbols, 0x4e2 + trial),
                    &mut wave,
                );
                total += wave.iter().map(|s| f64::from(s.norm_sqr())).sum::<f64>();
            }
            let measured = total / f64::from(trials);
            let want = params.frame_energy(symbols);
            assert!(
                (measured / want - 1.0).abs() < 1e-2,
                "{:?}: radiated {measured} on average, closed form {want}",
                params.domain()
            );
        }
    }

    /// The DMT flag's transmitter half: a Hermitian spectrum renders a real waveform. Measured
    /// as the quadrature rail's residual energy, which is rounding and nothing else.
    #[test]
    fn the_hermitian_flag_renders_a_real_waveform() {
        let params = OfdmParams::dmt_like();
        let mut m = OfdmMod::new(params.clone());
        let mut wave = Vec::new();
        m.frame(&points(params.data_subcarriers() * 4, 0xd07), &mut wave);
        let quadrature: f64 = wave.iter().map(|s| f64::from(s.im * s.im)).sum();
        let total: f64 = wave.iter().map(|s| f64::from(s.norm_sqr())).sum();
        assert!(
            quadrature / total < 1e-10,
            "quadrature energy fraction {}",
            quadrature / total
        );
        // The complex configuration is not accidentally real — otherwise the test above would
        // pass on a transmitter that ignored the flag.
        let mut complex = OfdmMod::new(OfdmParams::wifi_like());
        let mut wave = Vec::new();
        complex.frame(&points(48 * 4, 0xd07), &mut wave);
        let quadrature: f64 = wave.iter().map(|s| f64::from(s.im * s.im)).sum();
        let total: f64 = wave.iter().map(|s| f64::from(s.norm_sqr())).sum();
        assert!((quadrature / total - 0.5).abs() < 0.05);
        assert_eq!(params.domain(), Domain::RealHermitian);
    }

    /// The training symbol the receiver correlates against is the one the transmitter sent.
    #[test]
    fn the_long_training_symbol_is_the_one_in_the_preamble() {
        let params = OfdmParams::wifi_like();
        let mut m = OfdmMod::new(params.clone());
        let mut wave = Vec::new();
        m.preamble(&mut wave);
        let ltf = long_training_time(&params);
        let start = 160 + 32;
        for n in 0..params.fft() {
            assert!((ltf[n] - wave[start + n]).norm() < 1e-6, "sample {n}");
        }
    }

    #[test]
    #[should_panic(expected = "not a whole number of symbols")]
    fn a_partial_symbols_worth_of_points_is_rejected() {
        let mut m = OfdmMod::new(OfdmParams::wifi_like());
        m.modulate(&points(47, 1), &mut Vec::new());
    }
}
