use num_complex::Complex;
use sdrmm_dsp::design_lowpass;

use super::transform::Dft;

pub const MAX_FFT: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UfmcParams {
    pub fft: usize,
    pub subbands: usize,
    pub per_subband: usize,
    pub filter_len: usize,
}

impl UfmcParams {
    #[must_use]
    pub fn reference() -> Self {
        Self {
            fft: 128,
            subbands: 4,
            per_subband: 12,
            filter_len: 33,
        }
    }

    #[must_use]
    pub fn points(&self) -> usize {
        self.subbands * self.per_subband
    }

    #[must_use]
    pub fn samples(&self) -> usize {
        self.fft + self.filter_len - 1
    }

    #[must_use]
    pub fn overhead_db(&self) -> f64 {
        10.0 * (self.samples() as f64 / self.fft as f64).log10()
    }

    #[must_use]
    pub fn bin(&self, index: usize) -> usize {
        let half = (self.subbands * self.per_subband / 2) as isize;
        let offset = index as isize - half;
        let signed = if offset < 0 { offset } else { offset + 1 };
        (signed.rem_euclid(self.fft as isize)) as usize
    }

    #[must_use]
    pub fn subband_of(&self, index: usize) -> usize {
        index / self.per_subband
    }

    #[must_use]
    pub fn subband_filter(&self, b: usize) -> Vec<Complex<f32>> {
        assert!(
            (1..=MAX_FFT).contains(&self.fft)
                && (3..=self.fft + 1).contains(&self.filter_len)
                && self.subbands.is_multiple_of(2),
            "a UFMC symbol runs a 1..={MAX_FFT} transform, 3..={} filter taps and an even subband \
             count; got {} taps over {} subbands of a {}-point transform",
            self.fft + 1,
            self.filter_len,
            self.subbands,
            self.fft
        );
        let half_width = self.per_subband as f64 / 2.0 / self.fft as f64;
        let transition = 2.75 / self.filter_len as f64;
        let prototype = design_lowpass(self.filter_len, (half_width + transition).min(0.4999));
        let first = self.subband_of_first(b);
        let centre = 0.5 * (self.signed_bin(first) + self.signed_bin(first + self.per_subband - 1))
            / self.fft as f64;
        prototype
            .iter()
            .enumerate()
            .map(|(n, &tap)| {
                Complex::from_polar(tap, (std::f64::consts::TAU * centre * n as f64) as f32)
            })
            .collect()
    }

    fn subband_of_first(&self, b: usize) -> usize {
        b * self.per_subband
    }

    fn signed_bin(&self, index: usize) -> f64 {
        let half = (self.subbands * self.per_subband / 2) as isize;
        let offset = index as isize - half;
        (if offset < 0 { offset } else { offset + 1 }) as f64
    }
}

#[derive(Clone)]
pub struct UfmcMod {
    params: UfmcParams,
    dft: Dft,
    filters: Vec<Vec<Complex<f32>>>,
    grid: Vec<Complex<f32>>,
    symbol: Vec<Complex<f32>>,
}

impl UfmcMod {
    #[must_use]
    pub fn new(params: UfmcParams) -> Self {
        Self {
            dft: Dft::new(params.fft),
            filters: (0..params.subbands)
                .map(|b| params.subband_filter(b))
                .collect(),
            grid: vec![Complex::new(0.0, 0.0); params.fft],
            symbol: vec![Complex::new(0.0, 0.0); params.samples()],
            params,
        }
    }

    #[must_use]
    pub fn params(&self) -> &UfmcParams {
        &self.params
    }

    pub fn modulate(&mut self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let per_symbol = self.params.points();
        for chunk in points.chunks_exact(per_symbol) {
            self.symbol.fill(Complex::new(0.0, 0.0));
            for b in 0..self.params.subbands {
                self.grid.fill(Complex::new(0.0, 0.0));
                let first = b * self.params.per_subband;
                for k in 0..self.params.per_subband {
                    self.grid[self.params.bin(first + k)] = chunk[first + k];
                }
                self.dft.inverse(&mut self.grid);
                for (n, &x) in self.grid.iter().enumerate() {
                    for (l, &tap) in self.filters[b].iter().enumerate() {
                        self.symbol[n + l] += x * tap;
                    }
                }
            }
            out.extend_from_slice(&self.symbol);
        }
    }
}

#[derive(Clone)]
pub struct UfmcDemod {
    params: UfmcParams,
    dft: Dft,
    equaliser: Vec<Complex<f32>>,
    amplification: Vec<f32>,
    padded: Vec<Complex<f32>>,
}

