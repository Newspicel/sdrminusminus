use num_complex::Complex;

use super::params::MfskParams;

#[derive(Clone, Debug)]
pub struct ToneBank {
    m: usize,
    window: usize,
    rotors: Vec<Complex<f32>>,
    scale: f64,
}

impl ToneBank {
    #[must_use]
    pub fn new(params: &MfskParams) -> Self {
        let window = params.window();
        let m = params.m();
        let mut rotors = Vec::with_capacity(m * window);
        for k in 0..m {
            let omega = -std::f64::consts::TAU * params.tone_cycles_per_sample(k);
            for n in 0..window {
                let (sin, cos) = (omega * n as f64).sin_cos();
                rotors.push(Complex::new(cos as f32, sin as f32));
            }
        }
        Self {
            m,
            window,
            rotors,
            scale: 1.0 / window as f64,
        }
    }

    #[must_use]
    pub fn m(&self) -> usize {
        self.m
    }

    #[must_use]
    pub fn window(&self) -> usize {
        self.window
    }

    pub fn energies(&self, samples: &[Complex<f32>], at: usize, out: &mut [f32]) {
        assert_eq!(out.len(), self.m, "one energy per tone");
        for (k, slot) in out.iter_mut().enumerate() {
            let rotors = &self.rotors[k * self.window..(k + 1) * self.window];
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (n, rotor) in rotors.iter().enumerate() {
                let Some(x) = samples.get(at + n) else { break };
                let product = x * rotor;
                re += f64::from(product.re);
                im += f64::from(product.im);
            }
            *slot = ((re * re + im * im) * self.scale) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::rng::Rng;

    use super::{super::modulator::MfskMod, *};
    use crate::orthogonal::modulator::TonePhase;

    fn tone(params: &MfskParams, index: u8, symbols: usize) -> Vec<Complex<f32>> {
        let mut m = MfskMod::new(params.clone(), TonePhase::Continuous);
        let mut out = Vec::new();
        m.modulate(&vec![index; symbols], &mut out);
        out
    }

    #[test]
    fn each_tone_lights_its_own_bin_and_no_other() {
        let params = MfskParams::orthogonal(8, 16.0);
        let bank = ToneBank::new(&params);
        let mut energies = vec![0.0f32; 8];
        for k in 0..8u8 {
            let wave = tone(&params, k, 4);
            bank.energies(&wave, 0, &mut energies);
            assert!(
                (energies[k as usize] - 16.0).abs() < 1e-3,
                "tone {k}: {energies:?}"
            );
            for (j, &e) in energies.iter().enumerate() {
                if j != k as usize {
                    assert!(e < 1e-6, "tone {k} leaked into bin {j}: {energies:?}");
                }
            }
        }
    }

    #[test]
    fn a_noise_only_bin_reads_mean_n0() {
        for sps in [8.0, 32.0, 128.0] {
            let params = MfskParams::orthogonal(4, sps);
            let bank = ToneBank::new(&params);
            let mut rng = Rng::new(0x0f5c);
            let sigma = (0.5f64).sqrt();
            let symbols = 2_000;
            let noise: Vec<Complex<f32>> = (0..symbols * params.window())
                .map(|_| Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32))
                .collect();
            let mut energies = vec![0.0f32; 4];
            let mut sum = 0.0f64;
            for s in 0..symbols {
                bank.energies(&noise, s * params.window(), &mut energies);
                sum += energies.iter().map(|&e| f64::from(e)).sum::<f64>();
            }
            let mean = sum / (symbols * 4) as f64;
            assert!(
                (mean - 1.0).abs() < 0.05,
                "sps {sps}: mean {mean} vs N0 = 1"
            );
        }
    }

    #[test]
    fn a_misaligned_window_loses_energy() {
        let params = MfskParams::orthogonal(4, 16.0);
        let bank = ToneBank::new(&params);
        let wave = tone(&params, 2, 4);
        let mut aligned = vec![0.0f32; 4];
        let mut offset = vec![0.0f32; 4];
        bank.energies(&wave, 16, &mut aligned);
        bank.energies(&wave, 24, &mut offset);
        assert!((aligned[2] - 16.0).abs() < 1e-3, "{aligned:?}");
        assert!(offset[2] <= aligned[2] + 1e-3, "{offset:?} vs {aligned:?}");
    }
}
