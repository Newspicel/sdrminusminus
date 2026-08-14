//! Static multipath: a fixed FIR channel built from a named profile ( §4.3 —
//! limits rows are stated *per profile*, so the profiles are types, not ad-hoc tap vectors).
//! Taps are normalised to unit total power, so adding multipath never changes the waveform's
//! mean energy — an Eb/N0 stated before the channel still holds after it, and a multipath
//! limits row measures dispersion, not an accidental level change.

use num_complex::Complex;

use super::Impairment;
use crate::ber::rng::Rng;

/// The named profiles a limits table may cite.
#[derive(Clone, Copy, Debug)]
pub enum MultipathProfile {
    /// One echo: classic specular reflection. `relative_db` is the echo's power relative to
    /// the direct ray (negative for a weaker echo), `phase_rad` its carrier phase.
    TwoRay {
        delay_samples: usize,
        relative_db: f64,
        phase_rad: f64,
    },
    /// Exponentially decaying power-delay profile, the standard dense-scattering shape.
    /// Tap *magnitudes* follow the PDP deterministically; only the phases are drawn — so the
    /// realised per-tap powers are exactly the stated profile (a Rayleigh draw per tap would
    /// make every realisation's PDP a random variable, and the calibration meaningless).
    ExponentialPdp {
        rms_delay_spread_samples: f64,
        taps: usize,
    },
}

/// The FIR channel for a [`MultipathProfile`]. Length is preserved; the convolution tail
/// beyond the waveform end is dropped, as a capture window would drop it.
#[derive(Clone, Copy, Debug)]
pub struct Multipath {
    profile: MultipathProfile,
}

impl Multipath {
    #[must_use]
    pub fn new(profile: MultipathProfile) -> Self {
        Self { profile }
    }

    /// The tap vector one application uses, unit total power. Public because a limits run
    /// wants to record the realisation it measured through; phase draws for the exponential
    /// profile come from `rng`, so taps-then-apply with a shared generator reproduces.
    #[must_use]
    pub fn taps(&self, rng: &mut Rng) -> Vec<Complex<f64>> {
        let mut h = match self.profile {
            MultipathProfile::TwoRay {
                delay_samples,
                relative_db,
                phase_rad,
            } => {
                let mut h = vec![Complex::new(0.0, 0.0); delay_samples + 1];
                h[0] = Complex::new(1.0, 0.0);
                let r = 10f64.powf(relative_db / 20.0);
                let (sin, cos) = phase_rad.sin_cos();
                h[delay_samples] = Complex::new(r * cos, r * sin);
                h
            }
            MultipathProfile::ExponentialPdp {
                rms_delay_spread_samples,
                taps,
            } => (0..taps)
                .map(|k| {
                    let mag = (-(k as f64) / (2.0 * rms_delay_spread_samples)).exp();
                    let phase = std::f64::consts::TAU * rng.uniform();
                    let (sin, cos) = phase.sin_cos();
                    Complex::new(mag * cos, mag * sin)
                })
                .collect(),
        };
        let power: f64 = h.iter().map(|tap| tap.norm_sqr()).sum();
        let scale = 1.0 / power.sqrt();
        for tap in &mut h {
            *tap *= scale;
        }
        h
    }
}

impl Impairment for Multipath {
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng) {
        let h = self.taps(rng);
        let src = x.clone();
        for (n, s) in x.iter_mut().enumerate() {
            let mut acc = Complex::new(0.0f64, 0.0);
            for (k, tap) in h.iter().enumerate() {
                if let Some(v) = n.checked_sub(k).and_then(|i| src.get(i)) {
                    acc += tap * Complex::new(f64::from(v.re), f64::from(v.im));
                }
            }
            *s = Complex::new(acc.re as f32, acc.im as f32);
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::{Multipath, MultipathProfile};
    use crate::ber::{
        impair::{Impairment, mean_power, testutil::white},
        rng::Rng,
    };

    /// Cross-correlation of output against a known white input reads the impulse response
    /// back: `r[k] = E[y[n]·conj(x[n−k])] = h[k]` for unit-power white x. 200k samples put
    /// the estimation noise near 0.003 per tap.
    fn measured_taps(channel: &Multipath, max_lag: usize, seed: u64) -> Vec<Complex<f64>> {
        let x = white(&mut Rng::new(seed), 200_000);
        let mut y = x.clone();
        channel.apply(&mut y, &mut Rng::new(seed ^ 1));
        (0..=max_lag)
            .map(|k| {
                let mut acc = Complex::new(0.0f64, 0.0);
                for n in max_lag..y.len() {
                    let a = Complex::new(f64::from(y[n].re), f64::from(y[n].im));
                    let b = Complex::new(f64::from(x[n - k].re), f64::from(x[n - k].im));
                    acc += a * b.conj();
                }
                acc / (y.len() - max_lag) as f64
            })
            .collect()
    }

    /// Applied == measured for the two-ray profile: the echo shows up at the stated delay,
    /// at the stated relative level and phase, and nowhere else.
    #[test]
    fn two_ray_impulse_response_reads_back() {
        let channel = Multipath::new(MultipathProfile::TwoRay {
            delay_samples: 7,
            relative_db: -6.0,
            phase_rad: 1.0,
        });
        let r = measured_taps(&channel, 12, 0x2a4a);
        let echo = r[7] / r[0];
        let level_db = 20.0 * echo.norm().log10();
        assert!((level_db + 6.0).abs() < 0.3, "echo level {level_db} dB");
        assert!((echo.arg() - 1.0).abs() < 0.05, "echo phase {}", echo.arg());
        for (k, tap) in r.iter().enumerate() {
            if k != 0 && k != 7 {
                assert!(tap.norm() < 0.02, "spurious tap at {k}: {}", tap.norm());
            }
        }
    }

    /// Applied == measured for the exponential PDP: per-tap measured powers follow the
    /// stated profile (deterministic magnitudes make this exact up to estimation noise).
    #[test]
    fn exponential_pdp_tap_powers_read_back() {
        let spread = 2.0;
        let taps = 6;
        let channel = Multipath::new(MultipathProfile::ExponentialPdp {
            rms_delay_spread_samples: spread,
            taps,
        });
        let r = measured_taps(&channel, taps - 1, 0xedf);
        let total: f64 = (0..taps).map(|k| (-(k as f64) / spread).exp()).sum();
        for (k, tap) in r.iter().enumerate() {
            let expected = ((-(k as f64) / spread).exp() / total).sqrt();
            assert!(
                (tap.norm() - expected).abs() < 0.01,
                "tap {k}: expected |h| {expected}, measured {}",
                tap.norm()
            );
        }
    }

    /// Unit-power normalisation: multipath must not change the mean energy, or every Eb/N0
    /// stated upstream of it would silently shift.
    #[test]
    fn normalisation_preserves_mean_power() {
        for profile in [
            MultipathProfile::TwoRay {
                delay_samples: 3,
                relative_db: -3.0,
                phase_rad: 0.4,
            },
            MultipathProfile::ExponentialPdp {
                rms_delay_spread_samples: 3.0,
                taps: 10,
            },
        ] {
            let x = white(&mut Rng::new(0x9099), 200_000);
            let mut y = x.clone();
            Multipath::new(profile).apply(&mut y, &mut Rng::new(7));
            let ratio = mean_power(&y) / mean_power(&x);
            assert!((ratio - 1.0).abs() < 0.02, "power ratio {ratio}");
        }
    }
}
