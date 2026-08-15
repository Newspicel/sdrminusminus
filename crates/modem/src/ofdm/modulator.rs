use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::params::{Domain, OfdmParams};

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

    #[must_use]
    pub fn points_per_frame(&self, symbols: usize) -> usize {
        symbols * self.params.data_subcarriers()
    }

    pub fn frame(&mut self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.preamble(out);
        self.modulate(points, out);
    }

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
        for n in 0..params.preamble().long_guard {
            let tail = wave[long + params.fft() - params.preamble().long_guard + n];
            assert!((wave[short + n] - tail).norm() < 1e-6, "long guard {n}");
        }
    }

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
        let mut complex = OfdmMod::new(OfdmParams::wifi_like());
        let mut wave = Vec::new();
        complex.frame(&points(48 * 4, 0xd07), &mut wave);
        let quadrature: f64 = wave.iter().map(|s| f64::from(s.im * s.im)).sum();
        let total: f64 = wave.iter().map(|s| f64::from(s.norm_sqr())).sum();
        assert!((quadrature / total - 0.5).abs() < 0.05);
        assert_eq!(params.domain(), Domain::RealHermitian);
    }

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