impl UfmcDemod {
    #[must_use]
    pub fn new(params: UfmcParams) -> Self {
        let filters: Vec<Vec<Complex<f32>>> = (0..params.subbands)
            .map(|b| params.subband_filter(b))
            .collect();
        let equaliser: Vec<Complex<f32>> = (0..params.points())
            .map(|index| {
                let bin = params.bin(index) as f64;
                let response: Complex<f64> = filters[params.subband_of(index)]
                    .iter()
                    .enumerate()
                    .map(|(n, &tap)| {
                        Complex::new(f64::from(tap.re), f64::from(tap.im))
                            * Complex::from_polar(
                                1.0,
                                -std::f64::consts::TAU * bin * n as f64 / params.fft as f64,
                            )
                    })
                    .sum();
                let inverse = response.inv();
                Complex::new(inverse.re as f32, inverse.im as f32)
            })
            .collect();
        Self {
            dft: Dft::new(2 * params.fft),
            amplification: equaliser.iter().map(Complex::norm_sqr).collect(),
            equaliser,
            padded: vec![Complex::new(0.0, 0.0); 2 * params.fft],
            params,
        }
    }

    #[must_use]
    pub fn params(&self) -> &UfmcParams {
        &self.params
    }

    #[must_use]
    pub fn amplification(&self) -> &[f32] {
        &self.amplification
    }

    pub fn demodulate(&mut self, x: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let samples = self.params.samples();
        for chunk in x.chunks_exact(samples) {
            self.padded.fill(Complex::new(0.0, 0.0));
            self.padded[..samples].copy_from_slice(chunk);
            self.dft.forward(&mut self.padded);
            for (index, &gain) in self.equaliser.iter().enumerate() {
                let bin = 2 * self.params.bin(index);
                out.push(self.padded[bin] * gain * std::f32::consts::SQRT_2);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(count: usize) -> Vec<Complex<f32>> {
        let mut state = 0x0_fc5eu32;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let i = if state & 1 == 0 { 1.0 } else { -1.0 };
                let q = if state & 2 == 0 { 1.0 } else { -1.0 };
                Complex::new(i, q) / 2f32.sqrt()
            })
            .collect()
    }

    #[test]
    fn the_subcarrier_map_is_symmetric_and_skips_dc() {
        let params = UfmcParams::reference();
        let bins: Vec<usize> = (0..params.points()).map(|k| params.bin(k)).collect();
        assert_eq!(bins.len(), 48);
        assert!(!bins.contains(&0), "DC is allocated");
        let mut sorted = bins.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), bins.len(), "a bin is allocated twice");
        assert_eq!(bins[0], params.fft - 24);
        assert_eq!(bins[47], 24);
    }

    #[test]
    fn the_receiver_round_trips_exactly() {
        let params = UfmcParams::reference();
        let sent = points(params.points() * 4);
        let mut wave = Vec::new();
        UfmcMod::new(params).modulate(&sent, &mut wave);
        assert_eq!(wave.len(), 4 * params.samples());
        let mut got = Vec::new();
        UfmcDemod::new(params).demodulate(&wave, &mut got);
        assert_eq!(got.len(), sent.len());
        for (k, (a, b)) in got.iter().zip(&sent).enumerate() {
            assert!((a - b).norm() < 1e-3, "point {k}: {a} vs {b}");
        }
    }

    #[test]
    fn the_per_bin_division_costs_almost_nothing() {
        let demod = UfmcDemod::new(UfmcParams::reference());
        let worst = demod
            .amplification()
            .iter()
            .fold(0.0f32, |acc, &v| acc.max(v));
        let worst_db = 10.0 * f64::from(worst).log10();
        assert!(worst_db < 1.5, "worst noise amplification {worst_db} dB");
    }

    #[test]
    fn the_subband_filter_buys_out_of_band_suppression() {
        let params = UfmcParams::reference();
        let sent = points(params.points() * 8);
        let mut wave = Vec::new();
        UfmcMod::new(params).modulate(&sent, &mut wave);

        let level = |x: &[Complex<f32>], bin: f64| {
            let n = x.len() as f64;
            x.iter()
                .enumerate()
                .map(|(i, &v)| {
                    Complex::new(f64::from(v.re), f64::from(v.im))
                        * Complex::from_polar(
                            1.0,
                            -std::f64::consts::TAU * bin * i as f64 / params.fft as f64,
                        )
                })
                .sum::<Complex<f64>>()
                .norm_sqr()
                / n
        };
        let in_band = level(&wave, 12.0);
        let out_of_band = level(&wave, 40.0);
        let suppression = 10.0 * (in_band / out_of_band).log10();
        assert!(
            suppression > 25.0,
            "out-of-band suppression {suppression} dB"
        );
    }

    #[test]
    fn the_overhead_is_the_filter_tail() {
        let params = UfmcParams::reference();
        assert_eq!(params.samples(), 160);
        let want = 10.0 * (160.0f64 / 128.0).log10();
        assert!((params.overhead_db() - want).abs() < 1e-12);
        assert!((params.overhead_db() - 0.9691).abs() < 1e-4);
    }

    #[test]
    #[should_panic(expected = "a UFMC symbol runs")]
    fn a_prototype_the_receiver_cannot_hold_is_rejected() {
        let mut params = UfmcParams::reference();
        params.filter_len = params.fft + 2;
        let _ = UfmcMod::new(params);
    }

    #[test]
    #[should_panic(expected = "even subband count")]
    fn an_odd_subband_count_is_rejected() {
        let mut params = UfmcParams::reference();
        params.subbands = 3;
        let _ = UfmcMod::new(params);
    }
}
