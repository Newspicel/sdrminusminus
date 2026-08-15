use std::f64::consts::TAU;

use num_complex::Complex;

const PHYDYAS_K4: [f64; 3] = [0.971_960, std::f64::consts::FRAC_1_SQRT_2, 0.235_147];

pub const MAX_SUBCARRIERS: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FbmcParams {
    pub subcarriers: usize,
    pub allocated: usize,
}

impl FbmcParams {
    pub const OVERLAP: usize = 4;

    #[must_use]
    pub fn reference() -> Self {
        Self {
            subcarriers: 64,
            allocated: 48,
        }
    }

    #[must_use]
    pub fn prototype_len(&self) -> usize {
        Self::OVERLAP * self.subcarriers
    }

    #[must_use]
    pub fn slot_stride(&self) -> usize {
        self.subcarriers / 2
    }

    #[must_use]
    pub fn frame_samples(&self, points: usize) -> usize {
        let slots = 2 * points / self.allocated;
        (slots.max(1) - 1) * self.slot_stride() + self.prototype_len()
    }

    #[must_use]
    pub fn prototype(&self) -> Vec<f64> {
        assert!(
            (2..=MAX_SUBCARRIERS).contains(&self.subcarriers) && self.subcarriers.is_multiple_of(2),
            "an FBMC bank runs an even 2..={MAX_SUBCARRIERS} subcarriers"
        );
        assert!(
            (1..=self.subcarriers).contains(&self.allocated),
            "an allocation runs 1..={} subcarriers of the bank, got {}",
            self.subcarriers,
            self.allocated
        );
        let len = self.prototype_len();
        let mut p: Vec<f64> = (0..len)
            .map(|n| {
                let mut acc = 1.0;
                for (k, &h) in PHYDYAS_K4.iter().enumerate() {
                    let order = (k + 1) as f64;
                    let sign = if (k + 1) % 2 == 0 { 1.0 } else { -1.0 };
                    acc += 2.0 * sign * h * (TAU * order * n as f64 / len as f64).cos();
                }
                acc
            })
            .collect();
        let scale = p.iter().map(|v| v * v).sum::<f64>().sqrt().recip();
        for v in &mut p {
            *v *= scale;
        }
        p
    }

    #[must_use]
    pub fn bin(&self, index: usize) -> usize {
        let half = (self.allocated / 2) as isize;
        let offset = index as isize - half;
        let signed = if offset < 0 { offset } else { offset + 1 };
        signed.rem_euclid(self.subcarriers as isize) as usize
    }

    #[must_use]
    fn kernels(&self) -> Vec<Vec<Complex<f32>>> {
        let p = self.prototype();
        let len = self.prototype_len();
        let delay = (len / 2) as f64;
        (0..self.allocated)
            .map(|index| {
                let m = self.bin(index) as f64;
                (0..len)
                    .map(|u| {
                        let phase = TAU * m * (u as f64 - delay) / self.subcarriers as f64;
                        let z = Complex::from_polar(p[u], phase);
                        Complex::new(z.re as f32, z.im as f32)
                    })
                    .collect()
            })
            .collect()
    }

    fn oqam_phase(&self, index: usize, slot: usize) -> Complex<f32> {
        let m = self.bin(index);
        let quarter = (m + slot) % 4;
        let base = [
            Complex::new(1.0f32, 0.0),
            Complex::new(0.0, 1.0),
            Complex::new(-1.0, 0.0),
            Complex::new(0.0, -1.0),
        ][quarter];
        if (m * slot).is_multiple_of(2) {
            base
        } else {
            -base
        }
    }
}

#[derive(Clone)]
pub struct FbmcMod {
    params: FbmcParams,
    kernels: Vec<Vec<Complex<f32>>>,
}

impl FbmcMod {
    #[must_use]
    pub fn new(params: FbmcParams) -> Self {
        Self {
            kernels: params.kernels(),
            params,
        }
    }

    #[must_use]
    pub fn params(&self) -> &FbmcParams {
        &self.params
    }

