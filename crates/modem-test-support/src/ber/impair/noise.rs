use num_complex::Complex;

use super::{Impairment, signal_energy};
use crate::ber::rng::Rng;

#[must_use]
pub fn sigma_for_ebn0(signal_energy: f64, info_bits: u64, ebn0_db: f64) -> f64 {
    debug_assert!(info_bits > 0, "Eb is energy per bit; zero bits has no Eb");
    let eb = signal_energy / info_bits as f64;
    let n0 = eb / 10f64.powf(ebn0_db / 10.0);
    (n0 / 2.0).sqrt()
}

#[must_use]
pub fn sigma_for_channel_snr(mean_power: f64, bandwidth: f64, snr_db: f64) -> f64 {
    assert!(
        bandwidth.is_finite() && bandwidth > 0.0,
        "a channel SNR is stated in a message bandwidth; {bandwidth} is not one"
    );
    (mean_power / (2.0 * 10f64.powf(snr_db / 10.0) * bandwidth)).sqrt()
}

#[derive(Clone, Copy, Debug)]
enum Level {
    Sigma(f64),
    ChannelSnr { snr_db: f64, bandwidth: f64 },
    EbN0 { ebn0_db: f64, info_bits: u64 },
}

#[derive(Clone, Copy, Debug)]
pub struct Awgn {
    level: Level,
}

impl Awgn {
    #[must_use]
    pub fn with_sigma(sigma: f64) -> Self {
        Self {
            level: Level::Sigma(sigma),
        }
    }

    #[must_use]
    pub fn for_ebn0(ebn0_db: f64, info_bits: u64) -> Self {
        Self {
            level: Level::EbN0 { ebn0_db, info_bits },
        }
    }

    #[must_use]
    pub fn for_channel_snr(snr_db: f64, bandwidth: f64) -> Self {
        Self {
            level: Level::ChannelSnr { snr_db, bandwidth },
        }
    }
}

impl Impairment for Awgn {
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng) {
        let sigma = match self.level {
            Level::Sigma(s) => s,
            Level::EbN0 { ebn0_db, info_bits } => {
                sigma_for_ebn0(signal_energy(x), info_bits, ebn0_db)
            }
            Level::ChannelSnr { snr_db, bandwidth } => {
                sigma_for_channel_snr(super::mean_power(x), bandwidth, snr_db)
            }
        };
        for s in x.iter_mut() {
            let (i, q) = rng.normal_pair();
            s.re += (sigma * i) as f32;
            s.im += (sigma * q) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::{Awgn, sigma_for_channel_snr, sigma_for_ebn0};
    use crate::ber::{impair::Impairment, rng::Rng};

    #[test]
    fn added_noise_variance_matches_sigma_squared() {
        let sigma = 0.7;
        let mut x = vec![Complex::new(0.0f32, 0.0); 300_000];
        Awgn::with_sigma(sigma).apply(&mut x, &mut Rng::new(0xa969));
        let n = x.len() as f64;
        let var = x
            .iter()
            .map(|s| f64::from(s.re) * f64::from(s.re) + f64::from(s.im) * f64::from(s.im))
            .sum::<f64>()
            / (2.0 * n);
        let applied = sigma * sigma;
        assert!(
            (var / applied - 1.0).abs() < 0.01,
            "applied σ²={applied}, measured {var}"
        );
    }

    #[test]
    fn ebn0_derivation_on_round_numbers() {
        let sigma = sigma_for_ebn0(1000.0, 1000, 0.0);
        assert!((sigma * sigma - 0.5).abs() < 1e-12);
        let sigma3 = sigma_for_ebn0(1000.0, 1000, 10.0 * 2f64.log10());
        assert!((sigma3 * sigma3 - 0.25).abs() < 1e-12);
    }

    #[test]
    fn channel_snr_derivation_and_applied_level() {
        assert!((sigma_for_channel_snr(1.0, 0.25, 0.0).powi(2) - 2.0).abs() < 1e-12);
        assert!((sigma_for_channel_snr(1.0, 0.25, 10.0).powi(2) - 0.2).abs() < 1e-12);
        let mut x = vec![Complex::new(1.0f32, 0.0); 300_000];
        Awgn::for_channel_snr(6.0, 0.1).apply(&mut x, &mut Rng::new(0xa17));
        let measured = x
            .iter()
            .map(|s| f64::from(s.im) * f64::from(s.im))
            .sum::<f64>()
            / x.len() as f64;
        let applied = sigma_for_channel_snr(1.0, 0.1, 6.0).powi(2);
        assert!(
            (measured / applied - 1.0).abs() < 0.02,
            "applied σ²={applied}, measured {measured}"
        );
    }

    #[test]
    fn ebn0_level_tracks_waveform_energy() {
        let measure = |amp: f32, seed: u64| {
            let mut x = vec![Complex::new(amp, 0.0); 200_000];
            Awgn::for_ebn0(10.0, 200_000).apply(&mut x, &mut Rng::new(seed));
            x.iter()
                .map(|s| f64::from(s.im) * f64::from(s.im))
                .sum::<f64>()
                / x.len() as f64
        };
        let quiet = measure(1.0, 1);
        let loud = measure(2.0, 2);
        let ratio_db = 10.0 * (loud / quiet).log10();
        assert!((ratio_db - 6.02).abs() < 0.1, "ratio {ratio_db} dB");
    }
}