    pub fn modulate(&mut self, points: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let allocated = self.params.allocated;
        let stride = self.params.slot_stride();
        let start = out.len();
        out.resize(
            start + self.params.frame_samples(points.len()),
            Complex::new(0.0, 0.0),
        );
        for (symbol, chunk) in points.chunks_exact(allocated).enumerate() {
            for (index, &point) in chunk.iter().enumerate() {
                let kernel = &self.kernels[index];
                for (half, value) in [point.re, point.im].into_iter().enumerate() {
                    let slot = 2 * symbol + half;
                    let phase = self.params.oqam_phase(index, slot) * value;
                    let base = start + slot * stride;
                    for (u, &tap) in kernel.iter().enumerate() {
                        out[base + u] += phase * tap;
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct FbmcDemod {
    params: FbmcParams,
    kernels: Vec<Vec<Complex<f32>>>,
}

impl FbmcDemod {
    #[must_use]
    pub fn new(params: FbmcParams) -> Self {
        Self {
            kernels: params.kernels(),
            params,
        }
    }

    #[must_use]
    pub fn params(&self) -> &FbmcParams {
        &self.params
    }

    pub fn demodulate(&mut self, x: &[Complex<f32>], symbols: usize, out: &mut Vec<Complex<f32>>) {
        let (allocated, stride) = (self.params.allocated, self.params.slot_stride());
        let len = self.params.prototype_len();
        for symbol in 0..symbols {
            for index in 0..allocated {
                let kernel = &self.kernels[index];
                let mut halves = [0.0f32; 2];
                for (half, slot_value) in halves.iter_mut().enumerate() {
                    let slot = 2 * symbol + half;
                    let base = slot * stride;
                    if base + len > x.len() {
                        continue;
                    }
                    let mut acc = Complex::new(0.0f32, 0.0);
                    for (u, &tap) in kernel.iter().enumerate() {
                        acc += x[base + u] * tap.conj();
                    }
                    *slot_value = (acc * self.params.oqam_phase(index, slot).conj()).re;
                }
                out.push(Complex::new(halves[0], halves[1]));
            }
        }
    }
}

#[must_use]
pub fn prototype_response_db(params: &FbmcParams, offset: f64) -> f64 {
    let p = params.prototype();
    let m = params.subcarriers as f64;
    let at = |f: f64| {
        p.iter()
            .enumerate()
            .map(|(n, &tap)| Complex::from_polar(tap, -TAU * f * n as f64 / m))
            .sum::<Complex<f64>>()
            .norm_sqr()
    };
    10.0 * (at(offset) / at(0.0)).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(count: usize) -> Vec<Complex<f32>> {
        let mut state = 0x0_fb3cu32;
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
    fn the_phydyas_prototype_is_unit_energy_and_symmetric() {
        let params = FbmcParams::reference();
        let p = params.prototype();
        assert_eq!(p.len(), 256);
        let energy: f64 = p.iter().map(|v| v * v).sum();
        assert!((energy - 1.0).abs() < 1e-12, "energy {energy}");
        for n in 1..p.len() / 2 {
            assert!(
                (p[n] - p[p.len() - n]).abs() < 1e-12,
                "asymmetric at tap {n}"
            );
        }
        let one_away = prototype_response_db(&params, 1.0);
        let two_away = prototype_response_db(&params, 2.0);
        assert!(one_away < -20.0, "one spacing away: {one_away} dB");
        assert!(two_away < -35.0, "two spacings away: {two_away} dB");
    }

    #[test]
    fn a_noiseless_frame_round_trips() {
        let params = FbmcParams::reference();
        let symbols = 16;
        let sent = points(symbols * params.allocated);
        let mut wave = Vec::new();
        FbmcMod::new(params).modulate(&sent, &mut wave);
        assert_eq!(wave.len(), params.frame_samples(sent.len()));
        let mut got = Vec::new();
        FbmcDemod::new(params).demodulate(&wave, symbols, &mut got);
        assert_eq!(got.len(), sent.len());
        let interior = params.allocated * 4..sent.len() - params.allocated * 4;
        let worst = got[interior.clone()]
            .iter()
            .zip(&sent[interior])
            .map(|(a, b)| f64::from((a - b).norm()))
            .fold(0.0f64, f64::max);
        assert!(worst < 0.02, "worst interior error {worst}");
    }

    #[test]
    fn the_frame_carries_the_energy_of_its_points() {
        let params = FbmcParams::reference();
        let symbols = 32;
        let sent = points(symbols * params.allocated);
        let mut wave = Vec::new();
        FbmcMod::new(params).modulate(&sent, &mut wave);
        let wave_energy: f64 = wave.iter().map(|v| f64::from(v.norm_sqr())).sum();
        let point_energy: f64 = sent.iter().map(|v| f64::from(v.norm_sqr())).sum();
        assert!(
            (wave_energy / point_energy - 1.0).abs() < 0.05,
            "waveform {wave_energy}, points {point_energy}"
        );
    }

    #[test]
    fn without_the_half_symbol_offset_the_bank_is_not_orthogonal() {
        let params = FbmcParams::reference();
        let kernels = params.kernels();
        let inner: Complex<f64> = kernels[10]
            .iter()
            .zip(&kernels[11])
            .map(|(a, b)| {
                Complex::new(f64::from(a.re), f64::from(a.im))
                    * Complex::new(f64::from(b.re), f64::from(b.im)).conj()
            })
            .sum();
        let phase = params.oqam_phase(10, 0) * params.oqam_phase(11, 0).conj();
        let projected = inner * Complex::new(f64::from(phase.re), f64::from(phase.im));
        assert!(inner.norm() > 0.1, "neighbours do not overlap: {inner}");
        assert!(
            projected.re.abs() < 1e-3,
            "the real part is not orthogonal: {}",
            projected.re
        );
    }

    #[test]
    fn an_allocation_the_bank_cannot_carry_is_rejected() {
        for allocated in [0usize, 65] {
            let params = FbmcParams {
                subcarriers: 64,
                allocated,
            };
            let panicked = std::panic::catch_unwind(|| params.prototype()).is_err();
            assert!(panicked, "an allocation of {allocated} was accepted");
        }
    }
}
